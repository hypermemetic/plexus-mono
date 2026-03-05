use anyhow::anyhow;
use clap::Parser;
use plexus_core::plexus::DynamicHub;
use plexus_mono::MonoHub;
use plexus_transport::TransportServer;
use std::sync::Arc;

/// CLI arguments for the plexus-mono standalone server
#[derive(Parser, Debug)]
#[command(name = "plexus-mono")]
#[command(
    about = "Monochrome music API standalone Plexus RPC server — search, metadata, lyrics, recommendations, playback"
)]
struct Args {
    /// Run in stdio mode for MCP compatibility (line-delimited JSON-RPC over stdin/stdout)
    #[arg(long)]
    stdio: bool,

    /// WebSocket port (ignored in stdio mode)
    #[arg(short, long, default_value = "4448")]
    port: u16,

    /// Enable MCP HTTP server (on port + 1)
    #[arg(long)]
    mcp: bool,

    /// Override the Monochrome API base URL (no trailing slash)
    ///
    /// Default: https://api.monochrome.tf
    /// Alternative: https://monochrome-api.samidy.com
    #[arg(long, env = "MONO_API_URL")]
    api_url: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Tracing setup — suppress noise in stdio mode
    let filter = if args.stdio {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"))
    } else {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new("warn,plexus_mono=debug")
        })
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Starting plexus-mono at {}", chrono::Utc::now());

    // Build multi-threaded tokio runtime
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Initialize hub + server on the runtime, then serve in background
    let is_stdio = args.stdio;
    rt.spawn(async move {
        if let Err(e) = run_server(args).await {
            tracing::error!("server error: {e}");
            std::process::exit(1);
        }
    });

    // Main thread: run macOS event loop so media keys + Now Playing widget work.
    // On non-macOS, just block until the runtime shuts down.
    if is_stdio {
        // stdio mode: no media controls needed, just block
        rt.block_on(futures::future::pending::<()>());
    } else {
        run_main_loop();
    }

    Ok(())
}

async fn run_server(args: Args) -> anyhow::Result<()> {
    // Build the activation (initializes audio playback engine + media controls)
    let mono_hub = if let Some(url) = args.api_url {
        tracing::info!("Using custom API URL: {}", url);
        MonoHub::with_url(url).await
    } else {
        tracing::info!("Using default API: https://api.monochrome.tf");
        MonoHub::new().await
    };

    // Wrap in a DynamicHub named "monochrome"; the activation inside is "mono"
    let hub = Arc::new(DynamicHub::new("monochrome").register(mono_hub));

    tracing::info!("plexus-mono initialized");
    tracing::info!("  Hub:        monochrome");
    tracing::info!("  Activation: mono");
    tracing::info!("  Version:    {}", env!("CARGO_PKG_VERSION"));

    // Configure transport
    let rpc_converter = |arc: Arc<DynamicHub>| {
        DynamicHub::arc_into_rpc_module(arc)
            .map_err(|e| anyhow!("Failed to create RPC module: {e}"))
    };

    let mut builder = TransportServer::builder(hub, rpc_converter);

    if args.stdio {
        builder = builder.with_stdio();
    } else {
        builder = builder.with_websocket(args.port);
        if args.mcp {
            builder = builder.with_mcp_http(args.port + 1);
        }
    }

    if args.stdio {
        tracing::info!("Starting stdio transport (MCP-compatible)");
    } else {
        tracing::info!("plexus-mono server started");
        tracing::info!("  WebSocket: ws://127.0.0.1:{}", args.port);
        if args.mcp {
            tracing::info!(
                "  MCP HTTP:  http://127.0.0.1:{}/mcp",
                args.port + 1
            );
        }
        tracing::info!("");
        tracing::info!("Usage examples:");
        tracing::info!(
            "  synapse -P {} monochrome mono track --id 12345",
            args.port
        );
        tracing::info!(
            "  synapse -P {} monochrome mono search --query 'radiohead' --kind tracks",
            args.port
        );
        tracing::info!(
            "  synapse -P {} monochrome mono play --id 55391801",
            args.port
        );
        tracing::info!(
            "  synapse -P {} monochrome mono pause",
            args.port
        );
        tracing::info!(
            "  synapse -P {} monochrome mono now_playing",
            args.port
        );
    }

    builder.build().await?.serve().await
}

/// Run the platform event loop on the main thread.
/// On macOS this is required for media key handling (MPRemoteCommandCenter).
#[cfg(target_os = "macos")]
fn run_main_loop() {
    // CFRunLoopRun blocks forever, processing system events including
    // media key dispatches from MPRemoteCommandCenter (via souvlaki).
    extern "C" {
        fn CFRunLoopRun();
    }
    unsafe {
        CFRunLoopRun();
    }
}

#[cfg(not(target_os = "macos"))]
fn run_main_loop() {
    // On non-macOS, just park the main thread
    loop {
        std::thread::park();
    }
}
