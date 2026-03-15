use crate::config::Config;
use anyhow::Result;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tar::Archive;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EngineType {
    Whisper,
    Parakeet,
    Moonshine,
    MoonshineStreaming,
    SenseVoice,
    GigaAM,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub filename: String,
    pub url: Option<String>,
    pub size_mb: u64,
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub partial_size: u64,
    pub is_directory: bool,
    pub engine_type: EngineType,
    pub accuracy_score: f32,
    pub speed_score: f32,
    pub supports_translation: bool,
    pub is_recommended: bool,
    pub supported_languages: Vec<String>,
    pub is_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

pub struct ModelManager {
    models_dir: PathBuf,
    available_models: Mutex<HashMap<String, ModelInfo>>,
    cancel_flags: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    extracting_models: Arc<Mutex<HashSet<String>>>,
    /// Optional callback invoked with download progress updates
    progress_cb: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    /// Config reference for selected model updates
    config: Arc<Mutex<Config>>,
}

impl ModelManager {
    pub fn new(
        models_dir: PathBuf,
        config: Arc<Mutex<Config>>,
        progress_cb: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    ) -> Result<Self> {
        if !models_dir.exists() {
            fs::create_dir_all(&models_dir)?;
        }

        let mut available_models = HashMap::new();

        let whisper_languages: Vec<String> = vec![
            "en", "zh", "zh-Hans", "zh-Hant", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr",
            "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms",
            "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml",
            "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu",
            "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn",
            "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
            "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw",
            "ln", "ha", "ba", "jw", "su", "yue",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "small".to_string(),
            ModelInfo {
                id: "small".to_string(),
                name: "Whisper Small".to_string(),
                description: "Fast and fairly accurate.".to_string(),
                filename: "ggml-small.bin".to_string(),
                url: Some("https://blob.handy.computer/ggml-small.bin".to_string()),
                size_mb: 487,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.60,
                speed_score: 0.85,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                is_custom: false,
            },
        );

        available_models.insert(
            "medium".to_string(),
            ModelInfo {
                id: "medium".to_string(),
                name: "Whisper Medium".to_string(),
                description: "Good accuracy, medium speed.".to_string(),
                filename: "whisper-medium-q4_1.bin".to_string(),
                url: Some("https://blob.handy.computer/whisper-medium-q4_1.bin".to_string()),
                size_mb: 492,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.75,
                speed_score: 0.60,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                is_custom: false,
            },
        );

        available_models.insert(
            "turbo".to_string(),
            ModelInfo {
                id: "turbo".to_string(),
                name: "Whisper Turbo".to_string(),
                description: "Balanced accuracy and speed.".to_string(),
                filename: "ggml-large-v3-turbo.bin".to_string(),
                url: Some("https://blob.handy.computer/ggml-large-v3-turbo.bin".to_string()),
                size_mb: 1600,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.80,
                speed_score: 0.40,
                supports_translation: false,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                is_custom: false,
            },
        );

        available_models.insert(
            "large".to_string(),
            ModelInfo {
                id: "large".to_string(),
                name: "Whisper Large".to_string(),
                description: "High accuracy, slower.".to_string(),
                filename: "ggml-large-v3-q5_0.bin".to_string(),
                url: Some("https://blob.handy.computer/ggml-large-v3-q5_0.bin".to_string()),
                size_mb: 1100,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.85,
                speed_score: 0.30,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                is_custom: false,
            },
        );

        available_models.insert(
            "breeze-asr".to_string(),
            ModelInfo {
                id: "breeze-asr".to_string(),
                name: "Breeze ASR".to_string(),
                description: "Optimized for Taiwanese Mandarin. Code-switching support.".to_string(),
                filename: "breeze-asr-q5_k.bin".to_string(),
                url: Some("https://blob.handy.computer/breeze-asr-q5_k.bin".to_string()),
                size_mb: 1080,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.85,
                speed_score: 0.35,
                supports_translation: false,
                is_recommended: false,
                supported_languages: whisper_languages,
                is_custom: false,
            },
        );

        available_models.insert(
            "parakeet-tdt-0.6b-v2".to_string(),
            ModelInfo {
                id: "parakeet-tdt-0.6b-v2".to_string(),
                name: "Parakeet V2".to_string(),
                description: "English only. The best model for English speakers.".to_string(),
                filename: "parakeet-tdt-0.6b-v2-int8".to_string(),
                url: Some("https://blob.handy.computer/parakeet-v2-int8.tar.gz".to_string()),
                size_mb: 473,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Parakeet,
                accuracy_score: 0.85,
                speed_score: 0.85,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                is_custom: false,
            },
        );

        let parakeet_v3_languages: Vec<String> = vec![
            "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv",
            "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "parakeet-tdt-0.6b-v3".to_string(),
            ModelInfo {
                id: "parakeet-tdt-0.6b-v3".to_string(),
                name: "Parakeet V3".to_string(),
                description: "Fast and accurate. Supports 25 European languages.".to_string(),
                filename: "parakeet-tdt-0.6b-v3-int8".to_string(),
                url: Some("https://blob.handy.computer/parakeet-v3-int8.tar.gz".to_string()),
                size_mb: 478,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Parakeet,
                accuracy_score: 0.80,
                speed_score: 0.85,
                supports_translation: false,
                is_recommended: true,
                supported_languages: parakeet_v3_languages,
                is_custom: false,
            },
        );

        available_models.insert(
            "moonshine-base".to_string(),
            ModelInfo {
                id: "moonshine-base".to_string(),
                name: "Moonshine Base".to_string(),
                description: "Very fast, English only. Handles accents well.".to_string(),
                filename: "moonshine-base".to_string(),
                url: Some("https://blob.handy.computer/moonshine-base.tar.gz".to_string()),
                size_mb: 58,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Moonshine,
                accuracy_score: 0.70,
                speed_score: 0.90,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                is_custom: false,
            },
        );

        available_models.insert(
            "moonshine-tiny-streaming-en".to_string(),
            ModelInfo {
                id: "moonshine-tiny-streaming-en".to_string(),
                name: "Moonshine V2 Tiny".to_string(),
                description: "Ultra-fast, English only.".to_string(),
                filename: "moonshine-tiny-streaming-en".to_string(),
                url: Some(
                    "https://blob.handy.computer/moonshine-tiny-streaming-en.tar.gz".to_string(),
                ),
                size_mb: 31,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::MoonshineStreaming,
                accuracy_score: 0.55,
                speed_score: 0.95,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                is_custom: false,
            },
        );

        available_models.insert(
            "moonshine-small-streaming-en".to_string(),
            ModelInfo {
                id: "moonshine-small-streaming-en".to_string(),
                name: "Moonshine V2 Small".to_string(),
                description: "Fast, English only. Good balance of speed and accuracy.".to_string(),
                filename: "moonshine-small-streaming-en".to_string(),
                url: Some(
                    "https://blob.handy.computer/moonshine-small-streaming-en.tar.gz".to_string(),
                ),
                size_mb: 100,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::MoonshineStreaming,
                accuracy_score: 0.65,
                speed_score: 0.90,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                is_custom: false,
            },
        );

        available_models.insert(
            "moonshine-medium-streaming-en".to_string(),
            ModelInfo {
                id: "moonshine-medium-streaming-en".to_string(),
                name: "Moonshine V2 Medium".to_string(),
                description: "English only. High quality.".to_string(),
                filename: "moonshine-medium-streaming-en".to_string(),
                url: Some(
                    "https://blob.handy.computer/moonshine-medium-streaming-en.tar.gz".to_string(),
                ),
                size_mb: 192,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::MoonshineStreaming,
                accuracy_score: 0.75,
                speed_score: 0.80,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                is_custom: false,
            },
        );

        let sense_voice_languages: Vec<String> =
            vec!["zh", "zh-Hans", "zh-Hant", "en", "yue", "ja", "ko"]
                .into_iter()
                .map(String::from)
                .collect();

        available_models.insert(
            "sense-voice-int8".to_string(),
            ModelInfo {
                id: "sense-voice-int8".to_string(),
                name: "SenseVoice".to_string(),
                description: "Very fast. Chinese, English, Japanese, Korean, Cantonese.".to_string(),
                filename: "sense-voice-int8".to_string(),
                url: Some("https://blob.handy.computer/sense-voice-int8.tar.gz".to_string()),
                size_mb: 160,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::SenseVoice,
                accuracy_score: 0.65,
                speed_score: 0.95,
                supports_translation: false,
                is_recommended: false,
                supported_languages: sense_voice_languages,
                is_custom: false,
            },
        );

        available_models.insert(
            "gigaam-v3-e2e-ctc".to_string(),
            ModelInfo {
                id: "gigaam-v3-e2e-ctc".to_string(),
                name: "GigaAM v3".to_string(),
                description: "Russian speech recognition. Fast and accurate.".to_string(),
                filename: "giga-am-v3.int8.onnx".to_string(),
                url: Some("https://blob.handy.computer/giga-am-v3.int8.onnx".to_string()),
                size_mb: 225,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::GigaAM,
                accuracy_score: 0.85,
                speed_score: 0.75,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["ru".to_string()],
                is_custom: false,
            },
        );

        // Auto-discover custom Whisper .bin files
        if let Err(e) = Self::discover_custom_whisper_models(&models_dir, &mut available_models) {
            warn!("Failed to discover custom models: {}", e);
        }

        let manager = Self {
            models_dir,
            available_models: Mutex::new(available_models),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            extracting_models: Arc::new(Mutex::new(HashSet::new())),
            progress_cb,
            config,
        };

        manager.update_download_status()?;
        manager.auto_select_model_if_needed()?;

        Ok(manager)
    }

    pub fn get_available_models(&self) -> Vec<ModelInfo> {
        let models = self.available_models.lock().unwrap();
        let mut list: Vec<ModelInfo> = models.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        let models = self.available_models.lock().unwrap();
        models.get(model_id).cloned()
    }

    pub fn get_model_path(&self, model_id: &str) -> Result<PathBuf> {
        let models = self.available_models.lock().unwrap();
        let model = models
            .get(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;
        Ok(self.models_dir.join(&model.filename))
    }

    pub fn set_active_model(&self, model_id: &str) -> Result<()> {
        let models = self.available_models.lock().unwrap();
        if !models.contains_key(model_id) {
            anyhow::bail!("Model not found: {}", model_id);
        }
        drop(models);

        let mut cfg = self.config.lock().unwrap();
        cfg.model.selected = model_id.to_string();
        drop(cfg);

        let cfg = self.config.lock().unwrap();
        crate::config::save_config(&cfg)?;
        Ok(())
    }

    fn update_download_status(&self) -> Result<()> {
        let mut models = self.available_models.lock().unwrap();

        for model in models.values_mut() {
            if model.is_directory {
                let model_path = self.models_dir.join(&model.filename);
                let partial_path = self.models_dir.join(format!("{}.partial", &model.filename));
                let extracting_path = self
                    .models_dir
                    .join(format!("{}.extracting", &model.filename));

                // Clean up stale .extracting directories
                if extracting_path.exists() {
                    let extracting_models = self.extracting_models.lock().unwrap();
                    if !extracting_models.contains(&model.id) {
                        if let Err(e) = fs::remove_dir_all(&extracting_path) {
                            warn!("Failed to clean up extracting dir: {}", e);
                        }
                    }
                }

                model.is_downloaded = model_path.exists() && model_path.is_dir();
                model.partial_size = if partial_path.exists() {
                    fs::metadata(&partial_path)
                        .map(|m| m.len())
                        .unwrap_or(0)
                } else {
                    0
                };
            } else {
                let model_path = self.models_dir.join(&model.filename);
                let partial_path = self.models_dir.join(format!("{}.partial", &model.filename));

                model.is_downloaded = model_path.exists();
                model.partial_size = if partial_path.exists() {
                    fs::metadata(&partial_path)
                        .map(|m| m.len())
                        .unwrap_or(0)
                } else {
                    0
                };
            }
        }

        Ok(())
    }

    fn auto_select_model_if_needed(&self) -> Result<()> {
        let selected = {
            let cfg = self.config.lock().unwrap();
            cfg.model.selected.clone()
        };

        if !selected.is_empty() {
            return Ok(());
        }

        // Try Parakeet V3 first (recommended), then any downloaded model
        let models = self.available_models.lock().unwrap();
        let preferred = ["parakeet-tdt-0.6b-v3", "parakeet-tdt-0.6b-v2", "small"];

        for id in &preferred {
            if let Some(m) = models.get(*id) {
                if m.is_downloaded {
                    drop(models);
                    let mut cfg = self.config.lock().unwrap();
                    cfg.model.selected = id.to_string();
                    drop(cfg);
                    let cfg = self.config.lock().unwrap();
                    crate::config::save_config(&cfg)?;
                    info!("Auto-selected model: {}", id);
                    return Ok(());
                }
            }
        }

        // Fall back to first downloaded model
        let first_downloaded = models
            .values()
            .find(|m| m.is_downloaded)
            .map(|m| m.id.clone());

        drop(models);

        if let Some(id) = first_downloaded {
            let mut cfg = self.config.lock().unwrap();
            cfg.model.selected = id.clone();
            drop(cfg);
            let cfg = self.config.lock().unwrap();
            crate::config::save_config(&cfg)?;
            info!("Auto-selected model: {}", id);
        }

        Ok(())
    }

    fn discover_custom_whisper_models(
        models_dir: &Path,
        available_models: &mut HashMap<String, ModelInfo>,
    ) -> Result<()> {
        if !models_dir.exists() {
            return Ok(());
        }

        let known_filenames: HashSet<String> = available_models
            .values()
            .map(|m| m.filename.clone())
            .collect();

        for entry in fs::read_dir(models_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) == Some("bin") {
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                if known_filenames.contains(&filename) {
                    continue;
                }

                let model_id = format!(
                    "custom-{}",
                    path.file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                );

                let size_mb = fs::metadata(&path).map(|m| m.len() / (1024 * 1024)).unwrap_or(0);

                available_models.insert(
                    model_id.clone(),
                    ModelInfo {
                        id: model_id,
                        name: filename.clone(),
                        description: "Custom Whisper model".to_string(),
                        filename,
                        url: None,
                        size_mb,
                        is_downloaded: true,
                        is_downloading: false,
                        partial_size: 0,
                        is_directory: false,
                        engine_type: EngineType::Whisper,
                        accuracy_score: 0.0,
                        speed_score: 0.0,
                        supports_translation: true,
                        is_recommended: false,
                        supported_languages: vec!["auto".to_string()],
                        is_custom: true,
                    },
                );
            }
        }

        Ok(())
    }

    pub async fn download_model(&self, model_id: &str) -> Result<()> {
        let (url, filename, is_directory) = {
            let models = self.available_models.lock().unwrap();
            let model = models
                .get(model_id)
                .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

            if model.is_downloaded {
                return Ok(());
            }

            let url = model
                .url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Model {} has no download URL", model_id))?;

            (url, model.filename.clone(), model.is_directory)
        };

        // Set up cancellation flag
        let cancel_flag = Arc::new(AtomicBool::new(false));
        {
            let mut flags = self.cancel_flags.lock().unwrap();
            flags.insert(model_id.to_string(), cancel_flag.clone());
        }

        // Mark as downloading
        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(m) = models.get_mut(model_id) {
                m.is_downloading = true;
            }
        }

        let result = self
            .do_download(model_id, &url, &filename, is_directory, cancel_flag)
            .await;

        // Clear download flag
        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(m) = models.get_mut(model_id) {
                m.is_downloading = false;
                if result.is_ok() {
                    m.is_downloaded = true;
                }
            }
        }

        {
            let mut flags = self.cancel_flags.lock().unwrap();
            flags.remove(model_id);
        }

        result
    }

    async fn do_download(
        &self,
        model_id: &str,
        url: &str,
        filename: &str,
        is_directory: bool,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Download failed with status: {}", response.status());
        }

        let total = response.content_length().unwrap_or(0);

        if is_directory {
            // Download tar.gz and extract
            let partial_path = self.models_dir.join(format!("{}.partial.tar.gz", filename));
            let mut file = File::create(&partial_path)?;
            let mut downloaded: u64 = 0;
            let mut stream = response.bytes_stream();

            while let Some(chunk) = stream.next().await {
                if cancel_flag.load(Ordering::Relaxed) {
                    drop(file);
                    let _ = fs::remove_file(&partial_path);
                    anyhow::bail!("Download cancelled");
                }

                let chunk = chunk?;
                file.write_all(&chunk)?;
                downloaded += chunk.len() as u64;

                if let Some(cb) = &self.progress_cb {
                    cb(DownloadProgress {
                        model_id: model_id.to_string(),
                        downloaded,
                        total,
                        percentage: if total > 0 {
                            (downloaded as f64 / total as f64) * 100.0
                        } else {
                            0.0
                        },
                    });
                }
            }

            drop(file);

            // Extract
            let extracting_path = self.models_dir.join(format!("{}.extracting", filename));
            let final_path = self.models_dir.join(filename);

            {
                let mut extracting = self.extracting_models.lock().unwrap();
                extracting.insert(model_id.to_string());
            }

            debug!("Extracting {} to {:?}", filename, extracting_path);

            let gz = GzDecoder::new(File::open(&partial_path)?);
            let mut archive = Archive::new(gz);
            archive.unpack(&self.models_dir)?;

            // The archive should have extracted to a directory matching filename
            if !final_path.exists() {
                // Try to find what was extracted and rename if needed
                for entry in fs::read_dir(&self.models_dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_dir() && path != final_path {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if name.contains(&filename[..filename.len().min(10)]) {
                            fs::rename(&path, &final_path)?;
                            break;
                        }
                    }
                }
            }

            // Clean up
            let _ = fs::remove_file(&partial_path);
            if extracting_path.exists() {
                let _ = fs::remove_dir_all(&extracting_path);
            }

            {
                let mut extracting = self.extracting_models.lock().unwrap();
                extracting.remove(model_id);
            }

            if !final_path.exists() {
                anyhow::bail!("Extraction failed: expected directory {:?}", final_path);
            }
        } else {
            // Simple file download
            let partial_path = self.models_dir.join(format!("{}.partial", filename));
            let mut file = File::create(&partial_path)?;
            let mut downloaded: u64 = 0;
            let mut stream = response.bytes_stream();

            while let Some(chunk) = stream.next().await {
                if cancel_flag.load(Ordering::Relaxed) {
                    drop(file);
                    let _ = fs::remove_file(&partial_path);
                    anyhow::bail!("Download cancelled");
                }

                let chunk = chunk?;
                file.write_all(&chunk)?;
                downloaded += chunk.len() as u64;

                if let Some(cb) = &self.progress_cb {
                    cb(DownloadProgress {
                        model_id: model_id.to_string(),
                        downloaded,
                        total,
                        percentage: if total > 0 {
                            (downloaded as f64 / total as f64) * 100.0
                        } else {
                            0.0
                        },
                    });
                }
            }

            drop(file);

            let final_path = self.models_dir.join(filename);
            fs::rename(&partial_path, &final_path)?;
        }

        info!("Successfully downloaded model: {}", model_id);
        Ok(())
    }

    pub fn cancel_download(&self, model_id: &str) {
        let flags = self.cancel_flags.lock().unwrap();
        if let Some(flag) = flags.get(model_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn delete_model(&self, model_id: &str) -> Result<()> {
        let (filename, is_directory) = {
            let models = self.available_models.lock().unwrap();
            let model = models
                .get(model_id)
                .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

            if !model.is_downloaded {
                anyhow::bail!("Model {} is not downloaded", model_id);
            }

            (model.filename.clone(), model.is_directory)
        };

        let path = self.models_dir.join(&filename);

        if is_directory {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }

        let mut models = self.available_models.lock().unwrap();
        if let Some(m) = models.get_mut(model_id) {
            m.is_downloaded = false;
        }

        info!("Deleted model: {}", model_id);
        Ok(())
    }

    /// Ensure the Silero VAD model is present, downloading if necessary.
    pub async fn ensure_vad_model(&self) -> Result<PathBuf> {
        let vad_path = self.models_dir.join("silero_vad_v4.onnx");

        if vad_path.exists() {
            return Ok(vad_path);
        }

        info!("Downloading Silero VAD model...");
        let url = "https://blob.handy.computer/silero_vad_v4.onnx";
        let client = reqwest::Client::new();
        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to download VAD model: {}", response.status());
        }

        let bytes = response.bytes().await?;
        std::fs::write(&vad_path, &bytes)?;
        info!("VAD model downloaded to {:?}", vad_path);
        Ok(vad_path)
    }

    pub fn vad_model_path(&self) -> PathBuf {
        self.models_dir.join("silero_vad_v4.onnx")
    }
}
