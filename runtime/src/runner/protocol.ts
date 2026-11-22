/** Private, versioned runner transport. Frames include their terminating newline. */
export const COMMAND_LIMIT = 64 * 1024;
export const EVENT_LIMIT = 1024 * 1024;

export type RunnerState = 'preparing' | 'running' | 'awaiting_approval' | 'finishing';
export type Attach = { type: 'attach'; protocol: 1; last_seq: number };
export type Command = Attach | { type: 'start'; payload: unknown } | { type: 'approve'; approval_id: string; allow: boolean } | { type: 'stop' } | { type: 'steer'; request_id: string; text: string; images: unknown[] } | { type: 'set_mode'; mode: string } | { type: 'ack'; seq: number };
export type Attached = { type: 'attached'; state: RunnerState; seq: number; snapshot: unknown; pending_approval: unknown; head: unknown; before_record_id: unknown };
export type Event = { type: 'phase' | 'live' | 'commit' | 'approval_needed' | 'postimage' | 'record_before' | 'record_after' | 'finished'; seq: number; [key: string]: unknown };
export type RunnerMessage = Attached | Event;

function sequence(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
function strings(value: unknown): boolean { return Array.isArray(value) && value.every(item => typeof item === 'string'); }
function nullableString(value: unknown): boolean { return value === null || typeof value === 'string'; }
function approval(value: unknown): boolean {
  return object(value) && typeof value.id === 'string' && typeof value.tool_id === 'string'
    && (value.kind === 'edit' || value.kind === 'command') && typeof value.summary === 'string'
    && typeof value.details === 'string' && strings(value.dirty_paths) && typeof value.risk === 'string';
}
function image(value: unknown): boolean {
  return object(value) && typeof value.id === 'string' && (value.media_type === 'image/png' || value.media_type === 'image/jpeg')
    && sequence(value.size) && sequence(value.width) && sequence(value.height);
}
export function parseCommand(value: unknown): Command {
  if (!object(value)) throw new Error('invalid command');
  switch (value.type) {
    case 'attach': if (value.protocol === 1 && sequence(value.last_seq)) return value as Attach; break;
    case 'start': if (object(value.payload)) return value as Command; break;
    case 'approve': if (typeof value.approval_id === 'string' && typeof value.allow === 'boolean') return value as Command; break;
    case 'stop': return value as Command;
    case 'steer': if (typeof value.request_id === 'string' && typeof value.text === 'string' && Array.isArray(value.images) && value.images.every(image)) return value as Command; break;
    case 'set_mode': if (typeof value.mode === 'string') return value as Command; break;
    case 'ack': if (sequence(value.seq)) return value as Command; break;
  }
  throw new Error('invalid or unknown command');
}
export function parseRunnerMessage(value: unknown): RunnerMessage {
  if (!object(value) || !sequence(value.seq)) throw new Error('invalid runner message');
  switch (value.type) {
    case 'attached':
      if (['preparing', 'running', 'awaiting_approval', 'finishing'].includes(String(value.state)) && object(value.snapshot) && (value.pending_approval === null || approval(value.pending_approval)) && nullableString(value.head) && nullableString(value.before_record_id)) return value as Attached;
      break;
    case 'phase': if (['preparing', 'running', 'awaiting_approval', 'finishing'].includes(String(value.state))) return value as Event; break;
    case 'live': if (object(value.payload)) return value as Event; break;
    case 'commit': if (object(value.item) && nullableString(value.head) && strings(value.placed) && strings(value.changed_paths) && (value.usage === null || object(value.usage))) return value as Event; break;
    case 'approval_needed': if (approval(value.approval)) return value as Event; break;
    case 'postimage': if (object(value.postimage)) return value as Event; break;
    case 'record_before': case 'record_after': if (object(value.record)) return value as Event; break;
    case 'finished': if (object(value.result)) return value as Event; break;
  }
  throw new Error('invalid or unknown runner message');
}
export function encodeFrame(value: unknown, limit: number): Buffer {
  const frame = Buffer.from(`${JSON.stringify(value)}\n`, 'utf8');
  if (frame.length > limit) throw new Error('frame exceeds limit');
  return frame;
}
/** Incremental byte framing prevents split UTF-8 characters and oversized buffers. */
export class FrameDecoder<T> {
  private pending = Buffer.alloc(0);
  private readonly limit: number;
  private readonly parse: (value: unknown) => T;
  constructor(limit: number, parse: (value: unknown) => T) { this.limit = limit; this.parse = parse; }
  push(chunk: Buffer): T[] {
    const frames: T[] = [];
    let start = 0;
    while (start < chunk.length) {
      const end = chunk.indexOf(10, start);
      const part = chunk.subarray(start, end < 0 ? chunk.length : end + 1);
      if (this.pending.length + part.length > this.limit) throw new Error('frame exceeds limit');
      this.pending = Buffer.concat([this.pending, part]);
      if (end < 0) break;
      const text = new TextDecoder('utf-8', { fatal: true }).decode(this.pending.subarray(0, -1));
      this.pending = Buffer.alloc(0);
      frames.push(this.parse(JSON.parse(text)));
      start = end + 1;
    }
    return frames;
  }
}
export class EventSequence {
  private last: number;
  constructor(last: number = 0) { if (!sequence(last)) throw new Error('invalid sequence'); this.last = last; }
  accept(event: Event): void {
    if (event.seq <= this.last) throw new Error('non-monotonic sequence');
    this.last = event.seq;
  }
}

/** Retain events until the server has durably applied them. */
export class EventBuffer {
  private events: Event[] = [];
  private last = 0;
  private acknowledged = 0;
  publish(event: Event): void {
    parseRunnerMessage(event);
    if (event.seq <= this.last) throw new Error('non-monotonic sequence');
    this.last = event.seq;
    this.events.push(event);
  }
  ack(seq: number): void {
    if (!sequence(seq) || seq > this.last) throw new Error('invalid acknowledgement');
    if (seq <= this.acknowledged) return;
    this.acknowledged = seq;
    this.events = this.events.filter(event => event.seq > seq);
  }
  after(seq: number): Event[] {
    if (!sequence(seq) || seq < this.acknowledged || seq > this.last) throw new Error('invalid replay sequence');
    return this.events.filter(event => event.seq > seq);
  }
}
