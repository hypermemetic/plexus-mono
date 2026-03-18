//! Music provider trait hierarchy
//!
//! Five capability traits that abstract over the concrete music backend.
//! Each trait is independently implementable — a lyrics-only provider
//! (e.g. Genius) can implement just `MusicLyrics`, while a full provider
//! like Monochrome implements all five.

use async_trait::async_trait;

use crate::types::{MusicEvent, SearchKind};

/// Audio streaming and download capability.
#[async_trait]
pub trait MusicStreaming: Send + Sync + 'static {
    /// Resolve a stream manifest (pre-signed CDN URL) for a track.
    async fn stream_manifest(&self, id: u64, quality: &str) -> Result<MusicEvent, String>;

    /// Download a track to a local file, streaming progress events.
    async fn download(
        &self,
        id: u64,
        quality: &str,
        path: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<MusicEvent>, String>;
}

/// Track, album, and artist metadata lookup.
#[async_trait]
pub trait MusicMetadata: Send + Sync + 'static {
    /// Fetch track metadata by ID.
    async fn track_info(&self, id: u64) -> Result<MusicEvent, String>;

    /// Fetch album metadata and its track listing.
    async fn album(&self, id: u64) -> Result<(MusicEvent, Vec<MusicEvent>), String>;

    /// Fetch artist information by ID.
    async fn artist(&self, id: u64) -> Result<MusicEvent, String>;
}

/// Search across tracks, albums, and artists.
#[async_trait]
pub trait MusicSearch: Send + Sync + 'static {
    /// Search for music by query string.
    async fn search(
        &self,
        query: &str,
        kind: &SearchKind,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MusicEvent>, String>;
}

/// Lyrics retrieval.
#[async_trait]
pub trait MusicLyrics: Send + Sync + 'static {
    /// Fetch lyrics for a track (synced if available).
    async fn lyrics(&self, id: u64) -> Result<Vec<MusicEvent>, String>;
}

/// Enrichment data: cover art and recommendations.
#[async_trait]
pub trait MusicEnrichment: Send + Sync + 'static {
    /// Fetch cover art URLs for a track/album.
    async fn cover(&self, id: u64, size: u32) -> Result<Vec<MusicEvent>, String>;

    /// Fetch recommended tracks similar to a given track.
    async fn recommendations(&self, id: u64) -> Result<Vec<MusicEvent>, String>;
}

/// Blanket trait for providers that implement all capabilities.
pub trait MusicProvider:
    MusicStreaming + MusicMetadata + MusicSearch + MusicLyrics + MusicEnrichment
{
}

impl<T: MusicStreaming + MusicMetadata + MusicSearch + MusicLyrics + MusicEnrichment> MusicProvider
    for T
{
}
