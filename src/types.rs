//! Event types for the Monochrome music API activation

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Events emitted by Monochrome API activation methods
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MonoEvent {
    /// Track metadata from /info/?id=
    Track {
        /// Tidal track ID
        id: u64,
        /// Track title (including version if present)
        title: String,
        /// Primary artist name
        artist: String,
        /// Album title
        album: String,
        /// Tidal album ID
        album_id: u64,
        /// Duration in seconds
        duration_secs: u64,
        /// Track number within the album
        track_number: Option<u32>,
        /// Release date (ISO 8601)
        release_date: Option<String>,
        /// Audio quality (e.g. "LOSSLESS", "HI_RES_LOSSLESS", "HIGH")
        audio_quality: Option<String>,
        /// Tidal cover UUID (use with cover_url to build image URLs)
        cover_id: Option<String>,
    },

    /// Album metadata from /album/?id=
    Album {
        /// Tidal album ID
        id: u64,
        /// Album title
        title: String,
        /// Primary artist name
        artist: String,
        /// Release date (ISO 8601)
        release_date: Option<String>,
        /// Number of tracks in this album
        track_count: u32,
        /// Total album duration in seconds
        duration_secs: Option<u64>,
        /// Tidal cover UUID
        cover_id: Option<String>,
    },

    /// Individual track within an album listing (follows Album event)
    AlbumTrack {
        /// 1-based position in the album
        position: u32,
        /// Tidal track ID
        id: u64,
        /// Track title
        title: String,
        /// Primary artist name
        artist: String,
        /// Duration in seconds
        duration_secs: u64,
        /// Audio quality
        audio_quality: Option<String>,
    },

    /// Artist information from /artist/?id=
    Artist {
        /// Tidal artist ID
        id: u64,
        /// Artist name
        name: String,
        /// Tidal picture UUID (same format as track/album artist picture field)
        picture_id: Option<String>,
        /// Full cover image URL at 750x750 (directly from API response)
        cover_url: Option<String>,
    },

    /// A track from search results (follows SearchStart)
    SearchTrack {
        /// 0-based rank in results
        rank: u32,
        /// Tidal track ID
        id: u64,
        /// Track title
        title: String,
        /// Primary artist name
        artist: String,
        /// Album title
        album: String,
        /// Duration in seconds
        duration_secs: u64,
        /// Audio quality
        audio_quality: Option<String>,
    },

    /// An album from search results
    SearchAlbum {
        /// 0-based rank in results
        rank: u32,
        /// Tidal album ID
        id: u64,
        /// Album title
        title: String,
        /// Primary artist name
        artist: String,
        /// Number of tracks
        track_count: u32,
        /// Release date
        release_date: Option<String>,
    },

    /// An artist from search results
    SearchArtist {
        /// 0-based rank in results
        rank: u32,
        /// Tidal artist ID
        id: u64,
        /// Artist name
        name: String,
    },

    /// A single lyrics line from /lyrics/?id=
    LyricLine {
        /// Timestamp in milliseconds (None for unsynced lyrics)
        timestamp_ms: Option<u64>,
        /// Lyric text
        text: String,
    },

    /// A recommended track from /recommendations/?id=
    Recommendation {
        /// 0-based rank in results
        rank: u32,
        /// Tidal track ID
        id: u64,
        /// Track title
        title: String,
        /// Primary artist name
        artist: String,
        /// Duration in seconds
        duration_secs: u64,
    },

    /// A cover art URL from /cover/?id=
    Cover {
        /// Full HTTPS URL to the cover image
        url: String,
        /// Image dimension in pixels (width == height)
        size: u32,
    },

    /// Resolved stream manifest from /track/?id= (use url immediately — it expires)
    StreamManifest {
        /// Tidal track ID
        id: u64,
        /// Pre-signed direct CDN URL — short-lived, use within seconds
        url: String,
        /// MIME type: "audio/flac", "audio/mp4", "audio/mpeg"
        mime_type: String,
        /// Codec string: "flac", "mp4a.40.2", etc.
        codecs: String,
        /// Quality tier used: "LOSSLESS", "HIGH", "LOW", "HI_RES_LOSSLESS"
        quality: String,
        /// Bit depth (present for LOSSLESS / HI_RES_LOSSLESS)
        bit_depth: Option<u32>,
        /// Sample rate in Hz (present for LOSSLESS / HI_RES_LOSSLESS)
        sample_rate: Option<u32>,
        /// File extension inferred from MIME type
        extension: String,
    },

    /// Streaming download progress
    DownloadProgress {
        /// Absolute path of the file being written
        path: String,
        /// Bytes downloaded so far
        bytes_downloaded: u64,
        /// Total bytes (None if server didn't send Content-Length)
        total_bytes: Option<u64>,
        /// Completion percentage (None if total unknown)
        percent: Option<f32>,
    },

    /// Download finished successfully
    DownloadComplete {
        /// Absolute path of the saved file
        path: String,
        /// Total bytes written
        bytes: u64,
        /// MIME type of the saved audio
        mime_type: String,
    },

    /// Playback status event from mpv
    PlaybackStatus {
        /// Current player state
        status: PlayStatus,
        /// Elapsed seconds (available while playing)
        elapsed_secs: Option<f32>,
        /// Total duration in seconds (available while playing)
        duration_secs: Option<f32>,
    },

    /// Current playback state — streamed ~1s via now_playing
    NowPlaying {
        /// Currently playing track ID
        track_id: Option<u64>,
        /// Track title
        title: Option<String>,
        /// Artist name
        artist: Option<String>,
        /// Album title
        album: Option<String>,
        /// Player state
        status: PlayStatus,
        /// Current position in seconds
        position_secs: f32,
        /// Total duration in seconds
        duration_secs: f32,
        /// Volume level 0.0–1.0
        volume: f32,
        /// Pre-amp gain 0.0–4.0 (>1.0 boosts)
        preamp: f32,
        /// Number of tracks in queue
        queue_length: usize,
        /// Monochrome web URL for the current track
        url: Option<String>,
    },

    /// Queue contents snapshot
    Queue {
        /// Tracks in queue (current + upcoming)
        tracks: Vec<QueuedTrack>,
        /// Index of the currently playing track (if any)
        current_index: Option<usize>,
    },

    /// Playlist summary info (from playlist list)
    PlaylistInfo {
        /// Playlist name
        name: String,
        /// Playlist description
        description: String,
        /// Number of tracks in the playlist
        track_count: usize,
        /// ISO 8601 creation timestamp
        created_at: String,
        /// ISO 8601 last-updated timestamp
        updated_at: String,
    },

    /// Acknowledgement of a player action
    PlayerAck {
        /// Action that was performed
        action: String,
        /// Human-readable message
        message: String,
    },

    /// Error from any method
    Error {
        /// Human-readable error description
        message: String,
    },
}

/// Playback lifecycle state
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlayStatus {
    /// No track loaded
    Idle,
    /// Player process spawned, buffering/starting
    Starting,
    /// Buffering audio data from network
    Buffering,
    /// Currently playing
    Playing,
    /// Playback paused
    Paused,
    /// Playback stopped by user
    Stopped,
    /// Playback finished cleanly
    Finished,
    /// Player exited with an error
    Failed,
}

/// A track in the playback queue
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueuedTrack {
    /// Tidal track ID
    pub id: u64,
    /// Track title
    pub title: String,
    /// Primary artist name
    pub artist: String,
    /// Album title
    pub album: String,
    /// Duration in seconds
    pub duration_secs: u64,
    /// Quality tier requested
    pub quality: String,
    /// Cover art UUID
    pub cover_id: Option<String>,
}

/// Search target kind
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// Search for tracks (default)
    Tracks,
    /// Search for albums
    Albums,
    /// Search for artists
    Artists,
}

impl Default for SearchKind {
    fn default() -> Self {
        SearchKind::Tracks
    }
}
