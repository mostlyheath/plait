import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import { acquireCheckout, rootIdentity } from '../src/safety.ts';
import { threadOutputsPath } from '../src/tool-output.ts';
import { binary, request, setup, until } from './support/host.ts';
import type { TestScript } from './support/runner-test-provider.ts';

process.env.PLAIT_HOST_EXE ??= binary;
const DETACHED_PI_ENV = { PLAIT_DETACHED_RUNNERS: '1', PLAIT_RUNNER_TEST_TURNS: '1' };

async function readRunnerPid(pidFile: string): Promise<number> {
  const text = await until(() => readFile(pidFile, 'utf8'), value => /^\d+$/.test(value.trim()), 15000);
  return Number(text.trim());
}

function killPidBestEffort(pid: number) {
  if (pid > 0) { try { process.kill(pid, 'SIGKILL'); } catch {} }
}

async function writeScript(store: string, script: TestScript): Promise<void> {
  await writeFile(join(store, 'pi-test-script.json'), JSON.stringify(script));
}

test('detached Pi turn survives host kill -9 mid-turn, keeps continuous transcript, records, postimages, and releases lease', async t => {
  const f = await setup(t, 'pi-script', false, false, DETACHED_PI_ENV);
  const gate = join(f.base, 'gate');
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { text: 'Analyzing notes before editing.', tool: { name: 'read', args: { path: 'notes.txt' }, id: 'read-1' }, pid_file: pidFile },
      { text: 'Applying edit now.', gate, tool: { name: 'edit', args: { path: 'notes.txt', old_text: 'before', new_text: 'after' }, id: 'edit-1' } },
      { text: 'Finished editing notes.txt.' },
    ],
  });

  const thread = await f.create();
  const submitted = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-run-1', text: 'Update notes.txt', approval: 'auto' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  // Wait until streaming / first tool is visible and runner PID is written.
  await until(() => f.watch(thread.id), s => s.status === 'running' && s.tools.some(tool => tool.id === 'read-1' && tool.status === 'done'), 15000);
  runnerPid = await readRunnerPid(pidFile);

  // history works read-only (WAL) while the detached Pi runner is actively writing <thread_id>.sqlite.
  const midHistory = await request(f.store, { op: 'history', protocol_version: 6, thread_id: thread.id });
  assert.equal(midHistory.ok, true, JSON.stringify(midHistory));
  const midItems = (midHistory.result as unknown as { items: Array<{ type: string; text?: string }> }).items;
  assert.ok(midItems.some(item => item.type === 'message' && item.text?.includes('Update notes.txt')));

  // Kill -9 the host mid-turn and restart on the same store.
  f.host.kill('SIGKILL');
  await f.start('pi-script');

  // Thread never became interrupted.
  const afterRestart = await f.watch(thread.id);
  assert.equal(afterRestart.status, 'running');
  assert.equal(afterRestart.error, null);

  // Release gate so the edit tool runs once and the turn finishes.
  await writeFile(gate, 'open');
  const finished = await until(() => f.watch(thread.id), s => s.status === 'idle', 15000);
  assert.equal(finished.error, null);
  runnerPid = 0;

  // Edit applied once; watch and history show the continuous transcript.
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');
  assert.ok(finished.messages.some(m => m.role === 'user' && m.text === 'Update notes.txt'));
  assert.ok(finished.messages.some(m => m.role === 'assistant' && m.text.includes('Analyzing notes before editing.')));
  assert.ok(finished.messages.some(m => m.role === 'assistant' && m.text.includes('Finished editing notes.txt.')));
  assert.equal(finished.tools.filter(tool => tool.id === 'edit-1').length, 1);
  assert.equal(finished.tools.find(tool => tool.id === 'edit-1')?.status, 'done');

  const endHistory = await request(f.store, { op: 'history', protocol_version: 6, thread_id: thread.id });
  assert.equal(endHistory.ok, true, JSON.stringify(endHistory));
  const endItems = (endHistory.result as unknown as { items: Array<{ type: string; text?: string; id?: string }> }).items;
  assert.ok(endItems.some(item => item.type === 'message' && item.text?.includes('Finished editing notes.txt.')));

  // Before/after records exist (after_restart: false) and postimages are recorded.
  const recordsReply = await request(f.store, { op: 'turn_records', protocol_version: 6, thread_id: thread.id });
  assert.equal(recordsReply.ok, true, JSON.stringify(recordsReply));
  const records = (recordsReply.result as unknown as { records: Array<{ state: string; after_restart: boolean; paths: Array<{ path: string }> }> }).records;
  assert.equal(records.length, 1);
  assert.equal(records[0].state, 'ready');
  assert.equal(records[0].after_restart, false);
  assert.deepEqual(records[0].paths.map(p => p.path), ['notes.txt']);

  const changed = await request(f.store, { op: 'changed_files', thread_id: thread.id, protocol_version: 4 });
  assert.equal(changed.ok, true, JSON.stringify(changed));
  assert.deepEqual((changed.result as unknown as { files: string[] }).files, ['notes.txt']);

  // Runner files cleaned up, no cleanup_uncertain, and checkout lease is released.
  await until(async () => (await readdir(join(f.store, 'runners'))).length === 0, empty => empty, 5000);
  const cleanup = await request(f.store, { op: 'checkout_cleanup', checkout: f.repo });
  assert.equal(cleanup.ok, true);
  assert.equal((cleanup.result as unknown as { cleanup: unknown }).cleanup, null);
  const lease = await acquireCheckout(f.repo);
  await lease.close();
});

test('host killed while awaiting Pi edit approval -> restart -> approve -> edit applies once', async t => {
  const f = await setup(t, 'pi-script', false, false, DETACHED_PI_ENV);
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { pid_file: pidFile, tool: { name: 'edit', args: { path: 'notes.txt', old_text: 'before', new_text: 'approved' }, id: 'edit-approve' } },
      { text: 'Edit approved and applied.' },
    ],
  });

  const thread = await f.create();
  const submitted = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-approve-1', text: 'Edit with approval', approval: 'ask' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  const awaiting = await until(() => f.watch(thread.id), s => s.status === 'awaiting_approval' && s.pending_approval?.kind === 'edit', 15000);
  const approvalId = awaiting.pending_approval!.id;
  runnerPid = await readRunnerPid(pidFile);

  // While awaiting approval, history shows the in-flight tool as running.
  const activeHistory = await request(f.store, { op: 'history', protocol_version: 6, thread_id: thread.id });
  assert.equal(activeHistory.ok, true, JSON.stringify(activeHistory));
  const activeItems = (activeHistory.result as unknown as { items: Array<{ type: string; id?: string; status?: string }> }).items;
  assert.ok(activeItems.some(item => item.type === 'tool' && item.id === 'edit-approve' && item.status === 'running'));

  // Kill -9 host and restart.
  f.host.kill('SIGKILL');
  await f.start('pi-script');

  const reattached = await f.watch(thread.id);
  assert.equal(reattached.status, 'awaiting_approval');
  assert.equal(reattached.pending_approval?.id, approvalId);

  // Approve after restart -> edit applies once -> finishes idle.
  const approved = await f.approve(thread.id, approvalId, true);
  assert.equal(approved.ok, true, JSON.stringify(approved));
  assert.equal(approved.result.thread.pending_approval, null);

  const finished = await until(() => f.watch(thread.id), s => s.status === 'idle', 15000);
  assert.equal(finished.error, null);
  runnerPid = 0;

  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'approved\n');
  const recordsReply = await request(f.store, { op: 'turn_records', protocol_version: 6, thread_id: thread.id });
  const records = (recordsReply.result as unknown as { records: Array<{ state: string; paths: Array<{ path: string }> }> }).records;
  assert.equal(records.length, 1);
  assert.equal(records[0].state, 'ready');
  assert.deepEqual(records[0].paths.map(p => p.path), ['notes.txt']);
});

test('detached Pi runner finishes while host is down -> restart ingests receipt, records, and postimages cleanly', async t => {
  const f = await setup(t, 'pi-script', false, false, DETACHED_PI_ENV);
  const gate = join(f.base, 'gate');
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { text: 'Starting work.', tool: { name: 'read', args: { path: 'notes.txt' }, id: 'read-down' }, pid_file: pidFile },
      { gate, tool: { name: 'edit', args: { path: 'notes.txt', old_text: 'before', new_text: 'while-down' }, id: 'edit-down' } },
      { text: 'Completed while host was down.' },
    ],
  });

  const thread = await f.create();
  const submitted = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-down-1', text: 'Work while host is down', approval: 'auto' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  await until(() => f.watch(thread.id), s => s.status === 'running' && s.tools.some(tool => tool.id === 'read-down' && tool.status === 'done'), 15000);
  runnerPid = await readRunnerPid(pidFile);

  // Kill -9 the host, then open the gate so the runner finishes while the host is down.
  f.host.kill('SIGKILL');
  await writeFile(gate, 'open');
  await until(
    async () => (await readdir(join(f.store, 'runners'))).some(name => name.endsWith('.done.json')),
    done => done,
    15000,
  );
  await until(async () => { try { process.kill(runnerPid, 0); return false; } catch { return true; } }, gone => gone, 10000);
  runnerPid = 0;

  await f.start('pi-script');

  const snapshot = await f.watch(thread.id);
  assert.equal(snapshot.status, 'idle');
  assert.equal(snapshot.error, null);
  assert.ok(snapshot.messages.some(m => m.role === 'assistant' && m.text.includes('Completed while host was down.')));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'while-down\n');

  const recordsReply = await request(f.store, { op: 'turn_records', protocol_version: 6, thread_id: thread.id });
  const records = (recordsReply.result as unknown as { records: Array<{ state: string; after_restart: boolean; paths: Array<{ path: string }> }> }).records;
  assert.equal(records.length, 1);
  assert.equal(records[0].state, 'ready');
  assert.equal(records[0].after_restart, false);
  assert.deepEqual(records[0].paths.map(p => p.path), ['notes.txt']);

  const changed = await request(f.store, { op: 'changed_files', thread_id: thread.id, protocol_version: 4 });
  assert.deepEqual((changed.result as unknown as { files: string[] }).files, ['notes.txt']);

  assert.deepEqual(await readdir(join(f.store, 'runners')), []);
  const cleanup = await request(f.store, { op: 'checkout_cleanup', checkout: f.repo });
  assert.equal((cleanup.result as unknown as { cleanup: unknown }).cleanup, null);
});

test('command output from detached Pi runner is written to per-thread outputs sqlite and readable via read_output after restart', async t => {
  const f = await setup(t, 'pi-script', false, false, DETACHED_PI_ENV);
  const policy = await request(f.store, { op: 'set_command_policy', checkout: f.repo, policy: 'trusted' });
  assert.equal(policy.ok, true, JSON.stringify(policy));

  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { tool: { name: 'command', args: { command: 'printf "DETACHED_OUTPUT_MARKER_12345\\n"' }, id: 'cmd-1' } },
      { text: 'Command finished.' },
      { tool: { name: 'read_output', args: { id: 'cmd-1', offset: 0, limit: 1024 }, id: 'read-out-1' } },
      { text: 'Read stored output.' },
    ],
  });

  const thread = await f.create();
  const run1 = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-cmd-1', text: 'Run command', approval: 'auto' });
  assert.equal(run1.ok, true, JSON.stringify(run1));
  const afterRun1 = await until(() => f.watch(thread.id), s => s.status === 'idle', 15000);
  assert.equal(afterRun1.error, null);
  assert.ok(existsSync(threadOutputsPath(f.store, thread.id)), 'expected per-thread outputs sqlite file');

  // Kill -9 host and restart, then read_output from the previous run's command.
  f.host.kill('SIGKILL');
  await f.start('pi-script');

  const run2 = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-cmd-2', text: 'Read previous command output', approval: 'auto' });
  assert.equal(run2.ok, true, JSON.stringify(run2));
  const afterRun2 = await until(() => f.watch(thread.id), s => s.status === 'idle' && s.tools.some(tool => tool.id === 'read-out-1'), 15000);
  assert.equal(afterRun2.error, null);
  const readOut = afterRun2.tools.find(tool => tool.id === 'read-out-1');
  assert.equal(readOut?.status, 'done');
  assert.ok(readOut?.output.includes('DETACHED_OUTPUT_MARKER_12345'), JSON.stringify(readOut));
});

test('stop after host kill -9 stops detached Pi turn and releases checkout lease', async t => {
  const f = await setup(t, 'pi-script', false, false, DETACHED_PI_ENV);
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { pid_file: pidFile, tool: { name: 'edit', args: { path: 'notes.txt', old_text: 'before', new_text: 'never' }, id: 'edit-stop' } },
      { text: 'Should not run.' },
    ],
  });

  const thread = await f.create();
  const submitted = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-stop-1', text: 'Edit then stop', approval: 'ask' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  await until(() => f.watch(thread.id), s => s.status === 'awaiting_approval', 15000);
  runnerPid = await readRunnerPid(pidFile);

  f.host.kill('SIGKILL');
  await f.start('pi-script');
  assert.equal((await f.watch(thread.id)).status, 'awaiting_approval');

  const stopped = await request(f.store, { op: 'stop', thread_id: thread.id });
  assert.equal(stopped.ok, true, JSON.stringify(stopped));
  assert.equal(stopped.result.thread.status, 'stopped');

  await until(async () => { try { process.kill(runnerPid, 0); return false; } catch { return true; } }, gone => gone, 10000);
  runnerPid = 0;
  await until(async () => (await readdir(join(f.store, 'runners'))).length === 0, empty => empty, 5000);
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'before\n');
  const cleanup = await request(f.store, { op: 'checkout_cleanup', checkout: f.repo });
  assert.equal((cleanup.result as unknown as { cleanup: unknown }).cleanup, null);
  const lease = await acquireCheckout(f.repo);
  await lease.close();
});

test('production-configured detached Pi runner (PLAIT_RUNNER_TEST_TURNS unset) ignores PLAIT_TEST_SCENARIO', async t => {
  const f = await setup(t, 'pi-script', false, false, { PLAIT_DETACHED_RUNNERS: '1', PLAIT_RUNNER_TEST_TURNS: '' });
  const pidFile = join(f.base, 'must-not-run.pid');
  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { pid_file: pidFile, tool: { name: 'edit', args: { path: 'notes.txt', old_text: 'before', new_text: 'scripted' }, id: 'edit-ignored' } },
      { text: 'Should never run.' },
    ],
  });

  const thread = await f.create();
  const submitted = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-prod-1', text: 'Attempt scripted turn', approval: 'auto' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  // Runner ignores PLAIT_TEST_SCENARIO, loads real piProviders (which has no faux provider), and fails without running the script.
  const ended = await until(() => f.watch(thread.id), s => s.status === 'error' || s.status === 'idle', 15000);
  assert.equal(ended.status, 'error', JSON.stringify(ended));
  assert.equal(existsSync(pidFile), false);
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'before\n');
});

test('steering input survives host kill -9 and reattach resend is idempotent (received once)', async t => {
  const f = await setup(t, 'pi-script', false, false, DETACHED_PI_ENV);
  const pidFile = join(f.base, 'runner.pid');
  let runnerPid = 0;
  t.after(async () => { killPidBestEffort(runnerPid); });

  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { pid_file: pidFile, tool: { name: 'edit', args: { path: 'notes.txt', old_text: 'before', new_text: 'step1' }, id: 'edit-steer-1' } },
      { text: 'Applied step 1.' },
      { text: 'Handled steered message.' },
    ],
  });

  const thread = await f.create();
  const submitted = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-steer-run', text: 'Start turn', approval: 'ask' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  const awaiting = await until(() => f.watch(thread.id), s => s.status === 'awaiting_approval', 15000);
  const approvalId = awaiting.pending_approval!.id;
  runnerPid = await readRunnerPid(pidFile);

  // Queue a steering input while awaiting approval, then kill -9 host before approving so reconnect resends it.
  const steered = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-steer-msg', text: 'Also do this after restart', when_busy: 'steer' });
  assert.equal(steered.ok, true, JSON.stringify(steered));

  f.host.kill('SIGKILL');
  await f.start('pi-script');

  const reattached = await f.watch(thread.id);
  assert.equal(reattached.status, 'awaiting_approval');
  const approved = await f.approve(thread.id, approvalId, true);
  assert.equal(approved.ok, true, JSON.stringify(approved));

  const finished = await until(() => f.watch(thread.id), s => s.status === 'idle', 15000);
  assert.equal(finished.error, null);
  runnerPid = 0;

  // Steered input appears exactly once in the transcript and in turn_records.request_ids.
  const steerMessages = finished.messages.filter(m => m.role === 'user' && m.text === 'Also do this after restart');
  assert.equal(steerMessages.length, 1, JSON.stringify(finished.messages));
  const recordsReply = await request(f.store, { op: 'turn_records', protocol_version: 6, thread_id: thread.id });
  const records = (recordsReply.result as unknown as { records: Array<{ request_ids: string[] }> }).records;
  assert.equal(records.length, 1);
  assert.deepEqual(records[0].request_ids, ['pi-steer-run', 'pi-steer-msg']);
});

test('detached Pi runner receives refreshed root identity and command policy after refreshRootIdentity', async t => {
  const f = await setup(t, 'pi-script', false, false, DETACHED_PI_ENV);
  const policy = await request(f.store, { op: 'set_command_policy', checkout: f.repo, policy: 'trusted' });
  assert.equal(policy.ok, true, JSON.stringify(policy));

  const thread = await f.create();
  const realIdentity = await rootIdentity(f.repo);

  // Simulate a stale saved device ID (same inode): refreshRootIdentity must run before reading rootIdentity & commandPolicy.
  const db = new DatabaseSync(join(f.store, 'host.sqlite'));
  try {
    db.prepare('UPDATE roots SET identity=? WHERE id=?').run(JSON.stringify({ dev: '999999999', ino: realIdentity.ino }), thread.id);
  } finally {
    db.close();
  }

  await writeScript(f.store, {
    retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    steps: [
      { tool: { name: 'command', args: { command: 'printf "REFRESHED_POLICY_OK"' }, id: 'cmd-refreshed' } },
      { text: 'Refreshed identity worked.' },
    ],
  });

  const submitted = await request(f.store, { op: 'submit', protocol_version: 6, thread_id: thread.id, request_id: 'pi-refresh-1', text: 'Run trusted command', approval: 'auto' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));

  const finished = await until(() => f.watch(thread.id), s => s.status === 'idle', 15000);
  assert.equal(finished.error, null);
  const cmdTool = finished.tools.find(tool => tool.id === 'cmd-refreshed');
  assert.equal(cmdTool?.status, 'done', JSON.stringify(cmdTool));
  assert.ok(cmdTool?.output.includes('REFRESHED_POLICY_OK'), JSON.stringify(cmdTool));
});
