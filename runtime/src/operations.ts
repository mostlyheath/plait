import { HostError, fail } from './protocol.ts';
import { digest, type Store } from './store.ts';

const pending = new WeakMap<Store, Map<string, Promise<void>>>();
const retention = 24 * 60 * 60 * 1000;

/** Replays a completed intent and serializes concurrent uses of its ID. */
export async function once<T>(store: Store, opId: unknown, op: string, input: unknown, run: () => Promise<T> | T, now = Date.now): Promise<T> {
  if (opId === undefined) return run();
  if (typeof opId !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(opId)) fail('invalid_request', 'Invalid operation ID.');
  store.db.exec('CREATE TABLE IF NOT EXISTS operations(op_id TEXT PRIMARY KEY, op TEXT NOT NULL, hash TEXT NOT NULL, outcome TEXT NOT NULL, at INTEGER NOT NULL)');
  let active = pending.get(store);
  if (!active) { active = new Map(); pending.set(store, active); }
  while (active.has(opId)) await active.get(opId);
  const hash = digest(input);
  const row = store.db.prepare('SELECT op,hash,outcome FROM operations WHERE op_id=?').get(opId);
  if (row) {
    if (row.op !== op || row.hash !== hash) fail('conflict', 'Operation ID was already used for different input.');
    const outcome = JSON.parse(String(row.outcome));
    if (outcome.refusal) fail(outcome.refusal.code, outcome.refusal.message);
    return outcome.value as T;
  }
  let release!: () => void;
  const waiting = new Promise<void>(resolve => { release = resolve; });
  active.set(opId, waiting);
  try {
    let outcome: { value?: T; refusal?: { code: string; message: string } };
    try { outcome = { value: await run() }; }
    catch (error) {
      if (!(error instanceof HostError)) throw error;
      outcome = { refusal: { code: error.code, message: error.message } };
    }
    const at = now();
    store.db.prepare('DELETE FROM operations WHERE at<?').run(at - retention);
    store.db.prepare('INSERT INTO operations VALUES (?,?,?,?,?)').run(opId, op, hash, JSON.stringify(outcome), at);
    if (outcome.refusal) fail(outcome.refusal.code, outcome.refusal.message);
    return outcome.value as T;
  } finally { active.delete(opId); release(); }
}
