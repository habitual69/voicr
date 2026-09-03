//! Local config web UI, started alongside `voicr daemon`.
//!
//! Binds to 127.0.0.1 only — this is a developer convenience for editing config,
//! not something meant to be exposed on the network.

use crate::config::Config;
use crate::managers::model::{DownloadProgress, ModelManager};
use log::{error, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};
use tokio::runtime::Handle;

const UI_HTML: &str = include_str!("web_ui.html");
const ICON_PNG: &[u8] = include_bytes!("../icons/voicr.iconset/icon_128x128@2x.png");
const GROQ_SVG: &str = include_str!("../icons/groq.svg");
const OPENAI_SVG: &str = include_str!("../icons/openai.svg");
const SARVAM_SVG: &str = include_str!("../icons/sarvam.svg");

/// Latest progress for each model currently downloading, keyed by model id.
/// Entries are removed once the download finishes (success or failure).
pub type DownloadProgressMap = Arc<Mutex<HashMap<String, DownloadProgress>>>;

pub fn spawn(
    config: Arc<Mutex<Config>>,
    model_manager: Arc<ModelManager>,
    download_progress: DownloadProgressMap,
    runtime: Handle,
    port: u16,
) {
    std::thread::spawn(move || {
        let addr = format!("127.0.0.1:{}", port);
        let server = match Server::http(&addr) {
            Ok(s) => s,
            Err(e) => {
                warn!("Web UI failed to bind {}: {}", addr, e);
                return;
            }
        };
        info!("Web UI: http://{}", addr);

        for request in server.incoming_requests() {
            if let Err(e) = handle(
                request,
                &config,
                &model_manager,
                &download_progress,
                &runtime,
            ) {
                error!("Web UI request error: {}", e);
            }
        }
    });
}

fn handle(
    mut request: Request,
    config: &Arc<Mutex<Config>>,
    model_manager: &Arc<ModelManager>,
    download_progress: &DownloadProgressMap,
    runtime: &Handle,
) -> std::io::Result<()> {
    let method = request.method().clone();
    let url = request.url().to_string();

    match (&method, url.as_str()) {
        (Method::Get, "/") => respond_html(request, UI_HTML),
        (Method::Get, "/icon.png") => respond_png(request, ICON_PNG),
        (Method::Get, "/icons/groq.svg") => respond_svg(request, GROQ_SVG),
        (Method::Get, "/icons/openai.svg") => respond_svg(request, OPENAI_SVG),
        (Method::Get, "/icons/sarvam.svg") => respond_svg(request, SARVAM_SVG),
        (Method::Get, "/api/config") => {
            let cfg = config.lock().unwrap().clone();
            respond_json(request, 200, &cfg)
        }
        (Method::Put, "/api/config") => {
            let body = read_body(&mut request)?;
            match serde_json::from_str::<Config>(&body) {
                Ok(new_cfg) => {
                    if let Err(e) = crate::config::save_config(&new_cfg) {
                        return respond_json(
                            request,
                            500,
                            &serde_json::json!({"error": e.to_string()}),
                        );
                    }
                    *config.lock().unwrap() = new_cfg.clone();
                    respond_json(request, 200, &new_cfg)
                }
                Err(e) => respond_json(
                    request,
                    400,
                    &serde_json::json!({"error": format!("Invalid config: {}", e)}),
                ),
            }
        }
        (Method::Get, "/api/models") => {
            let selected = config.lock().unwrap().model.selected.clone();
            let models = model_manager.get_available_models();
            let progress = download_progress.lock().unwrap();
            let out: Vec<_> = models
                .into_iter()
                .map(|m| {
                    let pct = progress.get(&m.id).map(|p| p.percentage);
                    serde_json::json!({
                        "id": m.id,
                        "name": m.name,
                        "size_mb": m.size_mb,
                        "is_downloaded": m.is_downloaded,
                        "is_downloading": m.is_downloading,
                        "download_percentage": pct,
                        "is_recommended": m.is_recommended,
                        "selected": m.id == selected,
                    })
                })
                .collect();
            respond_json(request, 200, &out)
        }
        (Method::Post, "/api/models/download") => {
            let body = read_body(&mut request)?;
            let id = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string));
            let Some(id) = id else {
                return respond_json(
                    request,
                    400,
                    &serde_json::json!({"error": "Missing \"id\" field"}),
                );
            };
            if model_manager.get_model_info(&id).is_none() {
                return respond_json(
                    request,
                    404,
                    &serde_json::json!({"error": format!("Unknown model: {}", id)}),
                );
            }

            // Kick off the download on the tokio runtime and return immediately;
            // the web UI polls GET /api/models for progress.
            let mm = model_manager.clone();
            let cfg = config.clone();
            let progress_map = download_progress.clone();
            let download_id = id.clone();
            runtime.spawn(async move {
                match mm.download_model(&download_id).await {
                    Ok(()) => {
                        if let Err(e) = mm.set_active_model(&download_id) {
                            error!("Failed to auto-select {}: {}", download_id, e);
                        } else {
                            cfg.lock().unwrap().model.selected = download_id.clone();
                        }
                    }
                    Err(e) => error!("Download of {} failed: {}", download_id, e),
                }
                progress_map.lock().unwrap().remove(&download_id);
            });

            respond_json(request, 202, &serde_json::json!({"ok": true}))
        }
        (Method::Post, "/api/models/select") => {
            let body = read_body(&mut request)?;
            let id = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string));
            match id {
                Some(id) => match model_manager.set_active_model(&id) {
                    Ok(()) => {
                        config.lock().unwrap().model.selected = id;
                        respond_json(request, 200, &serde_json::json!({"ok": true}))
                    }
                    Err(e) => {
                        respond_json(request, 400, &serde_json::json!({"error": e.to_string()}))
                    }
                },
                None => respond_json(
                    request,
                    400,
                    &serde_json::json!({"error": "Missing \"id\" field"}),
                ),
            }
        }
        (Method::Get, "/api/cloud_models") => {
            let cfg = config.lock().unwrap().clone();
            
            if cfg.cloud.provider == crate::config::CloudProvider::SarvamAI {
                let models = vec!["saaras:v3".to_string(), "saaras:v2".to_string()];
                return respond_json(request, 200, &models);
            }

            let (api_key, base_url) = match cfg.cloud.provider {
                crate::config::CloudProvider::Groq => (cfg.cloud.groq_api_key.clone(), "https://api.groq.com/openai/v1"),
                crate::config::CloudProvider::OpenAI => (cfg.cloud.openai_api_key.clone(), "https://api.openai.com/v1"),
                crate::config::CloudProvider::Custom => {
                    let key = cfg.cloud.custom_api_key.clone();
                    let url = cfg.cloud.custom_base_url.clone().unwrap_or_default();
                    (key, Box::leak(url.into_boxed_str()) as &str)
                },
                crate::config::CloudProvider::SarvamAI => unreachable!(),
            };
            
            if let Some(key) = api_key {
                let client = reqwest::blocking::Client::new();
                let url = format!("{}/models", base_url.trim_end_matches('/'));
                match client.get(&url).bearer_auth(key).send() {
                    Ok(resp) => {
                        if let Ok(json) = resp.json::<serde_json::Value>() {
                            let mut models = vec![];
                            if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                                for m in data {
                                    if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                                        models.push(id.to_string());
                                    }
                                }
                            }
                            if cfg.cloud.provider == crate::config::CloudProvider::Groq {
                                models.retain(|m| m.contains("whisper"));
                            } else if cfg.cloud.provider == crate::config::CloudProvider::OpenAI {
                                models.retain(|m| m.contains("whisper"));
                            }
                            respond_json(request, 200, &models)
                        } else {
                            respond_json(request, 500, &serde_json::json!({"error": "Invalid response from provider"}))
                        }
                    },
                    Err(e) => respond_json(request, 500, &serde_json::json!({"error": e.to_string()})),
                }
            } else {
                respond_json(request, 400, &serde_json::json!({"error": "API key not set for this provider"}))
            }
        }
        (Method::Post, "/api/restart") => {
            info!("Restart requested from web UI");
            respond_json(request, 200, &serde_json::json!({"ok": true}))?;
            restart_process();
        }
        (Method::Get, "/api/audio_devices") => {
            let devices = crate::audio_toolkit::list_input_devices().unwrap_or_default();
            let out: Vec<_> = devices
                .into_iter()
                .map(|d| {
                    serde_json::json!({
                        "name": d.name,
                        "is_default": d.is_default
                    })
                })
                .collect();
            respond_json(request, 200, &out)
        }
        (Method::Get, "/api/autostart") => {
            let enabled = crate::autostart::is_enabled();
            respond_json(request, 200, &serde_json::json!({ "enabled": enabled }))
        }
        (Method::Post, "/api/autostart") => {
            let body = read_body(&mut request)?;
            let enabled = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
                .unwrap_or(false);

            let res = if enabled {
                crate::autostart::enable()
            } else {
                crate::autostart::disable()
            };

            match res {
                Ok(()) => respond_json(
                    request,
                    200,
                    &serde_json::json!({ "enabled": crate::autostart::is_enabled() }),
                ),
                Err(e) => respond_json(request, 500, &serde_json::json!({ "error": e.to_string() })),
            }
        }
        _ => request.respond(Response::from_string("Not found").with_status_code(404)),
    }
}

/// Re-run the same binary with the same args in place of this process.
/// Same PID on Unix (via execve), so the PID file stays valid and there's
/// no race with a second process trying to bind the same socket/port.
fn restart_process() -> ! {
    let exe = std::env::current_exe().expect("current_exe");
    let args: Vec<String> = std::env::args().skip(1).collect();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe).args(&args).exec();
        error!("Restart failed: {}", err);
        std::process::exit(1);
    }

    // ponytail: Windows has no exec(); this leaves a brief window where two
    // processes could race on the PID file. Fine for a manual restart button.
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new(&exe).args(&args).spawn();
        std::process::exit(0);
    }
}

fn read_body(request: &mut Request) -> std::io::Result<String> {
    let mut body = String::new();
    request.as_reader().read_to_string(&mut body)?;
    Ok(body)
}

fn respond_html(request: Request, html: &str) -> std::io::Result<()> {
    let header_ct = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("valid header");
    let header_cc = Header::from_bytes(&b"Cache-Control"[..], &b"no-cache, no-store, must-revalidate"[..]).expect("valid header");
    request.respond(Response::from_string(html).with_header(header_ct).with_header(header_cc))
}

fn respond_png(request: Request, bytes: &[u8]) -> std::io::Result<()> {
    let header_ct = Header::from_bytes(&b"Content-Type"[..], &b"image/png"[..]).expect("valid header");
    let header_cc = Header::from_bytes(&b"Cache-Control"[..], &b"no-cache, no-store, must-revalidate"[..]).expect("valid header");
    request.respond(Response::from_data(bytes).with_header(header_ct).with_header(header_cc))
}

fn respond_svg(request: Request, svg: &str) -> std::io::Result<()> {
    let header_ct = Header::from_bytes(&b"Content-Type"[..], &b"image/svg+xml"[..]).expect("valid header");
    let header_cc = Header::from_bytes(&b"Cache-Control"[..], &b"public, max-age=86400"[..]).expect("valid header");
    request.respond(Response::from_string(svg).with_header(header_ct).with_header(header_cc))
}

fn respond_json<T: serde::Serialize>(
    request: Request,
    status: u16,
    value: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let header_ct = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("valid header");
    let header_cc = Header::from_bytes(&b"Cache-Control"[..], &b"no-cache, no-store, must-revalidate"[..]).expect("valid header");
    let response = Response::from_string(body)
        .with_header(header_ct)
        .with_header(header_cc)
        .with_status_code(status);
    request.respond(response)
}
