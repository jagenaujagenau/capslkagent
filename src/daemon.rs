//! Detached blink daemon: `blink` spawns the current binary with `_daemon`,
//! records its pid, and any later `on`/`off` kills it first.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::led;

fn pidfile() -> PathBuf {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    std::env::temp_dir().join(format!("capslkagent-{user}.pid"))
}

/// Kill a running blink daemon, if any. Returns true if one was stopped.
pub fn stop() -> bool {
    let pf = pidfile();
    let Ok(contents) = fs::read_to_string(&pf) else {
        return false;
    };
    let _ = fs::remove_file(&pf);
    let Ok(pid) = contents.trim().parse::<u32>() else {
        return false;
    };
    kill(pid)
}

#[cfg(unix)]
fn kill(pid: u32) -> bool {
    // SIGTERM; the daemon's signal handling is simple LED-off-on-exit via
    // default termination — we turn the LED off ourselves after stopping.
    unsafe { libc_kill(pid as i32, 15) == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(windows)]
fn kill(pid: u32) -> bool {
    Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn start(interval: f64, target: led::Target) -> Result<(), String> {
    stop();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("_daemon")
        .arg(interval.to_string())
        .arg(target.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    fs::write(pidfile(), child.id().to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

/// The daemon loop itself (runs in the spawned process).
///
/// Blinks until the user touches an input device, then turns the light off and
/// exits — pressing any key (or moving the mouse) acknowledges the alert. We
/// detect this via the system's HID idle time: it climbs monotonically while
/// idle and drops to ~0 on any input. Our own LED toggling does not reset it
/// (verified on macOS), so a decrease means real user activity.
pub fn run(interval: f64, target: led::Target) -> ! {
    let backend = match led::backend(target) {
        Ok(b) => b,
        Err(_) => std::process::exit(1),
    };
    let mut state = true;
    let mut prev_idle = system_idle_millis();
    loop {
        let _ = backend.set(state);
        state = !state;
        std::thread::sleep(Duration::from_secs_f64(interval));

        if let Some(cur) = system_idle_millis() {
            // Normally idle grows by ~interval each tick; a drop of more than
            // half the interval means an input event reset the timer.
            let margin = ((interval * 1000.0) / 2.0) as u64 + 50;
            if prev_idle.is_some_and(|prev| cur + margin < prev) {
                let _ = backend.set(false);
                std::process::exit(0);
            }
            prev_idle = Some(cur);
        }
    }
}

/// Milliseconds since the last user input (keyboard or mouse), system-wide.
/// `None` if the platform can't report it (blinking then continues until the
/// next `on`/`off`).
#[cfg(target_os = "macos")]
fn system_idle_millis() -> Option<u64> {
    use std::ffi::{c_char, c_void};

    const KCF_NUMBER_SINT64: i64 = 4; // kCFNumberSInt64Type
    const KCF_STRING_UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            cstr: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFNumberGetValue(number: *const c_void, the_type: i64, value: *mut c_void) -> bool;
        fn CFRelease(cf: *const c_void);
    }

    unsafe {
        let service = IOServiceGetMatchingService(0, IOServiceMatching(c"IOHIDSystem".as_ptr()));
        if service == 0 {
            return None;
        }
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            c"HIDIdleTime".as_ptr(),
            KCF_STRING_UTF8,
        );
        let prop = IORegistryEntryCreateCFProperty(service, key, std::ptr::null(), 0);
        IOObjectRelease(service);
        if !key.is_null() {
            CFRelease(key);
        }
        if prop.is_null() {
            return None;
        }
        let mut nanos: i64 = 0;
        let ok = CFNumberGetValue(prop, KCF_NUMBER_SINT64, &mut nanos as *mut i64 as *mut c_void);
        CFRelease(prop);
        if ok && nanos >= 0 {
            Some(nanos as u64 / 1_000_000)
        } else {
            None
        }
    }
}

#[cfg(target_os = "windows")]
fn system_idle_millis() -> Option<u64> {
    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(pli: *mut LastInputInfo) -> i32;
        fn GetTickCount() -> u32;
    }

    unsafe {
        let mut info = LastInputInfo {
            cb_size: std::mem::size_of::<LastInputInfo>() as u32,
            dw_time: 0,
        };
        if GetLastInputInfo(&mut info) == 0 {
            return None;
        }
        // Both are millisecond tick counts; wrapping handles the 49.7-day roll.
        Some(GetTickCount().wrapping_sub(info.dw_time) as u64)
    }
}

// Linux: no permission-free system-wide idle source without X11/Wayland
// specifics, so auto-stop-on-keypress isn't wired up there yet. Blinking stops
// on the next `on`/`off` (e.g. Claude Code's UserPromptSubmit hook).
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_idle_millis() -> Option<u64> {
    None
}
