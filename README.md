# plexus-music

A generic music player library for Rust. Provides the provider trait hierarchy, a full playback engine (queue, playlists, listening stats, likes, downloads), an HTTP audio proxy, and a [Plexus](https://github.com/juggernautlabs/plexus-core) RPC server harness — all provider-agnostic. Implement `MusicProvider` for your streaming backend, call `build_player()`, wire up a `DynamicHub`, and call `serve()`.

## Provider Trait Hierarchy

Five capability traits compose into a single blanket `MusicProvider`:

```
MusicStreaming    — resolve stream manifests, download tracks
MusicMetadata    — track, album, and artist metadata lookup
MusicSearch      — search across tracks, albums, and artists
MusicLyrics      — fetch synced or plain lyrics
MusicEnrichment  — cover art and recommendations
        │
        └──► MusicProvider  (blanket impl for any T: all five)
```

Each trait is independently implementable. A lyrics-only integration can implement just `MusicLyrics`; a full streaming provider implements all five and automatically satisfies `MusicProvider`.

### Trait Signatures

```rust
#[async_trait]
pub trait MusicStreaming: Send + Sync + 'static {
    async fn stream_manifest(&self, id: u64, quality: &str) -> Result<MusicEvent, String>;
    async fn download(&self, id: u64, quality: &str, path: &str)
        -> Result<Receiver<MusicEvent>, String>;
}

#[async_trait]
pub trait MusicMetadata: Send + Sync + 'static {
    async fn track_info(&self, id: u64) -> Result<MusicEvent, String>;
    async fn album(&self, id: u64) -> Result<(MusicEvent, Vec<MusicEvent>), String>;
    async fn artist(&self, id: u64) -> Result<MusicEvent, String>;
}

#[async_trait]
pub trait MusicSearch: Send + Sync + 'static {
    async fn search(&self, query: &str, kind: &SearchKind, limit: u32, offset: u32)
        -> Result<Vec<MusicEvent>, String>;
}

#[async_trait]
pub trait MusicLyrics: Send + Sync + 'static {
    async fn lyrics(&self, id: u64) -> Result<Vec<MusicEvent>, String>;
}

#[async_trait]
pub trait MusicEnrichment: Send + Sync + 'static {
    async fn cover(&self, id: u64, size: u32) -> Result<Vec<MusicEvent>, String>;
    async fn recommendations(&self, id: u64) -> Result<Vec<MusicEvent>, String>;
}
```

## Key Modules

| Module | Description |
|--------|-------------|
| `provider` | Trait hierarchy (`MusicStreaming`, `MusicMetadata`, `MusicSearch`, `MusicLyrics`, `MusicEnrichment`, `MusicProvider`) |
| `player` | Audio playback engine backed by rodio — play, pause, seek, volume, pre-amp, media keys, Now Playing OS integration |
| `player_hub` | Plexus `Activation` wrapping the player with queue management and playlist child routing |
| `playlist` | Persistent playlist management (JSON on disk), playlist CRUD, save-queue-as-playlist |
| `storage` | SQLite-backed likes and download registry (`MonoStorage`) |
| `audio_server` | HTTP audio proxy for client-side stream failover (axum, port + 2 by default) |
| `server` | `MusicServerConfig`, `build_player()`, `serve()`, `run_main_loop()` — the server wiring entry points |
| `types` | `MusicEvent`, `MonoEvent`, `SearchKind` — the shared event envelope types |

## How to Use

### 1. Implement `MusicProvider`

```rust
use plexus_music::provider::{
    MusicEnrichment, MusicLyrics, MusicMetadata, MusicSearch, MusicStreaming,
};
use plexus_music::types::{MusicEvent, SearchKind};
use async_trait::async_trait;
use tokio::sync::mpsc::Receiver;

struct MyProvider { /* your HTTP client, auth tokens, etc. */ }

#[async_trait]
impl MusicStreaming for MyProvider {
    async fn stream_manifest(&self, id: u64, quality: &str) -> Result<MusicEvent, String> {
        // resolve a pre-signed CDN URL for the track
        todo!()
    }
    async fn download(&self, id: u64, quality: &str, path: &str)
        -> Result<Receiver<MusicEvent>, String>
    {
        todo!()
    }
}

#[async_trait]
impl MusicMetadata for MyProvider { /* ... */ }

#[async_trait]
impl MusicSearch for MyProvider { /* ... */ }

#[async_trait]
impl MusicLyrics for MyProvider { /* ... */ }

#[async_trait]
impl MusicEnrichment for MyProvider { /* ... */ }

// MyProvider now satisfies MusicProvider automatically — no extra impl needed.
```

### 2. Build the player and serve

```rust
use std::sync::Arc;
use plexus_music::{build_player, serve, run_main_loop, MusicServerConfig};
use plexus_core::plexus::DynamicHub;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = Arc::new(MyProvider::new());

    let config = MusicServerConfig {
        hub_name: "music".into(),
        port: 4448,
        stdio: false,
        mcp: false,
        audio_port: None,                          // defaults to port + 2
        db_path: MusicServerConfig::default_db_path(),
    };

    // Initialize storage, player engine, audio proxy, and signal handler
    let player_hub = build_player(&config, provider.clone()).await?;

    // Register your provider hub(s) alongside the built-in player hub
    let mut hub = DynamicHub::new(&config.hub_name);
    hub.register_hub(player_hub);
    // hub.register_hub(my_provider_hub);   // optional: expose raw provider RPC too

    // Start WebSocket server (and optional MCP HTTP server)
    let hub = Arc::new(hub);
    tokio::spawn(serve(&config, hub.clone()));  // or .await if this is the only task

    // On macOS: runs CFRunLoop for media key / Now Playing support.
    // On other platforms: parks the main thread.
    run_main_loop();
    Ok(())
}
```

### 3. Explore the API via Synapse

Once running, [Synapse CLI](https://github.com/juggernautlabs/synapse) gives you a self-documenting shell over the RPC API:

```bash
synapse -P 4448 player status
synapse -P 4448 player play --id 12345
synapse -P 4448 player queue_add --id 67890 --source "cli"
synapse -P 4448 player playlist list
synapse -P 4448 player now_playing      # streaming event subscription
```

## Player RPC Surface

`PlayerHub` exposes the following methods over Plexus JSON-RPC 2.0:

**Playback**: `play`, `pause`, `resume`, `stop`, `seek`, `next`, `previous`, `volume`, `preamp`, `now_playing`, `status`

**Queue**: `queue_add`, `queue_batch`, `queue_album`, `queue_clear`, `queue_get`, `queue_reorder`

**Likes**: `like`, `liked_tracks`

**Downloads**: `download_track`, `delete_download`

**Statistics**: `stats`, `stats_top`, `stats_recent`

**History**: `history`, `history_list`, `history_clear`

**Playlists** (child hub `player.playlist`): `create`, `list`, `load`, `add`, `remove`, `save`, `rename`, `delete`

## Transport Modes

`MusicServerConfig` supports three transport modes:

| Mode | How |
|------|-----|
| WebSocket (default) | `stdio: false` — listens on `ws://127.0.0.1:{port}` |
| MCP HTTP | `mcp: true` — also serves `http://127.0.0.1:{port+1}/mcp` |
| stdio | `stdio: true` — line-delimited JSON-RPC on stdin/stdout (MCP-compatible) |

## Adding as a Dependency

```toml
[dependencies]
plexus-music = "0.3"
```

The library depends on:

- `plexus-core` / `plexus-macros` / `plexus-transport` — Plexus RPC framework
- `tokio` — async runtime
- `rodio` — cross-platform audio playback
- `souvlaki` — OS media controls (Now Playing, media keys)
- `sqlx` / SQLite — likes and download registry
- `axum` — HTTP audio proxy
- `serde` / `schemars` — serialization and JSON Schema generation for Plexus

## License

MIT
