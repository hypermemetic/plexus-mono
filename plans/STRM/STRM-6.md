# STRM-6: Subscription lifecycle metrics & observability

blocked_by: [STRM-5]
unlocks: []

## Scope

Add metrics and logging to observe subscription lifecycle health:

- Total subscriptions created / completed / cancelled / timed-out
- Currently active subscription count
- Average subscription duration
- Subscription leak detection (long-running subscriptions with no recent activity)

## Design

Expose metrics via the existing `_info` or a new `_metrics` RPC method on each backend, so Synapse can query them:

```bash
synapse substrate _self metrics
# or
synapse music _metrics
```

Also emit structured tracing events at key lifecycle points:
- `tracing::debug!("subscription created", id, method)`
- `tracing::debug!("subscription completed", id, method, duration_ms)`
- `tracing::warn!("subscription timeout", id, method, age_ms)`

## Files

| File | Change |
|------|--------|
| `plexus-transport/src/server.rs` | Metric counters, `_metrics` RPC method |
| `plexus-transport/src/config.rs` | Optional metrics enable flag |

## Acceptance Criteria

- Subscription counts queryable via RPC
- Tracing events emitted at subscription lifecycle boundaries
- No performance impact when metrics are not queried

## Verification

1. Start backend, create/cancel several subscriptions
2. Query `_metrics` — verify counts match
3. Check log output for structured subscription events
