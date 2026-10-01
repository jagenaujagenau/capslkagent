//! capslkagent: drive a keyboard light as a coding-agent status light.
//!
//!   on     - agent is working (solid light)
//!   blink  - agent needs your attention (flashing light)
//!   off    - agent is idle (light off / restored)

mod agents;
mod daemon;
mod hooks;
mod led;

use agents::Agent;
use led::Target;
use std::path::Path;

const USAGE: &str = "\
capslkagent - coding agent status via a keyboard light

usage: capslkagent [--target capslock|backlight] <command>

commands:
  on                  solid light (agent working)
  off                 light off (agent idle)
  blink [interval]    flash the light (agent needs attention); default 0.4s
  toggle              flip the light
  status              print current light state
  install-hooks [--agent NAME] [path]
                      wire into a coding agent (default: claude)
  uninstall-hooks [--agent NAME] [path]
                      remove that integration

agents (--agent):
  claude              Claude Code hooks (~/.claude/settings.json)
  codex               Codex hooks (~/.codex/hooks.json)
  pi                  pi extension (~/.pi/agent/extensions/)
  omp                 oh-my-pi extension (~/.omp/agent/extensions/)
  amp                 Amp plugin (~/.config/amp/plugins/)
  opencode            opencode plugin (~/.config/opencode/plugins/)
  all                 every agent above that is set up on this machine

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

/// Pull `--agent NAME` out of `args`. `None` means not given.
fn parse_agents(args: &mut Vec<String>) -> Result<Option<Vec<Agent>>, String> {
    let Some(i) = args.iter().position(|a| a == "--agent" || a == "-a") else {
        return Ok(None);
    };
    let value = args
        .get(i + 1)
        .ok_or("--agent requires a value (claude, codex, pi, omp, amp, opencode, all)")?
        .clone();
    args.drain(i..i + 2);
    if value == "all" {
        return Ok(Some(agents::ALL.iter().copied().filter(|a| a.detected()).collect()));
    }
    let agent = Agent::parse(&value).ok_or_else(|| format!("Unknown agent: {value}"))?;
    Ok(Some(vec![agent]))
}

/// install-hooks / uninstall-hooks for one or more agents.
fn integrate(args: &mut Vec<String>, install: bool, target: Target) -> Result<i32, String> {
    let selected = parse_agents(args)?;
    let selected = selected.unwrap_or_else(|| vec![Agent::Claude]);
    let path = args.get(1).map(Path::new);
    if path.is_some() && selected.len() != 1 {
        return Err("A custom path needs a single --agent".into());
    }
    if selected.is_empty() {
        println!("No supported coding agents found on this machine.");
        return Ok(0);
    }
    let mut failed = false;
    for agent in selected {
        let result = if install {
            agents::install(agent, path, target)
        } else {
            agents::uninstall(agent, path)
        };
        let name = agent.display_name();
        match result {
            Ok(p) if install => {
                println!("{name}: installed in {}", p.display());
                println!("  {}", agent.next_step());
            }
            Ok(p) => println!("{name}: removed from {}", p.display()),
            Err(e) => {
                eprintln!("{name}: error: {e}");
                failed = true;
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
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
        "install-hooks" => integrate(&mut args, true, target),
        "uninstall-hooks" => integrate(&mut args, false, target),
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
