// The host's records of a checkout: one before and one after every run, in
// every harness. `plait-host record` writes each one into the host's own store
// (`STORE/records`) and never into the person's repository; this file decides
// when, keeps the list in `host.sqlite`, exposes version 5 views and undo,
// and prunes what has expired.
import { CleanupProof, RESTART_REASON, cleanupHelper, guardianProof, resolveProvedCleanup } from './cleanup.ts';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { readFile, rm, writeFile } from 'node:fs/promises';
import { recordsPage, undoReply } from './record-views.ts';
import { join, resolve } from 'node:path';
import { HostError, fail } from './protocol.ts';
import type { Snapshot } from './protocol.ts';
import type { Store } from './store.ts';
import { acquireCheckout, checkRoot } from './safety.ts';
import type { LeaseHandle } from './safety.ts';

/** Records are kept this long, those of archived threads included. */
export const RETENTION_MS = 30 * 24 * 60 * 60 * 1000;
const PRUNE_INTERVAL_MS = 24 * 60 * 60 * 1000;
/** A checkout's first record reads every file; later ones take a fraction of a second. */
const RECORD_TIMEOUT_MS = 300_000;
/** A run that rewrote many files lists each one; the reply is bounded all the same. */
const REPLY_LIMIT = 256 * 1024 * 1024;
/** Record IDs per prune request, well inside the helper's 1 MiB request limit. */
const PRUNE_BATCH = 5000;
export const RECORD_FAILED = 'The host could not record the checkout before this run, so the run did not start.';

type Uncovered = { path: string; reason: string };
type ChangedPath = {
  path: string; before_mode: string | null; before_oid: string | null; after_mode: string | null; after_oid: string | null;
  added: number | null; removed: number | null;
};
/** What `plait-host record` replies. */
type Made = {
  commit: string; tree: string; object_format: string; head: string | null; head_ref: string | null;
  not_covered: Uncovered[]; not_covered_total: number; paths: ChangedPath[] | null; elapsed_ms: number;
};

/** Runs one `plait-host` record helper: JSON in on stdin, JSON out on stdout. */
export function helper<T>(operation: 'record' | 'prune' | 'undo' | 'review', input: unknown, lease?: LeaseHandle | number, timeout = RECORD_TIMEOUT_MS): Promise<T> {
  const executable = process.env.PLAIT_HOST_EXE;
  if (!executable) return Promise.reject(new Error('Host record helper is not configured.'));
  const writerFd = typeof lease === 'number' ? lease : lease?.fd;
  const lifecycleFd = typeof lease === 'number' ? undefined : lease?.lifecycleFd;
  return new Promise<T>((resolve, reject) => {
    // The store lock travels with the helper, as with every helper: a new host
    // cannot start while one still writes the store. Its own process group lets a
    // timeout stop the sandboxed copy it starts as well.
    const child = spawn(executable, [operation], {
      env: { ...process.env, PLAIT_CLEANUP_RECEIPT_FD: '8' },
      stdio: ['pipe', 'pipe', 'pipe', process.env.PLAIT_LOCK_FD ? Number(process.env.PLAIT_LOCK_FD) : 'ignore', writerFd ?? 'ignore',
        'ignore', 'ignore', lifecycleFd ?? 'ignore', 'pipe'],
      detached: true,
    });
    if (typeof lease === 'object') lease.cleanup?.track(guardianProof(child), 'The checkout record helper did not confirm its process group stopped.');
    const chunks: Buffer[] = []; let size = 0; let diagnostic = '';
    const stop = () => { try { process.kill(-child.pid!, 'SIGKILL'); } catch { child.kill('SIGKILL'); } };
    const timer = setTimeout(stop, timeout);
    child.stdin!.on('error', () => {});
    child.stdin!.end(JSON.stringify(input));
    child.stdout!.on('data', (chunk: Buffer) => { size += chunk.length; if (size > REPLY_LIMIT) stop(); else chunks.push(chunk); });
    child.stderr!.setEncoding('utf8');
    child.stderr!.on('data', data => { diagnostic = (diagnostic + data).slice(-4096); });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', code => {
      clearTimeout(timer);
      if (operation === 'undo' || operation === 'review') stop(); // Reclaim a stray Git descendant too.
      // The diagnostic stays inside the runtime; clients only ever see a fixed message.
      if (code !== 0 || size > REPLY_LIMIT) return reject(new Error(diagnostic.trim() || `plait-host ${operation} failed`));
      try { resolve(JSON.parse(Buffer.concat(chunks).toString('utf8'))); } catch { reject(new Error(`plait-host ${operation} replied badly`)); }
    });
  });
}

/** Captures immutable artifacts; indexing the result remains the caller's job. */
function capture(directory: string, checkout: string, id: string, before: string | undefined, undo: boolean, lease?: LeaseHandle) {
  return helper<Made>('record', { checkout, store: directory, id, ...(before ? { before } : {}), ...(undo ? { undo: true } : {}) }, lease);
}

type UndoOutcome = { state: 'blocked' | 'applying' | 'undone' | 'incomplete'; applied: number; changed_since: string[]; not_covered: Uncovered[]; before: Made; after: Made | null };

export class Records {
  private store: Store;
  /** The host store; the helper keeps records in its `records` directory. */
  private directory: string;
  /** Records in one checkout are made one at a time, in the order asked. */
  private tails = new Map<string, Promise<unknown>>();
  /** Records being made now; a prune waits for them and holds new ones back. */
  private running = new Set<Promise<unknown>>();
  private pruning?: Promise<void>;
  private pending = new Set<Promise<unknown>>();
  private timer?: NodeJS.Timeout;
  private closing = false;

  private notify: (id: string) => void;
  constructor(store: Store, directory: string, notify: (id: string) => void = () => {}) {
    this.store = store; this.directory = resolve(directory); this.notify = notify;
  }
  /** Takes any record a host stop left unmade, then prunes now and once a day. */
  start() {
    const recovery = this.recoverUndos();
    for (const row of this.store.db.prepare('SELECT DISTINCT r.checkout FROM undo_operations u JOIN records r ON r.id=u.before_id WHERE u.result IS NULL').all()) {
      const checkout = String(row.checkout);
      const tail = recovery.catch(() => {});
      this.tails.set(checkout, tail);
      void tail.then(() => { if (this.tails.get(checkout) === tail) this.tails.delete(checkout); });
    }
    this.track(recovery.then(() => { this.recover(); return this.prune(); }).catch(() => {}));
    this.timer = setInterval(() => void this.prune().catch(() => {}), PRUNE_INTERVAL_MS);
    this.timer.unref();
  }
  /** Admission can defer behind startup recovery without blocking the socket reply. */
  busy(checkout: string) { return this.tails.has(checkout); }
  async settled(checkout: string) { await this.tails.get(checkout); }
  /**
   * The record a run starts from, made before any harness starts. A run that
   * cannot be recorded does not run: this throws `record_failed`.
   */
  async before(thread: string, run: string, checkout: string, lease?: LeaseHandle): Promise<string> {
    const id = randomUUID();
    try {
      this.insert(id, thread, run, 'before', checkout);
      this.ready(id, await this.queue(checkout, async () => {
        await this.recoverBefore(checkout, lease);
        return this.make(checkout, id, undefined, lease);
      }));
      return id;
    } catch {
      try { this.mark(id, 'failed'); } catch { /* The database itself may be unavailable. */ }
      throw new HostError('record_failed', RECORD_FAILED);
    }
  }
  /**
   * The record a run ends at, after every outcome, linked to its `before`
   * record. It never throws: a run that ends has ended, and a failed record is
   * only marked so that undo knows it cannot rely on it.
   */
  async after(before: string, lease?: LeaseHandle, restart = false): Promise<void> {
    let id: string | undefined;
    try {
      const row = this.store.db.prepare("SELECT thread,run_request,checkout,commit_id FROM records WHERE id=? AND kind='before' AND state='ready'").get(before);
      if (!row) return;
      const checkout = String(row.checkout);
      const existing = this.store.db.prepare("SELECT id FROM records WHERE before=? AND kind IN ('after','undo')").get(before);
      id = existing ? String(existing.id) : randomUUID();
      if (existing) this.store.db.prepare("UPDATE records SET state='recording',after_restart=?,created_at=? WHERE id=?").run(restart ? 1 : 0, Date.now(), id);
      else this.insert(id, String(row.thread), String(row.run_request), 'after', checkout, before, restart);
      const after = id;
      this.ready(id, await this.queue(checkout, async () => {
        const recoveryLease = restart ? await acquireCheckout(checkout, 'writer') : undefined;
        try { return await this.make(checkout, after, String(row.commit_id), recoveryLease ?? lease); }
        finally { await recoveryLease?.close(); }
      }));
    } catch {
      if (id) try { this.mark(id, 'failed'); } catch { /* The store is closing. */ }
    }
  }
  /**
   * A host that stopped mid-run never made that run's later record. It is made
   * now, before anything else in that checkout, and marked as taken after a
   * restart: it may include changes made while the host was down.
   */
  recoverCheckout(checkout: string) { this.recover(checkout); }
  private recover(checkout?: string) {
    const db = this.store.db;
    const open = db.prepare(`SELECT b.id FROM records b WHERE b.kind='before' AND b.state='ready'
      AND NOT EXISTS (SELECT 1 FROM records a WHERE a.kind IN ('after','undo') AND a.before=b.id AND (a.state IN ('ready','undone','no_effect') OR (a.state='failed' AND a.after_restart=0))) ORDER BY b.created_at, b.rowid`).all().map(row => String(row.id));
    // A record cut short by the stop left nothing usable; the before record of a run that never started its harness stays failed.
    db.prepare("UPDATE records SET state='failed' WHERE kind='before' AND state='recording'").run();
    for (const before of open) {
      if (checkout && this.store.db.prepare('SELECT checkout FROM records WHERE id=?').get(before)?.checkout !== checkout) continue;
      this.track(this.after(before, undefined, true));
    }
  }
  /** A busy checkout may have deferred startup recovery until its next leased run. */
  private async recoverBefore(checkout: string, lease?: LeaseHandle) {
    const pending = this.store.db.prepare(`SELECT a.id,b.commit_id FROM records a JOIN records b ON b.id=a.before
      WHERE a.checkout=? AND a.kind IN ('after','undo') AND a.state='failed' AND a.after_restart=1 AND b.state='ready'`).all(checkout);
    for (const row of pending) {
      const id = String(row.id);
      this.store.db.prepare("UPDATE records SET state='recording',created_at=? WHERE id=?").run(Date.now(), id);
      try { this.ready(id, await this.make(checkout, id, String(row.commit_id), lease)); }
      catch { this.mark(id, 'failed'); }
    }
  }
  /**
   * Drops records older than `RETENTION_MS`, and the store's copies of
   * checkouts that no longer exist. No record is made while it runs, so Git's
   * prune never sees an object a record has written but not yet named.
   */
  prune(now = Date.now()): Promise<void> {
    if (this.pruning) return this.pruning;
    if (this.closing || !existsSync(join(this.directory, 'records'))) return Promise.resolve();
    this.pruning = (async () => {
      await Promise.allSettled([...this.running]);
      const expired = this.store.db.prepare(`SELECT id FROM records r WHERE created_at<? AND state<>'recording'
        AND NOT EXISTS (SELECT 1 FROM records pair WHERE pair.thread=r.thread AND pair.run_request=r.run_request
          AND (pair.created_at>=? OR pair.state='recording'))
        AND NOT (kind='before' AND state='ready' AND NOT EXISTS (SELECT 1 FROM records a WHERE a.before=r.id))
        ORDER BY created_at`).all(now - RETENTION_MS, now - RETENTION_MS).map(row => String(row.id));
      expired.push(...this.store.db.prepare('SELECT id FROM record_garbage').all().map(row => String(row.id)));
      // One request even when nothing expired, so checkouts that are gone are dropped.
      for (let start = 0; start === 0 || start < expired.length; start += PRUNE_BATCH) {
        const batch = expired.slice(start, start + PRUNE_BATCH);
        await helper('prune', { store: this.directory, remove: batch });
        this.forget(batch);
        for (const id of batch) this.store.db.prepare('DELETE FROM record_garbage WHERE id=?').run(id);
      }
    })().finally(() => { this.pruning = undefined; });
    this.track(this.pruning.catch(() => {}));
    return this.pruning;
  }
  /** Waits for every record and prune under way, and stops the daily prune. */
  async close() {
    this.closing = true;
    clearInterval(this.timer);
    while (this.pending.size) await Promise.allSettled([...this.pending]);
  }

  revision(thread: string) {
    return Number(this.store.db.prepare('SELECT records_revision FROM threads WHERE id=?').get(thread)?.records_revision ?? 0);
  }
  page(thread: string, before?: string, limit?: number) { return recordsPage(this.store, thread, before, limit); }
  changedPaths(thread: string) {
    return this.store.db.prepare(`SELECT path FROM postimages WHERE thread_id=? UNION SELECT p.path FROM record_paths p JOIN records r ON r.id=p.record
      WHERE r.thread=? ORDER BY path LIMIT 500`).all(thread, thread).map(row => String(row.path));
  }
  private bump(thread: string) {
    this.store.db.prepare('UPDATE threads SET records_revision=records_revision+1 WHERE id=?').run(thread); this.notify(thread);
  }
  private bumpRecord(id: string) {
    const row = this.store.db.prepare('SELECT thread FROM records WHERE id=?').get(id);
    if (row) this.bump(String(row.thread));
  }
  /** Only applied results deduplicate; blocked/preflight failures may be retried. */
  async undo(snapshot: Snapshot, target: string, requestId: string, leaveChanged: boolean, active: boolean) {
    const thread = snapshot.id;
    const existing = this.store.db.prepare('SELECT * FROM undo_operations WHERE thread=? AND request_id=?').get(thread, requestId);
    if (existing) {
      if (existing.target !== target || existing.leave_changed !== Number(leaveChanged)) fail('conflict', 'Undo request ID already used for different input.');
      if (existing.result) return JSON.parse(String(existing.result));
      if (this.store.db.prepare("SELECT 1 FROM records WHERE id=? AND state='no_effect'").get(existing.record_id)) {
        fail('undo_no_effect', 'Undo could not restore any files. No files were changed; a new request may retry the original run.');
      }
      fail('undo_incomplete', 'This undo was interrupted and will not be repeated. Refresh the records and inspect the checkout.');
    }
    if (active || this.busy(snapshot.checkout) || this.pruning) fail('busy', 'This checkout already has a host writer.');
    const row = this.store.db.prepare(`SELECT a.*,b.commit_id AS before_commit FROM records a JOIN records b ON b.id=a.before
      WHERE a.id=? AND a.thread=? AND a.kind IN ('after','undo') AND a.state IN ('ready','undone') AND b.state='ready'`).get(target, thread);
    if (!row) fail('not_found', 'A complete record for this thread is not available.');
    const started = Date.now();
    return this.queue(snapshot.checkout, async () => {
      await checkRoot(snapshot.checkout, this.store.rootIdentity(thread));
      const lease = await acquireCheckout(snapshot.checkout, 'exclusive');
      const before = randomUUID(), id = randomUUID();
      lease.cleanup = new CleanupProof();
      let marked = false;
      try {
        await cleanupHelper('begin', snapshot.checkout, id, RESTART_REASON, lease);
        marked = true;
        this.transaction(() => {
          this.insert(before, thread, requestId, 'before', snapshot.checkout);
          this.insert(id, thread, requestId, 'undo', snapshot.checkout, before);
          this.store.db.prepare('UPDATE records SET undoes=? WHERE id=?').run(target, id);
          this.store.db.prepare('INSERT INTO undo_operations VALUES (?,?,?,?,?,?,NULL)').run(thread, requestId, target, Number(leaveChanged), before, id);
        });
        const paths = this.store.db.prepare('SELECT path,before_mode,before_oid,after_mode,after_oid,added,removed FROM record_paths WHERE record=? ORDER BY path').all(target);
        let outcome: UndoOutcome | undefined;
        try {
          await writeFile(this.undoPath(id).replace(/\.json$/, '.paths'), JSON.stringify({ paths, not_covered: JSON.parse(String(row.not_covered ?? '[]')) }), { mode: 0o600, flag: 'wx' });
          outcome = await helper<UndoOutcome>('undo', { checkout: snapshot.checkout, store: this.directory, before: row.before_commit,
            after: row.commit_id, before_id: before, id, identity: this.store.rootIdentity(thread), leave_changed: leaveChanged, paths_file: true }, lease, Math.max(1, 10_000 - (Date.now() - started)));
        } catch { outcome = await this.readUndo(id); }
        if (!outcome) {
          // No journal means no working-file writes were admitted.
          this.forget([before, id]);
          this.store.db.prepare('DELETE FROM undo_operations WHERE thread=? AND request_id=?').run(thread, requestId);
          fail('undo_failed', 'Undo could not finish its checks within the budget; no files were changed.');
        }
        this.acceptUndo(thread, requestId, target, before, id, outcome, row.after_restart === 1);
        if (outcome.state === 'blocked') {
          this.forget([before, id]);
          this.store.db.prepare('DELETE FROM undo_operations WHERE thread=? AND request_id=?').run(thread, requestId);
          await rm(this.undoPath(id), { force: true });
          return undoReply('blocked', outcome.changed_since, outcome.not_covered, target, row.after_restart === 1);
        }
        if (outcome.state === 'incomplete' && outcome.applied === 0) fail('undo_no_effect', 'Undo could not restore any files. No files were changed; a new request may retry the original run.');
        if (outcome.state !== 'undone') fail('undo_incomplete', 'Undo stopped after it may have changed files. Refresh the records and inspect the checkout; this request will not be repeated.');
        await rm(this.undoPath(id), { force: true });
        return undoReply('undone', outcome.changed_since, outcome.not_covered, id, row.after_restart === 1);
      } finally {
        try {
          await lease.cleanup.wait();
          const reason = lease.cleanup.reason();
          if (marked) await cleanupHelper(reason ? 'mark' : 'finish', snapshot.checkout, id, reason, lease);
          if (reason) void resolveProvedCleanup(lease.cleanup, snapshot.checkout, id).catch(() => {});
        } finally { await lease.close(); await rm(this.undoPath(id).replace(/\.json$/, '.paths'), { force: true }); }
      }
    });
  }
  private undoPath(id: string) { return join(this.directory, 'records', `undo-${id}.json`); }
  private async readUndo(id: string): Promise<UndoOutcome | undefined> {
    try { return JSON.parse(await readFile(this.undoPath(id), 'utf8')); } catch { return undefined; }
  }
  private acceptUndo(thread: string, request: string, target: string, before: string, id: string, outcome: UndoOutcome, restart: boolean) {
    this.ready(before, outcome.before);
    if (outcome.state === 'incomplete' && outcome.applied === 0) {
      // A terminal journal proves no file changed; keep dedup but no action child.
      this.store.db.prepare("UPDATE records SET state='no_effect' WHERE id=?").run(id);
      this.bump(thread);
      return;
    }
    if (outcome.after) this.ready(id, outcome.after);
    else this.store.db.prepare("UPDATE records SET state='failed',after_restart=1 WHERE id=?").run(id);
    if (outcome.state === 'undone') {
      const reply = undoReply('undone', outcome.changed_since, outcome.not_covered, id, restart);
      this.transaction(() => {
        this.store.db.prepare('UPDATE undo_operations SET result=? WHERE thread=? AND request_id=?').run(JSON.stringify(reply), thread, request);
        this.store.db.prepare("UPDATE records SET state='undone' WHERE id=?").run(target);
      });
      this.bump(thread);
    }
  }
  /** A saved applying journal is evidence of possible effects, never permission to replay. */
  private async recoverUndos() {
    for (const row of this.store.db.prepare('SELECT * FROM undo_operations WHERE result IS NULL AND EXISTS (SELECT 1 FROM records WHERE id=undo_operations.before_id)').all()) {
      const id = String(row.record_id), before = String(row.before_id);
      const outcome = await this.readUndo(id);
      if (!outcome || outcome.state === 'blocked') {
        this.forget([before, id]);
        this.store.db.prepare('DELETE FROM undo_operations WHERE thread=? AND request_id=?').run(row.thread, row.request_id);
      } else {
        const saved = this.store.db.prepare('SELECT state FROM records WHERE id=?').get(id);
        if (outcome.state !== 'undone' && (saved?.state === 'ready' || saved?.state === 'undone' || saved?.state === 'no_effect')) continue;
        const target = this.store.db.prepare('SELECT after_restart FROM records WHERE id=?').get(row.target);
        this.acceptUndo(String(row.thread), String(row.request_id), String(row.target), before, id, outcome, target?.after_restart === 1);
        if (outcome.state === 'undone') await rm(this.undoPath(id), { force: true });
      }
    }
  }

  private make(checkout: string, id: string, before: string | undefined, lease?: LeaseHandle) {
    const undo = this.store.db.prepare("SELECT 1 FROM records WHERE id=? AND kind='undo'").get(id) !== undefined;
    return capture(this.directory, checkout, id, before, undo, lease);
  }
  /** A read pins immutable record blobs against pruning without touching the recorder index. */
  read<T>(work: () => Promise<T>): Promise<T> {
    const waiting = this.pruning;
    const task = (async () => { await waiting; return work(); })();
    this.running.add(task); this.track(task.catch(() => {}));
    void task.finally(() => this.running.delete(task)).catch(() => {});
    return task;
  }
  private queue<T>(checkout: string, work: () => Promise<T>): Promise<T> {
    const turn = async () => {
      while (this.pruning) await this.pruning.catch(() => {});
      const task = work();
      this.running.add(task);
      try { return await task; } finally { this.running.delete(task); }
    };
    const result = (this.tails.get(checkout) ?? Promise.resolve()).then(turn, turn);
    const tail = result.catch(() => {});
    this.tails.set(checkout, tail);
    this.track(tail);
    void tail.then(() => { if (this.tails.get(checkout) === tail) this.tails.delete(checkout); });
    return result;
  }
  private track(work: Promise<unknown>) {
    this.pending.add(work);
    void work.finally(() => this.pending.delete(work));
  }
  private insert(id: string, thread: string, run: string, kind: 'before' | 'after' | 'undo', checkout: string, before?: string, restart = false) {
    this.store.db.prepare("INSERT INTO records (id,thread,run_request,kind,checkout,before,state,after_restart,created_at) VALUES (?,?,?,?,?,?,'recording',?,?)")
      .run(id, thread, run, kind, checkout, before ?? null, restart ? 1 : 0, Date.now());
    this.bump(thread);
  }
  private mark(id: string, state: 'failed') {
    this.store.db.prepare('UPDATE records SET state=? WHERE id=?').run(state, id);
    this.bumpRecord(id);
  }
  /** Saves a made record and, for a run's later record, every path the run changed. */
  private ready(id: string, made: Made) {
    this.transaction(() => {
      this.store.db.prepare("UPDATE records SET state='ready',commit_id=?,tree=?,head=?,head_ref=?,not_covered=?,not_covered_total=? WHERE id=?")
        .run(made.commit, made.tree, made.head, made.head_ref, JSON.stringify(made.not_covered), made.not_covered_total, id);
      this.store.db.prepare('DELETE FROM record_paths WHERE record=?').run(id);
      const path = this.store.db.prepare('INSERT INTO record_paths VALUES (?,?,?,?,?,?,?,?)');
      for (const changed of made.paths ?? []) {
        path.run(id, changed.path, changed.before_mode, changed.before_oid, changed.after_mode, changed.after_oid, changed.added, changed.removed);
      }
    });
    this.bumpRecord(id);
  }
  private forget(ids: string[]) {
    const paths = this.store.db.prepare('DELETE FROM record_paths WHERE record=?');
    const record = this.store.db.prepare('DELETE FROM records WHERE id=?');
    const threads = new Set(ids.map(id => this.store.db.prepare('SELECT thread FROM records WHERE id=?').get(id)?.thread).filter(Boolean));
    this.transaction(() => { for (const id of ids) { paths.run(id); record.run(id); this.store.db.prepare('INSERT OR IGNORE INTO record_garbage VALUES (?)').run(id); } });
    for (const thread of threads) this.bump(String(thread));
  }
  private transaction(work: () => void) {
    this.store.db.exec('BEGIN IMMEDIATE');
    try { work(); this.store.db.exec('COMMIT'); } catch (error) { this.store.db.exec('ROLLBACK'); throw error; }
  }
}
