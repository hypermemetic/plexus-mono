# STRM-4: Activation cancellation token wiring

blocked_by: [STRM-3]
unlocks: [STRM-5]

## Scope

Wire `CancellationToken` (or `tokio::sync::watch`) through the plexus-core activation layer so that streaming activation methods can detect when their subscription has been cancelled and exit early.

## Context

Currently, activation methods return `impl Stream<Item = Event>`. These streams are produced by `async_stream::stream!` blocks that run in spawned tasks. Even if the subscription sink is dropped (STRM-3), the stream-producing code keeps running — it just sends items into a closed channel.

The activation needs a way to check "should I stop?" inside its stream body.

## Design Options

### Option A: CancellationToken parameter
Add `CancellationToken` as a parameter to the generated activation dispatch code. The `stream!` macro would `select!` on the token:

```rust
stream! {
    tokio::select! {
        _ = cancel_token.cancelled() => {},
        _ = async {
            // existing stream body
            for item in items {
                yield item;
            }
        } => {}
    }
}
```

**Pro**: Explicit, works with existing stream patterns.
**Con**: Requires plexus-macros changes to inject the token.

### Option B: Check sink closed
Instead of a token, check if the sink is still accepting items after each yield:

```rust
stream! {
    for item in items {
        yield item;
        // generated wrapper checks sink.is_closed() after each yield
    }
}
```

**Pro**: No new parameter, activation code unchanged.
**Con**: Requires wrapping the stream in a filter/take_while, adds latency (only checks between items).

### Recommendation

Option B is simpler and doesn't require macro changes. The stream wrapper in plexus-transport can wrap any activation stream with a `take_while(!sink.is_closed())` check.

## Files

| File | Change |
|------|--------|
| `plexus-transport/src/server.rs` | Wrap activation streams with sink-closed check |
| `plexus-core/src/plexus/streaming.rs` | (Optional) Add cancellation-aware stream utilities |

## Acceptance Criteria

- Activation streams stop producing items after their subscription is cancelled
- No changes required to existing activation code (backward compatible)
- Spawned tasks exit promptly (within one poll cycle) after cancellation

## Verification

1. `cargo test` in plexus-core and plexus-transport
2. Integration test: streaming method with 100+ items, cancel at item 5, verify task exits before item 10
