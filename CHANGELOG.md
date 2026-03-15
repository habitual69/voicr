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
