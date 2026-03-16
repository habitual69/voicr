/// Voicr daemon – listens on a Unix socket and processes voice commands.
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

#[cfg(unix)]
use crate::audio_toolkit::{vad::SmoothedVad, AudioRecorder, SileroVad};
use crate::config::Config;
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use anyhow::Result;
use log::{error, info};
#[cfg(unix)]
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use std::io::Write;
#[cfg(unix)]
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

// ── Protocol types ────────────────────────────────────────────────────────────

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Deserialize)]
struct Command {
    cmd: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
}

#[cfg_attr(not(unix), allow(dead_code))]
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

// ── Daemon state ──────────────────────────────────────────────────────────────

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Clone, PartialEq)]
enum DaemonState {
    Idle,
    Recording,
    Transcribing,
}

type Clients = Arc<Mutex<Vec<Arc<Mutex<Box<dyn Write + Send>>>>>>;

fn broadcast(clients: &Clients, event: &Event) {
    let json = match serde_json::to_string(event) {
        Ok(j) => format!("{}\n", j),
        Err(e) => {
            error!("Failed to serialize event: {}", e);
            return;
        }
    };

    let mut list = clients.lock().unwrap();
    list.retain(|client| {
        let mut c = client.lock().unwrap();
        c.write_all(json.as_bytes()).is_ok()
    });
}

// ── Main daemon entry point ───────────────────────────────────────────────────

pub fn run_daemon(
    socket_path: PathBuf,
    config: Arc<Mutex<Config>>,
    model_manager: Arc<ModelManager>,
    vad_model_path: PathBuf,
) -> Result<()> {
    // Remove stale socket
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    let clients: Clients = Arc::new(Mutex::new(Vec::new()));
    let shutdown_flag = Arc::new(AtomicBool::new(false));

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
    let socket_path_clone = socket_path.clone();
    ctrlc::set_handler(move || {
        info!("Shutdown signal received");
        shutdown_clone.store(true, Ordering::Relaxed);
        // Touch socket to unblock accept()
        let _ = std::net::TcpStream::connect("127.0.0.1:0");
        drop(std::fs::remove_file(&socket_path_clone));
    })
    .ok();

    #[cfg(unix)]
    {
        let state = Arc::new(Mutex::new(DaemonState::Idle));
        let recorder: Arc<Mutex<Option<AudioRecorder>>> = Arc::new(Mutex::new(None));

        let listener = UnixListener::bind(&socket_path)?;
        info!("Daemon listening on {:?}", socket_path);
        eprintln!("voicr daemon running. Socket: {}", socket_path.display());
        eprintln!("Send commands via: echo '{{\"cmd\":\"toggle\"}}' | nc -U {}", socket_path.display());

        for stream in listener.incoming() {
            if shutdown_flag.load(Ordering::Relaxed) {
                break;
            }

            match stream {
                Ok(stream) => {
                    let clients_clone = clients.clone();
                    let state_clone = state.clone();
                    let tm_clone = transcription_manager.clone();
                    let mm_clone = model_manager.clone();
                    let config_clone = config.clone();
                    let recorder_clone = recorder.clone();
                    let vad_path_clone = vad_model_path.clone();
                    let shutdown_clone = shutdown_flag.clone();

                    std::thread::spawn(move || {
                        handle_client(
                            stream,
                            clients_clone,
                            state_clone,
                            tm_clone,
                            mm_clone,
                            config_clone,
                            recorder_clone,
                            vad_path_clone,
                            shutdown_clone,
                        );
                    });
                }
                Err(e) => {
                    if !shutdown_flag.load(Ordering::Relaxed) {
                        error!("Accept error: {}", e);
                    }
                }
            }
        }

        broadcast(&clients, &Event::Shutdown);
        info!("Daemon shut down");
        return Ok(());
    }

    #[cfg(not(unix))]
    {
        let _ = &vad_model_path;
        anyhow::bail!("Daemon mode is only supported on Unix systems");
    }
}

#[cfg(unix)]
fn handle_client(
    stream: UnixStream,
    clients: Clients,
    state: Arc<Mutex<DaemonState>>,
    transcription_manager: Arc<TranscriptionManager>,
    model_manager: Arc<crate::managers::model::ModelManager>,
    config: Arc<Mutex<Config>>,
    recorder: Arc<Mutex<Option<AudioRecorder>>>,
    vad_model_path: PathBuf,
    shutdown_flag: Arc<AtomicBool>,
) {
    let write_half = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to clone stream: {}", e);
            return;
        }
    };

    let client_writer: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(Box::new(write_half)));
    clients.lock().unwrap().push(client_writer.clone());

    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let cmd: Command = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                let event = Event::Error {
                    message: format!("Invalid command JSON: {}", e),
                };
                let json = serde_json::to_string(&event).unwrap_or_default();
                let _ = client_writer
                    .lock()
                    .unwrap()
                    .write_all(format!("{}\n", json).as_bytes());
                continue;
            }
        };

        debug!("Received command: {}", cmd.cmd);

        match cmd.cmd.as_str() {
            "start" => {
                do_start_recording(
                    &clients,
                    &state,
                    &recorder,
                    &vad_model_path,
                    &config,
                );
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
                    DaemonState::Idle => do_start_recording(
                        &clients,
                        &state,
                        &recorder,
                        &vad_model_path,
                        &config,
                    ),
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
                        let _ = r.stop(); // discard audio
                        let _ = r.close();
                    }
                    *rec = None;
                    *state.lock().unwrap() = DaemonState::Idle;
                    broadcast(&clients, &Event::Recording { state: "cancelled".to_string() });
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
                let _ = client_writer
                    .lock()
                    .unwrap()
                    .write_all(format!("{}\n", json).as_bytes());
            }
            "models" => {
                let models = model_manager.get_available_models();
                let event = Event::Models { models };
                let json = serde_json::to_string(&event).unwrap_or_default();
                let _ = client_writer
                    .lock()
                    .unwrap()
                    .write_all(format!("{}\n", json).as_bytes());
            }
            "set" => {
                if cmd.key.is_empty() {
                    let event = Event::Error {
                        message: "set requires \"key\" and \"value\" fields".to_string(),
                    };
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    let _ = client_writer
                        .lock()
                        .unwrap()
                        .write_all(format!("{}\n", json).as_bytes());
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
                    let _ = client_writer
                        .lock()
                        .unwrap()
                        .write_all(format!("{}\n", json).as_bytes());
                }
            }
            "shutdown" => {
                shutdown_flag.store(true, Ordering::Relaxed);
                broadcast(&clients, &Event::Shutdown);
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

    // Remove this client from the list
    let mut list = clients.lock().unwrap();
    list.retain(|c| !Arc::ptr_eq(c, &client_writer));
}

#[cfg(unix)]
fn do_start_recording(
    clients: &Clients,
    state: &Arc<Mutex<DaemonState>>,
    recorder: &Arc<Mutex<Option<AudioRecorder>>>,
    vad_model_path: &PathBuf,
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

    let (vad_enabled, vad_threshold, device_name) = {
        let cfg = config.lock().unwrap();
        (
            cfg.audio.vad_enabled,
            cfg.audio.vad_threshold,
            cfg.audio.device.clone(),
        )
    };

    let device = device_name.as_deref().and_then(find_device_by_name);

    let mut rec = AudioRecorder::new().unwrap();

    if vad_enabled && vad_model_path.exists() {
        if let Ok(silero) = SileroVad::new(vad_model_path, vad_threshold) {
            let smoothed = SmoothedVad::new(Box::new(silero), 15, 15, 2);
            rec = rec.with_vad(Box::new(smoothed));
        } else {
            warn!("Failed to load VAD model, recording without VAD");
        }
    }

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
    broadcast(clients, &Event::Recording { state: "started".to_string() });
    info!("Recording started");
}

#[cfg(unix)]
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

    // Stop recording
    let audio = {
        let mut rec = recorder.lock().unwrap();
        let audio = rec.as_ref().and_then(|r| r.stop().ok()).unwrap_or_default();
        if let Some(ref mut r) = *rec {
            let _ = r.close();
        }
        *rec = None;
        audio
    };

    broadcast(clients, &Event::Recording { state: "stopped".to_string() });
    *state.lock().unwrap() = DaemonState::Transcribing;
    broadcast(clients, &Event::Transcribing);

    // Transcribe in a background thread
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
                // Also print to stdout for piping
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

#[cfg(unix)]
fn find_device_by_name(name: &str) -> Option<cpal::Device> {
    use crate::audio_toolkit::list_input_devices;
    list_input_devices()
        .ok()?
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.device)
}
