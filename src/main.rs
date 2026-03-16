mod audio_toolkit;
mod cli;
mod config;
mod daemon;
mod hotkey;
mod managers;
mod paths;
mod setup;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, ConfigCommands, HistoryCommands, ModelCommands};
use config::Config;
use log::error;
use managers::model::ModelManager;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.debug { "debug" } else { "info" };
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(log_level),
    )
    .format_timestamp_secs()
    .init();

    // Ensure directories exist
    paths::ensure_dirs()?;

    // Load config
    let config = config::load_config()?;
    let config = Arc::new(Mutex::new(config));

    match cli.command {
        None => cmd_default(config).await,
        Some(Commands::Daemon { socket, foreground }) => {
            cmd_daemon(config, socket, foreground).await
        }
        Some(Commands::Transcribe {
            file,
            output,
            duration,
            no_vad,
            auto_stop,
        }) => cmd_transcribe(config, file, output, duration, no_vad, auto_stop).await,
        Some(Commands::Send { command, socket, wait }) => {
            cmd_send(&command, socket, wait).await
        }
        Some(Commands::Model(model_cmd)) => cmd_model(config, model_cmd).await,
        Some(Commands::Config(config_cmd)) => cmd_config(config, config_cmd),
        Some(Commands::History(history_cmd)) => cmd_history(config, history_cmd).await,
        Some(Commands::Devices) => cmd_devices(),
        Some(Commands::Hotkey { combo, no_paste }) => cmd_hotkey(config, combo, no_paste).await,
    }
}

// ── Default (no-arg) mode ─────────────────────────────────────────────────────

async fn cmd_default(config: Arc<Mutex<Config>>) -> Result<()> {
    let model_manager = Arc::new(build_model_manager(config.clone(), false)?);

    // Auto-setup: download model, install deps
    {
        let mut cfg = config.lock().unwrap().clone();
        setup::ensure_ready(&mut cfg, &model_manager).await?;
        // Persist any changes made by setup (e.g. model.selected)
        *config.lock().unwrap() = cfg;
    }

    // Ensure VAD model
    let vad_path = match model_manager.ensure_vad_model().await {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("Warning: VAD unavailable ({})", e);
            None
        }
    };

    // Run push-to-talk (blocking)
    tokio::task::spawn_blocking(move || {
        hotkey::run_hotkey(config, model_manager, vad_path, None, false)
    })
    .await?
}

// ── Daemon ────────────────────────────────────────────────────────────────────

async fn cmd_daemon(config: Arc<Mutex<Config>>, socket: Option<String>, foreground: bool) -> Result<()> {
    // If not foreground, daemonize by re-spawning with --foreground
    #[cfg(unix)]
    if !foreground {
        return daemonize(socket).await;
    }

    #[cfg(not(unix))]
    if !foreground {
        anyhow::bail!("Daemon mode is only supported on Unix systems");
    }

    // Write PID file
    let pid_path = paths::pid_path();
    std::fs::write(&pid_path, std::process::id().to_string())?;

    let socket_path = socket
        .map(std::path::PathBuf::from)
        .unwrap_or_else(paths::socket_path);

    let model_manager = build_model_manager(config.clone(), true)?;
    let model_manager = Arc::new(model_manager);

    // Ensure VAD model is available
    let vad_path = {
        let mm = model_manager.clone();
        match mm.ensure_vad_model().await {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to download VAD model: {}. VAD will be disabled.", e);
                mm.vad_model_path() // still pass the path even if it doesn't exist
            }
        }
    };

    let result = daemon::run_daemon(socket_path, config, model_manager, vad_path);

    // Clean up PID file on exit
    let _ = std::fs::remove_file(&pid_path);

    result
}

/// Daemonize by re-spawning the current process with --foreground,
/// detached from the terminal with stdout/stderr redirected to a log file.
#[cfg(unix)]
async fn daemonize(socket: Option<String>) -> Result<()> {
    use std::os::unix::process::CommandExt;

    // Check if daemon is already running
    let pid_path = paths::pid_path();
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                // Check if process is still alive
                if unsafe { libc::kill(pid, 0) } == 0 {
                    anyhow::bail!(
                        "voicr daemon is already running (PID {}). Stop it with: voicr send shutdown",
                        pid
                    );
                }
            }
        }
        // Stale PID file, remove it
        let _ = std::fs::remove_file(&pid_path);
    }

    let exe = std::env::current_exe()?;
    let log_path = paths::daemon_log_path()?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_file_err = log_file.try_clone()?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon").arg("--foreground");

    if let Some(ref s) = socket {
        cmd.arg("--socket").arg(s);
    }

    // Forward --debug if it was set
    if std::env::args().any(|a| a == "--debug" || a == "-d") {
        cmd.arg("--debug");
    }

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::from(log_file));
    cmd.stderr(std::process::Stdio::from(log_file_err));

    // Detach from terminal: create new session via setsid
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let child = cmd.spawn()?;

    let socket_display = socket
        .map(std::path::PathBuf::from)
        .unwrap_or_else(paths::socket_path);

    eprintln!("voicr daemon started (PID: {})", child.id());
    eprintln!("Socket: {}", socket_display.display());
    eprintln!("Log:    {}", log_path.display());
    eprintln!("Stop with: voicr send shutdown");

    Ok(())
}

// ── One-shot transcription ────────────────────────────────────────────────────

async fn cmd_transcribe(
    config: Arc<Mutex<Config>>,
    file: Option<String>,
    output: Option<String>,
    duration: u64,
    no_vad: bool,
    auto_stop: bool,
) -> Result<()> {
    let model_manager = Arc::new(build_model_manager(config.clone(), false)?);

    // Ensure VAD model
    let vad_path = if !no_vad {
        match model_manager.ensure_vad_model().await {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("Warning: VAD unavailable ({}). Recording without silence detection.", e);
                None
            }
        }
    } else {
        None
    };

    let audio = if let Some(wav_path) = file {
        // Read from WAV file
        read_wav_file(&wav_path)?
    } else {
        // Record from microphone
        record_from_microphone(&config, vad_path.as_deref(), duration, auto_stop)?
    };

    if audio.is_empty() {
        eprintln!("No audio captured.");
        return Ok(());
    }

    eprintln!("Transcribing {} seconds of audio...", audio.len() / 16000);

    let status_cb: managers::transcription::StatusCallback = Arc::new(|status| {
        use managers::transcription::ModelStatus;
        match status {
            ModelStatus::Loading { model_id } => {
                eprintln!("Loading model: {}", model_id);
            }
            ModelStatus::Loaded { model_name, .. } => {
                eprintln!("Model loaded: {}", model_name);
            }
            ModelStatus::Unloaded => {}
            ModelStatus::Error { message, .. } => {
                eprintln!("Model error: {}", message);
            }
        }
    });

    let tm = managers::transcription::TranscriptionManager::new(
        model_manager.clone(),
        config.clone(),
        Some(status_cb),
    )?;

    tm.ensure_model_loaded()?;

    let text = tm.transcribe(audio.clone())?;

    // Write output
    let result = if config.lock().unwrap().output.append_newline {
        format!("{}\n", text)
    } else {
        text.clone()
    };

    match output {
        Some(path) => {
            std::fs::write(&path, &result)?;
            eprintln!("Transcription saved to: {}", path);
        }
        None => print!("{}", result),
    }

    // Save to history if enabled
    let (history_enabled, history_limit, retention) = {
        let cfg = config.lock().unwrap();
        (
            cfg.history.enabled,
            cfg.history.limit,
            cfg.history.retention.clone(),
        )
    };

    if history_enabled {
        let history_manager = managers::history::HistoryManager::new(
            paths::recordings_dir()?,
            paths::history_db_path()?,
            history_limit,
            retention,
        )?;
        history_manager
            .save_transcription(audio, text, None, None)
            .await?;
    }

    Ok(())
}

fn record_from_microphone(
    config: &Arc<Mutex<Config>>,
    vad_path: Option<&std::path::Path>,
    duration: u64,
    _auto_stop: bool,
) -> Result<Vec<f32>> {
    use audio_toolkit::{vad::SmoothedVad, AudioRecorder, SileroVad};

    let (vad_enabled, vad_threshold, device_name) = {
        let cfg = config.lock().unwrap();
        (
            cfg.audio.vad_enabled,
            cfg.audio.vad_threshold,
            cfg.audio.device.clone(),
        )
    };

    let max_secs = if duration > 0 { duration } else { 300 }; // 5 min hard cap

    let device = device_name.as_deref().and_then(find_device_by_name);

    let mut rec = AudioRecorder::new().map_err(|e| anyhow::anyhow!("{}", e))?;

    if vad_enabled {
        if let Some(path) = vad_path {
            if path.exists() {
                if let Ok(silero) = SileroVad::new(path, vad_threshold) {
                    let smoothed = SmoothedVad::new(Box::new(silero), 15, 15, 2);
                    rec = rec.with_vad(Box::new(smoothed));
                }
            }
        }
    }

    rec.open(device).map_err(|e| anyhow::anyhow!("{}", e))?;
    rec.start().map_err(|e| anyhow::anyhow!("{}", e))?;

    eprintln!("Recording... (press Ctrl+C to stop)");

    // Wait for duration or Ctrl+C
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_clone = done.clone();

    ctrlc::set_handler(move || {
        done_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    })
    .ok();

    let start = std::time::Instant::now();
    loop {
        if done.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if start.elapsed().as_secs() >= max_secs {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    eprintln!("Stopping recording...");
    let audio = rec.stop().map_err(|e| anyhow::anyhow!("{}", e))?;
    let _ = rec.close();

    Ok(audio)
}

fn read_wav_file(path: &str) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.map_err(|e| anyhow::anyhow!(e)))
            .collect::<Result<Vec<_>>>()?,
        hound::SampleFormat::Int => {
            let max = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            match spec.bits_per_sample {
                16 => reader
                    .samples::<i16>()
                    .map(|s| s.map(|v| v as f32 / max).map_err(|e| anyhow::anyhow!(e)))
                    .collect::<Result<Vec<_>>>()?,
                32 => reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / max).map_err(|e| anyhow::anyhow!(e)))
                    .collect::<Result<Vec<_>>>()?,
                _ => anyhow::bail!("Unsupported bit depth: {}", spec.bits_per_sample),
            }
        }
    };

    // Resample to 16kHz if needed
    if spec.sample_rate != 16000 {
        eprintln!("Resampling {} Hz → 16000 Hz", spec.sample_rate);
        use rubato::{FftFixedIn, Resampler};
        let in_rate = spec.sample_rate as usize;
        let chunk = 1024usize;
        let mut resampler = FftFixedIn::<f32>::new(in_rate, 16000, chunk, 1, 1)
            .map_err(|e| anyhow::anyhow!("resampler init: {}", e))?;
        let mut out = Vec::new();
        let mut i = 0;
        while i + chunk <= samples.len() {
            let result = resampler
                .process(&[&samples[i..i + chunk]], None)
                .map_err(|e| anyhow::anyhow!("resample: {}", e))?;
            out.extend_from_slice(&result[0]);
            i += chunk;
        }
        if i < samples.len() {
            let mut tail = samples[i..].to_vec();
            tail.resize(chunk, 0.0);
            let result = resampler
                .process(&[&tail], None)
                .map_err(|e| anyhow::anyhow!("resample tail: {}", e))?;
            out.extend_from_slice(&result[0]);
        }
        Ok(out)
    } else {
        Ok(samples)
    }
}

// ── Send command to daemon ────────────────────────────────────────────────────

async fn cmd_send(command: &str, socket: Option<String>, wait: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let socket_path = socket
            .map(std::path::PathBuf::from)
            .unwrap_or_else(paths::socket_path);

        let mut stream = UnixStream::connect(&socket_path)
            .map_err(|_| anyhow::anyhow!("Cannot connect to daemon at {:?}. Is it running? Start with: voicr daemon", socket_path))?;

        let cmd = serde_json::json!({"cmd": command});
        writeln!(stream, "{}", cmd)?;

        if wait || command == "status" {
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let line = line?;
                println!("{}", line);

                // Stop reading after first response for status queries
                if command == "status" {
                    break;
                }

                // Stop on transcription result or error
                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let event_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if matches!(event_type, "transcription" | "error" | "shutdown") {
                    break;
                }
            }
        }

        return Ok(());
    }

    #[cfg(not(unix))]
    {
        let _ = (command, socket, wait);
        anyhow::bail!("send command is only supported on Unix systems");
    }
}

// ── Model commands ────────────────────────────────────────────────────────────

async fn cmd_model(config: Arc<Mutex<Config>>, cmd: ModelCommands) -> Result<()> {
    let model_manager = Arc::new(build_model_manager(config.clone(), false)?);

    match cmd {
        ModelCommands::List => {
            let models = model_manager.get_available_models();
            let selected = config.lock().unwrap().model.selected.clone();

            println!(
                "{:<32} {:<24} {:>8} {:>8} {:>8}  {}",
                "ID", "Name", "Size", "Accuracy", "Speed", "Status"
            );
            println!("{}", "-".repeat(100));

            for m in &models {
                let status = if m.is_downloading {
                    format!("Downloading ({:.0}%)", m.partial_size as f64 / (m.size_mb * 1024 * 1024) as f64 * 100.0)
                } else if m.is_downloaded {
                    if m.id == selected { "✓ (active)".to_string() } else { "✓ downloaded".to_string() }
                } else {
                    "not downloaded".to_string()
                };

                let recommended = if m.is_recommended { " ⭐" } else { "" };

                println!(
                    "{:<32} {:<24} {:>6}MB {:>7.0}% {:>7.0}%  {}{}",
                    m.id,
                    m.name,
                    m.size_mb,
                    m.accuracy_score * 100.0,
                    m.speed_score * 100.0,
                    status,
                    recommended,
                );
            }

            println!();
            println!("Use 'voicr model download <id>' to download a model.");
            println!("Use 'voicr model set <id>' to select the active model.");
        }

        ModelCommands::Download { model_id } => {
            let info = model_manager.get_model_info(&model_id)
                .ok_or_else(|| anyhow::anyhow!("Unknown model: {}. Run 'voicr model list' to see available models.", model_id))?;

            if info.is_downloaded {
                println!("Model '{}' is already downloaded.", model_id);
                return Ok(());
            }

            println!("Downloading {} ({} MB)...", info.name, info.size_mb);

            let last_pct = Arc::new(Mutex::new(0u64));
            let progress_cb: Arc<dyn Fn(managers::model::DownloadProgress) + Send + Sync> = {
                let last_pct = last_pct.clone();
                Arc::new(move |p| {
                    let pct = p.percentage as u64;
                    let mut last = last_pct.lock().unwrap();
                    if pct >= *last + 5 || pct == 100 {
                        eprint!(
                            "\r  {:.1}% ({:.1} / {:.1} MB)",
                            p.percentage,
                            p.downloaded as f64 / 1_048_576.0,
                            p.total as f64 / 1_048_576.0,
                        );
                        *last = pct;
                    }
                })
            };

            // Rebuild with progress callback
            let mm = Arc::new(build_model_manager_with_cb(
                config.clone(),
                false,
                Some(progress_cb),
            )?);

            mm.download_model(&model_id).await?;
            eprintln!();
            println!("✓ Downloaded '{}'", model_id);

            // Auto-set as active if none selected
            let selected = config.lock().unwrap().model.selected.clone();
            if selected.is_empty() {
                mm.set_active_model(&model_id)?;
                println!("✓ Set '{}' as active model.", model_id);
            }
        }

        ModelCommands::Delete { model_id } => {
            let info = model_manager.get_model_info(&model_id)
                .ok_or_else(|| anyhow::anyhow!("Unknown model: {}", model_id))?;

            if !info.is_downloaded {
                println!("Model '{}' is not downloaded.", model_id);
                return Ok(());
            }

            model_manager.delete_model(&model_id)?;
            println!("✓ Deleted '{}'", model_id);
        }

        ModelCommands::Set { model_id } => {
            let info = model_manager.get_model_info(&model_id)
                .ok_or_else(|| anyhow::anyhow!("Unknown model: {}. Run 'voicr model list'.", model_id))?;

            if !info.is_downloaded {
                anyhow::bail!(
                    "Model '{}' is not downloaded. Run: voicr model download {}",
                    model_id,
                    model_id
                );
            }

            model_manager.set_active_model(&model_id)?;
            println!("✓ Active model set to '{}'", model_id);
        }

        ModelCommands::Info { model_id } => {
            let info = model_manager.get_model_info(&model_id)
                .ok_or_else(|| anyhow::anyhow!("Unknown model: {}", model_id))?;

            println!("ID:           {}", info.id);
            println!("Name:         {}", info.name);
            println!("Description:  {}", info.description);
            println!("Engine:       {:?}", info.engine_type);
            println!("Size:         {} MB", info.size_mb);
            println!("Accuracy:     {:.0}%", info.accuracy_score * 100.0);
            println!("Speed:        {:.0}%", info.speed_score * 100.0);
            println!("Translation:  {}", info.supports_translation);
            println!("Recommended:  {}", info.is_recommended);
            println!("Downloaded:   {}", info.is_downloaded);
            println!("Languages:    {}", info.supported_languages.join(", "));
        }
    }

    Ok(())
}

// ── Config commands ───────────────────────────────────────────────────────────

fn cmd_config(config: Arc<Mutex<Config>>, cmd: ConfigCommands) -> Result<()> {
    match cmd {
        ConfigCommands::Show => {
            let cfg = config.lock().unwrap();
            let toml = toml::to_string_pretty(&*cfg)?;
            println!("{}", toml);
        }
        ConfigCommands::Set { key, value } => {
            let mut cfg = config.lock().unwrap();
            config::set_config_key(&mut cfg, &key, &value)?;
            config::save_config(&cfg)?;
            println!("✓ Set {} = {}", key, value);
        }
        ConfigCommands::Path => {
            println!("{}", paths::config_file()?.display());
        }
    }
    Ok(())
}

// ── History commands ──────────────────────────────────────────────────────────

async fn cmd_history(config: Arc<Mutex<Config>>, cmd: HistoryCommands) -> Result<()> {
    let (history_limit, retention) = {
        let cfg = config.lock().unwrap();
        (cfg.history.limit, cfg.history.retention.clone())
    };

    let history = managers::history::HistoryManager::new(
        paths::recordings_dir()?,
        paths::history_db_path()?,
        history_limit,
        retention,
    )?;

    match cmd {
        HistoryCommands::List { limit } => {
            let entries = history.get_history_entries().await?;
            let shown = entries.iter().take(limit);

            println!("{:<6} {:<8} {:<32} {}", "ID", "Saved", "Date", "Transcription");
            println!("{}", "-".repeat(100));

            for entry in shown {
                let preview = entry
                    .transcription_text
                    .chars()
                    .take(50)
                    .collect::<String>();
                let preview = if entry.transcription_text.len() > 50 {
                    format!("{}...", preview)
                } else {
                    preview
                };
                println!(
                    "{:<6} {:<8} {:<32} {}",
                    entry.id,
                    if entry.saved { "★" } else { "" },
                    entry.title,
                    preview
                );
            }

            if entries.len() > limit {
                println!("... ({} more entries)", entries.len() - limit);
            }
        }

        HistoryCommands::Get { id } => {
            match history.get_entry_by_id(id).await? {
                Some(entry) => {
                    println!("ID:          {}", entry.id);
                    println!("Date:        {}", entry.title);
                    println!("Saved:       {}", entry.saved);
                    println!("File:        {}", entry.file_name);
                    println!();
                    println!("Transcription:");
                    println!("{}", entry.transcription_text);
                    if let Some(ref pp) = entry.post_processed_text {
                        println!();
                        println!("Post-processed:");
                        println!("{}", pp);
                    }
                }
                None => {
                    anyhow::bail!("No entry with ID {}", id);
                }
            }
        }

        HistoryCommands::Delete { id } => {
            history.delete_entry(id).await?;
            println!("✓ Deleted entry {}", id);
        }

        HistoryCommands::Save { id } => {
            history.toggle_saved_status(id).await?;
            println!("✓ Toggled saved status for entry {}", id);
        }

        HistoryCommands::Export { output } => {
            let entries = history.get_history_entries().await?;
            let json = serde_json::to_string_pretty(&entries)?;
            match output {
                Some(path) => {
                    std::fs::write(&path, &json)?;
                    println!("✓ Exported {} entries to {}", entries.len(), path);
                }
                None => println!("{}", json),
            }
        }
    }

    Ok(())
}

// ── Devices ───────────────────────────────────────────────────────────────────

fn cmd_devices() -> Result<()> {
    println!("Input devices:");
    println!("{}", "-".repeat(50));

    match audio_toolkit::list_input_devices() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("  (none found)");
            }
            for d in &devices {
                let default = if d.is_default { " [default]" } else { "" };
                println!("  [{}] {}{}", d.index, d.name, default);
            }
        }
        Err(e) => eprintln!("  Error listing input devices: {}", e),
    }

    println!();
    println!("Output devices:");
    println!("{}", "-".repeat(50));

    match audio_toolkit::list_output_devices() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("  (none found)");
            }
            for d in &devices {
                let default = if d.is_default { " [default]" } else { "" };
                println!("  [{}] {}{}", d.index, d.name, default);
            }
        }
        Err(e) => eprintln!("  Error listing output devices: {}", e),
    }

    println!();
    println!("Set a specific device with: voicr config set audio.device \"Device Name\"");

    Ok(())
}

// ── Push-to-talk hotkey mode ──────────────────────────────────────────────────

async fn cmd_hotkey(
    config: Arc<Mutex<Config>>,
    combo: Option<String>,
    no_paste: bool,
) -> Result<()> {
    let model_manager = Arc::new(build_model_manager(config.clone(), false)?);

    let vad_path = match model_manager.ensure_vad_model().await {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("Warning: VAD unavailable ({}). Recording without silence detection.", e);
            None
        }
    };

    // run_hotkey blocks until Ctrl+C
    tokio::task::spawn_blocking(move || {
        hotkey::run_hotkey(config, model_manager, vad_path, combo, no_paste)
    })
    .await?
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_model_manager(config: Arc<Mutex<Config>>, _verbose: bool) -> Result<ModelManager> {
    build_model_manager_with_cb(config, _verbose, None)
}

fn build_model_manager_with_cb(
    config: Arc<Mutex<Config>>,
    _verbose: bool,
    progress_cb: Option<Arc<dyn Fn(managers::model::DownloadProgress) + Send + Sync>>,
) -> Result<ModelManager> {
    let models_dir = paths::models_dir()?;
    ModelManager::new(models_dir, config, progress_cb)
}

fn find_device_by_name(name: &str) -> Option<cpal::Device> {
    audio_toolkit::list_input_devices()
        .ok()?
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.device)
}
