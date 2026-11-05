//! Platform backends for driving the Caps Lock LED.
//!
//! macOS   - IOKit IOHIDSetModifierLockState (links the IOKit framework)
//! Linux   - /sys/class/leds/*capslock*/brightness, `xset led` fallback
//! Windows - user32 keybd_event(VK_CAPITAL)

pub trait Led {
    /// Current LED state, if it can be read.
    fn get(&self) -> Option<bool>;
    fn set(&self, state: bool) -> Result<(), String>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    CapsLock,
    /// Keyboard backlight (macOS laptops only).
    Backlight,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::CapsLock => "capslock",
            Target::Backlight => "backlight",
        }
    }

    pub fn parse(s: &str) -> Option<Target> {
        match s {
            "capslock" => Some(Target::CapsLock),
            "backlight" => Some(Target::Backlight),
            _ => None,
        }
    }

    /// Whether lighting this target also makes typing produce capitals.
    /// Linux's LED is independent of the typing state.
    pub fn shifts_typing(self) -> bool {
        self == Target::CapsLock && !cfg!(target_os = "linux")
    }
}

pub fn backend(target: Target) -> Result<Box<dyn Led>, String> {
    match target {
        Target::CapsLock => imp::new().map(|b| Box::new(b) as Box<dyn Led>),
        Target::Backlight => {
            #[cfg(target_os = "macos")]
            {
                backlight::new().map(|b| Box::new(b) as Box<dyn Led>)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err("The backlight target is only supported on macOS".into())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// macOS keyboard backlight via the private CoreBrightness framework.
//
// KeyboardBrightnessClient (ObjC) is reached through dlopen + the ObjC
// runtime, so nothing private is linked at build time. `set(true)` saves the
// user's current brightness to a temp file and goes to full; `set(false)`
// restores it.
#[cfg(target_os = "macos")]
mod backlight {
    use super::Led;
    use std::ffi::{c_char, c_void, CStr};
    use std::fs;
    use std::path::PathBuf;

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
    }

    extern "C" {
        fn dlopen(path: *const c_char, flag: i32) -> *mut c_void;
    }

    const RTLD_LAZY: i32 = 0x1;
    const FRAMEWORK: &CStr =
        c"/System/Library/PrivateFrameworks/CoreBrightness.framework/CoreBrightness";

    fn sel(name: &CStr) -> Sel {
        unsafe { sel_registerName(name.as_ptr()) }
    }

    macro_rules! msg {
        ($ret:ty, $recv:expr, $sel:expr $(, $argty:ty : $arg:expr)*) => {{
            let f: unsafe extern "C" fn(Id, Sel $(, $argty)*) -> $ret =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f($recv, sel($sel) $(, $arg)*)
        }};
    }

    pub struct Backend {
        client: Id,
        keyboards: Vec<u64>,
    }

    fn saved_brightness_file() -> PathBuf {
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
        std::env::temp_dir().join(format!("capslkagent-backlight-{user}.sav"))
    }

    pub fn new() -> Result<Backend, String> {
        unsafe {
            if dlopen(FRAMEWORK.as_ptr(), RTLD_LAZY).is_null() {
                return Err("Cannot load CoreBrightness.framework".into());
            }
            let cls = objc_getClass(c"KeyboardBrightnessClient".as_ptr());
            if cls.is_null() {
                return Err("KeyboardBrightnessClient class not found".into());
            }
            let obj = msg!(Id, cls, c"alloc");
            let client = msg!(Id, obj, c"init");
            if client.is_null() {
                return Err("KeyboardBrightnessClient init failed".into());
            }
            let ids = msg!(Id, client, c"copyKeyboardBacklightIDs");
            let mut keyboards = Vec::new();
            if !ids.is_null() {
                let count = msg!(usize, ids, c"count");
                for i in 0..count {
                    let num = msg!(Id, ids, c"objectAtIndex:", usize: i);
                    if !num.is_null() {
                        keyboards.push(msg!(u64, num, c"unsignedLongLongValue"));
                    }
                }
            }
            if keyboards.is_empty() {
                return Err("No backlit keyboard found on this Mac".into());
            }
            Ok(Backend { client, keyboards })
        }
    }

    impl Backend {
        fn brightness(&self) -> f32 {
            unsafe { msg!(f32, self.client, c"brightnessForKeyboard:", u64: self.keyboards[0]) }
        }

        fn set_brightness(&self, value: f32) -> Result<(), String> {
            for &kid in &self.keyboards {
                let ok = unsafe {
                    msg!(bool, self.client, c"setBrightness:forKeyboard:",
                         f32: value, u64: kid)
                };
                if !ok {
                    return Err("setBrightness:forKeyboard: failed".into());
                }
            }
            Ok(())
        }
    }

    impl Led for Backend {
        /// "On" means we've raised the backlight and hold the user's level
        /// to restore, not merely that the backlight is lit.
        fn get(&self) -> Option<bool> {
            Some(saved_brightness_file().exists())
        }

        fn set(&self, state: bool) -> Result<(), String> {
            let sav = saved_brightness_file();
            if state {
                if !sav.exists() {
                    fs::write(&sav, self.brightness().to_string())
                        .map_err(|e| format!("Cannot save current brightness: {e}"))?;
                }
                self.set_brightness(1.0)
            } else {
                // Nothing saved means we're already off; don't clobber the
                // user's brightness (e.g. `Stop` followed by `SessionEnd`).
                let Some(original) = fs::read_to_string(&sav)
                    .ok()
                    .and_then(|s| s.trim().parse::<f32>().ok())
                else {
                    let _ = fs::remove_file(&sav);
                    return Ok(());
                };
                self.set_brightness(original.clamp(0.0, 1.0))?;
                let _ = fs::remove_file(&sav);
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod imp {
    use super::Led;
    use std::ffi::c_char;

    type KernReturn = i32;
    type MachPort = u32;

    const KIO_HID_PARAM_CONNECT_TYPE: u32 = 1; // kIOHIDParamConnectType
    const KIO_HID_CAPS_LOCK_STATE: i32 = 1; // kIOHIDCapsLockState

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut core::ffi::c_void;
        fn IOServiceGetMatchingService(
            main_port: MachPort,
            matching: *mut core::ffi::c_void,
        ) -> MachPort;
        fn IOServiceOpen(
            service: MachPort,
            owning_task: MachPort,
            conn_type: u32,
            connect: *mut MachPort,
        ) -> KernReturn;
        fn IOServiceClose(connect: MachPort) -> KernReturn;
        fn IOObjectRelease(object: MachPort) -> KernReturn;
        fn IOHIDGetModifierLockState(
            handle: MachPort,
            selector: i32,
            state: *mut bool,
        ) -> KernReturn;
        fn IOHIDSetModifierLockState(
            handle: MachPort,
            selector: i32,
            state: bool,
        ) -> KernReturn;
    }

    extern "C" {
        static mach_task_self_: MachPort;
    }

    pub struct Backend;

    pub fn new() -> Result<Backend, String> {
        Ok(Backend)
    }

    fn with_connection<T>(f: impl FnOnce(MachPort) -> Result<T, String>) -> Result<T, String> {
        unsafe {
            let matching = IOServiceMatching(c"IOHIDSystem".as_ptr());
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return Err("IOHIDSystem service not found".into());
            }
            let mut conn: MachPort = 0;
            let kr = IOServiceOpen(
                service,
                mach_task_self_,
                KIO_HID_PARAM_CONNECT_TYPE,
                &mut conn,
            );
            IOObjectRelease(service);
            if kr != 0 {
                return Err(format!("IOServiceOpen failed: {kr:#x}"));
            }
            let result = f(conn);
            IOServiceClose(conn);
            result
        }
    }

    impl Led for Backend {
        fn get(&self) -> Option<bool> {
            with_connection(|conn| unsafe {
                let mut state = false;
                let kr = IOHIDGetModifierLockState(conn, KIO_HID_CAPS_LOCK_STATE, &mut state);
                if kr != 0 {
                    return Err(format!("get failed: {kr:#x}"));
                }
                Ok(state)
            })
            .ok()
        }

        fn set(&self, state: bool) -> Result<(), String> {
            with_connection(|conn| unsafe {
                let kr = IOHIDSetModifierLockState(conn, KIO_HID_CAPS_LOCK_STATE, state);
                if kr != 0 {
                    return Err(format!("IOHIDSetModifierLockState failed: {kr:#x}"));
                }
                Ok(())
            })
        }
    }
}

// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod imp {
    use super::Led;
    use std::fs;
    use std::process::Command;

    pub struct Backend {
        led_files: Vec<std::path::PathBuf>,
        use_xset: bool,
    }

    pub fn new() -> Result<Backend, String> {
        let mut led_files = Vec::new();
        if let Ok(entries) = fs::read_dir("/sys/class/leds") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("capslock") {
                    let p = entry.path().join("brightness");
                    // Writable check: try opening for append-less write.
                    if fs::OpenOptions::new().write(true).open(&p).is_ok() {
                        led_files.push(p);
                    }
                }
            }
        }
        let use_xset = std::env::var("DISPLAY").is_ok()
            && Command::new("xset").arg("q").output().is_ok();
        if led_files.is_empty() && !use_xset {
            return Err(
                "No writable caps-lock LED found.\n\
                 Grant access with a udev rule (see README) or run under X11 \
                 where `xset led` works."
                    .into(),
            );
        }
        Ok(Backend { led_files, use_xset })
    }

    impl Led for Backend {
        fn get(&self) -> Option<bool> {
            for p in &self.led_files {
                if let Ok(s) = fs::read_to_string(p) {
                    return Some(s.trim() != "0");
                }
            }
            None
        }

        fn set(&self, state: bool) -> Result<(), String> {
            let mut wrote = false;
            for p in &self.led_files {
                if fs::write(p, if state { "1" } else { "0" }).is_ok() {
                    wrote = true;
                }
            }
            if !wrote && self.use_xset {
                let arg = if state { "led" } else { "-led" };
                let out = Command::new("xset")
                    .args([arg, "named", "Caps Lock"])
                    .output()
                    .map_err(|e| format!("xset failed: {e}"))?;
                if !out.status.success() {
                    return Err("xset led failed".into());
                }
                wrote = true;
            }
            if wrote {
                Ok(())
            } else {
                Err("Failed to write any caps-lock LED".into())
            }
        }
    }
}

// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod imp {
    use super::Led;

    const VK_CAPITAL: i32 = 0x14;
    const KEYEVENTF_KEYUP: u32 = 0x02;

    #[link(name = "user32")]
    extern "system" {
        fn GetKeyState(n_virt_key: i32) -> i16;
        fn keybd_event(b_vk: u8, b_scan: u8, dw_flags: u32, dw_extra_info: usize);
    }

    pub struct Backend;

    pub fn new() -> Result<Backend, String> {
        Ok(Backend)
    }

    impl Led for Backend {
        fn get(&self) -> Option<bool> {
            unsafe { Some(GetKeyState(VK_CAPITAL) & 1 != 0) }
        }

        fn set(&self, state: bool) -> Result<(), String> {
            if self.get() != Some(state) {
                unsafe {
                    keybd_event(VK_CAPITAL as u8, 0x45, 0, 0);
                    keybd_event(VK_CAPITAL as u8, 0x45, KEYEVENTF_KEYUP, 0);
                }
            }
            Ok(())
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod imp {
    use super::Led;
    pub struct Backend;
    pub fn new() -> Result<Backend, String> {
        Err(format!("Unsupported platform: {}", std::env::consts::OS))
    }
    impl Led for Backend {
        fn get(&self) -> Option<bool> {
            None
        }
        fn set(&self, _state: bool) -> Result<(), String> {
            unreachable!()
        }
    }
}
