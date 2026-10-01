# capslkagent

Turn your keyboard's **Caps Lock LED** into a status light for your coding
agent — Claude Code, Codex, pi, oh-my-pi (omp), Amp or opencode:

| LED state | Meaning |
|-----------|---------|
| Solid on  | Agent is working |
| Blinking  | Agent needs your attention (permission prompt / notification) |
| Off       | Agent is idle |

Cross-platform (macOS, Linux, Windows). Written in Rust — one small
binary (~400 KB), no runtime dependencies beyond the OS (libc on Linux).

On Mac laptops you can alternatively use the **keyboard backlight** as the
indicator (`--backlight`) — solid bright while the agent works, flashing when
it needs you, and your previous brightness is restored when it goes idle. This
avoids the caps-lock caveat below entirely.

## Install

### Homebrew (macOS/Linux)

```sh
brew install jagenaujagenau/tap/capslkagent
```

### From source

```sh
cargo build --release
# optionally put it on your PATH:
cp target/release/capslkagent ~/.local/bin/
```

### Wire it into your agent

```sh
capslkagent install-hooks                      # Claude Code (default)
capslkagent install-hooks --agent codex        # or: codex, pi, omp, amp, opencode
capslkagent install-hooks --agent all          # every agent set up on this machine
capslkagent --backlight install-hooks          # keyboard backlight instead (macOS)
```

Build on each platform you use (or cross-compile with the matching Rust
target). `install-hooks` points the agent at the binary's own absolute path,
so re-run it if you move the binary. It is idempotent, and
`uninstall-hooks --agent <name>` (or `--agent all`) removes the integration.

| Agent | What gets installed | On | Blink | Off |
|-------|---------------------|----|-------|-----|
| Claude Code | hooks in `~/.claude/settings.json` | `UserPromptSubmit` | `Notification` | `Stop`, `SessionEnd` |
| Codex | hooks in `~/.codex/hooks.json` | `UserPromptSubmit` | `PermissionRequest` | `Stop`, `Interrupt`, `SessionEnd` |
| pi | extension `~/.pi/agent/extensions/capslkagent.ts` | `agent_start` | extension dialogs (`ui_prompt_start`) | `agent_settled`, `session_shutdown` |
| omp | extension `~/.omp/agent/extensions/capslkagent.ts` | `agent_start` | `tool_approval_requested` | `agent_end`, `session_shutdown` |
| Amp | plugin `~/.config/amp/plugins/capslkagent.ts` | `agent.start` | thread state `awaiting-approval` | `agent.end`, shutdown |
| opencode | plugin `~/.config/opencode/plugins/capslkagent.js` | `session.status` busy | `permission.asked`, `question.asked` | `session.status` idle |

Notes per agent:

- **Claude Code / Codex** — existing hooks are preserved; only capslkagent's
  own entries are added or replaced. `CLAUDE_CONFIG_DIR` / `CODEX_HOME` are
  honoured. **Codex asks you to trust new hooks**: run `/hooks` in Codex once
  after installing.
- **pi, omp, Amp, opencode** — these load JS/TS plugins, so capslkagent writes
  one small managed file that runs the binary. It never overwrites or deletes
  a file it didn't write. Restart the agent (or reload plugins) to pick it up.
  `PI_CODING_AGENT_DIR` and `XDG_CONFIG_HOME` are honoured.
- Subagents (opencode task sessions, omp subagents) don't drive the light.
- pi has no built-in permission prompts, so it only blinks for extension
  dialogs.

## Manual usage

```sh
capslkagent on          # solid LED
capslkagent blink       # flash (default 0.4s); `blink 0.2` for faster
capslkagent off         # LED off (also stops blinking)
capslkagent toggle
capslkagent status      # print on/off/blinking (doesn't stop a blink)
```

`blink` spawns a small detached daemon; any subsequent `on`/`off` kills it and
restores a steady state.

**Blinking stops as soon as you touch the keyboard or mouse** — pressing any
key acknowledges the alert and returns the light to its state before blinking
started (e.g. solid on if the agent is still working after you approve a
permission prompt). This is detected via the
system's HID idle timer (no accessibility permissions needed), so it works even
though the daemon has no window focus. On macOS and Windows this is automatic;
on Linux there's no permission-free system-wide idle source, so blinking there
stops on the next `on`/`off` instead (e.g. the agent's next prompt).

All commands accept `--target capslock|backlight` (or the `--backlight`
shorthand) to pick which light to drive; caps lock is the default.

## How it works per platform

- **macOS (caps lock)** — calls IOKit's
  `IOHIDSetModifierLockState(kIOHIDCapsLockState)` directly via FFI (links the
  IOKit framework; no cgo-style toolchain pain, which is why this is Rust and
  not Go).
- **macOS (backlight)** — talks to `KeyboardBrightnessClient` in the private
  `CoreBrightness.framework` via `dlopen` + the Objective-C runtime, so
  nothing private is linked at build time. `on` saves your current brightness
  to a temp file and goes to full; `off` restores it. Being a private
  framework, this could break in a future macOS release (works on current
  macOS 26). Note the system's automatic keyboard-backlight dimming may still
  adjust brightness on top of what we set.
- **Linux** — writes to `/sys/class/leds/*capslock*/brightness`, which drives
  the LED *without* changing typing state. Falls back to `xset led` under X11.
- **Windows** — synthesizes a `VK_CAPITAL` key press via `user32.keybd_event`.

### Caveat: caps lock is caps lock

On macOS and Windows the LED is tied to the actual caps-lock modifier state,
so while the light is on, **typing produces CAPITALS**. If you type into other
apps while the agent works, that may be annoying — it's inherent to how those
OSes expose the LED. On Linux the sysfs backend changes only the LED.

### Linux permissions

`/sys/class/leds/.../brightness` is root-writable by default. Grant your user
access with a udev rule:

```sh
sudo tee /etc/udev/rules.d/99-capslock-led.rules <<'EOF'
ACTION=="add", SUBSYSTEM=="leds", KERNEL=="*capslock*", RUN+="/bin/chmod 0666 /sys%p/brightness"
EOF
sudo udevadm control --reload && sudo udevadm trigger -s leds
```

## Building for other platforms

Each backend uses platform-native FFI (IOKit/CoreBrightness on macOS, `user32`
on Windows, sysfs on Linux) behind `#[cfg]` gates, so **binaries are built on
the target OS**, not cross-compiled from one machine. The
`.github/workflows/release.yml` matrix does exactly this on push of a `vX.Y.Z`
tag:

| Target | Runner | Rust target |
|--------|--------|-------------|
| macOS universal (arm64 + x86_64) | `macos-latest` | both, merged with `lipo` |
| Linux x86_64 | `ubuntu-latest` | `x86_64-unknown-linux-gnu` |
| Linux aarch64 | `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` |
| Windows x86_64 | `windows-latest` | `x86_64-pc-windows-msvc` |

It uploads a `.tar.gz`/`.zip` per platform and attaches them to a GitHub
Release.

You *can* cross-compile locally (`rustup target add …` then
`cargo build --target …`), but only for the host OS's own arches without extra
toolchains — Mac→Mac universal works locally; Mac→Linux/Windows needs a cross
linker (`cross`, mingw, etc.), which is why CI is the recommended path.

## License

MIT

