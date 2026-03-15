//! Global push-to-talk hotkey mode.
//!
//! Press the hotkey to start recording; press again to stop, transcribe, and
//! paste the result into the currently focused text field.

use crate::audio_toolkit::{vad::SmoothedVad, AudioRecorder, SileroVad};
use crate::config::Config;
use crate::managers::{model::ModelManager, transcription::TranscriptionManager};
use anyhow::Result;
use log::{error, warn};
use rdev::{listen, simulate, Event, EventType, Key};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};

// ── Hotkey combo parsing ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ParsedHotkey {
    need_ctrl: bool,
    need_alt: bool,
    need_shift: bool,
    need_meta: bool,
    trigger: Key,
}

pub fn parse_combo(combo: &str) -> Result<ParsedHotkey> {
    let mut need_ctrl = false;
    let mut need_alt = false;
    let mut need_shift = false;
    let mut need_meta = false;
    let mut trigger: Option<Key> = None;

    for part in combo.to_lowercase().split('+') {
        match part.trim() {
            "ctrl" | "control" => need_ctrl = true,
            "alt" => need_alt = true,
            "shift" => need_shift = true,
            "meta" | "super" | "win" | "cmd" | "command" => need_meta = true,
            key => trigger = Some(parse_trigger_key(key)?),
        }
    }

    Ok(ParsedHotkey {
        need_ctrl,
        need_alt,
        need_shift,
        need_meta,
        trigger: trigger
            .ok_or_else(|| anyhow::anyhow!("No trigger key in combo '{}'. Example: ctrl+space", combo))?,
    })
}

fn parse_trigger_key(s: &str) -> Result<Key> {
    Ok(match s {
        "space" => Key::Space,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "return" | "enter" => Key::Return,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        c if c.len() == 1 => {
            let ch = c.chars().next().unwrap();
            match ch {
                'a' => Key::KeyA, 'b' => Key::KeyB, 'c' => Key::KeyC,
                'd' => Key::KeyD, 'e' => Key::KeyE, 'f' => Key::KeyF,
                'g' => Key::KeyG, 'h' => Key::KeyH, 'i' => Key::KeyI,
                'j' => Key::KeyJ, 'k' => Key::KeyK, 'l' => Key::KeyL,
                'm' => Key::KeyM, 'n' => Key::KeyN, 'o' => Key::KeyO,
                'p' => Key::KeyP, 'q' => Key::KeyQ, 'r' => Key::KeyR,
                's' => Key::KeyS, 't' => Key::KeyT, 'u' => Key::KeyU,
                'v' => Key::KeyV, 'w' => Key::KeyW, 'x' => Key::KeyX,
                'y' => Key::KeyY, 'z' => Key::KeyZ,
                _ => anyhow::bail!("Unsupported trigger key: '{}'", ch),
            }
        }
        other => anyhow::bail!("Unknown key: '{}'", other),
    })
}

// ── Paste ─────────────────────────────────────────────────────────────────────

/// Copy text to clipboard then simulate Ctrl+V (or Cmd+V on macOS).
pub fn paste_text(text: &str, append_trailing_space: bool) -> Result<()> {
    let pasted = if append_trailing_space {
        format!("{} ", text)
    } else {
        text.to_string()
    };

    // Write to system clipboard
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("clipboard unavailable: {}", e))?;
    clipboard
        .set_text(&pasted)
        .map_err(|e| anyhow::anyhow!("clipboard write: {}", e))?;

    // Allow the OS to propagate the clipboard update before sending the paste key
    std::thread::sleep(std::time::Duration::from_millis(80));

    // Simulate modifier + V
    #[cfg(target_os = "macos")]
    let modifier = Key::MetaLeft; // Cmd
    #[cfg(not(target_os = "macos"))]
    let modifier = Key::ControlLeft; // Ctrl

    let delay = std::time::Duration::from_millis(20);

    simulate(&EventType::KeyPress(modifier))
        .map_err(|e| anyhow::anyhow!("simulate keypress: {:?}", e))?;
    std::thread::sleep(delay);
    simulate(&EventType::KeyPress(Key::KeyV))
        .map_err(|e| anyhow::anyhow!("simulate keypress: {:?}", e))?;
    std::thread::sleep(delay);
    simulate(&EventType::KeyRelease(Key::KeyV))
        .map_err(|e| anyhow::anyhow!("simulate keyrelease: {:?}", e))?;
    std::thread::sleep(delay);
    simulate(&EventType::KeyRelease(modifier))
        .map_err(|e| anyhow::anyhow!("simulate keyrelease: {:?}", e))?;

    Ok(())
}

// ── Main hotkey runner ────────────────────────────────────────────────────────

pub fn run_hotkey(
    config: Arc<Mutex<Config>>,
    model_manager: Arc<ModelManager>,
    vad_path: Option<std::path::PathBuf>,
    combo_override: Option<String>,
    no_paste: bool,
) -> Result<()> {
    let combo_str = combo_override
        .unwrap_or_else(|| config.lock().unwrap().hotkey.combo.clone());

    let hotkey = parse_combo(&combo_str)?;

    eprintln!("Push-to-talk ready  [{combo_str}] — press to start / press again to stop");
    if no_paste {
        eprintln!("Transcriptions will be printed to stdout.");
    } else {
        eprintln!("Transcriptions will be pasted into the active window.");
    }
    eprintln!("Press Ctrl+C to quit.\n");

    // Channel: hotkey listener → main loop
    let (tx, rx) = mpsc::channel::<()>();

    // Modifier state (shared with rdev callback)
    let ctrl  = Arc::new(AtomicBool::new(false));
    let alt   = Arc::new(AtomicBool::new(false));
    let shift = Arc::new(AtomicBool::new(false));
    let meta  = Arc::new(AtomicBool::new(false));

    let (ctrl2, alt2, shift2, meta2) = (ctrl.clone(), alt.clone(), shift.clone(), meta.clone());
    let hotkey2 = hotkey.clone();
    let tx2 = tx.clone();

    // rdev::listen blocks forever — run in a dedicated thread
    std::thread::spawn(move || {
        let cb = move |event: Event| {
            match event.event_type {
                EventType::KeyPress(key) => {
                    match key {
                        Key::ControlLeft | Key::ControlRight => ctrl2.store(true, Ordering::Relaxed),
                        Key::Alt | Key::AltGr            => alt2.store(true, Ordering::Relaxed),
                        Key::ShiftLeft | Key::ShiftRight => shift2.store(true, Ordering::Relaxed),
                        Key::MetaLeft  | Key::MetaRight  => meta2.store(true, Ordering::Relaxed),
                        _ => {}
                    }

                    if key == hotkey2.trigger {
                        let modifiers_ok =
                            (!hotkey2.need_ctrl  || ctrl2.load(Ordering::Relaxed))
                            && (!hotkey2.need_alt   || alt2.load(Ordering::Relaxed))
                            && (!hotkey2.need_shift || shift2.load(Ordering::Relaxed))
                            && (!hotkey2.need_meta  || meta2.load(Ordering::Relaxed));
                        if modifiers_ok {
                            let _ = tx2.send(());
                        }
                    }
                }
                EventType::KeyRelease(key) => match key {
                    Key::ControlLeft | Key::ControlRight => ctrl2.store(false, Ordering::Relaxed),
                    Key::Alt | Key::AltGr            => alt2.store(false, Ordering::Relaxed),
                    Key::ShiftLeft | Key::ShiftRight => shift2.store(false, Ordering::Relaxed),
                    Key::MetaLeft  | Key::MetaRight  => meta2.store(false, Ordering::Relaxed),
                    _ => {}
                },
                _ => {}
            }
        };

        if let Err(e) = listen(cb) {
            error!("Global hotkey listener failed: {:?}", e);
            eprintln!("\n[error] hotkey listener stopped: {:?}", e);
            #[cfg(target_os = "linux")]
            eprintln!(
                "  On Linux/X11 make sure you are not running as root and libxtst is installed.\n\
                 On Wayland, add yourself to the 'input' group:\n\
                 \tsudo usermod -aG input $USER\n\
                 then log out and back in."
            );
            #[cfg(target_os = "macos")]
            eprintln!(
                "  On macOS, grant Accessibility access to your terminal:\n\
                 System Settings → Privacy & Security → Accessibility"
            );
        }
    });

    // Build transcription manager once
    let status_cb: crate::managers::transcription::StatusCallback = Arc::new(|status| {
        use crate::managers::transcription::ModelStatus;
        match status {
            ModelStatus::Loading { model_id }       => eprintln!("[model] loading {}…", model_id),
            ModelStatus::Loaded { model_name, .. }  => eprintln!("[model] {} ready\n", model_name),
            ModelStatus::Unloaded                   => {}
            ModelStatus::Error { message, .. }      => eprintln!("[model] error: {}", message),
        }
    });

    let tm = Arc::new(
        TranscriptionManager::new(model_manager, config.clone(), Some(status_cb))?
    );
    tm.ensure_model_loaded()?;

    // ── Toggle loop ───────────────────────────────────────────────────────────
    let mut is_recording = false;
    let mut recorder: Option<AudioRecorder> = None;

    for () in &rx {
        if !is_recording {
            // ── Start recording ───────────────────────────────────────────────
            let (vad_enabled, vad_threshold, device_name) = {
                let cfg = config.lock().unwrap();
                (cfg.audio.vad_enabled, cfg.audio.vad_threshold, cfg.audio.device.clone())
            };

            let device = device_name.as_deref().and_then(|n| {
                crate::audio_toolkit::list_input_devices()
                    .ok()?
                    .into_iter()
                    .find(|d| d.name == n)
                    .map(|d| d.device)
            });

            let mut rec = match AudioRecorder::new().map_err(|e| anyhow::anyhow!("{}", e)) {
                Ok(r) => r,
                Err(e) => { eprintln!("[error] {}", e); continue; }
            };

            if vad_enabled {
                if let Some(ref path) = vad_path {
                    if path.exists() {
                        if let Ok(silero) = SileroVad::new(path, vad_threshold) {
                            let smoothed = SmoothedVad::new(Box::new(silero), 15, 15, 2);
                            rec = rec.with_vad(Box::new(smoothed));
                        } else {
                            warn!("VAD model failed to load, recording without VAD");
                        }
                    }
                }
            }

            if let Err(e) = rec.open(device).map_err(|e| anyhow::anyhow!("{}", e)) {
                eprintln!("[error] microphone: {}", e);
                continue;
            }
            if let Err(e) = rec.start().map_err(|e| anyhow::anyhow!("{}", e)) {
                eprintln!("[error] start recording: {}", e);
                continue;
            }

            recorder = Some(rec);
            is_recording = true;
            eprintln!("[recording] started — press [{combo_str}] to stop");
        } else {
            // ── Stop, transcribe, paste ───────────────────────────────────────
            is_recording = false;
            let audio = if let Some(mut rec) = recorder.take() {
                let a = rec.stop().unwrap_or_default();
                let _ = rec.close();
                a
            } else {
                vec![]
            };

            if audio.is_empty() {
                eprintln!("[warning] no audio captured");
                eprintln!("[ready] press [{combo_str}] to record");
                continue;
            }

            eprintln!("[recording] stopped — transcribing…");

            let tm_clone     = tm.clone();
            let config_clone = config.clone();
            let combo_disp   = combo_str.clone();

            std::thread::spawn(move || {
                match tm_clone.transcribe(audio) {
                    Ok(text) if text.is_empty() => {
                        eprintln!("[transcription] (empty)");
                    }
                    Ok(text) => {
                        eprintln!("[transcription] {}", text);
                        if no_paste {
                            println!("{}", text);
                        } else {
                            let trailing = config_clone
                                .lock().unwrap()
                                .output.append_trailing_space;
                            if let Err(e) = paste_text(&text, trailing) {
                                eprintln!(
                                    "[paste] failed: {}\n  Text copied to clipboard — paste manually with Ctrl+V",
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => eprintln!("[error] transcription: {}", e),
                }
                eprintln!("[ready] press [{combo_disp}] to record again");
            });
        }
    }

    Ok(())
}
