/// Voicr daemon – listens on a named pipe (Windows) or Unix socket and processes voice commands.
///
/// Protocol: newline-delimited JSON.
///
/// Commands (client → daemon):
///   {"cmd":"start"}                          Start recording
///   {"cmd":"stop"}                           Stop recording, transcribe, emit result
///   {"cmd":"toggle"}                         Start if idle, stop if recording
///   {"cmd":"cancel"}                         Cancel current recording
///   {"cmd":"status"}                         Query daemon state
///   {"cmd":"models"}                         List available models
///   {"cmd":"set","key":"<k>","value":"<v>"}  Set a config key (e.g. model.selected)
///   {"cmd":"shutdown"}                       Shut the daemon down gracefully
///
/// Events (daemon → all connected clients):
///   {"type":"recording","state":"started"}
///   {"type":"recording","state":"stopped"}
///   {"type":"transcribing"}
///   {"type":"transcription","text":"..."}
///   {"type":"model_status","status":"loading","model_id":"..."}
///   {"type":"model_status","status":"loaded","model_id":"...","model_name":"..."}
///   {"type":"model_status","status":"unloaded"}
///   {"type":"models","models":[...]}
///   {"type":"ok","message":"..."}
///   {"type":"error","message":"..."}
///   {"type":"status","state":"idle"|"recording"|"transcribing","model":"..."}
///   {"type":"shutdown"}

use crate::audio_toolkit::AudioRecorder;
use crate::config::Config;
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;

// ── Protocol types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Command {
    cmd: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Recording {
        state: String,
    },
    Transcribing,
    Transcription {
        text: String,
    },
    ModelStatus {
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model_name: Option<String>,
    },
    Models {
        models: Vec<crate::managers::model::ModelInfo>,
    },
    Ok {
        message: String,
    },
    Error {
        message: String,
    },
    Status {
        state: String,
        model: String,
    },
    Shutdown,
}

// ── Daemon state ───────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
enum DaemonState {
    Idle,
    Recording,
    Transcribing,
}

// ── Client broadcast infrastructure ───────────────────────────────────────────

static CLIENT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Each entry is (client_id, sender). broadcast() retains only live senders.
type Clients = Arc<Mutex<Vec<(u64, UnboundedSender<String>)>>>;

fn broadcast(clients: &Clients, event: &Event) {
    let json = match serde_json::to_string(event) {
        Ok(j) => format!("{}\n", j),
        Err(e) => {
            error!("Failed to serialize event: {}", e);
            return;
        }
    };
    let mut list = clients.lock().unwrap();
    list.retain(|(_, tx)| tx.send(json.clone()).is_ok());
}

// ── Main daemon entry point ────────────────────────────────────────────────────

pub async fn run_daemon(
    socket_path: PathBuf,
    config: Arc<Mutex<Config>>,
    model_manager: Arc<ModelManager>,
) -> Result<()> {
    let clients: Clients = Arc::new(Mutex::new(Vec::new()));
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let shutdown_notify = Arc::new(Notify::new());

    // Build status callback that broadcasts model events to all clients
    let clients_for_status = clients.clone();
    let status_cb: crate::managers::transcription::StatusCallback =
        Arc::new(move |status| {
            use crate::managers::transcription::ModelStatus;
            let event = match status {
                ModelStatus::Loading { model_id } => Event::ModelStatus {
                    status: "loading".to_string(),
                    model_id: Some(model_id),
                    model_name: None,
                },
                ModelStatus::Loaded { model_id, model_name } => Event::ModelStatus {
                    status: "loaded".to_string(),
                    model_id: Some(model_id),
                    model_name: Some(model_name),
                },
                ModelStatus::Unloaded => Event::ModelStatus {
                    status: "unloaded".to_string(),
                    model_id: None,
                    model_name: None,
                },
                ModelStatus::Error { message, .. } => Event::Error { message },
            };
            broadcast(&clients_for_status, &event);
        });

    let transcription_manager = Arc::new(TranscriptionManager::new(
        model_manager.clone(),
        config.clone(),
        Some(status_cb),
    )?);

    // Load model at startup
    {
        let tm = transcription_manager.clone();
        let clients_clone = clients.clone();
        std::thread::spawn(move || {
            if let Err(e) = tm.ensure_model_loaded() {
                broadcast(
                    &clients_clone,
                    &Event::Error {
                        message: format!("Model load failed: {}", e),
                    },
                );
                error!("Failed to load model: {}", e);
            }
        });
    }

    // Set up Ctrl+C / SIGTERM to shutdown gracefully
    let shutdown_clone = shutdown_flag.clone();
    let notify_clone = shutdown_notify.clone();
    ctrlc::set_handler(move || {
        info!("Shutdown signal received");
        shutdown_clone.store(true, Ordering::Relaxed);
        notify_clone.notify_one();
    })
    .ok();

    let state = Arc::new(Mutex::new(DaemonState::Idle));
    let recorder: Arc<Mutex<Option<AudioRecorder>>> = Arc::new(Mutex::new(None));

    // ── Spawn hotkey listener for hold-to-talk ──────────────────────────────────
    {
        let combo_str = config.lock().unwrap().hotkey.combo.clone();
        let clients_hk = clients.clone();
        let state_hk = state.clone();
        let recorder_hk = recorder.clone();
        let tm_hk = transcription_manager.clone();
        let config_hk = config.clone();

        if let Ok(hotkey) = crate::hotkey::parse_combo(&combo_str) {
            let (tx, rx) = std::sync::mpsc::channel::<crate::hotkey::HotkeySignal>();

            // Start evdev listener (Linux), fall back to rdev on all other platforms
            #[cfg(target_os = "linux")]
            let hotkey_ok = {
                let evdev_ok = crate::hotkey::spawn_evdev_listener_pub(&hotkey, tx.clone());
                if !evdev_ok
                    && crate::hotkey::detect_display_server()
                        == crate::hotkey::DisplayServer::X11
                {
                    crate::hotkey::spawn_rdev_listener(&hotkey, tx.clone());
                    true
                } else {
                    evdev_ok
                }
            };
            #[cfg(not(target_os = "linux"))]
            let hotkey_ok = {
                crate::hotkey::spawn_rdev_listener(&hotkey, tx.clone());
                true
            };

            if hotkey_ok {
                info!("Hotkey [{}] active (hold-to-talk)", combo_str);
            } else {
                warn!(
                    "Hotkey [{}] not available — use socket commands instead",
                    combo_str
                );
            }

            // Process hotkey signals in a background thread
            std::thread::spawn(move || {
                for signal in rx {
                    if signal {
                        // Key pressed → start recording
                        do_start_recording(
                            &clients_hk,
                            &state_hk,
                            &recorder_hk,
                            &config_hk,
                        );
                    } else {
                        // Key released → stop + transcribe + paste
                        do_stop_transcribe_paste(
                            &clients_hk,
                            &state_hk,
                            &recorder_hk,
                            &tm_hk,
                            &config_hk,
                        );
                    }
                }
            });
        } else {
            warn!("Invalid hotkey combo '{}', hotkey disabled", combo_str);
        }
    }

    // ── Platform-specific IPC accept loop ──────────────────────────────────────

    #[cfg(unix)]
    {
        // Remove stale socket file
        if socket_path.exists() {
            std::fs::remove_file(&socket_path)?;
        }
        let listener = tokio::net::UnixListener::bind(&socket_path)?;
        info!("Daemon listening on {:?}", socket_path);

        loop {
            let conn = tokio::select! {
                result = listener.accept() => match result {
                    Ok((stream, _)) => stream,
                    Err(e) => { error!("Accept error: {}", e); continue; }
                },
                _ = shutdown_notify.notified() => break,
            };

            let clients_c = clients.clone();
            let state_c = state.clone();
            let tm_c = transcription_manager.clone();
            let mm_c = model_manager.clone();
            let config_c = config.clone();
            let recorder_c = recorder.clone();
            let notify_c = shutdown_notify.clone();
            let flag_c = shutdown_flag.clone();

            tokio::spawn(handle_client(
                conn, clients_c, state_c, tm_c, mm_c, config_c, recorder_c,
                notify_c, flag_c,
            ));
        }

        let _ = std::fs::remove_file(&socket_path);
    }

    #[cfg(windows)]
    {
        use std::mem;
        use tokio::net::windows::named_pipe::ServerOptions;

        let pipe_name = socket_path.to_string_lossy().into_owned();
        info!("Daemon listening on {}", pipe_name);

        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)?;

        loop {
            let did_connect = tokio::select! {
                result = server.connect() => match result {
                    Ok(()) => true,
                    Err(e) => { error!("Pipe accept error: {}", e); break; }
                },
                _ = shutdown_notify.notified() => false,
            };

            if !did_connect {
                break;
            }

            // Create next server instance before handing off the connected one
            let next = match ServerOptions::new().create(&pipe_name) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create next pipe instance: {}", e);
                    break;
                }
            };
            let connected = mem::replace(&mut server, next);

            if shutdown_flag.load(Ordering::Relaxed) {
                break;
            }

            let clients_c = clients.clone();
            let state_c = state.clone();
            let tm_c = transcription_manager.clone();
            let mm_c = model_manager.clone();
            let config_c = config.clone();
            let recorder_c = recorder.clone();
            let notify_c = shutdown_notify.clone();
            let flag_c = shutdown_flag.clone();

            tokio::spawn(handle_client(
                connected, clients_c, state_c, tm_c, mm_c, config_c, recorder_c,
                notify_c, flag_c,
            ));
        }
    }

    broadcast(&clients, &Event::Shutdown);
    info!("Daemon shut down");
    Ok(())
}

// ── Per-client handler ────────────────────────────────────────────────────────

async fn handle_client<S>(
    stream: S,
    clients: Clients,
    state: Arc<Mutex<DaemonState>>,
    transcription_manager: Arc<TranscriptionManager>,
    model_manager: Arc<ModelManager>,
    config: Arc<Mutex<Config>>,
    recorder: Arc<Mutex<Option<AudioRecorder>>>,
    shutdown_notify: Arc<Notify>,
    shutdown_flag: Arc<AtomicBool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (read_half, mut write_half) = tokio::io::split(stream);

    // Channel used both for broadcast messages and direct responses to this client
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let client_id = CLIENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    clients.lock().unwrap().push((client_id, tx.clone()));

    // Writer task: drains the channel and writes to the stream
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_half.write_all(msg.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    // Reader loop
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF — client disconnected
            Err(_) => break,
            Ok(_) => {}
        }

        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }

        let cmd: Command = match serde_json::from_str(trimmed) {
            Ok(c) => c,
            Err(e) => {
                let event = Event::Error {
                    message: format!("Invalid command JSON: {}", e),
                };
                let json = serde_json::to_string(&event).unwrap_or_default();
                let _ = tx.send(format!("{}\n", json));
                continue;
            }
        };

        debug!("Received command: {}", cmd.cmd);

        match cmd.cmd.as_str() {
            "start" => {
                do_start_recording(&clients, &state, &recorder, &config);
            }
            "stop" => {
                do_stop_and_transcribe(
                    &clients,
                    &state,
                    &recorder,
                    &transcription_manager,
                    &config,
                );
            }
            "toggle" => {
                let current = state.lock().unwrap().clone();
                match current {
                    DaemonState::Idle => {
                        do_start_recording(&clients, &state, &recorder, &config)
                    }
                    DaemonState::Recording => do_stop_and_transcribe(
                        &clients,
                        &state,
                        &recorder,
                        &transcription_manager,
                        &config,
                    ),
                    DaemonState::Transcribing => {
                        broadcast(
                            &clients,
                            &Event::Error {
                                message: "Already transcribing, please wait".to_string(),
                            },
                        );
                    }
                }
            }
            "cancel" => {
                let current = state.lock().unwrap().clone();
                if current == DaemonState::Recording {
                    let mut rec = recorder.lock().unwrap();
                    if let Some(ref mut r) = *rec {
                        let _ = r.stop();
                        let _ = r.close();
                    }
                    *rec = None;
                    *state.lock().unwrap() = DaemonState::Idle;
                    broadcast(
                        &clients,
                        &Event::Recording {
                            state: "cancelled".to_string(),
                        },
                    );
                }
            }
            "status" => {
                let current = state.lock().unwrap().clone();
                let model = transcription_manager
                    .get_current_model()
                    .unwrap_or_default();
                let state_str = match current {
                    DaemonState::Idle => "idle",
                    DaemonState::Recording => "recording",
                    DaemonState::Transcribing => "transcribing",
                };
                let event = Event::Status {
                    state: state_str.to_string(),
                    model,
                };
                let json = serde_json::to_string(&event).unwrap_or_default();
                let _ = tx.send(format!("{}\n", json));
            }
            "models" => {
                let models = model_manager.get_available_models();
                let event = Event::Models { models };
                let json = serde_json::to_string(&event).unwrap_or_default();
                let _ = tx.send(format!("{}\n", json));
            }
            "set" => {
                if cmd.key.is_empty() {
                    let event = Event::Error {
                        message: "set requires \"key\" and \"value\" fields".to_string(),
                    };
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    let _ = tx.send(format!("{}\n", json));
                } else {
                    let result = {
                        let mut cfg = config.lock().unwrap();
                        crate::config::set_config_key(&mut cfg, &cmd.key, &cmd.value)
                            .and_then(|_| crate::config::save_config(&cfg))
                    };
                    let event = match result {
                        Ok(_) => Event::Ok {
                            message: format!("Set {} = {}", cmd.key, cmd.value),
                        },
                        Err(e) => Event::Error {
                            message: format!("Failed to set {}: {}", cmd.key, e),
                        },
                    };
                    // Reload model if model.selected changed
                    if cmd.key == "model.selected" && matches!(event, Event::Ok { .. }) {
                        let tm = transcription_manager.clone();
                        let clients_clone = clients.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = tm.ensure_model_loaded() {
                                broadcast(
                                    &clients_clone,
                                    &Event::Error {
                                        message: format!("Model reload failed: {}", e),
                                    },
                                );
                            }
                        });
                    }
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    let _ = tx.send(format!("{}\n", json));
                }
            }
            "shutdown" => {
                shutdown_flag.store(true, Ordering::Relaxed);
                broadcast(&clients, &Event::Shutdown);
                shutdown_notify.notify_one();
                break;
            }
            unknown => {
                broadcast(
                    &clients,
                    &Event::Error {
                        message: format!("Unknown command: {}", unknown),
                    },
                );
            }
        }
    }

    // Clean up this client from the broadcast list
    clients.lock().unwrap().retain(|(id, _)| *id != client_id);
}

// ── Recording helpers ──────────────────────────────────────────────────────────

fn do_start_recording(
    clients: &Clients,
    state: &Arc<Mutex<DaemonState>>,
    recorder: &Arc<Mutex<Option<AudioRecorder>>>,
    config: &Arc<Mutex<Config>>,
) {
    let current = state.lock().unwrap().clone();
    if current != DaemonState::Idle {
        broadcast(
            clients,
            &Event::Error {
                message: "Already recording or transcribing".to_string(),
            },
        );
        return;
    }

    let device_name = config.lock().unwrap().audio.device.clone();
    let device = device_name.as_deref().and_then(find_device_by_name);

    let mut rec = AudioRecorder::new().unwrap();

    if let Err(e) = rec.open(device) {
        broadcast(
            clients,
            &Event::Error {
                message: format!("Failed to open microphone: {}", e),
            },
        );
        return;
    }

    if let Err(e) = rec.start() {
        broadcast(
            clients,
            &Event::Error {
                message: format!("Failed to start recording: {}", e),
            },
        );
        return;
    }

    *recorder.lock().unwrap() = Some(rec);
    *state.lock().unwrap() = DaemonState::Recording;
    crate::audio_toolkit::sound::play(crate::audio_toolkit::sound::Sound::RecordStart);
    broadcast(
        clients,
        &Event::Recording {
            state: "started".to_string(),
        },
    );
    info!("Recording started");
}

fn do_stop_and_transcribe(
    clients: &Clients,
    state: &Arc<Mutex<DaemonState>>,
    recorder: &Arc<Mutex<Option<AudioRecorder>>>,
    transcription_manager: &Arc<TranscriptionManager>,
    _config: &Arc<Mutex<Config>>,
) {
    let current = state.lock().unwrap().clone();
    if current != DaemonState::Recording {
        broadcast(
            clients,
            &Event::Error {
                message: "Not currently recording".to_string(),
            },
        );
        return;
    }

    let audio = {
        let mut rec = recorder.lock().unwrap();
        let audio = rec.as_ref().and_then(|r| r.stop().ok()).unwrap_or_default();
        if let Some(ref mut r) = *rec {
            let _ = r.close();
        }
        *rec = None;
        audio
    };

    crate::audio_toolkit::sound::play(crate::audio_toolkit::sound::Sound::RecordStop);
    broadcast(
        clients,
        &Event::Recording {
            state: "stopped".to_string(),
        },
    );
    *state.lock().unwrap() = DaemonState::Transcribing;
    broadcast(clients, &Event::Transcribing);

    let clients_clone = clients.clone();
    let state_clone = state.clone();
    let tm = transcription_manager.clone();

    std::thread::spawn(move || {
        match tm.transcribe(audio) {
            Ok(text) => {
                broadcast(
                    &clients_clone,
                    &Event::Transcription { text: text.clone() },
                );
                println!("{}", text);
            }
            Err(e) => {
                broadcast(
                    &clients_clone,
                    &Event::Error {
                        message: format!("Transcription failed: {}", e),
                    },
                );
                error!("Transcription error: {}", e);
            }
        }
        *state_clone.lock().unwrap() = DaemonState::Idle;
    });
}

/// Stop recording, transcribe, and paste into the active window (used by hotkey).
fn do_stop_transcribe_paste(
    clients: &Clients,
    state: &Arc<Mutex<DaemonState>>,
    recorder: &Arc<Mutex<Option<AudioRecorder>>>,
    transcription_manager: &Arc<TranscriptionManager>,
    config: &Arc<Mutex<Config>>,
) {
    let current = state.lock().unwrap().clone();
    if current != DaemonState::Recording {
        return;
    }

    let audio = {
        let mut rec = recorder.lock().unwrap();
        let audio = rec.as_ref().and_then(|r| r.stop().ok()).unwrap_or_default();
        if let Some(ref mut r) = *rec {
            let _ = r.close();
        }
        *rec = None;
        audio
    };

    crate::audio_toolkit::sound::play(crate::audio_toolkit::sound::Sound::RecordStop);
    broadcast(
        clients,
        &Event::Recording {
            state: "stopped".to_string(),
        },
    );

    if audio.is_empty() {
        info!("Hotkey: no audio captured");
        *state.lock().unwrap() = DaemonState::Idle;
        return;
    }

    *state.lock().unwrap() = DaemonState::Transcribing;
    broadcast(clients, &Event::Transcribing);

    let clients_clone = clients.clone();
    let state_clone = state.clone();
    let tm = transcription_manager.clone();
    let config_clone = config.clone();

    std::thread::spawn(move || {
        let duration = audio.len() as f64 / 16000.0;
        info!("Hotkey: transcribing ({:.1}s)...", duration);

        match tm.transcribe(audio) {
            Ok(text) if text.trim().is_empty() => {
                info!("Hotkey: empty transcription");
            }
            Ok(text) => {
                broadcast(
                    &clients_clone,
                    &Event::Transcription { text: text.clone() },
                );
                info!("Hotkey: [transcription] {}", text);

                let trailing = config_clone
                    .lock()
                    .unwrap()
                    .output
                    .append_trailing_space;

                std::thread::sleep(std::time::Duration::from_millis(200));

                match crate::hotkey::paste_text(&text, trailing) {
                    Ok(_) => info!("Hotkey: pasted"),
                    Err(e) => error!("Hotkey: paste failed: {}", e),
                }
            }
            Err(e) => {
                broadcast(
                    &clients_clone,
                    &Event::Error {
                        message: format!("Transcription failed: {}", e),
                    },
                );
                error!("Hotkey: transcription error: {}", e);
            }
        }
        *state_clone.lock().unwrap() = DaemonState::Idle;
    });
}

fn find_device_by_name(name: &str) -> Option<cpal::Device> {
    use crate::audio_toolkit::list_input_devices;
    list_input_devices()
        .ok()?
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.device)
}
