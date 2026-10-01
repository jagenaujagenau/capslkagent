//! Install/uninstall shell-command hooks in a Claude-style hooks JSON file
//! (Claude Code's settings.json, Codex's hooks.json).

use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;

fn load(path: &Path) -> Result<Map<String, Value>, String> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{} is not a JSON object", path.display())),
        Err(e) => Err(format!("Cannot parse {}: {e}", path.display())),
    }
}

fn save(path: &Path, settings: &Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    text.push('\n');
    fs::write(path, text).map_err(|e| e.to_string())
}

fn is_ours(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .map(|hs| {
            hs.iter().any(|h| {
                h["command"].as_str().is_some_and(|c| {
                    // Current name and the legacy one, so uninstall-hooks
                    // cleans up entries written by an older version too.
                    c.contains("capslkagent") || c.contains("capslock-indicator")
                })
            })
        })
        .unwrap_or(false)
}

/// Build a hook command string. On Unix it is guarded so that if the binary is
/// gone (e.g. after `brew uninstall` without running `uninstall-hooks`), the
/// hook silently no-ops instead of erroring and disrupting Claude Code.
#[cfg(not(windows))]
fn hook_command(exe: &str, targ: &str, action: &str) -> String {
    let exe = shell_quote(exe);
    format!("[ -x {exe} ] && {exe}{targ} {action} || true")
}

/// Single-quote for sh, so paths containing `"`, `$` or backticks are safe.
#[cfg(not(windows))]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn hook_command(exe: &str, targ: &str, action: &str) -> String {
    format!("\"{exe}\"{targ} {action}")
}

/// Remove our entries from every event; drop events left empty.
fn strip_ours(hooks: &mut Map<String, Value>) {
    let empty: Vec<String> = hooks
        .iter_mut()
        .filter_map(|(event, entries)| {
            if let Some(arr) = entries.as_array_mut() {
                arr.retain(|e| !is_ours(e));
                if arr.is_empty() {
                    return Some(event.clone());
                }
            }
            None
        })
        .collect();
    for event in empty {
        hooks.remove(&event);
    }
}

/// Add our hook for each `(event, action)` pair, replacing any of ours.
pub fn install_at(
    path: &Path,
    exe: &str,
    target: crate::led::Target,
    events: &[(&str, &str)],
) -> Result<(), String> {
    let mut settings = load(path)?;
    let targ = if target != crate::led::Target::CapsLock {
        format!(" --target {}", target.as_str())
    } else {
        String::new()
    };

    let hooks = settings
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("`hooks` in settings.json is not an object")?;
    strip_ours(hooks);
    for (event, action) in events {
        let entry = json!({
            "hooks": [{"type": "command", "command": hook_command(exe, &targ, action)}]
        });
        hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{event} is not an array"))?
            .push(entry);
    }
    save(path, &settings)
}

pub fn uninstall_at(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let mut settings = load(path)?;
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        strip_ours(hooks);
        if hooks.is_empty() {
            settings.remove("hooks");
        }
    }
    save(path, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::led::Target;
    use std::path::PathBuf;

    const EVENTS: &[(&str, &str)] = &[
        ("UserPromptSubmit", "on"),
        ("Stop", "off"),
        ("Notification", "blink"),
        ("SessionEnd", "off"),
    ];

    fn temp_settings(name: &str, contents: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("capslkagent-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        match contents {
            Some(c) => fs::write(&path, c).unwrap(),
            None => {
                let _ = fs::remove_file(&path);
            }
        }
        path
    }

    fn read(path: &PathBuf) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    const EXE: &str = "/opt/bin/capslkagent";

    #[test]
    fn install_is_idempotent_and_preserves_other_hooks() {
        let foreign = r#"{"model":"x","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#;
        let path = temp_settings("idempotent.json", Some(foreign));
        install_at(&path, EXE, Target::CapsLock, EVENTS).unwrap();
        let once = read(&path);
        install_at(&path, EXE, Target::CapsLock, EVENTS).unwrap();
        assert_eq!(once, read(&path));

        assert_eq!(once["model"], "x");
        let stop = once["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        for (event, action) in EVENTS {
            let entries = once["hooks"][event].as_array().unwrap();
            let cmd = entries.last().unwrap()["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains(EXE) && cmd.contains(action), "{event}: {cmd}");
        }
    }

    #[test]
    fn uninstall_restores_original() {
        let foreign = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#;
        let path = temp_settings("restore.json", Some(foreign));
        install_at(&path, EXE, Target::Backlight, EVENTS).unwrap();
        assert!(read(&path)["hooks"]["Notification"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--target backlight"));
        uninstall_at(&path).unwrap();
        assert_eq!(read(&path), serde_json::from_str::<Value>(foreign).unwrap());
    }

    #[test]
    fn uninstall_drops_empty_hooks_object() {
        let path = temp_settings("empty.json", None);
        install_at(&path, EXE, Target::CapsLock, EVENTS).unwrap();
        uninstall_at(&path).unwrap();
        assert_eq!(read(&path), json!({}));
    }

    #[test]
    fn rejects_non_object_settings() {
        let path = temp_settings("array.json", Some("[]"));
        assert!(install_at(&path, EXE, Target::CapsLock, EVENTS).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn hook_command_quotes_awkward_paths() {
        let cmd = hook_command("/a b/it's/$x", "", "on");
        assert_eq!(cmd, r#"[ -x '/a b/it'\''s/$x' ] && '/a b/it'\''s/$x' on || true"#);
        let out = std::process::Command::new("sh").arg("-c").arg(&cmd).status().unwrap();
        assert!(out.success());
    }
}
