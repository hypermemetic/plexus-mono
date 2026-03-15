//! Playback engine — dedicated audio thread with queue management
//!
//! All rodio interaction is isolated to a single OS thread (OutputStream is !Send).
//! The Sink is Send+Sync and shared via Arc for control from async code.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};

use stream_download::source::SourceStream;

use crate::client::MonoClient;
use crate::storage::MonoStorage;
use crate::types::{ListenEvent, ListenOutcome, MonoEvent, PlayStatus, QueuedTrack, TrackStats};

/// Persisted player state — saved to disk so playback can resume across restarts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub current_track: Option<QueuedTrack>,
    pub position_secs: f32,
    pub queue: Vec<QueuedTrack>,
    pub history: Vec<QueuedTrack>,
    pub volume: f32,
    #[serde(default = "default_preamp")]
    pub preamp: f32,
}

fn default_preamp() -> f32 {
    1.0
}

impl PlayerState {
    fn state_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".plexus/monochrome/player/state.json")
    }

    pub fn load() -> Option<Self> {
        let path = Self::state_path();
        let data = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self) {
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

/// Persistent store for per-track stats and listen log
pub struct StatsStore {
    stats: HashMap<u64, TrackStats>,
    listen_log: Vec<ListenEvent>,
}

impl StatsStore {
    fn stats_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".plexus/monochrome/player/stats.json")
    }

    fn log_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".plexus/monochrome/player/listen_log.json")
    }

    pub fn load() -> Self {
        let stats: HashMap<u64, TrackStats> = Self::stats_path()
            .pipe(|p| std::fs::read_to_string(p).ok())
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_default();

        let listen_log: Vec<ListenEvent> = Self::log_path()
            .pipe(|p| std::fs::read_to_string(p).ok())
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_default();

        Self { stats, listen_log }
    }

    fn save(&self) {
        fn save_json(path: PathBuf, json: &str) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, json);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.stats) {
            save_json(Self::stats_path(), &json);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.listen_log) {
            save_json(Self::log_path(), &json);
        }
    }

    /// Record that a track started playing. Returns the ISO 8601 timestamp.
    pub fn record_start(&mut self, track: &QueuedTrack) -> String {
        let now = Utc::now().to_rfc3339();
        let entry = self.stats.entry(track.id).or_insert_with(|| TrackStats {
            id: track.id,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            play_count: 0,
            complete_count: 0,
            skip_count: 0,
            total_listen_secs: 0.0,
            first_played: now.clone(),
            last_played: now.clone(),
        });
        entry.play_count += 1;
        entry.last_played = now.clone();
        // Update metadata in case it changed
        entry.title = track.title.clone();
        entry.artist = track.artist.clone();
        entry.album = track.album.clone();
        self.save();
        now
    }

    /// Record that a track ended (complete, skip, or stop)
    pub fn record_end(
        &mut self,
        track_id: u64,
        started_at: &str,
        duration_listened: f32,
        outcome: ListenOutcome,
    ) {
        // Update aggregate stats
        if let Some(entry) = self.stats.get_mut(&track_id) {
            entry.total_listen_secs += duration_listened;
            match &outcome {
                ListenOutcome::Complete => entry.complete_count += 1,
                ListenOutcome::Skip => entry.skip_count += 1,
                ListenOutcome::Stop => {} // stop doesn't increment skip or complete
            }
        }

        // Append to listen log
        self.listen_log.push(ListenEvent {
            track_id,
            started_at: started_at.to_string(),
            duration_listened,
            outcome,
        });

        self.save();
    }
}

/// Pipe trait for ergonomic chaining
trait Pipe: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}
impl<T> Pipe for T {}

/// Helper trait to erase the concrete StreamDownload type behind Box.
/// Rust doesn't allow `dyn Read + Seek + Send` (multiple non-auto traits),
/// so we combine them into one trait and blanket-implement it.
trait ReadSeekSend: std::io::Read + std::io::Seek + Send + Sync {}
impl<T: std::io::Read + std::io::Seek + Send + Sync> ReadSeekSend for T {}

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
    pub preamp: f32,
    pub queue_length: usize,
    pub url: Option<String>,
    pub is_liked: Option<bool>,
    pub is_downloaded: Option<bool>,
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
            preamp: 1.0,
            queue_length: 0,
            url: None,
            is_liked: None,
            is_downloaded: None,
        }
    }
}

struct PlayerInner {
    queue: VecDeque<QueuedTrack>,
    current_track: Option<QueuedTrack>,
    status: PlayStatus,
    volume: f32,
    preamp: f32,
    history: Vec<QueuedTrack>,
    /// Pre-buffered audio readers keyed by track ID.
    /// Each entry is a StreamDownload that's already connected and downloading.
    /// Dropped automatically when removed (temp file cleaned up via RAII).
    prefetched: HashMap<u64, (Box<dyn ReadSeekSend>, Option<u64>)>,
    /// Timestamp when current track started playing (ISO 8601)
    listen_started_at: Option<String>,
}

/// Audio playback engine with queue and controls
pub struct Player {
    sink: Arc<rodio::Player>,
    inner: Mutex<PlayerInner>,
    stats: Mutex<StatsStore>,
    storage: Arc<MonoStorage>,
    now_playing_tx: watch::Sender<NowPlaying>,
    now_playing_rx: watch::Receiver<NowPlaying>,
    client: Arc<MonoClient>,
    // Dropping this signals the audio thread to exit
    _shutdown_tx: std::sync::mpsc::Sender<()>,
}

impl Player {
    /// Create a new Player. Spawns a dedicated audio thread and background watchers.
    pub async fn new(client: Arc<MonoClient>, storage: Arc<MonoStorage>) -> Arc<Self> {
        let (sink_tx, sink_rx) = std::sync::mpsc::channel();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();

        std::thread::spawn(move || {
            let mut stream = rodio::DeviceSinkBuilder::open_default_sink()
                .expect("failed to open default audio output device");
            stream.log_on_drop(false);
            let sink = rodio::Player::connect_new(stream.mixer());
            let _ = sink_tx.send(sink);
            // Keep stream alive until Player is dropped
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
                preamp: 1.0,
                history: Vec::new(),
                prefetched: HashMap::new(),
                listen_started_at: None,
            }),
            stats: Mutex::new(StatsStore::load()),
            storage,
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
                    // Track ended naturally — record as complete
                    let listen_info = inner.current_track.as_ref().map(|t| (t.id, t.duration_secs as f32));
                    let started_at = inner.listen_started_at.take();
                    if let (Some((track_id, duration)), Some(started_at)) = (listen_info, started_at) {
                        drop(inner);
                        let mut stats = this.stats.lock().await;
                        stats.record_end(track_id, &started_at, duration, ListenOutcome::Complete);
                        drop(stats);
                        inner = this.inner.lock().await;
                    }
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
                        this.save_state().await;
                    } else {
                        inner.status = PlayStatus::Idle;
                        inner.current_track = None;
                        drop(inner);
                        this.broadcast_now_playing().await;
                        this.save_state().await;
                    }
                }
            }
        });

        // Prefetch watcher — pre-buffers queued tracks when playing
        let weak = Arc::downgrade(&player);
        tokio::spawn(async move {
            let mut last_track_id: Option<u64> = None;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(this) = weak.upgrade() else { break };
                let current_id = {
                    let inner = this.inner.lock().await;
                    if !matches!(inner.status, PlayStatus::Playing) {
                        continue;
                    }
                    inner.current_track.as_ref().map(|t| t.id)
                };
                // Prefetch when track changes or on first play
                if current_id != last_track_id {
                    last_track_id = current_id;
                    this.prefetch_queue().await;
                }
            }
        });

        // OS media controls (play/pause keys, Now Playing widget)
        player.setup_media_controls();

        // Restore persisted state (queue, history, volume) from previous session
        player.restore_state().await;

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

                // Set initial state immediately to claim media keys from macOS
                let _ = controls.set_metadata(MediaMetadata {
                    title: Some("Plexus Mono"),
                    artist: None,
                    album: None,
                    duration: None,
                    cover_url: None,
                });
                let _ = controls.set_playback(MediaPlayback::Paused { progress: None });

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
        let track_id = inner.current_track.as_ref().map(|t| t.id);
        let (is_liked, is_downloaded) = if let Some(id) = track_id {
            let liked = self.storage.is_liked(id).await.unwrap_or(false);
            let downloaded = self.storage.is_downloaded(id).await.unwrap_or(false);
            (Some(liked), Some(downloaded))
        } else {
            (None, None)
        };
        let np = NowPlaying {
            track_id,
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
            preamp: inner.preamp,
            queue_length: inner.queue.len(),
            url: inner.current_track.as_ref().map(|t| format!("https://monochrome.tf/track/t/{}", t.id)),
            is_liked,
            is_downloaded,
        };
        let _ = self.now_playing_tx.send(np);
    }

    /// End the current listen session if one is active.
    /// Computes duration from listen_started_at to now, records stats.
    async fn end_current_listen(&self, outcome: ListenOutcome) {
        let (track_id, started_at, position) = {
            let mut inner = self.inner.lock().await;
            let started = inner.listen_started_at.take();
            match (inner.current_track.as_ref(), started) {
                (Some(track), Some(started_at)) => {
                    (track.id, started_at, self.sink.get_pos().as_secs_f32())
                }
                _ => return,
            }
        };
        let mut stats = self.stats.lock().await;
        stats.record_end(track_id, &started_at, position, outcome);
    }

    /// Resolve stream URL, create decoder, and start playback on the sink.
    async fn start_playback(&self, track: QueuedTrack) -> Result<(), String> {
        // Record start in stats
        {
            let mut stats = self.stats.lock().await;
            let ts = stats.record_start(&track);
            let mut inner = self.inner.lock().await;
            inner.listen_started_at = Some(ts);
        }

        {
            let mut inner = self.inner.lock().await;
            inner.current_track = Some(track.clone());
            inner.status = PlayStatus::Buffering;
        }
        self.broadcast_now_playing().await;

        // Priority: prefetch cache → local download → HTTP stream
        let (reader, content_length): (Box<dyn ReadSeekSend>, Option<u64>) = {
            let mut inner = self.inner.lock().await;
            if let Some((r, cl)) = inner.prefetched.remove(&track.id) {
                tracing::debug!("using prefetched audio for track {}", track.id);
                (r, cl)
            } else {
                drop(inner);
                self.resolve_audio_source(&track).await?
            }
        };

        // Decode on blocking thread — tell symphonia the stream is seekable
        let source = tokio::task::spawn_blocking(move || {
            let mut builder = rodio::Decoder::builder()
                .with_data(reader)
                .with_seekable(true);
            if let Some(len) = content_length {
                builder = builder.with_byte_len(len);
            }
            builder.build()
        })
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

    /// Try local download first, fall back to HTTP stream
    async fn resolve_audio_source(
        &self,
        track: &QueuedTrack,
    ) -> Result<(Box<dyn ReadSeekSend>, Option<u64>), String> {
        // Check download registry for offline playback
        if let Ok(Some(path)) = self.storage.get_download_path(track.id).await {
            let p = std::path::Path::new(&path);
            if p.exists() {
                if let Ok(file) = std::fs::File::open(p) {
                    let len = file.metadata().map(|m| m.len()).ok();
                    tracing::debug!("offline playback for track {}: {}", track.id, path);
                    return Ok((Box::new(file), len));
                }
            }
        }

        // Resolve CDN URL
        let manifest = self.client.stream_manifest(track.id, &track.quality).await?;
        let url = match &manifest {
            MonoEvent::StreamManifest { url, .. } => url.clone(),
            _ => return Err("unexpected manifest type".to_string()),
        };

        // Create streaming reader
        let http_stream = stream_download::http::HttpStream::<
            stream_download::http::reqwest::Client,
        >::create(
            url.parse::<reqwest::Url>().map_err(|e| format!("bad stream url: {e}"))?
        )
        .await
        .map_err(|e| format!("http stream error: {e}"))?;
        let cl = http_stream.content_length();
        let r = stream_download::StreamDownload::from_stream(
            http_stream,
            stream_download::storage::temp::TempStorageProvider::new(),
            stream_download::Settings::default(),
        )
        .await
        .map_err(|e| format!("stream download error: {e}"))?;
        Ok((Box::new(r), cl))
    }

    /// Play a track immediately, stopping whatever is currently playing.
    pub async fn play_track(&self, id: u64, quality: &str) -> Result<(), String> {
        let track_info = self.client.track_info(id).await.ok();
        let queued = make_queued_track(id, quality, track_info);

        // End current listen as skip (interrupting for a different track)
        self.end_current_listen(ListenOutcome::Skip).await;

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
        self.end_current_listen(ListenOutcome::Stop).await;
        self.sink.stop();
        let mut inner = self.inner.lock().await;
        if let Some(current) = inner.current_track.take() {
            inner.history.push(current);
        }
        inner.status = PlayStatus::Stopped;
        inner.prefetched.clear(); // Drop all pre-buffered temp files
        drop(inner);
        self.broadcast_now_playing().await;
        self.save_state().await;
    }

    /// Seek to a position in the current track (in seconds)
    pub async fn seek(&self, position_secs: f32) -> Result<(), String> {
        let has_track = {
            let inner = self.inner.lock().await;
            inner.current_track.is_some()
        };
        if !has_track {
            return Err("no track playing".to_string());
        }
        self.sink
            .try_seek(Duration::from_secs_f32(position_secs))
            .map_err(|e| format!("seek failed: {e}"))?;
        self.broadcast_now_playing().await;
        Ok(())
    }

    /// Skip to next track in queue
    pub async fn next(&self) -> Result<(), String> {
        self.end_current_listen(ListenOutcome::Skip).await;
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

    /// Go to previous track (from history), or restart current if >5s in
    pub async fn previous(&self) -> Result<(), String> {
        // If we're more than 5 seconds into the current track, restart it
        if self.sink.get_pos().as_secs_f32() > 5.0 {
            // End current listen as skip (restarting counts as a new play)
            self.end_current_listen(ListenOutcome::Skip).await;
            let track = {
                let inner = self.inner.lock().await;
                inner.current_track.clone()
            };
            if let Some(track) = track {
                return self.start_playback(track).await;
            }
        }

        self.end_current_listen(ListenOutcome::Skip).await;
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

    /// Apply combined volume (preamp × volume) to the sink
    fn apply_volume(&self, inner: &PlayerInner) {
        self.sink.set_volume(inner.preamp * inner.volume);
    }

    /// Set volume (0.0–1.0)
    pub async fn set_volume(&self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        let mut inner = self.inner.lock().await;
        inner.volume = level;
        self.apply_volume(&inner);
        drop(inner);
        self.broadcast_now_playing().await;
        self.save_state().await;
    }

    /// Set pre-amp gain (0.0–4.0, where >1.0 boosts)
    pub async fn set_preamp(&self, level: f32) {
        let level = level.clamp(0.0, 4.0);
        let mut inner = self.inner.lock().await;
        inner.preamp = level;
        self.apply_volume(&inner);
        drop(inner);
        self.broadcast_now_playing().await;
        self.save_state().await;
    }

    /// Add a track to the end of the queue. Auto-starts if idle.
    pub async fn queue_add(&self, id: u64, quality: &str) -> Result<(), String> {
        self.queue_add_with_source(id, quality, None).await
    }

    pub async fn queue_add_with_source(&self, id: u64, quality: &str, source: Option<String>) -> Result<(), String> {
        let track_info = self.client.track_info(id).await.ok();
        let mut queued = make_queued_track(id, quality, track_info);
        queued.source = source;

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

        let result = if should_start {
            self.start_playback(queued).await
        } else {
            self.broadcast_now_playing().await;
            Ok(())
        };
        self.save_state().await;
        result
    }

    /// Add all tracks from an album to the queue. Auto-starts if idle.
    pub async fn queue_album(&self, album_id: u64, quality: &str) -> Result<Vec<QueuedTrack>, String> {
        let (_album_event, track_events) = self.client.album(album_id).await?;

        let mut queued_tracks = Vec::new();
        for event in &track_events {
            if let MonoEvent::AlbumTrack { id, title, artist, duration_secs, .. } = event {
                queued_tracks.push(QueuedTrack {
                    id: *id,
                    title: title.clone(),
                    artist: artist.clone(),
                    album: String::new(), // filled below
                    duration_secs: *duration_secs,
                    quality: quality.to_string(),
                    cover_id: None,
                    source: None,
                });
            }
        }

        // Get album name from the album event
        let album_name = if let MonoEvent::Album { title, cover_id, .. } = &_album_event {
            for t in &mut queued_tracks {
                t.album = title.clone();
                t.cover_id = cover_id.clone();
            }
            title.clone()
        } else {
            format!("Album {album_id}")
        };

        if queued_tracks.is_empty() {
            return Err(format!("no tracks found in album {album_name}"));
        }

        let should_start = {
            let mut inner = self.inner.lock().await;
            let idle = matches!(inner.status, PlayStatus::Idle | PlayStatus::Stopped);
            if idle {
                // Queue all but the first; we'll start the first directly
                for t in queued_tracks.iter().skip(1) {
                    inner.queue.push_back(t.clone());
                }
                true
            } else {
                for t in &queued_tracks {
                    inner.queue.push_back(t.clone());
                }
                false
            }
        };

        if should_start {
            self.start_playback(queued_tracks[0].clone()).await?;
        } else {
            self.broadcast_now_playing().await;
        }

        Ok(queued_tracks)
    }

    /// Add multiple tracks to the queue at once. Auto-starts if idle.
    pub async fn queue_batch(&self, ids: &[u64], quality: &str) -> Result<Vec<QueuedTrack>, String> {
        if ids.is_empty() {
            return Err("no track IDs provided".into());
        }

        // Resolve all track metadata in parallel
        let futs: Vec<_> = ids.iter().map(|&id| {
            let client = self.client.clone();
            let q = quality.to_string();
            async move {
                let info = client.track_info(id).await.ok();
                make_queued_track(id, &q, info)
            }
        }).collect();
        let tracks: Vec<QueuedTrack> = futures::future::join_all(futs).await;

        let should_start = {
            let mut inner = self.inner.lock().await;
            let idle = matches!(inner.status, PlayStatus::Idle | PlayStatus::Stopped);
            if idle {
                // Queue all but the first; we'll start the first directly
                for t in tracks.iter().skip(1) {
                    inner.queue.push_back(t.clone());
                }
                true
            } else {
                for t in &tracks {
                    inner.queue.push_back(t.clone());
                }
                false
            }
        };

        if should_start {
            self.start_playback(tracks[0].clone()).await?;
        } else {
            self.broadcast_now_playing().await;
        }

        self.save_state().await;
        Ok(tracks)
    }

    /// Clear the queue (does not stop current track)
    pub async fn queue_clear(&self) {
        let mut inner = self.inner.lock().await;
        inner.queue.clear();
        inner.prefetched.clear(); // Drop all pre-buffered temp files
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

    /// Pre-buffer queued tracks by resolving their stream URLs and starting downloads.
    /// Each StreamDownload writes to a temp file (cleaned up on drop via RAII).
    async fn prefetch_queue(&self) {
        let tracks: Vec<QueuedTrack> = {
            let inner = self.inner.lock().await;
            inner
                .queue
                .iter()
                .filter(|t| !inner.prefetched.contains_key(&t.id))
                .take(10)
                .cloned()
                .collect()
        };

        for track in tracks {
            // Resolve manifest
            let manifest = match self.client.stream_manifest(track.id, &track.quality).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!("prefetch manifest failed for {}: {e}", track.id);
                    continue;
                }
            };
            let url = match &manifest {
                MonoEvent::StreamManifest { url, .. } => url.clone(),
                _ => continue,
            };
            let parsed = match url.parse::<reqwest::Url>() {
                Ok(u) => u,
                Err(_) => continue,
            };

            // Start download with content_length extraction
            let http_stream = match stream_download::http::HttpStream::<
                stream_download::http::reqwest::Client,
            >::create(parsed)
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("prefetch http stream failed for {}: {e}", track.id);
                    continue;
                }
            };
            let content_length = http_stream.content_length();
            let reader = match stream_download::StreamDownload::from_stream(
                http_stream,
                stream_download::storage::temp::TempStorageProvider::new(),
                stream_download::Settings::default(),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("prefetch download failed for {}: {e}", track.id);
                    continue;
                }
            };

            tracing::debug!("prefetched track {} ({})", track.id, track.title);
            let mut inner = self.inner.lock().await;
            inner.prefetched.insert(track.id, (Box::new(reader), content_length));
        }
    }

    /// Subscribe to now-playing updates
    pub fn subscribe_now_playing(&self) -> watch::Receiver<NowPlaying> {
        self.now_playing_rx.clone()
    }

    /// Snapshot current state for persistence
    pub async fn get_state(&self) -> PlayerState {
        let inner = self.inner.lock().await;
        PlayerState {
            current_track: inner.current_track.clone(),
            position_secs: self.sink.get_pos().as_secs_f32(),
            queue: inner.queue.iter().cloned().collect(),
            history: inner.history.clone(),
            volume: inner.volume,
            preamp: inner.preamp,
        }
    }

    /// Save current state to disk
    pub async fn save_state(&self) {
        let state = self.get_state().await;
        state.save();
    }

    /// Restore state from disk — resumes playback at the saved position
    pub async fn restore_state(&self) {
        if let Some(state) = PlayerState::load() {
            let resume_track = state.current_track.clone();
            let resume_pos = state.position_secs;

            {
                let mut inner = self.inner.lock().await;
                inner.queue = state.queue.into_iter().collect();
                inner.history = state.history;
                inner.volume = state.volume;
                inner.preamp = state.preamp;
                self.apply_volume(&inner);
            }

            // Resume the track that was playing, seeking to saved position
            if let Some(track) = resume_track {
                tracing::info!(
                    "resuming '{}' at {:.0}s",
                    track.title,
                    resume_pos
                );
                match self.start_playback(track).await {
                    Ok(()) => {
                        // Start paused so it doesn't blast on startup
                        self.sink.pause();
                        let mut inner = self.inner.lock().await;
                        inner.status = PlayStatus::Paused;
                        drop(inner);
                        self.broadcast_now_playing().await;
                    }
                    Err(e) => {
                        tracing::error!("failed to resume track: {e}");
                    }
                }
            } else {
                self.broadcast_now_playing().await;
            }

            tracing::info!("restored player state from disk");
        }
    }

    // ── Stats & Listen History queries ────────────────────────────────

    /// Get stats for a specific track by ID
    pub async fn get_track_stats(&self, id: u64) -> Option<TrackStats> {
        let stats = self.stats.lock().await;
        stats.stats.get(&id).cloned()
    }

    /// Get top N most-played tracks, sorted by play_count descending
    pub async fn get_top_tracks(&self, limit: usize) -> Vec<TrackStats> {
        let stats = self.stats.lock().await;
        let mut sorted: Vec<TrackStats> = stats.stats.values().cloned().collect();
        sorted.sort_by(|a, b| b.play_count.cmp(&a.play_count));
        sorted.truncate(limit);
        sorted
    }

    /// Get most recent listen events
    pub async fn get_recent_listens(&self, limit: usize) -> Vec<ListenEvent> {
        let stats = self.stats.lock().await;
        let len = stats.listen_log.len();
        let start = len.saturating_sub(limit);
        stats.listen_log[start..].iter().rev().cloned().collect()
    }

    /// Get full listen log
    pub async fn get_listen_log(&self) -> Vec<ListenEvent> {
        let stats = self.stats.lock().await;
        stats.listen_log.clone()
    }

    /// Clear the listen log (keeps aggregate stats)
    pub async fn clear_listen_log(&self) {
        let mut stats = self.stats.lock().await;
        stats.listen_log.clear();
        stats.save();
    }

    // ── Likes & Downloads ────────────────────────────────────────────────

    /// Get a reference to the storage layer
    pub fn storage(&self) -> &MonoStorage {
        &self.storage
    }

    /// Toggle like on a track. Returns new liked state.
    pub async fn toggle_like(&self, track_id: u64) -> Result<bool, String> {
        let result = self.storage.toggle_like(track_id).await?;
        self.broadcast_now_playing().await;
        Ok(result)
    }

    /// Get all liked track IDs
    pub async fn liked_ids(&self) -> Result<Vec<u64>, String> {
        self.storage.liked_ids().await
    }

    /// Download a track, register in storage, stream progress events via channel
    pub async fn download_track(
        self: &Arc<Self>,
        id: u64,
        quality: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<MonoEvent>, String> {
        // Get track info for organized path
        let track_info = self.client.track_info(id).await.ok();
        let (title, artist, album) = match &track_info {
            Some(MonoEvent::Track { title, artist, album, .. }) => {
                (title.clone(), artist.clone(), album.clone())
            }
            _ => (format!("Track {id}"), String::from("Unknown"), String::from("Unknown")),
        };

        // Get stream manifest for extension
        let manifest = self.client.stream_manifest(id, quality).await?;
        let ext = match &manifest {
            MonoEvent::StreamManifest { extension, .. } => extension.clone(),
            _ => "flac".to_string(),
        };

        // Build path: ~/Music/mono-tray/{artist}/{album}/{title}.{ext}
        let base = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music/mono-tray");
        let sanitize = |s: &str| s.replace('/', "_").replace('\\', "_").replace(':', "_");
        let dir = base.join(sanitize(&artist)).join(sanitize(&album));
        std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir failed: {e}"))?;
        let path = dir.join(format!("{}.{}", sanitize(&title), ext));
        let path_str = path.to_string_lossy().to_string();

        // Start download — returns a channel of progress events
        let mut inner_rx = self.client.download(id, quality, &path_str).await?;

        // Wrap in a new channel so we can register + broadcast after completion
        let (tx, rx) = tokio::sync::mpsc::channel::<MonoEvent>(16);
        let player = self.clone();
        let title_c = title.clone();
        let artist_c = artist.clone();
        let album_c = album.clone();
        let quality_c = quality.to_string();

        tokio::spawn(async move {
            while let Some(event) = inner_rx.recv().await {
                let is_complete = matches!(&event, MonoEvent::DownloadComplete { .. });
                if tx.send(event).await.is_err() {
                    return;
                }
                if is_complete {
                    let _ = player.storage
                        .register_download(
                            id,
                            &path_str,
                            Some(&title_c),
                            Some(&artist_c),
                            Some(&album_c),
                            Some(&quality_c),
                        )
                        .await;
                    player.broadcast_now_playing().await;
                    return;
                }
            }
        });

        Ok(rx)
    }

    /// Delete a downloaded track from local storage
    pub async fn delete_download(&self, track_id: u64) -> Result<Option<String>, String> {
        let path = self.storage.delete_download(track_id).await?;
        self.broadcast_now_playing().await;
        Ok(path)
    }

    /// Get playback history (queue of previously played tracks)
    pub async fn get_history(&self) -> Vec<QueuedTrack> {
        let inner = self.inner.lock().await;
        inner.history.clone()
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
            source: None,
        },
        _ => QueuedTrack {
            id,
            title: format!("Track {id}"),
            artist: String::new(),
            album: String::new(),
            duration_secs: 0,
            quality: quality.to_string(),
            cover_id: None,
            source: None,
        },
    }
}
