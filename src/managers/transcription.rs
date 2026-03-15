use crate::audio_toolkit::{apply_custom_words, filter_transcription_output};
use crate::config::{Config, ModelUnloadTimeout};
use crate::managers::model::{EngineType, ModelManager};
use anyhow::Result;
use log::{debug, error, info, warn};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime};
use transcribe_rs::{
    engines::{
        gigaam::GigaAMEngine,
        moonshine::{
            ModelVariant, MoonshineEngine, MoonshineModelParams, MoonshineStreamingEngine,
            StreamingModelParams,
        },
        parakeet::{
            ParakeetEngine, ParakeetInferenceParams, ParakeetModelParams, TimestampGranularity,
        },
        sense_voice::{
            Language as SenseVoiceLanguage, SenseVoiceEngine, SenseVoiceInferenceParams,
            SenseVoiceModelParams,
        },
    },
    TranscriptionEngine,
};
#[cfg(feature = "whisper")]
use transcribe_rs::engines::whisper::{WhisperEngine, WhisperInferenceParams};

pub type StatusCallback = Arc<dyn Fn(ModelStatus) + Send + Sync>;

#[derive(Debug, Clone)]
pub enum ModelStatus {
    Loading { model_id: String },
    Loaded { model_id: String, model_name: String },
    Unloaded,
    Error { model_id: String, message: String },
}

enum LoadedEngine {
    #[cfg(feature = "whisper")]
    Whisper(WhisperEngine),
    Parakeet(ParakeetEngine),
    Moonshine(MoonshineEngine),
    MoonshineStreaming(MoonshineStreamingEngine),
    SenseVoice(SenseVoiceEngine),
    GigaAM(GigaAMEngine),
}

struct LoadingGuard {
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        let mut is_loading = self.is_loading.lock().unwrap();
        *is_loading = false;
        self.loading_condvar.notify_all();
    }
}

#[derive(Clone)]
pub struct TranscriptionManager {
    engine: Arc<Mutex<Option<LoadedEngine>>>,
    model_manager: Arc<ModelManager>,
    config: Arc<Mutex<Config>>,
    current_model_id: Arc<Mutex<Option<String>>>,
    last_activity: Arc<AtomicU64>,
    shutdown_signal: Arc<AtomicBool>,
    watcher_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
    status_cb: Option<StatusCallback>,
}

impl TranscriptionManager {
    pub fn new(
        model_manager: Arc<ModelManager>,
        config: Arc<Mutex<Config>>,
        status_cb: Option<StatusCallback>,
    ) -> Result<Self> {
        let manager = Self {
            engine: Arc::new(Mutex::new(None)),
            model_manager,
            config: config.clone(),
            current_model_id: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(AtomicU64::new(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            )),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            watcher_handle: Arc::new(Mutex::new(None)),
            is_loading: Arc::new(Mutex::new(false)),
            loading_condvar: Arc::new(Condvar::new()),
            status_cb,
        };

        // Start idle watcher thread
        {
            let manager_cloned = manager.clone();
            let shutdown_signal = manager.shutdown_signal.clone();
            let handle = thread::spawn(move || {
                while !shutdown_signal.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(10));

                    if shutdown_signal.load(Ordering::Relaxed) {
                        break;
                    }

                    let timeout = {
                        let cfg = manager_cloned.config.lock().unwrap();
                        cfg.model.unload_timeout.clone()
                    };

                    if let Some(limit_seconds) = timeout.to_seconds() {
                        if limit_seconds == 0 {
                            continue; // "Immediately" is handled per-transcription
                        }
                        let last = manager_cloned.last_activity.load(Ordering::Relaxed);
                        let now_ms = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64;

                        if now_ms.saturating_sub(last) > limit_seconds * 1000 {
                            if manager_cloned.is_model_loaded() {
                                debug!("Unloading model due to inactivity");
                                if let Ok(()) = manager_cloned.unload_model() {
                                    if let Some(cb) = &manager_cloned.status_cb {
                                        cb(ModelStatus::Unloaded);
                                    }
                                }
                            }
                        }
                    }
                }
            });
            *manager.watcher_handle.lock().unwrap() = Some(handle);
        }

        Ok(manager)
    }

    fn lock_engine(&self) -> MutexGuard<'_, Option<LoadedEngine>> {
        self.engine.lock().unwrap_or_else(|poisoned| {
            warn!("Engine mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }

    pub fn is_model_loaded(&self) -> bool {
        self.lock_engine().is_some()
    }

    pub fn get_current_model(&self) -> Option<String> {
        self.current_model_id.lock().unwrap().clone()
    }

    pub fn unload_model(&self) -> Result<()> {
        {
            let mut engine = self.lock_engine();
            if let Some(ref mut loaded_engine) = *engine {
                match loaded_engine {
                    #[cfg(feature = "whisper")]
                    LoadedEngine::Whisper(ref mut e) => e.unload_model(),
                    LoadedEngine::Parakeet(ref mut e) => e.unload_model(),
                    LoadedEngine::Moonshine(ref mut e) => e.unload_model(),
                    LoadedEngine::MoonshineStreaming(ref mut e) => e.unload_model(),
                    LoadedEngine::SenseVoice(ref mut e) => e.unload_model(),
                    LoadedEngine::GigaAM(ref mut e) => e.unload_model(),
                }
            }
            *engine = None;
        }
        *self.current_model_id.lock().unwrap() = None;

        if let Some(cb) = &self.status_cb {
            cb(ModelStatus::Unloaded);
        }

        Ok(())
    }

    pub fn load_model(&self, model_id: &str) -> Result<()> {
        let load_start = std::time::Instant::now();
        debug!("Loading model: {}", model_id);

        if let Some(cb) = &self.status_cb {
            cb(ModelStatus::Loading {
                model_id: model_id.to_string(),
            });
        }

        let model_info = self
            .model_manager
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        if !model_info.is_downloaded {
            let msg = format!("Model '{}' is not downloaded. Run: voicr model download {}", model_info.name, model_id);
            if let Some(cb) = &self.status_cb {
                cb(ModelStatus::Error {
                    model_id: model_id.to_string(),
                    message: msg.clone(),
                });
            }
            return Err(anyhow::anyhow!(msg));
        }

        let model_path = self.model_manager.get_model_path(model_id)?;

        let loaded_engine = match model_info.engine_type {
            #[cfg(feature = "whisper")]
            EngineType::Whisper => {
                let mut engine = WhisperEngine::new();
                engine
                    .load_model(&model_path)
                    .map_err(|e| anyhow::anyhow!("Failed to load Whisper model: {}", e))?;
                LoadedEngine::Whisper(engine)
            }
            #[cfg(not(feature = "whisper"))]
            EngineType::Whisper => {
                anyhow::bail!("Whisper support not compiled in. Rebuild with: cargo build --features whisper\n(Requires: sudo apt install libvulkan-dev glslang-tools)");
            }
            EngineType::Parakeet => {
                let mut engine = ParakeetEngine::new();
                engine
                    .load_model_with_params(&model_path, ParakeetModelParams::int8())
                    .map_err(|e| anyhow::anyhow!("Failed to load Parakeet model: {}", e))?;
                LoadedEngine::Parakeet(engine)
            }
            EngineType::Moonshine => {
                let mut engine = MoonshineEngine::new();
                engine
                    .load_model_with_params(
                        &model_path,
                        MoonshineModelParams::variant(ModelVariant::Base),
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to load Moonshine model: {}", e))?;
                LoadedEngine::Moonshine(engine)
            }
            EngineType::MoonshineStreaming => {
                let mut engine = MoonshineStreamingEngine::new();
                engine
                    .load_model_with_params(&model_path, StreamingModelParams::default())
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to load Moonshine streaming model: {}", e)
                    })?;
                LoadedEngine::MoonshineStreaming(engine)
            }
            EngineType::SenseVoice => {
                let mut engine = SenseVoiceEngine::new();
                engine
                    .load_model_with_params(&model_path, SenseVoiceModelParams::int8())
                    .map_err(|e| anyhow::anyhow!("Failed to load SenseVoice model: {}", e))?;
                LoadedEngine::SenseVoice(engine)
            }
            EngineType::GigaAM => {
                let mut engine = GigaAMEngine::new();
                engine
                    .load_model(&model_path)
                    .map_err(|e| anyhow::anyhow!("Failed to load GigaAM model: {}", e))?;
                LoadedEngine::GigaAM(engine)
            }
        };

        {
            let mut engine = self.lock_engine();
            *engine = Some(loaded_engine);
        }
        *self.current_model_id.lock().unwrap() = Some(model_id.to_string());

        if let Some(cb) = &self.status_cb {
            cb(ModelStatus::Loaded {
                model_id: model_id.to_string(),
                model_name: model_info.name.clone(),
            });
        }

        debug!(
            "Model loaded: {} ({}ms)",
            model_id,
            load_start.elapsed().as_millis()
        );
        Ok(())
    }

    /// Load the configured model if not already loaded.
    pub fn ensure_model_loaded(&self) -> Result<()> {
        // Wait if loading is in progress
        {
            let mut is_loading = self.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.loading_condvar.wait(is_loading).unwrap();
            }
        }

        if self.is_model_loaded() {
            return Ok(());
        }

        // Mark as loading
        {
            let mut is_loading = self.is_loading.lock().unwrap();
            *is_loading = true;
        }
        let guard = LoadingGuard {
            is_loading: self.is_loading.clone(),
            loading_condvar: self.loading_condvar.clone(),
        };

        let model_id = {
            let cfg = self.config.lock().unwrap();
            cfg.model.selected.clone()
        };

        if model_id.is_empty() {
            drop(guard);
            anyhow::bail!("No model selected. Run: voicr model set <model-id>");
        }

        let result = self.load_model(&model_id);
        drop(guard); // clears is_loading and notifies condvar
        result
    }

    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        self.last_activity.store(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            Ordering::Relaxed,
        );

        let st = std::time::Instant::now();
        debug!("Audio samples: {}", audio.len());

        if audio.is_empty() {
            debug!("Empty audio, skipping transcription");
            return Ok(String::new());
        }

        // Wait for any in-progress model load
        {
            let mut is_loading = self.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.loading_condvar.wait(is_loading).unwrap();
            }
        }

        if !self.is_model_loaded() {
            return Err(anyhow::anyhow!(
                "No model loaded. Call ensure_model_loaded() first."
            ));
        }

        let cfg = self.config.lock().unwrap().clone();

        let result = {
            let mut engine_guard = self.lock_engine();
            let mut engine = match engine_guard.take() {
                Some(e) => e,
                None => {
                    return Err(anyhow::anyhow!("Model not available for transcription."));
                }
            };
            drop(engine_guard);

            let transcribe_result = catch_unwind(AssertUnwindSafe(
                || -> Result<transcribe_rs::TranscriptionResult> {
                    match &mut engine {
                        #[cfg(feature = "whisper")]
                        LoadedEngine::Whisper(whisper_engine) => {
                            let whisper_language = if cfg.transcription.language == "auto" {
                                None
                            } else {
                                let normalized = if cfg.transcription.language == "zh-Hans"
                                    || cfg.transcription.language == "zh-Hant"
                                {
                                    "zh".to_string()
                                } else {
                                    cfg.transcription.language.clone()
                                };
                                Some(normalized)
                            };

                            let params = WhisperInferenceParams {
                                language: whisper_language,
                                translate: cfg.transcription.translate_to_english,
                                initial_prompt: if cfg.transcription.custom_words.is_empty() {
                                    None
                                } else {
                                    Some(cfg.transcription.custom_words.join(", "))
                                },
                                ..Default::default()
                            };

                            whisper_engine
                                .transcribe_samples(audio, Some(params))
                                .map_err(|e| anyhow::anyhow!("Whisper transcription failed: {}", e))
                        }
                        LoadedEngine::Parakeet(parakeet_engine) => {
                            let params = ParakeetInferenceParams {
                                timestamp_granularity: TimestampGranularity::Segment,
                                ..Default::default()
                            };
                            parakeet_engine
                                .transcribe_samples(audio, Some(params))
                                .map_err(|e| {
                                    anyhow::anyhow!("Parakeet transcription failed: {}", e)
                                })
                        }
                        LoadedEngine::Moonshine(moonshine_engine) => moonshine_engine
                            .transcribe_samples(audio, None)
                            .map_err(|e| anyhow::anyhow!("Moonshine transcription failed: {}", e)),
                        LoadedEngine::MoonshineStreaming(streaming_engine) => streaming_engine
                            .transcribe_samples(audio, None)
                            .map_err(|e| {
                                anyhow::anyhow!("Moonshine streaming transcription failed: {}", e)
                            }),
                        LoadedEngine::SenseVoice(sense_voice_engine) => {
                            let language = match cfg.transcription.language.as_str() {
                                "zh" | "zh-Hans" | "zh-Hant" => SenseVoiceLanguage::Chinese,
                                "en" => SenseVoiceLanguage::English,
                                "ja" => SenseVoiceLanguage::Japanese,
                                "ko" => SenseVoiceLanguage::Korean,
                                "yue" => SenseVoiceLanguage::Cantonese,
                                _ => SenseVoiceLanguage::Auto,
                            };
                            let params = SenseVoiceInferenceParams {
                                language,
                                use_itn: true,
                            };
                            sense_voice_engine
                                .transcribe_samples(audio, Some(params))
                                .map_err(|e| {
                                    anyhow::anyhow!("SenseVoice transcription failed: {}", e)
                                })
                        }
                        LoadedEngine::GigaAM(gigaam_engine) => gigaam_engine
                            .transcribe_samples(audio, None)
                            .map_err(|e| anyhow::anyhow!("GigaAM transcription failed: {}", e)),
                    }
                },
            ));

            match transcribe_result {
                Ok(inner_result) => {
                    let mut engine_guard = self.lock_engine();
                    *engine_guard = Some(engine);
                    inner_result?
                }
                Err(panic_payload) => {
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    error!("Transcription engine panicked: {}", panic_msg);
                    *self.current_model_id.lock().unwrap_or_else(|e| e.into_inner()) = None;

                    if let Some(cb) = &self.status_cb {
                        cb(ModelStatus::Error {
                            model_id: String::new(),
                            message: format!("Engine panicked: {}", panic_msg),
                        });
                    }

                    return Err(anyhow::anyhow!(
                        "Transcription engine panicked: {}. Model unloaded, will reload on next attempt.",
                        panic_msg
                    ));
                }
            }
        };

        // Apply word correction (skip for Whisper since custom_words passed as initial_prompt)
        let model_id = cfg.model.selected.clone();
        let is_whisper = {
            #[cfg(feature = "whisper")]
            {
                self.model_manager
                    .get_model_info(&model_id)
                    .map(|info| matches!(info.engine_type, EngineType::Whisper))
                    .unwrap_or(false)
            }
            #[cfg(not(feature = "whisper"))]
            { false }
        };

        let corrected = if !cfg.transcription.custom_words.is_empty() && !is_whisper {
            apply_custom_words(
                &result.text,
                &cfg.transcription.custom_words,
                cfg.transcription.word_correction_threshold,
            )
        } else {
            result.text
        };

        // Filter filler words
        let filtered = if cfg.transcription.filter_filler_words {
            filter_transcription_output(
                &corrected,
                &cfg.transcription.app_language,
                &cfg.transcription.custom_filler_words,
            )
        } else {
            corrected
        };

        info!(
            "Transcription done ({}ms): {}",
            st.elapsed().as_millis(),
            if filtered.is_empty() { "(empty)" } else { &filtered }
        );

        // Maybe unload immediately
        if cfg.model.unload_timeout == ModelUnloadTimeout::Immediately && self.is_model_loaded() {
            if let Err(e) = self.unload_model() {
                warn!("Failed to unload model immediately: {}", e);
            }
        }

        Ok(filtered)
    }
}

impl Drop for TranscriptionManager {
    fn drop(&mut self) {
        self.shutdown_signal.store(true, Ordering::Relaxed);
        if let Some(handle) = self.watcher_handle.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}
