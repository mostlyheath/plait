import assert from 'node:assert/strict';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { confirmStopped, request, setup, until } from './support/host.ts';

const DETACHED_ENV = { PLAIT_DETACHED_RUNNERS: '1', PLAIT_RUNNER_TEST_TURNS: '1' };
const testTurn = (payload: Record<string, unknown>) => `__plait_runner_test__:${JSON.stringify(payload)}`;
const postimage = (path: string) => ({ path, dev: '1', ino: '2', size: 5, mtime_ns: '1', hash: 'abc' });

async function readRunnerPid(pidFile: string): Promise<number> {
  const text = await until(() => readFile(pidFile, 'utf8'), value => /^\d+$/.test(value.trim()), 10000);
  return Number(text.trim());
}

function killPidBestEffort(pid: number) {
  if (pid > 0) { try { process.kill(pid, 'SIGKILL'); } catch {} }
}

test('detached runner survives host kill -9, routes steer and drain_for_update, and finishes idle with full effects', async t => {
  const f = await setup(t, 'answer', false, true, DETACHED_ENV);
  const gate = join(f.base, 'gate');
  const pidFile = join(f.base, 'runner.pid');
  const steerFile = join(f.base, 'steer.json');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  const thread = await f.create();
  const submitted = await f.submit(thread.id, 'run-1', testTurn({
    skip_approval: true,
    gate,
    pid_file: pidFile,
    steer_file: steerFile,
    after_gate_postimage: postimage('notes.txt'),
  }));
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  // Streaming text is visible via watch before restart.
  const streaming = await until(() => f.watch(thread.id), s => s.status === 'running' && s.live_text === 'working');
  assert.equal(streaming.status, 'running');
  runnerPid = await readRunnerPid(pidFile);

  // drain_for_update counts live detached runs as active.
  const drain = await request(f.store, { op: 'drain_for_update' });
  assert.equal(drain.ok, true, JSON.stringify(drain));
  assert.equal((drain.result as unknown as { active_runs: number }).active_runs, 1);
  assert.equal((await request(f.store, { op: 'cancel_drain' })).ok, true);

  // Kill -9 the host mid-turn and restart on the same store.
  f.host.kill('SIGKILL');
  await f.start();

  // Thread stayed running (never interrupted) and can still be steered after restart.
  const afterRestart = await f.watch(thread.id);
  assert.equal(afterRestart.status, 'running');
  assert.equal(afterRestart.error, null);

  const steered = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'steer-1', text: 'nudge', when_busy: 'steer' });
  assert.equal(steered.ok, true, JSON.stringify(steered));
  const steerPayload = JSON.parse(await until(() => readFile(steerFile, 'utf8'), value => value.includes('nudge'), 5000));
  assert.equal(steerPayload.type, 'steer');
  assert.equal(steerPayload.text, 'nudge');

  // Release gate: runner finishes and thread transitions to idle with postimage effect applied.
  await writeFile(gate, 'open');
  const finished = await until(() => f.watch(thread.id), s => s.status === 'idle');
  assert.equal(finished.error, null);
  runnerPid = 0;

  const changed = await request(f.store, { op: 'changed_files', thread_id: thread.id, protocol_version: 4 });
  assert.equal(changed.ok, true, JSON.stringify(changed));
  assert.deepEqual((changed.result as unknown as { files: string[] }).files, ['notes.txt']);

  // Runner files are cleaned up once the runner exits and releases its lock, and checkout has no cleanup_uncertain.
  await until(async () => (await readdir(join(f.store, 'runners'))).length === 0, empty => empty, 5000);
  const cleanup = await request(f.store, { op: 'checkout_cleanup', checkout: f.repo });
  assert.equal(cleanup.ok, true);
  assert.equal((cleanup.result as unknown as { cleanup: unknown }).cleanup, null);
});

test('approval pending across host kill -9 can be answered after restart', async t => {
  const f = await setup(t, 'answer', false, true, DETACHED_ENV);
  const gate = join(f.base, 'gate');
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  const thread = await f.create();
  const submitted = await f.submit(thread.id, 'run-1', testTurn({ gate, pid_file: pidFile }));
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  const awaiting = await until(() => f.watch(thread.id), s => s.status === 'awaiting_approval' && s.pending_approval?.id === 'approval');
  assert.equal(awaiting.pending_approval?.id, 'approval');
  runnerPid = await readRunnerPid(pidFile);

  f.host.kill('SIGKILL');
  await f.start();

  // Pending approval survives host restart.
  const reattached = await f.watch(thread.id);
  assert.equal(reattached.status, 'awaiting_approval');
  assert.equal(reattached.pending_approval?.id, 'approval');

  // Approve waits for approval_settled before replying so pending_approval is cleared immediately.
  const approved = await f.approve(thread.id, 'approval', true);
  assert.equal(approved.ok, true, JSON.stringify(approved));
  assert.equal(approved.result.thread.pending_approval, null);
  assert.equal(approved.result.thread.status, 'running');

  await writeFile(gate, 'open');
  const finished = await until(() => f.watch(thread.id), s => s.status === 'idle');
  assert.equal(finished.error, null);
  runnerPid = 0;
});

test('runner finishes while host is down -> after restart thread is idle with receipt effects applied', async t => {
  const f = await setup(t, 'answer', false, true, DETACHED_ENV);
  const gate = join(f.base, 'gate');
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  const thread = await f.create();
  const submitted = await f.submit(thread.id, 'run-1', testTurn({
    skip_approval: true,
    gate,
    pid_file: pidFile,
    after_gate_postimage: postimage('notes.txt'),
  }));
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  await until(() => f.watch(thread.id), s => s.status === 'running' && s.live_text === 'working');
  runnerPid = await readRunnerPid(pidFile);

  // Kill -9 the host, then let the runner complete while the host is down.
  f.host.kill('SIGKILL');
  await writeFile(gate, 'open');
  await until(
    async () => (await readdir(join(f.store, 'runners'))).some(name => name.endsWith('.done.json')),
    done => done,
    10000,
  );
  // Wait for runner process to exit so its lock is released before restart.
  await until(async () => { try { process.kill(runnerPid, 0); return false; } catch { return true; } }, gone => gone, 5000);
  runnerPid = 0;

  await f.start();

  const snapshot = await f.watch(thread.id);
  assert.equal(snapshot.status, 'idle');
  assert.equal(snapshot.error, null);

  // Unacknowledged postimage boundary event from the done receipt was applied.
  const changed = await request(f.store, { op: 'changed_files', thread_id: thread.id, protocol_version: 4 });
  assert.equal(changed.ok, true, JSON.stringify(changed));
  assert.deepEqual((changed.result as unknown as { files: string[] }).files, ['notes.txt']);

  // Runner files were removed and checkout has no cleanup_uncertain.
  assert.deepEqual(await readdir(join(f.store, 'runners')), []);
  const cleanup = await request(f.store, { op: 'checkout_cleanup', checkout: f.repo });
  assert.equal(cleanup.ok, true);
  assert.equal((cleanup.result as unknown as { cleanup: unknown }).cleanup, null);
});

test('runner SIGKILL interrupts thread and leaves checkout cleanup marker for cleanup_uncertain', async t => {
  const f = await setup(t, 'answer', false, true, DETACHED_ENV);
  const gate = join(f.base, 'gate');
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  const thread = await f.create();
  const submitted = await f.submit(thread.id, 'run-1', testTurn({ skip_approval: true, gate, pid_file: pidFile }));
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  await until(() => f.watch(thread.id), s => s.status === 'running' && s.live_text === 'working');
  runnerPid = await readRunnerPid(pidFile);

  // SIGKILL the runner directly.
  process.kill(runnerPid, 'SIGKILL');
  runnerPid = 0;

  const interrupted = await until(() => f.watch(thread.id), s => s.status === 'interrupted');
  assert.equal(interrupted.error?.code, 'interrupted');

  // Manifest/socket were removed, but checkout cleanup marker remains for confirmation.
  await until(async () => (await readdir(join(f.store, 'runners'))).length === 0, empty => empty, 5000);
  const cleanup = await request(f.store, { op: 'checkout_cleanup', checkout: f.repo });
  assert.equal(cleanup.ok, true);
  assert.ok((cleanup.result as unknown as { cleanup: { token: string } | null }).cleanup !== null);

  await confirmStopped(f);
});

test('stop after host kill -9 stops the detached runner and releases the checkout', async t => {
  const f = await setup(t, 'answer', false, true, DETACHED_ENV);
  const gate = join(f.base, 'gate');
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  const thread = await f.create();
  const submitted = await f.submit(thread.id, 'run-1', testTurn({ skip_approval: true, gate, pid_file: pidFile }));
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  await until(() => f.watch(thread.id), s => s.status === 'running' && s.live_text === 'working');
  runnerPid = await readRunnerPid(pidFile);

  f.host.kill('SIGKILL');
  await f.start();

  assert.equal((await f.watch(thread.id)).status, 'running');

  const stopped = await request(f.store, { op: 'stop', thread_id: thread.id });
  assert.equal(stopped.ok, true, JSON.stringify(stopped));
  assert.equal(stopped.result.thread.status, 'stopped');

  // Runner exits, files are removed, and checkout is clean.
  await until(async () => { try { process.kill(runnerPid, 0); return false; } catch { return true; } }, gone => gone, 5000);
  runnerPid = 0;
  await until(async () => (await readdir(join(f.store, 'runners'))).length === 0, empty => empty, 5000);
  const cleanup = await request(f.store, { op: 'checkout_cleanup', checkout: f.repo });
  assert.equal(cleanup.ok, true);
  assert.equal((cleanup.result as unknown as { cleanup: unknown }).cleanup, null);
});
