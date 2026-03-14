// Auto-generated RPC client interface
// This is Layer 1: raw RPC that returns PlexusStreamItem

import type { PlexusStreamItem } from './types';
import { PlexusError } from './types';

/**
 * Raw RPC client interface for hub communication.
 *
 * This is the low-level transport layer. All methods return AsyncGenerator<PlexusStreamItem>.
 * Use the typed client wrappers for a better developer experience.
 */
export interface RpcClient {
  /**
   * Call a method and receive a stream of PlexusStreamItem responses.
   *
   * @param method - Fully qualified method name (e.g., "echo.once", "cone.chat")
   * @param params - Method parameters as a JSON-serializable object
   * @returns AsyncGenerator yielding PlexusStreamItem events
   */
  call(method: string, params?: unknown): AsyncGenerator<PlexusStreamItem>;
}

/**
 * Convert snake_case string to camelCase.
 * Used to transform backend field names to TypeScript conventions.
 */
function toCamelCase(str: string): string {
  return str.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase());
}

/**
 * Recursively transform all object keys from snake_case to camelCase.
 * This allows TypeScript types to use idiomatic camelCase while the backend sends snake_case.
 * Similar to Rust's #[serde(rename_all = "camelCase")].
 */
function transformKeys(obj: unknown): unknown {
  if (obj === null || obj === undefined) return obj;
  if (typeof obj !== 'object') return obj;
  if (Array.isArray(obj)) return obj.map(transformKeys);

  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(obj)) {
    const camelKey = toCamelCase(key);
    result[camelKey] = transformKeys(value);
  }
  return result;
}

/**
 * Helper to extract data content from a PlexusStreamItem stream.
 * Throws PlexusError on error events.
 * Automatically transforms response field names from snake_case to camelCase.
 *
 * @param stream - AsyncGenerator of PlexusStreamItem
 * @returns AsyncGenerator of the unwrapped content (typed as T)
 */
export async function* extractData<T>(
  stream: AsyncGenerator<PlexusStreamItem>
): AsyncGenerator<T> {
  for await (const item of stream) {
    switch (item.type) {
      case 'data':
        yield transformKeys(item.content) as T;
        break;
      case 'error':
        throw new PlexusError(
          item.message,
          item.code,
          item.recoverable,
          item.metadata
        );
      case 'progress':
        // Progress events are informational, skip
        break;
      case 'done':
        // Stream completed
        return;
    }
  }
}

/**
 * Helper to collect a single result from a non-streaming method.
 * Throws PlexusError on error events.
 * Throws if no data is received.
 * Automatically transforms response field names from snake_case to camelCase.
 *
 * @param stream - AsyncGenerator of PlexusStreamItem
 * @returns Promise of the unwrapped content (typed as T)
 */
export async function collectOne<T>(
  stream: AsyncGenerator<PlexusStreamItem>
): Promise<T> {
  for await (const item of stream) {
    switch (item.type) {
      case 'data':
        return transformKeys(item.content) as T;
      case 'error':
        throw new PlexusError(
          item.message,
          item.code,
          item.recoverable,
          item.metadata
        );
      case 'progress':
        // Progress events are informational, skip
        break;
      case 'done':
        break;
    }
  }
  throw new Error('No data received from method call');
}

// Re-export PlexusError for convenience
export { PlexusError } from './types';
