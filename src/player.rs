//! Playback engine — dedicated audio thread with queue management
//!
//! All rodio interaction is isolated to a single OS thread (OutputStream is !Send).
//! The Sink is Send+Sync and shared via Arc for control from async code.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex};

use crate::client::MonoClient;
use crate::types::{MonoEvent, PlayStatus, QueuedTrack};

/// Snapshot of current playback state, broadcast via watch channel
#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub track_id: Option<u64>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub status: PlayStatus,
    pub position_secs: f32,
    pub duration_secs: f32,
    pub volume: f32,
    pub queue_length: usize,
}

impl Default for NowPlaying {
    fn default() -> Self {
        Self {
            track_id: None,
            title: None,
            artist: None,
            album: None,
            status: PlayStatus::Idle,
            position_secs: 0.0,
            duration_secs: 0.0,
            volume: 1.0,
            queue_length: 0,
        }
    }
}

struct PlayerInner {
    queue: VecDeque<QueuedTrack>,
    current_track: Option<QueuedTrack>,
    status: PlayStatus,
    volume: f32,
    history: Vec<QueuedTrack>,
}

/// Audio playback engine with queue and controls
pub struct Player {
    sink: Arc<rodio::Sink>,
    inner: Mutex<PlayerInner>,
    now_playing_tx: watch::Sender<NowPlaying>,
    now_playing_rx: watch::Receiver<NowPlaying>,
    client: Arc<MonoClient>,
    // Dropping this signals the audio thread to exit
    _shutdown_tx: std::sync::mpsc::Sender<()>,
}

impl Player {
    /// Create a new Player. Spawns a dedicated audio thread and background watchers.
    pub async fn new(client: Arc<MonoClient>) -> Arc<Self> {
        let (sink_tx, sink_rx) = std::sync::mpsc::channel();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();

        std::thread::spawn(move || {
            let (_stream, handle) = rodio::OutputStream::try_default()
                .expect("failed to open default audio output device");
            let sink = rodio::Sink::try_new(&handle)
                .expect("failed to create audio sink");
            let _ = sink_tx.send(sink);
            // Keep _stream alive until Player is dropped
            let _ = shutdown_rx.recv();
        });

        let sink = Arc::new(sink_rx.recv().expect("audio thread failed to initialize"));
        sink.pause(); // Start idle

        let (now_playing_tx, now_playing_rx) = watch::channel(NowPlaying::default());

        let player = Arc::new(Self {
            sink,
            inner: Mutex::new(PlayerInner {
                queue: VecDeque::new(),
                current_track: None,
                status: PlayStatus::Idle,
                volume: 1.0,
                history: Vec::new(),
            }),
            now_playing_tx,
            now_playing_rx,
            client,
            _shutdown_tx: shutdown_tx,
        });

        // Position reporter (~1s updates while playing)
        let weak = Arc::downgrade(&player);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(this) = weak.upgrade() else { break };
                let is_playing = {
                    let inner = this.inner.lock().await;
                    matches!(inner.status, PlayStatus::Playing)
                };
                if is_playing {
                    this.broadcast_now_playing().await;
                }
            }
        });

        // Track watcher — auto-advance when current track ends
        let weak = Arc::downgrade(&player);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let Some(this) = weak.upgrade() else { break };
                if !this.sink.empty() {
                    continue;
                }
                let mut inner = this.inner.lock().await;
                if matches!(inner.status, PlayStatus::Playing) {
                    // Track ended naturally
                    if let Some(current) = inner.current_track.take() {
                        inner.history.push(current);
                    }
                    if let Some(next) = inner.queue.pop_front() {
                        inner.status = PlayStatus::Buffering;
                        drop(inner);
                        if let Err(e) = this.start_playback(next).await {
                            tracing::error!("auto-advance failed: {e}");
                            let mut inner = this.inner.lock().await;
                            inner.status = PlayStatus::Idle;
                            inner.current_track = None;
                            drop(inner);
                            this.broadcast_now_playing().await;
                        }
                    } else {
                        inner.status = PlayStatus::Idle;
                        inner.current_track = None;
                        drop(inner);
                        this.broadcast_now_playing().await;
                    }
                }
            }
        });

        // OS media controls (play/pause keys, Now Playing widget)
        player.setup_media_controls();

        player
    }

    /// Wire up macOS Now Playing / media key integration via souvlaki.
    /// Spawns a dedicated thread that owns MediaControls and polls for
    /// metadata updates from the watch channel.
    fn setup_media_controls(self: &Arc<Self>) {
        use souvlaki::{
            MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
            PlatformConfig,
        };

        let tokio_handle = tokio::runtime::Handle::current();
        let weak = Arc::downgrade(self);
        let mut np_rx = self.subscribe_now_playing();

        std::thread::Builder::new()
            .name("media-controls".into())
            .spawn(move || {
                let config = PlatformConfig {
                    dbus_name: "plexus_mono",
                    display_name: "Plexus Mono",
                    hwnd: None,
                };
                let mut controls = match MediaControls::new(config) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("media controls unavailable: {e:?}");
                        return;
                    }
                };

                // Event handler — dispatches media key presses to player via tokio
                let weak2 = weak.clone();
                let handle = tokio_handle.clone();
                if let Err(e) = controls.attach(move |event: MediaControlEvent| {
                    let Some(player) = weak2.upgrade() else {
                        return;
                    };
                    let player = player.clone();
                    handle.spawn(async move {
                        match event {
                            MediaControlEvent::Play => player.resume().await,
                            MediaControlEvent::Pause => player.pause().await,
                            MediaControlEvent::Toggle => {
                                let is_playing = {
                                    let inner = player.inner.lock().await;
                                    matches!(inner.status, PlayStatus::Playing)
                                };
                                if is_playing {
                                    player.pause().await;
                                } else {
                                    player.resume().await;
                                }
                            }
                            MediaControlEvent::Next => {
                                let _ = player.next().await;
                            }
                            MediaControlEvent::Previous => {
                                let _ = player.previous().await;
                            }
                            _ => {}
                        }
                    });
                }) {
                    tracing::warn!("failed to attach media controls: {e:?}");
                    return;
                }

                tracing::info!("media controls active (Now Playing + media keys)");

                // Poll watch channel and update OS metadata
                loop {
                    std::thread::sleep(Duration::from_millis(500));

                    if !np_rx.has_changed().unwrap_or(false) {
                        // Also check if player is dropped
                        if weak.upgrade().is_none() {
                            break;
                        }
                        continue;
                    }

                    let np = np_rx.borrow_and_update().clone();

                    // Build cover art URL from Tidal cover UUID
                    let cover_url = np.title.as_ref().and_then(|_| {
                        // We don't have cover_id in NowPlaying, so skip for now
                        None::<String>
                    });

                    let _ = controls.set_metadata(MediaMetadata {
                        title: np.title.as_deref(),
                        artist: np.artist.as_deref(),
                        album: np.album.as_deref(),
                        duration: if np.duration_secs > 0.0 {
                            Some(Duration::from_secs_f32(np.duration_secs))
                        } else {
                            None
                        },
                        cover_url: cover_url.as_deref(),
                    });

                    let playback = match np.status {
                        PlayStatus::Playing => MediaPlayback::Playing {
                            progress: Some(MediaPosition(Duration::from_secs_f32(
                                np.position_secs,
                            ))),
                        },
                        PlayStatus::Paused => MediaPlayback::Paused {
                            progress: Some(MediaPosition(Duration::from_secs_f32(
                                np.position_secs,
                            ))),
                        },
                        _ => MediaPlayback::Stopped,
                    };
                    let _ = controls.set_playback(playback);
                }
            })
            .expect("failed to spawn media-controls thread");
    }

    /// Broadcast current state through the watch channel
    async fn broadcast_now_playing(&self) {
        let inner = self.inner.lock().await;
        let np = NowPlaying {
            track_id: inner.current_track.as_ref().map(|t| t.id),
            title: inner.current_track.as_ref().map(|t| t.title.clone()),
            artist: inner.current_track.as_ref().map(|t| t.artist.clone()),
            album: inner.current_track.as_ref().map(|t| t.album.clone()),
            status: inner.status.clone(),
            position_secs: self.sink.get_pos().as_secs_f32(),
            duration_secs: inner
                .current_track
                .as_ref()
                .map(|t| t.duration_secs as f32)
                .unwrap_or(0.0),
            volume: inner.volume,
            queue_length: inner.queue.len(),
        };
        let _ = self.now_playing_tx.send(np);
    }

    /// Resolve stream URL, create decoder, and start playback on the sink.
    /// Assumes status is already set to Buffering and current_track is set.
    async fn start_playback(&self, track: QueuedTrack) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().await;
            inner.current_track = Some(track.clone());
            inner.status = PlayStatus::Buffering;
        }
        self.broadcast_now_playing().await;

        // Resolve CDN URL
        let manifest = self.client.stream_manifest(track.id, &track.quality).await?;
        let url = match &manifest {
            MonoEvent::StreamManifest { url, .. } => url.clone(),
            _ => return Err("unexpected manifest type".to_string()),
        };

        // Create streaming reader (async HTTP → Read+Seek buffer)
        let reader = stream_download::StreamDownload::new_http(
            url.parse::<reqwest::Url>()
                .map_err(|e| format!("bad stream url: {e}"))?,
            stream_download::storage::temp::TempStorageProvider::new(),
            stream_download::Settings::default(),
        )
        .await
        .map_err(|e| format!("stream download error: {e}"))?;

        // Decode on blocking thread (reads file headers from network buffer)
        let source = tokio::task::spawn_blocking(move || rodio::Decoder::new(reader))
            .await
            .map_err(|e| format!("decoder task panicked: {e}"))?
            .map_err(|e| format!("audio decode error: {e}"))?;

        // Stop previous audio, append new source, play
        self.sink.stop();
        self.sink.append(source);
        self.sink.play();

        {
            let mut inner = self.inner.lock().await;
            inner.status = PlayStatus::Playing;
        }
        self.broadcast_now_playing().await;

        Ok(())
    }

    /// Play a track immediately, stopping whatever is currently playing.
    pub async fn play_track(&self, id: u64, quality: &str) -> Result<(), String> {
        let track_info = self.client.track_info(id).await.ok();
        let queued = make_queued_track(id, quality, track_info);

        // Move current to history
        {
            let mut inner = self.inner.lock().await;
            if let Some(current) = inner.current_track.take() {
                inner.history.push(current);
            }
        }

        self.start_playback(queued).await
    }

    /// Pause playback
    pub async fn pause(&self) {
        self.sink.pause();
        let mut inner = self.inner.lock().await;
        if matches!(inner.status, PlayStatus::Playing | PlayStatus::Buffering) {
            inner.status = PlayStatus::Paused;
        }
        drop(inner);
        self.broadcast_now_playing().await;
    }

    /// Resume playback
    pub async fn resume(&self) {
        self.sink.play();
        let mut inner = self.inner.lock().await;
        if matches!(inner.status, PlayStatus::Paused) {
            inner.status = PlayStatus::Playing;
        }
        drop(inner);
        self.broadcast_now_playing().await;
    }

    /// Stop playback and clear current track
    pub async fn stop(&self) {
        self.sink.stop();
        let mut inner = self.inner.lock().await;
        if let Some(current) = inner.current_track.take() {
            inner.history.push(current);
        }
        inner.status = PlayStatus::Stopped;
        drop(inner);
        self.broadcast_now_playing().await;
    }

    /// Skip to next track in queue
    pub async fn next(&self) -> Result<(), String> {
        self.sink.stop();
        let next = {
            let mut inner = self.inner.lock().await;
            if let Some(current) = inner.current_track.take() {
                inner.history.push(current);
            }
            inner.queue.pop_front()
        };

        if let Some(track) = next {
            self.start_playback(track).await
        } else {
            let mut inner = self.inner.lock().await;
            inner.status = PlayStatus::Idle;
            drop(inner);
            self.broadcast_now_playing().await;
            Err("queue is empty".to_string())
        }
    }

    /// Go to previous track (from history)
    pub async fn previous(&self) -> Result<(), String> {
        self.sink.stop();
        let prev = {
            let mut inner = self.inner.lock().await;
            // Push current back to front of queue
            if let Some(current) = inner.current_track.take() {
                inner.queue.push_front(current);
            }
            inner.history.pop()
        };

        if let Some(track) = prev {
            self.start_playback(track).await
        } else {
            Err("no previous track".to_string())
        }
    }

    /// Set volume (0.0–1.0)
    pub async fn set_volume(&self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        self.sink.set_volume(level);
        let mut inner = self.inner.lock().await;
        inner.volume = level;
        drop(inner);
        self.broadcast_now_playing().await;
    }

    /// Add a track to the end of the queue. Auto-starts if idle.
    pub async fn queue_add(&self, id: u64, quality: &str) -> Result<(), String> {
        let track_info = self.client.track_info(id).await.ok();
        let queued = make_queued_track(id, quality, track_info);

        let should_start = {
            let mut inner = self.inner.lock().await;
            let idle = matches!(inner.status, PlayStatus::Idle | PlayStatus::Stopped);
            if idle {
                // Will start this track directly
                true
            } else {
                inner.queue.push_back(queued.clone());
                false
            }
        };

        if should_start {
            self.start_playback(queued).await
        } else {
            self.broadcast_now_playing().await;
            Ok(())
        }
    }

    /// Clear the queue (does not stop current track)
    pub async fn queue_clear(&self) {
        let mut inner = self.inner.lock().await;
        inner.queue.clear();
        drop(inner);
        self.broadcast_now_playing().await;
    }

    /// Get current track and queue contents
    pub async fn queue_get(&self) -> (Option<QueuedTrack>, Vec<QueuedTrack>) {
        let inner = self.inner.lock().await;
        (
            inner.current_track.clone(),
            inner.queue.iter().cloned().collect(),
        )
    }

    /// Reorder a track in the queue
    pub async fn queue_reorder(&self, from: usize, to: usize) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        if from >= inner.queue.len() || to >= inner.queue.len() {
            return Err(format!(
                "index out of bounds (queue has {} tracks)",
                inner.queue.len()
            ));
        }
        let track = inner.queue.remove(from).unwrap();
        inner.queue.insert(to, track);
        Ok(())
    }

    /// Subscribe to now-playing updates
    pub fn subscribe_now_playing(&self) -> watch::Receiver<NowPlaying> {
        self.now_playing_rx.clone()
    }
}

/// Build a QueuedTrack from track info (or fallback to minimal metadata)
fn make_queued_track(id: u64, quality: &str, info: Option<MonoEvent>) -> QueuedTrack {
    match info {
        Some(MonoEvent::Track {
            title,
            artist,
            album,
            duration_secs,
            cover_id,
            ..
        }) => QueuedTrack {
            id,
            title,
            artist,
            album,
            duration_secs,
            quality: quality.to_string(),
            cover_id,
        },
        _ => QueuedTrack {
            id,
            title: format!("Track {id}"),
            artist: String::new(),
            album: String::new(),
            duration_secs: 0,
            quality: quality.to_string(),
            cover_id: None,
        },
    }
}
