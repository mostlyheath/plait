import { threadKind } from './store.ts';
// Additive immutable turn review. The desktop receives bytes, never store paths.
import { randomUUID } from 'node:crypto';
import { fail } from './protocol.ts';
import { helper, type Records } from './records.ts';
import { checkRoot, safePath } from './safety.ts';
import type { Store } from './store.ts';

type Checkout = { host_id: string; id: string; catalog_revision: number; review_revision: number };
type Entry = { mode: '100644' | '100755' | '120000'; fingerprint: string; bytes: number } | null;
type Hunk = { first_line: number; last_line: number; old_start: number; old_count: number; new_start: number; new_count: number };
type Read = { before: Entry; after: Entry; patch: number[]; hunks: Hunk[]; status: string; truncated: boolean; elapsed_ms: number; processes: number };
export type Page = { token: string; offset: number; total: number; items: unknown[]; next_offset: number | null };
type Document = { checkout: string; items: unknown[]; bytes: number; touched: number; offsets: Set<number>; pageLimit: number };
const operations: Record<string, string[]> = { host_identity: [], turn_review_info: ['thread_id'], turn_diff: ['thread_id', 'record_id', 'path'], turn_review_page: ['token', 'offset'] };
const id = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
export function validateReview(input: unknown): Record<string, unknown> & { op: string } {
  if (!input || typeof input !== 'object' || Array.isArray(input)) fail('invalid_request', 'Invalid turn review request.');
  const request = input as Record<string, unknown> & { op: string };
  const fields = Object.hasOwn(operations, request.op) ? operations[request.op] : undefined;
  if (!fields || Object.keys(request).some(key => key !== 'op' && !fields.includes(key)) || fields.some(key => request[key] === undefined)
      || Buffer.byteLength(JSON.stringify(request)) > 65_000) fail('invalid_request', 'Unknown operation or field.');
  for (const key of ['thread_id', 'record_id', 'token']) if (fields.includes(key) && !id(request[key])) fail('invalid_request', `Invalid ${key}.`);
  if (fields.includes('offset') && (!Number.isSafeInteger(request.offset) || Number(request.offset) < 0)) fail('invalid_request', 'Invalid page offset.');
  if (fields.includes('path')) {
    if (typeof request.path !== 'string' || Buffer.byteLength(request.path) > 4096 || /[\u0000-\u001f\u007f\ud800-\udfff]/u.test(request.path)) fail('invalid_request', 'Invalid path.');
    try { safePath(request.path); } catch { fail('invalid_request', 'Path must stay inside the checkout and outside Git metadata.'); }
  }
  return request;
}

/** Tokens belong to this owner-authenticated runtime instance, across socket exchanges. */
export class TurnReview {
  private documents = new Map<string, Document>();
  private host: string;
  private store: Store;
  private records: Records;
  private directory: string;
  private now: () => number;
  constructor(store: Store, records: Records, directory: string, now = Date.now) {
    this.store = store; this.records = records; this.directory = directory; this.now = now;
    this.host = store.setting('review_host_id') ?? randomUUID();
    store.setSetting('review_host_id', this.host);
  }
  handles(input: unknown) { return !!input && typeof input === 'object' && Object.hasOwn(operations, String((input as { op?: unknown }).op)); }
  async handle(input: unknown) {
    const request = validateReview(input);
    if (request.op === 'host_identity') return { schema: 1, host_id: this.host, store: this.directory };
    if (request.op === 'turn_review_page') return this.page(String(request.token), Number(request.offset));
    const thread = String(request.thread_id);
    const snapshot = this.store.get(thread);
    const identity = this.store.rootIdentity(thread);
    await checkRoot(snapshot.checkout, identity);
    const checkout = this.checkout(snapshot.checkout, identity);
    if (request.op === 'turn_review_info') return { schema: 1, checkout, features: { record_diff: threadKind(this.store, String(request.thread_id)) === 'git', attribution: false, stage_turn: false, since_review: false }, retention_days: 30 };
    return this.records.read(async () => {
      const row = this.store.db.prepare(`SELECT r.*,roots.identity FROM records r JOIN roots ON roots.id=r.thread WHERE r.id=? AND r.checkout=? AND r.kind IN ('after','undo')`).get(String(request.record_id), snapshot.checkout);
      if (!row) fail('not_found', "This turn's record is no longer retained.");
      const saved = JSON.parse(String(row.identity));
      if (saved.dev !== identity.dev || saved.ino !== identity.ino) fail('permission_denied', 'The record belongs to another checkout.');
      const changed = this.store.db.prepare('SELECT path,before_mode,before_oid,after_mode,after_oid,added,removed FROM record_paths WHERE record=? AND path=?').get(String(request.record_id), String(request.path));
      let read: Read | undefined;
      if (changed && (row.state === 'ready' || row.state === 'undone')) {
        try {
          read = await helper<Read>('review', { store: this.directory, checkout: snapshot.checkout, dev: Number(identity.dev), ino: Number(identity.ino),
            object_format: String(row.commit_id).length === 64 ? 'sha256' : 'sha1', changed }, undefined, 10_000);
        } catch { /* Public reads never reveal helper diagnostics or store paths. */ }
      }
      await checkRoot(snapshot.checkout, identity);
      const bytes = Buffer.from(read?.patch ?? []);
      const chunks = [];
      for (let offset = 0; offset < bytes.length; offset += 65_536) chunks.push({ byte_offset: offset, bytes_base64: bytes.subarray(offset, offset + 65_536).toString('base64') });
      const origin = { kind: 'turn', record_id: String(row.id), thread_id: String(row.thread), request_id: String(row.run_request), title: null, created_at: Number(row.created_at) };
      const hunks = (read?.hunks ?? []).map((hunk, index) => ({ ...hunk, id: `${row.id}-${index}`, origin, stageable: false, refusal: 'unsupported' }));
      return this.withDocuments(checkout.id, { schema: 1, checkout, view_id: randomUUID(), path: String(request.path), side: null, basis: 'record', source_record_id: String(row.id),
        before: read?.before ?? null, after: read?.after ?? null, status: read?.status ?? 'unavailable', reason: read ? null : (changed ? 'missing_object' : 'not_covered'),
        patch_bytes: bytes.length,
        review: { state: 'unreviewed', revision: 0, reviewed_at: null, expires_at: null, source_record_id: null, can_mark: false },
        truncated: read?.truncated ?? false, elapsed_ms: read?.elapsed_ms ?? 0 }, chunks, hunks);
    });
  }
  private checkout(root: string, identity: { dev: string; ino: string }): Checkout {
    this.store.db.prepare('INSERT OR IGNORE INTO review_checkouts VALUES (?,?,?,?,0)').run(randomUUID(), root, identity.dev, identity.ino);
    const row = this.store.db.prepare('SELECT id,catalog_revision FROM review_checkouts WHERE canonical_root=? AND root_dev=? AND root_ino=?').get(root, identity.dev, identity.ino)!;
    return { host_id: this.host, id: String(row.id), catalog_revision: Number(row.catalog_revision), review_revision: 0 };
  }
  withDocuments<T extends object>(checkout: string, metadata: T, patch: unknown[], hunks: unknown[]): T & { patch: Page; hunks: Page } {
    // Both initial pages travel in one 1 MiB transport frame, including its envelope.
    const budget = 1_048_576 - 1024 - Buffer.byteLength(JSON.stringify(metadata));
    const pageLimit = Math.min(750_000, Math.floor(budget / 2));
    if (pageLimit < 1024) fail('review_limit', 'This preview exceeds the review limit.');
    return { ...metadata, patch: this.document(checkout, patch, pageLimit), hunks: this.document(checkout, hunks, pageLimit) };
  }
  document(checkout: string, items: unknown[], pageLimit = 750_000): Page {
    this.evict();
    const bytes = Buffer.byteLength(JSON.stringify(items));
    if (bytes > 32 * 1024 * 1024) fail('review_limit', 'This preview exceeds the review limit.');
    const token = randomUUID();
    this.documents.set(token, { checkout, items, bytes, touched: this.now(), offsets: new Set([0]), pageLimit });
    this.evict();
    return this.page(token, 0);
  }
  page(token: string, offset: number): Page {
    this.evict();
    const doc = this.documents.get(token);
    if (!doc) fail('expired_view', 'This view expired. Refresh to read the record again.');
    if (!doc.offsets.has(offset)) fail('invalid_request', 'Invalid page offset.');
    const items: unknown[] = [];
    let size = 1024;
    for (const item of doc.items.slice(offset, offset + 500)) {
      const bytes = Buffer.byteLength(JSON.stringify(item)) + 1;
      if (size + bytes > doc.pageLimit) {
        if (items.length === 0) fail('review_limit', 'This preview exceeds the review limit.');
        break;
      }
      items.push(item); size += bytes;
    }
    const next_offset = offset + items.length < doc.items.length ? offset + items.length : null;
    if (next_offset !== null) doc.offsets.add(next_offset);
    doc.touched = this.now();
    return { token, offset, total: doc.items.length, items, next_offset };
  }
  private evict() {
    for (const [token, doc] of this.documents) if (this.now() - doc.touched >= 300_000) this.documents.delete(token);
    let total = 0; const checkouts = new Map<string, number>();
    for (const [, doc] of this.documents) { total += doc.bytes; checkouts.set(doc.checkout, (checkouts.get(doc.checkout) ?? 0) + doc.bytes); }
    for (const [token, doc] of [...this.documents].sort((a, b) => a[1].touched - b[1].touched)) {
      if (total <= 128 * 1024 * 1024 && checkouts.get(doc.checkout)! <= 32 * 1024 * 1024) continue;
      this.documents.delete(token); total -= doc.bytes; checkouts.set(doc.checkout, checkouts.get(doc.checkout)! - doc.bytes);
    }
  }
}
