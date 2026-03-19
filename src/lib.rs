//! plexus-music — Generic music player library
//!
//! Provider traits, playback engine, queue management, playlists,
//! audio proxy, and RPC server infrastructure. Provider-agnostic:
//! implement `MusicProvider` to plug in any streaming backend.

pub mod audio_server;
pub mod player;
pub mod player_hub;
pub mod playlist;
pub mod provider;
pub mod server;
pub mod storage;
pub mod types;

// Required by plexus-macros generated code
pub use plexus_core::serde_helpers;

// Re-exports for convenience
pub use player_hub::PlayerHub;
pub use provider::MusicProvider;
pub use server::{build_player, run_main_loop, serve, MusicServerConfig};
pub use types::{MonoEvent, MusicEvent, SearchKind};
