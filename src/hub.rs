//! MonoHub — Plexus RPC activation for the Monochrome music API
//!
//! Wraps the Monochrome / Hi-Fi Tidal proxy API and exposes track metadata,
//! album listings, artist info, search, lyrics, recommendations, cover art,
//! and full playback controls as streaming Plexus RPC methods.

use async_stream::stream;
use futures::Stream;
use std::sync::Arc;

use crate::client::MonoClient;
use crate::player::Player;
use crate::types::{MonoEvent, SearchKind};

/// Monochrome music API activation with playback engine.
#[derive(Clone)]
pub struct MonoHub {
    client: Arc<MonoClient>,
    player: Arc<Player>,
}

impl MonoHub {
    /// Create a hub targeting the default Monochrome API instance.
    pub async fn new() -> Self {
        let client = Arc::new(MonoClient::default_instance());
        let player = Player::new(client.clone()).await;
        Self { client, player }
    }

    /// Create a hub targeting a specific API base URL (no trailing slash).
    pub async fn with_url(base_url: impl Into<String>) -> Self {
        let client = Arc::new(MonoClient::new(base_url));
        let player = Player::new(client.clone()).await;
        Self { client, player }
    }
}

#[plexus_macros::hub_methods(
    namespace = "mono",
    version = "0.2.0",
    description = "Monochrome music API — track metadata, search, lyrics, recommendations, and playback",
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

    /// Play a track immediately (stops current playback)
    #[plexus_macros::hub_method(
        description = "Play a track through speakers. Stops any current playback.",
        params(
            id = "Tidal track ID",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW"
        )
    )]
    pub async fn play(
        &self,
        id: u64,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            match player.play_track(id, &quality).await {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "play".to_string(),
                    message: format!("playing track {id}"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Pause playback
    #[plexus_macros::hub_method(
        description = "Pause the current playback"
    )]
    pub async fn pause(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            player.pause().await;
            yield MonoEvent::PlayerAck {
                action: "pause".to_string(),
                message: "playback paused".to_string(),
            };
        }
    }

    /// Resume playback
    #[plexus_macros::hub_method(
        description = "Resume paused playback"
    )]
    pub async fn resume(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            player.resume().await;
            yield MonoEvent::PlayerAck {
                action: "resume".to_string(),
                message: "playback resumed".to_string(),
            };
        }
    }

    /// Stop playback
    #[plexus_macros::hub_method(
        description = "Stop playback and clear current track"
    )]
    pub async fn stop(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            player.stop().await;
            yield MonoEvent::PlayerAck {
                action: "stop".to_string(),
                message: "playback stopped".to_string(),
            };
        }
    }

    /// Skip to next track in queue
    #[plexus_macros::hub_method(
        description = "Skip to the next track in the queue"
    )]
    pub async fn next(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            match player.next().await {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "next".to_string(),
                    message: "skipped to next track".to_string(),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Go to previous track
    #[plexus_macros::hub_method(
        description = "Go back to the previous track"
    )]
    pub async fn previous(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            match player.previous().await {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "previous".to_string(),
                    message: "went to previous track".to_string(),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Set volume level
    #[plexus_macros::hub_method(
        description = "Set playback volume",
        params(level = "Volume level from 0.0 (mute) to 1.0 (full)")
    )]
    pub async fn volume(
        &self,
        level: f32,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            player.set_volume(level).await;
            yield MonoEvent::PlayerAck {
                action: "volume".to_string(),
                message: format!("volume set to {:.0}%", level * 100.0),
            };
        }
    }

    /// Add a track to the playback queue
    #[plexus_macros::hub_method(
        description = "Add a track to the end of the playback queue. Auto-starts if nothing is playing.",
        params(
            id = "Tidal track ID",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW"
        )
    )]
    pub async fn queue_add(
        &self,
        id: u64,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            match player.queue_add(id, &quality).await {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "queue_add".to_string(),
                    message: format!("track {id} added to queue"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Clear the playback queue
    #[plexus_macros::hub_method(
        description = "Clear all tracks from the queue (does not stop current track)"
    )]
    pub async fn queue_clear(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            player.queue_clear().await;
            yield MonoEvent::PlayerAck {
                action: "queue_clear".to_string(),
                message: "queue cleared".to_string(),
            };
        }
    }

    /// List queue contents
    #[plexus_macros::hub_method(
        description = "Get the current queue contents including the now-playing track"
    )]
    pub async fn queue_get(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            let (current, upcoming) = player.queue_get().await;
            let current_index = if current.is_some() { Some(0usize) } else { None };
            let mut tracks = Vec::new();
            if let Some(c) = current {
                tracks.push(c);
            }
            tracks.extend(upcoming);
            yield MonoEvent::Queue {
                tracks,
                current_index,
            };
        }
    }

    /// Reorder tracks in the queue
    #[plexus_macros::hub_method(
        description = "Move a track within the queue",
        params(
            from = "Source index in the queue (0-based)",
            to = "Destination index in the queue (0-based)"
        )
    )]
    pub async fn queue_reorder(
        &self,
        from: u32,
        to: u32,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            match player.queue_reorder(from as usize, to as usize).await {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "queue_reorder".to_string(),
                    message: format!("moved track from position {from} to {to}"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Stream now-playing updates (~1s while playing)
    #[plexus_macros::hub_method(
        streaming,
        description = "Stream real-time playback position and status updates (~1s interval while playing)"
    )]
    pub async fn now_playing(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let mut rx = self.player.subscribe_now_playing();
        stream! {
            // Emit current state immediately
            {
                let np = rx.borrow().clone();
                yield MonoEvent::NowPlaying {
                    track_id: np.track_id,
                    title: np.title,
                    artist: np.artist,
                    album: np.album,
                    status: np.status,
                    position_secs: np.position_secs,
                    duration_secs: np.duration_secs,
                    volume: np.volume,
                    queue_length: np.queue_length,
                };
            }
            // Then stream updates
            while rx.changed().await.is_ok() {
                let np = rx.borrow().clone();
                yield MonoEvent::NowPlaying {
                    track_id: np.track_id,
                    title: np.title,
                    artist: np.artist,
                    album: np.album,
                    status: np.status,
                    position_secs: np.position_secs,
                    duration_secs: np.duration_secs,
                    volume: np.volume,
                    queue_length: np.queue_length,
                };
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
