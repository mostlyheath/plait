import type { Message, Part } from '../transcript.ts';
import type { Usage } from '../protocol.ts';
import { clipTail } from '../text.ts';
import { NOTIFICATIONS } from './protocol.ts';
import type {
  AgentMessageItem, CommandExecutionItem, DeltaNotification, FileChangeItem, FileChangePatchUpdatedNotification,
  FileUpdateChange, ItemNotification, McpToolCallItem, OtherItem, ReasoningItem, ReasoningSummaryDeltaNotification,
  ReasoningTextDeltaNotification, ThreadItem, ThreadTokenUsage, TokenUsageNotification, TurnNotification, TurnStatus,
  UserMessageItem, WebSearchItem,
} from './protocol.ts';

import type { Applied, Entry } from '../external/entries.ts';

const VIEW_ENTRIES = 100;
/** Live buffers keep their tail; the snapshot shows at most 16 KiB of each. */
const LIVE_LIMIT = 65536;
/** A persisted tool result keeps its tail, as the end of command output matters most. */
const RESULT_LIMIT = 262144;
const UNFINISHED = 'The turn ended before this finished.';

type Tool = CommandExecutionItem | FileChangeItem | McpToolCallItem | WebSearchItem;
type LiveReasoning = { id: string; summary: string[]; content: string[] };

const isTool = (item: ThreadItem | OtherItem): item is Tool =>
  ['commandExecution', 'fileChange', 'mcpToolCall', 'webSearch'].includes(item.type);
const append = (buffer: string, delta: string) => clipTail(buffer + delta, LIVE_LIMIT);
const joined = (parts: readonly string[]) => parts.filter(Boolean).join('\n\n');
function bounded(text: string) {
  return text.length > RESULT_LIMIT ? `[Earlier output omitted]\n${clipTail(text, RESULT_LIMIT)}` : text;
}

/** How a completed turn ended, for the runtime's run state. `error` is Codex's message and may hold provider text. */
export function isTerminalTurn(method: string, params: any): { turnId: string; status: Exclude<TurnStatus, 'inProgress'>; error?: string } | undefined {
  if (method !== NOTIFICATIONS.turnCompleted) return undefined;
  const turn = (params as TurnNotification | undefined)?.turn;
  if (!turn || typeof turn.id !== 'string') return undefined;
  const status = turn.status === 'inProgress' ? 'completed' : turn.status;
  return { turnId: turn.id, status, ...(turn.error?.message ? { error: turn.error.message } : {}) };
}

function toolName(item: Tool) {
  switch (item.type) {
    case 'commandExecution': return 'command';
    case 'fileChange': return 'edit';
    case 'mcpToolCall': return `mcp:${item.server}/${item.tool}`;
    case 'webSearch': return 'web_search';
  }
}

function lineCounts(diff: string) {
  let added = 0; let removed = 0;
  for (const line of diff.split('\n')) {
    if (line.startsWith('+') && !line.startsWith('+++')) added++;
    else if (line.startsWith('-') && !line.startsWith('---')) removed++;
  }
  return { added, removed };
}

function changeSummary(changes: readonly FileUpdateChange[]) {
  const counts = changes.map(change => lineCounts(change.diff));
  const added = counts.reduce((sum, count) => sum + count.added, 0);
  const removed = counts.reduce((sum, count) => sum + count.removed, 0);
  return `${changes.length} ${changes.length === 1 ? 'file' : 'files'}, +${added} -${removed}`;
}

function toolArguments(item: Tool): unknown {
  switch (item.type) {
    case 'commandExecution': return { command: item.command, cwd: item.cwd };
    case 'fileChange': return { paths: item.changes.map(change => change.path), summary: changeSummary(item.changes) };
    case 'mcpToolCall': return item.arguments ?? {};
    case 'webSearch': return { query: item.query };
  }
}

function mcpText(item: McpToolCallItem) {
  if (item.error) return item.error.message;
  const content = item.result?.content ?? [];
  const text = content.map(part => (part as { type?: string; text?: string })?.type === 'text' ? (part as { text?: string }).text ?? '' : '')
    .filter(Boolean).join('\n');
  return text || (item.result ? JSON.stringify(item.result.structuredContent ?? content) : '');
}

function webSearchText(item: WebSearchItem) {
  const action = item.action;
  if (action?.type === 'openPage') return action.url ? `Opened ${action.url}` : 'Opened a page';
  if (action?.type === 'findInPage') return `Searched ${action.url ?? 'a page'}${action.pattern ? ` for ${action.pattern}` : ''}`;
  const queries = action?.type === 'search' ? action.queries ?? (action.query ? [action.query] : []) : [];
  return `Searched for ${(queries.length ? queries : [item.query]).filter(Boolean).join('; ') || 'the web'}`;
}

/** A finished tool's result text and whether it failed. */
function toolResult(item: Tool, liveOutput: string): { text: string; isError: boolean } {
  if ('status' in item && item.status === 'declined') return { text: 'Declined.', isError: true };
  switch (item.type) {
    case 'commandExecution': {
      const output = item.aggregatedOutput ?? liveOutput;
      const failed = item.status !== 'completed' || (item.exitCode !== null && item.exitCode !== 0);
      const code = item.exitCode !== null && item.exitCode !== 0 ? `${output ? '\n' : ''}[exit code ${item.exitCode}]` : '';
      return { text: bounded(output + code), isError: failed };
    }
    case 'fileChange': {
      const diff = item.changes.map(change => change.diff).filter(Boolean).join('\n');
      return item.status === 'completed' ? { text: bounded(diff || changeSummary(item.changes)), isError: false }
        : { text: bounded(`The edit was not applied.${diff ? `\n${diff}` : ''}`), isError: true };
    }
    case 'mcpToolCall': return { text: bounded(mcpText(item)), isError: item.status !== 'completed' || item.error !== null };
    case 'webSearch': return { text: webSearchText(item), isError: false };
  }
}

/**
 * Turns one Codex thread's notifications into append-only transcript entries and
 * live state. Completed items persist; deltas only change the live document.
 */
export class CodexTranscript {
  private entries: Entry[];
  private nextId: number;
  /** Tool calls whose call is persisted, and those whose result is too. */
  private readonly called = new Set<string>();
  private readonly answered = new Set<string>();
  /** User message items already persisted; a message can arrive as started and completed. */
  private readonly users = new Set<string>();
  /** Tool calls persisted and still without a result; a turn's end closes them. */
  private readonly open = new Set<string>();
  /** Started edit items, so an approval request can show their changes. */
  private readonly edits = new Map<string, FileChangeItem>();
  private turn: string | null = null;
  private tokens: ThreadTokenUsage | null = null;
  // Live state, cleared as each item completes.
  private liveText: { id: string; text: string } | null = null;
  private liveReasoning: LiveReasoning | null = null;
  /** Completed reasoning, joined to the next persisted assistant message as Pi stores thinking. */
  private thinking: string[] = [];
  private readonly liveTools = new Map<string, string>();

  constructor(entries: Entry[], nextId: number) {
    this.entries = entries.slice(-VIEW_ENTRIES);
    this.nextId = nextId;
    for (const entry of entries) for (const message of entry.model) {
      if (message.role === 'toolResult' && message.toolCallId) this.answered.add(message.toolCallId);
      if (Array.isArray(message.content)) for (const part of message.content) if (part.type === 'toolCall' && part.id) this.called.add(part.id);
    }
  }

  /** A user entry for typed text. Codex runs place user messages from `userMessage` items instead. */
  userInput(text: string): Entry {
    return this.push([{ role: 'user', content: text }]);
  }

  apply(method: string, params: any): Applied {
    const result: Applied = { append: [], changed: false, placed: [] };
    if (!params || typeof params !== 'object') return result;
    const before = this.nextId;
    result.changed = this.route(method, params, result);
    result.append = this.entries.filter(entry => entry.id >= before);
    if (result.append.length) result.changed = true;
    return result;
  }

  view(): { entries: Entry[]; docs: Record<string, unknown> } {
    const live = this.live();
    return { entries: this.entries.slice(-VIEW_ENTRIES), docs: live ? { 'pi.live': live } : {} };
  }

  /** The latest model request's usage, as Pi reports the latest response's. */
  usage(contextWindow: number | null): Usage | null {
    if (!this.tokens) return null;
    const last = this.tokens.last;
    // Codex counts cached and cache-write input inside inputTokens; Plait counts them apart.
    const input = Math.max(0, last.inputTokens - last.cachedInputTokens - last.cacheWriteInputTokens);
    return {
      input, output: last.outputTokens, cache_read: last.cachedInputTokens, cache_write: last.cacheWriteInputTokens,
      reasoning: last.reasoningOutputTokens, total: last.totalTokens, context_used: last.totalTokens,
      context_window: this.tokens.modelContextWindow ?? contextWindow,
    };
  }

  activeTurn(): string | null {
    return this.turn;
  }

  /** The started edit an approval request names. */
  edit(itemId: string): FileChangeItem | undefined {
    return this.edits.get(itemId);
  }

  private route(method: string, params: any, result: Applied): boolean {
    switch (method) {
      case NOTIFICATIONS.turnStarted: return this.turnStarted(params as TurnNotification);
      case NOTIFICATIONS.turnCompleted: return this.turnCompleted();
      case NOTIFICATIONS.itemStarted: return this.itemStarted((params as ItemNotification).item, result);
      case NOTIFICATIONS.itemCompleted: return this.itemCompleted((params as ItemNotification).item, result);
      case NOTIFICATIONS.agentMessageDelta: return this.textDelta(params as DeltaNotification);
      case NOTIFICATIONS.reasoningSummaryDelta: {
        const delta = params as ReasoningSummaryDeltaNotification;
        return this.reasoningDelta(delta, 'summary', delta.summaryIndex);
      }
      case NOTIFICATIONS.reasoningTextDelta: {
        const delta = params as ReasoningTextDeltaNotification;
        return this.reasoningDelta(delta, 'content', delta.contentIndex);
      }
      case NOTIFICATIONS.commandOutputDelta: return this.outputDelta(params as DeltaNotification);
      case NOTIFICATIONS.fileChangePatchUpdated: return this.patchUpdated(params as FileChangePatchUpdatedNotification);
      case NOTIFICATIONS.tokenUsage: return this.tokenUsage(params as TokenUsageNotification);
      default: return false;
    }
  }

  private push(model: Message[]): Entry {
    const entry: Entry = { id: this.nextId++, kind: 'message', model };
    this.entries.push(entry);
    if (this.entries.length > VIEW_ENTRIES * 2) this.entries = this.entries.slice(-VIEW_ENTRIES);
    return entry;
  }

  /** Thinking parts for completed reasoning not yet persisted, consumed by the next assistant message. */
  private takeThinking(): Part[] {
    const parts = this.thinking.map(thinking => ({ type: 'thinking', thinking }));
    this.thinking = [];
    return parts;
  }

  private turnStarted(params: TurnNotification) {
    if (typeof params.turn?.id !== 'string') return false;
    this.turn = params.turn.id;
    return false;
  }

  private turnCompleted() {
    this.turn = null;
    // Reasoning with no following message still belongs in the transcript.
    if (this.thinking.length) this.push([{ role: 'assistant', content: this.takeThinking() }]);
    // A tool Codex never finished would otherwise read as running forever.
    for (const id of this.open) this.result(id, { text: UNFINISHED, isError: true });
    this.open.clear();
    this.edits.clear();
    const live = this.live() !== undefined;
    this.liveText = null; this.liveReasoning = null; this.liveTools.clear();
    return live;
  }

  private itemStarted(item: ThreadItem | OtherItem | undefined, result: Applied): boolean {
    if (!item || typeof item.id !== 'string') return false;
    if (item.type === 'userMessage') return this.user(item as UserMessageItem, result);
    if (item.type === 'agentMessage') { this.liveText = { id: item.id, text: (item as AgentMessageItem).text ?? '' }; return true; }
    if (item.type === 'reasoning') {
      const reasoning = item as ReasoningItem;
      this.liveReasoning = { id: item.id, summary: [...reasoning.summary ?? []], content: [...reasoning.content ?? []] };
      return true;
    }
    if (item.type === 'fileChange') this.edits.set(item.id, item as FileChangeItem);
    // A web search's query is known only when it completes.
    if (isTool(item) && item.type !== 'webSearch') { this.call(item); return true; }
    return false;
  }

  private itemCompleted(item: ThreadItem | OtherItem | undefined, result: Applied): boolean {
    if (!item || typeof item.id !== 'string') return false;
    if (item.type === 'userMessage') return this.user(item as UserMessageItem, result);
    if (item.type === 'agentMessage') return this.agentMessage(item as AgentMessageItem);
    if (item.type === 'reasoning') return this.reasoning(item as ReasoningItem);
    if (!isTool(item)) return false;
    this.edits.delete(item.id);
    if (this.answered.has(item.id)) return false;
    this.call(item);
    this.result(item.id, toolResult(item, this.liveTools.get(item.id) ?? ''));
    this.liveTools.delete(item.id);
    return true;
  }

  private user(item: UserMessageItem, result: Applied) {
    if (this.users.has(item.id)) return false;
    this.users.add(item.id);
    const text = (item.content ?? []).map(input => input.type === 'text' ? (input as { text: string }).text : '').filter(Boolean).join('\n');
    const entry = this.push([{ role: 'user', content: text }]);
    if (typeof item.clientId === 'string' && item.clientId) result.placed.push({ clientId: item.clientId, entry: entry.id });
    return true;
  }

  private agentMessage(item: AgentMessageItem) {
    const text = item.text || (this.liveText?.id === item.id ? this.liveText.text : '');
    if (this.liveText?.id === item.id) this.liveText = null;
    const content = [...this.takeThinking(), ...(text ? [{ type: 'text', text }] : [])];
    if (content.length) this.push([{ role: 'assistant', content, stopReason: 'stop' }]);
    return true;
  }

  private reasoning(item: ReasoningItem) {
    const live = this.liveReasoning?.id === item.id ? this.liveReasoning : undefined;
    // The summary is what Codex shows; raw reasoning appears only when a provider sends it.
    const text = joined(item.summary ?? []) || joined(item.content ?? []) || joined(live?.summary ?? []) || joined(live?.content ?? []);
    if (live) this.liveReasoning = null;
    if (text) this.thinking.push(text);
    return true;
  }

  /** Persists a tool call once, with any reasoning that led to it. */
  private call(item: Tool) {
    if (this.called.has(item.id)) return;
    this.called.add(item.id);
    this.open.add(item.id);
    const part: Part = { type: 'toolCall', id: item.id, name: toolName(item), arguments: toolArguments(item) };
    this.push([{ role: 'assistant', content: [...this.takeThinking(), part], stopReason: 'toolUse' }]);
  }

  /** Persists a tool result once. */
  private result(id: string, result: { text: string; isError: boolean }) {
    if (this.answered.has(id)) return;
    this.answered.add(id);
    this.open.delete(id);
    this.push([{ role: 'toolResult', toolCallId: id, content: [{ type: 'text', text: result.text }], isError: result.isError }]);
  }

  private textDelta(params: DeltaNotification) {
    if (typeof params.delta !== 'string' || typeof params.itemId !== 'string') return false;
    if (this.liveText?.id !== params.itemId) this.liveText = { id: params.itemId, text: '' };
    this.liveText.text = append(this.liveText.text, params.delta);
    return true;
  }

  private reasoningDelta(params: DeltaNotification, field: 'summary' | 'content', index: number) {
    if (typeof params.delta !== 'string' || typeof params.itemId !== 'string' || !Number.isSafeInteger(index) || index < 0 || index > 256) return false;
    if (this.liveReasoning?.id !== params.itemId) this.liveReasoning = { id: params.itemId, summary: [], content: [] };
    const parts = this.liveReasoning[field];
    while (parts.length <= index) parts.push('');
    parts[index] = append(parts[index], params.delta);
    return true;
  }

  private outputDelta(params: DeltaNotification) {
    if (typeof params.delta !== 'string' || typeof params.itemId !== 'string' || this.answered.has(params.itemId)) return false;
    this.liveTools.set(params.itemId, append(this.liveTools.get(params.itemId) ?? '', params.delta));
    return true;
  }

  private patchUpdated(params: FileChangePatchUpdatedNotification) {
    const item = this.edits.get(params.itemId);
    if (item && Array.isArray(params.changes)) this.edits.set(params.itemId, { ...item, changes: params.changes });
    return false;
  }

  private tokenUsage(params: TokenUsageNotification) {
    if (!params.tokenUsage?.last) return false;
    this.tokens = params.tokenUsage;
    return true;
  }

  /** The live document in the shape `update()` reads from Pi's `pi.live`. */
  private live() {
    const reasoning = [...this.thinking];
    if (this.liveReasoning) reasoning.push(joined(this.liveReasoning.summary) || joined(this.liveReasoning.content));
    const thinking = clipTail(joined(reasoning), LIVE_LIMIT);
    const content: Part[] = [
      ...(thinking ? [{ type: 'thinking', thinking }] : []),
      ...(this.liveText?.text ? [{ type: 'text', text: this.liveText.text }] : []),
    ];
    const tools = [...this.liveTools].map(([callId, output]) => ({ callId, output }));
    if (!content.length && !tools.length) return undefined;
    return { ...(content.length ? { generation: { message: { role: 'assistant', content } } } : {}), tools };
  }
}
