//! Detached blink daemon: `blink` spawns the current binary with `_daemon`,
//! records its pid, and any later `on`/`off` kills it first.
//!
//! The pidfile holds `<pid> <restore>`, where `restore` (0/1) is the light
//! state the daemon returns to when the user acknowledges the alert: the state
//! from before blinking started, except that caps lock the agent turned on
//! counts as off, so the user doesn't keep typing in capitals.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::led;

/// Accepted range for the blink interval, in seconds.
pub const MIN_INTERVAL: f64 = 0.05;
pub const MAX_INTERVAL: f64 = 60.0;

/// How often the daemon checks for user input while blinking.
const INPUT_POLL: Duration = Duration::from_millis(20);

/// Longest a blink keeps caps lock on, in seconds. A key pressed in this window
/// comes out capitalized, because input is only visible once it has happened.
const MAX_CAPS_LIT: f64 = 0.1;

fn pidfile() -> PathBuf {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    // Prefer the per-user runtime dir over a shared /tmp on Linux.
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("capslkagent-{user}.pid"))
}

fn read_pidfile() -> Option<(u32, bool)> {
    let contents = fs::read_to_string(pidfile()).ok()?;
    let mut parts = contents.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let restore = parts.next() == Some("1");
    Some((pid, restore))
}

/// Exists while caps lock is on only because `on` turned it on.
fn agent_lit_file() -> PathBuf {
    pidfile().with_extension("agent-lit")
}

/// Record that the agent, not the user, turned caps lock on.
pub fn mark_agent_lit() {
    let _ = fs::write(agent_lit_file(), "");
}

/// Forget that record. Returns whether there was one.
pub fn clear_agent_lit() -> bool {
    fs::remove_file(agent_lit_file()).is_ok()
}

/// Kill a running blink daemon, if any. Returns the light state it would have
/// restored, if one was stopped.
pub fn stop() -> Option<bool> {
    let (pid, restore) = read_pidfile()?;
    let _ = fs::remove_file(pidfile());
    kill(pid).then_some(restore)
}

/// Whether a blink daemon is currently running.
pub fn running() -> bool {
    read_pidfile().is_some_and(|(pid, _)| alive(pid))
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
fn kill(pid: u32) -> bool {
    if unsafe { libc_kill(pid as i32, 15) } != 0 {
        return false;
    }
    // Wait briefly for it to go away, so a toggle it was in the middle of
    // can't land after the caller sets the final state.
    for _ in 0..50 {
        if !alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

// The daemon deletes its pidfile on exit, so an existing one is trusted.
#[cfg(windows)]
fn alive(_pid: u32) -> bool {
    true
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
    let agent_lit = clear_agent_lit();
    // If we're replacing a running daemon, keep its restore state: the light
    // is mid-blink now, so reading it would be meaningless.
    let restore = match stop() {
        Some(restore) => restore,
        None => led::backend(target)?.get().unwrap_or(false) && !agent_lit,
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("_daemon")
        .arg(interval.to_string())
        .arg(target.as_str())
        .arg(if restore { "1" } else { "0" })
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
    fs::write(pidfile(), format!("{} {}", child.id(), restore as u8))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Lit and dark durations of one blink cycle. A target that changes what the
/// user types stays lit only briefly, within the same overall period.
fn phases(interval: f64, target: led::Target) -> (Duration, Duration) {
    let lit = if target.shifts_typing() {
        interval.min(MAX_CAPS_LIT)
    } else {
        interval
    };
    (
        Duration::from_secs_f64(lit),
        Duration::from_secs_f64(2.0 * interval - lit),
    )
}

/// Remove the pidfile if it still points at this process (a newer `blink`
/// may already have replaced it).
fn release_pidfile() {
    if read_pidfile().is_some_and(|(pid, _)| pid == std::process::id()) {
        let _ = fs::remove_file(pidfile());
    }
}

/// The daemon loop itself (runs in the spawned process).
///
/// Blinks until the user touches an input device, then restores the light and
/// exits — pressing any key (or moving the mouse)
/// acknowledges the alert. We detect this via the system's HID idle time: it
/// climbs steadily while idle and drops to ~0 on any input.
///
/// The baseline is sampled right *after* each LED change, so even a backend
/// whose toggling counts as input (Windows' synthesized key presses) only
/// resets the timer before the measurement window, not inside it.
pub fn run(interval: f64, target: led::Target, restore: bool) -> ! {
    let Ok(backend) = led::backend(target) else {
        release_pidfile();
        std::process::exit(1);
    };
    let (lit, dark) = phases(interval.clamp(MIN_INTERVAL, MAX_INTERVAL), target);
    let mut state = true;
    loop {
        let _ = backend.set(state);
        let phase = if state { lit } else { dark };
        state = !state;
        let Some(prev) = system_idle_millis() else {
            std::thread::sleep(phase);
            continue;
        };
        let started = Instant::now();
        while started.elapsed() < phase {
            std::thread::sleep(INPUT_POLL.min(phase.saturating_sub(started.elapsed())));
            let Some(cur) = system_idle_millis() else {
                continue;
            };
            // Without input, idle grows by exactly the elapsed time; allow a
            // little slack for timer granularity.
            let elapsed = started.elapsed().as_millis() as u64;
            if cur + 100 < prev + elapsed {
                let _ = backend.set(restore);
                release_pidfile();
                std::process::exit(0);
            }
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
    }

    #[link(name = "kernel32")]
    extern "system" {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_lock_pulses_briefly_within_the_same_period() {
        let (lit, dark) = phases(0.4, led::Target::CapsLock);
        assert_eq!(lit + dark, Duration::from_millis(800));
        if led::Target::CapsLock.shifts_typing() {
            assert_eq!(lit, Duration::from_millis(100));
        }
    }

    #[test]
    fn backlight_blinks_evenly() {
        assert_eq!(
            phases(0.4, led::Target::Backlight),
            (Duration::from_millis(400), Duration::from_millis(400))
        );
    }
}
