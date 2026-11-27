import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { Runtime } from '../src/runtime.ts';

test('host release operations are versionless and drain admission', async t => {
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-drain-'));
  const runtime = new Runtime(directory, createModels(), new Set());
  t.after(async () => { await runtime.close(); await rm(directory, { recursive: true, force: true }); });
  assert.deepEqual(await runtime.handle({ op: 'host_info' }), { schema: 1, release: null, build_id: null, protocol_min: 1, protocol_max: 7, capabilities: ['drain_v1', 'checkout_identity_v1', 'project_fingerprint_v1', 'guarded_discard_v1'], storage_schema: 1 });
  assert.equal((await runtime.reply({ op: 'host_info', protocol_version: 1 })).error?.code, 'invalid_request');
  const versions = [1, 2, 3, 4, 5, 6, 7];
  const statuses = await Promise.all(versions.map(protocol_version => runtime.handle({ op: 'status', protocol_version })));
  assert.deepEqual(await runtime.handle({ op: 'drain_for_update' }), { draining: true, active_runs: 0 });
  assert.deepEqual(await runtime.handle({ op: 'drain_for_update' }), { draining: true, active_runs: 0 });
  for (const op of ['submit', 'create_thread', 'submit_checkout', 'create_worktree_thread', 'create_admitted_thread', 'undo_turn']) {
    const reply = await runtime.reply({ op });
    assert.equal(reply.error?.code, 'busy');
    assert.equal(reply.error?.message, 'The host is updating. Try again when it finishes.');
  }
  assert.deepEqual(await Promise.all(versions.map(protocol_version => runtime.handle({ op: 'status', protocol_version }))), statuses);
  assert.deepEqual(await runtime.handle({ op: 'cancel_drain' }), { draining: false });
  assert.notEqual((await runtime.reply({ op: 'create_thread' })).error?.code, 'busy');
});

test('drain lets a running run finish and holds the checkout queue until cancelled', async t => {
  const { setup } = await import('./support/safety.ts');
  const { fauxProvider, fauxAssistantMessage } = await import('@earendil-works/pi-ai/providers/faux');
  const { root } = await setup(t);
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-drain-run-'));
  const leases = await fixtureDirectory(join(tmpdir(), 'plait-drain-leases-'));
  process.env.PLAIT_LEASE_DIR = leases;
  const models = createModels(); const faux = fauxProvider({ tokensPerSecond: 1e8 }); models.setProvider(faux.provider);
  const runtime = new Runtime(directory, models, new Set(['faux']));
  t.after(async () => { await runtime.close(); await rm(directory, { recursive: true, force: true }); await rm(leases, { recursive: true, force: true }); });
  let release!: () => void; let started = false;
  const hold = new Promise<void>(resolve => { release = resolve; });
  faux.setResponses([async () => { started = true; await hold; return fauxAssistantMessage('done'); }, fauxAssistantMessage('next')]);
  const create = async (id: string) => (await runtime.handle({ op: 'create_thread', protocol_version: 6, request_id: id, checkout: root, provider: 'faux', model_id: 'faux-1' }) as { thread: { id: string } }).thread.id;
  const a = await create('a'), b = await create('b');
  const until = async (done: () => boolean) => { const deadline = Date.now() + 30000; while (!done()) { assert.ok(Date.now() < deadline); await new Promise(resolve => setTimeout(resolve, 20)); } };
  try {
    await runtime.handle({ op: 'submit', protocol_version: 6, thread_id: a, request_id: 'first', text: 'first', approval: 'auto' });
    await until(() => started);
    await runtime.handle({ op: 'submit_checkout', thread_id: b, request_id: 'second', text: 'second', approval: 'auto' });
    assert.deepEqual(await runtime.handle({ op: 'drain_for_update' }), { draining: true, active_runs: 1 });
    assert.equal((await runtime.reply({ op: 'submit', protocol_version: 6, thread_id: b, request_id: 'new', text: 'new' })).error?.code, 'busy');
    release(); await until(() => runtime.active.size === 0);
    assert.equal(runtime.store.waiters.get(b, 'second')?.state, 'waiting');
    await runtime.handle({ op: 'stop', thread_id: a });
    assert.equal(runtime.records.page(a, undefined, 1, 7).records[0].outcome, 'finished');
    await runtime.handle({ op: 'cancel_drain' });
    await until(() => runtime.store.waiters.get(b, 'second')?.state === 'started' && runtime.active.size === 0);
  } finally { release(); }
});
