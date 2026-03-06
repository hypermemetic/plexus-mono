---
name: mono
description: "Use when the user wants to control music playback, search for music, manage playlists, adjust volume, queue tracks, or interact with plexus-mono / monochrome in any way. Trigger on: play, pause, skip, next, previous, volume, preamp, queue, playlist, search, now playing, what's playing, music."
argument-hint: "[command]"
allowed-tools:
  - Bash
  - Read
  - AskUserQuestion
---

# Plexus Mono — Music Player Control

Plexus Mono is a Rust-based music server wrapping the Monochrome (Tidal) API with a built-in playback engine. All interaction goes through `synapse` CLI.

## Connection

```bash
# Default port
synapse -P 4448 monochrome <activation> <method> [--param value ...]
```

Two activations:
- `monochrome` — stateless API (search, metadata, lyrics, covers)
- `player` — playback engine (play, queue, volume, playlists)

Synapse auto-strips duplicate namespace, so `synapse -P 4448 monochrome search` works (no need for `monochrome monochrome`).

## Quick Reference

### Search & Discovery
```bash
# Search tracks (default)
synapse -P 4448 monochrome search --query "artist or song name"

# Search albums
synapse -P 4448 monochrome search --query "album name" --kind albums

# Search artists
synapse -P 4448 monochrome search --query "artist" --kind artists

# Track metadata
synapse -P 4448 monochrome track --id <TIDAL_ID>

# Album tracks
synapse -P 4448 monochrome album --id <ALBUM_ID>

# Lyrics
synapse -P 4448 monochrome lyrics --id <TRACK_ID>

# Recommendations
synapse -P 4448 monochrome recommendations --id <TRACK_ID>

# Cover art
synapse -P 4448 monochrome cover --id <TRACK_ID>
```

### Playback Control
```bash
# Play a track immediately
synapse -P 4448 monochrome player play --id <TRACK_ID>

# Pause / Resume / Stop
synapse -P 4448 monochrome player pause
synapse -P 4448 monochrome player resume
synapse -P 4448 monochrome player stop

# Skip / Previous
synapse -P 4448 monochrome player next
synapse -P 4448 monochrome player previous
# Note: previous restarts the track if >5s in; press again within 5s to go back

# Now playing (streams updates ~1s)
synapse -P 4448 monochrome player now_playing
```

### Volume & Pre-amp
```bash
# Volume: 0.0 to 1.0 (master fader)
synapse -P 4448 monochrome player volume --level 0.5

# Pre-amp: 0.0 to 4.0 (gain boost, >1.0 amplifies)
synapse -P 4448 monochrome player preamp --level 1.5

# Effective output = volume × preamp
# e.g., volume 0.5 × preamp 2.0 = 1.0 effective
```

When the user says "volume 10" or "turn it to 5", interpret as percentage (0.10, 0.05).

### Queue Management
```bash
# Add single track
synapse -P 4448 monochrome player queue_add --id <TRACK_ID>

# Add multiple tracks at once
synapse -P 4448 monochrome player queue_batch --ids <ID1> --ids <ID2> --ids <ID3>

# Queue entire album
synapse -P 4448 monochrome player queue_album --id <ALBUM_ID>

# View queue
synapse -P 4448 monochrome player queue_get

# Clear queue (doesn't stop current track)
synapse -P 4448 monochrome player queue_clear

# Reorder
synapse -P 4448 monochrome player queue_reorder --from <IDX> --to <IDX>
```

### Playlists
```bash
# List playlists
synapse -P 4448 monochrome player playlist list

# Create with tracks
synapse -P 4448 monochrome player playlist create --name "my playlist" --ids <ID1> --ids <ID2>

# Play a playlist
synapse -P 4448 monochrome player playlist play --name "my playlist"

# Save current queue as playlist
synapse -P 4448 monochrome player playlist save --name "my playlist"

# Add track to playlist
synapse -P 4448 monochrome player playlist add --name "my playlist" --id <TRACK_ID>

# Remove track by index
synapse -P 4448 monochrome player playlist remove --name "my playlist" --index 0

# Show playlist tracks
synapse -P 4448 monochrome player playlist show --name "my playlist"

# Delete / Rename
synapse -P 4448 monochrome player playlist delete --name "old name"
synapse -P 4448 monochrome player playlist rename --name "old" --new_name "new"
```

### JSON Output
Add `-j` flag BEFORE the backend name for raw JSON (useful for parsing):
```bash
synapse -P 4448 -j monochrome player queue_get
```

## Monochrome Web URLs

Link to tracks, albums, and artists on monochrome.tf:
- Track: `https://monochrome.tf/track/t/<TIDAL_ID>`
- Album: `https://monochrome.tf/album/t/<TIDAL_ID>`
- Artist: `https://monochrome.tf/artist/t/<TIDAL_ID>`

## Server Management

```bash
# Start (runs in background, port 4448)
plexus-mono &

# Install from source
cargo install --path /Users/shmendez/dev/controlflow/hypermemetic/plexus-mono

# Hotswap: build first, THEN kill+restart
cargo install --path . && lsof -ti :4448 | xargs kill; sleep 1 && plexus-mono &
```

On restart, the player restores: current track (from beginning, paused), queue, history, volume, and preamp.

## Behavioral Notes

- When user says "play X" — search, pick the top result, and play it
- When user says "put on an album" — search albums, queue the whole thing
- When user says numbers for volume (e.g., "turn it to 7") — interpret as percentage (0.07)
- Queue auto-starts if idle when adding tracks
- Tracks pre-buffer in the background for instant skipping
- All state persists to `~/.plexus/monochrome/player/state.json`
- Playlists persist to `~/.plexus/monochrome/player/playlists/`
