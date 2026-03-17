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
    #[allow(dead_code)]
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

/// Hold-to-talk signal: `true` = key pressed (start), `false` = key released (stop).
pub type HotkeySignal = bool;

/// Public wrapper for daemon use.
#[cfg(target_os = "linux")]
pub fn spawn_evdev_listener_pub(hotkey: &ParsedHotkey, tx: mpsc::Sender<HotkeySignal>) -> bool {
    spawn_evdev_listener(hotkey, tx)
}

#[cfg(target_os = "linux")]
fn spawn_evdev_listener(hotkey: &ParsedHotkey, tx: mpsc::Sender<HotkeySignal>) -> bool {
    use evdev::{InputEventKind, Key as EK};

    let evdev_trigger = match trigger_str_to_evdev(&hotkey.trigger_str) {
        Some(k) => k,
        None => return false,
    };

    // Find keyboard devices (those that have letter keys)
    let keyboards: Vec<evdev::Device> = evdev::enumerate()
        .filter_map(|(_, dev)| {
            // Skip virtual/uinput keyboards (ydotoold, voicr-paste, dotool, etc.)
            let name = dev.name().unwrap_or("").to_lowercase();
            if name.contains("virtual")
                || name.contains("ydotool")
                || name.contains("dotool")
                || name.contains("voicr")
                || name.contains("uinput")
            {
                return None;
            }
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
        return false;
    }

    // Shared modifier state across ALL keyboard devices.
    let ctrl = Arc::new(AtomicBool::new(false));
    let alt = Arc::new(AtomicBool::new(false));
    let shift = Arc::new(AtomicBool::new(false));
    let meta = Arc::new(AtomicBool::new(false));
    // Track whether the hotkey combo is currently active (held down)
    let combo_active = Arc::new(AtomicBool::new(false));

    let hk = hotkey.clone();
    for mut device in keyboards {
        let tx2 = tx.clone();
        let hk2 = hk.clone();
        let trigger2 = evdev_trigger;
        let (c, a, s, m) = (ctrl.clone(), alt.clone(), shift.clone(), meta.clone());
        let active = combo_active.clone();

        std::thread::spawn(move || {
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
                                if press { c.store(true, Ordering::Relaxed); }
                                else if release { c.store(false, Ordering::Relaxed); }
                            }
                            EK::KEY_LEFTALT | EK::KEY_RIGHTALT => {
                                if press { a.store(true, Ordering::Relaxed); }
                                else if release { a.store(false, Ordering::Relaxed); }
                            }
                            EK::KEY_LEFTSHIFT | EK::KEY_RIGHTSHIFT => {
                                if press { s.store(true, Ordering::Relaxed); }
                                else if release { s.store(false, Ordering::Relaxed); }
                            }
                            EK::KEY_LEFTMETA | EK::KEY_RIGHTMETA => {
                                if press { m.store(true, Ordering::Relaxed); }
                                else if release { m.store(false, Ordering::Relaxed); }
                            }
                            _ => {}
                        }

                        // Hold-to-talk: trigger key pressed with modifiers → start
                        if press && key == trigger2 {
                            let ok = (!hk2.need_ctrl || c.load(Ordering::Relaxed))
                                && (!hk2.need_alt || a.load(Ordering::Relaxed))
                                && (!hk2.need_shift || s.load(Ordering::Relaxed))
                                && (!hk2.need_meta || m.load(Ordering::Relaxed));
                            if ok && !active.swap(true, Ordering::Relaxed) {
                                let _ = tx2.send(true); // start recording
                            }
                        }

                        // Trigger key released → stop
                        if release && key == trigger2 && active.swap(false, Ordering::Relaxed) {
                            let _ = tx2.send(false); // stop recording
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

    #[cfg(target_os = "linux")]
    return paste_linux(&pasted);

    #[cfg(target_os = "macos")]
    return paste_macos(&pasted);

    #[cfg(target_os = "windows")]
    return paste_windows(&pasted);

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!("paste not supported on this platform"))
}

/// Linux paste — same fallback chain used by Handy:
///   1. dotool   stdin pipe, no daemon, uinput-based, works everywhere
///   2. ydotool  uinput via daemon, works everywhere incl. GNOME Wayland
///   3. wtype    Wayland virtual keyboard (wlroots: sway/hyprland, NOT GNOME)
///   4. xdotool  X11 / XWayland
///   5. clipboard — last resort, user pastes manually
///   6. clipboard only — user pastes manually
#[cfg(target_os = "linux")]
fn paste_linux(text: &str) -> Result<()> {
    // 1. dotool — no daemon, uinput, reads from stdin
    if type_text_dotool(text) {
        eprintln!("[paste] via dotool");
        return Ok(());
    }

    // 2. ydotool — uinput via daemon (auto-started)
    if type_text_ydotool(text) {
        eprintln!("[paste] via ydotool");
        return Ok(());
    }

    // 3. wtype — Wayland virtual keyboard (wlroots only, not GNOME)
    if type_text_wtype(text) {
        eprintln!("[paste] via wtype");
        return Ok(());
    }

    // 4. xdotool — X11 / XWayland
    if type_text_xdotool(text) {
        eprintln!("[paste] via xdotool");
        return Ok(());
    }

    // 5. clipboard only
    set_clipboard_linux(text)?;
    eprintln!("[paste] clipboard only — press Ctrl+V to paste (no typing tool available)");
    Ok(())
}

/// Type via dotool (stdin pipe, no daemon needed, uses /dev/uinput).
/// `echo "type hello" | dotool`
#[cfg(target_os = "linux")]
fn type_text_dotool(text: &str) -> bool {
    use std::io::Write;
    if !crate::setup::cmd_exists("dotool") {
        return false;
    }
    let Ok(mut child) = std::process::Command::new("dotool")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        // dotool protocol: "type <text>\n"
        let _ = writeln!(stdin, "type {}", text);
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// Type via ydotool (uinput daemon). Auto-starts ydotoold if needed.
#[cfg(target_os = "linux")]
fn type_text_ydotool(text: &str) -> bool {
    if !crate::setup::cmd_exists("ydotool") {
        return false;
    }
    let _ = ensure_ydotoold();
    let out = std::process::Command::new("ydotool")
        .args(["type", "--"])
        .arg(text)
        .output();
    match out {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.is_empty() {
                eprintln!("[paste] ydotool error: {}", err.trim());
            }
            false
        }
        Err(e) => {
            eprintln!("[paste] ydotool failed: {}", e);
            false
        }
    }
}

/// Return the full path to ydotoold, searching common install locations.
#[cfg(target_os = "linux")]
fn find_ydotoold() -> Option<std::path::PathBuf> {
    let candidates = [
        "/usr/bin/ydotoold",
        "/usr/local/bin/ydotoold",
        "/usr/sbin/ydotoold",
        "/usr/local/sbin/ydotoold",
    ];
    for p in &candidates {
        if std::path::Path::new(p).exists() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    // Also try PATH-based lookup
    if let Ok(out) = std::process::Command::new("which")
        .arg("ydotoold")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(std::path::PathBuf::from(path));
            }
        }
    }
    None
}

/// Start ydotoold daemon if not already running.
/// Returns true if the daemon is running after this call.
#[cfg(target_os = "linux")]
pub fn ensure_ydotoold() -> bool {
    let running = std::process::Command::new("pgrep")
        .args(["-x", "ydotoold"])
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if running {
        return true;
    }

    let Some(bin) = find_ydotoold() else {
        return false;
    };

    eprintln!("[paste] starting ydotoold daemon ({})...", bin.display());
    match std::process::Command::new(&bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {
            std::thread::sleep(std::time::Duration::from_millis(600));
            // Confirm it's now running
            let ok = std::process::Command::new("pgrep")
                .args(["-x", "ydotoold"])
                .stdout(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                eprintln!("[paste] ydotoold daemon started");
            } else {
                eprintln!("[paste] ydotoold started but pgrep didn't find it (may be fine)");
            }
            true
        }
        Err(e) => {
            eprintln!("[paste] could not start ydotoold: {}", e);
            false
        }
    }
}

/// Type via wtype (Wayland virtual keyboard — wlroots compositors, NOT GNOME).
#[cfg(target_os = "linux")]
fn type_text_wtype(text: &str) -> bool {
    std::process::Command::new("wtype")
        .arg("--")
        .arg(text)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Type via xdotool (X11 / XWayland).
#[cfg(target_os = "linux")]
fn type_text_xdotool(text: &str) -> bool {
    std::process::Command::new("xdotool")
        .args(["type", "--clearmodifiers", "--delay", "0", "--"])
        .arg(text)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}


/// Write text to the system clipboard on Linux.
/// On Wayland uses wl-copy (background process handles clipboard ownership);
/// on X11 uses arboard.
#[cfg(target_os = "linux")]
fn set_clipboard_linux(text: &str) -> Result<()> {
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        use std::io::Write;
        // Spawn wl-copy; it stays alive as clipboard owner until something else pastes
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .arg("--")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).ok();
            }
            // child keeps running in background — drop without wait
            return Ok(());
        }
    }
    // X11 or wl-copy missing: use arboard
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("clipboard: {}", e))?;
    cb.set_text(text)
        .map_err(|e| anyhow::anyhow!("clipboard write: {}", e))?;
    Ok(())
}

#[allow(dead_code)]
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
fn paste_macos(text: &str) -> Result<()> {
    // Try to type directly via osascript keystroke
    let script = format!(
        "tell application \"System Events\" to keystroke \"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    );
    if run_cmd("osascript", &["-e", &script]) {
        return Ok(());
    }
    // Fallback: clipboard + Cmd+V
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("clipboard: {}", e))?;
    cb.set_text(text)
        .map_err(|e| anyhow::anyhow!("clipboard write: {}", e))?;
    std::thread::sleep(std::time::Duration::from_millis(80));
    if run_cmd(
        "osascript",
        &["-e", "tell application \"System Events\" to keystroke \"v\" using {command down}"],
    ) {
        return Ok(());
    }
    eprintln!("[paste] text in clipboard — press Cmd+V to paste");
    Ok(())
}

#[cfg(target_os = "windows")]
fn paste_windows(text: &str) -> Result<()> {
    use rdev::{simulate, EventType, Key};

    let mut cb = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("clipboard: {}", e))?;
    cb.set_text(text)
        .map_err(|e| anyhow::anyhow!("clipboard write: {}", e))?;

    // Brief pause so modifier keys from the hotkey are fully released
    std::thread::sleep(std::time::Duration::from_millis(80));

    // Simulate Ctrl+V directly — no process spawn, no window flash, no focus steal
    let _ = simulate(&EventType::KeyPress(Key::ControlLeft));
    let _ = simulate(&EventType::KeyPress(Key::KeyV));
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = simulate(&EventType::KeyRelease(Key::KeyV));
    let _ = simulate(&EventType::KeyRelease(Key::ControlLeft));

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
    let ds = detect_display_server();

    // ── Channel: true = key pressed (start), false = key released (stop) ─────
    let (tx, rx) = mpsc::channel::<HotkeySignal>();

    // ── Source 1: evdev (Linux — works on Wayland + X11, needs input group) ──
    #[cfg(target_os = "linux")]
    let mut global_hotkey_ok = {
        spawn_evdev_listener(&hotkey, tx.clone())
    };

    // ── Source 2: rdev (macOS / Windows / Linux X11 fallback) ────────────────
    #[cfg(not(target_os = "linux"))]
    let global_hotkey_ok = {
        spawn_rdev_listener(&hotkey, tx.clone());
        true
    };

    #[cfg(target_os = "linux")]
    if !global_hotkey_ok && ds == DisplayServer::X11 {
        spawn_rdev_listener(&hotkey, tx.clone());
        global_hotkey_ok = true;
    }

    // ── Source 3: stdin Enter key (toggle fallback) ──────────────────────────
    {
        let tx_stdin = tx.clone();
        let stdin_recording = Arc::new(AtomicBool::new(false));
        let sr = stdin_recording.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for _line in std::io::stdin().lock().lines().flatten() {
                let was = sr.fetch_xor(true, Ordering::Relaxed);
                let _ = tx_stdin.send(!was); // toggle: send true then false
            }
        });
    }

    // ── Startup banner ────────────────────────────────────────────────────────
    eprintln!();
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!(" voicr  hold-to-talk");
    eprintln!("─────────────────────────────────────");

    if global_hotkey_ok {
        eprintln!(" Hotkey  : hold [{}]  (global)", combo_str);
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

    eprintln!(" Fallback: press Enter to toggle");
    if no_paste {
        eprintln!(" Output  : stdout (--no-paste)");
    } else {
        eprintln!(" Output  : paste into active window");
    }
    eprintln!("─────────────────────────────────────");
    eprintln!(" Ctrl+C to quit");
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!();

    // Pre-start ydotoold daemon so it's ready before first paste
    #[cfg(target_os = "linux")]
    {
        if crate::setup::cmd_exists("ydotool") {
            let _ = ensure_ydotoold();
        }
    }

    // ── Build transcription manager ───────────────────────────────────────────
    let status_cb: crate::managers::transcription::StatusCallback = Arc::new(|status| {
        use crate::managers::transcription::ModelStatus;
        match status {
            ModelStatus::Loading { model_id } => eprintln!("[model] loading {}...", model_id),
            ModelStatus::Loaded { model_name, .. } => {
                eprintln!("[model] {} ready", model_name)
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

    // Drain any stale events that arrived during model loading
    while rx.try_recv().is_ok() {}

    eprintln!("Ready — hold [{}] and speak\n", combo_str);

    // ── Hold-to-talk loop ────────────────────────────────────────────────────
    let mut recorder: Option<AudioRecorder> = None;

    for signal in &rx {
        if signal {
            // ── KEY PRESSED: start recording ─────────────────────────────
            // Ignore if already recording
            if recorder.is_some() {
                continue;
            }

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
            crate::audio_toolkit::sound::play(crate::audio_toolkit::sound::Sound::RecordStart);
            eprintln!("recording...");
        } else {
            // ── KEY RELEASED: stop recording + transcribe + paste ─────────
            let audio = if let Some(mut rec) = recorder.take() {
                let a = rec.stop().unwrap_or_default();
                let _ = rec.close();
                a
            } else {
                continue; // not recording, ignore release
            };

            if audio.is_empty() {
                eprintln!("[skip] no audio captured");
                continue;
            }

            crate::audio_toolkit::sound::play(crate::audio_toolkit::sound::Sound::RecordStop);

            let duration = audio.len() as f64 / 16000.0;
            eprintln!("transcribing ({:.1}s)...", duration);

            let tm_clone = tm.clone();
            let config_clone = config.clone();

            std::thread::spawn(move || {
                match tm_clone.transcribe(audio) {
                    Ok(t) if t.trim().is_empty() => {
                        eprintln!("[transcription] (empty)");
                    }
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
                            // Brief pause so modifier keys are fully released
                            std::thread::sleep(std::time::Duration::from_millis(200));
                            match paste_text(&text, trailing) {
                                Ok(_) => eprintln!("[pasted]"),
                                Err(e) => eprintln!("[paste] {}", e),
                            }
                        }
                    }
                    Err(e) => eprintln!("[error] {}", e),
                }
            });
        }
    }
    Ok(())
}

/// Spawn an rdev-based keyboard listener (macOS / Windows / Linux X11 fallback).
pub fn spawn_rdev_listener(hotkey: &ParsedHotkey, tx: mpsc::Sender<HotkeySignal>) {
    let ctrl = Arc::new(AtomicBool::new(false));
    let alt = Arc::new(AtomicBool::new(false));
    let shift = Arc::new(AtomicBool::new(false));
    let meta = Arc::new(AtomicBool::new(false));
    let combo_active = Arc::new(AtomicBool::new(false));
    let (c2, a2, sh2, m2) = (ctrl.clone(), alt.clone(), shift.clone(), meta.clone());
    let active2 = combo_active.clone();
    let hotkey2 = hotkey.clone();

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
                        if ok && !active2.swap(true, Ordering::Relaxed) {
                            let _ = tx.send(true);
                        }
                    }
                }
                EventType::KeyRelease(key) => {
                    match key {
                        Key::ControlLeft | Key::ControlRight => {
                            c2.store(false, Ordering::Relaxed)
                        }
                        Key::Alt | Key::AltGr => a2.store(false, Ordering::Relaxed),
                        Key::ShiftLeft | Key::ShiftRight => sh2.store(false, Ordering::Relaxed),
                        Key::MetaLeft | Key::MetaRight => m2.store(false, Ordering::Relaxed),
                        _ => {}
                    }
                    if key == hotkey2.trigger && active2.swap(false, Ordering::Relaxed) {
                        let _ = tx.send(false);
                    }
                }
                _ => {}
            }
        };
        if let Err(e) = listen(cb) {
            warn!("rdev listener stopped: {:?}", e);
        }
    });
}
