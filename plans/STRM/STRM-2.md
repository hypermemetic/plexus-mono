# STRM-2: Client-side generator cleanup in codegen transport

blocked_by: []
unlocks: [STRM-3]

## Scope

Add a `finally` block to the async generator in `PlexusRpcClient.call()` (generated transport template) that:
1. Sends an `${backend}.unsubscribe` JSON-RPC notification with the subscription ID when the generator exits early (before receiving `done`/`error`)
2. Deletes the subscription from the client-side `subscriptions` map

## Current Behavior

```typescript
// In call() generator — transport.rs template lines ~195-240
async *call(method, params) {
  // ... setup subscription ...
  while (true) {
    const item = await nextItem();
    if (item.type === 'done') { this.subscriptions.delete(subId); return; }
    if (item.type === 'error') { this.subscriptions.delete(subId); throw ...; }
    yield item;
  }
  // NO CLEANUP if consumer breaks out of for-await loop
}
```

When consumer does `break`, JavaScript calls `generator.return()`, which exits the generator without hitting any cleanup. The subscription stays in the map (memory leak) and server keeps streaming (resource leak).

## Target Behavior

```typescript
async *call(method, params) {
  let subId: number | undefined;
  try {
    // ... setup subscription, set subId ...
    while (true) {
      const item = await nextItem();
      if (item.type === 'done') return;
      if (item.type === 'error') throw ...;
      yield item;
    }
  } finally {
    if (subId !== undefined) {
      const sub = this.subscriptions.get(subId);
      if (sub && !sub.done && this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify({
          jsonrpc: '2.0',
          method: `${this.backend}.unsubscribe`,
          params: { subscription: subId }
        }));
      }
      this.subscriptions.delete(subId);
    }
  }
}
```

## Files

| File | Change |
|------|--------|
| `hub-codegen/src/generator/typescript/transport.rs` | Wrap generator body in `try/finally`, send unsubscribe in `finally` |

## Acceptance Criteria

- Generator cleanup sends unsubscribe notification on early exit
- Subscription map entry is always cleaned up (early exit or normal completion)
- No double-cleanup on normal `done`/`error` exit paths
- Regenerated clients work with existing servers (unsubscribe is a no-op notification until STRM-3)

## Verification

1. `cargo test` in hub-codegen
2. Regenerate mono-tray clients (`synapse-cc build`)
3. `make full` in mono-tray — all screenshot tests pass
4. Manual: open artist page with 10+ albums, navigate away quickly — no console errors, no leaked subscriptions
