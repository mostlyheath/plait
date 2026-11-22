// One durable order for every input which must acquire a checkout for a new run.
import type { Store } from './store.ts';
import { commandPolicy } from './command/policy.ts';
import { fail, validate, type ApprovalMode, type Failure, type Snapshot } from './protocol.ts';
import type { FileIdentity } from './safety.ts';

export const CHECKOUT_QUEUE_LIMIT = 32;
export const CHECKOUT_QUEUE_BYTES = 32 * 1024 * 1024;
export type WaitState = 'waiting' | 'held' | 'started' | 'cancelled' | 'failed';
export type Waiter = {
  thread_id: string; request_id: string; checkout: string; identity: FileIdentity;
  kind: 'checkout' | 'follow_up'; state: WaitState; order: number;
  settings: string; policy: string; approval: ApprovalMode; error: Failure | null;
};
export type QueueSummary = { request_id: string; state: 'waiting' | 'held'; position: number; count: number };
export const queueSettings = (snapshot: Snapshot) => JSON.stringify({ execution: 'write', model: snapshot.model, harness: snapshot.harness?.id ?? 'pi', mode: snapshot.harness?.mode ?? null, thinking: snapshot.thinking_level ?? null });
const pending = "state IN ('waiting','held')";
export function queueOperation(input: unknown): boolean {
  return !!input && typeof input === 'object' && ['checkout_queue_info','submit_checkout','checkout_queue','cancel_checkout_wait','resume_checkout_queue','watch_threads_queue','list_threads_queue'].includes(String((input as { op?: unknown }).op));
}
export function queueRequest(input: unknown): Record<string, any> {
  if (!queueOperation(input) || Array.isArray(input)) fail('invalid_request', 'Unknown checkout queue operation.');
  const value = input as Record<string, any>;
  const fields: Record<string,string[]> = {
    checkout_queue_info: [], submit_checkout: ['thread_id','request_id','text','approval','images','identity'],
    checkout_queue: ['thread_id'], cancel_checkout_wait: ['thread_id','request_id'], resume_checkout_queue: ['thread_id'],
    watch_threads_queue: ['archived'], list_threads_queue: ['archived','checkout','limit','cursor'],
  };
  if (Object.keys(value).some(key => key !== 'op' && !fields[value.op].includes(key))) fail('invalid_request', 'Unknown checkout queue field.');
  if (value.op === 'submit_checkout') validate({ ...value, op: 'submit', protocol_version: 6 });
  else if (value.op.endsWith('threads_queue')) validate({ ...value, op: value.op.replace('_queue',''), protocol_version: 6 });
  else for (const key of fields[value.op]) if (typeof value[key] !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(value[key])) fail('invalid_request','Invalid queue thread or request ID.');
  return value;
}

export class CheckoutQueue {
  private store: Store;
  /** Positions are in every summary, so any change to a pending row is a summary change. */
  changed: () => void = () => {};
  constructor(store: Store) {
    this.store = store;
    store.db.exec(`CREATE TABLE IF NOT EXISTS checkout_waiters(thread TEXT NOT NULL,request TEXT NOT NULL,checkout TEXT NOT NULL,identity TEXT NOT NULL,kind TEXT NOT NULL,state TEXT NOT NULL,ordering INTEGER NOT NULL,settings TEXT NOT NULL,policy TEXT NOT NULL,approval TEXT NOT NULL,bytes INTEGER NOT NULL,error TEXT,PRIMARY KEY(thread,request));
      CREATE INDEX IF NOT EXISTS checkout_waiters_order ON checkout_waiters(checkout,state,ordering);`);
    // Old follow-ups join the same order. Nothing left pending at startup runs itself.
    for (const row of store.db.prepare("SELECT scope,id FROM requests WHERE state='queued' AND NOT EXISTS (SELECT 1 FROM checkout_waiters WHERE thread=scope AND request=id)").all()) {
      const snapshot = store.get(String(row.scope));
      const input = store.queue(snapshot.id).find(input => input.request_id === row.id)!;
      this.add(snapshot, input.request_id, input.queued!.approval, 'follow_up', Buffer.byteLength(input.text) + (input.images ?? []).reduce((n, image) => n + image.size, 0));
    }
    store.db.exec("UPDATE checkout_waiters SET state='held' WHERE state='waiting'");
  }
  private decode(row: Record<string, unknown>): Waiter {
    return { thread_id: String(row.thread), request_id: String(row.request), checkout: String(row.checkout), identity: JSON.parse(String(row.identity)), kind: row.kind as Waiter['kind'], state: row.state as WaitState, order: Number(row.ordering), settings: String(row.settings), policy: String(row.policy), approval: row.approval as ApprovalMode, error: row.error ? JSON.parse(String(row.error)) : null };
  }
  get(thread: string, request: string): Waiter | undefined {
    const row = this.store.db.prepare('SELECT * FROM checkout_waiters WHERE thread=? AND request=?').get(thread,request);
    return row && this.decode(row);
  }
  receipts(thread: string, request = ''): Waiter[] {
    // Keep every pending receipt visible, plus the exact operation's result.
    return this.store.db.prepare(`SELECT * FROM checkout_waiters WHERE thread=? AND kind='checkout'
      ORDER BY (request=?) DESC,(${pending}) DESC,ordering DESC LIMIT 32`).all(thread,request)
      .map(row => this.decode(row)).sort((a,b) => a.order-b.order);
  }
  line(checkout: string): Waiter[] { return this.store.db.prepare(`SELECT * FROM checkout_waiters WHERE checkout=? AND ${pending} ORDER BY ordering`).all(checkout).map(row => this.decode(row)); }
  roots(): string[] { return this.store.db.prepare("SELECT DISTINCT checkout FROM checkout_waiters WHERE state='waiting'").all().map(row => String(row.checkout)); }
  check(snapshot: Snapshot, bytes: number) {
    const row = this.store.db.prepare(`SELECT COUNT(*) AS n,COALESCE(SUM(bytes),0) AS bytes FROM checkout_waiters WHERE checkout=? AND ${pending}`).get(snapshot.checkout)!;
    if (Number(row.n) >= CHECKOUT_QUEUE_LIMIT) fail('queue_full', `This checkout already has ${CHECKOUT_QUEUE_LIMIT} waiting messages. Cancel one or wait for a run to finish.`);
    if (Number(row.bytes) + bytes > CHECKOUT_QUEUE_BYTES) fail('queue_full','This checkout has reached its 32 MiB queued-message limit, including images. Cancel a message or wait for a run to finish.');
  }
  add(snapshot: Snapshot, request: string, approval: ApprovalMode, kind: Waiter['kind'], bytes: number) {
    const identity = this.store.rootIdentity(snapshot.id);
    const order = this.store.db.prepare('SELECT rowid FROM requests WHERE scope=? AND id=?').get(snapshot.id,request)!.rowid;
    this.store.db.prepare('INSERT INTO checkout_waiters VALUES (?,?,?,?,?,?,?,?,?,?,?,NULL)').run(snapshot.id,request,snapshot.checkout,JSON.stringify(identity),kind,'waiting',Number(order),queueSettings(snapshot),commandPolicy(this.store,snapshot.checkout,identity),approval,bytes);
    this.changed();
  }
  set(thread: string, request: string, state: WaitState, error?: Failure) {
    if (this.store.db.prepare(`UPDATE checkout_waiters SET state=?,error=? WHERE thread=? AND request=? AND ${pending}`).run(state,error ? JSON.stringify(error) : null,thread,request).changes) this.changed();
  }
  /** Holds every waiting input of one checkout, as a restart does; only Resume releases it. */
  holdCheckout(checkout: string) { if (this.store.db.prepare("UPDATE checkout_waiters SET state='held' WHERE checkout=? AND state='waiting'").run(checkout).changes) this.changed(); }
  hold(thread: string) { if (this.store.db.prepare("UPDATE checkout_waiters SET state='held' WHERE thread=? AND kind='follow_up' AND state='waiting'").run(thread).changes) this.changed(); }
  /** A held follow-up keeps its own thread's next-send rule; another thread's Resume does not release it. */
  resume(checkout: string) { if (this.store.db.prepare("UPDATE checkout_waiters SET state='waiting' WHERE checkout=? AND kind='checkout' AND state='held'").run(checkout).changes) this.changed(); }
  resumeThread(thread: string) { if (this.store.db.prepare("UPDATE checkout_waiters SET state='waiting' WHERE thread=? AND kind='follow_up' AND state='held'").run(thread).changes) this.changed(); }
  summary(thread: string, checkout: string, line = this.line(checkout)): QueueSummary | null {
    const at = line.findIndex(waiter => waiter.thread_id === thread && waiter.kind === 'checkout');
    if (at < 0) return null;
    return { request_id: line[at].request_id, state: line[at].state as 'waiting' | 'held', position: at + 1, count: line.filter(waiter => waiter.thread_id === thread && waiter.kind === 'checkout').length };
  }
  /** A summary page reads each checkout's line once: the stream repeats this on every change. */
  summaries<T extends { id: string; checkout: string }>(threads: T[]): (T & { checkout_queue: QueueSummary | null })[] {
    const lines = new Map<string, Waiter[]>();
    return threads.map(thread => {
      let line = lines.get(thread.checkout);
      if (!line) { line = this.line(thread.checkout); lines.set(thread.checkout, line); }
      return { ...thread, checkout_queue: this.summary(thread.id, thread.checkout, line) };
    });
  }
}
