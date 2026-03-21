# Provider Extraction, FOSS Hardening, and Royalty-Free Provider

**Date:** 2026-03-18 through 2026-03-20
**Scope:** plexus-mono → plexus-music rename, mono-provider extraction, plexus-music-royalty-free creation, production hardening, strict linting

## Summary

Split the monolithic `plexus-mono` crate into a generic open-source library (`plexus-music`) and a private provider (`mono-provider`), then built a second provider (`plexus-music-royalty-free`) against the Openverse API to prove the architecture works. Along the way: production hardening, strict linting, ID type migration from u64 to String, and unified provider namespace.

## Architecture Decisions

### 1. Library / Provider Split

**Before:** One crate (`plexus-mono`) containing the player engine, provider traits, Monochrome API client, and the binary.

**After:** Three crates:
- `plexus-music` — generic library (MIT, publishable). Provider traits, player engine, queue, playlists, storage, audio proxy, RPC server harness.
- `mono-provider` — private binary. Monochrome/Tidal API client + hub, thin binary that wires everything together.
- `plexus-music-royalty-free` — public binary. Openverse CC-licensed music client + hub, same thin binary pattern.

**Why `build_player()` + `serve()` instead of a trait object:**
`DynamicHub::register_hub()` requires `A: Activation + ChildRouter + Clone + 'static` — can't be type-erased behind `Box<dyn Trait>`. So the library provides building blocks and the binary is the composition root:

```rust
let provider_hub = MyProviderHub::new();
let player_hub = build_player(&config, provider_hub.client()).await?;
let hub = DynamicHub::new("music")
    .register_hub(provider_hub)
    .register_hub(player_hub);
serve(&config, Arc::new(hub)).await
```

### 2. Configurable Data Directory

**Before:** Hardcoded `~/.plexus/monochrome/` everywhere — player state, stats, playlists, SQLite.

**After:** `MusicServerConfig` has a `data_dir: PathBuf` field. Each provider sets its own:
- mono-provider: `~/.plexus/monochrome/`
- royalty-free: `~/.plexus/royalty-free/`

Also added `track_url_template: Option<String>` to config — mono-provider sets `"https://monochrome.tf/track/t/{}"`, royalty-free leaves it `None`. The NowPlaying `url` field uses this template.

### 3. String IDs (was u64)

**Before:** All track/album/artist IDs were `u64` throughout the type system, traits, storage, and frontend.

**After:** All IDs are `String`. This was required because Openverse uses UUID strings (`"98b472ae-b415-401f-9a4b-4c0a033390ac"`). Numeric IDs from Monochrome are stored as `"58990516"`.

**Migration path:**
- SQLite: `run_migrations()` detects INTEGER columns via `pragma_table_info`, does rename → create TEXT → copy → drop.
- Playlist JSON: `QueuedTrack.id` uses `#[serde(deserialize_with = "deserialize_string_or_number")]` to accept both `"id": 12345` (old) and `"id": "abc-def"` (new).
- Frontend: all ID types changed from `number` to `string`, `Map<number, ...>` → `Map<string, ...>`.

### 4. Unified Provider Namespace

**Before:** Each provider had its own namespace — `monochrome`, `openverse`. The frontend imported `createMonochromeClient` and hardcoded calls to `mono.cover()`, `mono.search()`, etc.

**After:** All providers use `namespace = "provider"` and `router_namespace() -> "provider"`. The frontend imports `createProviderClient` and calls `provider.search()`, `provider.cover()`, etc. Swapping backends requires no frontend changes — just restart with a different binary on port 4448.

### 5. Openverse Provider Implementation

**API:** `https://api.openverse.org/v1/` — no authentication required. Aggregates 4.9M CC-licensed audio items from Jamendo (628k), Wikimedia Commons (3.8M), and Freesound (585k).

**Key design decisions:**

- **Album reconstruction:** Openverse has no album endpoint. Albums are reconstructed by: (1) extracting the creator name from the `audio_set` field, (2) searching by `?creator={name}`, (3) filtering results by `audio_set.foreign_landing_url` match. An `album_creators` cache (populated from search results) enables album lookups even when the slug search fails.

- **Cover art:** Uses `thumbnail` field directly — it's a URL to the Openverse thumbnail proxy. Stored in `cover_id` on `QueuedTrack`. The frontend's `cacheCoverFromTrack()` seeds the cover cache from queue data so covers load without an extra RPC call.

- **No hashing:** Early implementation hashed UUIDs to u64 for compatibility. This broke JavaScript (overflow past `Number.MAX_SAFE_INTEGER`) and required a reverse-lookup cache. Replaced with String IDs — UUIDs pass through unchanged.

### 6. CSP for Provider-Agnostic Image Loading

**Before:** `img-src 'self' https://*.tidal.com` — only Tidal cover art allowed.

**After:** `img-src 'self' https:` — any HTTPS image source. Required for Openverse thumbnails (`api.openverse.org`) and Jamendo covers (`usercontent.jamendo.com`).

### 7. Frontend Provider Abstraction

The frontend (`mono-tray/src/app.ts`) was refactored to be provider-agnostic:

- `createProviderClient(rpc)` instead of `createMonochromeClient(rpc)`
- `provider.search()`, `provider.cover()`, `provider.album()` instead of `mono.*`
- Album links are non-clickable when `albumName.startsWith('CC ·')` (genre labels, not real albums)
- Track URL uses `np.url` from NowPlaying (set by `track_url_template`) instead of hardcoded `monochrome.tf`
- `playTrack()` helper shows immediate loading state (title, artist, pause icon) before backend responds — critical UX when API is slow

### 8. Production Hardening

**Panic removal:**
- `rodio::DeviceSinkBuilder::open_default_sink().expect()` → match with `tracing::error!` + early return
- `signal(SignalKind::terminate()).expect()` → match with `tracing::warn!` fallback
- Media controls `.expect("failed to spawn")` → `if let Err(e)` with warning
- `queue.remove(from).unwrap()` → `.ok_or_else()?`

**SQLite migrations:**
- `ALTER TABLE ... .ok()` (swallowed all errors) → specific check for "duplicate column"
- Added INTEGER → TEXT migration for track_id columns

**Silent failures:**
- `let _ = fs::create_dir_all()` → `if let Err(e) { tracing::warn!() }`
- Same for state file writes

**Tauri restart server:**
- Updated from deleted `src/bin/plexus_mono.rs` path to `plexus-mono` binary in PATH, with fallback to `mono-provider/` directory

### 9. Strict Linting

**Rust (both crates):**
```toml
[lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "deny", priority = -1 }
```
With targeted allows for `module_name_repetitions`, `must_use_candidate`, `missing_errors_doc`, `cast_possible_truncation`, etc.

`rustfmt.toml`: `edition = "2021"`, `max_width = 100`, `use_field_init_shorthand = true`

**TypeScript:**
```json
{
  "strict": true,
  "noUncheckedIndexedAccess": true,
  "noUnusedLocals": true,
  "noUnusedParameters": true,
  "exactOptionalPropertyTypes": true,
  "noImplicitReturns": true,
  "noPropertyAccessFromIndexSignature": true,
  "verbatimModuleSyntax": true
}
```

Project references: `tsconfig.json` (strict, src/) references `generated/tsconfig.json` (basic strict, codegen output). `tsc --build` compiles generated first, then src/ against those declarations.

Build pipeline: `bun run build` = `tsc --build --force` + `bun build` (type errors fail the build).

### 10. synapse-cc Integration-Mode tsconfig

Modified `Pipeline.hs` to generate `tsconfig.json` in integration mode (previously skipped). The generated tsconfig has `composite: true`, `declaration: true` — compatible with TypeScript project references. Host projects reference it as a sub-project with its own (relaxed) lint settings.

## Files Created

| File | Purpose |
|------|---------|
| `plexus-mono/src/server.rs` | Library entry point: `MusicServerConfig`, `build_player()`, `serve()`, `run_main_loop()` |
| `plexus-mono/LICENSE` | MIT license text |
| `plexus-mono/Dockerfile` | Multi-stage build/test container |
| `plexus-mono/rustfmt.toml` | Rust formatting config |
| `plexus-mono/mono-tray/.prettierrc` | TypeScript formatting config |
| `plexus-mono/mono-tray/README.md` | Mono Tray app documentation (moved from root) |
| `plexus-mono/mono-tray/generated/tsconfig.json` | Generated by synapse-cc for project references |
| `mono-provider/` | Private Monochrome/Tidal provider crate (entire new crate) |
| `plexus-music-royalty-free/` | Public Openverse CC provider crate (entire new crate) |
| `synapse-cc/src/SynapseCC/Pipeline.hs` | Integration-mode tsconfig generation |

## Files Deleted

| File | Reason |
|------|--------|
| `plexus-mono/src/client.rs` | Moved to `mono-provider/src/client.rs` |
| `plexus-mono/src/hub.rs` | Moved to `mono-provider/src/hub.rs` |
| `plexus-mono/src/sanity.rs` | Moved to `mono-provider/src/sanity.rs` |
| `plexus-mono/src/bin/plexus_mono.rs` | Binary moved to `mono-provider/src/bin/mono.rs` |

## Key Learnings

1. **synapse-cc codegen reorders parameters alphabetically.** This silently broke `claudecode.create()` and `claudecode.chat()` in the frontend. Always check generated signatures after codegen.

2. **SQLite column type affinity is not the same as column type.** `CREATE TABLE IF NOT EXISTS` with a TEXT column doesn't alter an existing table with INTEGER columns. The `CAST(x AS TEXT)` UPDATE changes values but not affinity. Must do rename → create → copy → drop.

3. **JavaScript `Number.MAX_SAFE_INTEGER` is 2^53 - 1.** Hashing UUIDs to u64 produced values that JS silently corrupted. String IDs are the correct approach for cross-language compatibility.

4. **Openverse API has no album endpoint.** Albums are reconstructed by searching by creator name and filtering by `audio_set.foreign_landing_url`. Requires a creator cache populated from search results.

5. **Tauri WebView CSP blocks images.** `img-src` must include the domains serving cover art. Using `https:` is the provider-agnostic solution.

6. **Playwright tests don't catch Tauri-specific issues.** They run in a plain browser — CSP, WebView caching, and media key integration are invisible to them.
