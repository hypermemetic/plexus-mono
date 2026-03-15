//! PlaylistHub — persistent playlist management for plexus-mono
//!
//! Stores playlists as JSON files under `~/.plexus/monochrome/player/playlists/`.
//! Registered as a child activation of MonoHub via ChildRouter.

use async_stream::stream;
use async_trait::async_trait;
use futures::{self, Stream};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use plexus_core::plexus::{ChildRouter, PlexusError, PlexusStream};
use plexus_core::Activation;

use serde_json::Value;

use crate::client::MonoClient;
use crate::player::Player;
use crate::types::{MonoEvent, QueuedTrack};

/// On-disk playlist format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistData {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub tracks: Vec<QueuedTrack>,
    pub created_at: String,
    pub updated_at: String,
}

/// Persistent playlist management activation
#[derive(Clone)]
pub struct PlaylistHub {
    player: Arc<Player>,
    client: Arc<MonoClient>,
    data_dir: PathBuf,
}

impl PlaylistHub {
    pub fn new(player: Arc<Player>, client: Arc<MonoClient>) -> Self {
        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".plexus/monochrome/player/playlists");
        Self {
            player,
            client,
            data_dir,
        }
    }

    fn playlist_path(&self, name: &str) -> PathBuf {
        self.data_dir.join(format!("{name}.json"))
    }

    fn ensure_dir(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.data_dir)
            .map_err(|e| format!("failed to create playlist dir: {e}"))
    }

    fn load(&self, name: &str) -> Result<PlaylistData, String> {
        let path = self.playlist_path(name);
        let data = std::fs::read_to_string(&path)
            .map_err(|e| format!("playlist '{name}' not found: {e}"))?;
        serde_json::from_str(&data)
            .map_err(|e| format!("failed to parse playlist '{name}': {e}"))
    }

    fn write_playlist(&self, data: &PlaylistData) -> Result<(), String> {
        self.ensure_dir()?;
        let path = self.playlist_path(&data.name);
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| format!("failed to serialize playlist: {e}"))?;
        std::fs::write(&path, json)
            .map_err(|e| format!("failed to write playlist: {e}"))
    }

    fn now_iso() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    fn research_dir(&self) -> PathBuf {
        self.data_dir.parent().unwrap_or(&self.data_dir).join("research")
    }

    fn research_path(&self, name: &str) -> PathBuf {
        self.research_dir().join(format!("{name}.json"))
    }

    fn write_research(&self, name: &str, data: &Value) -> Result<(), String> {
        let dir = self.research_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("failed to create research dir: {e}"))?;
        let path = self.research_path(name);
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| format!("failed to serialize research: {e}"))?;
        std::fs::write(&path, json)
            .map_err(|e| format!("failed to write research: {e}"))
    }

    fn load_research(&self, name: &str) -> Result<Value, String> {
        let path = self.research_path(name);
        let data = std::fs::read_to_string(&path)
            .map_err(|e| format!("research '{name}' not found: {e}"))?;
        serde_json::from_str(&data)
            .map_err(|e| format!("failed to parse research '{name}': {e}"))
    }
}

#[plexus_macros::hub_methods(
    namespace = "playlist",
    version = "0.1.0",
    description = "Persistent playlist management — save, load, and play named track lists",
    crate_path = "plexus_core"
)]
impl PlaylistHub {
    /// Create an empty or pre-populated playlist
    #[plexus_macros::hub_method(
        streaming,
        description = "Create a new playlist. Pass track IDs to pre-populate, or omit for empty.",
        params(
            name = "Playlist name",
            description = "Optional description of the playlist",
            ids = "Optional list of Tidal track IDs to populate the playlist with",
            quality = "Quality tier for track metadata (default LOSSLESS)"
        )
    )]
    pub async fn create(
        &self,
        name: String,
        description: Option<String>,
        ids: Option<Vec<u64>>,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".into());
        stream! {
            if hub.playlist_path(&name).exists() {
                yield MonoEvent::Error { message: format!("playlist '{name}' already exists") };
                return;
            }
            let tracks = if let Some(ids) = ids {
                // Resolve all track metadata in parallel
                let futs: Vec<_> = ids.iter().map(|&id| {
                    let client = hub.client.clone();
                    let q = quality.clone();
                    async move {
                        let info = client.track_info(id).await.ok();
                        match info {
                            Some(MonoEvent::Track { title, artist, album, duration_secs, cover_id, .. }) => {
                                QueuedTrack { id, title, artist, album, duration_secs, quality: q, cover_id, source: None }
                            }
                            _ => QueuedTrack {
                                id, title: format!("Track {id}"), artist: String::new(),
                                album: String::new(), duration_secs: 0, quality: q, cover_id: None, source: None,
                            },
                        }
                    }
                }).collect();
                futures::future::join_all(futs).await
            } else {
                vec![]
            };
            let count = tracks.len();
            let description = description.unwrap_or_default();
            let data = PlaylistData {
                name: name.clone(),
                description,
                tracks,
                created_at: Self::now_iso(),
                updated_at: Self::now_iso(),
            };
            match hub.write_playlist(&data) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "playlist_create".into(),
                    message: if count > 0 {
                        format!("created playlist '{name}' with {count} tracks")
                    } else {
                        format!("created playlist '{name}'")
                    },
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// List all saved playlists
    #[plexus_macros::hub_method(
        streaming,
        description = "List all saved playlists with summary info"
    )]
    pub async fn list(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            if let Err(e) = hub.ensure_dir() {
                yield MonoEvent::Error { message: e };
                return;
            }
            let entries = match std::fs::read_dir(&hub.data_dir) {
                Ok(e) => e,
                Err(e) => {
                    yield MonoEvent::Error { message: format!("failed to read playlist dir: {e}") };
                    return;
                }
            };
            let mut found = false;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    if let Ok(data) = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<PlaylistData>(&s).ok())
                        .ok_or(())
                    {
                        found = true;
                        yield MonoEvent::PlaylistInfo {
                            name: data.name,
                            description: data.description,
                            track_count: data.tracks.len(),
                            created_at: data.created_at,
                            updated_at: data.updated_at,
                        };
                    }
                }
            }
            if !found {
                yield MonoEvent::PlayerAck {
                    action: "playlist_list".into(),
                    message: "no playlists found".into(),
                };
            }
        }
    }

    /// Get full playlist info (metadata + tracks) — suitable for UI rendering
    #[plexus_macros::hub_method(
        streaming,
        description = "Get full playlist details: name, description, track count, timestamps, then all tracks",
        params(name = "Playlist name")
    )]
    pub async fn show(
        &self,
        name: String,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            match hub.load(&name) {
                Ok(data) => {
                    yield MonoEvent::PlaylistInfo {
                        name: data.name,
                        description: data.description,
                        track_count: data.tracks.len(),
                        created_at: data.created_at,
                        updated_at: data.updated_at,
                    };
                    yield MonoEvent::Queue {
                        tracks: data.tracks,
                        current_index: None,
                    };
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Delete a playlist
    #[plexus_macros::hub_method(
        description = "Delete a saved playlist",
        params(name = "Playlist name")
    )]
    pub async fn delete(
        &self,
        name: String,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            let path = hub.playlist_path(&name);
            match std::fs::remove_file(&path) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "playlist_delete".into(),
                    message: format!("deleted playlist '{name}'"),
                },
                Err(e) => yield MonoEvent::Error {
                    message: format!("failed to delete playlist '{name}': {e}"),
                },
            }
        }
    }

    /// Rename a playlist
    #[plexus_macros::hub_method(
        description = "Rename a playlist",
        params(
            name = "Current playlist name",
            new_name = "New playlist name"
        )
    )]
    pub async fn rename(
        &self,
        name: String,
        new_name: String,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            match hub.load(&name) {
                Ok(mut data) => {
                    if hub.playlist_path(&new_name).exists() {
                        yield MonoEvent::Error {
                            message: format!("playlist '{new_name}' already exists"),
                        };
                        return;
                    }
                    // Remove old file
                    let _ = std::fs::remove_file(hub.playlist_path(&name));
                    data.name = new_name.clone();
                    data.updated_at = Self::now_iso();
                    match hub.write_playlist(&data) {
                        Ok(()) => yield MonoEvent::PlayerAck {
                            action: "playlist_rename".into(),
                            message: format!("renamed '{name}' to '{new_name}'"),
                        },
                        Err(e) => yield MonoEvent::Error { message: e },
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Add a track to a playlist by ID
    #[plexus_macros::hub_method(
        description = "Fetch track info and append to a playlist",
        params(
            name = "Playlist name",
            id = "Tidal track ID",
            quality = "Quality tier (default LOSSLESS)"
        )
    )]
    pub async fn add(
        &self,
        name: String,
        id: u64,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".into());
        stream! {
            let mut data = match hub.load(&name) {
                Ok(d) => d,
                Err(e) => { yield MonoEvent::Error { message: e }; return; }
            };
            // Fetch track metadata
            let track_info = hub.client.track_info(id).await.ok();
            let queued = match track_info {
                Some(MonoEvent::Track { title, artist, album, duration_secs, cover_id, .. }) => {
                    QueuedTrack { id, title, artist, album, duration_secs, quality, cover_id, source: None }
                }
                _ => QueuedTrack {
                    id,
                    title: format!("Track {id}"),
                    artist: String::new(),
                    album: String::new(),
                    duration_secs: 0,
                    quality,
                    cover_id: None,
                    source: None,
                },
            };
            let track_title = queued.title.clone();
            data.tracks.push(queued);
            data.updated_at = Self::now_iso();
            match hub.write_playlist(&data) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "playlist_add".into(),
                    message: format!("added '{track_title}' to playlist '{name}'"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Remove a track from a playlist by index
    #[plexus_macros::hub_method(
        description = "Remove a track at a given index from a playlist",
        params(
            name = "Playlist name",
            index = "0-based index of the track to remove"
        )
    )]
    pub async fn remove(
        &self,
        name: String,
        index: u32,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            let mut data = match hub.load(&name) {
                Ok(d) => d,
                Err(e) => { yield MonoEvent::Error { message: e }; return; }
            };
            let idx = index as usize;
            if idx >= data.tracks.len() {
                yield MonoEvent::Error {
                    message: format!("index {index} out of bounds (playlist has {} tracks)", data.tracks.len()),
                };
                return;
            }
            let removed = data.tracks.remove(idx);
            data.updated_at = Self::now_iso();
            match hub.write_playlist(&data) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "playlist_remove".into(),
                    message: format!("removed '{}' from playlist '{name}'", removed.title),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Set or update a playlist's description
    #[plexus_macros::hub_method(
        description = "Set or update the description of a playlist",
        params(
            name = "Playlist name",
            description = "New description text"
        )
    )]
    pub async fn describe(
        &self,
        name: String,
        description: String,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            let mut data = match hub.load(&name) {
                Ok(d) => d,
                Err(e) => { yield MonoEvent::Error { message: e }; return; }
            };
            data.description = description;
            data.updated_at = Self::now_iso();
            match hub.write_playlist(&data) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "playlist_describe".into(),
                    message: format!("updated description for playlist '{name}'"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Reorder a track within a playlist
    #[plexus_macros::hub_method(
        description = "Move a track within a playlist from one position to another",
        params(
            name = "Playlist name",
            from = "Source index (0-based)",
            to = "Destination index (0-based)"
        )
    )]
    pub async fn reorder(
        &self,
        name: String,
        from: u32,
        to: u32,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            let mut data = match hub.load(&name) {
                Ok(d) => d,
                Err(e) => { yield MonoEvent::Error { message: e }; return; }
            };
            let (f, t) = (from as usize, to as usize);
            if f >= data.tracks.len() || t >= data.tracks.len() {
                yield MonoEvent::Error {
                    message: format!("index out of bounds (playlist has {} tracks)", data.tracks.len()),
                };
                return;
            }
            let track = data.tracks.remove(f);
            let title = track.title.clone();
            data.tracks.insert(t, track);
            data.updated_at = Self::now_iso();
            match hub.write_playlist(&data) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "playlist_reorder".into(),
                    message: format!("moved '{title}' from position {from} to {to}"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Load a playlist into the queue and start playing
    #[plexus_macros::hub_method(
        description = "Load playlist tracks into the playback queue and start playing",
        params(name = "Playlist name")
    )]
    pub async fn play(
        &self,
        name: String,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            let data = match hub.load(&name) {
                Ok(d) => d,
                Err(e) => { yield MonoEvent::Error { message: e }; return; }
            };
            if data.tracks.is_empty() {
                yield MonoEvent::Error {
                    message: format!("playlist '{name}' is empty"),
                };
                return;
            }
            // Stop current playback and clear queue before loading playlist
            hub.player.stop().await;
            hub.player.queue_clear().await;
            for track in &data.tracks {
                match hub.player.queue_add_with_source(track.id, &track.quality, Some(name.clone())).await {
                    Ok(()) => {}
                    Err(e) => {
                        yield MonoEvent::Error { message: e };
                        return;
                    }
                }
            }
            yield MonoEvent::PlayerAck {
                action: "playlist_play".into(),
                message: format!("playing playlist '{name}' ({} tracks)", data.tracks.len()),
            };
        }
    }

    /// Save the current queue as a playlist
    #[plexus_macros::hub_method(
        description = "Save the current playback queue as a named playlist (creates or overwrites)",
        params(name = "Playlist name")
    )]
    pub async fn save(
        &self,
        name: String,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            let (current, upcoming) = hub.player.queue_get().await;
            let mut tracks = Vec::new();
            if let Some(c) = current {
                tracks.push(c);
            }
            tracks.extend(upcoming);
            if tracks.is_empty() {
                yield MonoEvent::Error {
                    message: "queue is empty — nothing to save".into(),
                };
                return;
            }
            let count = tracks.len();
            let now = Self::now_iso();
            let data = PlaylistData {
                name: name.clone(),
                description: String::new(),
                tracks,
                created_at: now.clone(),
                updated_at: now,
            };
            match hub.write_playlist(&data) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "playlist_save".into(),
                    message: format!("saved {count} tracks as playlist '{name}'"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Save AI research data associated with a playlist
    #[plexus_macros::hub_method(
        description = "Save research data (search suggestions, all found tracks, Claude output) for a playlist",
        params(
            name = "Playlist name this research is associated with",
            data = "Research data as JSON (searches, found tracks, curation output, etc.)"
        )
    )]
    pub async fn research_save(
        &self,
        name: String,
        data: Value,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            match hub.write_research(&name, &data) {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "research_save".into(),
                    message: format!("saved research for '{name}'"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Get research data for a playlist
    #[plexus_macros::hub_method(
        description = "Retrieve saved research data for a playlist",
        params(name = "Playlist name")
    )]
    pub async fn research_get(
        &self,
        name: String,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let hub = self.clone();
        stream! {
            match hub.load_research(&name) {
                Ok(data) => yield MonoEvent::PlayerAck {
                    action: "research_get".into(),
                    message: serde_json::to_string(&data).unwrap_or_default(),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }
}

#[async_trait]
impl ChildRouter for PlaylistHub {
    fn router_namespace(&self) -> &str {
        "playlist"
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
