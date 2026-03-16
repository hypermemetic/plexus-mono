//! PlayerHub — Plexus RPC activation for stateful playback
//!
//! Owns the audio playback engine, queue, and playlist child router.
//! Requires speakers. Registered as a hub activation under `monochrome`.

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use std::sync::Arc;

use plexus_core::plexus::schema::ChildSummary;
use plexus_core::plexus::{ChildRouter, PlexusError, PlexusStream};
use plexus_core::Activation;

use crate::client::MonoClient;
use crate::player::Player;
use crate::playlist::PlaylistHub;
use crate::storage::MonoStorage;
use crate::types::MonoEvent;

/// Stateful playback engine activation with queue and playlist management.
#[derive(Clone)]
pub struct PlayerHub {
    player: Arc<Player>,
    playlist: PlaylistHub,
}

impl PlayerHub {
    /// Create a new PlayerHub from a shared MonoClient.
    pub async fn new(client: Arc<MonoClient>, storage: Arc<MonoStorage>) -> Self {
        let player = Player::new(client.clone(), storage).await;
        let playlist = PlaylistHub::new(player.clone(), client);
        Self { player, playlist }
    }

    /// Return child activation summaries for schema discovery
    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        vec![ChildSummary {
            namespace: "playlist".into(),
            description: "Persistent playlist management".into(),
            hash: String::new(),
        }]
    }
}

#[async_trait]
impl ChildRouter for PlayerHub {
    fn router_namespace(&self) -> &str {
        "player"
    }

    async fn router_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<PlexusStream, PlexusError> {
        self.call(method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        match name {
            "playlist" => Some(Box::new(self.playlist.clone())),
            _ => None,
        }
    }
}

#[plexus_macros::hub_methods(
    namespace = "player",
    version = "0.3.0",
    hub,
    description = "Playback engine — play, queue, and control audio with playlist management",
    crate_path = "plexus_core"
)]
impl PlayerHub {
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

    /// Set pre-amp gain level
    #[plexus_macros::hub_method(
        description = "Set pre-amp gain. Values above 1.0 boost the signal (max 4.0). Effective volume = preamp × volume.",
        params(level = "Gain level from 0.0 (silent) to 4.0 (4× boost)")
    )]
    pub async fn preamp(
        &self,
        level: f32,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            player.set_preamp(level).await;
            yield MonoEvent::PlayerAck {
                action: "preamp".to_string(),
                message: format!("preamp set to {:.1}×", level.clamp(0.0, 4.0)),
            };
        }
    }

    /// Add an entire album to the playback queue
    #[plexus_macros::hub_method(
        streaming,
        description = "Add all tracks from an album to the queue. Auto-starts if nothing is playing.",
        params(
            id = "Tidal album ID",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW"
        )
    )]
    pub async fn queue_album(
        &self,
        id: u64,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            match player.queue_album(id, &quality).await {
                Ok(tracks) => {
                    let count = tracks.len();
                    yield MonoEvent::PlayerAck {
                        action: "queue_album".to_string(),
                        message: format!("{count} tracks queued"),
                    };
                    let current_index = Some(0usize);
                    yield MonoEvent::Queue {
                        tracks,
                        current_index,
                    };
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Add a track to the playback queue
    #[plexus_macros::hub_method(
        description = "Add a track to the end of the playback queue. Auto-starts if nothing is playing.",
        params(
            id = "Tidal track ID",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW",
            source = "Where this track was queued from (playlist name, album, etc.)"
        )
    )]
    pub async fn queue_add(
        &self,
        id: u64,
        quality: Option<String>,
        source: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            match player.queue_add_with_source(id, &quality, source).await {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "queue_add".to_string(),
                    message: format!("track {id} added to queue"),
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Add multiple tracks to the queue at once
    #[plexus_macros::hub_method(
        streaming,
        description = "Add multiple tracks by ID in one call. Resolves metadata in parallel. Auto-starts if nothing is playing.",
        params(
            ids = "List of Tidal track IDs",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW"
        )
    )]
    pub async fn queue_batch(
        &self,
        ids: Vec<u64>,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            match player.queue_batch(&ids, &quality).await {
                Ok(tracks) => {
                    let count = tracks.len();
                    yield MonoEvent::PlayerAck {
                        action: "queue_batch".to_string(),
                        message: format!("{count} tracks queued"),
                    };
                    yield MonoEvent::Queue {
                        tracks,
                        current_index: Some(0),
                    };
                }
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

    /// Get current playback status (single snapshot, returns immediately)
    #[plexus_macros::hub_method(
        description = "Get current playback status — track, position, volume, queue length"
    )]
    pub async fn status(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let rx = self.player.subscribe_now_playing();
        let np = rx.borrow().clone();
        stream! {
            yield MonoEvent::NowPlaying {
                track_id: np.track_id,
                title: np.title,
                artist: np.artist,
                album: np.album,
                status: np.status,
                position_secs: np.position_secs,
                duration_secs: np.duration_secs,
                volume: np.volume,
                preamp: np.preamp,
                queue_length: np.queue_length,
                url: np.url,
                is_liked: np.is_liked,
                is_downloaded: np.is_downloaded,
                audio_peak: Some(np.audio_peak),
            };
        }
    }

    /// Seek to a position in the current track
    #[plexus_macros::hub_method(
        description = "Seek to a position in the current track",
        params(position_secs = "Position in seconds to seek to")
    )]
    pub async fn seek(
        &self,
        position_secs: f32,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            match player.seek(position_secs).await {
                Ok(()) => yield MonoEvent::PlayerAck {
                    action: "seek".to_string(),
                    message: format!("seeked to {position_secs:.1}s"),
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
                    preamp: np.preamp,
                    queue_length: np.queue_length,
                    url: np.url,
                    is_liked: np.is_liked,
                    is_downloaded: np.is_downloaded,
                    audio_peak: Some(np.audio_peak),
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
                    preamp: np.preamp,
                    queue_length: np.queue_length,
                    url: np.url,
                    is_liked: np.is_liked,
                    is_downloaded: np.is_downloaded,
                    audio_peak: Some(np.audio_peak),
                };
            }
        }
    }

    /// Get buffered waveform peak history for instant rendering on connect
    #[plexus_macros::hub_method(
        description = "Get buffered peak history (~2048 samples at 30fps ≈ 68s) for instant waveform rendering"
    )]
    pub async fn waveform(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let (track_id, peaks) = self.player.peak_history().await;
        stream! {
            if let Some(track_id) = track_id {
                yield MonoEvent::Waveform { track_id, peaks };
            }
        }
    }

    /// Stream live audio peak levels at ~30fps for waveform visualization
    #[plexus_macros::hub_method(
        streaming,
        description = "Stream real-time audio peak levels (~30fps) for waveform visualization"
    )]
    pub async fn audio_peaks(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let peak_handle = self.player.audio_peak_handle();
        stream! {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(33)).await;
                let bits = peak_handle.load(std::sync::atomic::Ordering::Relaxed);
                let peak = f32::from_bits(bits);
                yield MonoEvent::AudioPeak { peak };
            }
        }
    }

    /// Get listening stats for a specific track
    #[plexus_macros::hub_method(
        description = "Get per-track listening statistics (play count, skip count, total listen time)",
        params(id = "Tidal track ID")
    )]
    pub async fn stats(
        &self,
        id: u64,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            match player.get_track_stats(id).await {
                Some(s) => yield MonoEvent::TrackStats {
                    id: s.id,
                    title: s.title,
                    artist: s.artist,
                    album: s.album,
                    play_count: s.play_count,
                    complete_count: s.complete_count,
                    skip_count: s.skip_count,
                    total_listen_secs: s.total_listen_secs,
                    first_played: s.first_played,
                    last_played: s.last_played,
                },
                None => yield MonoEvent::Error {
                    message: format!("no stats for track {id}"),
                },
            }
        }
    }

    /// Get top most-played tracks
    #[plexus_macros::hub_method(
        streaming,
        description = "Get top N most-played tracks sorted by play count",
        params(limit = "Number of tracks to return (default 10)")
    )]
    pub async fn stats_top(
        &self,
        limit: Option<u32>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let limit = limit.unwrap_or(10) as usize;
        stream! {
            let tracks = player.get_top_tracks(limit).await;
            for s in tracks {
                yield MonoEvent::TrackStats {
                    id: s.id,
                    title: s.title,
                    artist: s.artist,
                    album: s.album,
                    play_count: s.play_count,
                    complete_count: s.complete_count,
                    skip_count: s.skip_count,
                    total_listen_secs: s.total_listen_secs,
                    first_played: s.first_played,
                    last_played: s.last_played,
                };
            }
        }
    }

    /// Get most recently played tracks
    #[plexus_macros::hub_method(
        streaming,
        description = "Get the most recent listen events (newest first)",
        params(limit = "Number of events to return (default 10)")
    )]
    pub async fn stats_recent(
        &self,
        limit: Option<u32>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let limit = limit.unwrap_or(10) as usize;
        stream! {
            let events = player.get_recent_listens(limit).await;
            for e in events {
                yield MonoEvent::ListenEvent {
                    track_id: e.track_id,
                    started_at: e.started_at,
                    duration_listened: e.duration_listened,
                    outcome: e.outcome,
                };
            }
        }
    }

    /// Stream full listen log
    #[plexus_macros::hub_method(
        streaming,
        description = "Stream the full listen history log"
    )]
    pub async fn history(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            let events = player.get_listen_log().await;
            for e in events {
                yield MonoEvent::ListenEvent {
                    track_id: e.track_id,
                    started_at: e.started_at,
                    duration_listened: e.duration_listened,
                    outcome: e.outcome,
                };
            }
        }
    }

    /// Clear listen history log
    #[plexus_macros::hub_method(
        description = "Clear the listen history log (aggregate stats are preserved)"
    )]
    pub async fn history_clear(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            player.clear_listen_log().await;
            yield MonoEvent::PlayerAck {
                action: "history_clear".to_string(),
                message: "listen history cleared".to_string(),
            };
        }
    }

    /// Toggle like on a track
    #[plexus_macros::hub_method(
        description = "Toggle like/heart on a track. Returns the new liked state.",
        params(
            id = "Tidal track ID",
            source = "Where the like was triggered from (e.g. now-playing, playlist:name)"
        )
    )]
    pub async fn like(
        &self,
        id: u64,
        source: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            match player.toggle_like(id, source).await {
                Ok(liked) => yield MonoEvent::PlayerAck {
                    action: "like".to_string(),
                    message: if liked { format!("liked track {id}") } else { format!("unliked track {id}") },
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Get all liked track IDs
    #[plexus_macros::hub_method(
        description = "Get all liked track IDs (most recently liked first)"
    )]
    pub async fn liked_tracks(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            match player.liked_ids().await {
                Ok(ids) => {
                    let tracks: Vec<crate::types::QueuedTrack> = ids.iter().map(|&id| {
                        crate::types::QueuedTrack {
                            id,
                            title: String::new(),
                            artist: String::new(),
                            album: String::new(),
                            duration_secs: 0,
                            quality: String::new(),
                            cover_id: None,
                            source: Some("liked".to_string()),
                        }
                    }).collect();
                    yield MonoEvent::Queue {
                        tracks,
                        current_index: None,
                    };
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Download a track to local storage
    #[plexus_macros::hub_method(
        streaming,
        description = "Download a track to ~/Music/mono-tray/{artist}/{album}/ and register for offline playback",
        params(
            id = "Tidal track ID (if omitted, downloads the current track)",
            quality = "Quality: LOSSLESS (default), HI_RES_LOSSLESS, HIGH, LOW"
        )
    )]
    pub async fn download_track(
        &self,
        id: Option<u64>,
        quality: Option<String>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        let quality = quality.unwrap_or_else(|| "LOSSLESS".to_string());
        stream! {
            // Resolve track ID — use current track if not specified
            let track_id = match id {
                Some(id) => id,
                None => {
                    let (current, _) = player.queue_get().await;
                    match current {
                        Some(t) => t.id,
                        None => {
                            yield MonoEvent::Error { message: "no track playing and no id specified".to_string() };
                            return;
                        }
                    }
                }
            };
            match player.download_track(track_id, &quality).await {
                Ok(mut rx) => {
                    while let Some(event) = rx.recv().await {
                        yield event;
                    }
                }
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Delete a downloaded track from local storage
    #[plexus_macros::hub_method(
        description = "Delete a downloaded track from local storage and remove the file",
        params(id = "Tidal track ID (if omitted, uses current track)")
    )]
    pub async fn delete_download(
        &self,
        id: Option<u64>,
    ) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            let track_id = match id {
                Some(id) => id,
                None => {
                    let (current, _) = player.queue_get().await;
                    match current {
                        Some(t) => t.id,
                        None => {
                            yield MonoEvent::Error { message: "no track playing and no id specified".to_string() };
                            return;
                        }
                    }
                }
            };
            match player.delete_download(track_id).await {
                Ok(path) => yield MonoEvent::PlayerAck {
                    action: "delete_download".to_string(),
                    message: match path {
                        Some(p) => format!("deleted download: {p}"),
                        None => format!("track {track_id} was not downloaded"),
                    },
                },
                Err(e) => yield MonoEvent::Error { message: e },
            }
        }
    }

    /// Get playback history (previously played tracks)
    #[plexus_macros::hub_method(
        description = "Get the list of previously played tracks (most recent last, frontend reverses)"
    )]
    pub async fn history_list(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let player = self.player.clone();
        stream! {
            let tracks = player.get_history().await;
            yield MonoEvent::Queue {
                tracks,
                current_index: None,
            };
        }
    }
}
