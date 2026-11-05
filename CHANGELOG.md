# Changelog

All notable changes to this project are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.1] - 2026-11-05

### Changed
- Blinking on macOS and Windows lights caps lock for at most 0.1s per cycle
  instead of half of it, so a keypress is far less likely to land while caps
  lock is on.

### Fixed
- Acknowledging a blink by typing left caps lock on if the agent had turned it
  on, so your text came out in capitals. Caps lock now ends up off unless you
  turned it on yourself.
- The blink daemon only checked for input once per blink phase. It now checks
  every 20ms, so caps lock stops blinking almost immediately.

## [0.2.0] - 2026-10-01

### Changed
- Acknowledging a blink (pressing a key or moving the mouse) now returns the
  light to its state before blinking started, instead of always turning it
  off. After approving a permission prompt the light goes back to solid on
  while the agent keeps working.
- `status` no longer stops a running blink, and prints `blinking` while one is
  active.
- `toggle` during a blink toggles relative to the pre-blink state.
- On Linux the blink pidfile lives in `$XDG_RUNTIME_DIR` when available,
  instead of the shared `/tmp`.
- `uninstall-hooks` removes the `hooks` key from settings.json when it ends up
  empty.

### Fixed
- A blink daemon that exited on its own left a stale pidfile behind, so a
  later `on`/`off` could send SIGTERM to an unrelated process that reused the
  PID.
- `blink` with a negative, zero, NaN or infinite interval crashed the daemon
  (leaving the light stuck on) or spun the CPU. Intervals must now be between
  0.05 and 60 seconds.
- Backlight: a second `off` (for example `Stop` followed by `SessionEnd`) set
  the keyboard backlight to zero instead of leaving the user's brightness
  alone.
- Windows: the blink daemon could mistake its own synthesized Caps Lock key
  presses for user input and stop blinking immediately.
- Hook commands broke when the binary path contained quotes, `$` or
  backticks; the path is now shell-quoted.
- `on`/`off` could race with a toggle the daemon was in the middle of; they
  now wait for the daemon to exit first.

### Added
- Support for more coding agents: `install-hooks --agent codex|pi|omp|amp|opencode`
  (Claude Code stays the default). Codex gets hooks in `~/.codex/hooks.json`;
  pi, omp, Amp and opencode get a small managed plugin file that runs the
  binary. `--agent all` installs for every agent set up on the machine.
- `CLAUDE_CONFIG_DIR` is honoured when locating Claude Code's settings.
- CI workflow running clippy and the tests on macOS, Linux and Windows.
- Unit tests for hook install/uninstall.

## [0.1.0] - 2026-07-20

### Added
- Initial release: Caps Lock LED (macOS, Linux, Windows) and macOS keyboard
  backlight as a Claude Code status light, with `install-hooks` /
  `uninstall-hooks`.

[Unreleased]: https://github.com/jagenaujagenau/capslkagent/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/jagenaujagenau/capslkagent/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/jagenaujagenau/capslkagent/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jagenaujagenau/capslkagent/releases/tag/v0.1.0
