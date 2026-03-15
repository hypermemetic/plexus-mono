//! MonoHub — Plexus RPC activation for the Monochrome music API
//!
//! Stateless API proxy: track metadata, album listings, artist info,
//! search, lyrics, recommendations, cover art, stream URLs, and downloads.
//! No audio hardware, no persistence.

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use std::sync::Arc;

use plexus_core::plexus::{ChildRouter, PlexusError, PlexusStream};
use plexus_core::Activation;

use crate::client::MonoClient;
use crate::types::{MonoEvent, SearchKind};

/// Monochrome music API activation — stateless API proxy.
#[derive(Clone)]
pub struct MonoHub {
    client: Arc<MonoClient>,
}

impl MonoHub {
    /// Create a hub targeting the default Monochrome API instance.
    pub async fn new() -> Self {
        let client = Arc::new(MonoClient::default_instance());
        Self { client }
    }

    /// Create a hub targeting a specific API base URL (no trailing slash).
    pub async fn with_url(base_url: impl Into<String>) -> Self {
        let client = Arc::new(MonoClient::new(base_url));
        Self { client }
    }

    /// Get a shared reference to the underlying MonoClient.
    pub fn client(&self) -> Arc<MonoClient> {
        self.client.clone()
    }

    /// No children — leaf hub for schema compatibility
    pub fn plugin_children(&self) -> Vec<plexus_core::plexus::schema::ChildSummary> {
        vec![]
    }
}

#[plexus_macros::hub_methods(
    namespace = "monochrome",
    version = "0.2.0",
    hub,
    description = "Monochrome music API — track metadata, search, lyrics, recommendations, cover art",
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
                Ok(mut rx) => {
                    while let Some(event) = rx.recv().await {
                        yield event;
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
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

#[async_trait]
impl ChildRouter for MonoHub {
    fn router_namespace(&self) -> &str {
        "monochrome"
    }

    async fn router_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<PlexusStream, PlexusError> {
        self.call(method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None
    }
}
