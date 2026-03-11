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
use crate::types::MonoEvent;

/// Stateful playback engine activation with queue and playlist management.
#[derive(Clone)]
pub struct PlayerHub {
    player: Arc<Player>,
    playlist: PlaylistHub,
}

impl PlayerHub {
    /// Create a new PlayerHub from a shared MonoClient.
    pub async fn new(client: Arc<MonoClient>) -> Self {
        let player = Player::new(client.clone()).await;
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
    version = "0.2.0",
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
            };
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
                };
            }
        }
    }
}
