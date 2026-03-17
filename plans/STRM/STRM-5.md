# STRM-5: Subscription timeout for leaked subscriptions

blocked_by: [STRM-3, STRM-4]
unlocks: [STRM-6]

## Scope

Add a configurable timeout to server-side subscriptions. If a subscription has not been explicitly cancelled (STRM-3) and has not completed naturally within the timeout window, the server auto-cancels it.

This is a safety net for cases where:
- Client disconnects uncleanly (no unsubscribe sent)
- Client bug fails to consume or cancel a stream
- Network partition leaves subscription orphaned

## Design

- Default timeout: 5 minutes (configurable via `PlexusServerConfig`)
- Timeout resets on each item sent (active streams don't timeout)
- A background `tokio::spawn` task sweeps the subscription map periodically (every 30s)
- Expired subscriptions are cancelled via the same mechanism as STRM-3/STRM-4

## Files

| File | Change |
|------|--------|
| `plexus-transport/src/config.rs` | Add `subscription_timeout: Duration` field |
| `plexus-transport/src/server.rs` | Background sweeper task, last-activity tracking |

## Acceptance Criteria

- Abandoned subscriptions are cleaned up within timeout + sweep interval
- Active streams (sending items regularly) never timeout
- Timeout is configurable, with a sensible default
- Log warning when a subscription is auto-cancelled (helps diagnose leaks)

## Verification

1. Integration test: create subscription, don't consume, wait for timeout, verify cleanup
2. Integration test: active streaming subscription does NOT timeout
