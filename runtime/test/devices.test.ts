import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Devices } from '../src/devices.ts';
import { HostError } from '../src/protocol.ts';
import { Store } from '../src/store.ts';

const code = (expected: string) => (error: unknown) => error instanceof HostError && error.code === expected;
async function setup(t: { after: (fn: () => unknown) => void }) {
  const root = await fixtureDirectory(join(tmpdir(), 'plait-devices-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  let count = 0;
  const devices = new Devices(new Store(root), () => { count++; });
  return { devices, count: () => count };
}
const get = (device: string) => ({ op: 'device_state_get', device });
const set = (device: string, revision: number, state: unknown) => ({ op: 'device_state_set', device, revision, state });

test('round trip and stale write', async t => {
  const { devices, count } = await setup(t);
  assert.deepEqual(devices.handle(get('mac')), { revision: 0, state: null });
  assert.deepEqual(devices.handle(set('mac', 0, { tabs: ['a'] })), { revision: 1 });
  assert.throws(() => devices.handle(set('mac', 0, { secret: 'do not expose' })), code('conflict'));
  assert.deepEqual(devices.handle(get('mac')), { revision: 1, state: { tabs: ['a'] } });
  assert.deepEqual(devices.handle(set('mac', 1, {})), { revision: 2 });
  assert.equal(count(), 2);
});

test('state size is measured in serialized bytes', async t => {
  const { devices } = await setup(t);
  assert.deepEqual(devices.handle(set('d', 0, { x: 'a'.repeat(59992) })), { revision: 1 });
  assert.throws(() => devices.handle(set('d', 1, { x: 'é'.repeat(30000) })), code('invalid_request'));
});

test('inserting the 65th device evicts the least recently updated', async t => {
  const { devices } = await setup(t);
  for (let i = 0; i < 64; i++) devices.handle(set(`d${i}`, 0, { i }));
  devices.handle(set('d0', 1, { refreshed: true }));
  devices.handle(set('new', 0, {}));
  assert.deepEqual(devices.handle(get('d1')), { revision: 0, state: null });
  assert.deepEqual(devices.handle(get('d0')), { revision: 2, state: { refreshed: true } });
});

test('invalid fields and values are refused', async t => {
  const { devices } = await setup(t);
  for (const request of [get(''), get('../x'), get('x'.repeat(129)), { ...get('d'), extra: 1 },
    { op: 'device_state_set', device: 'd', state: {} }, set('d', -1, {}), set('d', 1.5, {}),
    set('d', Number.MAX_SAFE_INTEGER + 1, {}), set('d', 0, null), set('d', 0, []), set('d', 0, 'x')]) {
    assert.throws(() => devices.handle(request), code('invalid_request'));
  }
  assert.deepEqual(devices.handle(get('d')), { revision: 0, state: null });
});
