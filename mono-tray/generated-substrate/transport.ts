// Plexus WebSocket transport
// Depends on ./types (protocol types) and ./rpc (RpcClient interface + helpers).
import type {
  PlexusStreamItem,
  PlexusStreamItemRequest,
  StandardRequest,
  StandardResponse,
} from './types';
import type { RpcClient } from './rpc';

// ─── WebSocket transport ───────────────────────────────────────────────────

export interface PlexusRpcConfig {
  backend: string;
  url: string;
  connectionTimeout?: number;
  debug?: boolean;
  onBidirectionalRequest?: BidirectionalRequestHandler;
}

export type BidirectionalRequestHandler = (
  request: StandardRequest
) => Promise<StandardResponse | undefined>;

interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number;
  method: string;
  params?: unknown;
}

interface JsonRpcSuccess { jsonrpc: '2.0'; id: number; result: unknown; }
interface JsonRpcError   { jsonrpc: '2.0'; id: number; error: { code: number; message: string; data?: unknown }; }
type JsonRpcResponse = JsonRpcSuccess | JsonRpcError;

interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: 'subscription';
  params: { subscription: number; result: PlexusStreamItem };
}

interface PendingRequest {
  resolve: (subscriptionId: number) => void;
  reject: (error: Error) => void;
}

interface ActiveSubscription {
  queue: PlexusStreamItem[];
  waiting: ((item: PlexusStreamItem | null) => void) | null;
  done: boolean;
}

export class PlexusRpcClient implements RpcClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pendingRequests = new Map<number, PendingRequest>();
  private subscriptions = new Map<number, ActiveSubscription>();
  private pendingSubscriptionMessages = new Map<number, PlexusStreamItem[]>();
  private config: Omit<Required<PlexusRpcConfig>, 'onBidirectionalRequest'>;
  private connectionPromise: Promise<void> | null = null;
  private onBidirectionalRequest?: BidirectionalRequestHandler;

  constructor(config: PlexusRpcConfig) {
    this.config = {
      backend: config.backend,
      url: config.url,
      connectionTimeout: config.connectionTimeout ?? 5000,
      debug: config.debug ?? false,
    };
    this.onBidirectionalRequest = config.onBidirectionalRequest;
  }

  setBidirectionalHandler(handler: BidirectionalRequestHandler | undefined): void {
    this.onBidirectionalRequest = handler;
  }

  private log(...args: unknown[]): void {
    if (this.config.debug) console.log('[PlexusRpcClient]', ...args);
  }

  async connect(): Promise<void> {
    if (this.ws?.readyState === WebSocket.OPEN) return;
    if (this.connectionPromise) return this.connectionPromise;

    this.connectionPromise = new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error(`Connection timeout after ${this.config.connectionTimeout}ms`));
      }, this.config.connectionTimeout);

      this.ws = new WebSocket(this.config.url);
      this.ws.onopen  = () => { clearTimeout(timeout); this.log('Connected to', this.config.url); resolve(); };
      this.ws.onerror = (event) => { clearTimeout(timeout); this.log('WebSocket error:', event); reject(new Error('WebSocket connection failed')); };
      this.ws.onclose = (event) => { this.log('WebSocket closed:', event.code, event.reason); this.handleDisconnect(); };
      this.ws.onmessage = (event) => { this.handleMessage(event.data.toString()); };
    });

    try { await this.connectionPromise; } finally { this.connectionPromise = null; }
  }

  disconnect(): void {
    if (this.ws) { this.ws.close(1000, 'Client disconnect'); this.ws = null; }
    this.handleDisconnect();
  }

  private handleDisconnect(): void {
    for (const [id, pending] of this.pendingRequests) { pending.reject(new Error('Connection closed')); this.pendingRequests.delete(id); }
    for (const [id, sub] of this.subscriptions) { sub.done = true; if (sub.waiting) { sub.waiting(null); sub.waiting = null; } this.subscriptions.delete(id); }
  }

  private handleMessage(data: string): void {
    this.log('Received:', data);
    let msg: unknown;
    try { msg = JSON.parse(data); } catch { this.log('Failed to parse message:', data); return; }
    const obj = msg as Record<string, unknown>;
    if ('method' in obj && !('id' in obj) && obj.params && typeof (obj.params as any).subscription !== 'undefined') {
      this.handleNotification(msg as JsonRpcNotification); return;
    }
    if ('id' in obj) { this.handleResponse(msg as JsonRpcResponse); return; }
    this.log('Unknown message format:', msg);
  }

  private handleResponse(resp: JsonRpcResponse): void {
    const pending = this.pendingRequests.get(resp.id);
    if (!pending) { this.log('Unknown request id:', resp.id); return; }
    this.pendingRequests.delete(resp.id);
    if ('error' in resp) { pending.reject(new Error(`RPC error ${resp.error.code}: ${resp.error.message}`)); }
    else { pending.resolve(resp.result as number); }
  }

  private handleNotification(notif: JsonRpcNotification): void {
    const subscriptionId = notif.params.subscription;
    const item = notif.params.result;
    let sub = this.subscriptions.get(subscriptionId);
    if (!sub) {
      if (!this.pendingSubscriptionMessages.has(subscriptionId)) this.pendingSubscriptionMessages.set(subscriptionId, []);
      this.pendingSubscriptionMessages.get(subscriptionId)!.push(item);
      return;
    }
    if (item.type === 'request') { this.handleBidirectionalRequest(item as PlexusStreamItemRequest); return; }
    if (item.type === 'done' || item.type === 'error') sub.done = true;
    if (sub.waiting) { const w = sub.waiting; sub.waiting = null; w(item); }
    else { sub.queue.push(item); }
    if (sub.done && sub.queue.length === 0) this.subscriptions.delete(subscriptionId);
  }

  private async handleBidirectionalRequest(requestItem: PlexusStreamItemRequest): Promise<void> {
    const { requestId, requestData, timeoutMs } = requestItem;
    if (!this.onBidirectionalRequest) {
      this.log('No bidirectional handler, auto-cancelling:', requestId);
      await this.sendBidirectionalResponse(requestId, { type: 'cancelled' }); return;
    }
    const timeoutPromise = new Promise<undefined>(resolve => setTimeout(() => resolve(undefined), timeoutMs));
    try {
      const response = await Promise.race([this.onBidirectionalRequest(requestData), timeoutPromise]);
      await this.sendBidirectionalResponse(requestId, response ?? { type: 'cancelled' });
    } catch (err) {
      this.log('Bidirectional handler error:', err);
      await this.sendBidirectionalResponse(requestId, { type: 'cancelled' });
    }
  }

  private async sendBidirectionalResponse(requestId: string, response: StandardResponse): Promise<void> {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) { this.log('Cannot send response, not connected'); return; }
    const id = this.nextId++;
    this.ws.send(JSON.stringify({ jsonrpc: '2.0', id, method: `${this.config.backend}.respond`, params: { request_id: requestId, response_data: response } }));
  }

  async *call(method: string, params?: unknown): AsyncGenerator<PlexusStreamItem> {
    await this.connect();
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) throw new Error('Not connected');

    const sub: ActiveSubscription = { queue: [], waiting: null, done: false };
    const id = this.nextId++;
    const request: JsonRpcRequest = {
      jsonrpc: '2.0', id,
      method: `${this.config.backend}.call`,
      params: { method, params: params ?? {} },
    };
    this.log('Sending:', JSON.stringify(request));

    const subscriptionIdPromise = new Promise<number>((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
    });
    this.ws.send(JSON.stringify(request));

    const subscriptionId = await subscriptionIdPromise;
    this.log('Got subscription ID:', subscriptionId);
    this.subscriptions.set(subscriptionId, sub);

    const pendingMessages = this.pendingSubscriptionMessages.get(subscriptionId);
    if (pendingMessages) {
      this.pendingSubscriptionMessages.delete(subscriptionId);
      for (const msg of pendingMessages) {
        if (msg.type === 'done' || msg.type === 'error') sub.done = true;
        sub.queue.push(msg);
      }
    }

    try {
      while (true) {
        if (sub.queue.length > 0) {
          const item = sub.queue.shift()!;
          yield item;
          if (item.type === 'done' || item.type === 'error') return;
          continue;
        }
        if (sub.done) return;
        const item = await new Promise<PlexusStreamItem | null>(resolve => { sub.waiting = resolve; });
        if (item === null) return;
        yield item;
        if (item.type === 'done' || item.type === 'error') return;
      }
    } finally {
      this.subscriptions.delete(subscriptionId);
    }
  }
}

export function createClient(config: PlexusRpcConfig): PlexusRpcClient {
  return new PlexusRpcClient(config);
}