//! MonoHub — Plexus RPC activation for the Monochrome music API
//!
//! Wraps the Monochrome / Hi-Fi Tidal proxy API and exposes track metadata,
//! album listings, artist info, search, lyrics, recommendations, and cover art
//! as streaming Plexus RPC methods.

use async_stream::stream;
use futures::Stream;
use std::sync::Arc;

use crate::client::MonoClient;
use crate::types::{MonoEvent, PlayStatus, SearchKind};

/// Monochrome music API activation — stateless except for the HTTP client.
#[derive(Clone)]
pub struct MonoHub {
    client: Arc<MonoClient>,
}

impl MonoHub {
    /// Create a hub targeting the default Monochrome API instance.
    pub fn new() -> Self {
        Self {
            client: Arc::new(MonoClient::default_instance()),
        }
    }

    /// Create a hub targeting a specific API base URL (no trailing slash).
    ///
    /// Example: `MonoHub::with_url("https://monochrome-api.samidy.com")`
    pub fn with_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Arc::new(MonoClient::new(base_url)),
        }
    }
}

impl Default for MonoHub {
    fn default() -> Self {
        Self::new()
    }
}

#[plexus_macros::hub_methods(
    namespace = "mono",
    version = "0.1.0",
    description = "Monochrome music API — track metadata, search, lyrics, and recommendations",
    crate_path = "plexus_core"
)]
impl MonoHub {
    /// Get track metadata by Tidal track ID
    #[plexus_macros::hub_method(
        description = "Fetch track metadata (title, artist, album, duration, audio quality)",
        params(id = "Tidal track ID (integer)")
    )]
    pub async fn track(
        &self,
        id: u64,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            match client.track_info(id).await {
                Ok(event) => yield event,
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Get album metadata and its full track listing by Tidal album ID
    #[plexus_macros::hub_method(
        streaming,
        description = "Fetch album metadata then stream each track. Yields Album followed by AlbumTrack events.",
        params(id = "Tidal album ID (integer)")
    )]
    pub async fn album(
        &self,
        id: u64,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            match client.album(id).await {
                Ok((album, tracks)) => {
                    yield album;
                    for track in tracks {
                        yield track;
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Get artist metadata by Tidal artist ID
    #[plexus_macros::hub_method(
        description = "Fetch artist name and image",
        params(id = "Tidal artist ID (integer)")
    )]
    pub async fn artist(
        &self,
        id: u64,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            match client.artist(id).await {
                Ok(event) => yield event,
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Search for tracks, albums, or artists
    #[plexus_macros::hub_method(
        streaming,
        description = "Search the Monochrome API. Streams one event per result.",
        params(
            query = "Search query string",
            kind = "What to search: tracks (default), albums, or artists",
            limit = "Maximum number of results (default 25, max 500)",
            offset = "Pagination offset (default 0)"
        )
    )]
    pub async fn search(
        &self,
        query: String,
        kind: Option<SearchKind>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        let kind = kind.unwrap_or_default();
        let limit = limit.unwrap_or(25).min(500);
        let offset = offset.unwrap_or(0);

        stream! {
            match client.search(&query, &kind, limit, offset).await {
                Ok(results) => {
                    if results.is_empty() {
                        yield MonoEvent::Error {
                            message: format!("no results for {:?}", query),
                        };
                    } else {
                        for event in results {
                            yield event;
                        }
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Get synchronized lyrics for a track by Tidal track ID
    #[plexus_macros::hub_method(
        streaming,
        description = "Fetch lyrics. Streams one LyricLine per line (with timestamps if available).",
        params(id = "Tidal track ID (integer)")
    )]
    pub async fn lyrics(
        &self,
        id: u64,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            match client.lyrics(id).await {
                Ok(lines) => {
                    for line in lines {
                        yield line;
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Get recommended tracks similar to a given track
    #[plexus_macros::hub_method(
        streaming,
        description = "Fetch track recommendations. Streams Recommendation events.",
        params(id = "Tidal track ID to base recommendations on")
    )]
    pub async fn recommendations(
        &self,
        id: u64,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            match client.recommendations(id).await {
                Ok(recs) => {
                    for rec in recs {
                        yield rec;
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Resolve the direct CDN stream URL for a track
    #[plexus_macros::hub_method(
        description = "Resolve the pre-signed stream URL for a track. Use the url immediately — it expires in ~60s.",
        params(
            id = "Tidal track ID",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW"
        )
    )]
    pub async fn stream_url(
        &self,
        id: u64,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            match client.stream_manifest(id, &quality).await {
                Ok(event) => yield event,
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Download a track to a local file
    #[plexus_macros::hub_method(
        streaming,
        description = "Download a track to disk. Streams DownloadProgress events then DownloadComplete.",
        params(
            id = "Tidal track ID",
            path = "Output file path (e.g. /tmp/track.flac)",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW"
        )
    )]
    pub async fn download(
        &self,
        id: u64,
        path: String,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            match client.download(id, &quality, &path).await {
                Ok(events) => {
                    for event in events {
                        yield event;
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Play a track via mpv (ephemeral — no file saved)
    #[plexus_macros::hub_method(
        streaming,
        description = "Play a track via mpv. Streams PlaybackStatus events for the duration of playback.",
        params(
            id = "Tidal track ID",
            quality = "Quality: LOSSLESS (default), HIGH, LOW"
        )
    )]
    pub async fn play(
        &self,
        id: u64,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            // Resolve manifest first
            let manifest = match client.stream_manifest(id, &quality).await {
                Ok(m) => m,
                Err(e) => {
                    yield MonoEvent::Error { message: e };
                    return;
                }
            };

            let url = match &manifest {
                MonoEvent::StreamManifest { url, .. } => url.clone(),
                _ => {
                    yield MonoEvent::Error { message: "unexpected manifest type".to_string() };
                    return;
                }
            };

            yield manifest;

            yield MonoEvent::PlaybackStatus {
                status: PlayStatus::Starting,
                elapsed_secs: None,
                duration_secs: None,
            };

            // Spawn mpv. Use --term-status-msg for parseable position output.
            let result = tokio::process::Command::new("mpv")
                .args([
                    "--no-video",
                    "--quiet",
                    "--term-status-msg=${time-pos}/${duration}",
                    &url,
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn();

            let mut child = match result {
                Ok(c) => c,
                Err(e) => {
                    yield MonoEvent::Error {
                        message: format!("failed to spawn mpv: {e}"),
                    };
                    return;
                }
            };

            yield MonoEvent::PlaybackStatus {
                status: PlayStatus::Playing,
                elapsed_secs: None,
                duration_secs: None,
            };

            // Read mpv stderr: collect all lines, parse progress from status lines.
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut stderr_lines: Vec<String> = Vec::new();
            if let Some(stderr) = child.stderr.take() {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    // mpv writes "HH:MM:SS.ss/HH:MM:SS.ss" via --term-status-msg
                    if let Some((elapsed, duration)) = parse_mpv_status(&line) {
                        yield MonoEvent::PlaybackStatus {
                            status: PlayStatus::Playing,
                            elapsed_secs: Some(elapsed),
                            duration_secs: Some(duration),
                        };
                    } else {
                        stderr_lines.push(line);
                    }
                }
            }

            let exit = child.wait().await;
            match exit {
                Ok(status) if status.success() => {
                    yield MonoEvent::PlaybackStatus {
                        status: PlayStatus::Finished,
                        elapsed_secs: None,
                        duration_secs: None,
                    };
                }
                Ok(status) => {
                    let detail = if stderr_lines.is_empty() {
                        format!("mpv exited with {status}")
                    } else {
                        format!("mpv exited with {status}: {}", stderr_lines.join("; "))
                    };
                    yield MonoEvent::Error { message: detail };
                }
                Err(e) => {
                    yield MonoEvent::Error {
                        message: format!("mpv wait failed: {e}"),
                    };
                }
            }
        }
    }

    /// Get cover art URL(s) for a track or album
    #[plexus_macros::hub_method(
        description = "Fetch cover art URL. Yields one or more Cover events with image URLs.",
        params(
            id = "Tidal track ID (integer)",
            size = "Image size in pixels (0 = all sizes: 80, 640, 1280 — default 1280)"
        )
    )]
    pub async fn cover(
        &self,
        id: u64,
        size: Option<u32>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        let size = size.unwrap_or(1280);
        stream! {
            match client.cover(id, size).await {
                Ok(covers) => {
                    for cover in covers {
                        yield cover;
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }
}

/// Parse mpv status lines. Handles two formats:
/// - `--term-status-msg` output: "HH:MM:SS/HH:MM:SS"
/// - Default AV line: "AV: HH:MM:SS / HH:MM:SS (X%)" or "A: HH:MM:SS / HH:MM:SS (X%)"
fn parse_mpv_status(line: &str) -> Option<(f32, f32)> {
    let line = line.trim();
    // Strip control chars and leading "AV: " / "A: " / "KA: " prefixes
    let line = line
        .trim_start_matches(|c: char| !c.is_ascii_digit() && c != ':')
        .trim();
    let (a, rest) = line.split_once('/')?;
    // Duration may be followed by " (X%)" — take up to space or end
    let b = rest.trim().split_whitespace().next().unwrap_or(rest.trim());
    let elapsed = parse_mpv_time(a.trim())?;
    let duration = parse_mpv_time(b)?;
    Some((elapsed, duration))
}

/// Parse mpv time format: "HH:MM:SS.ss" or "SS.ss" → seconds.
fn parse_mpv_time(s: &str) -> Option<f32> {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [ss] => ss.parse().ok(),
        [mm, ss] => {
            let m: f32 = mm.parse().ok()?;
            let s: f32 = ss.parse().ok()?;
            Some(m * 60.0 + s)
        }
        [hh, mm, ss] => {
            let h: f32 = hh.parse().ok()?;
            let m: f32 = mm.parse().ok()?;
            let s: f32 = ss.parse().ok()?;
            Some(h * 3600.0 + m * 60.0 + s)
        }
        _ => None,
    }
}
