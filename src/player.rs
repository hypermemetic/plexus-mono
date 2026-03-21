//! Playback engine — dedicated audio thread with queue management
//!
//! All rodio interaction is isolated to a single OS thread (`OutputStream` is !Send).
//! The Sink is `Send+Sync` and shared via Arc for control from async code.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};

use stream_download::source::SourceStream;

use crate::playlist::PlaylistData;
use crate::provider::MusicProvider;
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
    /// Set by `graceful_shutdown()` — signals `restore_state()` to auto-resume playback
    #[serde(default)]
    pub was_playing: bool,
}

fn default_preamp() -> f32 {
    1.0
}

impl PlayerState {
    fn state_path(data_dir: &std::path::Path) -> PathBuf {
        data_dir.join("player/state.json")
    }

    pub fn load(data_dir: &std::path::Path) -> Option<Self> {
        let path = Self::state_path(data_dir);
        let data = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self, data_dir: &std::path::Path) {
        let path = Self::state_path(data_dir);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("failed to create directory {}: {e}", parent.display());
            }
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if let Err(e) = std::fs::write(&path, &json) {
                tracing::warn!("failed to write state to {}: {e}", path.display());
            }
        }
    }
}

/// Persistent store for per-track stats and listen log
pub struct StatsStore {
    stats: HashMap<String, TrackStats>,
    listen_log: Vec<ListenEvent>,
    data_dir: PathBuf,
}

impl StatsStore {
    fn stats_path(data_dir: &std::path::Path) -> PathBuf {
        data_dir.join("player/stats.json")
    }

    fn log_path(data_dir: &std::path::Path) -> PathBuf {
        data_dir.join("player/listen_log.json")
    }

    pub fn load(data_dir: PathBuf) -> Self {
        let stats: HashMap<String, TrackStats> = Self::stats_path(&data_dir)
            .pipe(|p| std::fs::read_to_string(p).ok())
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_default();

        let listen_log: Vec<ListenEvent> = Self::log_path(&data_dir)
            .pipe(|p| std::fs::read_to_string(p).ok())
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_default();

        Self { stats, listen_log, data_dir }
    }

    fn save(&self) {
        fn save_json(path: &std::path::Path, json: &str) {
            if let Some(parent) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!("failed to create directory {}: {e}", parent.display());
                }
            }
            if let Err(e) = std::fs::write(path, json) {
                tracing::warn!("failed to write state to {}: {e}", path.display());
            }
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.stats) {
            save_json(&Self::stats_path(&self.data_dir), &json);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.listen_log) {
            save_json(&Self::log_path(&self.data_dir), &json);
        }
    }

    /// Record that a track started playing. Returns the ISO 8601 timestamp.
    pub fn record_start(&mut self, track: &QueuedTrack) -> String {
        let now = Utc::now().to_rfc3339();
        let entry = self.stats.entry(track.id.clone()).or_insert_with(|| TrackStats {
            id: track.id.clone(),
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
        entry.last_played.clone_from(&now);
        // Update metadata in case it changed
        entry.title.clone_from(&track.title);
        entry.artist.clone_from(&track.artist);
        entry.album.clone_from(&track.album);
        self.save();
        now
    }

    /// Record that a track ended (complete, skip, or stop)
    pub fn record_end(
        &mut self,
        track_id: &str,
        started_at: &str,
        duration_listened: f32,
        outcome: ListenOutcome,
    ) {
        // Update aggregate stats
        if let Some(entry) = self.stats.get_mut(track_id) {
            entry.total_listen_secs += duration_listened;
            match &outcome {
                ListenOutcome::Complete => entry.complete_count += 1,
                ListenOutcome::Skip => entry.skip_count += 1,
                ListenOutcome::Stop => {} // stop doesn't increment skip or complete
            }
        }

        // Append to listen log
        self.listen_log.push(ListenEvent {
            track_id: track_id.to_string(),
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

/// Transparent audio source wrapper that computes peak levels in ~33ms windows.
/// Passes samples through unchanged; stores peak in an AtomicU32 for lock-free reads.
struct LevelMonitor<S> {
    inner: S,
    peak_atom: Arc<AtomicU32>,
    window_size: usize,
    window_pos: usize,
    window_peak: f32,
}

impl<S: rodio::Source<Item = f32>> LevelMonitor<S> {
    fn new(source: S, peak_atom: Arc<AtomicU32>) -> Self {
        // ~33ms window: sample_rate * channels / 30
        let window_size =
            (source.sample_rate().get() as usize * source.channels().get() as usize) / 30;
        Self {
            inner: source,
            peak_atom,
            window_size: window_size.max(1),
            window_pos: 0,
            window_peak: 0.0,
        }
    }
}

impl<S: rodio::Source<Item = f32>> Iterator for LevelMonitor<S> {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        let abs = sample.abs();
        if abs > self.window_peak {
            self.window_peak = abs;
        }
        self.window_pos += 1;
        if self.window_pos >= self.window_size {
            self.peak_atom
                .store(self.window_peak.to_bits(), Ordering::Relaxed);
            self.window_pos = 0;
            self.window_peak = 0.0;
        }
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: rodio::Source<Item = f32>> rodio::Source for LevelMonitor<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> std::num::NonZeroU16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> std::num::NonZeroU32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.window_pos = 0;
        self.window_peak = 0.0;
        self.inner.try_seek(pos)
    }
}

/// Snapshot of current playback state, broadcast via watch channel
#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub track_id: Option<String>,
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
    pub audio_peak: f32,
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
            audio_peak: 0.0,
        }
    }
}

/// Shuffle radio mode — randomly picks tracks from a source pool
struct ShufflePool {
    track_pool: Vec<String>,
    played: HashSet<String>,
    active: bool,
}

impl ShufflePool {
    fn new() -> Self {
        Self {
            track_pool: Vec::new(),
            played: HashSet::new(),
            active: false,
        }
    }

    fn pick_next(&mut self) -> Option<String> {
        let available: Vec<String> = self
            .track_pool
            .iter()
            .filter(|id| !self.played.contains(*id))
            .cloned()
            .collect();
        if available.is_empty() {
            // All played — reset and re-pick
            self.played.clear();
            if self.track_pool.is_empty() {
                return None;
            }
            let idx = rand::random_range(0..self.track_pool.len());
            let id = self.track_pool[idx].clone();
            self.played.insert(id.clone());
            Some(id)
        } else {
            let idx = rand::random_range(0..available.len());
            let id = available[idx].clone();
            self.played.insert(id.clone());
            Some(id)
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
    prefetched: HashMap<String, (Box<dyn ReadSeekSend>, Option<u64>)>,
    /// Timestamp when current track started playing (ISO 8601)
    listen_started_at: Option<String>,
    /// Shuffle radio mode state
    shuffle: ShufflePool,
}

/// Circular buffer of recent audio peak values for waveform history.
struct PeakHistory {
    buffer: Vec<f32>,
    capacity: usize,
    track_id: Option<String>,
}

impl PeakHistory {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            capacity,
            track_id: None,
        }
    }

    fn push(&mut self, peak: f32, track_id: Option<String>) {
        if track_id != self.track_id {
            self.buffer.clear();
            self.track_id = track_id;
        }
        if self.buffer.len() >= self.capacity {
            self.buffer.remove(0);
        }
        self.buffer.push(peak);
    }

    fn snapshot(&self) -> (Option<String>, Vec<f32>) {
        (self.track_id.clone(), self.buffer.clone())
    }
}

/// Audio playback engine with queue and controls
pub struct Player {
    sink: Arc<rodio::Player>,
    inner: Mutex<PlayerInner>,
    stats: Mutex<StatsStore>,
    storage: Arc<MonoStorage>,
    now_playing_tx: watch::Sender<NowPlaying>,
    now_playing_rx: watch::Receiver<NowPlaying>,
    client: Arc<dyn MusicProvider>,
    audio_peak: Arc<AtomicU32>,
    peak_history: Arc<Mutex<PeakHistory>>,
    /// Base directory for persistent data (state, stats, playlists)
    data_dir: PathBuf,
    /// Optional URL template for track links (e.g. "https://example.com/track/t/{}")
    track_url_template: Option<String>,
    // Dropping this signals the audio thread to exit
    _shutdown_tx: std::sync::mpsc::Sender<()>,
}

impl Player {
    /// Create a new Player. Spawns a dedicated audio thread and background watchers.
    #[allow(clippy::too_many_lines)]
    pub async fn new(
        client: Arc<dyn MusicProvider>,
        storage: Arc<MonoStorage>,
        data_dir: PathBuf,
        track_url_template: Option<String>,
    ) -> Arc<Self> {
        let (sink_tx, sink_rx) = std::sync::mpsc::channel();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();

        std::thread::spawn(move || {
            let mut stream = match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("failed to open default audio output device: {e}");
                    return;
                }
            };
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
                shuffle: ShufflePool::new(),
            }),
            stats: Mutex::new(StatsStore::load(data_dir.clone())),
            storage,
            now_playing_tx,
            now_playing_rx,
            client,
            audio_peak: Arc::new(AtomicU32::new(0)),
            peak_history: Arc::new(Mutex::new(PeakHistory::new(2048))),
            data_dir,
            track_url_template,
            _shutdown_tx: shutdown_tx,
        });

        // Peak history collector (~30fps, mirrors LevelMonitor output into buffer)
        let weak = Arc::downgrade(&player);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(33)).await;
                let Some(this) = weak.upgrade() else { break };
                let bits = this.audio_peak.load(Ordering::Relaxed);
                let peak = f32::from_bits(bits);
                let track_id = {
                    let inner = this.inner.lock().await;
                    inner.current_track.as_ref().map(|t| t.id.clone())
                };
                let mut hist = this.peak_history.lock().await;
                hist.push(peak, track_id);
            }
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
                let should_advance =
                    matches!(inner.status, PlayStatus::Playing | PlayStatus::Failed);
                if should_advance {
                    // Track ended naturally or failed — record completion if it was playing
                    if matches!(inner.status, PlayStatus::Playing) {
                        let listen_info = inner
                            .current_track
                            .as_ref()
                            .map(|t| (t.id.clone(), t.duration_secs as f32));
                        let started_at = inner.listen_started_at.take();
                        if let (Some((track_id, duration)), Some(started_at)) =
                            (listen_info, started_at)
                        {
                            drop(inner);
                            let mut stats = this.stats.lock().await;
                            stats.record_end(
                                &track_id,
                                &started_at,
                                duration,
                                ListenOutcome::Complete,
                            );
                            drop(stats);
                            inner = this.inner.lock().await;
                        }
                    }
                    if let Some(current) = inner.current_track.take() {
                        inner.history.push(current);
                    }
                    if let Some(next) = inner.queue.pop_front() {
                        inner.status = PlayStatus::Buffering;
                        drop(inner);
                        if let Err(e) = this.start_playback(next).await {
                            tracing::error!("auto-advance failed: {e}");
                        }
                        // Refill shuffle queue after advancing
                        this.shuffle_refill().await;
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
            let mut last_track_id: Option<String> = None;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(this) = weak.upgrade() else { break };
                let current_id = {
                    let inner = this.inner.lock().await;
                    if !matches!(inner.status, PlayStatus::Playing) {
                        continue;
                    }
                    inner.current_track.as_ref().map(|t| t.id.clone())
                };
                // Prefetch when track changes or on first play
                if current_id != last_track_id {
                    last_track_id = current_id;
                    this.prefetch_queue().await;
                }
            }
        });

        // Lock watchdog — detects when PlayerInner mutex is held too long
        let weak = Arc::downgrade(&player);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let Some(this) = weak.upgrade() else { break };
                let acquire_start = tokio::time::Instant::now();
                let result = tokio::time::timeout(Duration::from_secs(3), this.inner.lock()).await;
                match result {
                    Ok(guard) => {
                        let elapsed = acquire_start.elapsed();
                        drop(guard);
                        if elapsed > Duration::from_secs(1) {
                            tracing::warn!(
                                "lock watchdog: lock acquired after {:.1}s (slow)",
                                elapsed.as_secs_f32()
                            );
                        }
                    }
                    Err(_) => {
                        tracing::error!(
                            "LOCK WATCHDOG: PlayerInner mutex blocked for >3s — possible deadlock/stall"
                        );
                    }
                }
            }
        });

        // OS media controls (play/pause keys, Now Playing widget)
        player.setup_media_controls();

        // Restore persisted state (queue, history, volume) from previous session
        player.restore_state().await;

        // Sync liked playlist on startup
        {
            let p = player.clone();
            tokio::spawn(async move { p.sync_liked_playlist().await });
        }

        player
    }

    /// Wire up macOS Now Playing / media key integration via souvlaki.
    /// Spawns a dedicated thread that owns MediaControls and polls for
    /// metadata updates from the watch channel.
    #[allow(clippy::too_many_lines)]
    fn setup_media_controls(self: &Arc<Self>) {
        use souvlaki::{
            MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
            PlatformConfig,
        };

        let tokio_handle = tokio::runtime::Handle::current();
        let weak = Arc::downgrade(self);
        let mut np_rx = self.subscribe_now_playing();

        if let Err(e) = std::thread::Builder::new()
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

                    // We don't have cover_id in NowPlaying, so skip for now
                    let cover_url: Option<String> = None;

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
        {
            tracing::warn!("media controls unavailable: {e}");
        }
    }

    /// Broadcast current state through the watch channel.
    /// Lock is held only briefly to snapshot state — storage I/O happens outside.
    async fn broadcast_now_playing(&self) {
        // Snapshot everything we need under the lock, then drop it
        let (
            track_id,
            title,
            artist,
            album,
            status,
            duration_secs,
            volume,
            preamp,
            queue_length,
            url,
        ) = {
            let inner = self.inner.lock().await;
            (
                inner.current_track.as_ref().map(|t| t.id.clone()),
                inner.current_track.as_ref().map(|t| t.title.clone()),
                inner.current_track.as_ref().map(|t| t.artist.clone()),
                inner.current_track.as_ref().map(|t| t.album.clone()),
                inner.status.clone(),
                inner
                    .current_track
                    .as_ref()
                    .map_or(0.0, |t| t.duration_secs as f32),
                inner.volume,
                inner.preamp,
                inner.queue.len(),
                {
                    let track_id = inner.current_track.as_ref().map(|t| t.id.clone());
                    match (&self.track_url_template, track_id) {
                        (Some(tpl), Some(id)) => Some(tpl.replace("{}", &id)),
                        _ => None,
                    }
                },
            )
        };
        // Storage I/O outside the lock — no mutex contention
        let (is_liked, is_downloaded) = if let Some(ref id) = track_id {
            let liked = self.storage.is_liked(id).await.unwrap_or(false);
            let downloaded = self.storage.is_downloaded(id).await.unwrap_or(false);
            (Some(liked), Some(downloaded))
        } else {
            (None, None)
        };
        let np = NowPlaying {
            track_id,
            title,
            artist,
            album,
            status,
            position_secs: self.sink.get_pos().as_secs_f32(),
            duration_secs,
            volume,
            preamp,
            queue_length,
            url,
            is_liked,
            is_downloaded,
            audio_peak: f32::from_bits(self.audio_peak.load(Ordering::Relaxed)),
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
                    (track.id.clone(), started_at, self.sink.get_pos().as_secs_f32())
                }
                _ => return,
            }
        };
        let mut stats = self.stats.lock().await;
        stats.record_end(&track_id, &started_at, position, outcome);
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
        let audio_result: Result<(Box<dyn ReadSeekSend>, Option<u64>), String> = {
            let mut inner = self.inner.lock().await;
            if let Some((r, cl)) = inner.prefetched.remove(&track.id as &str) {
                tracing::debug!("using prefetched audio for track {}", track.id);
                Ok((r, cl))
            } else {
                drop(inner);
                self.resolve_audio_source(&track).await
            }
        };
        let (reader, content_length) = match audio_result {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to resolve audio for track {}: {e}", track.id);
                let mut inner = self.inner.lock().await;
                inner.status = PlayStatus::Failed;
                drop(inner);
                self.broadcast_now_playing().await;
                return Err(e);
            }
        };

        // Decode on blocking thread — tell symphonia the stream is seekable
        let source = match tokio::task::spawn_blocking(move || {
            let mut builder = rodio::Decoder::builder()
                .with_data(reader)
                .with_seekable(true);
            if let Some(len) = content_length {
                builder = builder.with_byte_len(len);
            }
            builder.build()
        })
        .await
        {
            Ok(Ok(source)) => source,
            Ok(Err(e)) => {
                let msg = format!("audio decode error: {e}");
                tracing::error!("{msg}");
                let mut inner = self.inner.lock().await;
                inner.status = PlayStatus::Failed;
                drop(inner);
                self.broadcast_now_playing().await;
                return Err(msg);
            }
            Err(e) => {
                let msg = format!("decoder task panicked: {e}");
                tracing::error!("{msg}");
                let mut inner = self.inner.lock().await;
                inner.status = PlayStatus::Failed;
                drop(inner);
                self.broadcast_now_playing().await;
                return Err(msg);
            }
        };

        // Stop previous audio, wrap with level monitor, append and play
        self.sink.stop();
        self.audio_peak.store(0, Ordering::Relaxed);
        let monitored = LevelMonitor::new(source, self.audio_peak.clone());
        self.sink.append(monitored);
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
        if let Ok(Some(path)) = self.storage.get_download_path(&track.id).await {
            let p = std::path::Path::new(&path);
            if p.exists() {
                if let Ok(file) = std::fs::File::open(p) {
                    let len = file.metadata().map(|m| m.len()).ok();
                    tracing::debug!("offline playback for track {}: {}", track.id, path);
                    return Ok((Box::new(file), len));
                }
            }
        }

        // Resolve CDN URL (with timeout to avoid hanging on expired sessions)
        let manifest = tokio::time::timeout(
            Duration::from_secs(30),
            self.client.stream_manifest(&track.id, &track.quality),
        )
        .await
        .map_err(|_| {
            "stream manifest timed out (30s) — provider session may have expired".to_string()
        })?
        .map_err(|e| format!("stream manifest error: {e}"))?;
        let url = match &manifest {
            MonoEvent::StreamManifest { url, .. } => url.clone(),
            _ => return Err("unexpected manifest type".to_string()),
        };

        // Create streaming reader (with timeout)
        let http_stream = tokio::time::timeout(
            Duration::from_secs(15),
            stream_download::http::HttpStream::<stream_download::http::reqwest::Client>::create(
                url.parse::<reqwest::Url>()
                    .map_err(|e| format!("bad stream url: {e}"))?,
            ),
        )
        .await
        .map_err(|_| "CDN stream connection timed out (15s)".to_string())?
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
    pub async fn play_track(&self, id: &str, quality: &str) -> Result<(), String> {
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
    pub async fn queue_add(&self, id: &str, quality: &str) -> Result<(), String> {
        self.queue_add_with_source(id, quality, None).await
    }

    pub async fn queue_add_with_source(
        &self,
        id: &str,
        quality: &str,
        source: Option<String>,
    ) -> Result<(), String> {
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

    /// Add a track to the front of the queue (play next). Auto-starts if idle.
    pub async fn queue_add_next(
        &self,
        id: &str,
        quality: &str,
        source: Option<String>,
    ) -> Result<(), String> {
        let track_info = self.client.track_info(id).await.ok();
        let mut queued = make_queued_track(id, quality, track_info);
        queued.source = source;

        let should_start = {
            let mut inner = self.inner.lock().await;
            let idle = matches!(inner.status, PlayStatus::Idle | PlayStatus::Stopped);
            if idle {
                true
            } else {
                inner.queue.push_front(queued.clone());
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
    pub async fn queue_album(
        &self,
        album_id: &str,
        quality: &str,
    ) -> Result<Vec<QueuedTrack>, String> {
        let (album_event, track_events) = self.client.album(album_id).await?;

        let mut queued_tracks = Vec::new();
        for event in &track_events {
            if let MonoEvent::AlbumTrack {
                id,
                title,
                artist,
                duration_secs,
                ..
            } = event
            {
                queued_tracks.push(QueuedTrack {
                    id: id.clone(),
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
        let album_name = if let MonoEvent::Album {
            title, cover_id, ..
        } = &album_event
        {
            for t in &mut queued_tracks {
                t.album.clone_from(title);
                t.cover_id.clone_from(cover_id);
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
    pub async fn queue_batch(
        &self,
        ids: &[String],
        quality: &str,
    ) -> Result<Vec<QueuedTrack>, String> {
        if ids.is_empty() {
            return Err("no track IDs provided".into());
        }

        // Resolve all track metadata in parallel
        let futs: Vec<_> = ids
            .iter()
            .map(|id| {
                let client = self.client.clone();
                let q = quality.to_string();
                let id = id.clone();
                async move {
                    let info = client.track_info(&id).await.ok();
                    make_queued_track(&id, &q, info)
                }
            })
            .collect();
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
        let track = inner
            .queue
            .remove(from)
            .ok_or_else(|| "queue index out of bounds".to_string())?;
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
                .filter(|t| !inner.prefetched.contains_key(&t.id as &str))
                .take(10)
                .cloned()
                .collect()
        };

        for track in tracks {
            // Each prefetch gets a 30s budget — prevents stalled CDN connections from
            // blocking the prefetch watcher indefinitely.
            let result = tokio::time::timeout(Duration::from_secs(30), async {
                // Resolve manifest
                let manifest = match self.client.stream_manifest(&track.id, &track.quality).await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!("prefetch manifest failed for {}: {e}", track.id);
                        return;
                    }
                };
                let url = match &manifest {
                    MonoEvent::StreamManifest { url, .. } => url.clone(),
                    _ => return,
                };
                let Ok(parsed) = url.parse::<reqwest::Url>() else {
                    return;
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
                        return;
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
                        return;
                    }
                };

                tracing::debug!("prefetched track {} ({})", track.id, track.title);
                let mut inner = self.inner.lock().await;
                inner
                    .prefetched
                    .insert(track.id.clone(), (Box::new(reader), content_length));
            })
            .await;

            if result.is_err() {
                tracing::warn!(
                    "prefetch timed out for track {} ({})",
                    track.id,
                    track.title
                );
            }
        }
    }

    /// Subscribe to now-playing updates
    /// Get a handle to the live audio peak atomic (updated at ~30fps by LevelMonitor)
    pub fn audio_peak_handle(&self) -> Arc<AtomicU32> {
        self.audio_peak.clone()
    }

    /// Get a snapshot of the buffered peak history for waveform seeding.
    pub async fn peak_history(&self) -> (Option<String>, Vec<f32>) {
        self.peak_history.lock().await.snapshot()
    }

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
            was_playing: false,
        }
    }

    /// Graceful shutdown — finalize stats, persist was_playing flag, save state.
    /// Called from SIGTERM handler before process exit.
    pub async fn graceful_shutdown(&self) {
        let was_playing = {
            let inner = self.inner.lock().await;
            matches!(inner.status, PlayStatus::Playing | PlayStatus::Buffering)
        };

        // Finalize listen stats for current track
        self.end_current_listen(ListenOutcome::Stop).await;

        // Save state with was_playing flag so next startup auto-resumes
        let mut state = self.get_state().await;
        state.was_playing = was_playing;
        state.save(&self.data_dir);

        tracing::info!(
            "graceful shutdown complete (was_playing={was_playing}, pos={:.1}s)",
            state.position_secs
        );
    }

    /// Save current state to disk
    pub async fn save_state(&self) {
        let state = self.get_state().await;
        state.save(&self.data_dir);
    }

    /// Restore state from disk — resumes playback at the saved position.
    /// If `was_playing` is set (from graceful_shutdown), auto-resumes instead of pausing.
    pub async fn restore_state(&self) {
        if let Some(state) = PlayerState::load(&self.data_dir) {
            let resume_track = state.current_track.clone();
            let resume_pos = state.position_secs;
            let auto_resume = state.was_playing;

            {
                let mut inner = self.inner.lock().await;
                inner.queue = state.queue.into_iter().collect();
                inner.history = state.history;
                inner.volume = state.volume;
                inner.preamp = state.preamp;
                self.apply_volume(&inner);
            }

            // Clear was_playing on disk immediately (crash safety — only intentional
            // shutdown triggers auto-resume, not a crash-loop)
            if auto_resume {
                if let Some(mut cleared) = PlayerState::load(&self.data_dir) {
                    cleared.was_playing = false;
                    cleared.save(&self.data_dir);
                }
            }

            // Resume the track that was playing, seeking to saved position
            if let Some(track) = resume_track {
                tracing::info!(
                    "resuming '{}' at {:.0}s (auto_resume={})",
                    track.title,
                    resume_pos,
                    auto_resume,
                );
                match self.start_playback(track).await {
                    Ok(()) => {
                        // Seek to saved position
                        if resume_pos > 1.0 {
                            if let Err(e) = self.sink.try_seek(Duration::from_secs_f32(resume_pos))
                            {
                                tracing::warn!("seek to resume position failed: {e}");
                            }
                        }

                        if auto_resume {
                            // Graceful restart — keep playing
                            tracing::info!("auto-resuming playback");
                        } else {
                            // Normal startup — start paused so it doesn't blast
                            self.sink.pause();
                            let mut inner = self.inner.lock().await;
                            inner.status = PlayStatus::Paused;
                            drop(inner);
                        }
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
    pub async fn get_track_stats(&self, id: &str) -> Option<TrackStats> {
        let stats = self.stats.lock().await;
        stats.stats.get(id).cloned()
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
    pub fn storage(&self) -> &Arc<MonoStorage> {
        &self.storage
    }

    /// Get a reference to the music provider
    pub fn client(&self) -> &Arc<dyn MusicProvider> {
        &self.client
    }

    /// Toggle like on a track. Returns new liked state.
    pub async fn toggle_like(
        self: &Arc<Self>,
        track_id: &str,
        source: Option<String>,
    ) -> Result<bool, String> {
        let result = self.storage.toggle_like(track_id, source).await?;
        self.broadcast_now_playing().await;
        let this = self.clone();
        tokio::spawn(async move { this.sync_liked_playlist().await });
        Ok(result)
    }

    /// Sync liked tracks to {data_dir}/player/playlists/Liked.json
    async fn sync_liked_playlist(&self) {
        let liked = match self.storage.liked_ids_with_source().await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("sync_liked_playlist: failed to get liked ids: {e}");
                return;
            }
        };

        let playlist_path = self.data_dir.join("player/playlists/Liked.json");

        if liked.is_empty() {
            let _ = std::fs::remove_file(&playlist_path);
            return;
        }

        // Load existing Liked.json to reuse cached metadata
        let existing: HashMap<String, QueuedTrack> = std::fs::read_to_string(&playlist_path)
            .ok()
            .and_then(|s| serde_json::from_str::<PlaylistData>(&s).ok())
            .map(|p| p.tracks.into_iter().map(|t| { let id = t.id.clone(); (id, t) }).collect())
            .unwrap_or_default();

        let mut tracks = Vec::with_capacity(liked.len());
        for (id, source) in &liked {
            if let Some(mut cached) = existing.get(id).cloned() {
                cached.source.clone_from(source);
                tracks.push(cached);
            } else {
                // Fetch metadata for new likes
                let info = self.client.track_info(id).await.ok();
                let queued = match info {
                    Some(MonoEvent::Track {
                        title,
                        artist,
                        album,
                        duration_secs,
                        cover_id,
                        ..
                    }) => QueuedTrack {
                        id: id.clone(),
                        title,
                        artist,
                        album,
                        duration_secs,
                        quality: "LOSSLESS".into(),
                        cover_id,
                        source: source.clone(),
                    },
                    _ => QueuedTrack {
                        id: id.clone(),
                        title: format!("Track {id}"),
                        artist: String::new(),
                        album: String::new(),
                        duration_secs: 0,
                        quality: "LOSSLESS".into(),
                        cover_id: None,
                        source: source.clone(),
                    },
                };
                tracks.push(queued);
            }
        }

        let data = PlaylistData {
            name: "Liked".into(),
            description: "Auto-synced liked tracks".into(),
            tracks,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };

        if let Some(parent) = playlist_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&data) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&playlist_path, json) {
                    tracing::error!("sync_liked_playlist: write failed: {e}");
                }
            }
            Err(e) => tracing::error!("sync_liked_playlist: serialize failed: {e}"),
        }
    }

    /// Get all liked track IDs
    pub async fn liked_ids(&self) -> Result<Vec<String>, String> {
        self.storage.liked_ids().await
    }

    /// Download a track, register in storage, stream progress events via channel
    pub async fn download_track(
        self: &Arc<Self>,
        id: &str,
        quality: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<MonoEvent>, String> {
        // Get track info for organized path
        let track_info = self.client.track_info(id).await.ok();
        let (title, artist, album) = match &track_info {
            Some(MonoEvent::Track {
                title,
                artist,
                album,
                ..
            }) => (title.clone(), artist.clone(), album.clone()),
            _ => (
                format!("Track {id}"),
                String::from("Unknown"),
                String::from("Unknown"),
            ),
        };

        // Get stream manifest for extension
        let manifest = self.client.stream_manifest(id, quality).await?;
        let id = id.to_string();
        let ext = match &manifest {
            MonoEvent::StreamManifest { extension, .. } => extension.clone(),
            _ => "flac".to_string(),
        };

        // Build path: ~/Music/mono-tray/{artist}/{album}/{title}.{ext}
        let base = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music/mono-tray");
        let sanitize = |s: &str| s.replace(['/', '\\', ':'], "_");
        let dir = base.join(sanitize(&artist)).join(sanitize(&album));
        std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir failed: {e}"))?;
        let path = dir.join(format!("{}.{}", sanitize(&title), ext));
        let path_str = path.to_string_lossy().to_string();

        // Start download — returns a channel of progress events
        let mut inner_rx = self.client.download(&id, quality, &path_str).await?;

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
                    let _ = player
                        .storage
                        .register_download(
                            &id,
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
    pub async fn delete_download(&self, track_id: &str) -> Result<Option<String>, String> {
        let path = self.storage.delete_download(track_id).await?;
        self.broadcast_now_playing().await;
        Ok(path)
    }

    /// Get playback history (queue of previously played tracks)
    pub async fn get_history(&self) -> Vec<QueuedTrack> {
        let inner = self.inner.lock().await;
        inner.history.clone()
    }

    // --- Shuffle radio mode ---

    /// Start shuffle mode from a pool of track IDs. Clears the queue, picks
    /// a first track and queues 1-2 upcoming.
    pub async fn shuffle_start(&self, track_ids: Vec<String>) -> Result<(), String> {
        if track_ids.is_empty() {
            return Err("no tracks in shuffle pool".to_string());
        }

        {
            let mut inner = self.inner.lock().await;
            inner.shuffle.track_pool = track_ids;
            inner.shuffle.played.clear();
            inner.shuffle.active = true;
            inner.queue.clear();
        }

        // Pick the first track and start it
        let first_id = {
            let mut inner = self.inner.lock().await;
            inner.shuffle.pick_next()
        };
        if let Some(ref id) = first_id {
            let _ = self.queue_add(id, "LOSSLESS").await;
        }

        // Refill upcoming
        self.shuffle_refill().await;
        Ok(())
    }

    /// Stop shuffle mode. Leaves current track playing.
    pub async fn shuffle_stop(&self) {
        let mut inner = self.inner.lock().await;
        inner.shuffle.active = false;
        inner.shuffle.track_pool.clear();
        inner.shuffle.played.clear();
    }

    /// Refill the queue to have 1-2 upcoming tracks from the shuffle pool.
    async fn shuffle_refill(&self) {
        let ids_to_add: Vec<String> = {
            let mut inner = self.inner.lock().await;
            if !inner.shuffle.active {
                return;
            }
            let needed = 2usize.saturating_sub(inner.queue.len());
            let mut ids = Vec::new();
            for _ in 0..needed {
                if let Some(id) = inner.shuffle.pick_next() {
                    ids.push(id);
                } else {
                    break;
                }
            }
            ids
        };

        for id in &ids_to_add {
            let _ = self.queue_add(id, "LOSSLESS").await;
        }
    }

    /// Check if shuffle mode is active
    pub async fn shuffle_active(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.shuffle.active
    }

    /// Get shuffle pool info for status display
    pub async fn shuffle_status(&self) -> (bool, usize, usize) {
        let inner = self.inner.lock().await;
        (
            inner.shuffle.active,
            inner.shuffle.track_pool.len(),
            inner.shuffle.played.len(),
        )
    }
}

/// Build a QueuedTrack from track info (or fallback to minimal metadata)
fn make_queued_track(id: &str, quality: &str, info: Option<MonoEvent>) -> QueuedTrack {
    match info {
        Some(MonoEvent::Track {
            title,
            artist,
            album,
            duration_secs,
            cover_id,
            ..
        }) => QueuedTrack {
            id: id.to_string(),
            title,
            artist,
            album,
            duration_secs,
            quality: quality.to_string(),
            cover_id,
            source: None,
        },
        _ => QueuedTrack {
            id: id.to_string(),
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
