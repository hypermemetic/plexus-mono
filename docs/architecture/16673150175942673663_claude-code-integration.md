# Claude Code Integration

How Mono Tray integrates Claude Code for AI-powered playlist research.

## Two-Backend Architecture

Mono Tray connects to two independent Plexus RPC servers over WebSocket:

```
┌──────────────┐     ws://127.0.0.1:4448      ┌──────────────┐
│              │ ◄──── JSON-RPC (music) ─────► │  plexus-mono │ ──► Tidal API
│   Mono Tray  │                               │  (Rust)      │ ──► SQLite
│   (Tauri)    │     ws://127.0.0.1:4444       ├──────────────┤
│              │ ◄──── JSON-RPC (AI) ────────► │  Substrate   │ ──► claude CLI
└──────────────┘                               │  (Rust)      │ ──► Arbor trees
                                               └──────────────┘
```

Music operations and AI research run on separate servers so neither blocks the other. Both use the same Plexus streaming protocol — the TypeScript clients are auto-generated from each server's live IR schema via `synapse-cc`.

### Client Setup (app.ts)

```typescript
// Music server
const rpc = new PlexusRpcClient({ backend: 'monochrome', url: 'ws://127.0.0.1:4448' });
const player = createPlayerClient(rpc);
const mono = createMonoClient(rpc);

// AI server
const substrateRpc = new SubstratePlexusRpcClient({ backend: 'substrate', url: 'ws://127.0.0.1:4444' });
const claudecode = createClaudecodeClient(substrateRpc);
```

## Claude Code Activation (Substrate Side)

The Substrate server hosts a `claudecode` activation that wraps the `claude` CLI binary.

**Location**: `plexus-substrate/src/activations/claudecode/`

### Key Files

| File | Purpose |
|------|---------|
| `activation.rs` | Hub methods: `create`, `chat`, `list`, `poll`, `renderContext` |
| `executor.rs` | Spawns `claude` CLI in JSON-RPC stdio mode, parses event stream |
| `storage.rs` | SQLite persistence for sessions and messages |
| `sessions.rs` | Arbor tree integration for conversation history |
| `types.rs` | `ChatEvent`, `ClaudeCodeHandle`, `ClaudeCodeInfo` |

### How `chat` Works

1. Load session by name from SQLite
2. Store user message (optionally ephemeral — deleted after response)
3. Create Arbor node for the message
4. Emit `ChatEvent::Start` with session ID and user position
5. Build `LaunchConfig` with query, model, working_dir, allowed_tools
6. Spawn `claude` CLI subprocess
7. Parse line-delimited JSON events from stdout
8. For each raw event, emit typed `ChatEvent`:
   - `Content { text }` — streamed text chunks
   - `Thinking { thinking }` — reasoning content
   - `ToolUse { tool_name, tool_use_id, input }` — tool invocation
   - `ToolResult { tool_use_id, output, is_error }` — tool output
   - `Complete { new_head, usage }` — conversation done

### Claude CLI Discovery

The executor searches for `claude` at:
- `~/.claude/local/claude`
- `~/.npm/bin/claude`, `~/.bun/bin/claude`, `~/.local/bin/claude`
- `/usr/local/bin/claude`, `/opt/homebrew/bin/claude`
- `PATH`

## Research Feature: End-to-End Flow

The sparkle button (or double-enter in search) triggers a three-phase AI research pipeline.

### Phase 1: Theme Research (Claude + WebSearch)

```
User enters "upbeat indie pop" → researchPlaylist("upbeat indie pop")
                                         │
                                         ▼
                              ensureClaudeSession()
                  claudecode.create('sonnet', 'mono-tray-research', ...)
                                         │
                                         ▼
                    askClaude(researchPrompt, ['WebSearch'])
                                         │
                                         ▼
              Claude uses WebSearch to find artists/albums/tracks
              Returns: {"searches": ["Arctic Monkeys", "Vampire Weekend", ...]}
```

A dedicated session named `mono-tray-research` is created once and reused. The system prompt tells Claude it's a music researcher. `allowed_tools: ['WebSearch']` restricts Claude to only web search — no file access or code execution.

### Phase 2: Catalog Search (Parallel Tidal Queries)

```
searchSuggestions = ["Arctic Monkeys", "Vampire Weekend", ...]
         │
         ▼
  Promise.all(suggestions.map(async term => {
    mono.search(term, 'tracks', 10)    // track search
    mono.search(term, 'albums', 5)     // album search → drill into tracks
  }))
         │
         ▼
  Deduplicate by track ID → allTracks[]
```

All search suggestions run in parallel against the Tidal catalog. Album results are expanded — the app searches for tracks within each found album. Results are deduplicated by track ID.

If AI research returned nothing, falls back to a direct `mono.search(query, 'tracks', 50)`.

### Phase 3: Curation (Claude Picks + Orders)

```
askClaude(curatePrompt)  // no tools needed, just reasoning
         │
         ▼
  Claude picks best tracks matching the theme
  Orders them into a thematic arc
  Explains each choice
         │
         ▼
  Returns: {
    "name": "Upbeat Indie Mix",
    "tracks": [
      {"id": 123, "title": "...", "artist": "...", "reason": "Perfect opener"},
      ...
    ]
  }
```

Retry logic: if Claude returns invalid JSON, it retries up to 3 times with a corrective prompt.

### Save & Display

```
playlist.researchSave(result.name, {
  query,
  searches: searchSuggestions,
  allFoundTracks,
  curatedResult: result,
  timestamp: Date.now()
})
         │
         ▼
  ~/.plexus/monochrome/player/research/{name}.json
```

The research view shows curated tracks with per-track reasoning. User can:
- **Play** individual tracks
- **Add All to Queue** to listen immediately
- **Done** to save as a persistent playlist

## Streaming Event Protocol

All Claude interactions use `AsyncGenerator<ChatEvent>` — events stream to the UI in real time.

### ChatEvent Types

```typescript
type ChatEvent =
  | { type: 'start'; id: string; userPosition: Position }
  | { type: 'content'; text: string }
  | { type: 'thinking'; thinking: string }
  | { type: 'tool_use'; toolName: string; toolUseId: string; input: unknown }
  | { type: 'tool_result'; toolUseId: string; output: string; isError: boolean }
  | { type: 'complete'; claudeSessionId: string; newHead: Position; usage: ChatUsage | null }
  | { type: 'error'; message: string }
```

These are wrapped in `PlexusStreamItem` at the transport layer:

```typescript
type PlexusStreamItem =
  | { type: 'data'; contentType: string; content: ChatEvent; metadata: StreamMetadata }
  | { type: 'progress'; message: string; percentage?: number }
  | { type: 'error'; message: string; recoverable: boolean }
  | { type: 'done' }
```

The generated client's `extractData<ChatEvent>()` helper unwraps data items and throws on errors.

## Arbor Tree Integration

Conversation history is stored as an Arbor tree in the Substrate activation:

```
Tree: mono-tray-research
  ├── Node: user message ("Research upbeat indie pop...")
  │   ├── Node: content text ("Here are my suggestions...")
  │   ├── Node: tool_use (WebSearch, {"query": "indie pop artists 2024"})
  │   ├── Node: tool_result ("Found: Arctic Monkeys, ...")
  │   └── Node: content text ("Based on my research...")
  └── Node: user message ("Pick the best tracks...")
      └── Node: content text ('{"name": "Upbeat Indie Mix", ...}')
```

Key concepts:
- **Head** — points to the latest conversation leaf (advanced on each turn)
- **External nodes** — reference messages stored in SQLite via handles
- **Ephemeral nodes** — research turns that don't advance the head (cleaned up after)

## Tool Control

The `allowed_tools` parameter restricts what Claude can do per request:

```typescript
// Phase 1: web research — only WebSearch
askClaude(researchPrompt, ['WebSearch']);

// Phase 3: curation — no tools (pure reasoning)
askClaude(curatePrompt);  // allowed_tools omitted
```

This prevents Claude from accessing the filesystem, running code, or using other MCP tools during music research.

## UI Status Updates

The research flow provides real-time status to the user:

| Status | When |
|--------|------|
| "Researching theme..." | Phase 1 starts |
| "Searching the web..." | Claude uses WebSearch tool |
| "Searching library..." | Phase 2 parallel Tidal queries |
| "Curating..." | Phase 3 Claude picks tracks |
| Notification | Research complete |

Tool use callbacks drive the status transitions:

```typescript
const onToolUse = (toolName: string) => {
  if (toolName === 'WebSearch') {
    researchStatus.textContent = 'Searching the web...';
  }
};
```
