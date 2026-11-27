import { fixtureDirectory, socketTempRoot } from './support/temp.ts';
import { confirmStopped } from './support/host.ts';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { connect } from 'node:net';
import { readFile, writeFile, rm, stat, realpath, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import type { Snapshot } from '../src/protocol.ts';
import { exec, binary, fixture, delay, childPids, until, request, raw, setup } from './support/host.ts';

test('start is independent, status is usable, and repeated start does not replace the daemon', { timeout: 15000 }, async t => {
  const store = await fixtureDirectory(join(socketTempRoot, 'plait-start-'));
  let pid: number | undefined;
  t.after(async () => { if (pid) process.kill(pid, 'SIGKILL'); await delay(300); await rm(store, { recursive: true, force: true }); });
  await exec(binary, ['start', '--store', store, '--runtime', fixture], { env: { ...process.env, PLAIT_TEST_SCENARIO: 'answer' } });
  const processes = JSON.parse(await readFile(join(store, 'fixture-processes.json'), 'utf8'));
  pid = processes.host;
  assert.equal(processes.store, await realpath(store));
  const status = JSON.parse((await exec(binary, ['status', '--store', store])).stdout);
  assert.equal(status.ok, true);
  await exec(binary, ['start', '--store', store, '--runtime', fixture]);
  assert.equal(JSON.parse(await readFile(join(store, 'fixture-processes.json'), 'utf8')).host, pid);
});

test('watch streams replacement snapshots and disconnect does not own the run', { timeout: 15000 }, async t => {
  const f = await setup(t); const thread = await f.create();
  const socket = connect(join(f.store, 'host.sock'));
  t.after(async () => { socket.destroy(); });
  const lines: { event?: string; thread?: Snapshot; ok?: boolean }[] = [];
  let pending = '';
  socket.on('error', () => {});
  socket.on('data', data => {
    pending += data.toString(); let end: number;
    while ((end = pending.indexOf('\n')) >= 0) { lines.push(JSON.parse(pending.slice(0, end))); pending = pending.slice(end + 1); }
  });
  socket.once('connect', () => socket.write(JSON.stringify({ op: 'watch', thread_id: thread.id }) + '\n'));
  await until(async () => lines.length, value => value > 0);
  await f.submit(thread.id);
  await until(async () => lines, values => values.some(value => value.event === 'snapshot'));
  socket.destroy();
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(done.messages.filter(message => message.role === 'user').length, 1);
});

for (const scenario of ['oversize', 'stall']) test(`sidecar ${scenario} is bounded without an unresponsive host`, { timeout: 30000 }, async t => {
  const store = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-protocol-')));
  const host = spawn(binary, ['run', '--store', store, '--runtime', resolve('test/stdio-fixture.ts')], { stdio: 'ignore', env: { ...process.env, PLAIT_TEST_SCENARIO: scenario } });
  t.after(async () => { host.kill('SIGKILL'); await delay(100); await rm(store, { recursive: true, force: true }); });
  await until(() => stat(join(store, 'host.sock')), value => value.isSocket());
  if (scenario === 'oversize') {
    assert.equal((await request(store, { op: 'status' })).ok, false);
    assert.equal((await request(store, { op: 'status' })).ok, true);
  } else {
    const start = Date.now();
    await assert.rejects(request(store, { op: 'status' }));
    await until(async () => host.exitCode !== null, value => value, 24000);
    assert(Date.now() - start >= 19000 && Date.now() - start < 25000);
  }
});

test('socket bounds, private permissions, locks, model/settings and dedup survive restart', { timeout: 30000 }, async t => {
  const f = await setup(t);
  assert.equal((await stat(f.store)).mode & 0o777, 0o700);
  assert.equal((await stat(join(f.store, 'host.sock'))).mode & 0o777, 0o600);
  assert.equal((await raw(f.store, 'not json\n')).error?.code, 'invalid_request');
  assert.equal((await raw(f.store, 'x'.repeat(65537) + '\n')).error?.code, 'invalid_request');
  assert.equal((await request(f.store, { op: 'status', extra: true })).error?.code, 'invalid_request');
  await assert.rejects(exec(binary, ['run', '--store', f.store, '--runtime', fixture]), /store already has a writer/);
  assert.equal((await request(f.store, { op: 'set_default_model', provider: 'faux', model_id: 'missing' })).error?.code, 'model_unavailable');
  assert.equal((await request(f.store, { op: 'set_default_model', provider: 'faux', model_id: 'faux-1' })).ok, true);
  const thread = await f.create();
  assert.equal((await f.create()).id, thread.id);
  assert.equal((await f.submit(thread.id)).ok, true);
  assert.equal((await f.submit(thread.id)).ok, true);
  assert.equal((await f.submit(thread.id, 'submit-1', 'different')).error?.code, 'conflict');
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(done.messages.filter(message => message.role === 'user').length, 1);
  f.host.kill('SIGKILL');
  await delay(500); await f.start();
  const status = await request(f.store, { op: 'status' });
  assert.deepEqual(status.result.default_model, { provider: 'faux', model_id: 'faux-1' });
  assert.equal((await f.create()).id, thread.id);
  assert.equal((await f.submit(thread.id)).ok, true);
  assert.deepEqual((await f.watch(thread.id)).model, thread.model);
  const listed = await request(f.store, { op: 'list_threads', checkout: f.repo });
  assert.deepEqual(Object.keys(listed.result.threads[0]).sort(), ['checkout', 'id', 'model', 'status']);
});

for (const change of ['unchanged', 'external', 'other-thread', 'ignored']) test(`reopened file-only postimage provenance: ${change}`, { timeout: 30000 }, async t => {
  const f = await setup(t, 'edit'); const thread = await f.create(); await f.submit(thread.id);
  for (const kind of ['edit']) {
    const pending = await until(() => f.watch(thread.id), value => value.pending_approval?.kind === kind);
    await f.approve(thread.id, pending.pending_approval!.id);
  }
  await until(() => f.watch(thread.id), value => value.status === 'idle');
  f.host.kill('SIGKILL'); await delay(400);
  if (change === 'external') await writeFile(join(f.repo, 'notes.txt'), 'after\nexternal user change\n');
  if (change === 'ignored') await writeFile(join(f.repo, '.gitignore'), 'notes.txt\n');
  await f.start('followup');
  const target = change === 'other-thread' ? await f.create('other-thread') : thread;
  await f.submit(target.id, 'followup-submit');
  if (change === 'unchanged') {
    const pending = await until(() => f.watch(target.id), value => !!value.pending_approval);
    await f.approve(target.id, pending.pending_approval!.id);
  }
  const done = await until(() => f.watch(target.id), value => value.status === 'idle');
  assert(done.tools.some(tool => tool.id === 'followup-1' && tool.status === (change === 'unchanged' ? 'done' : 'error')), JSON.stringify(done));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), change === 'unchanged' ? 'followup\n' : change === 'external' ? 'after\nexternal user change\n' : 'after\n');
});

test('idle is published only after cleanup so the immediate next submit is accepted', { timeout: 15000 }, async t => {
  const f = await setup(t, 'cleanup'); const thread = await f.create();
  assert.equal((await f.submit(thread.id, 'first')).ok, true);
  await until(() => f.watch(thread.id), value => value.status === 'idle');
  const next = await f.submit(thread.id, 'immediate-next', 'Follow up without a client sleep.');
  assert.equal(next.ok, true, JSON.stringify(next));
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(done.messages.filter(message => message.role === 'user').length, 2);
});

test('a stopped thread survives host restart after cleanup completes', { timeout: 15000 }, async t => {
  const f = await setup(t, 'cleanup'); const thread = await f.create();
  await f.submit(thread.id, 'stop-before-restart');
  assert.equal((await request(f.store, { op: 'stop', thread_id: thread.id })).result.thread.status, 'stopped');
  await until(async () => JSON.parse(await readFile(join(f.store, `${thread.id}.settled`), 'utf8')),
    marker => marker.fixture === 'SCRIPTED_FIXTURE_ONLY' && marker.request_id === 'stop-before-restart' && marker.status === 'stopped');
  const stopped = await f.watch(thread.id);
  f.host.kill('SIGTERM'); await delay(500); await f.start();
  const restored = await f.watch(thread.id);
  assert.equal(restored.status, 'stopped');
  assert.equal(restored.pending_approval, null);
  assert.deepEqual(restored.messages, stopped.messages);
});

test('different stores and TMPDIR values still share the checkout inode lease', { timeout: 15000 }, async t => {
  const f = await setup(t, 'crash'); const first = await f.create(); await f.submit(first.id);
  await until(() => f.watch(first.id), value => !!value.pending_approval);
  const store = join(f.base, 'second-store'); const temporary = join(f.base, 'other-temp');
  await exec('/bin/mkdir', ['-p', store, temporary]);
  const host = spawn(binary, ['run', '--store', store, '--runtime', fixture], { stdio: 'ignore', env: { ...process.env, TMPDIR: temporary, PLAIT_TEST_SCENARIO: 'crash' } });
  f.own(host); // Stop both hosts before the shared fixture directory is removed.
  await until(() => request(store, { op: 'status' }), reply => reply.ok);
  const created = await request(store, { op: 'create_thread', checkout: f.repo, provider: 'faux', model_id: 'faux-1', request_id: 'other-create' });
  const id = created.result.thread.id;
  const submit = () => request(store, { op: 'submit', thread_id: id, request_id: 'other-submit', text: 'No simultaneous writers.' });
  assert.equal((await submit()).error?.code, 'busy');
  await request(f.store, { op: 'stop', thread_id: first.id });
  assert.equal((await until(submit, reply => reply.ok)).ok, true);
  await request(store, { op: 'stop', thread_id: id });
});

for (const problem of ['symlink-instructions', 'oversized-baseline']) test(`accepted input survives preflight failure: ${problem}`, { timeout: 20000 }, async t => {
  const f = await setup(t); const thread = await f.create();
  if (problem === 'symlink-instructions') {
    await rm(join(f.repo, 'AGENTS.md')); await symlink('notes.txt', join(f.repo, 'AGENTS.md'));
  } else await Promise.all(Array.from({ length: 501 }, (_, index) => writeFile(join(f.repo, `dirty-${index}`), 'preserve')));
  const text = `Keep this original accepted input: ${problem}.`;
  assert.equal((await f.submit(thread.id, 'preflight', text)).ok, true);
  const failed = await until(() => f.watch(thread.id), value => value.status === 'error');
  assert(failed.messages.some(message => message.id === 'request-preflight' && message.text === text));
  assert.equal((await f.submit(thread.id, 'preflight', text)).ok, true);
  assert.equal((await f.watch(thread.id)).messages.filter(message => message.id === 'request-preflight').length, 1);
  f.host.kill('SIGKILL'); await delay(400); await f.start();
  assert((await f.watch(thread.id)).messages.some(message => message.text === text));
});

test('crash after acceptance before Durable submit retains input and never replays it', { timeout: 15000 }, async t => {
  const f = await setup(t, 'before-submit'); const thread = await f.create();
  const text = 'This accepted input must remain visible even when execution never began.';
  await f.submit(thread.id, 'admitted', text).catch(() => {});
  await until(() => readFile(join(f.store, 'accepted.marker'), 'utf8'), value => value.includes('accepted'));
  await until(async () => f.host.exitCode !== null, value => value);
  await f.start('answer');
  const interrupted = await f.watch(thread.id);
  assert.equal(interrupted.status, 'interrupted');
  assert(interrupted.messages.some(message => message.id === 'request-admitted' && message.text === text));
  assert.equal((await f.submit(thread.id, 'admitted', text)).ok, true);
  assert.equal((await f.watch(thread.id)).status, 'interrupted');
  await confirmStopped(f);
  await f.submit(thread.id, 'explicit-new', 'Start this new request only.');
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(done.messages.filter(message => message.role === 'user').length, 2);
  assert.equal(done.messages.find(message => message.role === 'user')!.text, text);
});

test('dirty baseline survives denied edits/commands and unsafe file paths fail', { timeout: 30000 }, async t => {
  const f = await setup(t, 'dirty');
  await writeFile(join(f.repo, 'notes.txt'), 'dirty\n');
  await writeFile(join(f.repo, 'untracked.txt'), 'keep\n');
  const thread = await f.create(); await f.submit(thread.id);
  const approval = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  assert.equal(approval.pending_approval!.kind, 'command');
  await f.approve(thread.id, approval.pending_approval!.id);
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert(done.tools.filter(tool => tool.status === 'error').length >= 2, JSON.stringify(done));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'dirty\n');
  assert.equal(await readFile(join(f.repo, 'untracked.txt'), 'utf8'), 'keep\n');
  await symlink(f.base, join(f.repo, 'escape'));
  for (const path of ['../outside', '.git/config', 'escape/outside']) {
    await assert.rejects(exec(binary, ['file', 'read', f.repo, path]));
  }
});

test('crash after effect before provenance record leaves the dirty file protected', { timeout: 30000 }, async t => {
  const f = await setup(t, 'unrecorded'); const thread = await f.create(); await f.submit(thread.id);
  const approval = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  await f.approve(thread.id, approval.pending_approval!.id);
  await until(() => readFile(join(f.repo, 'before-record.marker'), 'utf8'), value => value.includes('provenance absent'));
  await until(async () => f.host.exitCode !== null, value => value);
  await f.start('followup');
  assert.equal((await f.watch(thread.id)).status, 'interrupted');
  await confirmStopped(f);
  await f.submit(thread.id, 'followup');
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert(done.tools.some(tool => tool.id === 'followup-1' && tool.status === 'error'));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');
});

test('crash after atomic file effect before tool completion never repeats the effect', { timeout: 30000 }, async t => {
  const f = await setup(t, 'effect'); const thread = await f.create(); await f.submit(thread.id);
  const approval = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  await f.approve(thread.id, approval.pending_approval!.id);
  await until(() => readFile(join(f.repo, 'attempts.txt'), 'utf8'), value => value === 'effect\n');
  const [node] = await childPids(f.host.pid!);
  process.kill(node, 'SIGKILL');
  await until(async () => f.host.exitCode !== null, value => value);
  await f.start('answer');
  assert.equal((await f.watch(thread.id)).status, 'interrupted');
  assert.equal((await f.submit(thread.id)).ok, true);
  await confirmStopped(f);
  await f.submit(thread.id, 'resume-2', 'Inspect the interrupted result.');
  await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');
  assert.equal(await readFile(join(f.repo, 'attempts.txt'), 'utf8'), 'effect\n');
});
test('store aliases and symlink rejection do not depend on a command sandbox', { timeout: 15000 }, async t => {
  const f = await setup(t, 'edit', true);
  const thread = await f.create(); await f.submit(thread.id);
  for (const kind of ['edit']) {
    const pending = await until(() => f.watch(thread.id), state => state.pending_approval?.kind === kind);
    assert.equal((await f.approve(thread.id, pending.pending_approval!.id)).ok, true);
  }
  const done = await until(() => f.watch(thread.id), state => state.status === 'idle');
  assert(done.tools.every(tool => tool.status === 'done'));
  const alias = join(f.base, 'store-symlink'); await symlink(f.store, alias);
  await assert.rejects(exec(binary, ['run', '--store', alias, '--runtime', fixture]), /owned directory/);
});

test('approved edit survives client reconnect and excludes a second checkout writer', { timeout: 30000 }, async t => {
  const f = await setup(t, 'edit'); const thread = await f.create(); const other = await f.create('create-2');
  assert.equal((await f.submit(thread.id)).ok, true);
  const edit = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  assert.equal(edit.pending_approval!.kind, 'edit');
  assert.deepEqual(JSON.parse(edit.pending_approval!.details), { path: 'notes.txt', old_text: 'before', new_text: 'after' });
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'before\n');
  assert.equal((await f.submit(other.id)).error?.code, 'busy');
  assert.equal((await f.approve(thread.id, edit.pending_approval!.id)).ok, true);
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle' || value.status === 'error');
  assert.equal(done.status, 'idle', JSON.stringify(done));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');
  assert(done.tools.some(tool => tool.name === 'edit' && tool.status === 'done'), JSON.stringify(done));
});

test('provider errors stay private and Stop revokes pending edit approval', { timeout: 30000 }, async t => {
  const e = await setup(t, 'error'); const errorThread = await e.create(); await e.submit(errorThread.id);
  const failed = await until(() => e.watch(errorThread.id), value => value.status === 'error');
  assert(!JSON.stringify(failed).includes('SECRET_PROVIDER_TOKEN'));
  const f = await setup(t, 'edit'); const thread = await f.create(); await f.submit(thread.id);
  const pending = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  assert.equal(pending.pending_approval!.kind, 'edit');
  const approval = pending.pending_approval!.id;
  assert.equal((await request(f.store, { op: 'stop', thread_id: thread.id })).ok, true);
  assert.equal((await f.approve(thread.id, approval)).error?.code, 'not_found');
  assert.equal((await f.watch(thread.id)).status, 'stopped');
  assert.equal((await f.watch(thread.id)).pending_approval, null);
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'before\n');
});
