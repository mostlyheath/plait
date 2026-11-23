import { CheckoutQueue } from './checkout-queue.ts';
import { Images } from './images.ts';
import type { ImageRef } from './images.ts';
import { DatabaseSync } from 'node:sqlite';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import type { HarnessId, ModelChoice, Snapshot, InputRecord, Summary } from './protocol.ts';
import { fail, inputMessage, queuedView, readsAsTyped } from './protocol.ts';
import { boundSnapshot, pushMessage } from './transcript.ts';
import { title } from './text.ts';
import { CommandOutputs } from './tool-output.ts';
import { LinearCache } from './linear/cache.ts';
import { checkRoot } from './safety.ts';
import type { FileIdentity, Postimage } from './safety.ts';

type Row = Record<string, unknown>;
/** An input placed in the conversation store. `text` is absent for an input saved without its text. */
export type Placed = { request_id: string; text?: string; images?: ImageRef[] };
export type PendingInput = { rowid: number; request_id: string; text: string; anchor: number; images?: ImageRef[] };
// Additive columns. Summary columns mirror the snapshot so listing never parses transcripts.
const columns = [
  ['threads', 'head', 'INTEGER NOT NULL DEFAULT 0'], ['requests', 'input', 'TEXT'], ['requests', 'entry', 'INTEGER'], ['requests', 'anchor', 'INTEGER'],
  ['threads', 'checkout', 'TEXT'], ['threads', 'status', 'TEXT'], ['threads', 'model', 'TEXT'], ['threads', 'title', 'TEXT'],
  ['threads', 'archived', 'INTEGER NOT NULL DEFAULT 0'], ['threads', 'pending', 'INTEGER NOT NULL DEFAULT 0'],
  ['threads', 'created_at', 'INTEGER'], ['threads', 'updated_at', 'INTEGER'],
  ['requests', 'state', 'TEXT'], ['requests', 'queue', 'TEXT'],
  // Null for Pi, so threads saved before version 4 need no backfill.
  ['threads', 'harness', 'TEXT'],
  ['threads', 'records_revision', 'INTEGER NOT NULL DEFAULT 0'],
  ['records', 'undoes', 'TEXT'],
  ['runs', 'outcome', 'TEXT'],
];
const summary = (row: Row): Summary => ({
  id: String(row.id), checkout: String(row.checkout), model: JSON.parse(String(row.model)), status: String(row.status),
  title: String(row.title ?? ''), created_at: Number(row.created_at ?? 0), updated_at: Number(row.updated_at ?? 0),
  archived: row.archived === 1, pending_approval: row.pending === 1,
  ...(typeof row.harness === 'string' ? { harness: row.harness as HarnessId } : {}),
});
const input = (row: Row): InputRecord => ({
  request_id: String(row.id), text: JSON.parse(String(row.input)), entry: row.entry === null ? null : Number(row.entry), anchor: Number(row.anchor ?? 0),
  ...(row.state === 'queued' ? { queued: JSON.parse(String(row.queue)) } : {}),
});

export class Store {
  db: DatabaseSync;
  outputs: CommandOutputs;
  images: Images;
  linear: LinearCache;
  waiters: CheckoutQueue;
  private txDepth = 0;
  constructor(directory: string) {
    this.db = new DatabaseSync(join(directory, 'host.sqlite'));
    this.db.exec(`PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
      CREATE TABLE IF NOT EXISTS threads(id TEXT PRIMARY KEY, snapshot TEXT NOT NULL, head INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS requests(scope TEXT NOT NULL, id TEXT NOT NULL, hash TEXT NOT NULL, result TEXT NOT NULL, input TEXT, entry INTEGER, anchor INTEGER, PRIMARY KEY(scope,id));
      CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS thread_vcs(thread TEXT PRIMARY KEY, vcs TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS roots(id TEXT PRIMARY KEY, identity TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS postimages(thread_id TEXT NOT NULL, path TEXT NOT NULL, image TEXT NOT NULL, PRIMARY KEY(thread_id,path));
      CREATE TABLE IF NOT EXISTS records(id TEXT PRIMARY KEY, thread TEXT NOT NULL, run_request TEXT NOT NULL, kind TEXT NOT NULL, checkout TEXT NOT NULL,
        before TEXT, commit_id TEXT, tree TEXT, head TEXT, head_ref TEXT, state TEXT NOT NULL, not_covered TEXT, not_covered_total INTEGER,
        after_restart INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS records_thread ON records(thread, created_at);
      CREATE INDEX IF NOT EXISTS records_before ON records(before);
      CREATE TABLE IF NOT EXISTS record_garbage(id TEXT PRIMARY KEY);
      CREATE TABLE IF NOT EXISTS undo_operations(thread TEXT NOT NULL, request_id TEXT NOT NULL, target TEXT NOT NULL, leave_changed INTEGER NOT NULL,
        before_id TEXT NOT NULL, record_id TEXT NOT NULL, result TEXT, PRIMARY KEY(thread,request_id));
      CREATE TABLE IF NOT EXISTS record_paths(record TEXT NOT NULL, path TEXT NOT NULL, before_mode TEXT, before_oid TEXT, after_mode TEXT, after_oid TEXT,
        added INTEGER, removed INTEGER, PRIMARY KEY(record,path));
      CREATE TABLE IF NOT EXISTS runs(run_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, request_id TEXT NOT NULL, harness TEXT NOT NULL,
        state TEXT NOT NULL, last_seq INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, finished_at INTEGER);
      CREATE INDEX IF NOT EXISTS runs_thread ON runs(thread_id, created_at DESC);`);
    for (const [table, column, type] of columns) {
      if (!this.db.prepare(`PRAGMA table_info(${table})`).all().some(row => row.name === column)) this.db.exec(`ALTER TABLE ${table} ADD COLUMN ${column} ${type}`);
    }
    this.db.exec(`CREATE TABLE IF NOT EXISTS review_checkouts(id TEXT PRIMARY KEY, canonical_root TEXT NOT NULL, root_dev TEXT NOT NULL, root_ino TEXT NOT NULL, catalog_revision INTEGER NOT NULL DEFAULT 0, UNIQUE(canonical_root,root_dev,root_ino));`);
    for (const operation of ['INSERT', 'UPDATE', 'DELETE']) {
      const row = operation === 'DELETE' ? 'OLD' : 'NEW';
      this.db.exec(`CREATE TRIGGER IF NOT EXISTS review_records_${operation} AFTER ${operation} ON records BEGIN
        UPDATE review_checkouts SET catalog_revision=catalog_revision+1 WHERE canonical_root=${row}.checkout
          AND root_dev=(SELECT json_extract(identity,'$.dev') FROM roots WHERE id=${row}.thread)
          AND root_ino=(SELECT json_extract(identity,'$.ino') FROM roots WHERE id=${row}.thread); END;`);
    }
    this.db.exec('CREATE INDEX IF NOT EXISTS threads_recent ON threads(updated_at DESC)');
    this.outputs = new CommandOutputs(this.db, { threadOutputsDir: directory });
    this.images = new Images(this.db);
    this.linear = new LinearCache(this.db);
    // Repair queued cancellations made by earlier image-capable hosts, without reading BLOBs.
    this.transaction(() => {
      for (const row of this.db.prepare(`SELECT message_images.thread,message_images.request FROM message_images
        JOIN requests ON requests.scope=message_images.thread AND requests.id=message_images.request WHERE requests.state='cancelled'`).all()) {
        this.images.detach(String(row.thread), String(row.request));
      }
    });
    this.backfill();
    this.waiters = new CheckoutQueue(this);
  }

  /** Threads saved before version 3 gain summary columns, timestamps, and a derived title once. */
  private backfill() {
    for (const row of this.db.prepare('SELECT snapshot FROM threads WHERE checkout IS NULL').all()) {
      const snapshot = JSON.parse(String(row.snapshot)) as Snapshot;
      const first = this.db.prepare('SELECT input FROM requests WHERE scope=? AND input IS NOT NULL ORDER BY rowid LIMIT 1').get(snapshot.id);
      const text = first ? JSON.parse(String(first.input)) : undefined;
      if (!snapshot.title && typeof text === 'string') snapshot.title = title(text);
      this.save(snapshot);
    }
  }
  recoverable(): Snapshot[] {
    return this.db.prepare(`SELECT snapshot FROM threads t WHERE status IN ('running','awaiting_approval')
      OR EXISTS (SELECT 1 FROM runs WHERE thread_id=t.id AND finished_at IS NULL)`).all()
      .map(row => JSON.parse(String(row.snapshot)) as Snapshot);
  }
  /** Match the unfinished row to the latest accepted input, excluding queued and cancelled requests. */
  closeRecoveredRuns(snapshot: Snapshot) {
    const user = snapshot.messages.findLast(message => message.role === 'user');
    // The transcript can be clipped. Use the durable input position and use the
    // visible request to break ties between admissions at the same position.
    const inputs = this.db.prepare(`SELECT id,entry,COALESCE(entry,anchor,0) AS position,entry IS NULL AS pending
      FROM requests WHERE scope=? AND state IS NULL AND input IS NOT NULL
      ORDER BY position DESC,pending DESC,('request-' || id=?) DESC LIMIT 2`).all(snapshot.id, user?.id ?? '');
    const [latest, prior] = inputs;
    const tied = prior && latest.position === prior.position && latest.pending === prior.pending;
    const matches = latest && (user ? user.id === `request-${latest.id}` : !tied);
    // A placed input belongs to the most recent turn start at or before its entry.
    // A direct admission starts a turn even if its before record is missing.
    const owner = matches && latest.entry !== null && this.db.prepare(`SELECT r.run_request FROM records r
      JOIN requests q ON q.scope=r.thread AND q.id=r.run_request
      WHERE r.thread=? AND r.kind='before' AND q.entry<=?
        AND NOT EXISTS (SELECT 1 FROM records a WHERE a.before=r.id AND a.kind='undo')
        AND NOT EXISTS (SELECT 1 FROM requests n
          WHERE n.scope=q.scope AND n.state IS NULL AND n.input IS NOT NULL AND n.entry>q.entry AND n.entry<=?
            AND (NOT EXISTS (SELECT 1 FROM checkout_waiters w WHERE w.thread=n.scope AND w.request=n.id AND w.kind='follow_up')
              OR EXISTS (SELECT 1 FROM records b WHERE b.thread=n.scope AND b.run_request=n.id AND b.kind='before')))
      ORDER BY q.entry DESC LIMIT 1`).get(snapshot.id, latest.entry, latest.entry);
    const current = matches && this.db.prepare(`SELECT run_id FROM runs WHERE thread_id=?
      AND request_id=? AND finished_at IS NULL`).get(snapshot.id, owner ? owner.run_request : latest.id);
    if (current) this.saveRunEvent(String(current.run_id), 0, snapshot, { state: 'dead', finished: true });
    else this.save(snapshot);
    // A rollback may have left other admissions open. Their snapshots are gone.
    const closed = this.db.prepare("UPDATE runs SET state='dead',finished_at=? WHERE thread_id=? AND finished_at IS NULL")
      .run(Date.now(), snapshot.id);
    if (closed.changes) this.db.prepare('UPDATE threads SET records_revision=records_revision+1 WHERE id=?').run(snapshot.id);
  }
  createRun(runId: string, threadId: string, requestId: string, harness: string): void {
    this.db.prepare('INSERT OR IGNORE INTO runs (run_id,thread_id,request_id,harness,state,last_seq,created_at) VALUES (?,?,?,?,?,0,?)')
      .run(runId, threadId, requestId, harness, 'preparing', Date.now());
  }
  run(runId: string): { run_id: string; thread_id: string; request_id: string; harness: string; state: string; last_seq: number; created_at: number; finished_at: number | null } | undefined {
    const row = this.db.prepare('SELECT run_id,thread_id,request_id,harness,state,last_seq,created_at,finished_at FROM runs WHERE run_id=?').get(runId);
    if (!row) return undefined;
    return {
      run_id: String(row.run_id), thread_id: String(row.thread_id), request_id: String(row.request_id), harness: String(row.harness),
      state: String(row.state), last_seq: Number(row.last_seq ?? 0), created_at: Number(row.created_at ?? 0),
      finished_at: row.finished_at === null ? null : Number(row.finished_at),
    };
  }
  private rememberOutcome(runId: string, status: Snapshot['status']) {
    const outcome = { idle: 'finished', error: 'failed', stopped: 'stopped', interrupted: 'interrupted',
      running: null, awaiting_approval: null }[status];
    if (!outcome) throw new Error('A terminal run needs a terminal snapshot.');
    this.db.prepare('UPDATE runs SET outcome=? WHERE run_id=? AND finished_at IS NULL AND outcome IS NULL').run(outcome, runId);
  }
  /** Persist snapshot, optional side effects, and run sequence in one transaction. */
  saveRunEvent(runId: string, seq: number, snapshot: Snapshot, options: { head?: number; state?: string; finished?: boolean; extra?: () => void } = {}): boolean {
    return this.transaction(() => {
      options.extra?.();
      const changed = this.save(snapshot, options.head ?? 0);
      if (snapshot.status === 'stopped' || snapshot.status === 'interrupted' || options.finished) this.rememberOutcome(runId, snapshot.status);
      if (options.finished) {
        const ended = this.db.prepare('UPDATE runs SET finished_at=? WHERE run_id=? AND finished_at IS NULL').run(Date.now(), runId);
        if (ended.changes) this.db.prepare('UPDATE threads SET records_revision=records_revision+1 WHERE id=?').run(snapshot.id);
      }
      this.db.prepare('UPDATE runs SET last_seq=MAX(last_seq,?), state=COALESCE(?,state) WHERE run_id=?')
        .run(seq, options.state ?? null, runId);
      return changed;
    });
  }
  advanceRunSeq(runId: string, seq: number): void {
    this.db.prepare('UPDATE runs SET last_seq=MAX(last_seq,?) WHERE run_id=?').run(seq, runId);
  }
  list(checkout: string) {
    return this.db.prepare('SELECT id,checkout,model,status FROM threads WHERE checkout=? ORDER BY rowid DESC LIMIT 100').all(checkout)
      .map(row => ({ id: String(row.id), checkout: String(row.checkout), model: JSON.parse(String(row.model)), status: String(row.status) }));
  }
  /** Version 3 listing: most recently updated first, across checkouts unless one is given. */
  summaries(options: { checkout?: string; limit: number; cursor?: string; archived: boolean }) {
    const where = ['1=1']; const values: (string | number)[] = [];
    if (options.checkout !== undefined) { where.push('checkout=?'); values.push(options.checkout); }
    if (!options.archived) where.push('archived=0');
    if (options.cursor !== undefined) {
      const [updated, rowid] = options.cursor.split(':').map(Number);
      where.push('(updated_at<? OR (updated_at=? AND rowid<?))'); values.push(updated, updated, rowid);
    }
    const rows = this.db.prepare(`SELECT rowid,id,checkout,model,status,title,archived,pending,created_at,updated_at,harness FROM threads WHERE ${where.join(' AND ')} ORDER BY updated_at DESC, rowid DESC LIMIT ?`)
      .all(...values, options.limit + 1);
    const page = rows.slice(0, options.limit); const last = page.at(-1);
    return { threads: page.map(summary), next_cursor: rows.length > options.limit && last ? `${last.updated_at}:${last.rowid}` : null };
  }
  get(id: string): Snapshot {
    const row = this.db.prepare('SELECT snapshot FROM threads WHERE id=?').get(id);
    if (!row) fail('not_found', 'Thread not found.');
    return JSON.parse(String(row.snapshot));
  }
  /** Saves the snapshot and its summary columns; reports whether the summary changed. */
  save(snapshot: Snapshot, head = 0): boolean {
    const previous = this.db.prepare('SELECT status,title,archived,pending,model,updated_at FROM threads WHERE id=?').get(snapshot.id);
    const now = Date.now();
    snapshot.created_at ??= now;
    // A status change is activity; streaming text within one status is not.
    if (snapshot.updated_at === undefined || (previous && previous.status !== null && previous.status !== snapshot.status)) snapshot.updated_at = now;
    const next = {
      status: snapshot.status, title: snapshot.title ?? '', archived: snapshot.archived ? 1 : 0, pending: snapshot.pending_approval ? 1 : 0,
      model: JSON.stringify(snapshot.model), updated_at: snapshot.updated_at,
    };
    // A thread never changes harness, so its column is written once and never compared.
    const harness = snapshot.harness && snapshot.harness.id !== 'pi' ? snapshot.harness.id : null;
    this.db.prepare(`INSERT INTO threads (id,snapshot,head,checkout,status,model,title,archived,pending,created_at,updated_at,harness) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
      ON CONFLICT(id) DO UPDATE SET snapshot=excluded.snapshot,head=MAX(threads.head,excluded.head),checkout=excluded.checkout,status=excluded.status,
      model=excluded.model,title=excluded.title,archived=excluded.archived,pending=excluded.pending,created_at=excluded.created_at,updated_at=excluded.updated_at,harness=excluded.harness`)
      .run(snapshot.id, JSON.stringify(boundSnapshot(snapshot)), head, snapshot.checkout, next.status, next.model, next.title, next.archived, next.pending, snapshot.created_at, next.updated_at, harness);
    return !previous || (Object.keys(next) as (keyof typeof next)[]).some(key => previous[key] !== next[key]);
  }
  private input(id: string, row: Row): InputRecord {
    const record = input(row); const images = this.images.forRequest(id, record.request_id);
    return images.length ? { ...record, images } : record;
  }
  inputs(id: string): InputRecord[] {
    return this.db.prepare("SELECT id,input,entry,anchor,state,queue FROM requests WHERE scope=? AND input IS NOT NULL AND (state IS NULL OR state='queued') ORDER BY rowid DESC LIMIT 100").all(id).reverse().map(row => this.input(id, row));
  }
  place(id: string, requestId: string, entry: number) {
    // A steering input the run took leaves the queue in the same write.
    this.transaction(() => {
      this.db.prepare('UPDATE requests SET entry=?,state=NULL,queue=NULL WHERE scope=? AND id=?').run(entry, id, requestId);
      this.waiters.set(id, requestId, 'started');
    });
  }
  /** Inputs waiting behind a run, oldest first. */
  queue(id: string): InputRecord[] {
    return this.db.prepare("SELECT id,input,entry,anchor,state,queue FROM requests WHERE scope=? AND state='queued' ORDER BY rowid").all(id).map(row => this.input(id, row));
  }
  waitingInput(id: string, request: string): InputRecord {
    const row = this.db.prepare("SELECT id,input,entry,anchor,state,queue FROM requests WHERE scope=? AND id=? AND state IN ('queued','checkout_queued')").get(id,request);
    if (!row) fail('not_found','Waiting message already started or was cancelled.');
    return this.input(id,row);
  }
  private head(id: string) { return Number(this.db.prepare('SELECT head FROM threads WHERE id=?').get(id)?.head ?? 0); }
  transaction<T>(work: () => T): T {
    if (this.txDepth > 0) {
      const sp = `sp_${this.txDepth}`;
      this.db.exec(`SAVEPOINT ${sp}`);
      this.txDepth++;
      try {
        const result = work();
        this.txDepth--;
        this.db.exec(`RELEASE SAVEPOINT ${sp}`);
        return result;
      } catch (error) {
        this.txDepth--;
        this.db.exec(`ROLLBACK TO SAVEPOINT ${sp}; RELEASE SAVEPOINT ${sp}`);
        throw error;
      }
    }
    this.db.exec('BEGIN IMMEDIATE');
    this.txDepth = 1;
    try {
      const result = work();
      this.txDepth = 0;
      this.db.exec('COMMIT');
      return result;
    } catch (error) {
      this.txDepth = 0;
      this.db.exec('ROLLBACK');
      throw error;
    }
  }
  private refreshQueue(snapshot: Snapshot) { snapshot.queued = this.queue(snapshot.id).map(queuedView); }
  /** Accepts an input behind the active run. It joins the conversation only when it starts. */
  enqueue(id: string, text: string, result: unknown, queued: NonNullable<InputRecord['queued']>, snapshot: Snapshot, images: string[] = [], checkoutWait = false) {
    const bytes = Buffer.byteLength(text) + this.images.refs(snapshot.id, images).reduce((n, image) => n + image.size, 0);
    this.waiters.check(snapshot, bytes);
    return this.transaction(() => {
      this.db.prepare("INSERT INTO requests (scope,id,hash,result,input,anchor,state,queue) VALUES (?,?,?,?,?,?,'queued',?)")
        .run(snapshot.id, id, digest(images.length ? { text, images } : text), JSON.stringify(result), JSON.stringify(text), this.head(snapshot.id), JSON.stringify(queued));
      this.images.attach(snapshot.id, id, images);
      this.waiters.add(snapshot, id, queued.approval, checkoutWait ? 'checkout' : 'follow_up', bytes);
      if (checkoutWait) {
        this.db.prepare("UPDATE requests SET state='checkout_queued' WHERE scope=? AND id=?").run(snapshot.id,id);
        snapshot.updated_at = Date.now();
      }
      if (!snapshot.title && !snapshot.renamed) snapshot.title = title(text);
      this.refreshQueue(snapshot);
      return this.save(snapshot);
    });
  }
  /** Moves a queued input into the conversation at the current position, as its run starts. */
  dequeue(input: InputRecord, snapshot: Snapshot) {
    return this.transaction(() => {
      const head = this.head(snapshot.id);
      const changed = this.db.prepare("UPDATE requests SET state=NULL,queue=NULL,anchor=? WHERE scope=? AND id=? AND state IN ('queued','checkout_queued')").run(head, snapshot.id, input.request_id);
      if (!changed.changes) fail('not_found', 'Queued message already started or was cancelled.');
      this.waiters.set(snapshot.id,input.request_id,'started');
      pushMessage(snapshot, inputMessage(input));
      if (input.text.length > 4096) snapshot.truncated = true;
      this.refreshQueue(snapshot);
      return this.save(snapshot);
    });
  }
  /** Cancels one queued input, or all of them, and returns how many. The request IDs stay used. */
  cancel(id: string, requestId?: string): number {
    return this.transaction(() => {
      const changed = requestId === undefined
        ? this.db.prepare("UPDATE requests SET state='cancelled',queue=NULL WHERE scope=? AND state IN ('queued','checkout_queued') RETURNING id").all(id)
        : this.db.prepare("UPDATE requests SET state='cancelled',queue=NULL WHERE scope=? AND id=? AND state IN ('queued','checkout_queued') RETURNING id").all(id, requestId);
      for (const row of changed) { this.images.detach(id, String(row.id)); this.waiters.set(id,String(row.id),'cancelled'); }
      return changed.length;
    });
  }
  /** Accepted inputs that never reached the conversation store, newest first, below a history position. */
  pendingBefore(id: string, anchor: number, rowid: number, limit: number): PendingInput[] {
    return this.db.prepare(`SELECT rowid,id,input,COALESCE(anchor,0) AS anchor FROM requests WHERE scope=? AND input IS NOT NULL AND entry IS NULL AND state IS NULL
      AND (COALESCE(anchor,0)<? OR (COALESCE(anchor,0)=? AND rowid<?)) ORDER BY COALESCE(anchor,0) DESC, rowid DESC LIMIT ?`).all(id, anchor, anchor, rowid, limit)
      .map(row => ({ rowid: Number(row.rowid), request_id: String(row.id), text: JSON.parse(String(row.input)), anchor: Number(row.anchor), images: this.images.forRequest(id, String(row.id)) }));
  }
  /**
   * The inputs placed at the given conversation entries: request ID and, for an input
   * that reads as typed (a `/skill:` command), that text rather than its expansion.
   */
  placed(id: string, low: number, high: number): Map<number, Placed> {
    return new Map(this.db.prepare('SELECT id,entry,input FROM requests WHERE scope=? AND entry BETWEEN ? AND ?').all(id, low, high)
      .map(row => {
        const text = row.input === null ? undefined : JSON.parse(String(row.input)) as string;
        return [Number(row.entry), { request_id: String(row.id), images: this.images.forRequest(id, String(row.id)), ...(readsAsTyped(text) ? { text } : {}) }];
      }));
  }
  journal(id: string, rowid: number): string | undefined {
    const row = this.db.prepare('SELECT input FROM requests WHERE scope=? AND rowid=? AND input IS NOT NULL AND entry IS NULL AND state IS NULL').get(id, rowid);
    return row ? JSON.parse(String(row.input)) : undefined;
  }
  journalHasImages(id: string, rowid: number): boolean {
    return !!this.db.prepare(`SELECT 1 FROM requests JOIN message_images
      ON message_images.thread=requests.scope AND message_images.request=requests.id
      WHERE requests.scope=? AND requests.rowid=?`).get(id, rowid);
  }
  owned(id: string): Postimage[] {
    return this.db.prepare('SELECT image FROM postimages WHERE thread_id=? LIMIT 500').all(id).map(row => JSON.parse(String(row.image)));
  }
  changedPaths(id: string): string[] {
    return this.db.prepare('SELECT path FROM postimages WHERE thread_id=? ORDER BY path LIMIT 500').all(id).map(row => String(row.path));
  }
  /** A file another harness wrote. It lists in changed files; it has no postimage, so no Pi run can claim it. */
  recordWrite(id: string, path: string) {
    this.db.prepare("INSERT INTO postimages VALUES (?,?,'null') ON CONFLICT(thread_id,path) DO NOTHING").run(id, path);
  }
  record(id: string, image: Postimage) {
    // FULL synchronous commits provenance before a tool reports completion.
    this.db.prepare('INSERT INTO postimages VALUES (?,?,?) ON CONFLICT(thread_id,path) DO UPDATE SET image=excluded.image')
      .run(id, image.path, JSON.stringify(image));
  }
  async refreshRootIdentity(id: string, root: string): Promise<FileIdentity> {
    const saved = this.rootIdentity(id);
    const current = await checkRoot(root, saved);
    if (saved.dev !== current.dev) {
      this.transaction(() => {
        // Do not overwrite a newer identity observed by another operation.
        const changed = this.db.prepare('UPDATE roots SET identity=? WHERE id=? AND identity=?')
          .run(JSON.stringify(current), id, JSON.stringify(saved));
        if (changed.changes !== 1) fail('permission_denied', 'Checkout identity changed while refreshing.');
      });
    }
    return current;
  }
  rootIdentity(id: string): FileIdentity {
    const row = this.db.prepare('SELECT identity FROM roots WHERE id=?').get(id);
    if (!row) fail('permission_denied', 'Thread has no saved checkout identity; create a new thread.');
    return JSON.parse(String(row.identity));
  }
  defaultModel(): ModelChoice | null {
    const row = this.db.prepare("SELECT value FROM settings WHERE key='default_model'").get();
    return row ? JSON.parse(String(row.value)) : null;
  }
  setDefault(model: ModelChoice) {
    this.db.prepare("INSERT INTO settings VALUES ('default_model',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").run(JSON.stringify(model));
  }
  setting(key: string): string | null {
    const row = this.db.prepare('SELECT value FROM settings WHERE key=?').get(key);
    return row ? String(row.value) : null;
  }
  setSetting(key: string, value: string | null) {
    if (value === null) this.db.prepare('DELETE FROM settings WHERE key=?').run(key);
    else this.db.prepare('INSERT INTO settings VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value').run(key, value);
  }
  forget(id: string, path: string) {
    this.db.prepare('DELETE FROM postimages WHERE thread_id=? AND path=?').run(id, path);
  }
  duplicate(scope: string, id: string, input: unknown): unknown | undefined {
    const row = this.db.prepare('SELECT hash,result FROM requests WHERE scope=? AND id=?').get(scope, id);
    if (!row) return undefined;
    if (row.hash !== digest(input)) fail('conflict', 'Request ID was already used for different input.');
    return JSON.parse(String(row.result));
  }
  admit(scope: string, id: string, input: unknown, result: unknown, snapshot: Snapshot, root?: FileIdentity, images: string[] = [], vcs?: 'git' | 'folder') {
    // Acceptance and dedup commit before the harness can execute any effect.
    return this.transaction(() => {
      const head = this.head(snapshot.id);
      if (scope === snapshot.id && typeof input === 'string') {
        pushMessage(snapshot, inputMessage({ request_id: id, text: input, images: this.images.refs(snapshot.id, images), entry: null, anchor: head }));
        if (input.length > 4096) snapshot.truncated = true;
        // The first accepted message names the thread until the user renames it.
        if (!snapshot.renamed && !snapshot.title) snapshot.title = title(input);
      }
      const changed = this.save(snapshot);
      if (root) this.db.prepare('INSERT INTO roots VALUES (?,?)').run(snapshot.id, JSON.stringify(root));
      if (vcs) this.db.prepare('INSERT INTO thread_vcs VALUES (?,?)').run(snapshot.id, vcs);
      this.db.prepare('INSERT INTO requests (scope,id,hash,result,input,anchor) VALUES (?,?,?,?,?,?)').run(scope, id, digest(images.length ? { text: input, images } : input), JSON.stringify(result), JSON.stringify(input), head);
      this.images.attach(snapshot.id, id, images);
      return changed;
    });
  }
  close() { this.db.close(); }
}
export function digest(input: unknown) { return createHash('sha256').update(JSON.stringify(input)).digest('hex'); }

export function threadKind(store: Store, id: string): 'git' | 'folder' {
  return store.db.prepare('SELECT vcs FROM thread_vcs WHERE thread=?').get(id)?.vcs === 'folder' ? 'folder' : 'git';
}
