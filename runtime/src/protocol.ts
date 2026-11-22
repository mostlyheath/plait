import type { ImageRef } from './images.ts';
import { clipTail, escapeDisplay } from './text.ts';

export type QueueMode = 'follow_up' | 'steer';
/**
 * A `/skill:` input reads back as typed, since the skill's instructions are not the
 * person's words; a prompt template reads back expanded, as Pi shows it.
 */
export const readsAsTyped = (text: string | undefined): text is string => text?.startsWith('/skill:') === true;
export type InputRecord = {
  request_id: string; text: string; images?: ImageRef[]; entry: number | null; anchor: number;
  /** Set while the input waits behind a run; it is not part of the conversation yet. */
  queued?: { mode: QueueMode; approval: ApprovalMode };
};
export const inputMessage = (input: InputRecord) => ({ id: `request-${input.request_id}`, role: 'user' as const, text: clipTail(input.images?.length ? escapeDisplay(input.text) : input.text, 4096), ...(input.images?.length ? { images: input.images } : {}) });

export type ModelChoice = { provider: string; model_id: string };
export type Status = 'idle' | 'running' | 'awaiting_approval' | 'stopped' | 'interrupted' | 'error';
export type Failure = { code: string; message: string; outcome?: 'not_sent' | 'rejected' | 'accepted' | 'unknown'; retry_after_seconds?: number };
export type Approval = {
  id: string; tool_id: string; kind: 'edit' | 'command'; summary: string;
  details: string; dirty_paths: string[]; risk: string;
};
export type ApprovalMode = 'ask' | 'auto';
export const THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const;
export type ThinkingLevel = typeof THINKING_LEVELS[number];
export type Queued = { id: string; request_id: string; text: string; mode: QueueMode; approval: ApprovalMode };
/** Agent harnesses a thread can run in. Pi is built in; the others are the person's installs. */
export const HARNESSES = ['pi', 'codex', 'claude'] as const;
export type HarnessId = typeof HARNESSES[number];
/**
 * The harness that runs a thread, and the version of its latest run (null before one, and for Pi).
 * `mode` is the harness's own permission mode chosen for the thread; `reported` is the mode the
 * harness said it used in its latest run. Pi threads have neither: Plait's approval mode applies.
 */
export type ThreadHarness = { id: HarnessId; version: string | null; mode?: string; reported?: string | null };
/** One of a harness's own permission modes, as Settings and the composer offer it. */
export type HarnessMode = { id: string; label: string; detail: string };
export type Usage = {
  input: number; output: number; cache_read: number; cache_write: number; reasoning: number | null; total: number;
  context_used: number | null; context_window: number | null;
};
export type SnapshotMessage = { id: string; role: 'user' | 'assistant'; text: string; reasoning?: string; images?: ImageRef[] };
export type SnapshotTool = {
  id: string; name: string; status: 'running' | 'done' | 'error' | 'interrupted'; input: string; output: string;
  after?: string | null;
  /** Version 3 order, kept only when it differs: the tool followed a reasoning-only message. */
  after3?: string;
};
// Version 2 adds tool ordering (`after`) and the run's approval mode. Version 3 adds
// thread metadata, reasoning, usage, and queued inputs. Saved snapshots from older
// hosts lack them; `view` fills or strips them per client.
export type Snapshot = {
  id: string; checkout: string; model: ModelChoice; status: Status;
  messages: SnapshotMessage[];
  live_text: string;
  tools: SnapshotTool[];
  pending_approval: Approval | null; error: Failure | null; truncated: boolean;
  approval_mode?: ApprovalMode;
  title?: string; renamed?: boolean; created_at?: number; updated_at?: number; archived?: boolean;
  thinking_level?: ThinkingLevel; queued?: Queued[]; usage?: Usage | null;
  live_reasoning?: string; retry?: { attempt: number; at: number } | null;
  /** Absent for Pi threads, including every thread saved before version 4. */
  harness?: ThreadHarness;
  /** Version 5: changes to this thread's checkout records. */
  records_revision?: number;
  image_input?: 'yes' | 'no' | 'unknown';
};
export type Summary = {
  id: string; checkout: string; model: ModelChoice; status: string;
  title: string; created_at: number; updated_at: number; archived: boolean; pending_approval: boolean;
  /** Version 4 only. */
  harness?: HarnessId;
};
export const PROTOCOL_VERSIONS = [1, 2, 3, 4, 5, 6] as const;
export type Protocol = typeof PROTOCOL_VERSIONS[number];
export type Request = Record<string, string | boolean | number | string[]> & { op: string };
export function version(request: Request): Protocol {
  const asked = request.protocol_version;
  return asked === 6 ? 6 : asked === 5 ? 5 : asked === 4 ? 4 : asked === 3 ? 3 : asked === 2 ? 2 : 1;
}
const TURN_LIMIT = 'Agent ended without an answer (turn_limit). Inspect the transcript before retrying.';
/**
 * The snapshot exactly as a version 2 host stored it. Reasoning-only messages have
 * no text, so older clients never see them and tool order falls back to `after`.
 */
export function legacy(snapshot: Snapshot) {
  const { id, checkout, model, status, messages, live_text, tools, pending_approval, error, truncated, approval_mode } = snapshot;
  return {
    id, checkout, model, status,
    messages: messages.filter(message => message.text).map(({ id, role, text }) => ({ id, role, text })),
    live_text,
    tools: tools.map(({ after3: _after3, ...tool }) => tool),
    pending_approval,
    // Older clients know this failure only by its earlier generic code.
    error: error?.code === 'turn_limit' ? { code: 'runtime_error', message: TURN_LIMIT }
      : error?.code === 'context_compaction' ? { code: 'runtime_error', message: 'Agent ended without an answer (model_error). Inspect the transcript before retrying.' }
      : error?.code === 'record_failed' ? { code: 'runtime_error', message: error.message } : error,
    truncated,
    ...(approval_mode ? { approval_mode } : {}),
  };
}
/** The wire shape a client negotiated. Version 1 and 2 clients reject unknown fields. */
export function view(snapshot: Snapshot, protocol: Protocol) {
  const { approval_mode, tools, ...rest } = legacy(snapshot);
  if (protocol === 1) return { ...rest, tools: tools.map(({ after: _after, ...tool }) => tool) };
  // Tools saved before version 2 keep no `after`; clients show them in the old order.
  if (protocol === 2) return { ...rest, tools, approval_mode: approval_mode ?? 'ask' };
  const third = {
    ...rest, error: snapshot.error,
    messages: snapshot.messages.map(({ id, role, text, reasoning, images }) => ({ id, role, text, reasoning: reasoning ?? '', ...(protocol >= 6 && images?.length ? { images } : {}) })),
    tools: snapshot.tools.map(({ after3, ...tool }) => after3 === undefined ? tool : { ...tool, after: after3 }),
    approval_mode: approval_mode ?? 'ask',
    title: snapshot.title ?? '', created_at: snapshot.created_at ?? 0, updated_at: snapshot.updated_at ?? 0,
    archived: snapshot.archived ?? false, thinking_level: snapshot.thinking_level ?? 'off',
    queued: snapshot.queued ?? [], usage: snapshot.usage ?? null,
    live_reasoning: snapshot.live_reasoning ?? '', retry: snapshot.retry ?? null,
  };
  // Version 4 adds the harness; Pi threads saved earlier have none recorded.
  const harness = snapshot.harness;
  return protocol === 3 ? third : {
    ...(protocol >= 6 ? { image_input: snapshot.image_input ?? 'unknown' } : {}),
    ...(protocol >= 5 ? { records_revision: snapshot.records_revision ?? 0 } : {}),
    ...third,
    harness: { id: harness?.id ?? 'pi', version: harness?.version ?? null, mode: harness?.mode ?? null, reported: harness?.reported ?? null },
  };
}
/** A thread summary in the negotiated shape: the harness is a version 4 field. */
export function summaryView(summary: Summary, protocol: Protocol): Summary {
  const { harness, ...rest } = summary;
  return protocol >= 4 ? { ...rest, harness: harness ?? 'pi' } : rest;
}
export const queuedView = (input: InputRecord): Queued => ({
  id: `request-${input.request_id}`, request_id: input.request_id, text: inputMessage(input).text,
  mode: input.queued!.mode, approval: input.queued!.approval,
});
export class HostError extends Error {
  code: string;
  outcome?: Failure['outcome'];
  retryAfterSeconds?: number;
  constructor(code: string, message: string) { super(message); this.code = code; }
}
export function fail(code: string, message: string): never { throw new HostError(code, message); }
export function failure(error: unknown): Failure {
  return error instanceof HostError ? { code: error.code, message: error.message, ...(error.outcome ? { outcome: error.outcome } : {}),
    ...(error.code === 'linear_rate_limited' && Number.isSafeInteger(error.retryAfterSeconds)
      && error.retryAfterSeconds! > 0 && error.retryAfterSeconds! <= 86_400 ? { retry_after_seconds: error.retryAfterSeconds } : {}) }
    : { code: 'internal', message: 'Host operation failed. No credentials or provider details are exposed.' };
}
const fields: Record<string, string[]> = { install_harness: ['harness'], start_harness_sign_in: ['harness'],
  status: [], set_default_model: ['provider', 'model_id'], list_threads: ['checkout'],
  create_thread: ['checkout', 'provider', 'model_id', 'request_id'],
  submit: ['thread_id', 'request_id', 'text'], stop: ['thread_id'],
  approve: ['thread_id', 'approval_id', 'allow'], watch: ['thread_id'],
  command_policy: ['checkout'], set_command_policy: ['checkout', 'policy'],
};
// Operations and fields that exist only for a request that negotiated version 3.
const fields3: Record<string, string[]> = {
  list_threads: [], history: ['thread_id'], history_text: ['thread_id', 'ref', 'offset'],
  rename_thread: ['thread_id', 'title'], archive_thread: ['thread_id', 'archived'], watch_threads: [],
  configure_thread: ['thread_id'], cancel_queued: ['thread_id', 'request_id'], changed_files: ['thread_id'],
  agent_resources: ['checkout'], set_mcp_server: ['checkout', 'name', 'enabled'],
};
// Version 4 adds harnesses: an operation to list them and a choice when a thread starts.
const fields4: Record<string, string[]> = { harnesses: [], set_harness_path: ['harness'], set_harness_mode: ['harness', 'mode'], set_up_harness: ['harness'] };
const fields6: Record<string, string[]> = { image_put: ['thread_id', 'image_id', 'offset', 'total', 'data'], image_read: ['thread_id', 'image_id', 'offset'] };
const optional6: Record<string, string[]> = { submit: ['images'] };
const fields5: Record<string, string[]> = { turn_records: ['thread_id'], undo_turn: ['thread_id', 'record_id', 'request_id'] };
const optional5: Record<string, string[]> = { turn_records: ['before', 'limit'], undo_turn: ['leave_changed'] };
export const RECORD_CURSOR = /^\d{1,16}:\d{1,16}:\d{1,16}:\d{1,16}$/;
const optional4: Record<string, string[]> = { create_thread: ['harness', 'mode'], set_harness_path: ['path'], configure_thread: ['mode'] };
const optional3: Record<string, string[]> = {
  set_mcp_server: ['definition'],
  submit: ['when_busy'], list_threads: ['checkout', 'limit', 'cursor', 'archived'], history: ['before', 'limit'],
  watch_threads: ['archived'], configure_thread: ['provider', 'model_id', 'thinking'],
  create_thread: ['thinking'],
};
const limits: Record<string, number> = { list_threads: 200, history: 500, turn_records: 50 };
export const CURSOR = /^\d{1,16}:\d{1,16}$/;
export const HISTORY_CURSOR = /^\d{1,16}:[01]:\d{1,16}$/;
export const HISTORY_REF = /^(?:e\d{1,16}\.\d{1,6}\.(?:text|reasoning|output|input\.\d{1,6})|[pq]\d{1,16})$/;
const text = (value: unknown): value is string => typeof value === 'string' && !!value.trim();
function validate3(request: Request) {
  const invalid = (message: string): never => fail('invalid_request', message);
  if (request.definition !== undefined && (typeof request.definition !== 'string' || !/^[a-f0-9]{64}$/.test(request.definition))) invalid('Invalid MCP definition digest.');
  if (request.checkout !== undefined && !text(request.checkout)) invalid('checkout must be a nonempty string.');
  if (request.limit !== undefined && (!Number.isSafeInteger(request.limit) || (request.limit as number) < 1 || (request.limit as number) > limits[request.op])) {
    invalid(`limit must be an integer from 1 to ${limits[request.op]}.`);
  }
  if (request.cursor !== undefined && (typeof request.cursor !== 'string' || !CURSOR.test(request.cursor))) invalid('Invalid cursor.');
  if (request.before !== undefined && (typeof request.before !== 'string' || !(request.op === 'turn_records' ? RECORD_CURSOR : HISTORY_CURSOR).test(request.before))) invalid('Invalid before cursor.');
  if (request.op === 'turn_records' && request.before !== undefined && !(request.before as string).split(':').every(part => Number.isSafeInteger(Number(part)))) invalid('Invalid before cursor.');
  if (request.ref !== undefined && !HISTORY_REF.test(request.ref as string)) invalid('Invalid ref.');
  if (request.archived !== undefined && typeof request.archived !== 'boolean') invalid('archived must be boolean.');
  if (request.when_busy !== undefined && request.when_busy !== 'follow_up' && request.when_busy !== 'steer') invalid('when_busy must be follow_up or steer.');
  if (request.thinking !== undefined && !THINKING_LEVELS.includes(request.thinking as ThinkingLevel)) invalid(`thinking must be one of ${THINKING_LEVELS.join(', ')}.`);
  if (request.op !== 'configure_thread') return;
  if ((request.provider === undefined) !== (request.model_id === undefined)) invalid('provider and model_id must be given together.');
  if (request.provider !== undefined && (!text(request.provider) || !text(request.model_id))) invalid('provider and model_id must be nonempty strings.');
  if (request.provider === undefined && request.thinking === undefined && request.mode === undefined) invalid('Give a model, a thinking level, a mode, or more than one.');
}
export function validate(value: unknown): Request {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail('invalid_request', 'Expected an object.');
  const request = value as Request;
  const third = version(request) >= 3;
  const fourth = version(request) >= 4;
  const sixth = version(request) >= 6;
  const fifth = version(request) >= 5;
  const known = typeof request.op === 'string' ? request.op : '';
  const expected = sixth && Object.hasOwn(fields6, known) ? fields6[known] : fifth && Object.hasOwn(fields5, known) ? fields5[known] : fourth && Object.hasOwn(fields4, known) ? fields4[known]
    : third && Object.hasOwn(fields3, known) ? fields3[known] : Object.hasOwn(fields, known) ? fields[known] : undefined;
  // `approval` stays allowed on every version so version 1 keeps its specific error.
  const allowed = expected && [...expected, 'protocol_version', ...(known === 'submit' ? ['approval'] : []), ...(third ? optional3[known] ?? [] : []),
    ...(fourth ? optional4[known] ?? [] : []), ...(fifth ? optional5[known] ?? [] : []), ...(sixth ? optional6[known] ?? [] : [])];
  if (!allowed || Object.keys(request).some(key => key !== 'op' && !allowed.includes(key))) fail('invalid_request', 'Unknown operation or field.');
  if (request.protocol_version !== undefined && !PROTOCOL_VERSIONS.includes(request.protocol_version as Protocol)) {
    fail('invalid_request', 'Unsupported protocol_version.');
  }
  if (request.approval !== undefined && (version(request) === 1 || (request.approval !== 'ask' && request.approval !== 'auto'))) {
    fail('invalid_request', 'approval must be ask or auto and requires protocol_version 2.');
  }
  for (const key of expected) {
    if (key === 'allow' || key === 'archived' || key === 'enabled') {
      if (typeof request[key] !== 'boolean') fail('invalid_request', `${key} must be boolean.`);
    } else if (key === 'offset' || key === 'total') {
      if (!Number.isSafeInteger(request[key]) || (request[key] as number) < 0) fail('invalid_request', 'offset must be a non-negative integer.');
    } else if (key === 'text' && sixth && Array.isArray(request.images) && request.images.length && typeof request.text === 'string') {
      // Image-only messages are valid in version 6.
    } else if (!text(request[key])) fail('invalid_request', `${key} must be a nonempty string.`);
  }
  if (request.images !== undefined && (!Array.isArray(request.images) || request.images.length > 4 || request.images.some(id => typeof id !== 'string' || !/^[a-f0-9]{64}$/.test(id)))) fail('invalid_request', 'A message can have at most 4 image IDs.');
  if (request.image_id !== undefined && (typeof request.image_id !== 'string' || !/^[a-f0-9]{64}$/.test(request.image_id))) fail('invalid_request', 'Invalid image ID.');
  if (third) validate3(request);
  if (request.harness !== undefined && !HARNESSES.includes(request.harness as HarnessId)) fail('invalid_request', `harness must be one of ${HARNESSES.join(', ')}.`);
  if (request.mode !== undefined && (typeof request.mode !== 'string' || !/^[a-z_]{1,32}$/.test(request.mode))) fail('invalid_request', 'Invalid mode.');
  if (request.path !== undefined && (typeof request.path !== 'string' || !request.path.startsWith('/') || request.path.length > 4096)) {
    fail('invalid_request', 'path must be an absolute path.');
  }
  if (request.leave_changed !== undefined && typeof request.leave_changed !== 'boolean') fail('invalid_request', 'leave_changed must be boolean.');
  for (const key of ['request_id', 'thread_id', 'approval_id', 'record_id']) {
    if (request[key] !== undefined && !/^[A-Za-z0-9_-]{1,128}$/.test(request[key] as string)) fail('invalid_request', `Invalid ${key}.`);
  }
  if (Buffer.byteLength(JSON.stringify(request)) > 65000) fail('invalid_request', 'Request is too large.');
  return request;
}
