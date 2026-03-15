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

    // ydotool injects keystrokes via /dev/uinput — works on all compositors
    // (GNOME doesn't support virtual keyboard protocol that wtype needs)
    if !cmd_exists("ydotool") {
        eprintln!("Installing ydotool...");
        if !try_install("ydotool") {
            eprintln!("Could not auto-install ydotool.");
            eprintln!("  Install manually:  sudo apt install ydotool");
        }
    }

    // /dev/uinput must be accessible to the input group.
    // By default Ubuntu sets it as root:root 0600 — fix with a udev rule.
    ensure_uinput_group_access();

    // Check input group for THIS process (not just login session config)
    if !current_process_in_input_group() {
        // Check if the user IS in the group in system config but session hasn't loaded it
        if user_configured_in_input_group() {
            // Group was added in a previous run — just re-exec via `sg input` to activate it
            // without requiring a full re-login.
            eprintln!("Activating input group (no re-login needed)...");
            reexec_with_input_group();
            // reexec_with_input_group calls process::exit, so we never get here
        }

        // Not in the group at all — add the user, then re-exec
        eprintln!("Setting up global hotkey access (evdev)...");
        if try_add_to_input_group() {
            eprintln!("Added to 'input' group.");
            eprintln!("Activating now (no re-login needed)...");
            reexec_with_input_group();
        } else {
            eprintln!("Could not add to 'input' group automatically.");
            eprintln!("  Run once:  sudo usermod -aG input $USER   (then re-login)");
            eprintln!("  Until then, press Enter in this terminal to toggle recording.");
        }
    } else {
        eprintln!("Input group (global hotkey) ready");
    }

}

/// Ensure /dev/uinput is accessible to the `input` group.
/// Ubuntu sets /dev/uinput as root:root 0600 by default.
/// We fix this two ways:
///   1. Immediately: chmod 0660 + chgrp input (via pkexec) — takes effect NOW
///   2. Persistently: write a udev rule so it survives reboots
#[cfg(target_os = "linux")]
fn ensure_uinput_group_access() {
    if !std::path::Path::new("/dev/uinput").exists() {
        return;
    }
    // Already accessible?
    if std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok() {
        return;
    }

    eprintln!("Granting input group access to /dev/uinput (one-time setup)...");

    // Step 1: Fix permissions RIGHT NOW via pkexec (survives until reboot)
    let chmod_ok = std::process::Command::new("pkexec")
        .args(["sh", "-c", "chmod 0660 /dev/uinput && chgrp input /dev/uinput"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || std::process::Command::new("sudo")
            .args(["-n", "sh", "-c", "chmod 0660 /dev/uinput && chgrp input /dev/uinput"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    // Step 2: Write udev rule for persistence across reboots
    let rule_path = "/etc/udev/rules.d/99-voicr-uinput.rules";
    if !std::path::Path::new(rule_path).exists() {
        let rule = "KERNEL==\"uinput\", GROUP=\"input\", MODE=\"0660\"\n";
        let tmp = "/tmp/voicr-uinput.rules";
        if std::fs::write(tmp, rule).is_ok() {
            let _ = std::process::Command::new("pkexec")
                .args(["sh", "-c", &format!(
                    "cp {} {} && udevadm control --reload-rules",
                    tmp, rule_path
                )])
                .status();
        }
    }

    if chmod_ok {
        eprintln!("/dev/uinput accessible (input group)");
    } else {
        eprintln!("Could not set /dev/uinput permissions automatically.");
        eprintln!("  Run once:");
        eprintln!("    echo 'KERNEL==\"uinput\", GROUP=\"input\", MODE=\"0660\"' | sudo tee /etc/udev/rules.d/99-voicr-uinput.rules");
        eprintln!("    sudo udevadm control --reload-rules && sudo udevadm trigger --name-match=uinput");
    }
}

/// Check if THIS running process has the input group in its supplementary groups.
/// Uses `id -Gn` which reflects the actual kernel-level groups for the current process,
/// not just what's stored in /etc/group.
#[cfg(target_os = "linux")]
fn current_process_in_input_group() -> bool {
    std::process::Command::new("id")
        .arg("-Gn")
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .any(|g| g == "input")
        })
        .unwrap_or(false)
}

/// Check if the user account is listed in the input group in /etc/group.
/// This is true even before a re-login / sg session.
#[cfg(target_os = "linux")]
fn user_configured_in_input_group() -> bool {
    let user = match std::env::var("USER").ok().filter(|u| !u.is_empty()) {
        Some(u) => u,
        None => return false,
    };
    // `getent group input` prints: input:x:GID:user1,user2,...
    std::process::Command::new("getent")
        .args(["group", "input"])
        .output()
        .map(|o| {
            let line = String::from_utf8_lossy(&o.stdout);
            line.split(':')
                .nth(3)
                .unwrap_or("")
                .split(',')
                .any(|u| u.trim() == user)
        })
        .unwrap_or(false)
}

/// Re-exec the current process under `sg input -c "..."` so the input group is
/// active immediately, without requiring the user to log out.
/// This function does not return — it calls process::exit.
#[cfg(target_os = "linux")]
fn reexec_with_input_group() {
    let exe = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => {
            eprintln!("  (could not find current executable — please re-login)");
            return;
        }
    };
    // Rebuild the original command line with shell-safe quoting.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = if args.is_empty() {
        exe.clone()
    } else {
        let quoted_args: Vec<String> = args
            .iter()
            .map(|a| {
                if a.contains(' ') || a.contains('\'') || a.contains('"') {
                    format!("'{}'", a.replace('\'', "'\\''"))
                } else {
                    a.clone()
                }
            })
            .collect();
        format!("{} {}", exe, quoted_args.join(" "))
    };

    let status = std::process::Command::new("sg")
        .args(["input", "-c", &cmd])
        .status()
        .unwrap_or_else(|e| {
            eprintln!("  sg failed: {} — please re-login for hotkey to work", e);
            std::process::exit(1);
        });
    std::process::exit(status.code().unwrap_or(0));
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

pub fn cmd_exists(name: &str) -> bool {
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
