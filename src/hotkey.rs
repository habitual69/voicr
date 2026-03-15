//! Global push-to-talk hotkey mode.

use crate::audio_toolkit::{vad::SmoothedVad, AudioRecorder, SileroVad};
use crate::config::Config;
use crate::managers::{model::ModelManager, transcription::TranscriptionManager};
use anyhow::Result;
use log::warn;
use rdev::{listen, Event, EventType, Key};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};

// ── Display server detection ──────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
pub enum DisplayServer {
    X11,
    Wayland,
    Other,
}

pub fn detect_display_server() -> DisplayServer {
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        DisplayServer::Wayland
    } else if std::env::var("DISPLAY").is_ok() {
        DisplayServer::X11
    } else {
        DisplayServer::Other
    }
}

// ── Hotkey combo parsing ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ParsedHotkey {
    pub need_ctrl: bool,
    pub need_alt: bool,
    pub need_shift: bool,
    pub need_meta: bool,
    pub trigger: Key,
    pub trigger_str: String, // original string like "space"
}

pub fn parse_combo(combo: &str) -> Result<ParsedHotkey> {
    let mut need_ctrl = false;
    let mut need_alt = false;
    let mut need_shift = false;
    let mut need_meta = false;
    let mut trigger: Option<Key> = None;
    let mut trigger_str = String::new();

    for part in combo.to_lowercase().split('+') {
        match part.trim() {
            "ctrl" | "control" => need_ctrl = true,
            "alt" => need_alt = true,
            "shift" => need_shift = true,
            "meta" | "super" | "win" | "cmd" | "command" => need_meta = true,
            key => {
                trigger = Some(parse_trigger_key(key)?);
                trigger_str = key.to_string();
            }
        }
    }

    Ok(ParsedHotkey {
        need_ctrl,
        need_alt,
        need_shift,
        need_meta,
        trigger: trigger.ok_or_else(|| anyhow::anyhow!("No trigger key in '{}'", combo))?,
        trigger_str,
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
        c if c.len() == 1 => match c.chars().next().unwrap() {
            'a' => Key::KeyA,
            'b' => Key::KeyB,
            'c' => Key::KeyC,
            'd' => Key::KeyD,
            'e' => Key::KeyE,
            'f' => Key::KeyF,
            'g' => Key::KeyG,
            'h' => Key::KeyH,
            'i' => Key::KeyI,
            'j' => Key::KeyJ,
            'k' => Key::KeyK,
            'l' => Key::KeyL,
            'm' => Key::KeyM,
            'n' => Key::KeyN,
            'o' => Key::KeyO,
            'p' => Key::KeyP,
            'q' => Key::KeyQ,
            'r' => Key::KeyR,
            's' => Key::KeyS,
            't' => Key::KeyT,
            'u' => Key::KeyU,
            'v' => Key::KeyV,
            'w' => Key::KeyW,
            'x' => Key::KeyX,
            'y' => Key::KeyY,
            'z' => Key::KeyZ,
            ch => anyhow::bail!("Unsupported key '{}'", ch),
        },
        other => anyhow::bail!("Unknown key '{}'", other),
    })
}

// ── evdev trigger key mapping (Linux only) ────────────────────────────────────

#[cfg(target_os = "linux")]
fn trigger_str_to_evdev(s: &str) -> Option<evdev::Key> {
    use evdev::Key as EK;
    Some(match s {
        "space" => EK::KEY_SPACE,
        "tab" => EK::KEY_TAB,
        "escape" | "esc" => EK::KEY_ESC,
        "backspace" => EK::KEY_BACKSPACE,
        "return" | "enter" => EK::KEY_ENTER,
        "f1" => EK::KEY_F1,
        "f2" => EK::KEY_F2,
        "f3" => EK::KEY_F3,
        "f4" => EK::KEY_F4,
        "f5" => EK::KEY_F5,
        "f6" => EK::KEY_F6,
        "f7" => EK::KEY_F7,
        "f8" => EK::KEY_F8,
        "f9" => EK::KEY_F9,
        "f10" => EK::KEY_F10,
        "f11" => EK::KEY_F11,
        "f12" => EK::KEY_F12,
        "a" => EK::KEY_A,
        "b" => EK::KEY_B,
        "c" => EK::KEY_C,
        "d" => EK::KEY_D,
        "e" => EK::KEY_E,
        "f" => EK::KEY_F,
        "g" => EK::KEY_G,
        "h" => EK::KEY_H,
        "i" => EK::KEY_I,
        "j" => EK::KEY_J,
        "k" => EK::KEY_K,
        "l" => EK::KEY_L,
        "m" => EK::KEY_M,
        "n" => EK::KEY_N,
        "o" => EK::KEY_O,
        "p" => EK::KEY_P,
        "q" => EK::KEY_Q,
        "r" => EK::KEY_R,
        "s" => EK::KEY_S,
        "t" => EK::KEY_T,
        "u" => EK::KEY_U,
        "v" => EK::KEY_V,
        "w" => EK::KEY_W,
        "x" => EK::KEY_X,
        "y" => EK::KEY_Y,
        "z" => EK::KEY_Z,
        _ => return None,
    })
}

// ── evdev global hotkey listener (Linux) ─────────────────────────────────────

#[cfg(target_os = "linux")]
fn spawn_evdev_listener(hotkey: &ParsedHotkey, tx: mpsc::Sender<()>) -> bool {
    use evdev::{InputEventKind, Key as EK};

    let evdev_trigger = match trigger_str_to_evdev(&hotkey.trigger_str) {
        Some(k) => k,
        None => return false,
    };

    // Find keyboard devices (those that have letter keys)
    let keyboards: Vec<evdev::Device> = evdev::enumerate()
        .filter_map(|(_, dev)| {
            if dev
                .supported_keys()
                .map(|k| k.contains(EK::KEY_A))
                .unwrap_or(false)
            {
                Some(dev)
            } else {
                None
            }
        })
        .collect();

    if keyboards.is_empty() {
        return false; // no access to input devices
    }

    let hk = hotkey.clone();
    for mut device in keyboards {
        let tx2 = tx.clone();
        let hk2 = hk.clone();
        let trigger2 = evdev_trigger;

        std::thread::spawn(move || {
            let mut ctrl = false;
            let mut alt = false;
            let mut shift = false;
            let mut meta = false;

            loop {
                let events = match device.fetch_events() {
                    Ok(e) => e,
                    Err(_) => break,
                };
                for event in events {
                    if let InputEventKind::Key(key) = event.kind() {
                        let press = event.value() == 1;
                        let release = event.value() == 0;

                        match key {
                            EK::KEY_LEFTCTRL | EK::KEY_RIGHTCTRL => {
                                ctrl = if press { true } else if release { false } else { ctrl }
                            }
                            EK::KEY_LEFTALT | EK::KEY_RIGHTALT => {
                                alt = if press { true } else if release { false } else { alt }
                            }
                            EK::KEY_LEFTSHIFT | EK::KEY_RIGHTSHIFT => {
                                shift = if press { true } else if release { false } else { shift }
                            }
                            EK::KEY_LEFTMETA | EK::KEY_RIGHTMETA => {
                                meta = if press { true } else if release { false } else { meta }
                            }
                            _ => {}
                        }

                        if press && key == trigger2 {
                            let ok = (!hk2.need_ctrl || ctrl)
                                && (!hk2.need_alt || alt)
                                && (!hk2.need_shift || shift)
                                && (!hk2.need_meta || meta);
                            if ok {
                                let _ = tx2.send(());
                            }
                        }
                    }
                }
            }
        });
    }
    true
}

// ── Paste ─────────────────────────────────────────────────────────────────────

pub fn paste_text(text: &str, append_trailing_space: bool) -> Result<()> {
    let pasted = if append_trailing_space {
        format!("{} ", text)
    } else {
        text.to_string()
    };

    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("clipboard: {}", e))?;
    clipboard
        .set_text(&pasted)
        .map_err(|e| anyhow::anyhow!("clipboard write: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(80));
    simulate_paste()
}

fn run_cmd(prog: &str, args: &[&str]) -> bool {
    std::process::Command::new(prog)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn simulate_paste() -> Result<()> {
    if run_cmd(
        "osascript",
        &[
            "-e",
            "tell application \"System Events\" to keystroke \"v\" using {command down}",
        ],
    ) {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "paste failed — text is in clipboard, paste with Cmd+V"
    ))
}

#[cfg(target_os = "windows")]
fn simulate_paste() -> Result<()> {
    if run_cmd(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('^v')",
        ],
    ) {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "paste failed — text is in clipboard, paste with Ctrl+V"
    ))
}

#[cfg(target_os = "linux")]
fn simulate_paste() -> Result<()> {
    use rdev::{simulate, EventType, Key};

    let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    // wtype: best option for Wayland
    if run_cmd("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"]) {
        return Ok(());
    }
    // ydotool: alternative Wayland tool
    if run_cmd("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]) {
        return Ok(());
    }

    if !is_wayland {
        // X11: rdev simulate
        let d = std::time::Duration::from_millis(20);
        if simulate(&EventType::KeyPress(Key::ControlLeft)).is_ok() {
            std::thread::sleep(d);
            simulate(&EventType::KeyPress(Key::KeyV)).ok();
            std::thread::sleep(d);
            simulate(&EventType::KeyRelease(Key::KeyV)).ok();
            std::thread::sleep(d);
            simulate(&EventType::KeyRelease(Key::ControlLeft)).ok();
            return Ok(());
        }
        // xdotool fallback
        if run_cmd("xdotool", &["key", "--clearmodifiers", "ctrl+v"]) {
            return Ok(());
        }
    }

    // Text is in clipboard — user can paste manually
    Err(anyhow::anyhow!(
        "no paste tool available\n  Install:  sudo apt install wtype\n  Text is in clipboard — paste with Ctrl+V"
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn simulate_paste() -> Result<()> {
    Err(anyhow::anyhow!("paste not supported on this platform"))
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
    let ds = detect_display_server();

    // ── Channel shared by all trigger sources ─────────────────────────────────
    let (tx, rx) = mpsc::channel::<()>();

    // global_hotkey_ok tracks whether a global listener was successfully started
    let mut global_hotkey_ok = false;

    // ── Source 1: evdev (Linux — works on Wayland + X11, needs input group) ──
    #[cfg(target_os = "linux")]
    {
        if spawn_evdev_listener(&hotkey, tx.clone()) {
            global_hotkey_ok = true;
        }
    }

    // ── Source 2: rdev (macOS / Windows / Linux X11 fallback) ────────────────
    #[cfg(not(target_os = "linux"))]
    {
        let ctrl = Arc::new(AtomicBool::new(false));
        let alt = Arc::new(AtomicBool::new(false));
        let shift = Arc::new(AtomicBool::new(false));
        let meta = Arc::new(AtomicBool::new(false));
        let (c2, a2, sh2, m2) = (ctrl.clone(), alt.clone(), shift.clone(), meta.clone());
        let hotkey2 = hotkey.clone();
        let tx2 = tx.clone();

        std::thread::spawn(move || {
            let cb = move |event: Event| {
                match event.event_type {
                    EventType::KeyPress(key) => {
                        match key {
                            Key::ControlLeft | Key::ControlRight => {
                                c2.store(true, Ordering::Relaxed)
                            }
                            Key::Alt | Key::AltGr => a2.store(true, Ordering::Relaxed),
                            Key::ShiftLeft | Key::ShiftRight => {
                                sh2.store(true, Ordering::Relaxed)
                            }
                            Key::MetaLeft | Key::MetaRight => m2.store(true, Ordering::Relaxed),
                            _ => {}
                        }
                        if key == hotkey2.trigger {
                            let ok = (!hotkey2.need_ctrl || c2.load(Ordering::Relaxed))
                                && (!hotkey2.need_alt || a2.load(Ordering::Relaxed))
                                && (!hotkey2.need_shift || sh2.load(Ordering::Relaxed))
                                && (!hotkey2.need_meta || m2.load(Ordering::Relaxed));
                            if ok {
                                let _ = tx2.send(());
                            }
                        }
                    }
                    EventType::KeyRelease(key) => match key {
                        Key::ControlLeft | Key::ControlRight => {
                            c2.store(false, Ordering::Relaxed)
                        }
                        Key::Alt | Key::AltGr => a2.store(false, Ordering::Relaxed),
                        Key::ShiftLeft | Key::ShiftRight => sh2.store(false, Ordering::Relaxed),
                        Key::MetaLeft | Key::MetaRight => m2.store(false, Ordering::Relaxed),
                        _ => {}
                    },
                    _ => {}
                }
            };
            if let Err(e) = listen(cb) {
                warn!("rdev listener stopped: {:?}", e);
            }
        });
        global_hotkey_ok = true;
    }

    // On Linux also run rdev as a fallback for X11 sessions when evdev failed
    #[cfg(target_os = "linux")]
    if !global_hotkey_ok && ds == DisplayServer::X11 {
        let ctrl = Arc::new(AtomicBool::new(false));
        let alt = Arc::new(AtomicBool::new(false));
        let shift = Arc::new(AtomicBool::new(false));
        let meta = Arc::new(AtomicBool::new(false));
        let (c2, a2, sh2, m2) = (ctrl.clone(), alt.clone(), shift.clone(), meta.clone());
        let hotkey2 = hotkey.clone();
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            let cb = move |event: Event| {
                match event.event_type {
                    EventType::KeyPress(key) => {
                        match key {
                            Key::ControlLeft | Key::ControlRight => {
                                c2.store(true, Ordering::Relaxed)
                            }
                            Key::Alt | Key::AltGr => a2.store(true, Ordering::Relaxed),
                            Key::ShiftLeft | Key::ShiftRight => {
                                sh2.store(true, Ordering::Relaxed)
                            }
                            Key::MetaLeft | Key::MetaRight => m2.store(true, Ordering::Relaxed),
                            _ => {}
                        }
                        if key == hotkey2.trigger {
                            let ok = (!hotkey2.need_ctrl || c2.load(Ordering::Relaxed))
                                && (!hotkey2.need_alt || a2.load(Ordering::Relaxed))
                                && (!hotkey2.need_shift || sh2.load(Ordering::Relaxed))
                                && (!hotkey2.need_meta || m2.load(Ordering::Relaxed));
                            if ok {
                                let _ = tx2.send(());
                            }
                        }
                    }
                    EventType::KeyRelease(key) => match key {
                        Key::ControlLeft | Key::ControlRight => {
                            c2.store(false, Ordering::Relaxed)
                        }
                        Key::Alt | Key::AltGr => a2.store(false, Ordering::Relaxed),
                        Key::ShiftLeft | Key::ShiftRight => sh2.store(false, Ordering::Relaxed),
                        Key::MetaLeft | Key::MetaRight => m2.store(false, Ordering::Relaxed),
                        _ => {}
                    },
                    _ => {}
                }
            };
            if let Err(e) = listen(cb) {
                warn!("rdev (X11) listener stopped: {:?}", e);
            }
        });
        global_hotkey_ok = true;
    }

    // ── Source 3: stdin Enter key (always available) ──────────────────────────
    {
        let tx_stdin = tx.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for _line in std::io::stdin().lock().lines().flatten() {
                let _ = tx_stdin.send(());
            }
        });
    }

    // ── Startup banner ────────────────────────────────────────────────────────
    eprintln!();
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!(" voicr  push-to-talk");
    eprintln!("─────────────────────────────────────");

    if global_hotkey_ok {
        eprintln!(" Hotkey  : [{}]  (global)", combo_str);
    } else {
        eprintln!(" Hotkey  : [{}]  not active", combo_str);
        match ds {
            DisplayServer::Wayland => {
                eprintln!("           Wayland: add yourself to the input group:");
                eprintln!("           sudo usermod -aG input $USER  (then re-login)");
            }
            _ => {
                eprintln!("           Could not start global listener.");
            }
        }
    }

    eprintln!(" Fallback: press Enter here to toggle");
    if no_paste {
        eprintln!(" Output  : stdout (--no-paste)");
    } else {
        eprintln!(" Output  : paste into active window");
    }
    eprintln!("─────────────────────────────────────");
    eprintln!(" Ctrl+C to quit");
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!();

    // ── Build transcription manager ───────────────────────────────────────────
    let status_cb: crate::managers::transcription::StatusCallback = Arc::new(|status| {
        use crate::managers::transcription::ModelStatus;
        match status {
            ModelStatus::Loading { model_id } => eprintln!("[model] loading {}...", model_id),
            ModelStatus::Loaded { model_name, .. } => {
                eprintln!("[model] {} ready\n", model_name)
            }
            ModelStatus::Unloaded => {}
            ModelStatus::Error { message, .. } => eprintln!("[model] error: {}", message),
        }
    });

    let tm = Arc::new(TranscriptionManager::new(
        model_manager,
        config.clone(),
        Some(status_cb),
    )?);
    tm.ensure_model_loaded()?;

    eprintln!(
        "[ready] press [{}] or Enter to start recording",
        combo_str
    );
    eprintln!();

    // ── Toggle loop ───────────────────────────────────────────────────────────
    let mut is_recording = false;
    let mut recorder: Option<AudioRecorder> = None;

    for () in &rx {
        if !is_recording {
            let (vad_enabled, vad_threshold, device_name) = {
                let cfg = config.lock().unwrap();
                (
                    cfg.audio.vad_enabled,
                    cfg.audio.vad_threshold,
                    cfg.audio.device.clone(),
                )
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
                Err(e) => {
                    eprintln!("[error] {}", e);
                    continue;
                }
            };

            if vad_enabled {
                if let Some(ref path) = vad_path {
                    if path.exists() {
                        if let Ok(silero) = SileroVad::new(path, vad_threshold) {
                            let smoothed = SmoothedVad::new(Box::new(silero), 15, 15, 2);
                            rec = rec.with_vad(Box::new(smoothed));
                        } else {
                            warn!("VAD model failed to load");
                        }
                    }
                }
            }

            if let Err(e) = rec.open(device).map_err(|e| anyhow::anyhow!("{}", e)) {
                eprintln!("[error] microphone: {}", e);
                continue;
            }
            if let Err(e) = rec.start().map_err(|e| anyhow::anyhow!("{}", e)) {
                eprintln!("[error] start: {}", e);
                continue;
            }

            recorder = Some(rec);
            is_recording = true;
            eprintln!(
                "recording  — press [{}] or Enter to stop",
                combo_str
            );
        } else {
            is_recording = false;
            let audio = if let Some(mut rec) = recorder.take() {
                let a = rec.stop().unwrap_or_default();
                let _ = rec.close();
                a
            } else {
                vec![]
            };

            if audio.is_empty() {
                eprintln!("[warning] no audio");
                eprintln!("[ready] press [{}] or Enter to record", combo_str);
                continue;
            }

            eprintln!("stopped  — transcribing...");

            let tm_clone = tm.clone();
            let config_clone = config.clone();
            let combo_disp = combo_str.clone();

            std::thread::spawn(move || {
                match tm_clone.transcribe(audio) {
                    Ok(t) if t.trim().is_empty() => eprintln!("[transcription] (empty)"),
                    Ok(text) => {
                        eprintln!("[transcription] {}", text);
                        if no_paste {
                            println!("{}", text);
                        } else {
                            let trailing = config_clone
                                .lock()
                                .unwrap()
                                .output
                                .append_trailing_space;
                            match paste_text(&text, trailing) {
                                Ok(_) => eprintln!("[pasted]"),
                                Err(e) => eprintln!("[paste] {}", e),
                            }
                        }
                    }
                    Err(e) => eprintln!("[error] {}", e),
                }
                eprintln!();
                eprintln!(
                    "[ready] press [{}] or Enter to record again",
                    combo_disp
                );
            });
        }
    }
    Ok(())
}
