# STRM-3: Server-side unsubscribe RPC method

blocked_by: [STRM-2]
unlocks: [STRM-5, STRM-6]

## Scope

Add a `${backend}.unsubscribe` RPC method to `plexus-transport` that cancels an active server-side subscription by its ID. When received, the server should drop the `tokio::task` (or signal its `CancellationToken`) that is producing the stream.

## Context

jsonrpsee v0.26 uses `register_subscription` which returns a `SubscriptionSink`. When the sink is dropped, the server stops sending. But the underlying stream producer (the activation's async generator) keeps running unless explicitly cancelled.

The RPC module needs:
1. A method that accepts `{ subscription: number }`
2. Looks up the subscription's cancellation handle
3. Signals cancellation so the stream-producing task exits

## Implementation Notes

- jsonrpsee subscriptions are identified by `SubscriptionId` — we need to maintain a `HashMap<SubscriptionId, CancellationToken>` in the server state
- When `${backend}.call` creates a subscription, it stores a `CancellationToken` in this map
- When `${backend}.unsubscribe` is called, it triggers the token and removes the entry
- The stream-producing task should `select!` on the cancellation token alongside its normal work

## Files

| File | Change |
|------|--------|
| `plexus-transport/src/websocket.rs` | Add unsubscribe method, subscription tracking map |
| `plexus-transport/src/server.rs` | Wire cancellation tokens into RPC module registration |

## Acceptance Criteria

- `${backend}.unsubscribe` RPC method exists and is callable
- Calling it with a valid subscription ID stops the server-side stream producer
- Calling it with an unknown subscription ID is a no-op (not an error)
- Normal stream completion still works (done/error paths unaffected)
- No race conditions between natural completion and explicit cancellation

## Dependencies

- Requires STRM-4 for full effect (activation code must respect cancellation tokens)
- Without STRM-4, unsubscribe drops the sink but the activation's stream task may keep running until it tries to send and finds the sink closed

## Verification

1. `cargo test` in plexus-transport
2. Integration test: start a long-running streaming call, send unsubscribe, verify server-side task exits
3. `make full` in mono-tray — all tests pass
