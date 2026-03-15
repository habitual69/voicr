use anyhow::Result;
use std::path::PathBuf;

/// Returns the voicr data directory: ~/.local/share/voicr (Linux/macOS) or %APPDATA%\voicr (Windows)
pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
        .ok_or_else(|| anyhow::anyhow!("Cannot determine data directory"))?;
    Ok(base.join("voicr"))
}

/// Returns the voicr config directory: ~/.config/voicr (Linux/macOS) or %APPDATA%\voicr (Windows)
pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
    Ok(base.join("voicr"))
}

pub fn config_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn models_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("models"))
}

pub fn recordings_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("recordings"))
}

pub fn history_db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("history.db"))
}

pub fn socket_path() -> PathBuf {
    std::env::temp_dir().join("voicr.sock")
}

/// Ensure all required directories exist
pub fn ensure_dirs() -> Result<()> {
    std::fs::create_dir_all(config_dir()?)?;
    std::fs::create_dir_all(data_dir()?)?;
    std::fs::create_dir_all(models_dir()?)?;
    std::fs::create_dir_all(recordings_dir()?)?;
    Ok(())
}
