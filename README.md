# Mono Tray

A macOS menu bar music player for Tidal, powered by [Plexus](https://github.com/juggernautlabs/plexus-core) RPC and the [Monochrome](https://monochrome.tf) API. Lives in your menu bar, streams lossless audio, downloads for offline, and builds playlists with AI research.

## Architecture

```
┌─────────────┐    WebSocket     ┌──────────────┐     HTTPS     ┌─────────────────┐
│  Mono Tray   │ ◄──JSON-RPC──► │  plexus-mono  │ ◄──────────► │  Monochrome API  │
│  (Tauri app) │                 │  (Rust server)│              │  (Tidal proxy)   │
└─────────────┘                  └──────────────┘              └─────────────────┘
     ▲                                  │
     │ TypeScript                       │ rodio (audio)
     │ generated clients                │ SQLite (likes, downloads)
     │ (synapse-cc)                     │ JSON (playlists, stats, state)
```

- **Frontend**: TypeScript + Tauri 2, bundled with Bun
- **Backend**: Rust, Plexus RPC over WebSocket (port 4448)
- **Audio**: rodio on a dedicated OS thread (Sink is Send+Sync via Arc)
- **Codegen**: `synapse-cc` generates typed TypeScript clients from the live backend schema

## Features

### Playback

- Play, pause, resume, stop, seek, next, previous
- Lossless streaming (LOSSLESS, HI_RES_LOSSLESS, HIGH, LOW quality tiers)
- Offline playback from local downloads
- Volume control (0–100%) and pre-amp gain (0–4x boost)
- Playback state persists across restarts (current track, position, queue, volume)
- macOS Now Playing integration and media key support

### Queue

- Add individual tracks, batch add, or queue entire albums
- Reorder tracks via drag indices
- Auto-starts playback when adding to an empty queue
- Save current queue as a playlist
- Source labels track where each queued item came from

### Downloads

- Download tracks to `~/Music/mono-tray/{artist}/{album}/`
- Real-time progress ring in the UI (circular SVG fill)
- Click to cancel mid-download
- Downloaded tracks toggle to a delete button — click to remove file and DB entry
- Empty artist/album directories auto-pruned on delete
- Download state reflected in now-playing updates

### Likes

- Toggle heart on any track (optimistic UI with pop animation)
- Liked songs appear as a virtual playlist in the library
- Like state shown in now-playing stream

### Playlists

- Create, rename, delete playlists
- Add/remove tracks, save queue as playlist
- Stored as JSON in `~/.plexus/monochrome/player/playlists/`
- Playlist picker overlay for quick "add to playlist" from any track row

### AI Research

Three-phase AI-powered playlist curation via Claude + web search:

1. **Theme Research** — Claude searches the web to understand a music theme or query, generates 10–20 specific search terms
2. **Catalog Search** — Executes all suggestions in parallel against the Tidal catalog, pulls album tracks, deduplicates
3. **Curation** — Claude picks the best tracks, orders them into a thematic arc, explains each choice

Results are saved with full provenance (search terms used, all found tracks, curation reasoning).

### Search & Browse

- Real-time search across tracks, albums, and artists with tab switching
- Artist view shows all albums; album view shows all tracks with metadata
- Clickable artist/album names in now-playing to navigate directly
- Lazy-loaded cover art throughout

### Listening Statistics

- Per-track stats: play count, complete count, skip count, total listen time
- Full listen log with timestamps and outcome (complete, skip, stop)
- Top tracks and recent listens queries
- Stats persist in `~/.plexus/monochrome/player/stats.json`

### History

- Previously played tracks listed most-recent-first
- Click to replay any track from history

### UI & Interactions

- **Menu bar panel** — transparent, borderless, always-on-top NSPanel positioned under the tray icon
- **View navigation** — horizontal slide transitions between Now Playing, Browse, Detail, Queue, Research, and History views
- **Breadcrumbs** — clickable navigation path
- **Progress scrubbing** — drag the progress bar thumb for precise seeking
- **Hover effects** — native mouse monitoring forwarded to the webview for CSS hover states
- **Click feedback** — brief accent flash on interactive elements
- **Like animation** — pop/bounce keyframes on heart toggle
- **Download progress** — circular ring that fills in real time, smooth CSS transitions
- **Disconnect overlay** — blurred overlay with auto-reconnect when backend is unreachable
- **Desktop notifications** — track changes, research completion, playlist creation
- **Window auto-resize** — height adjusts per view (556px now-playing, 600px browse/queue/history, 650px research)
- **Click-away dismiss** — panel hides when clicking outside
- **Space change dismiss** — panel hides when switching macOS Spaces

### Tray Context Menu

Right-click the menu bar icon for:

- **Open Music Folder** — opens `~/Music/mono-tray/` in Finder
- **Restart Server** — kills and relaunches the plexus-mono backend
- **Restart** — restarts the app
- **Quit** — exits

## Building

### Prerequisites

- Rust toolchain
- [Bun](https://bun.sh)
- [synapse-cc](https://github.com/juggernautlabs/synapse-cc) (for codegen)
- macOS (Tauri + native panel APIs)

### Quick Start

```bash
# Start the backend
make backend

# In another terminal — full build cycle:
# restarts backend, regenerates TS clients, builds release app, launches
cd mono-tray
make full
```

### Build Targets

| Target | Description |
|--------|-------------|
| `make full` | Restart backend + codegen + build + run (the everything target) |
| `make run` | Build release app, install to ~/Applications, launch |
| `make dev` | Tauri dev server with hot reload |
| `make backend` | Start plexus-mono on port 4448 |
| `make backend-restart` | Kill and restart the backend |
| `make codegen-force` | Regenerate TypeScript clients from live backend |
| `make frontend` | Bundle TypeScript with Bun |
| `make app` | Build native macOS .app (release) |
| `make clean` | Remove build artifacts |

### Data Locations

| What | Where |
|------|-------|
| Downloads | `~/Music/mono-tray/{artist}/{album}/` |
| Playlists | `~/.plexus/monochrome/player/playlists/` |
| Player state | `~/.plexus/monochrome/player/state.json` |
| Listen stats | `~/.plexus/monochrome/player/stats.json` |
| Listen log | `~/.plexus/monochrome/player/listen_log.json` |
| Likes & download registry | SQLite (in-process) |

## RPC API

The backend exposes two Plexus hubs over WebSocket JSON-RPC 2.0:

**`monochrome`** (raw Tidal proxy): `track`, `album`, `artist`, `cover`, `search`, `lyrics`, `recommendations`, `stream_url`, `download`

**`player`** (stateful playback engine): `play`, `pause`, `resume`, `stop`, `seek`, `next`, `previous`, `volume`, `preamp`, `queue_add`, `queue_batch`, `queue_album`, `queue_clear`, `queue_get`, `queue_reorder`, `status`, `now_playing`, `like`, `liked_tracks`, `download_track`, `delete_download`, `stats`, `stats_top`, `stats_recent`, `history`, `history_list`, `history_clear`

**`player.playlist`** (nested): `create`, `list`, `load`, `add`, `remove`, `save`, `rename`, `delete`, `research_save`

Use [Synapse CLI](https://github.com/juggernautlabs/synapse) to explore interactively:

```bash
synapse -P 4448 player status
synapse -P 4448 player play --id 12345
synapse -P 4448 player queue_add --id 67890 --source "from CLI"
synapse -P 4448 player playlist list
```
