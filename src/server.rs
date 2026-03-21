//! Library entry point for plexus-music servers
//!
//! Provides `build_player()` to initialize the player stack and `serve()` to run
//! transport. The binary wires provider-specific hubs, calls `build_player()`,
//! builds a `DynamicHub` with everything, then calls `serve()`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use plexus_core::plexus::DynamicHub;
use plexus_transport::TransportServer;
use tokio::signal::unix::{signal, SignalKind};

use crate::player_hub::PlayerHub;
use crate::provider::MusicProvider;
use crate::storage::MonoStorage;

/// Configuration for a plexus-music server instance.
pub struct MusicServerConfig {
    /// DynamicHub name (e.g. "music")
    pub hub_name: String,
    /// WebSocket port
    pub port: u16,
    /// Run in stdio transport mode (line-delimited JSON-RPC over stdin/stdout)
    pub stdio: bool,
    /// Enable MCP HTTP server on port + 1
    pub mcp: bool,
    /// HTTP audio proxy port (default: port + 2)
    pub audio_port: Option<u16>,
    /// Path to the SQLite database for likes & downloads
    pub db_path: PathBuf,
    /// Base directory for player state, playlists, and other persistent data.
    /// e.g. `~/.plexus/monochrome/` for monochrome, `~/.plexus/myfm/` for another provider.
    pub data_dir: PathBuf,
    /// Optional URL template for track links (e.g. "https://monochrome.tf/track/t/{}").
    /// `{}` is replaced with the track ID. If `None`, NowPlaying.url is always None.
    pub track_url_template: Option<String>,
}

impl MusicServerConfig {
    /// Default data directory for a named app: `~/.plexus/{app_name}/`
    pub fn default_data_dir(app_name: &str) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(format!(".plexus/{app_name}"))
    }

    /// Default db path within a data directory: `{data_dir}/mono.db`
    pub fn default_db_path(data_dir: &std::path::Path) -> PathBuf {
        data_dir.join("mono.db")
    }
}

/// Initialize the player stack.
///
/// Creates storage, builds the `PlayerHub`, installs SIGTERM handler for
/// graceful shutdown, and spawns the HTTP audio proxy (in non-stdio mode).
///
/// Returns the `PlayerHub` ready for `DynamicHub::register_hub()`.
pub async fn build_player(
    config: &MusicServerConfig,
    provider: Arc<dyn MusicProvider>,
) -> anyhow::Result<PlayerHub> {
    // Initialize SQLite storage for likes & downloads
    let storage = Arc::new(
        MonoStorage::new(config.db_path.clone())
            .await
            .map_err(|e| anyhow!("storage init failed: {e}"))?,
    );

    // Build the player activation (stateful — audio engine + queue + playlists)
    let player_hub = PlayerHub::new(
        provider,
        storage,
        config.data_dir.clone(),
        config.track_url_template.clone(),
    )
    .await;

    // SIGTERM handler — graceful shutdown saves state + was_playing flag
    {
        let player = player_hub.player();
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::spawn(async move {
                    sigterm.recv().await;
                    tracing::info!("received SIGTERM — starting graceful shutdown");
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        player.graceful_shutdown(),
                    )
                    .await
                    {
                        Ok(()) => tracing::info!("graceful shutdown complete"),
                        Err(_) => {
                            tracing::warn!("graceful shutdown timed out after 5s — forcing exit");
                        }
                    }
                    std::process::exit(0);
                });
            }
            Err(e) => {
                tracing::warn!(
                    "failed to install SIGTERM handler: {e} — graceful shutdown unavailable"
                );
            }
        }
    }

    // Spawn HTTP audio proxy for client-side failover
    if !config.stdio {
        let audio_port = config.audio_port.unwrap_or(config.port + 2);
        let player_ref = player_hub.player();
        let audio_client = Arc::clone(player_ref.client());
        let audio_storage = Arc::clone(player_ref.storage());
        tokio::spawn(async move {
            if let Err(e) =
                crate::audio_server::start_audio_server(audio_port, audio_client, audio_storage)
                    .await
            {
                tracing::error!("audio HTTP server error: {e}");
            }
        });
    }

    Ok(player_hub)
}

/// Run the transport server with a fully-built `DynamicHub`.
pub async fn serve(config: &MusicServerConfig, hub: Arc<DynamicHub>) -> anyhow::Result<()> {
    let rpc_converter = |arc: Arc<DynamicHub>| {
        DynamicHub::arc_into_rpc_module(arc)
            .map_err(|e| anyhow!("Failed to create RPC module: {e}"))
    };

    let mut builder = TransportServer::builder(hub, rpc_converter);

    if config.stdio {
        builder = builder.with_stdio();
    } else {
        builder = builder.with_websocket(config.port);
        if config.mcp {
            builder = builder.with_mcp_http(config.port + 1);
        }
    }

    if config.stdio {
        tracing::info!("Starting stdio transport (MCP-compatible)");
    } else {
        tracing::info!("plexus-music server started");
        tracing::info!("  WebSocket: ws://127.0.0.1:{}", config.port);
        if config.mcp {
            tracing::info!("  MCP HTTP:  http://127.0.0.1:{}/mcp", config.port + 1);
        }
    }

    builder.build().await?.serve().await
}

/// Run the platform event loop on the main thread.
///
/// On macOS this runs CFRunLoop for media key handling (MPRemoteCommandCenter).
/// On other platforms, parks the main thread.
/// In stdio mode, callers should block on a pending future instead.
#[cfg(target_os = "macos")]
pub fn run_main_loop() {
    extern "C" {
        fn CFRunLoopRun();
    }
    unsafe {
        CFRunLoopRun();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn run_main_loop() {
    loop {
        std::thread::park();
    }
}
