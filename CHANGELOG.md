## What's New

### Hold-to-Talk Mode
voicr now uses **hold-to-talk** instead of toggle. Hold your hotkey while speaking, release to transcribe and paste. No more accidental double-triggers or stale recordings.

### Bug Fixes

- **Fix empty transcriptions after long recordings** — Silero VAD LSTM state was never reset between recordings, causing the model to classify speech as noise after extended use. Both `SileroVad` and `SmoothedVad` now properly reset internal state on each new recording.
- **Fix unreliable hotkey on Linux laptops** — Modifier keys (ctrl, shift, alt) are now tracked globally across all evdev keyboard devices using shared atomic state. Previously each evdev device tracked modifiers independently, so combos like `ctrl+shift+space` would fail when keys arrived on different devices (common on laptops with separate media-key devices).
- **Fix ydotoold daemon startup** — `ensure_ydotoold()` now searches common install paths (`/usr/bin/ydotoold`, `/usr/local/bin/ydotoold`, etc.) instead of relying on PATH, which was unavailable in background threads.
- **Fix paste interfering with active window** — Added 200ms delay before pasting so modifier keys from the hotkey combo are fully released before ydotool injects keystrokes.

### Improvements

- **Configurable hotkey** — Default `ctrl+space`, configurable via `voicr config set hotkey.combo "ctrl+shift+space"`. Supports ctrl, alt, shift, meta modifiers with any key.
- **Linux auto-setup** — First run automatically installs `ydotool`, sets up `/dev/uinput` permissions (udev rule + immediate chmod), adds user to `input` group, and re-execs via `sg` without requiring re-login.
- **Better paste fallback chain** — dotool > ydotool > wtype > xdotool > clipboard. Works on GNOME Wayland, wlroots, X11, and XWayland.
- **Wayland clipboard** — Uses `wl-copy` for reliable clipboard writes from background processes.
- **Pre-started ydotoold** — Daemon is started at voicr launch so paste is instant on first use.
- **rdev fallback** — macOS/Windows use rdev for global hotkeys; Linux X11 falls back to rdev when evdev is unavailable.
- **Enter key fallback** — Press Enter in the terminal to toggle recording when the global hotkey isn't available.

## Installation

Download the binary for your platform, make it executable, and place it somewhere in your `$PATH`.

### Linux (x86_64)
```bash
curl -LO https://github.com/habitual69/voicr/releases/latest/download/voicr-linux-x86_64
chmod +x voicr-linux-x86_64
sudo mv voicr-linux-x86_64 /usr/local/bin/voicr
```

### macOS (Apple Silicon)
```bash
curl -LO https://github.com/habitual69/voicr/releases/latest/download/voicr-macos-arm64
chmod +x voicr-macos-arm64
sudo mv voicr-macos-arm64 /usr/local/bin/voicr
```

### macOS (Intel)
```bash
curl -LO https://github.com/habitual69/voicr/releases/latest/download/voicr-macos-x86_64
chmod +x voicr-macos-x86_64
sudo mv voicr-macos-x86_64 /usr/local/bin/voicr
```

### Windows (x86_64)
Download `voicr-windows-x86_64.exe`, rename to `voicr.exe`, and add to a folder in your `%PATH%`.

## Quick Start
```bash
voicr                    # hold-to-talk mode (downloads model on first run)
voicr model list         # see available models
voicr config show        # show current settings
voicr --help             # full help
```
