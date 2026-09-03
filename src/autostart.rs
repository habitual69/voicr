use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Path to the autostart entry for the current platform.
pub fn autostart_file_path() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let config_dir = dirs::config_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
            .context("Cannot determine user config directory")?;
        Ok(config_dir.join("autostart").join("voicr.desktop"))
    }

    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join("com.voicr.daemon.plist"))
    }

    #[cfg(target_os = "windows")]
    {
        let app_data = dirs::data_dir().context("Cannot determine APPDATA directory")?;
        Ok(app_data
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup")
            .join("voicr.cmd"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        anyhow::bail!("Autostart is not supported on this operating system");
    }
}

/// Checks whether autostart is currently enabled.
pub fn is_enabled() -> bool {
    autostart_file_path().map(|p| p.exists()).unwrap_or(false)
}

/// Enables autostart for the current user.
pub fn enable() -> Result<()> {
    let file_path = autostart_file_path()?;
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let exe = std::env::current_exe().context("Cannot determine current executable path")?;
    let exe_str = exe.to_string_lossy();

    #[cfg(target_os = "linux")]
    {
        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Voicr\n\
             Comment=Speech-to-text daemon\n\
             Exec={} daemon\n\
             Terminal=false\n\
             Categories=Utility;Audio;\n\
             X-GNOME-Autostart-enabled=true\n",
            exe_str
        );
        fs::write(&file_path, content)?;
    }

    #[cfg(target_os = "macos")]
    {
        let content = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
                 <key>Label</key>\n\
                 <string>com.voicr.daemon</string>\n\
                 <key>ProgramArguments</key>\n\
                 <array>\n\
                     <string>{}</string>\n\
                     <string>daemon</string>\n\
                 </array>\n\
                 <key>RunAtLoad</key>\n\
                 <true/>\n\
                 <key>KeepAlive</key>\n\
                 <false/>\n\
             </dict>\n\
             </plist>\n",
            exe_str
        );
        fs::write(&file_path, content)?;
    }

    #[cfg(target_os = "windows")]
    {
        let content = format!("@echo off\r\nstart \"\" \"{}\" daemon\r\n", exe_str);
        fs::write(&file_path, content)?;
    }

    Ok(())
}

/// Disables autostart for the current user.
pub fn disable() -> Result<()> {
    let file_path = autostart_file_path()?;
    if file_path.exists() {
        fs::remove_file(&file_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autostart_file_path() {
        let path = autostart_file_path().expect("should determine path");
        #[cfg(target_os = "linux")]
        assert!(path.to_string_lossy().ends_with("autostart/voicr.desktop"));
        #[cfg(target_os = "macos")]
        assert!(path.to_string_lossy().ends_with("LaunchAgents/com.voicr.daemon.plist"));
        #[cfg(target_os = "windows")]
        assert!(path.to_string_lossy().ends_with("Startup\\voicr.cmd"));
    }

    #[test]
    fn test_enable_and_disable_cycle() {
        let initial_state = is_enabled();

        enable().expect("enable should succeed");
        assert!(is_enabled());

        let path = autostart_file_path().expect("path should exist");
        assert!(path.exists());
        let content = fs::read_to_string(&path).expect("content should be readable");
        assert!(content.contains("daemon"));

        disable().expect("disable should succeed");
        assert!(!is_enabled());
        assert!(!path.exists());

        if initial_state {
            let _ = enable();
        }
    }
}
