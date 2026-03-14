// Auto-generated typed client (Layer 2)
// Wraps RPC layer and unwraps PlexusStreamItem to domain types

import type { RpcClient } from '../rpc';
import { extractData, collectOne } from '../rpc';
import type { ChatEvent, ChatStartResult, CreateResult, DeleteResult, ForkResult, GetResult, GetTreeResult, ListResult, Model, PollResult, RenderResult, SessionsDeleteResult, SessionsExportResult, SessionsGetResult, SessionsImportResult, SessionsListResult, StreamListResult } from './types';
import type { UUID } from '../cone/types';

/** Typed client interface for claudecode plugin */
export interface ClaudecodeClient {
  /** Chat with a session, streaming tokens like Cone */
  chat(name: string, prompt: string, ephemeral?: boolean | null): AsyncGenerator<ChatEvent>;
  /** Start an async chat - returns immediately with stream_id for polling  This is the non-blocking version of chat, designed for loopback scenarios where the parent needs to poll for events and handle tool approvals. */
  chatAsync(name: string, prompt: string, ephemeral?: boolean | null): Promise<ChatStartResult>;
  /** Create a new Claude Code session */
  create(model: Model, name: string, workingDir: string, loopbackEnabled?: boolean | null, systemPrompt?: string | null): Promise<CreateResult>;
  /** Delete a session */
  delete(name: string): Promise<DeleteResult>;
  /** Fork a session to create a branch point */
  fork(name: string, newName: string): Promise<ForkResult>;
  /** Get session configuration details */
  get(name: string): Promise<GetResult>;
  /** Get arbor tree information for a session */
  getTree(name: string): Promise<GetTreeResult>;
  /** List all Claude Code sessions */
  list(): Promise<ListResult>;
  /** Poll a stream for new events  Returns events since the last poll (or from the specified offset). Use this to read events from an async chat started with chat_async. */
  poll(streamId: string, fromSeq?: number | null, limit?: number | null): Promise<PollResult>;
  /** Render arbor tree as Claude API messages */
  renderContext(name: string, end?: UUID | null, start?: UUID | null): Promise<RenderResult>;
  /** Get plugin or method schema. Pass {"method": "name"} for a specific method. */
  schema(): Promise<unknown>;
  /** Delete a session file */
  sessionsDelete(projectPath: string, sessionId: string): Promise<SessionsDeleteResult>;
  /** Export an arbor tree to a session file */
  sessionsExport(projectPath: string, sessionId: string, treeId: UUID): Promise<SessionsExportResult>;
  /** Get events from a session file */
  sessionsGet(projectPath: string, sessionId: string): Promise<SessionsGetResult>;
  /** Import a session file into arbor */
  sessionsImport(projectPath: string, sessionId: string, ownerId?: string | null): Promise<SessionsImportResult>;
  /** List all session files for a project */
  sessionsList(projectPath: string): Promise<SessionsListResult>;
  /** List active streams  Returns all active streams, optionally filtered by session. */
  streams(sessionId?: string | null): Promise<StreamListResult>;
}

/** Typed client implementation for claudecode plugin */
class ClaudecodeClientImpl implements ClaudecodeClient {
  private rpc: RpcClient;
  constructor(rpc: RpcClient) { this.rpc = rpc; }

  async *chat(name: string, prompt: string, ephemeral?: boolean | null): AsyncGenerator<ChatEvent> {
    const stream = this.rpc.call('claudecode.chat', { ephemeral, name, prompt });
    yield* extractData<ChatEvent>(stream);
  }

  async chatAsync(name: string, prompt: string, ephemeral?: boolean | null): Promise<ChatStartResult> {
    const stream = this.rpc.call('claudecode.chat_async', { ephemeral, name, prompt });
    return collectOne<ChatStartResult>(stream);
  }

  async create(model: Model, name: string, workingDir: string, loopbackEnabled?: boolean | null, systemPrompt?: string | null): Promise<CreateResult> {
    const stream = this.rpc.call('claudecode.create', { loopback_enabled: loopbackEnabled, model, name, system_prompt: systemPrompt, working_dir: workingDir });
    return collectOne<CreateResult>(stream);
  }

  async delete(name: string): Promise<DeleteResult> {
    const stream = this.rpc.call('claudecode.delete', { name });
    return collectOne<DeleteResult>(stream);
  }

  async fork(name: string, newName: string): Promise<ForkResult> {
    const stream = this.rpc.call('claudecode.fork', { name, new_name: newName });
    return collectOne<ForkResult>(stream);
  }

  async get(name: string): Promise<GetResult> {
    const stream = this.rpc.call('claudecode.get', { name });
    return collectOne<GetResult>(stream);
  }

  async getTree(name: string): Promise<GetTreeResult> {
    const stream = this.rpc.call('claudecode.get_tree', { name });
    return collectOne<GetTreeResult>(stream);
  }

  async list(): Promise<ListResult> {
    const stream = this.rpc.call('claudecode.list', {});
    return collectOne<ListResult>(stream);
  }

  async poll(streamId: string, fromSeq?: number | null, limit?: number | null): Promise<PollResult> {
    const stream = this.rpc.call('claudecode.poll', { from_seq: fromSeq, limit, stream_id: streamId });
    return collectOne<PollResult>(stream);
  }

  async renderContext(name: string, end?: UUID | null, start?: UUID | null): Promise<RenderResult> {
    const stream = this.rpc.call('claudecode.render_context', { end, name, start });
    return collectOne<RenderResult>(stream);
  }

  async schema(): Promise<unknown> {
    const stream = this.rpc.call('claudecode.schema', {});
    return collectOne<unknown>(stream);
  }

  async sessionsDelete(projectPath: string, sessionId: string): Promise<SessionsDeleteResult> {
    const stream = this.rpc.call('claudecode.sessions_delete', { project_path: projectPath, session_id: sessionId });
    return collectOne<SessionsDeleteResult>(stream);
  }

  async sessionsExport(projectPath: string, sessionId: string, treeId: UUID): Promise<SessionsExportResult> {
    const stream = this.rpc.call('claudecode.sessions_export', { project_path: projectPath, session_id: sessionId, tree_id: treeId });
    return collectOne<SessionsExportResult>(stream);
  }

  async sessionsGet(projectPath: string, sessionId: string): Promise<SessionsGetResult> {
    const stream = this.rpc.call('claudecode.sessions_get', { project_path: projectPath, session_id: sessionId });
    return collectOne<SessionsGetResult>(stream);
  }

  async sessionsImport(projectPath: string, sessionId: string, ownerId?: string | null): Promise<SessionsImportResult> {
    const stream = this.rpc.call('claudecode.sessions_import', { owner_id: ownerId, project_path: projectPath, session_id: sessionId });
    return collectOne<SessionsImportResult>(stream);
  }

  async sessionsList(projectPath: string): Promise<SessionsListResult> {
    const stream = this.rpc.call('claudecode.sessions_list', { project_path: projectPath });
    return collectOne<SessionsListResult>(stream);
  }

  async streams(sessionId?: string | null): Promise<StreamListResult> {
    const stream = this.rpc.call('claudecode.streams', { session_id: sessionId });
    return collectOne<StreamListResult>(stream);
  }
}

/** Create a typed claudecode client from an RPC client */
export function createClaudecodeClient(rpc: RpcClient): ClaudecodeClient {
  return new ClaudecodeClientImpl(rpc);
}