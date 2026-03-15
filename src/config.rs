use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub audio: AudioConfig,
    pub model: ModelConfig,
    pub transcription: TranscriptionConfig,
    pub history: HistoryConfig,
    pub output: OutputConfig,
    pub hotkey: HotkeyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// Key combo to trigger push-to-talk (e.g. "ctrl+space", "alt+shift+r")
    pub combo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Input device name (empty = system default)
    pub device: Option<String>,
    /// Silero VAD threshold 0.0–1.0 (default 0.3)
    pub vad_threshold: f32,
    /// Whether to use VAD to filter silence (default true)
    pub vad_enabled: bool,
    /// Maximum recording duration in seconds (0 = unlimited)
    pub max_duration_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// ID of the active model
    pub selected: String,
    /// How long to keep the model in memory after last use
    pub unload_timeout: ModelUnloadTimeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelUnloadTimeout {
    Never,
    Immediately,
    Min2,
    Min5,
    Min10,
    Min15,
    Hour1,
}

impl ModelUnloadTimeout {
    pub fn to_seconds(&self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0),
            ModelUnloadTimeout::Min2 => Some(120),
            ModelUnloadTimeout::Min5 => Some(300),
            ModelUnloadTimeout::Min10 => Some(600),
            ModelUnloadTimeout::Min15 => Some(900),
            ModelUnloadTimeout::Hour1 => Some(3600),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    /// Language code ("auto" for automatic detection)
    pub language: String,
    /// Translate output to English (Whisper models only)
    pub translate_to_english: bool,
    /// Custom words for fuzzy correction
    pub custom_words: Vec<String>,
    /// Filter filler words like "um", "uh"
    pub filter_filler_words: bool,
    /// Override filler word list (None = use language defaults)
    pub custom_filler_words: Option<Vec<String>>,
    /// Fuzzy match threshold for custom word correction (0.0–1.0, lower = stricter)
    pub word_correction_threshold: f64,
    /// App language code used for filler word selection (default "en")
    pub app_language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// Whether to save transcriptions to the history database
    pub enabled: bool,
    /// Maximum number of entries to keep (0 = unlimited when using count retention)
    pub limit: usize,
    /// How long to keep recordings
    pub retention: RecordingRetention,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordingRetention {
    /// Never delete recordings
    Never,
    /// Keep at most N entries
    PreserveLimit,
    Days3,
    Weeks2,
    Months3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Where to write transcription output
    pub method: OutputMethod,
    /// File path when method = "file"
    pub file_path: Option<String>,
    /// Append a newline after each transcription
    pub append_newline: bool,
    /// Append a trailing space after each transcription
    pub append_trailing_space: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputMethod {
    /// Print to stdout
    Stdout,
    /// Copy to system clipboard
    Clipboard,
    /// Append to a file
    File,
}

// ── Defaults ──────────────────────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            audio: AudioConfig::default(),
            model: ModelConfig::default(),
            transcription: TranscriptionConfig::default(),
            history: HistoryConfig::default(),
            output: OutputConfig::default(),
            hotkey: HotkeyConfig::default(),
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            combo: "ctrl+space".to_string(),
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: None,
            vad_threshold: 0.3,
            vad_enabled: true,
            max_duration_secs: 0,
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            selected: String::new(),
            unload_timeout: ModelUnloadTimeout::Never,
        }
    }
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            language: "auto".to_string(),
            translate_to_english: false,
            custom_words: Vec::new(),
            filter_filler_words: true,
            custom_filler_words: None,
            word_correction_threshold: 0.3,
            app_language: "en".to_string(),
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            limit: 100,
            retention: RecordingRetention::Months3,
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            method: OutputMethod::Stdout,
            file_path: None,
            append_newline: true,
            append_trailing_space: false,
        }
    }
}

// ── Load / Save ───────────────────────────────────────────────────────────────

pub fn load_config() -> Result<Config> {
    let path = crate::paths::config_file()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let contents = std::fs::read_to_string(&path)?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

pub fn save_config(config: &Config) -> Result<()> {
    let path = crate::paths::config_file()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(config)?;
    std::fs::write(&path, contents)?;
    Ok(())
}

/// Set a dot-separated key in the config. Supported keys:
///   model.selected, audio.device, audio.vad_threshold, audio.vad_enabled,
///   transcription.language, transcription.translate_to_english,
///   transcription.filter_filler_words, transcription.word_correction_threshold,
///   transcription.app_language, history.enabled, history.limit,
///   output.method, output.append_newline, output.append_trailing_space
pub fn set_config_key(config: &mut Config, key: &str, value: &str) -> Result<()> {
    match key {
        "model.selected" => config.model.selected = value.to_string(),
        "model.unload_timeout" => {
            config.model.unload_timeout = match value {
                "never" => ModelUnloadTimeout::Never,
                "immediately" => ModelUnloadTimeout::Immediately,
                "2min" | "min2" => ModelUnloadTimeout::Min2,
                "5min" | "min5" => ModelUnloadTimeout::Min5,
                "10min" | "min10" => ModelUnloadTimeout::Min10,
                "15min" | "min15" => ModelUnloadTimeout::Min15,
                "1h" | "hour1" => ModelUnloadTimeout::Hour1,
                _ => anyhow::bail!("Unknown unload timeout: {}. Use never/immediately/2min/5min/10min/15min/1h", value),
            }
        }
        "audio.device" => {
            config.audio.device = if value.is_empty() { None } else { Some(value.to_string()) }
        }
        "audio.vad_threshold" => {
            config.audio.vad_threshold = value.parse::<f32>()
                .map_err(|_| anyhow::anyhow!("vad_threshold must be a float between 0.0 and 1.0"))?;
        }
        "audio.vad_enabled" => {
            config.audio.vad_enabled = value.parse::<bool>()
                .map_err(|_| anyhow::anyhow!("vad_enabled must be true or false"))?;
        }
        "audio.max_duration_secs" => {
            config.audio.max_duration_secs = value.parse::<u64>()
                .map_err(|_| anyhow::anyhow!("max_duration_secs must be an integer"))?;
        }
        "transcription.language" => config.transcription.language = value.to_string(),
        "transcription.translate_to_english" => {
            config.transcription.translate_to_english = value.parse::<bool>()
                .map_err(|_| anyhow::anyhow!("translate_to_english must be true or false"))?;
        }
        "transcription.filter_filler_words" => {
            config.transcription.filter_filler_words = value.parse::<bool>()
                .map_err(|_| anyhow::anyhow!("filter_filler_words must be true or false"))?;
        }
        "transcription.word_correction_threshold" => {
            config.transcription.word_correction_threshold = value.parse::<f64>()
                .map_err(|_| anyhow::anyhow!("word_correction_threshold must be a float"))?;
        }
        "transcription.app_language" => config.transcription.app_language = value.to_string(),
        "history.enabled" => {
            config.history.enabled = value.parse::<bool>()
                .map_err(|_| anyhow::anyhow!("history.enabled must be true or false"))?;
        }
        "history.limit" => {
            config.history.limit = value.parse::<usize>()
                .map_err(|_| anyhow::anyhow!("history.limit must be an integer"))?;
        }
        "history.retention" => {
            config.history.retention = match value {
                "never" => RecordingRetention::Never,
                "preserve_limit" => RecordingRetention::PreserveLimit,
                "3days" | "days3" => RecordingRetention::Days3,
                "2weeks" | "weeks2" => RecordingRetention::Weeks2,
                "3months" | "months3" => RecordingRetention::Months3,
                _ => anyhow::bail!("Unknown retention: {}. Use never/preserve_limit/3days/2weeks/3months", value),
            }
        }
        "output.method" => {
            config.output.method = match value {
                "stdout" => OutputMethod::Stdout,
                "clipboard" => OutputMethod::Clipboard,
                "file" => OutputMethod::File,
                _ => anyhow::bail!("Unknown output method: {}. Use stdout/clipboard/file", value),
            }
        }
        "output.file_path" => {
            config.output.file_path = if value.is_empty() { None } else { Some(value.to_string()) }
        }
        "output.append_newline" => {
            config.output.append_newline = value.parse::<bool>()
                .map_err(|_| anyhow::anyhow!("append_newline must be true or false"))?;
        }
        "output.append_trailing_space" => {
            config.output.append_trailing_space = value.parse::<bool>()
                .map_err(|_| anyhow::anyhow!("append_trailing_space must be true or false"))?;
        }
        "hotkey.combo" => config.hotkey.combo = value.to_string(),
        _ => anyhow::bail!("Unknown config key: {}", key),
    }
    Ok(())
}
