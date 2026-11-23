import assert from 'node:assert/strict';
import { access, mkdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { acquireCheckout } from '../src/safety.ts';
import { RunnerSupervisor } from '../src/runner/supervisor.ts';
import { RunnerRegistry } from '../src/runner/registry.ts';
import { RunnerServer } from '../src/runner/socket.ts';
import { spawnRunner } from '../src/runner/spawn.ts';
import type { Event } from '../src/runner/protocol.ts';
import { setup } from './support/safety.ts';

async function waitFor(check: () => boolean | Promise<boolean>): Promise<void> {
  const end = Date.now() + 5000;
  while (!await check()) { if (Date.now() > end) throw new Error('Timed out'); await new Promise(resolve => setTimeout(resolve, 25)); }
}

for (const mode of ['finish', 'stop', 'reattach', 'kill'] as const) test(`runner supervisor: ${mode}`, async t => {
  const { root, base } = await setup(t); const lease = await acquireCheckout(root);
  const previous = process.env.PLAIT_RUNNER_TEST_TURNS; process.env.PLAIT_RUNNER_TEST_TURNS = '1';
  const events: Event[] = []; let disconnected = '';
  const supervisor = new RunnerSupervisor({ store: base, onEvent: event => { events.push(event); }, onDisconnect: entry => { disconnected = entry?.status ?? ''; } });
  let next: RunnerSupervisor | undefined; let pid = 0;
  try {
    const gate = join(base, 'gate');
    const run = await supervisor.launch({ threadId: 'thread', requestId: 'request', lease, payload: { kind: 'test', gate } }); pid = run.pid;
    await waitFor(() => events.some(event => event.type === 'approval_needed'));
    if (mode === 'kill') {
      process.kill(pid, 'SIGKILL'); pid = 0;
      await waitFor(() => disconnected === 'dead');
    } else if (mode === 'stop') {
      supervisor.stop(); await waitFor(() => events.some(event => event.type === 'finished'));
      assert.equal((events.at(-1)?.result as { status: string }).status, 'stopped');
    } else {
      if (mode === 'reattach') {
        await waitFor(async () => { await new Promise(resolve => setTimeout(resolve, 50)); return true; });
        const last = events.at(-1)!.seq; supervisor.detach();
        const replay: Event[] = [];
        next = new RunnerSupervisor({ store: base, onEvent: event => { replay.push(event); } });
        await next.attach(run.runId, last); next.approve('approval', true); await writeFile(gate, 'open');
        await waitFor(() => replay.some(event => event.type === 'finished'));
        assert.ok(replay.every(event => event.seq > last));
      } else {
        supervisor.approve('approval', true); await writeFile(gate, 'open');
        await waitFor(() => events.some(event => event.type === 'finished'));
        assert.deepEqual(events.map(event => event.type), ['phase', 'live', 'commit', 'approval_needed', 'approval_settled', 'finished']);
      }
      // Read the receipt from the scan that saw it finished: cleanup may remove the files right after.
      let finished: Awaited<ReturnType<RunnerRegistry['scan']>>[number] | undefined;
      await waitFor(async () => { finished = (await new RunnerRegistry(base).scan())[0]; return finished?.status === 'finished' || !finished; });
      if (finished) assert.ok(finished.receipt);
    }
    await waitFor(async () => { try { const acquired = await acquireCheckout(root); await acquired.close(); return true; } catch { return false; } });
    pid = 0;
  } finally {
    supervisor.detach(); next?.detach(); if (pid) { try { process.kill(pid, 'SIGKILL'); } catch {} }
    await lease.close(); if (previous === undefined) delete process.env.PLAIT_RUNNER_TEST_TURNS; else process.env.PLAIT_RUNNER_TEST_TURNS = previous;
  }
});

test('runner exits without attach and start', async t => {
  const { root, base } = await setup(t); const lease = await acquireCheckout(root); const registry = new RunnerRegistry(base);
  const runId = 'orphan';
  await registry.writeManifest({ run_id: runId, thread_id: 'thread', request_id: 'request', checkout: root, harness: 'test', build_id: 'test', lease: { root, identity: lease.identity } });
  const pid = await spawnRunner(resolve('src/runner/main.ts'), lease, { PLAIT_RUNNER_STORE: base, PLAIT_RUNNER_ID: runId, PLAIT_RUNNER_TEST_TURNS: '1', PLAIT_RUNNER_TEST_ORPHAN_MS: '500' });
  try {
    // The runner may come and go before a scan sees it live on a slow machine,
    // so wait for the process itself to exit, then check what it left behind.
    await lease.close();
    await waitFor(() => { try { process.kill(pid, 0); return false; } catch { return true; } });
    assert.equal((await registry.scan())[0]?.status, 'dead');
    const next = await acquireCheckout(root); await next.close();
  } finally { try { process.kill(pid, 'SIGKILL'); } catch {} await lease.close(); }
});

test('heartbeat timeout reconnects while runner lock is held and never declares it dead', async t => {
  const { root, base } = await setup(t);
  const registry = new RunnerRegistry(base);
  const runId = 'hb-reconnect';
  await registry.writeManifest({ run_id: runId, thread_id: 'thread', request_id: 'request', checkout: root, harness: 'test', build_id: 'test' });
  const lock = await registry.acquire(runId);
  let attaches = 0;
  let seq = 0;
  let ignorePing = true;
  const server = new RunnerServer(
    registry.socketPath(runId),
    () => ({ type: 'attached', state: 'running', seq, snapshot: {}, pending_approval: null, head: null, before_record_id: null }),
    command => {
      // Ignore the first ping to force a heartbeat timeout; answer pings after reconnect.
      if (command.type === 'ping' && !ignorePing) {
        server.publish({ type: 'pong', seq: ++seq, nonce: command.nonce });
      }
    },
    () => {
      attaches++;
      if (attaches >= 2) {
        ignorePing = false;
        server.publish({ type: 'live', seq: ++seq, payload: { text: 'after-reconnect' } });
      }
    },
  );
  await server.listen();
  const events: Event[] = [];
  let disconnected = false;
  const supervisor = new RunnerSupervisor({
    store: base,
    heartbeatMs: 40,
    heartbeatTimeoutMs: 40,
    retryBaseMs: 25,
    onEvent: event => { events.push(event); },
    onDisconnect: () => { disconnected = true; },
  });
  try {
    await supervisor.attach(runId, 0);
    await waitFor(() => attaches >= 2 && events.some(e => e.type === 'live'));
    assert.equal(disconnected, false);
    assert.equal(attaches >= 2, true);
  } finally {
    supervisor.detach();
    await server.close();
    await lock.close();
  }
});

test('start is not sent if acceptance fails; runner stops and releases checkout', async t => {
  const { root, base } = await setup(t);
  const lease = await acquireCheckout(root);
  const previous = process.env.PLAIT_RUNNER_TEST_TURNS; process.env.PLAIT_RUNNER_TEST_TURNS = '1';
  const pidFile = join(base, 'should-not-exist.pid');
  const supervisor = new RunnerSupervisor({ store: base, onEvent: () => {} });
  try {
    await assert.rejects(
      supervisor.launch({
        threadId: 'thread',
        requestId: 'request',
        lease,
        payload: { kind: 'test', pid_file: pidFile },
        beforeStart: () => { throw new Error('acceptance failed'); },
      }),
      /acceptance failed/,
    );
    // Runner never received `start`, so it never wrote pidFile.
    await assert.rejects(access(pidFile));
    // Registry is clean and checkout lease is released immediately.
    assert.deepEqual(await new RunnerRegistry(base).scan(), []);
    const next = await acquireCheckout(root);
    await next.close();
  } finally {
    supervisor.detach();
    await lease.close();
    if (previous === undefined) delete process.env.PLAIT_RUNNER_TEST_TURNS; else process.env.PLAIT_RUNNER_TEST_TURNS = previous;
  }
});

test('registry scan error is unknown (never dead) and supervisor retries until reconnect succeeds', async t => {
  const { root, base } = await setup(t);
  const registry = new RunnerRegistry(base);
  const runId = 'scan-error-retry';
  await registry.writeManifest({ run_id: runId, thread_id: 'thread', request_id: 'request', checkout: root, harness: 'test', build_id: 'test' });
  const lock = await registry.acquire(runId);
  let attaches = 0;
  let seq = 0;
  const server = new RunnerServer(
    registry.socketPath(runId),
    () => ({ type: 'attached', state: 'running', seq, snapshot: {}, pending_approval: null, head: null, before_record_id: null }),
    command => { if (command.type === 'ping') server.publish({ type: 'pong', seq: ++seq, nonce: command.nonce }); },
    () => {
      attaches++;
      if (attaches >= 2) server.publish({ type: 'live', seq: ++seq, payload: { text: 'after-scan-error' } });
    },
  );
  await server.listen();
  const events: Event[] = [];
  let disconnected = false;
  const supervisor = new RunnerSupervisor({
    store: base,
    retryBaseMs: 25,
    onEvent: event => { events.push(event); },
    onDisconnect: () => { disconnected = true; },
  });
  // Fail the first scan after disconnect, then delegate to the real scan.
  const internalRegistry = (supervisor as unknown as { registry: RunnerRegistry }).registry;
  const realScan = internalRegistry.scan.bind(internalRegistry);
  let scanErrors = 0;
  internalRegistry.scan = async () => {
    if (scanErrors++ === 0) throw new Error('transient EIO on scan');
    return realScan();
  };
  try {
    await supervisor.attach(runId, 0);
    // Drop the active client by closing its socket; supervisor must treat the scan error as unknown and retry.
    ((supervisor as unknown as { client?: { close(): void } }).client)?.close();
    await waitFor(() => attaches >= 2 && events.some(e => e.type === 'live'));
    assert.equal(disconnected, false);
    assert.ok(scanErrors >= 2);
  } finally {
    supervisor.detach();
    await server.close();
    await lock.close();
  }
});

test('newly attached socket closing immediately still schedules retry and reconnects', async t => {
  const { root, base } = await setup(t);
  const registry = new RunnerRegistry(base);
  const runId = 'close-retry';
  await registry.writeManifest({ run_id: runId, thread_id: 'thread', request_id: 'request', checkout: root, harness: 'test', build_id: 'test' });
  const lock = await registry.acquire(runId);
  let attaches = 0;
  let seq = 0;
  const server = new RunnerServer(
    registry.socketPath(runId),
    () => ({ type: 'attached', state: 'running', seq, snapshot: {}, pending_approval: null, head: null, before_record_id: null }),
    command => { if (command.type === 'ping') server.publish({ type: 'pong', seq: ++seq, nonce: command.nonce }); },
    () => {
      attaches++;
      if (attaches === 2) {
        // Close the newly attached socket immediately on the next tick after `attached` is sent.
        const current = (server as unknown as { current?: { destroy(): void } }).current;
        setImmediate(() => current?.destroy());
      } else if (attaches >= 3) {
        server.publish({ type: 'live', seq: ++seq, payload: { text: 'third-attach-ok' } });
      }
    },
  );
  await server.listen();
  const events: Event[] = [];
  let disconnected = false;
  const supervisor = new RunnerSupervisor({
    store: base,
    retryBaseMs: 25,
    onEvent: event => { events.push(event); },
    onDisconnect: () => { disconnected = true; },
  });
  try {
    await supervisor.attach(runId, 0);
    // Trigger first disconnect -> attach #2 closes immediately -> must still schedule and reach attach #3.
    ((supervisor as unknown as { client?: { close(): void } }).client)?.close();
    await waitFor(() => attaches >= 3 && events.some(e => e.type === 'live'));
    assert.equal(disconnected, false);
    assert.equal(attaches, 3);
  } finally {
    supervisor.detach();
    await server.close();
    await lock.close();
  }
});

test('failed acceptance leaves registry files intact when runner stays live past stop timeout', async t => {
  const { root, base } = await setup(t);
  const lease = await acquireCheckout(root);
  const stubborn = join(base, 'stubborn-runner.mjs');
  const pidFile = join(base, 'stubborn.pid');
  // Minimal runner that acquires its lock, answers attach, and ignores stop so the lock stays held.
  await writeFile(stubborn, `
    import { writeFileSync } from 'node:fs';
    import { RunnerRegistry } from ${JSON.stringify(resolve('src/runner/registry.ts'))};
    import { RunnerServer } from ${JSON.stringify(resolve('src/runner/socket.ts'))};
    writeFileSync(${JSON.stringify(pidFile)}, String(process.pid));
    const registry = new RunnerRegistry(process.env.PLAIT_RUNNER_STORE);
    const id = process.env.PLAIT_RUNNER_ID;
    const lock = await registry.acquire(id);
    const server = new RunnerServer(registry.socketPath(id), () => ({ type: 'attached', state: 'preparing', seq: 0, snapshot: {}, pending_approval: null, head: null, before_record_id: null }), () => {});
    await server.listen();
    await new Promise(() => {});
  `);
  const supervisor = new RunnerSupervisor({ store: base, entry: stubborn, failedStopTimeoutMs: 150, onEvent: () => {} });
  let stubbornPid = 0;
  try {
    await assert.rejects(
      supervisor.launch({
        threadId: 'thread',
        requestId: 'request',
        lease,
        payload: { kind: 'test' },
        beforeStart: () => { throw new Error('acceptance failed'); },
      }),
      /acceptance failed/,
    );
    const { readFile } = await import('node:fs/promises');
    stubbornPid = Number((await readFile(pidFile, 'utf8')).trim());
    // Lock is still held, so registry files must NOT have been unlinked.
    const entries = await new RunnerRegistry(base).scan();
    assert.equal(entries.length, 1);
    assert.equal(entries[0].status, 'live');
  } finally {
    supervisor.detach();
    if (stubbornPid) { try { process.kill(stubbornPid, 'SIGKILL'); } catch {} }
    await lease.close();
  }
});

test('a runner that cannot listen explains why the launch failed', async t => {
  const { root, base } = await setup(t); const lease = await acquireCheckout(root);
  // A store this deep puts the runner socket past the Unix socket path limit.
  const store = join(base, 'x'.repeat(100)); await mkdir(store, { mode: 0o700 });
  const supervisor = new RunnerSupervisor({ store, onEvent: () => {} });
  try {
    await assert.rejects(supervisor.launch({ threadId: 'thread', requestId: 'request', lease, payload: { kind: 'test' } }), /Runner did not start[\s\S]*too long/);
  } finally { supervisor.detach(); await lease.close(); }
});

test('an attach that times out while connecting leaves no unhandled rejection', async t => {
  const { base } = await setup(t);
  const unhandled: unknown[] = []; const record = (error: unknown) => { unhandled.push(error); };
  process.on('unhandledRejection', record);
  try {
    const supervisor = new RunnerSupervisor({ store: base, onEvent: () => {} });
    // No runner listens, so connect fails; a 1 ms budget makes the timer fire around it.
    await assert.rejects(supervisor.attach('absent', 0, 1));
    await new Promise(resolve => setTimeout(resolve, 50));
    assert.deepEqual(unhandled, []);
  } finally { process.off('unhandledRejection', record); }
});
