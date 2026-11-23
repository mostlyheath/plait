import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { acquireCheckout } from '../src/safety.ts';
import { RunnerSupervisor } from '../src/runner/supervisor.ts';
import { RunnerRegistry } from '../src/runner/registry.ts';
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
      await waitFor(async () => (await new RunnerRegistry(base).scan())[0]?.status === 'finished');
      assert.ok((await new RunnerRegistry(base).scan())[0]?.receipt);
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
    await waitFor(async () => (await registry.scan())[0]?.status === 'live');
    await lease.close();
    await waitFor(async () => (await registry.scan())[0]?.status === 'dead');
    const next = await acquireCheckout(root); await next.close();
  } finally { try { process.kill(pid, 'SIGKILL'); } catch {} await lease.close(); }
});
