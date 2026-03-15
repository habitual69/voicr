# voicr

A headless, terminal-first speech-to-text tool. Run it as a background daemon controlled via Unix socket, or use it for quick one-shot transcriptions — no GUI required.

Built on [transcribe-rs](https://github.com/cjpais/transcribe-rs) with support for Parakeet, Moonshine, SenseVoice, GigaAM, and Whisper models.

---

## Installation

### Download a pre-built binary

Go to the [latest release](https://github.com/habitual69/voicr/releases/latest) and download the binary for your platform.

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

## Quick start

```bash
# 1. Download a model
voicr model download moonshine-base

# 2. Transcribe from microphone (press Ctrl+C to stop)
voicr transcribe

# 3. Or run as a background daemon
voicr daemon &
voicr send toggle        # start recording
voicr send toggle        # stop and transcribe
```

---

## Commands

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

| ID | Name | Size | Accuracy | Speed |
|----|------|------|----------|-------|
| `moonshine-tiny-streaming-en` | Moonshine V2 Tiny | 31 MB | 55% | 95% |
| `moonshine-base` | Moonshine Base | 58 MB | 70% | 90% |
| `moonshine-small-streaming-en` | Moonshine V2 Small | 100 MB | 65% | 90% |
| `sense-voice-int8` | SenseVoice | 160 MB | 65% | 95% |
| `moonshine-medium-streaming-en` | Moonshine V2 Medium | 192 MB | 75% | 80% |
| `gigaam-v3-e2e-ctc` | GigaAM v3 | 225 MB | 85% | 75% |
| `parakeet-tdt-0.6b-v2` | Parakeet V2 | 473 MB | 85% | 85% |
| `parakeet-tdt-0.6b-v3` ⭐ | Parakeet V3 | 478 MB | 80% | 85% |
| `small` | Whisper Small | 487 MB | 60% | 85% |
| `medium` | Whisper Medium | 492 MB | 75% | 60% |
| `breeze-asr` | Breeze ASR | 1080 MB | 85% | 35% |
| `large` | Whisper Large | 1100 MB | 85% | 30% |
| `turbo` | Whisper Turbo | 1600 MB | 80% | 40% |

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
vad_enabled = true          # filter silence with Voice Activity Detection
vad_threshold = 0.3         # VAD sensitivity (0.0–1.0, lower = more sensitive)
max_duration_secs = 0       # max recording length in seconds (0 = unlimited)
device = ""                 # audio device name (empty = system default)

[model]
selected = ""               # active model ID
unload_timeout = "never"    # when to unload model from memory after use
                            # values: never | immediately | 2min | 5min | 10min | 15min | 1h

[transcription]
language = "auto"           # language code or "auto" for detection
translate_to_english = false  # translate to English (Whisper only)
custom_words = []           # words to boost recognition for (e.g. ["Alice", "Bob"])
filter_filler_words = true  # remove um/uh/etc from output
word_correction_threshold = 0.3

[history]
enabled = true              # save transcriptions to local SQLite database
limit = 100                 # max entries to keep
retention = "months3"       # how long to keep entries
                            # values: never | preserve_limit | 3days | 2weeks | 3months

[output]
method = "stdout"           # where to send results: stdout | clipboard | file
file_path = ""              # path when method = "file"
append_newline = true
append_trailing_space = false
```

### Common config examples

```bash
# Set active model
voicr config set model.selected moonshine-base

# Use a specific microphone
voicr devices                               # find device name
voicr config set audio.device "HyperX SoloCast"

# Unload model 5 minutes after last use (saves RAM)
voicr config set model.unload_timeout 5min

# Transcribe in Spanish
voicr config set transcription.language es

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
{"cmd":"set","key":"model.selected","value":"moonshine-base"}
{"cmd":"shutdown"}
```

### Events (daemon → clients)

```json
{"type":"recording","state":"started"}
{"type":"recording","state":"stopped"}
{"type":"recording","state":"cancelled"}
{"type":"transcribing"}
{"type":"transcription","text":"Hello, world."}
{"type":"model_status","status":"loading","model_id":"moonshine-base"}
{"type":"model_status","status":"loaded","model_id":"moonshine-base","model_name":"Moonshine Base"}
{"type":"model_status","status":"unloaded"}
{"type":"models","models":[{"id":"moonshine-base","name":"Moonshine Base","is_downloaded":true,...}]}
{"type":"ok","message":"Set model.selected = moonshine-base"}
{"type":"status","state":"idle","model":"moonshine-base"}
{"type":"error","message":"..."}
{"type":"shutdown"}
```

### Example: pipe daemon output to a script

```bash
# Start daemon in background
voicr daemon &

# Listen for transcriptions and auto-type them
nc -U /tmp/voicr.sock | while IFS= read -r line; do
  text=$(echo "$line" | jq -r 'select(.type=="transcription") | .text')
  [ -n "$text" ] && xdotool type --clearmodifiers "$text"
done
```

### Integration with a hotkey daemon (e.g. `sxhkd`)

```
# ~/.config/sxhkd/sxhkdrc
super + space
    echo '{"cmd":"toggle"}' | nc -U /tmp/voicr.sock
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
