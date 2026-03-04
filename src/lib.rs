//! plexus-mono — Monochrome music API Plexus RPC activation
//!
//! Exposes the Monochrome / Hi-Fi Tidal proxy API as a Plexus RPC activation:
//! track metadata, album listings, artist info, search, lyrics,
//! recommendations, and cover art.

pub mod client;
pub mod hub;
pub mod types;

// Required by plexus-macros generated code
pub use plexus_core::serde_helpers;

// Re-exports for convenience
pub use hub::MonoHub;
pub use types::{MonoEvent, SearchKind};
