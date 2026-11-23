import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxText } from '@earendil-works/pi-ai/providers/faux';
import './support/safety.ts';
import type { Snapshot } from '../src/protocol.ts';
import { RunnerRegistry } from '../src/runner/registry.ts';
import { LOST_PROGRESS_MESSAGE, RunnerSession, applyRunnerEvent } from '../src/runner/session.ts';
import { RunnerClient, RunnerServer } from '../src/runner/socket.ts';
import { Runtime } from '../src/runtime.ts';
import { Store } from '../src/store.ts';
import { TurnSnapshotApplier } from '../src/turn-executor.ts';

const exec = promisify(execFile);

function makeSnapshot(id: string, checkout: string): Snapshot {
  return {
    id, checkout, model: { provider: 'faux', model_id: 'faux-1' }, status: 'running',
    messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false,
  };
}

test('finished event persists terminal snapshot and last_seq in one transaction; recovery keeps it finished', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'pr-'));
  try {
    const store = new Store(dir);
    const snapshot = makeSnapshot('thread-1', dir);
    store.save(snapshot);
    store.createRun('run-1', 'thread-1', 'req-1', 'test');

    const applier = new TurnSnapshotApplier(snapshot);
    applyRunnerEvent(store, 'run-1', snapshot, applier, { type: 'phase', seq: 1, state: 'running' });
    applyRunnerEvent(store, 'run-1', snapshot, applier, { type: 'live', seq: 2, payload: { text: 'hello' } });
    // Simulates applying `finished` right before sending `ack` (and crashing before onRunnerClosed).
    applyRunnerEvent(store, 'run-1', snapshot, applier, { type: 'finished', seq: 3, result: { status: 'finished' } });

    const saved = store.get('thread-1');
    const run = store.run('run-1');
    assert.equal(saved.status, 'idle');
    assert.equal(saved.live_text, '');
    assert.equal(run?.last_seq, 3);
    assert.equal(run?.state, 'finished');
    assert.ok(run?.finished_at !== null);
    store.close();

    // Even if the runner manifest is left behind with no receipt (e.g. crash before receipt/ack), recovery never marks it interrupted.
    const registry = new RunnerRegistry(dir);
    await registry.writeManifest({ run_id: 'run-1', thread_id: 'thread-1', request_id: 'req-1', checkout: dir, harness: 'test', build_id: 'test' });
    const runtime = new Runtime(dir, createModels(), new Set());
    try {
      await runtime.recover();
      assert.equal(runtime.snapshot('thread-1').status, 'idle');
      assert.equal(runtime.snapshot('thread-1').error, null);
      assert.deepEqual(await registry.scan(), []);
    } finally {
      await runtime.close();
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('failing onClosed reconciliation does not cause an unhandled rejection', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'pr-'));
  const unhandled: unknown[] = [];
  const onUnhandled = (reason: unknown) => { unhandled.push(reason); };
  process.on('unhandledRejection', onUnhandled);
  const store = new Store(dir);
  try {
    const registry = new RunnerRegistry(dir);
    const runId = 'run-err';
    await registry.writeManifest({ run_id: runId, thread_id: 'thread-err', request_id: 'req-err', checkout: dir, harness: 'test', build_id: 'test' });
    const server = new RunnerServer(
      registry.socketPath(runId),
      () => ({ type: 'attached', state: 'running', seq: 0, snapshot: {}, pending_approval: null, head: null, before_record_id: null }),
      () => {},
    );
    await server.listen();

    let closedCalled = false;
    const snapshot = makeSnapshot('thread-err', dir);
    store.save(snapshot);
    store.createRun(runId, 'thread-err', 'req-err', 'test');
    const session = new RunnerSession(runId, 'thread-err', 'req-err', snapshot, {
      store,
      directory: dir,
      notify: () => {},
      phase: () => {},
      onClosed: async () => {
        closedCalled = true;
        throw new Error('simulated sqlite reconciliation failure');
      },
    });
    try {
      await session.supervisor.attach(runId, 0);
      await server.close();
      // Wait for disconnect + rejected onClosed promise to settle.
      const deadline = Date.now() + 3000;
      while (!closedCalled && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25));
      await new Promise(resolve => setTimeout(resolve, 50));
      assert.equal(closedCalled, true);
      assert.deepEqual(unhandled, []);
    } finally {
      session.detach();
    }
  } finally {
    process.off('unhandledRejection', onUnhandled);
    store.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('attach below runner pruned floor stops runner, interrupts with lost-progress message, and reconciles receipt', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'pr-'));
  const registry = new RunnerRegistry(dir);
  const runId = 'run-gap';
  let lock: Awaited<ReturnType<RunnerRegistry['acquire']>> | undefined;
  let server: RunnerServer | undefined;
  let runtime: Runtime | undefined;
  try {
    const store = new Store(dir);
    store.save(makeSnapshot('thread-gap', dir));
    store.createRun(runId, 'thread-gap', 'req-gap', 'test');
    store.close();

    await registry.writeManifest({ run_id: runId, thread_id: 'thread-gap', request_id: 'req-gap', checkout: dir, harness: 'test', build_id: 'test' });
    lock = await registry.acquire(runId);

    let stopReceived = false;
    server = new RunnerServer(
      registry.socketPath(runId),
      () => ({ type: 'attached', state: 'running', seq: 2, snapshot: {}, pending_approval: null, head: null, before_record_id: null }),
      command => { if (command.type === 'stop') stopReceived = true; },
    );
    await server.listen();
    server.publish({ type: 'live', seq: 1, payload: { text: 'pruned-event' } });
    server.publish({ type: 'live', seq: 2, payload: { text: 'stale-later-event' } });

    // Force buffer pruning of seq 1 via a prior client ack so floor becomes 2 while host last_seq stays 0.
    const prior = await RunnerClient.connect(registry.socketPath(runId), 0, () => {});
    prior.send({ type: 'ack', seq: 1 });
    await new Promise(resolve => setTimeout(resolve, 60));
    prior.close();
    await new Promise(resolve => setTimeout(resolve, 40));

    runtime = new Runtime(dir, createModels(), new Set());
    await runtime.recover();

    const deadline = Date.now() + 3000;
    while (!stopReceived && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(stopReceived, true);

    // Must not apply seq 2 over stale snapshot; thread is interrupted with the exact lost-progress message and state='gap'.
    const snap = runtime.snapshot('thread-gap');
    assert.equal(snap.status, 'interrupted');
    assert.equal(snap.live_text, '');
    assert.deepEqual(snap.error, { code: 'interrupted', message: LOST_PROGRESS_MESSAGE });
    assert.equal(runtime.store.run(runId)?.state, 'gap');

    // Runner writes receipt containing a postimage boundary event and exits; host reconciles effects while preserving interrupted status.
    await registry.writeReceipt(runId, {
      result: { status: 'stopped' },
      events: [{ type: 'postimage', seq: 3, postimage: { path: 'edited.txt', dev: '1', ino: '2', size: 4, mtime_ns: '1', hash: 'h' } }],
    });
    await server.close(); server = undefined;
    await lock.close(); lock = undefined;

    const cleanDeadline = Date.now() + 3000;
    while (runtime.runnerSessions.size > 0 && Date.now() < cleanDeadline) await new Promise(resolve => setTimeout(resolve, 25));
    assert.equal(runtime.runnerSessions.size, 0);
    assert.equal(runtime.active.size, 0);
    assert.deepEqual(await registry.scan(), []);
    assert.equal(runtime.snapshot('thread-gap').status, 'interrupted');
    assert.equal(runtime.snapshot('thread-gap').error?.message, LOST_PROGRESS_MESSAGE);
    assert.deepEqual(runtime.store.changedPaths('thread-gap'), ['edited.txt']);
  } finally {
    await runtime?.close();
    await server?.close();
    await lock?.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('gap persisted across host restart still reconciles receipt postimages and stays interrupted', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'pr-'));
  const registry = new RunnerRegistry(dir);
  const runId = 'run-gap-restart';
  let lock: Awaited<ReturnType<RunnerRegistry['acquire']>> | undefined;
  let server: RunnerServer | undefined;
  let runtime1: Runtime | undefined;
  let runtime2: Runtime | undefined;
  try {
    const store = new Store(dir);
    store.save(makeSnapshot('thread-gap-r', dir));
    store.createRun(runId, 'thread-gap-r', 'req-gap-r', 'test');
    store.close();

    await registry.writeManifest({ run_id: runId, thread_id: 'thread-gap-r', request_id: 'req-gap-r', checkout: dir, harness: 'test', build_id: 'test' });
    lock = await registry.acquire(runId);
    server = new RunnerServer(
      registry.socketPath(runId),
      () => ({ type: 'attached', state: 'running', seq: 2, snapshot: {}, pending_approval: null, head: null, before_record_id: null }),
      () => {},
    );
    await server.listen();
    server.publish({ type: 'live', seq: 1, payload: { text: 'pruned' } });
    server.publish({ type: 'live', seq: 2, payload: { text: 'later' } });

    const prior = await RunnerClient.connect(registry.socketPath(runId), 0, () => {});
    prior.send({ type: 'ack', seq: 1 });
    await new Promise(resolve => setTimeout(resolve, 60));
    prior.close();
    await new Promise(resolve => setTimeout(resolve, 40));

    // Host 1 detects the gap, persists runs.state='gap' + interrupted snapshot, then dies/closes before runner finishes.
    runtime1 = new Runtime(dir, createModels(), new Set());
    await runtime1.recover();
    assert.equal(runtime1.store.run(runId)?.state, 'gap');
    assert.equal(runtime1.snapshot('thread-gap-r').status, 'interrupted');
    await runtime1.close(); runtime1 = undefined;

    // Runner finishes while host is down and writes a receipt with a postimage and result status='stopped'.
    await registry.writeReceipt(runId, {
      result: { status: 'stopped' },
      events: [{ type: 'postimage', seq: 3, postimage: { path: 'receipt-effect.txt', dev: '1', ino: '2', size: 4, mtime_ns: '1', hash: 'h' } }],
    });
    await server.close(); server = undefined;
    await lock.close(); lock = undefined;

    // Host 2 restarts: thread MUST stay interrupted with LOST_PROGRESS_MESSAGE, and receipt postimage is recorded.
    runtime2 = new Runtime(dir, createModels(), new Set());
    await runtime2.recover();
    assert.equal(runtime2.snapshot('thread-gap-r').status, 'interrupted');
    assert.equal(runtime2.snapshot('thread-gap-r').error?.message, LOST_PROGRESS_MESSAGE);
    assert.deepEqual(runtime2.store.changedPaths('thread-gap-r'), ['receipt-effect.txt']);
    assert.deepEqual(await registry.scan(), []);
  } finally {
    await runtime1?.close();
    await runtime2?.close();
    await server?.close();
    await lock?.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('accepted run whose host crashed before start is marked interrupted after orphan exit and recover', async () => {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'pr-')));
  const repo = join(base, 'repo');
  const storeDir = join(base, 'store');
  let runtime: Runtime | undefined;
  let pid = 0;
  try {
    await exec('/bin/mkdir', ['-p', repo, storeDir]);
    await exec('/usr/bin/git', ['init', '-q', repo]);
    await writeFile(join(repo, 'notes.txt'), 'before\n');
    await exec('/usr/bin/git', ['-C', repo, 'add', '.']);
    await exec('/usr/bin/git', ['-C', repo, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', 'commit', '-qm', 'init']);

    const { acquireCheckout } = await import('../src/safety.ts');
    const { spawnRunner } = await import('../src/runner/spawn.ts');
    const { resolve } = await import('node:path');
    const lease = await acquireCheckout(repo);
    const registry = new RunnerRegistry(storeDir);
    const runId = 'accepted-not-started';
    try {
      // Simulate durable acceptance committed right before `start` was sent.
      const store = new Store(storeDir);
      const snap = makeSnapshot('thread-orphan', repo);
      store.save(snap);
      store.createRun(runId, 'thread-orphan', 'req-orphan', 'test');
      store.close();

      await registry.writeManifest({ run_id: runId, thread_id: 'thread-orphan', request_id: 'req-orphan', checkout: repo, harness: 'test', build_id: 'test', lease: { root: repo, identity: lease.identity } });
      pid = await spawnRunner(resolve('src/runner/main.ts'), lease, {
        PLAIT_RUNNER_STORE: storeDir, PLAIT_RUNNER_ID: runId, PLAIT_RUNNER_TEST_TURNS: '1', PLAIT_RUNNER_TEST_ORPHAN_MS: '200',
      });
      // Wait for runner to acquire its lock, then drop host lease copy (as if host crashed right before `start`).
      const liveDeadline = Date.now() + 5000;
      while ((await registry.scan())[0]?.status !== 'live' && Date.now() < liveDeadline) await new Promise(resolve => setTimeout(resolve, 25));
      await lease.close();
      // Runner never receives `start`, exits on orphan timeout -> status becomes `dead`.
      const deadDeadline = Date.now() + 5000;
      while ((await registry.scan())[0]?.status !== 'dead' && Date.now() < deadDeadline) await new Promise(resolve => setTimeout(resolve, 25));
      pid = 0;

      runtime = new Runtime(storeDir, createModels(), new Set());
      await runtime.recover();
      assert.equal(runtime.snapshot('thread-orphan').status, 'interrupted');
      assert.equal(runtime.snapshot('thread-orphan').error?.code, 'interrupted');
      assert.deepEqual(await registry.scan(), []);
      const next = await acquireCheckout(repo);
      await next.close();
    } finally {
      if (pid) { try { process.kill(pid, 'SIGKILL'); } catch {} }
      await lease.close();
    }
  } finally {
    await runtime?.close();
    await rm(base, { recursive: true, force: true });
  }
});

test('accept and createRun commit in one transaction; failure rolls back both and stops runner before start', async () => {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'pr-')));
  const repo = join(base, 'repo');
  const storeDir = join(base, 'store');
  const prevDetached = process.env.PLAIT_DETACHED_RUNNERS;
  const prevTestTurns = process.env.PLAIT_RUNNER_TEST_TURNS;
  process.env.PLAIT_DETACHED_RUNNERS = '1';
  process.env.PLAIT_RUNNER_TEST_TURNS = '1';
  let runtime: Runtime | undefined;
  try {
    await exec('/bin/mkdir', ['-p', repo, storeDir]);
    await exec('/usr/bin/git', ['init', '-q', repo]);
    await writeFile(join(repo, 'notes.txt'), 'before\n');
    await exec('/usr/bin/git', ['-C', repo, 'add', '.']);
    await exec('/usr/bin/git', ['-C', repo, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', 'commit', '-qm', 'init']);

    const models = createModels();
    const faux = fauxProvider({});
    faux.setResponses([fauxAssistantMessage([fauxText('ok')])]);
    models.setProvider(faux.provider);
    runtime = new Runtime(storeDir, models, new Set(['faux']));
    await runtime.recover();

    const created = await runtime.handle({ op: 'create_thread', checkout: repo, provider: 'faux', model_id: 'faux-1', request_id: 'create-1' }) as { thread: Snapshot };
    const threadId = created.thread.id;
    const pidFile = join(base, 'must-not-start.pid');
    const text = `__plait_runner_test__:${JSON.stringify({ pid_file: pidFile })}`;

    // Inject failure right after accept() inside the combined transaction.
    runtime.store.createRun = () => { throw new Error('simulated createRun failure'); };

    await assert.rejects(
      runtime.submit(threadId, 'submit-atomic-1', text),
      /simulated createRun failure/,
    );

    // Neither the request acceptance nor the run row was persisted, and the runner never started.
    assert.equal(runtime.store.duplicate(threadId, 'submit-atomic-1', text), undefined);
    const runCount = Number(runtime.store.db.prepare('SELECT count(*) AS c FROM runs').get()?.c ?? -1);
    assert.equal(runCount, 0);
    assert.equal(runtime.snapshot(threadId).status, 'idle');
    assert.equal(runtime.active.size, 0);
    assert.equal(runtime.runnerSessions.size, 0);
    await assert.rejects(access(pidFile));
    assert.deepEqual(await new RunnerRegistry(storeDir).scan(), []);
  } finally {
    await runtime?.close();
    if (prevDetached === undefined) delete process.env.PLAIT_DETACHED_RUNNERS; else process.env.PLAIT_DETACHED_RUNNERS = prevDetached;
    if (prevTestTurns === undefined) delete process.env.PLAIT_RUNNER_TEST_TURNS; else process.env.PLAIT_RUNNER_TEST_TURNS = prevTestTurns;
    await rm(base, { recursive: true, force: true });
  }
});
