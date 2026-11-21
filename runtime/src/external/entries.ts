// An external thread's translated transcript, one SQLite file per thread beside the
// Pi conversation stores. Entries are written as they finish, so history, full
// text, and a restarted host read the thread without asking the harness again.
import { DatabaseSync } from 'node:sqlite';
import { join } from 'node:path';
import type { Message } from '../transcript.ts';

/**
 * One persisted transcript entry, in the shape Pi's conversation view uses, so
 * `update()` in ../transcript.ts and the history pages read external threads unchanged.
 */
export type Entry = { id: number; kind: 'message'; model: Message[] };
export type Placed = { clientId: string; entry: number };
export type Applied = { append: Entry[]; changed: boolean; placed: Placed[] };

export function transcriptPath(directory: string, threadId: string, harness: 'codex' | 'claude'): string {
  return join(directory, `${threadId}.${harness}.sqlite`);
}

type Query = { minEntryId?: number; maxEntryId?: number; order: 'ascending' | 'descending' };

export class ExternalEntries {
  private readonly db: DatabaseSync;
  constructor(path: string, options: { readOnly?: boolean } = {}) {
    this.db = new DatabaseSync(path, { readOnly: options.readOnly === true });
    if (options.readOnly) return;
    this.db.exec(`PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
      CREATE TABLE IF NOT EXISTS entries(id INTEGER PRIMARY KEY, model TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);`);
  }
  /** The external thread this Plait thread continues, once one was started. */
  thread(): string | undefined {
    const row = this.db.prepare("SELECT value FROM meta WHERE key='thread'").get();
    return row ? String(row.value) : undefined;
  }
  setThread(id: string) {
    this.db.prepare("INSERT INTO meta VALUES ('thread',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").run(id);
  }
  /** The newest entries, oldest first, and the ID the next one takes. */
  tail(limit: number): { entries: Entry[]; next: number } {
    const entries = this.db.prepare('SELECT id,model FROM entries ORDER BY id DESC LIMIT ?').all(limit).reverse().map(entry);
    return { entries, next: (entries.at(-1)?.id ?? 0) + 1 };
  }
  append(entries: readonly Entry[]) {
    if (!entries.length) return;
    const insert = this.db.prepare('INSERT OR REPLACE INTO entries VALUES (?,?)');
    this.db.exec('BEGIN IMMEDIATE');
    try {
      for (const item of entries) insert.run(item.id, JSON.stringify(item.model));
      this.db.exec('COMMIT');
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  /** A page in the form history reads from Pi's conversation store. */
  entries(query: Query, limit: number): Entry[] {
    const where = ['id >= ?', 'id <= ?'];
    const values = [query.minEntryId ?? 0, query.maxEntryId ?? Number.MAX_SAFE_INTEGER];
    const order = query.order === 'descending' ? 'DESC' : 'ASC';
    return this.db.prepare(`SELECT id,model FROM entries WHERE ${where.join(' AND ')} ORDER BY id ${order} LIMIT ?`)
      .all(...values, limit).map(entry);
  }
  close() { this.db.close(); }
}

function entry(row: Record<string, unknown>): Entry {
  return { id: Number(row.id), kind: 'message', model: JSON.parse(String(row.model)) };
}
