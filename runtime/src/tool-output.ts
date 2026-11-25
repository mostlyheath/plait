import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fail } from './protocol.ts';

export function threadOutputsPath(store: string, threadId: string): string {
  return join(store, `${threadId}.outputs.sqlite`);
}

export const OUTPUT_LIMIT = 16 * 1024 * 1024;
export const OUTPUT_COUNT = 32;
export type TextRange = { text: string; offset: number; next_offset: number | null; total_bytes: number };

export function outputTail(text: string) {
  const bytes = Buffer.from(text);
  if (bytes.length <= 12000) return text;
  let start = bytes.length - 12000;
  while ((bytes[start] & 0xc0) === 0x80) start++;
  return '[Tail preview; use read_output for earlier output.]\n' + bytes.subarray(start).toString('utf8');
}

export function rangeArgs(args: Record<string, unknown>) {
  const offset = Object.hasOwn(args, 'offset') ? args.offset : 0;
  const limit = Object.hasOwn(args, 'limit') ? args.limit : 8192;
  if (typeof offset !== 'number' || !Number.isSafeInteger(offset) || offset < 0
    || typeof limit !== 'number' || !Number.isSafeInteger(limit) || limit < 4 || limit > 12000) {
    fail('invalid_request', 'offset must be a non-negative byte offset; limit must be 4–12000 bytes.');
  }
  return { offset, limit };
}

export function rangeText(page: TextRange, next: (offset: number) => string) {
  if (page.next_offset === null && page.offset === 0) return page.text;
  return page.text + `\n\n[Bytes ${page.offset}–${page.offset + Buffer.byteLength(page.text)} of ${page.total_bytes}. `
    + (page.next_offset === null ? 'End of output.]' : `Continue with ${next(page.next_offset)}.]`);
}

export type CommandOutputsOptions = {
  /** Fallback read-only database (e.g., host.sqlite when writing to a per-thread outputs database). */
  fallbackDbPath?: string;
  /** Store directory to check per-thread `<thread_id>.outputs.sqlite` before falling back to `this.db`. */
  threadOutputsDir?: string;
};

function readFromDb(db: DatabaseSync, thread: string, id: string, offset: number, limit: number): string | null {
  let row: Record<string, unknown> | undefined;
  try {
    row = db.prepare('SELECT * FROM command_outputs WHERE thread=? AND (id=? OR call_id=?) ORDER BY rowid DESC LIMIT 1').get(thread, id, id) as Record<string, unknown> | undefined;
  } catch {
    return null;
  }
  if (!row) return null;
  const total = Number(row.bytes);
  if (offset > total) fail('invalid_request', 'Output offset is beyond the stored output.');
  const chunks = db.prepare('SELECT start,data FROM command_output_chunks WHERE id=? AND start<? AND start+length(data)>? ORDER BY start').all(row.id as string, offset + limit, offset);
  const parts = chunks.map(chunk => Buffer.from(chunk.data as Uint8Array).subarray(Math.max(0, offset - Number(chunk.start)), offset + limit - Number(chunk.start)));
  let data = Buffer.concat(parts);
  let end = data.length;
  if (offset + end < total) {
    // Trim only an incomplete final codepoint; an invalid starting offset is rejected below.
    let start = end - 1;
    while (start >= 0 && (data[start] & 0xc0) === 0x80) start--;
    if (start >= 0) {
      const first = data[start]; const width = first < 0x80 ? 1 : first < 0xe0 ? 2 : first < 0xf0 ? 3 : 4;
      if (start + width > end) end = start;
    }
  }
  data = data.subarray(0, end);
  let text: string;
  try { text = new TextDecoder('utf-8', { fatal: true }).decode(data); }
  catch { fail('invalid_request', 'Output offset must be at a UTF-8 character boundary.'); }
  const next = offset + data.length;
  const page = rangeText({ text, offset, next_offset: next < total ? next : null, total_bytes: total },
    value => `read_output(${JSON.stringify({ id, offset: value })})`);
  return page + (row.truncated ? '\n[Output exceeded 16 MiB; later bytes were not retained.]' : '')
    + (!row.complete ? '\n[Capture did not finish; output may be incomplete.]' : '');
}

function readFromPath(path: string, thread: string, id: string, offset: number, limit: number): string | null {
  if (!existsSync(path)) return null;
  const db = new DatabaseSync(path, { readOnly: true, timeout: 5000 });
  try { return readFromDb(db, thread, id, offset, limit); }
  finally { db.close(); }
}

/** Host-owned logs share the durable store; no agent-controlled filesystem path is opened. */
export class CommandOutputs {
  private db: DatabaseSync;
  private options: CommandOutputsOptions;
  constructor(db: DatabaseSync, options: CommandOutputsOptions = {}) {
    this.db = db;
    this.options = options;
    db.exec(`CREATE TABLE IF NOT EXISTS command_outputs(id TEXT PRIMARY KEY, thread TEXT NOT NULL, call_id TEXT, bytes INTEGER NOT NULL DEFAULT 0, complete INTEGER NOT NULL DEFAULT 0, truncated INTEGER NOT NULL DEFAULT 0);
      CREATE INDEX IF NOT EXISTS command_outputs_thread ON command_outputs(thread);
      CREATE TABLE IF NOT EXISTS command_output_chunks(id TEXT NOT NULL, start INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY(id,start));`);
    if (!db.prepare('PRAGMA table_info(command_outputs)').all().some(row => row.name === 'call_id')) {
      db.exec('ALTER TABLE command_outputs ADD COLUMN call_id TEXT');
    }
  }
  begin(thread: string, callId?: string) {
    const id = randomUUID();
    this.db.exec('BEGIN IMMEDIATE');
    try {
      const old = this.db.prepare('SELECT id FROM command_outputs WHERE thread=? ORDER BY rowid DESC LIMIT -1 OFFSET ?').all(thread, OUTPUT_COUNT - 1);
      for (const row of old) {
        this.db.prepare('DELETE FROM command_output_chunks WHERE id=?').run(row.id!);
        this.db.prepare('DELETE FROM command_outputs WHERE id=?').run(row.id!);
      }
      this.db.prepare('INSERT INTO command_outputs(id,thread,call_id) VALUES(?,?,?)').run(id, thread, callId ?? null);
      this.db.exec('COMMIT');
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
    let bytes = 0; let truncated = false; let failure: unknown;
    const append = (text: string) => {
      if (failure || truncated || bytes >= OUTPUT_LIMIT) { truncated ||= text.length > 0; return; }
      let data = Buffer.from(text);
      if (data.length > OUTPUT_LIMIT - bytes) {
        let end = OUTPUT_LIMIT - bytes;
        while (end > 0 && (data[end] & 0xc0) === 0x80) end--;
        data = data.subarray(0, end); truncated = true;
      }
      // A capture error must not escape a process event callback or replay the command.
      try {
        this.db.exec('BEGIN IMMEDIATE');
        if (data.length) this.db.prepare('INSERT INTO command_output_chunks VALUES(?,?,?)').run(id, bytes, data);
        this.db.prepare('UPDATE command_outputs SET bytes=?,truncated=? WHERE id=?').run(bytes + data.length, +truncated, id);
        this.db.exec('COMMIT'); bytes += data.length;
      } catch (error) {
        try { this.db.exec('ROLLBACK'); } catch { /* BEGIN may itself have failed. */ }
        failure = error;
      }
    };
    return { id, append, finish: () => {
      if (!failure) {
        try { this.db.prepare('UPDATE command_outputs SET complete=1,truncated=? WHERE id=?').run(+truncated, id); }
        catch (error) { failure = error; }
      }
      return `\n[Command output: ${id}. ${failure ? 'Capture failed; stored output is incomplete. ' : truncated ? 'Output exceeded 16 MiB; only the beginning was retained. ' : ''}`
        + `Read with read_output({"id":"${id}","offset":0}). The newest ${OUTPUT_COUNT} command logs per thread are retained.]`;
    } };
  }
  read(thread: string, id: string, offset: number, limit: number): string {
    rangeArgs({ offset, limit });
    if (this.options.threadOutputsDir) {
      const fromThread = readFromPath(threadOutputsPath(this.options.threadOutputsDir, thread), thread, id, offset, limit);
      if (fromThread !== null) return fromThread;
    }
    const primary = readFromDb(this.db, thread, id, offset, limit);
    if (primary !== null) return primary;
    if (this.options.fallbackDbPath) {
      const fallback = readFromPath(this.options.fallbackDbPath, thread, id, offset, limit);
      if (fallback !== null) return fallback;
    }
    fail('not_found', 'Command output is unavailable in this thread or has expired.');
  }
}
