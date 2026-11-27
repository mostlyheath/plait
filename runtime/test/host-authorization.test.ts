import { fixtureDirectory, socketTempRoot } from './support/temp.ts';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { mkdir, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { acquireCheckout } from '../src/safety.ts';
import { binary, childPids, delay, exec, fixture, request, setup, until } from './support/host.ts';

test('each manual turn creates fresh permission and rejects an earlier approval', { timeout: 30000 }, async t => {
  const f = await setup(t, 'separate-edits');
  const thread = await f.create();
  assert.equal((await f.submit(thread.id)).ok, true);
  const first = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  const firstId = first.pending_approval!.id;
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'before\n');
  assert.equal((await f.approve(thread.id, firstId)).ok, true);
  await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');

  assert.equal((await f.submit(thread.id, 'submit-2', 'Make the next scripted edit.')).ok, true);
  const second = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  assert.notEqual(second.pending_approval!.id, firstId);
  assert.equal((await f.approve(thread.id, firstId)).error?.code, 'not_found');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');
  assert.equal((await f.approve(thread.id, second.pending_approval!.id)).ok, true);
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'followup\n');
  assert.equal(done.tools.filter(tool => tool.name === 'edit' && tool.status === 'done').length, 2);
});

test('denied manual edit has no file effect and cannot be approved later', { timeout: 30000 }, async t => {
  const f = await setup(t, 'edit');
  const thread = await f.create();
  await f.submit(thread.id);
  const pending = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  const id = pending.pending_approval!.id;
  assert.equal((await f.approve(thread.id, id, false)).ok, true);
  assert.equal((await f.approve(thread.id, id, true)).error?.code, 'not_found');
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(done.pending_approval, null);
  assert(done.tools.some(tool => tool.name === 'edit' && tool.status === 'error'));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'before\n');
});

test('a permissive fixture policy cannot bypass dirty paths, path checks, or command containment', { timeout: 30000 }, async t => {
  const f = await setup(t, 'permissive-policy');
  await writeFile(join(f.repo, 'notes.txt'), 'dirty\n');
  const thread = await f.create();
  assert.equal((await f.submit(thread.id)).ok, true);
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(done.pending_approval, null);
  assert.equal(done.tools.length, 4);
  assert(done.tools.every(tool => tool.status === 'error'), JSON.stringify(done.tools));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'dirty\n');
  await assert.rejects(readFile(join(f.base, 'outside.txt')), { code: 'ENOENT' });
  await assert.rejects(readFile(join(f.repo, '.git', 'automation.txt')), { code: 'ENOENT' });
});

/**
 * Starts a second host on its own short store. macOS limits a socket path to
 * 104 bytes, and a store nested in the first host's temporary folder can exceed
 * it; the host's stderr is kept so a failed start says why.
 */
async function secondHost(t: { after: (fn: () => Promise<void>) => void }, own: (child: ChildProcess) => void) {
  const store = await realpath(await fixtureDirectory(join(socketTempRoot, 'plait-store-')));
  t.after(async () => { await rm(store, { recursive: true, force: true }); });
  const child = spawn(binary, ['run', '--store', store, '--runtime', fixture], {
    stdio: ['ignore', 'ignore', 'pipe'],
    env: { ...process.env, PLAIT_TEST_SCENARIO: 'answer' },
  });
  own(child);
  let stderr = ''; child.stderr!.on('data', chunk => { stderr = (stderr + chunk.toString()).slice(-8192); });
  try { await until(async () => (await request(store, { op: 'status' })).ok, ok => ok); }
  catch (error) { throw new Error(`Second host did not start: ${(error as Error).message}\n${stderr}`); }
  return store;
}

test('two host stores contend on one inode and its alias while an unrelated checkout progresses', { timeout: 30000 }, async t => {
  process.env.PLAIT_HOST_EXE ??= binary;
  const f = await setup(t, 'edit');
  const secondStore = await secondHost(t, f.own);
  const alias = join(f.base, 'repo-alias');
  const unrelated = join(f.base, 'unrelated');
  await symlink(f.repo, alias);
  await mkdir(unrelated);
  await exec('/usr/bin/git', ['init', '-q', unrelated]);
  await writeFile(join(unrelated, 'notes.txt'), 'other\n');
  await exec('/usr/bin/git', ['-C', unrelated, 'add', '.']);
  await exec('/usr/bin/git', ['-C', unrelated, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', 'commit', '-qm', 'init']);


  // Store 1 starts an active run on f.repo (paused awaiting approval, holding writer + lifecycle leases).
  const t1 = await f.create('create-s1');
  assert.equal((await f.submit(t1.id, 'submit-s1')).ok, true);
  const pending = await until(() => f.watch(t1.id), value => !!value.pending_approval);

  // Store 2 targets the same checkout via a symlink alias and is refused as busy.
  const t2 = (await request(secondStore, { op: 'create_thread', checkout: alias, provider: 'faux', model_id: 'faux-1', request_id: 'create-s2' })).result.thread;
  assert.equal(t2.checkout, f.repo);
  const contended = await request(secondStore, { op: 'submit', thread_id: t2.id, request_id: 'submit-s2', text: 'Contend.' });
  assert.equal(contended.ok, false);
  assert.equal(contended.error?.code, 'busy');

  // Exclusive desktop-style lease on the same checkout is also refused while store 1 holds it.
  await assert.rejects(acquireCheckout(f.repo, 'exclusive'), { code: 'busy' });

  // Meanwhile store 2 runs freely in an unrelated checkout.
  const other = (await request(secondStore, { op: 'create_thread', checkout: unrelated, provider: 'faux', model_id: 'faux-1', request_id: 'create-other' })).result.thread;
  assert.equal((await request(secondStore, { op: 'submit', thread_id: other.id, request_id: 'submit-other', text: 'Independent.' })).ok, true);
  await until(async () => (await request(secondStore, { op: 'watch', thread_id: other.id })).result.thread, value => value.status === 'idle');

  // Once store 1 finishes its run, store 2 can run on the checkout immediately.
  assert.equal((await f.approve(t1.id, pending.pending_approval!.id, false)).ok, true);
  await until(() => f.watch(t1.id), value => value.status === 'idle');
  assert.equal((await request(secondStore, { op: 'submit', thread_id: t2.id, request_id: 'submit-s2-after', text: 'Now free.' })).ok, true);
  await until(async () => (await request(secondStore, { op: 'watch', thread_id: t2.id })).result.thread, value => value.status === 'idle');
});

test('a host crash releases OS leases but quarantines the checkout across stores until confirmed', { timeout: 30000 }, async t => {
  process.env.PLAIT_HOST_EXE ??= binary;
  const f = await setup(t, 'edit');
  const secondStore = await secondHost(t, f.own);

  const t1 = await f.create('create-crash-1');
  assert.equal((await f.submit(t1.id, 'submit-crash-1')).ok, true);
  await until(() => f.watch(t1.id), value => !!value.pending_approval);
  await assert.rejects(acquireCheckout(f.repo, 'exclusive'), { code: 'busy' });

  // Kill the first host's sidecar (SIGKILL) and wait for the host to exit.
  const [node] = await childPids(f.host.pid!);
  process.kill(node, 'SIGKILL');
  await until(async () => f.host.exitCode !== null || f.host.signalCode !== null, done => done);
  await delay(50);

  // A lost sidecar cannot vouch for descendants. The durable marker survives
  // its kernel locks and excludes desktop writes and every other host store.
  await assert.rejects(acquireCheckout(f.repo, 'exclusive'), { code: 'cleanup_uncertain' });
  const cleanup = ((await request(secondStore, { op: 'checkout_cleanup', checkout: f.repo })).result as unknown as { cleanup: { token: string; reason: string } }).cleanup;
  assert.match(cleanup.reason, /host ended/);
  assert.equal((await request(secondStore, { op: 'resolve_checkout_cleanup', checkout: f.repo, token: 'stale', confirm_stopped: true })).ok, false);
  assert.equal((await request(secondStore, { op: 'resolve_checkout_cleanup', checkout: f.repo, token: cleanup.token, confirm_stopped: true })).ok, true);
  const exclusive = await acquireCheckout(f.repo, 'exclusive');
  await exclusive.close();
  const t2 = (await request(secondStore, { op: 'create_thread', checkout: f.repo, provider: 'faux', model_id: 'faux-1', request_id: 'create-crash-2' })).result.thread;
  assert.equal((await request(secondStore, { op: 'submit', thread_id: t2.id, request_id: 'submit-crash-2', text: 'Take over.' })).ok, true);
  await until(async () => (await request(secondStore, { op: 'watch', thread_id: t2.id })).result.thread, value => value.status === 'idle');
});
