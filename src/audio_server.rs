//! HTTP audio proxy for client-side failover.
//!
//! Serves audio files so the Tauri frontend can buffer the current track
//! and take over playback instantly when the backend disconnects.
//!
//! - Fast path: serve from local download cache
//! - Slow path: resolve stream manifest → proxy CDN bytes

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};

use crate::provider::MusicProvider;
use crate::storage::MonoStorage;
use crate::types::MonoEvent;

#[derive(Clone)]
struct AudioServerState {
    client: Arc<dyn MusicProvider>,
    storage: Arc<MonoStorage>,
}

#[derive(serde::Deserialize)]
struct AudioQuery {
    quality: Option<String>,
}

/// Start the HTTP audio server on the given port.
pub async fn start_audio_server(
    port: u16,
    client: Arc<dyn MusicProvider>,
    storage: Arc<MonoStorage>,
) -> anyhow::Result<()> {
    let state = AudioServerState { client, storage };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/audio/{track_id}", axum::routing::get(serve_audio))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    tracing::info!("  Audio HTTP: http://127.0.0.1:{port}/audio/{{track_id}}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn serve_audio(
    State(state): State<AudioServerState>,
    Path(track_id): Path<u64>,
    Query(query): Query<AudioQuery>,
) -> impl IntoResponse {
    let quality = query.quality.as_deref().unwrap_or("LOSSLESS");

    // Fast path: serve from local download cache
    if let Ok(Some(path)) = state.storage.get_download_path(track_id).await {
        let p = std::path::Path::new(&path);
        if p.exists() {
            match tokio::fs::read(p).await {
                Ok(bytes) => {
                    let content_type = mime_from_ext(p.extension().and_then(|e| e.to_str()));
                    tracing::debug!("audio proxy: serving local file for track {track_id}");
                    return (
                        StatusCode::OK,
                        [
                            (header::CONTENT_TYPE, content_type),
                            (header::CACHE_CONTROL, "no-cache".to_string()),
                        ],
                        bytes,
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::warn!("audio proxy: failed to read local file {path}: {e}");
                    // Fall through to CDN
                }
            }
        }
    }

    // Slow path: resolve manifest and proxy CDN bytes
    let manifest = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        state.client.stream_manifest(track_id, quality),
    )
    .await
    {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => {
            tracing::error!("audio proxy: manifest error for track {track_id}: {e}");
            return (StatusCode::BAD_GATEWAY, e).into_response();
        }
        Err(_) => {
            return (StatusCode::GATEWAY_TIMEOUT, "manifest timed out").into_response();
        }
    };

    let (url, content_type) = match &manifest {
        MonoEvent::StreamManifest { url, mime_type, .. } => (url.clone(), mime_type.clone()),
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "unexpected manifest type",
            )
                .into_response();
        }
    };

    // Proxy the CDN response
    let http_client = reqwest::Client::new();
    let cdn_resp = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        http_client.get(&url).send(),
    )
    .await
    {
        Ok(Ok(resp)) if resp.status().is_success() => resp,
        Ok(Ok(resp)) => {
            let status = resp.status();
            return (StatusCode::BAD_GATEWAY, format!("CDN returned {status}")).into_response();
        }
        Ok(Err(e)) => {
            return (StatusCode::BAD_GATEWAY, format!("CDN fetch failed: {e}")).into_response();
        }
        Err(_) => {
            return (StatusCode::GATEWAY_TIMEOUT, "CDN fetch timed out").into_response();
        }
    };

    match cdn_resp.bytes().await {
        Ok(bytes) => {
            tracing::debug!(
                "audio proxy: proxied {} bytes from CDN for track {track_id}",
                bytes.len()
            );
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "no-cache".to_string()),
                ],
                bytes.to_vec(),
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("CDN read failed: {e}")).into_response(),
    }
}

fn mime_from_ext(ext: Option<&str>) -> String {
    match ext {
        Some("flac") => "audio/flac",
        Some("m4a" | "aac") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
    .to_string()
}
