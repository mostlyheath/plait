import assert from 'node:assert/strict';
import { fstatSync } from 'node:fs';
import { open, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { acquireCheckout, adoptDescriptors, handoffDescriptors, releaseAfterHandoff } from '../src/safety.ts';
import { cleanupHelper, RESTART_REASON } from '../src/cleanup.ts';
import { spawnRunner } from '../src/runner/spawn.ts';
import { setup } from './support/safety.ts';

async function waitFor<T>(read: () => Promise<T>): Promise<T> {
  const deadline = Date.now() + 5000;
  for (;;) {
    try { return await read(); } catch (error) {
      if (Date.now() >= deadline) throw error;
      await new Promise(resolve => setTimeout(resolve, 25));
    }
  }
}

for (const killed of [false, true]) test(`runner holds checkout lease until ${killed ? 'SIGKILL' : 'exit'}`, async t => {
  const { root, base } = await setup(t);
  const ready = join(base, 'ready'); const stop = join(base, 'stop');
  const lease = await acquireCheckout(root);
  await cleanupHelper('begin', root, 'runner-test', RESTART_REASON, lease);
  const descriptors = handoffDescriptors(lease);
  const identities = descriptors.map(fd => {
    const stat = fstatSync(fd, { bigint: true });
    return { dev: String(stat.dev), ino: String(stat.ino) };
  });
  const store = await open(join(base, 'store-lock'), 'w+');
  const old = { ...process.env };
  let pid: number | undefined;
  try {
    process.env.PLAIT_LOCK_FD = String(store.fd);
    process.env.PLAIT_NOTIFY_FD = String(store.fd);
    process.env.PLAIT_TEST_LEASE = JSON.stringify({ root, identity: lease.identity, ready, stop });
    pid = await spawnRunner(resolve('test/support/runner-lease-child.ts'), lease);
    const child = JSON.parse(await waitFor(() => readFile(ready, 'utf8')));
    assert.deepEqual(child.identities, identities);
    assert.equal(child.lock, undefined); assert.equal(child.notify, undefined);
    const storeStat = await store.stat({ bigint: true });
    assert.notDeepEqual(child.identities[0], { dev: String(storeStat.dev), ino: String(storeStat.ino) });
    await releaseAfterHandoff(lease);
    await assert.rejects(acquireCheckout(root), { code: 'busy' });
    assert.equal((await cleanupHelper('read', root)).cleanup, null);
    if (killed) {
      process.kill(pid, 'SIGKILL');
      await waitFor(async () => { assert.equal((await cleanupHelper('read', root)).cleanup?.token, 'runner-test'); });
      await cleanupHelper('resolve', root, 'runner-test');
    } else {
      await writeFile(stop, 'stop');
      await waitFor(async () => { assert.equal((await cleanupHelper('read', root)).cleanup?.token, 'runner-test'); });
      await cleanupHelper('resolve', root, 'runner-test');
    }
    pid = undefined;
    const next = await acquireCheckout(root); await next.close();
  } finally {
    if (pid !== undefined) { try { process.kill(pid, 'SIGKILL'); } catch {} }
    await lease.close(); await store.close();
    for (const key of ['PLAIT_LOCK_FD', 'PLAIT_NOTIFY_FD', 'PLAIT_TEST_LEASE']) {
      if (old[key] === undefined) delete process.env[key]; else process.env[key] = old[key];
    }
  }
});

test('adoption rejects a descriptor for a different file', async t => {
  const { root, base } = await setup(t);
  const lease = await acquireCheckout(root);
  const other = await open(join(base, 'other'), 'w+');
  try {
    const [lifecycle, , owner] = handoffDescriptors(lease);
    await assert.rejects(adoptDescriptors([lifecycle, other.fd, owner], lease), /identity mismatch/);
  } finally { await other.close(); await lease.close(); }
});
