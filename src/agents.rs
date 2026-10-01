//! Per-agent integration: where each coding agent keeps its hooks or plugins,
//! and how `install-hooks` / `uninstall-hooks` wire capslkagent into it.
//!
//! Claude Code and Codex run shell-command hooks from a JSON file. pi, omp,
//! Amp and opencode load JS/TS plugins from a directory, so for those we write
//! one small managed plugin file that spawns this binary.

use std::fs;
use std::path::{Path, PathBuf};

use crate::hooks;
use crate::led::Target;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agent {
    Claude,
    Codex,
    Pi,
    Omp,
    Amp,
    OpenCode,
}

pub const ALL: &[Agent] = &[
    Agent::Claude,
    Agent::Codex,
    Agent::Pi,
    Agent::Omp,
    Agent::Amp,
    Agent::OpenCode,
];

const CLAUDE_EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "on"), // agent starts working
    ("Stop", "off"),            // agent finished / idle
    ("Notification", "blink"),  // agent needs attention
    ("SessionEnd", "off"),      // clean up on exit
];

const CODEX_EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "on"),
    ("Stop", "off"),
    ("PermissionRequest", "blink"), // approval prompt
    ("Interrupt", "off"),           // user cancelled the turn
    ("SessionEnd", "off"),
];

/// First line of every plugin file we write; uninstall only deletes files
/// that carry it.
const MARKER: &str = "// capslkagent: managed plugin";

const COMMON_JS: &str = include_str!("plugins/common.js");
const PI_JS: &str = include_str!("plugins/pi.js");
const AMP_JS: &str = include_str!("plugins/amp.js");
const OPENCODE_JS: &str = include_str!("plugins/opencode.js");

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Pi => "pi",
            Agent::Omp => "omp",
            Agent::Amp => "amp",
            Agent::OpenCode => "opencode",
        }
    }

    pub fn parse(s: &str) -> Option<Agent> {
        ALL.iter().copied().find(|a| a.as_str() == s)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Pi => "pi",
            Agent::Omp => "omp",
            Agent::Amp => "Amp",
            Agent::OpenCode => "opencode",
        }
    }

    /// The agent's own config directory; its existence means "installed".
    fn config_dir(self) -> Result<PathBuf, String> {
        Ok(match self {
            Agent::Claude => env_dir("CLAUDE_CONFIG_DIR").unwrap_or(home()?.join(".claude")),
            Agent::Codex => env_dir("CODEX_HOME").unwrap_or(home()?.join(".codex")),
            Agent::Pi => env_dir("PI_CODING_AGENT_DIR").unwrap_or(home()?.join(".pi").join("agent")),
            Agent::Omp => home()?.join(".omp").join("agent"),
            Agent::Amp => xdg_config()?.join("amp"),
            Agent::OpenCode => xdg_config()?.join("opencode"),
        })
    }

    /// The file `install-hooks` writes to (settings JSON or plugin file).
    pub fn default_path(self) -> Result<PathBuf, String> {
        let dir = self.config_dir()?;
        Ok(match self {
            Agent::Claude => dir.join("settings.json"),
            Agent::Codex => dir.join("hooks.json"),
            Agent::Pi | Agent::Omp => dir.join("extensions").join("capslkagent.ts"),
            Agent::Amp => dir.join("plugins").join("capslkagent.ts"),
            Agent::OpenCode => dir.join("plugins").join("capslkagent.js"),
        })
    }

    pub fn detected(self) -> bool {
        self.config_dir().is_ok_and(|d| d.is_dir())
    }

    fn json_events(self) -> Option<&'static [(&'static str, &'static str)]> {
        match self {
            Agent::Claude => Some(CLAUDE_EVENTS),
            Agent::Codex => Some(CODEX_EVENTS),
            _ => None,
        }
    }

    fn plugin_body(self) -> &'static str {
        match self {
            Agent::Pi | Agent::Omp => PI_JS,
            Agent::Amp => AMP_JS,
            Agent::OpenCode => OPENCODE_JS,
            Agent::Claude | Agent::Codex => unreachable!("JSON-hook agent"),
        }
    }

    /// What the user has to do after installing for the change to apply.
    pub fn next_step(self) -> &'static str {
        match self {
            Agent::Claude => "Restart Claude Code sessions to pick them up.",
            Agent::Codex => "Run /hooks in Codex to review and trust the new hooks.",
            Agent::Pi | Agent::Omp => "Restart the agent (or /reload) to load the extension.",
            Agent::Amp => "Restart Amp or run `plugins: reload` from the command palette.",
            Agent::OpenCode => "Restart opencode to load the plugin.",
        }
    }
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn home() -> Result<PathBuf, String> {
    env_dir("HOME")
        .or_else(|| env_dir("USERPROFILE"))
        .ok_or_else(|| "Cannot locate home directory".to_string())
}

fn xdg_config() -> Result<PathBuf, String> {
    match env_dir("XDG_CONFIG_HOME") {
        Some(d) => Ok(d),
        None => Ok(home()?.join(".config")),
    }
}

fn exe_path() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe.to_string_lossy().into_owned())
}

pub fn install(agent: Agent, path: Option<&Path>, target: Target) -> Result<PathBuf, String> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => agent.default_path()?,
    };
    let exe = exe_path()?;
    match agent.json_events() {
        Some(events) => hooks::install_at(&path, &exe, target, events)?,
        None => write_plugin(&path, &plugin_source(agent, &exe, target))?,
    }
    Ok(path)
}

pub fn uninstall(agent: Agent, path: Option<&Path>) -> Result<PathBuf, String> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => agent.default_path()?,
    };
    match agent.json_events() {
        Some(_) => hooks::uninstall_at(&path)?,
        None => remove_plugin(&path)?,
    }
    Ok(path)
}

fn plugin_source(agent: Agent, exe: &str, target: Target) -> String {
    let args: Vec<&str> = if target == Target::CapsLock {
        vec![]
    } else {
        vec!["--target", target.as_str()]
    };
    // JSON string/array literals are valid JS, and escape the path safely.
    let common = COMMON_JS
        .replace("__EXE__", &serde_json::to_string(exe).unwrap())
        .replace("__ARGS__", &serde_json::to_string(&args).unwrap());
    format!(
        "{MARKER} for {}.\n// Regenerate with `capslkagent install-hooks --agent {}`; edits are overwritten.\n\n{common}{}",
        agent.display_name(),
        agent.as_str(),
        agent.plugin_body()
    )
}

fn is_ours(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|s| s.starts_with(MARKER))
}

fn write_plugin(path: &Path, source: &str) -> Result<(), String> {
    if path.exists() && !is_ours(path) {
        return Err(format!(
            "{} exists and wasn't written by capslkagent; not overwriting it",
            path.display()
        ));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(path, source).map_err(|e| e.to_string())
}

fn remove_plugin(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if !is_ours(path) {
        return Err(format!(
            "{} wasn't written by capslkagent; leaving it in place",
            path.display()
        ));
    }
    fs::remove_file(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_source_embeds_exe_and_target() {
        let src = plugin_source(Agent::Pi, r#"/opt/a "b"/capslkagent"#, Target::Backlight);
        assert!(src.starts_with(MARKER));
        assert!(src.contains(r#"const EXE = "/opt/a \"b\"/capslkagent";"#));
        assert!(src.contains(r#"const ARGS = ["--target","backlight"];"#));
        assert!(!src.contains("__EXE__") && !src.contains("__ARGS__"));
        assert!(src.contains("export default function (pi)"));
    }

    #[test]
    fn plugin_install_and_uninstall_respect_foreign_files() {
        let dir = std::env::temp_dir().join(format!("capslkagent-plugin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ours = dir.join("plugins").join("capslkagent.js");
        write_plugin(&ours, &plugin_source(Agent::OpenCode, "/x", Target::CapsLock)).unwrap();
        // Reinstalling over our own file is fine.
        write_plugin(&ours, &plugin_source(Agent::OpenCode, "/y", Target::CapsLock)).unwrap();
        remove_plugin(&ours).unwrap();
        assert!(!ours.exists());
        remove_plugin(&ours).unwrap(); // already gone: no-op

        let foreign = dir.join("plugins").join("other.js");
        fs::write(&foreign, "export const X = 1;\n").unwrap();
        assert!(write_plugin(&foreign, "x").is_err());
        assert!(remove_plugin(&foreign).is_err());
        assert!(foreign.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_names_round_trip() {
        for a in ALL {
            assert_eq!(Agent::parse(a.as_str()), Some(*a));
        }
        assert_eq!(Agent::parse("nope"), None);
    }
}
