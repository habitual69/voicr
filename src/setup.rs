//! First-run auto-setup: downloads the default model and installs OS-level
//! dependencies (wtype on Wayland, input-group membership for evdev).

use anyhow::Result;

pub const DEFAULT_MODEL: &str = "parakeet-tdt-0.6b-v3";

/// Run all setup steps. Called before the hotkey loop starts.
pub async fn ensure_ready(
    config: &mut crate::config::Config,
    model_manager: &crate::managers::model::ModelManager,
) -> Result<()> {
    ensure_default_model(config, model_manager).await?;

    #[cfg(target_os = "linux")]
    ensure_linux_setup();

    #[cfg(target_os = "macos")]
    check_macos_accessibility();

    Ok(())
}

// ── Model ─────────────────────────────────────────────────────────────────────

async fn ensure_default_model(
    config: &mut crate::config::Config,
    model_manager: &crate::managers::model::ModelManager,
) -> Result<()> {
    // Set selected model if none configured
    if config.model.selected.is_empty() {
        config.model.selected = DEFAULT_MODEL.to_string();
        crate::config::save_config(config)?;
    }

    let info = match model_manager.get_model_info(DEFAULT_MODEL) {
        Some(i) => i,
        None => return Ok(()), // unknown model id — skip
    };

    if info.is_downloaded {
        eprintln!("Model: {} (ready)", info.name);
        return Ok(());
    }

    eprintln!("Downloading default model: {} ({} MB) — first run only...", info.name, info.size_mb);

    // Show a progress line that updates in place
    let last_pct = std::sync::Arc::new(std::sync::Mutex::new(0u64));
    let last_pct2 = last_pct.clone();
    let progress_cb: std::sync::Arc<dyn Fn(crate::managers::model::DownloadProgress) + Send + Sync> =
        std::sync::Arc::new(move |p: crate::managers::model::DownloadProgress| {
            let pct = p.percentage as u64;
            let mut last = last_pct2.lock().unwrap();
            if pct >= *last + 5 || pct == 100 {
                eprint!(
                    "\r  {:.0}%  ({:.1} / {:.1} MB)   ",
                    p.percentage,
                    p.downloaded as f64 / 1_048_576.0,
                    p.total as f64 / 1_048_576.0,
                );
                *last = pct;
            }
        });

    // Rebuild model manager with progress callback so the download shows progress
    let models_dir = crate::paths::models_dir()?;
    let config_arc = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));
    let mm_with_progress = crate::managers::model::ModelManager::new(
        models_dir,
        config_arc,
        Some(progress_cb),
    )?;
    mm_with_progress.download_model(DEFAULT_MODEL).await?;
    eprintln!("\nModel downloaded");
    Ok(())
}

// ── Linux setup ───────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn ensure_linux_setup() {
    let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    if is_wayland {
        // Install wtype if missing (needed for Wayland paste)
        if !cmd_exists("wtype") {
            eprintln!("Installing wtype (Wayland paste tool)...");
            if try_install("wtype") {
                eprintln!("wtype installed");
            } else {
                eprintln!("Could not auto-install wtype.");
                eprintln!("  Install manually:  sudo apt install wtype");
                eprintln!("  Until then, text is copied to clipboard after transcription.");
            }
        } else {
            eprintln!("wtype (Wayland paste) ready");
        }
    }

    // Check input group (needed for evdev global hotkeys on Wayland)
    if !is_in_input_group() {
        eprintln!("Setting up global hotkey access...");
        if try_add_to_input_group() {
            eprintln!("Added to 'input' group.");
            eprintln!("  Log out and back in for the global hotkey to work.");
            eprintln!("  Until then, press Enter in this terminal to toggle recording.");
        } else {
            eprintln!("Could not add to 'input' group automatically.");
            eprintln!("  Run once:  sudo usermod -aG input $USER   (then re-login)");
            eprintln!("  Until then, press Enter in this terminal to toggle recording.");
        }
    } else {
        eprintln!("Input group (global hotkey) ready");
    }
}

#[cfg(target_os = "linux")]
fn is_in_input_group() -> bool {
    std::process::Command::new("groups")
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .any(|g| g == "input")
        })
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn try_add_to_input_group() -> bool {
    let user = match std::env::var("USER") {
        Ok(u) if !u.is_empty() => u,
        _ => return false,
    };

    // pkexec shows a graphical auth dialog
    if std::process::Command::new("pkexec")
        .args(["usermod", "-aG", "input", &user])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return true;
    }

    // sudo -n = passwordless sudo (works if user has NOPASSWD in sudoers)
    std::process::Command::new("sudo")
        .args(["-n", "usermod", "-aG", "input", &user])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cmd_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn try_install(pkg: &str) -> bool {
    // Try pkexec apt-get (graphical auth, works on GNOME/KDE)
    if std::process::Command::new("pkexec")
        .args(["apt-get", "install", "-y", pkg])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return true;
    }

    // Try sudo -n (passwordless)
    std::process::Command::new("sudo")
        .args(["-n", "apt-get", "install", "-y", pkg])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── macOS setup ───────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn check_macos_accessibility() {
    // rdev requires Accessibility permissions on macOS.
    // We can't check this programmatically without extra deps, so just remind.
    eprintln!(
        "macOS: if the hotkey does not work, grant Accessibility access to your terminal:\n\
         System Settings -> Privacy & Security -> Accessibility"
    );
}
