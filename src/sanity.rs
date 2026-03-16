//! SanityHub — diagnostic sub-activation for the Monochrome API
//!
//! Quick checks to diagnose latency and connectivity issues with
//! the upstream Monochrome API (api.monochrome.tf).

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use std::sync::Arc;
use std::time::Instant;

use plexus_core::plexus::{ChildRouter, PlexusError, PlexusStream};
use plexus_core::Activation;

use crate::client::MonoClient;
use crate::types::MonoEvent;

/// Known track ID for sanity checks (Radiohead — Karma Police)
const SANITY_TRACK_ID: u64 = 58990516;

/// Diagnostic sub-activation for checking Monochrome API health.
#[derive(Clone)]
pub struct SanityHub {
    client: Arc<MonoClient>,
}

impl SanityHub {
    pub fn new(client: Arc<MonoClient>) -> Self {
        Self { client }
    }

    /// No children
    pub fn plugin_children(&self) -> Vec<plexus_core::plexus::schema::ChildSummary> {
        vec![]
    }
}

fn sanity_ok(check: &str, start: Instant, message: String) -> MonoEvent {
    MonoEvent::SanityResult {
        check: check.to_string(),
        passed: true,
        duration_ms: start.elapsed().as_millis() as u64,
        message,
    }
}

fn sanity_fail(check: &str, start: Instant, message: String) -> MonoEvent {
    MonoEvent::SanityResult {
        check: check.to_string(),
        passed: false,
        duration_ms: start.elapsed().as_millis() as u64,
        message,
    }
}

async fn run_ping(client: &MonoClient) -> MonoEvent {
    let start = Instant::now();
    let url = client.base_url().to_string();
    match reqwest::Client::new().get(&url).send().await {
        Ok(resp) => sanity_ok(
            "ping",
            start,
            format!("HTTP {} from {}", resp.status(), client.base_url()),
        ),
        Err(e) => sanity_fail("ping", start, format!("connection failed: {e}")),
    }
}

async fn run_track(client: &MonoClient) -> MonoEvent {
    let start = Instant::now();
    match client.track_info(SANITY_TRACK_ID).await {
        Ok(MonoEvent::Track { title, artist, .. }) => {
            sanity_ok("track", start, format!("{artist} — {title}"))
        }
        Ok(_) => sanity_ok("track", start, "got response".into()),
        Err(e) => sanity_fail("track", start, e),
    }
}

async fn run_search(client: &MonoClient) -> MonoEvent {
    let start = Instant::now();
    match client
        .search("test", &crate::types::SearchKind::Tracks, 1, 0)
        .await
    {
        Ok(results) => sanity_ok(
            "search",
            start,
            format!("{} result(s)", results.len()),
        ),
        Err(e) => sanity_fail("search", start, e),
    }
}

async fn run_stream(client: &MonoClient) -> MonoEvent {
    let start = Instant::now();
    match client.stream_manifest(SANITY_TRACK_ID, "LOSSLESS").await {
        Ok(MonoEvent::StreamManifest { quality, .. }) => {
            sanity_ok("stream", start, format!("resolved {quality} manifest"))
        }
        Ok(_) => sanity_ok("stream", start, "got manifest".into()),
        Err(e) => sanity_fail("stream", start, e),
    }
}

async fn run_cover(client: &MonoClient) -> MonoEvent {
    let start = Instant::now();
    match client.cover(SANITY_TRACK_ID, 640).await {
        Ok(covers) => sanity_ok(
            "cover",
            start,
            format!("{} size(s) returned", covers.len()),
        ),
        Err(e) => sanity_fail("cover", start, e),
    }
}

#[plexus_macros::hub_methods(
    namespace = "sanity",
    version = "0.1.0",
    hub,
    description = "Diagnostic checks for the Monochrome API — ping, latency, connectivity",
    crate_path = "plexus_core"
)]
impl SanityHub {
    /// Ping the API with a HEAD request to check basic connectivity
    #[plexus_macros::hub_method(
        description = "HTTP HEAD to the API base URL — reports response time and status code"
    )]
    pub async fn ping(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            yield run_ping(&client).await;
        }
    }

    /// Fetch track metadata for a known track to verify the /info endpoint
    #[plexus_macros::hub_method(
        description = "Fetch track info for Karma Police (58990516) — verifies /info endpoint and reports timing"
    )]
    pub async fn track(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            yield run_track(&client).await;
        }
    }

    /// Run a minimal search to verify the /search endpoint
    #[plexus_macros::hub_method(
        description = "Search 'test' with limit 1 — verifies /search endpoint and reports timing"
    )]
    pub async fn search(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            yield run_search(&client).await;
        }
    }

    /// Resolve a stream manifest to verify the /track endpoint
    #[plexus_macros::hub_method(
        description = "Resolve stream manifest for Karma Police — verifies /track endpoint and reports timing"
    )]
    pub async fn stream_url(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            yield run_stream(&client).await;
        }
    }

    /// Fetch cover art to verify the /cover endpoint
    #[plexus_macros::hub_method(
        description = "Fetch cover art for Karma Police — verifies /cover endpoint and reports timing"
    )]
    pub async fn cover(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            yield run_cover(&client).await;
        }
    }

    /// Run all sanity checks sequentially, streaming each result
    #[plexus_macros::hub_method(
        streaming,
        description = "Run all diagnostic checks (ping, track, search, stream, cover) and stream results"
    )]
    pub async fn all(&self) -> impl Stream<Item = MonoEvent> + Send + 'static {
        let client = self.client.clone();
        stream! {
            yield run_ping(&client).await;
            yield run_track(&client).await;
            yield run_search(&client).await;
            yield run_stream(&client).await;
            yield run_cover(&client).await;
        }
    }
}

#[async_trait]
impl ChildRouter for SanityHub {
    fn router_namespace(&self) -> &str {
        "sanity"
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
