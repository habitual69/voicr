<div align="center">

<h1>voicr</h1>

<p><strong>Press a key. Speak. Release. Your words appear wherever your cursor is.</strong><br>
No cloud. No API key. No GUI. Runs entirely on your machine.</p>

[![Release](https://img.shields.io/github/v/release/habitual69/voicr?style=flat-square&color=4a90d9)](https://github.com/habitual69/voicr/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue?style=flat-square)](#installation)

</div>

---

<!-- Demo video — replace with your recording -->
> 📹 **Demo coming soon** — drop a screen recording into `video/demo.mp4` and it will embed here.

<!-- Uncomment once video/demo.mp4 is added:
<div align="center">
  <video src="video/demo.mp4" autoplay loop muted playsinline width="700"></video>
</div>
-->

---

## Why voicr?

Most speech-to-text tools are either a cloud service (sends your audio to someone's server) or a GUI app (heavy, mouse-driven, not scriptable). voicr is neither.

It runs as a **background daemon** you control with a hotkey or a Unix socket. Hold `Ctrl+Space`, speak, release — the transcription is pasted directly into whatever window is focused. No switching apps. No clicking. Just talk.

```
Hold Ctrl+Space → speak → release → text appears in your editor / terminal / browser
```

Everything runs locally. Your audio never leaves your machine.

---

## Features

- **Hold-to-talk hotkey** — configurable combo (`Ctrl+Space` by default), pastes into the active window automatically
- **Daemon mode** — persistent background process, scriptable over a Unix socket with newline-delimited JSON
- **Multiple engines** — Parakeet, Moonshine (streaming), SenseVoice, GigaAM, Whisper — pick your accuracy/speed trade-off
- **Voice Activity Detection** — silence filtering for `--auto-stop` transcription mode
- **Transcription history** — SQLite-backed log with search, export, and retention policies
- **Zero cloud dependency** — fully offline, no API keys, no telemetry
- **Single binary** — one file, no runtime dependencies, works on Linux / macOS / Windows

---

## Installation

### Download a pre-built binary

Go to the [latest release](https://github.com/habitual69/voicr/releases/latest) and grab the binary for your platform.

**Linux (x86_64)**
```bash
curl -L https://github.com/habitual69/voicr/releases/latest/download/voicr-linux-x86_64 -o voicr
chmod +x voicr
sudo mv voicr /usr/local/bin/
```

**macOS (Apple Silicon)**
```bash
curl -L https://github.com/habitual69/voicr/releases/latest/download/voicr-macos-arm64 -o voicr
chmod +x voicr
sudo mv voicr /usr/local/bin/
```

**macOS (Intel)**
```bash
curl -L https://github.com/habitual69/voicr/releases/latest/download/voicr-macos-x86_64 -o voicr
chmod +x voicr
sudo mv voicr /usr/local/bin/
```

**Windows (x86_64)**

Download `voicr-windows-x86_64.exe`, rename it to `voicr.exe`, and move it to a folder in your `%PATH%` (e.g. `C:\Windows\System32\`).

> **Note:** Daemon mode requires Linux or macOS (Unix socket).

### Build from source

Requires [Rust](https://rustup.rs/) stable.

```bash
# Linux: install ALSA headers first
sudo apt-get install libasound2-dev pkg-config

git clone https://github.com/habitual69/voicr
cd voicr
cargo build --release
# binary at: target/release/voicr
```

Enable Whisper support (requires Vulkan SDK: `libvulkan-dev glslang-tools`):
```bash
cargo build --release --features whisper
```

---

## Quick Start

```bash
# 1. Download a model
voicr model download parakeet-tdt-0.6b-v3

# 2. Run — hold Ctrl+Space, speak, release
voicr

# 3. Or run as a daemon and control it from anywhere
voicr daemon &
voicr send toggle        # start recording
voicr send toggle        # stop and transcribe
```

---

## Commands

### Default mode

```bash
voicr          # hold-to-talk with auto-paste (Ctrl+Space)
```

On first run, voicr downloads the recommended model automatically. After that, just hold the hotkey and speak.

### `voicr daemon`

Starts a background daemon that listens on a Unix socket for commands.

```bash
voicr daemon                          # uses /tmp/voicr.sock
voicr daemon --socket /run/voicr.sock # custom socket path
```

The daemon loads the configured model at startup, then waits for commands. Transcription results are broadcast to all connected clients and also printed to stdout (useful for piping).

### `voicr send <command>`

Sends a command to a running daemon.

```bash
voicr send start           # start recording
voicr send stop            # stop recording and transcribe
voicr send toggle          # start if idle, stop if recording
voicr send cancel          # cancel recording without transcribing
voicr send status          # query current state
voicr send shutdown        # shut the daemon down

# wait for and print the result
voicr send toggle --wait
```

### `voicr transcribe`

One-shot: record from microphone, transcribe, print result.

```bash
voicr transcribe                      # record until Ctrl+C
voicr transcribe --duration 10        # stop after 10 seconds
voicr transcribe --auto-stop          # stop on silence
voicr transcribe --no-vad             # disable silence filtering
voicr transcribe --file audio.wav     # transcribe a WAV file
voicr transcribe --output result.txt  # save to file instead of stdout
```

### `voicr hotkey`

Push-to-talk mode with a custom hotkey combo.

```bash
voicr hotkey                          # uses configured combo
voicr hotkey --combo "alt+shift+r"    # override combo
voicr hotkey --no-paste               # print to stdout instead of pasting
```

### `voicr model`

```bash
voicr model list                      # list all models with status
voicr model download <id>             # download a model
voicr model set <id>                  # set as active model
voicr model delete <id>               # delete a downloaded model
voicr model info <id>                 # show model details
```

### `voicr config`

```bash
voicr config show                     # print current config
voicr config set <key> <value>        # update a setting
voicr config path                     # print config file location
```

### `voicr history`

```bash
voicr history list                    # list recent transcriptions
voicr history list --limit 50         # show more entries
voicr history get <id>                # show full text of an entry
voicr history save <id>               # toggle saved/starred status
voicr history delete <id>             # delete an entry
voicr history export                  # export all history as JSON
voicr history export --output out.json
```

### `voicr devices`

```bash
voicr devices                         # list audio input/output devices
```

---

## Models

| ID | Name | Size | Languages | Notes |
|----|------|------|-----------|-------|
| `moonshine-tiny-streaming-en` | Moonshine V2 Tiny | 31 MB | English | Ultra-fast, lowest latency |
| `moonshine-base` | Moonshine Base | 58 MB | English | Fast, handles accents well |
| `moonshine-small-streaming-en` | Moonshine V2 Small | 100 MB | English | Great speed/accuracy balance |
| `sense-voice-int8` | SenseVoice | 160 MB | EN / ZH / JA | CJK + English specialist |
| `moonshine-medium-streaming-en` | Moonshine V2 Medium | 192 MB | English | Near-Parakeet accuracy |
| `gigaam-v3-e2e-ctc` | GigaAM v3 | 225 MB | Multilingual | Strong on Russian |
| `parakeet-tdt-0.6b-v2` | Parakeet V2 | 473 MB | English | Best English accuracy |
| `parakeet-tdt-0.6b-v3` ⭐ | Parakeet V3 | 478 MB | 25 European | Best overall — recommended |
| `small` | Whisper Small | 487 MB | 100+ | Multilingual, translation |
| `medium` | Whisper Medium | 492 MB | 100+ | Higher accuracy, slower |
| `large` | Whisper Large | 1100 MB | 100+ | Max accuracy, slow |
| `turbo` | Whisper Turbo | 1600 MB | 100+ | Balanced large variant |
| `breeze-asr` | Breeze ASR | 1080 MB | ZH-TW / EN | Taiwanese Mandarin specialist |

⭐ Recommended starting point. Whisper models require `--features whisper` at build time.

Models are stored in `~/.local/share/voicr/models/` (Linux/macOS) or `%APPDATA%\voicr\models\` (Windows).

---

## Configuration

Config file location:
```bash
voicr config path
# ~/.config/voicr/config.toml  (Linux)
# ~/Library/Application Support/voicr/config.toml  (macOS)
# %APPDATA%\voicr\config.toml  (Windows)
```

Full config with defaults:
```toml
[audio]
vad_enabled = true              # silence filtering for --auto-stop transcription
vad_threshold = 0.3             # VAD sensitivity (0.0–1.0, lower = more sensitive)
vad_hangover_frames = 8         # silence frames before stopping (1 frame = 30ms)
vad_prefill_frames = 5          # lead-in frames captured before speech onset
max_duration_secs = 0           # max recording length in seconds (0 = unlimited)
device = ""                     # audio device name (empty = system default)

[model]
selected = ""                   # active model ID
unload_timeout = "never"        # when to unload model from memory after use
                                # values: never | immediately | 2min | 5min | 10min | 15min | 1h

[transcription]
language = "auto"               # language code or "auto" for detection
translate_to_english = false    # translate to English (Whisper only)
custom_words = []               # words to boost recognition for (e.g. ["Alice", "Bob"])
filter_filler_words = true      # remove um/uh/etc from output
word_correction_threshold = 0.3

[history]
enabled = true                  # save transcriptions to local SQLite database
limit = 100                     # max entries to keep
retention = "months3"           # how long to keep entries
                                # values: never | preserve_limit | 3days | 2weeks | 3months

[output]
method = "stdout"               # where to send results: stdout | clipboard | file
file_path = ""                  # path when method = "file"
append_newline = true
append_trailing_space = false

[hotkey]
combo = "ctrl+space"            # global hotkey combo
```

### Common config examples

```bash
# Set active model
voicr config set model.selected parakeet-tdt-0.6b-v3

# Use a specific microphone
voicr devices                               # find device name
voicr config set audio.device "HyperX SoloCast"

# Unload model 5 minutes after last use (saves RAM)
voicr config set model.unload_timeout 5min

# Transcribe in Spanish
voicr config set transcription.language es

# Tune silence detection for --auto-stop
voicr config set audio.vad_hangover_frames 8    # 240ms (default, snappy)
voicr config set audio.vad_hangover_frames 15   # 450ms (more forgiving pauses)

# Keep recordings for 2 weeks only
voicr config set history.retention 2weeks
```

---

## Daemon protocol

The daemon speaks newline-delimited JSON over a Unix socket. Any number of clients can connect simultaneously — events are broadcast to all.

### Commands (client → daemon)

```json
{"cmd":"start"}
{"cmd":"stop"}
{"cmd":"toggle"}
{"cmd":"cancel"}
{"cmd":"status"}
{"cmd":"models"}
{"cmd":"set","key":"model.selected","value":"parakeet-tdt-0.6b-v3"}
{"cmd":"shutdown"}
```

### Events (daemon → clients)

```json
{"type":"recording","state":"started"}
{"type":"recording","state":"stopped"}
{"type":"recording","state":"cancelled"}
{"type":"transcribing"}
{"type":"transcription","text":"Hello, world."}
{"type":"model_status","status":"loading","model_id":"parakeet-tdt-0.6b-v3"}
{"type":"model_status","status":"loaded","model_id":"parakeet-tdt-0.6b-v3","model_name":"Parakeet V3"}
{"type":"model_status","status":"unloaded"}
{"type":"models","models":[{"id":"parakeet-tdt-0.6b-v3","name":"Parakeet V3","is_downloaded":true,...}]}
{"type":"ok","message":"Set model.selected = parakeet-tdt-0.6b-v3"}
{"type":"status","state":"idle","model":"parakeet-tdt-0.6b-v3"}
{"type":"error","message":"..."}
{"type":"shutdown"}
```

### Integrations

**Auto-type transcriptions (X11)**
```bash
voicr daemon &
nc -U /tmp/voicr.sock | while IFS= read -r line; do
  text=$(echo "$line" | jq -r 'select(.type=="transcription") | .text')
  [ -n "$text" ] && xdotool type --clearmodifiers "$text"
done
```

**Hotkey daemon (`sxhkd`)**
```
# ~/.config/sxhkd/sxhkdrc
super + space
    echo '{"cmd":"toggle"}' | nc -U /tmp/voicr.sock
```

**Pipe to an LLM**
```bash
voicr transcribe | llm "answer concisely:"
```

---

## Autostart (systemd)

```ini
# ~/.config/systemd/user/voicr.service
[Unit]
Description=voicr speech-to-text daemon
After=sound.target

[Service]
ExecStart=/usr/local/bin/voicr daemon
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```bash
systemctl --user enable --now voicr
systemctl --user status voicr
```

---

## License

MIT
