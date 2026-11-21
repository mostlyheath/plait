// A Claude Code thread's translated transcript: SDK messages in, the same
// append-only entries and live document a Codex or Pi thread produces out, so
// `update()` in ../transcript.ts and the history pages read it unchanged.
import type { SDKMessage, PermissionMode } from '@anthropic-ai/claude-agent-sdk';
import type { Message, Part } from '../transcript.ts';
import type { Usage } from '../protocol.ts';
import type { Applied, Entry } from '../external/entries.ts';
import { clipHead, clipTail } from '../text.ts';
import { mcpName } from './approval.ts';

export type TurnEnd = { status: 'completed' | 'interrupted' | 'failed'; error?: string; queued: number };

const VIEW_ENTRIES = 100;
/** Live buffers keep their tail; the snapshot shows at most 16 KiB of each. */
const LIVE_LIMIT = 65536;
/** A persisted tool result keeps its tail, as the end of command output matters most. */
const RESULT_LIMIT = 262144;
/** Each string in persisted tool arguments; a whole written file does not belong in the call row. */
const ARGUMENT_LIMIT = 8192;
const ERROR_LIMIT = 1024;
const UNFINISHED = 'The turn ended before this finished.';
const ABORTED = new Set(['aborted_streaming', 'aborted_tools']);

// The SDK's message types are wide unions; the transcript reads the few fields it needs defensively.
type Loose = Record<string, any>;
type Sent = { requestId: string; text: string };

const isObject = (value: unknown): value is Loose => typeof value === 'object' && value !== null;
const text = (value: unknown) => typeof value === 'string' ? value : '';
function bounded(output: string) {
  return output.length > RESULT_LIMIT ? `[Earlier output omitted]\n${clipTail(output, RESULT_LIMIT)}` : output;
}
const clipArgument = (value: unknown) => typeof value === 'string' && value.length > ARGUMENT_LIMIT ? `${clipHead(value, ARGUMENT_LIMIT)}…` : value;

/** How a turn ended, from its `result` message. `error` is Claude Code's text and may quote the provider. */
export function isTurnEnd(input: SDKMessage | Loose): TurnEnd | undefined {
  const message = input as Loose;
  if (!isObject(message) || message.type !== 'result') return undefined;
  const queued = typeof message.queued_turn_count === 'number' && message.queued_turn_count > 0 ? message.queued_turn_count : 0;
  if (ABORTED.has(message.terminal_reason)) return { status: 'interrupted', queued };
  if (message.subtype === 'success' && !message.is_error) return { status: 'completed', queued };
  const error = message.subtype === 'success' ? text(message.result)
    : (Array.isArray(message.errors) ? message.errors.filter((item: unknown) => typeof item === 'string').join('; ') : '') || String(message.subtype ?? '');
  return { status: 'failed', queued, ...(error ? { error: clipHead(error, ERROR_LIMIT) } : {}) };
}

/** The row name and arguments the desktop knows: `command` reads `command`, the others read `path`. */
export function toolCall(name: string, input: Loose): { name: string; arguments: unknown } {
  const path = text(input.file_path) || text(input.notebook_path) || text(input.path);
  switch (name) {
    case 'Bash': return { name: 'command', arguments: { command: clipArgument(text(input.command)), ...(input.description ? { description: clipArgument(input.description) } : {}) } };
    case 'Edit': case 'MultiEdit': case 'NotebookEdit': return { name: 'edit', arguments: { path, tool: name } };
    case 'Write': return { name: 'write', arguments: { path, bytes: Buffer.byteLength(text(input.content)) } };
    case 'Read': return { name: 'read', arguments: { path, ...(input.offset !== undefined ? { offset: input.offset } : {}), ...(input.limit !== undefined ? { limit: input.limit } : {}) } };
    case 'WebFetch': return { name: 'web_fetch', arguments: { url: clipArgument(text(input.url)) } };
    case 'WebSearch': return { name: 'web_search', arguments: { query: clipArgument(text(input.query)) } };
  }
  const mcp = mcpName(name);
  const args: Loose = {};
  for (const [key, value] of Object.entries(input).slice(0, 64)) args[key] = clipArgument(value);
  return { name: mcp ? `mcp:${mcp.server}/${mcp.tool}` : name, arguments: args };
}

function resultText(content: unknown): string {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  const parts: string[] = [];
  for (const block of content) {
    if (block?.type === 'text' && typeof block.text === 'string') parts.push(block.text);
    else if (block?.type === 'image') parts.push('[image]');
  }
  return parts.join('\n');
}

/**
 * Turns one Claude Code session's SDK messages into append-only transcript
 * entries and live state. Complete assistant messages and tool results
 * persist; stream events only change the live document. Subagent frames
 * (`parent_tool_use_id` set) are left out; the Agent call and its result show.
 */
export class ClaudeTranscript {
  private entries: Entry[];
  private nextId: number;
  private readonly called = new Set<string>();
  private readonly answered = new Set<string>();
  /** Tool calls persisted and still without a result; a turn's end closes them. */
  private readonly open = new Set<string>();
  /** Messages sent and not yet placed, by SDK uuid. */
  private readonly pending = new Map<string, Sent>();
  private session: string | null = null;
  private mode: PermissionMode | null = null;
  private capabilities: readonly string[] = [];
  private tokens: { input: number; output: number; cacheRead: number; cacheWrite: number; model: string | null } | null = null;
  private window: number | null = null;
  /** Completed thinking, joined to the next persisted assistant message as Pi stores it. */
  private thinking: string[] = [];
  // Live state from stream events, cleared as complete messages arrive.
  private liveText = '';
  private liveThinking = '';

  constructor(entries: Entry[], nextId: number) {
    this.entries = entries.slice(-VIEW_ENTRIES);
    this.nextId = nextId;
    for (const entry of entries) for (const message of entry.model) {
      if (message.role === 'toolResult' && message.toolCallId) this.answered.add(message.toolCallId);
      if (Array.isArray(message.content)) for (const part of message.content) if (part.type === 'toolCall' && part.id) this.called.add(part.id);
    }
  }

  /**
   * Registers a message the runtime sent, so its echo places it. Call it with
   * the uuid `ClaudeSession.send` returns, before the next `apply`.
   */
  sent(uuid: string, requestId: string, input: string) {
    this.pending.set(uuid, { requestId, text: input });
  }

  /** Requests sent and not placed yet, oldest first. */
  unplaced(): string[] {
    return [...this.pending.values()].map(sent => sent.requestId);
  }

  /** Forgets sends Claude Code dropped (an interrupt's `cancelled` list), returning their requests. */
  drop(uuids: readonly string[]): string[] {
    const dropped: string[] = [];
    for (const uuid of uuids) {
      const sent = this.pending.get(uuid);
      if (sent) { dropped.push(sent.requestId); this.pending.delete(uuid); }
    }
    return dropped;
  }

  /**
   * Ends the turn without a `result`, when Claude Code exited or the run gave
   * up waiting: open tools are closed and live text cleared, as a result would.
   */
  finish(): Applied {
    const result: Applied = { append: [], changed: false, placed: [] };
    const before = this.nextId;
    result.changed = this.turnEnded({}, result);
    if (this.nextId !== before) { result.append = this.entries.filter(entry => entry.id >= before); result.changed = true; }
    return result;
  }

  sessionId(): string | null { return this.session; }
  /** The mode Claude Code last reported, which may differ from the one requested. */
  permissionMode(): PermissionMode | null { return this.mode; }
  capabilityList(): readonly string[] { return this.capabilities; }

  apply(message: SDKMessage): Applied {
    const result: Applied = { append: [], changed: false, placed: [] };
    if (!isObject(message)) return result;
    const before = this.nextId;
    if (typeof message.session_id === 'string' && message.session_id) this.session = message.session_id;
    result.changed = this.route(message as Loose, result);
    if (this.nextId !== before) {
      result.append = this.entries.filter(entry => entry.id >= before);
      result.changed = true;
    }
    return result;
  }

  view(): { entries: Entry[]; docs: Record<string, unknown> } {
    const live = this.live();
    return { entries: this.entries.slice(-VIEW_ENTRIES), docs: live ? { 'pi.live': live } : {} };
  }

  /** The latest model response's usage, as Pi reports the latest response's. */
  usage(contextWindow: number | null): Usage | null {
    const tokens = this.tokens;
    if (!tokens) return null;
    const total = tokens.input + tokens.output + tokens.cacheRead + tokens.cacheWrite;
    return {
      input: tokens.input, output: tokens.output, cache_read: tokens.cacheRead, cache_write: tokens.cacheWrite, reasoning: null,
      total, context_used: total, context_window: this.window ?? contextWindow,
    };
  }

  private route(message: Loose, result: Applied): boolean {
    switch (message.type) {
      case 'system': return this.system(message);
      case 'stream_event': return message.parent_tool_use_id == null && this.stream(message, result);
      case 'assistant': return message.parent_tool_use_id == null && this.assistant(message, result);
      case 'user': return message.parent_tool_use_id == null && this.user(message, result);
      case 'result': return this.turnEnded(message, result);
      default: return false;
    }
  }

  private system(message: Loose): boolean {
    if (message.subtype === 'init') {
      if (typeof message.permissionMode === 'string') this.mode = message.permissionMode as PermissionMode;
      if (Array.isArray(message.capabilities)) this.capabilities = message.capabilities.filter((item: unknown) => typeof item === 'string').slice(0, 256);
      return false;
    }
    if (message.subtype === 'status') {
      if (typeof message.permissionMode === 'string') this.mode = message.permissionMode as PermissionMode;
      return false;
    }
    if (message.subtype === 'local_command_output' && typeof message.content === 'string' && message.content) {
      // A local slash command's output is shown as Claude Code's reply.
      this.push([{ role: 'assistant', content: [{ type: 'text', text: bounded(message.content) }], stopReason: 'stop' }]);
      return true;
    }
    return false;
  }

  private push(model: Message[]): Entry {
    const entry: Entry = { id: this.nextId++, kind: 'message', model };
    this.entries.push(entry);
    if (this.entries.length > VIEW_ENTRIES * 2) this.entries = this.entries.slice(-VIEW_ENTRIES);
    return entry;
  }

  private takeThinking(): Part[] {
    if (!this.thinking.length) return [];
    const parts = this.thinking.map(thinking => ({ type: 'thinking', thinking }));
    this.thinking = [];
    return parts;
  }

  /** Persists a sent message once, where Claude Code took it. */
  private place(uuid: string, result: Applied) {
    const sent = this.pending.get(uuid);
    if (!sent) return;
    this.pending.delete(uuid);
    const entry = this.push([{ role: 'user', content: sent.text }]);
    result.placed.push({ clientId: sent.requestId, entry: entry.id });
  }

  /** The fallback when no echo came first: a reply frame names the sends its turn consumed. */
  private stamped(message: Loose, result: Applied) {
    if (!this.pending.size) return;
    const uuids: unknown[] = Array.isArray(message.user_message_uuids) ? message.user_message_uuids
      : typeof message.user_message_uuid === 'string' ? [message.user_message_uuid] : [];
    for (const uuid of uuids) if (typeof uuid === 'string') this.place(uuid, result);
  }

  private stream(message: Loose, result: Applied): boolean {
    this.stamped(message, result);
    const event = message.event;
    if (!isObject(event)) return false;
    switch (event.type) {
      case 'content_block_delta': {
        const delta = event.delta;
        if (delta?.type === 'text_delta' && typeof delta.text === 'string') {
          this.liveText = this.grow(this.liveText, delta.text);
          return true;
        }
        if (delta?.type === 'thinking_delta' && typeof delta.thinking === 'string' && delta.thinking) {
          this.liveThinking = this.grow(this.liveThinking, delta.thinking);
          return true;
        }
        return false;
      }
      default: return false;
    }
  }

  /** Appends a delta; clipping only past twice the limit keeps each delta to one concatenation. */
  private grow(buffer: string, delta: string) {
    const next = buffer + delta;
    return next.length > LIVE_LIMIT * 2 ? clipTail(next, LIVE_LIMIT) : next;
  }

  private assistant(message: Loose, result: Applied): boolean {
    this.stamped(message, result);
    const body = message.message;
    if (!isObject(body) || !Array.isArray(body.content)) return false;
    const failed = typeof message.error === 'string';
    if (!failed && isObject(body.usage)) this.recordUsage(body.usage, typeof body.model === 'string' ? body.model : null);
    let changed = false;
    for (const block of body.content) {
      if (!isObject(block)) continue;
      switch (block.type) {
        case 'thinking':
          // With thinking display omitted the block carries no text; nothing readable to keep.
          if (typeof block.thinking === 'string' && block.thinking) this.thinking.push(clipTail(block.thinking, LIVE_LIMIT));
          this.liveThinking = '';
          changed = true;
          break;
        case 'text': {
          const value = text(block.text);
          const content = [...this.takeThinking(), ...(value ? [{ type: 'text', text: value }] : [])];
          if (content.length) this.push([{ role: 'assistant', content, stopReason: failed ? 'error' : 'stop' }]);
          this.liveText = '';
          changed = true;
          break;
        }
        case 'tool_use': case 'server_tool_use': case 'mcp_tool_use':
          if (typeof block.id === 'string' && typeof block.name === 'string') { this.call(block.id, block.name, isObject(block.input) ? block.input : {}); changed = true; }
          break;
        case 'web_search_tool_result': case 'web_fetch_tool_result': case 'mcp_tool_result':
          // Server tools answer inside the assistant message.
          if (typeof block.tool_use_id === 'string') { this.answer(block.tool_use_id, this.serverResult(block), block.is_error === true); changed = true; }
          break;
      }
    }
    return changed;
  }

  private serverResult(block: Loose) {
    if (block.type === 'mcp_tool_result') return resultText(block.content);
    const content = block.content;
    if (Array.isArray(content)) return content.map((item: Loose) => text(item?.url)).filter(Boolean).join('\n') || 'Searched the web';
    return text(content?.url) ? `Fetched ${content.url}` : '';
  }

  private user(message: Loose, result: Applied): boolean {
    if (message.isReplay === true) {
      // The echo of a message this runtime sent, once a turn took it. Other echoes are Claude Code's own.
      if (typeof message.uuid === 'string' && this.pending.has(message.uuid)) { this.place(message.uuid, result); return true; }
      return false;
    }
    const content = message.message?.content;
    if (!Array.isArray(content)) return false;
    let changed = false;
    for (const block of content) {
      if (block?.type !== 'tool_result' || typeof block.tool_use_id !== 'string') continue;
      this.answer(block.tool_use_id, resultText(block.content), block.is_error === true);
      changed = true;
    }
    return changed;
  }

  private turnEnded(message: Loose, result: Applied): boolean {
    this.stamped(message, result);
    if (isObject(message.modelUsage)) {
      const usage = (this.tokens?.model && message.modelUsage[this.tokens.model]) || Object.values(message.modelUsage)[0];
      if (isObject(usage) && typeof usage.contextWindow === 'number' && usage.contextWindow > 0) this.window = usage.contextWindow;
    }
    // Reasoning with no following message still belongs in the transcript.
    if (this.thinking.length) this.push([{ role: 'assistant', content: this.takeThinking() }]);
    // A tool Claude Code never answered would otherwise read as running forever.
    for (const id of this.open) this.answer(id, UNFINISHED, true);
    const live = this.liveText !== '' || this.liveThinking !== '';
    this.liveText = ''; this.liveThinking = '';
    return live;
  }

  private recordUsage(usage: Loose, model: string | null) {
    const count = (value: unknown) => typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : 0;
    this.tokens = {
      input: count(usage.input_tokens), output: count(usage.output_tokens),
      cacheRead: count(usage.cache_read_input_tokens), cacheWrite: count(usage.cache_creation_input_tokens), model,
    };
  }

  /** Persists a tool call once, with any thinking that led to it. */
  private call(id: string, name: string, input: Loose) {
    if (this.called.has(id)) return;
    this.called.add(id);
    this.open.add(id);
    const call = toolCall(name, input);
    const part: Part = { type: 'toolCall', id, name: call.name, arguments: call.arguments };
    this.push([{ role: 'assistant', content: [...this.takeThinking(), part], stopReason: 'toolUse' }]);
  }

  /** Persists a tool result once. */
  private answer(id: string, output: string, isError: boolean) {
    if (this.answered.has(id)) return;
    this.answered.add(id);
    this.open.delete(id);
    this.push([{ role: 'toolResult', toolCallId: id, content: [{ type: 'text', text: bounded(output) }], isError }]);
  }

  /** The live document in the shape `update()` reads from Pi's `pi.live`. */
  private live() {
    const reasoning = this.thinking.length ? [...this.thinking, this.liveThinking].filter(Boolean).join('\n\n') : this.liveThinking;
    const thinking = reasoning.length > LIVE_LIMIT ? clipTail(reasoning, LIVE_LIMIT) : reasoning;
    const liveText = this.liveText.length > LIVE_LIMIT ? clipTail(this.liveText, LIVE_LIMIT) : this.liveText;
    if (!thinking && !liveText) return undefined;
    const content: Part[] = [
      ...(thinking ? [{ type: 'thinking', thinking }] : []),
      ...(liveText ? [{ type: 'text', text: liveText }] : []),
    ];
    return { generation: { message: { role: 'assistant', content } }, tools: [] };
  }
}
