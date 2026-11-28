import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdir, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { Runtime } from '../src/runtime.ts';
import { rootIdentity } from '../src/safety.ts';
import { HostError } from '../src/protocol.ts';

test('optional checkout identities reject replacements and preserve old replies', async t => {
  const base = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-host-identity-')));
  const root = join(base, 'repo'); const store = join(base, 'store');
  await mkdir(root); await mkdir(store);
  const host = new Runtime(store, createModels(), new Set());
  t.after(async () => { await host.close(); await rm(base, { recursive: true, force: true }); });
  const identity = await rootIdentity(root);
  const wrong = { ...identity, ino: identity.ino === '1' ? '2' : '1' };
  const requests = [
    { op: 'checkout_cleanup', checkout: root },
    { op: 'command_policy', checkout: root },
    { op: 'agent_resources', protocol_version: 3, checkout: root },
    { op: 'list_threads', protocol_version: 3, checkout: root },
    { op: 'create_thread', checkout: root, provider: 'missing', model_id: 'missing', request_id: 'one' },
  ];
  for (const request of requests) {
    await assert.rejects(host.handle({ ...request, identity: wrong }), (e: HostError) => e.code === 'invalid_request' && e.message === 'This folder changed since it was opened.');
  }
  for (const request of requests.slice(1, 4)) {
    assert.deepEqual(await host.handle(request), await host.handle({ ...request, identity }));
  }
  await assert.rejects(host.handle({ op: 'list_threads', protocol_version: 3, identity }), (e: HostError) => e.code === 'invalid_request');
});
