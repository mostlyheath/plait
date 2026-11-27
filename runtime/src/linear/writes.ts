import type { DatabaseSync } from 'node:sqlite';
import { fail } from '../protocol.ts';
import type { EditVerification, LinearIssueDetail } from './types.ts';

export const LINEAR_WRITE_HISTORY_LIMIT = 10_000;
export type WriteOutcome = 'not_sent' | 'rejected' | 'accepted' | 'unknown';
export type WriteReceipt = {
  verification?: EditVerification;
  comment_id?: string;
  /** A created issue's id and, when Linear returned it, its identifier. */
  issue_id?: string;
  identifier?: string | null;
  issue: LinearIssueDetail | null;
  partial: boolean;
  warning: string | null;
  outcome: 'accepted';
};
export type WriteIntent = {
  request_id: string;
  op: string;
  hash: string;
  workspace_id: string | null;
  issue_id: string | null;
  payload: string | null;
  outcome: WriteOutcome;
  receipt: WriteReceipt | null;
  created_at: number;
  updated_at: number;
};

/** Retry identity is permanent. At capacity, refuse new intents instead of forgetting old effects. */
export class LinearWriteJournal {
  private readonly db: DatabaseSync;

  constructor(db: DatabaseSync) {
    this.db = db;
    db.exec(`
      CREATE TABLE IF NOT EXISTS linear_write_intents(
        request_id TEXT PRIMARY KEY,
        op TEXT NOT NULL,
        hash TEXT NOT NULL,
        workspace_id TEXT,
        issue_id TEXT,
        payload TEXT,
        outcome TEXT NOT NULL,
        receipt TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      );
      INSERT OR IGNORE INTO linear_write_intents
        (request_id, op, hash, outcome, receipt, created_at, updated_at)
      SELECT request_id, op, hash, 'accepted',
        json_set(result, '$.issue', json('null'), '$.outcome', 'accepted'), created_at, created_at
      FROM linear_writes;
    `);
  }

  get(requestId: string): WriteIntent | null {
    const row = this.db.prepare('SELECT * FROM linear_write_intents WHERE request_id=?').get(requestId);
    if (!row) return null;
    return {
      request_id: String(row.request_id), op: String(row.op), hash: String(row.hash),
      workspace_id: row.workspace_id === null ? null : String(row.workspace_id),
      issue_id: row.issue_id === null ? null : String(row.issue_id),
      payload: row.payload === null ? null : String(row.payload),
      outcome: row.outcome as WriteOutcome,
      receipt: row.receipt === null ? null : JSON.parse(String(row.receipt)) as WriteReceipt,
      created_at: Number(row.created_at), updated_at: Number(row.updated_at),
    };
  }

  begin(intent: Omit<WriteIntent, 'outcome' | 'receipt' | 'updated_at'>): WriteIntent {
    const saved = this.get(intent.request_id);
    if (saved) {
      if (saved.op !== intent.op || saved.hash !== intent.hash
          || (saved.workspace_id !== null && saved.workspace_id !== intent.workspace_id)) {
        fail('conflict', 'Request ID was already used for a different Linear write or workspace.');
      }
      return saved;
    }
    const count = Number(this.db.prepare('SELECT COUNT(*) AS n FROM linear_write_intents').get()?.n);
    if (count >= LINEAR_WRITE_HISTORY_LIMIT) {
      fail('linear_write_history_full', 'Linear write history is full. New writes are disabled to preserve retry safety.');
    }
    this.db.prepare(`INSERT INTO linear_write_intents
      (request_id, op, hash, workspace_id, issue_id, payload, outcome, created_at, updated_at)
      VALUES (?, ?, ?, ?, ?, ?, 'not_sent', ?, ?)`)
      .run(intent.request_id, intent.op, intent.hash, intent.workspace_id, intent.issue_id,
        intent.payload, intent.created_at, intent.created_at);
    return this.get(intent.request_id)!;
  }

  /** Commit uncertainty before handing bytes to fetch; a crash in either gap must never replay. */
  claim(requestId: string, payload: string, now: number): boolean {
    return this.db.prepare(`UPDATE linear_write_intents SET outcome='unknown', payload=?, updated_at=?
      WHERE request_id=? AND outcome IN ('not_sent', 'rejected')`).run(payload, now, requestId).changes === 1;
  }

  finish(requestId: string, outcome: 'accepted' | 'rejected', receipt: WriteReceipt | null, now: number): void {
    this.db.prepare(`UPDATE linear_write_intents SET outcome=?, receipt=?, payload=NULL, updated_at=?
      WHERE request_id=? AND outcome='unknown'`)
      .run(outcome, receipt ? JSON.stringify({ ...receipt, issue: null }) : null, now, requestId);
  }
}
