import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createModels } from '@earendil-works/pi-ai/models';
import { PROTOCOL_VERSIONS } from '../src/protocol.ts';
import type { Snapshot } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { applyRunnerReceipt } from '../src/runner/session.ts';

test('terminal receipt outcomes appear on every v7 fragment and survive replay', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-outcome-'));
  const runtime = new Runtime(dir, createModels(), new Set());
  const store = runtime.store;
  t.after(async () => { await runtime.close(); await rm(dir, { recursive: true, force: true }); });

  for (const [status, outcome] of [['finished', 'finished'], ['error', 'failed'], ['stopped', 'stopped']] as const) {
    const snapshot: Snapshot = { id: outcome, checkout: dir, model: { provider: 'faux', model_id: 'm' },
      status: 'running', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
    store.save(snapshot);
    store.createRun(outcome, snapshot.id, 'q', 'test');
    store.db.prepare(`INSERT INTO records(id,thread,run_request,kind,checkout,state,created_at,not_covered)
      VALUES (?,?,'q','before',?,'ready',1,?)`).run(outcome, snapshot.id, dir, JSON.stringify(Array.from({ length: 501 },
        (_, i) => ({ path: String(i), reason: 'ignored_limit' }))));
    const read = (protocol_version: number, before?: string) => runtime.handle({
      op: 'turn_records', protocol_version, thread_id: snapshot.id, ...(before ? { before } : {}),
    }) as Promise<{ before: string; records: Record<string, unknown>[] }>;
    const old = await read(5);
    assert.deepEqual(await read(7), old);
    if (status === 'stopped') {
      snapshot.status = 'stopped';
      store.saveRunEvent(outcome, 0, snapshot);
      assert.deepEqual(await read(7), old);
    }
    applyRunnerReceipt(store, outcome, snapshot, { result: { status } }, 0);
    assert.deepEqual(await read(5), old);
    assert.deepEqual(await read(6), old);
    const first = await read(7);
    assert.equal(first.records[0].outcome, outcome);
    assert.ok(first.before);
    assert.equal((await read(7, first.before)).records[0].outcome, outcome);
    const revision = runtime.records.revision(snapshot.id);
    assert.ok(revision > 0);
    applyRunnerReceipt(store, outcome, snapshot, { result: { status: 'error' } }, 0);
    assert.equal((await read(7)).records[0].outcome, outcome);
    assert.equal(runtime.records.revision(snapshot.id), revision);
    store.db.prepare(`INSERT INTO records(id,thread,run_request,kind,checkout,before,state,created_at)
      VALUES (?,?,'q','undo',?,?,'ready',2)`).run(`undo-${outcome}`, snapshot.id, dir, outcome);
    const undo = (await read(7)).records[0];
    assert.equal(undo.kind, 'undo');
    assert.equal('outcome' in undo, false);
  }
});

test('host documentation lists the supported opt-in protocol range', async () => {
  const guide = await readFile(new URL('../../docs/HOST.md', import.meta.url), 'utf8');
  assert.equal(guide.match(/versions (\d+–\d+) are opt-in/)?.[1], `2–${PROTOCOL_VERSIONS.at(-1)}`);
});
