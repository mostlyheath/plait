import { fixtureDirectory, socketTempRoot } from './support/temp.ts';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { realpath, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import type { TestContext } from 'node:test';
import { binary, childPids, delay, request, setup, until } from './support/host.ts';
import { watch } from './support/watch.ts';

type FixtureSnapshot = { id: string; revision: number };
async function transport(t: TestContext) {
  const store = await realpath(await fixtureDirectory(join(socketTempRoot, 'plait-notifications-')));
  const host = spawn(binary, ['run', '--store', store, '--runtime', resolve('test/transport-fixture.ts')], { stdio: 'ignore' });
  t.after(async () => {
    if (host.exitCode === null && host.signalCode === null) {
      const exited = once(host, 'exit'); host.kill('SIGKILL'); await exited;
    }
    await delay(100); await rm(store, { recursive: true, force: true });
  });
  await until(() => request(store, { op: 'status' }), reply => reply.ok);
  return {
    store, host,
    async requests() { return (await request(store, { op: 'status' })).result as unknown as { watch_requests: number }; },
  };
}

test('idle watches do not poll and changes wake only the selected thread', { timeout: 15000 }, async t => {
  const f = await transport(t);
  const watches = Array.from({ length: 10 }, (_, index) => watch<FixtureSnapshot>(f.store, `thread-${index}`));
  t.after(async () => { for (const item of watches) item.socket.destroy(); });
  await until(async () => watches.every(item => item.latest()), Boolean);
  assert.equal((await f.requests()).watch_requests, 10);
  await delay(450);
  assert.equal((await f.requests()).watch_requests, 10);
  await request(f.store, { op: 'submit', thread_id: 'thread-0' });
  await until(async () => watches[0].latest()?.revision, value => value === 1);
  assert.equal((await f.requests()).watch_requests, 11);
  assert(watches.slice(1).every(item => item.frames.length === 1));
});

test('idle disconnect releases all connection slots without waiting for a change', { timeout: 15000 }, async t => {
  const f = await transport(t);
  const watches: ReturnType<typeof watch>[] = [];
  t.after(async () => { for (const item of watches) item.socket.destroy(); });
  for (let round = 0; round < 2; round++) {
    const current = Array.from({ length: 60 }, (_, index) => watch(f.store, `thread-${round}-${index}`));
    watches.push(...current);
    await until(async () => current.every(item => item.latest()), Boolean);
    await Promise.all(current.map(async item => {
      const closed = once(item.socket, 'close'); item.socket.destroy(); await closed;
    }));
  }
  assert.equal((await f.requests()).watch_requests, 120);
});

test('sidecar death closes idle watches without a polling monitor', { timeout: 15000 }, async t => {
  const f = await transport(t);
  const subscription = watch(f.store, 'thread');
  t.after(async () => { subscription.socket.destroy(); });
  await until(async () => subscription.latest(), Boolean);
  const [pid] = await childPids(f.host.pid!);
  assert(pid);
  process.kill(pid, 'SIGKILL');
  await until(async () => f.host.exitCode, code => code === 1);
  await until(async () => subscription.socket.destroyed, Boolean);
});

test('notifications deliver approval, Stop, and fresh reconnect state', { timeout: 30000 }, async t => {
  const f = await setup(t, 'edit'); const thread = await f.create();
  const first = watch(f.store, thread.id);
  t.after(async () => { first.socket.destroy(); });
  await until(async () => first.latest(), state => state?.status === 'idle');
  await f.submit(thread.id);
  const pending = await until(async () => first.latest(), state => !!state?.pending_approval);
  await request(f.store, { op: 'stop', thread_id: thread.id });
  await until(async () => first.latest(), state => state?.status === 'stopped' && !state.pending_approval);
  assert.equal((await f.approve(thread.id, pending!.pending_approval!.id)).ok, false);
  first.socket.destroy();
  const second = watch(f.store, thread.id);
  t.after(async () => { second.socket.destroy(); });
  await until(async () => second.latest(), state => state?.status === 'stopped');
});

test('a new watch during streaming reaches completion without a polling client', { timeout: 15000 }, async t => {
  const f = await setup(t); const thread = await f.create();
  await f.submit(thread.id);
  const subscription = watch(f.store, thread.id);
  t.after(async () => { subscription.socket.destroy(); });
  const done = await until(async () => subscription.latest(), state => state?.status === 'idle');
  assert(done!.messages.some(message => message.role === 'assistant'));
  assert.equal(done!.messages.filter(message => message.role === 'user').length, 1);
});
