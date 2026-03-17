# STRM-1: Streaming Transport Cleanup & Subscription Lifecycle

## Goal

Fix resource leaks in the Plexus RPC streaming transport. When a client abandons a streaming generator (e.g., `break` in a `for await` loop), the server-side stream continues running to completion with no way to cancel it. This causes:

- Server CPU/memory waste on abandoned streams
- N+1 RPC floods when many short-lived streams fire concurrently (e.g., album cover loading)
- Potential connection saturation under load

## Dependency DAG

```
STRM-2 (client-side cleanup)
    ↓
STRM-3 (server-side unsubscribe)  ←  STRM-4 (activation cancellation wiring)
    ↓
STRM-5 (subscription timeout)
    ↓
STRM-6 (metrics & observability)
```

STRM-2 is independently shippable and fixes the immediate symptom.
STRM-3 + STRM-4 are the full fix — server stops work on cancelled streams.
STRM-5 and STRM-6 are hardening.

## Phase Breakdown

### Phase 1: Client cleanup (STRM-2)
Ship a `finally` block in the generated transport that sends an unsubscribe notification when a generator exits early. No server changes needed — the message is a no-op until STRM-3 lands, but the client-side subscription map is properly cleaned up.

### Phase 2: Server cancellation (STRM-3, STRM-4)
Add an `unsubscribe` RPC method to plexus-transport and wire cancellation tokens through plexus-core activations so server-side streams are actually dropped when cancelled.

### Phase 3: Hardening (STRM-5, STRM-6)
Subscription timeouts for leaked subscriptions, and metrics to observe subscription lifecycle health.

## Repos Touched

| Repo | Tickets |
|------|---------|
| `hub-codegen` | STRM-2 |
| `plexus-transport` | STRM-3, STRM-5 |
| `plexus-core` | STRM-4 |
| `plexus-mono` (consumer) | STRM-6 |
