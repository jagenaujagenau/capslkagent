//! capslkagent: drive a keyboard light as a Claude Code status light.
//!
//!   on     - agent is working (solid light)
//!   blink  - agent needs your attention (flashing light)
//!   off    - agent is idle (light off / restored)

mod daemon;
mod hooks;
mod led;

use led::Target;

const USAGE: &str = "\
capslkagent - Claude Code agent status via a keyboard light

usage: capslkagent [--target capslock|backlight] <command>

commands:
  on                  solid light (agent working)
  off                 light off (agent idle)
  blink [interval]    flash the light (agent needs attention); default 0.4s
  toggle              flip the light
  status              print current light state
  install-hooks       wire into Claude Code (~/.claude/settings.json)
  uninstall-hooks     remove the Claude Code hooks

targets:
  capslock            the Caps Lock LED (default; macOS, Linux, Windows)
  backlight           the keyboard backlight (macOS laptops; --backlight
                      is a shorthand). `off` restores your previous
                      brightness.
";

fn parse_target(args: &mut Vec<String>) -> Result<Target, String> {
    let mut target = Target::CapsLock;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backlight" => {
                target = Target::Backlight;
                args.remove(i);
            }
            "--target" | "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or("--target requires a value: capslock|backlight")?;
                target = Target::parse(value)
                    .ok_or_else(|| format!("Unknown target: {value}"))?;
                args.drain(i..i + 2);
            }
            _ => i += 1,
        }
    }
    Ok(target)
}

fn parse_interval(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if (daemon::MIN_INTERVAL..=daemon::MAX_INTERVAL).contains(&v) => Ok(v),
        _ => Err(format!(
            "Invalid interval: {s} (expected seconds between {} and {})",
            daemon::MIN_INTERVAL,
            daemon::MAX_INTERVAL
        )),
    }
}

fn run() -> Result<i32, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let target = parse_target(&mut args)?;
    let cmd = args.first().map(String::as_str).unwrap_or("help");

    match cmd {
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(0)
        }
        "_daemon" => {
            let interval: f64 = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.4);
            let target = args
                .get(2)
                .and_then(|s| Target::parse(s))
                .unwrap_or(Target::CapsLock);
            let restore = args.get(3).is_some_and(|s| s == "1");
            daemon::run(interval, target, restore)
        }
        "blink" => {
            let interval: f64 = args
                .get(1)
                .map(|s| parse_interval(s))
                .transpose()?
                .unwrap_or(0.4);
            daemon::start(interval, target)?;
            Ok(0)
        }
        "install-hooks" => {
            let path = hooks::install(args.get(1).map(String::as_str), target)?;
            println!("Hooks installed in {}", path.display());
            println!("Restart Claude Code sessions to pick them up.");
            Ok(0)
        }
        "uninstall-hooks" => {
            let path = hooks::uninstall(args.get(1).map(String::as_str))?;
            println!("Hooks removed from {}", path.display());
            Ok(0)
        }
        "status" => {
            // Read-only: don't disturb a running blink.
            if daemon::running() {
                println!("blinking");
                return Ok(0);
            }
            match led::backend(target)?.get() {
                Some(true) => println!("on"),
                Some(false) => println!("off"),
                None => println!("unknown"),
            }
            Ok(0)
        }
        "on" | "off" | "toggle" => {
            let restore = daemon::stop();
            let backend = led::backend(target)?;
            match cmd {
                "toggle" => {
                    // Mid-blink the LED state is arbitrary; toggle relative to
                    // the state blinking would have returned to.
                    let cur = restore.or_else(|| backend.get()).unwrap_or(false);
                    backend.set(!cur)?;
                }
                _ => backend.set(cmd == "on")?,
            }
            Ok(0)
        }
        other => {
            eprintln!("Unknown command: {other}\n\n{USAGE}");
            Ok(2)
        }
    }
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
