## What's New in v0.3.2

### Performance: Eliminated VAD overhead in push-to-talk mode

In daemon and hotkey (hold-to-talk) modes, the recording start and stop are controlled entirely by
the key press and release — VAD has no role in those boundaries. Despite this, Silero VAD was
running ONNX inference on every 30 ms audio frame during every recording, silently dropping frames
classified as silence (natural mid-sentence pauses) and adding unnecessary CPU overhead.

**Changes:**
- Removed VAD from the daemon recording path (`do_start_recording`) — recordings are now raw and
  unfiltered, preserving natural pauses and reducing CPU usage during hold-to-talk
- Removed VAD from the standalone hotkey mode (`voicr hotkey` / default `voicr` mode) for the same reason
- Removed the VAD model download step that was triggered even in daemon/hotkey mode (saves startup time and disk I/O on first run)
- VAD is **kept** for `voicr transcribe --auto-stop`, where silence detection is genuinely needed to
  determine when to stop recording

### Configurable VAD parameters for transcribe mode

The VAD hangover and prefill were previously hardcoded at 450 ms each. They are now configurable
and the defaults have been tuned for faster response:

| Setting | Old (hardcoded) | New default | Effect |
|---------|----------------|-------------|--------|
| `audio.vad_hangover_frames` | 15 frames (450 ms) | 8 frames (240 ms) | Stops recording ~210 ms sooner after silence |
| `audio.vad_prefill_frames` | 15 frames (450 ms) | 5 frames (150 ms) | Shorter lead-in buffer |

```bash
# Tune for your environment
voicr config set audio.vad_hangover_frames 8   # 240ms silence before stopping (default)
voicr config set audio.vad_prefill_frames 5    # 150ms lead-in capture (default)
```

---

## What's New in v0.2.2

### Build Fix
- Eliminated all 21 compiler warnings from cross-platform conditional compilation by properly gating unix-only imports, types, and functions behind `#[cfg(unix)]` / `#[cfg_attr]` attributes

## What's New in v0.2.1

### Windows Icon
The Windows `.exe` now has an embedded application icon (visible in Explorer, taskbar, and Alt+Tab). macOS iconset included for `.app` bundle packaging.

### Build Improvements
- Added `build.rs` with `winresource` to embed icon and version metadata into Windows binaries
- Suppressed unused code warnings for cross-platform conditional compilation

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
