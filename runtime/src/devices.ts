import { fail } from './protocol.ts';
import type { Store } from './store.ts';

const operations: Record<string, string[]> = {
  device_state_get: ['device'],
  device_state_set: ['device', 'revision', 'state'],
};

export class Devices {
  private store: Store;
  private changed: () => void;
  constructor(store: Store, changed: () => void) {
    this.store = store; this.changed = changed;
    store.db.exec('CREATE TABLE IF NOT EXISTS device_state(device TEXT PRIMARY KEY, revision INTEGER NOT NULL, state TEXT NOT NULL, updated_at INTEGER NOT NULL)');
  }

  handles(input: unknown) {
    return !!input && typeof input === 'object' && Object.hasOwn(operations, String((input as { op?: unknown }).op));
  }

  handle(input: unknown) {
    if (!input || typeof input !== 'object' || Array.isArray(input)) fail('invalid_request', 'Invalid device request.');
    const request = input as Record<string, unknown>;
    const fields = typeof request.op === 'string' && Object.hasOwn(operations, request.op) ? operations[request.op] : undefined;
    if (!fields || Object.keys(request).some(key => key !== 'op' && !fields.includes(key))
      || fields.some(key => !Object.hasOwn(request, key)) || Buffer.byteLength(JSON.stringify(request)) > 65_000) fail('invalid_request', 'Unknown operation or field.');
    if (typeof request.device !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(request.device)) fail('invalid_request', 'Invalid device.');
    if (request.op === 'device_state_get') return this.get(request.device);
    if (!Number.isSafeInteger(request.revision) || Number(request.revision) < 0) fail('invalid_request', 'Invalid revision.');
    if (!request.state || typeof request.state !== 'object' || Array.isArray(request.state) || Object.getPrototypeOf(request.state) !== Object.prototype) fail('invalid_request', 'Invalid state.');
    const state = JSON.stringify(request.state);
    if (Buffer.byteLength(state) > 60_000) fail('invalid_request', 'State is too large.');
    return this.set(request.device, Number(request.revision), state);
  }

  private get(device: string) {
    const row = this.store.db.prepare('SELECT revision,state FROM device_state WHERE device=?').get(device);
    return row ? { revision: Number(row.revision), state: JSON.parse(String(row.state)) } : { revision: 0, state: null };
  }

  private set(device: string, revision: number, state: string) {
    const db = this.store.db;
    db.exec('BEGIN IMMEDIATE');
    try {
      const current = this.get(device);
      if (current.revision !== revision) fail('conflict', 'Device state changed; read it again.');
      if (revision === Number.MAX_SAFE_INTEGER) fail('invalid_request', 'Revision is exhausted.');
      // Preserve write order when several updates share a clock tick.
      const latest = db.prepare('SELECT MAX(updated_at) AS latest FROM device_state').get();
      const updated = Math.max(Date.now(), Number(latest?.latest ?? 0) + 1);
      db.prepare('INSERT INTO device_state VALUES (?,?,?,?) ON CONFLICT(device) DO UPDATE SET revision=excluded.revision,state=excluded.state,updated_at=excluded.updated_at').run(device, revision + 1, state, updated);
      db.exec('DELETE FROM device_state WHERE device NOT IN (SELECT device FROM device_state ORDER BY updated_at DESC, device LIMIT 64)');
      db.exec('COMMIT');
    } catch (error) { db.exec('ROLLBACK'); throw error; }
    this.changed();
    return { revision: revision + 1 };
  }
}
