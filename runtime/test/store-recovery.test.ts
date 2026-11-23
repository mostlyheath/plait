import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import type { Snapshot } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';

for (const status of ['running', 'awaiting_approval'] as const) {
  test(`startup interrupts ${status} only during recovery`, async () => {
    const directory = await mkdtemp(join(tmpdir(), 'plait-recovery-'));
    const first = new Runtime(directory, createModels(), new Set());
    const snapshot: Snapshot = { id: 'thread', checkout: '/checkout', model: { provider: 'faux', model_id: 'model' },
      status, messages: [], tools: [], live_text: 'partial', pending_approval: null, error: null, truncated: false };
    first.store.admit('thread', 'request', 'first', { accepted: true }, snapshot);
    first.store.createRun('run', 'thread', 'request', 'pi');
    if (status === 'awaiting_approval') {
      snapshot.messages = []; // A long run can push its input out of the bounded snapshot.
      snapshot.truncated = true;
      first.store.save(snapshot);
    }
    first.store.db.prepare("INSERT INTO records(id,thread,run_request,kind,checkout,state,created_at) VALUES ('before','thread','request','before','/checkout','failed',0)").run();
    await first.close();
    const runtime = new Runtime(directory, createModels(), new Set());
    try {
      assert.equal(runtime.store.get('thread').status, status);
      await runtime.recover();
      const reply = await runtime.handle({ op: 'watch', thread_id: 'thread' }) as { thread: Snapshot };
      assert.equal(reply.thread.status, 'interrupted');
      assert.equal(runtime.records.page('thread', undefined, 1, 7).records[0].outcome, 'interrupted');
      assert.equal(reply.thread.live_text, '');
      assert.equal(reply.thread.error?.code, 'interrupted');
    } finally { await runtime.close(); await rm(directory, { recursive: true, force: true }); }
  });
}

test('recovery after rollback does not assign a rowless run to the earlier admission', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-rollback-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const open = () => new Runtime(directory, createModels(), new Set());
  const first = open();
  const snapshot: Snapshot = { id: 'thread', checkout: directory, model: { provider: 'faux', model_id: 'model' },
    status: 'running', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
  const admit = (host: Runtime, request: string) => host.store.admit(snapshot.id, request, request,
    { thread_id: snapshot.id, request_id: request, accepted: true }, snapshot);
  admit(first, 'original');
  first.store.createRun('run', snapshot.id, 'original', 'pi');
  await first.close();

  const previous = open();
  snapshot.status = 'interrupted';
  previous.store.save(snapshot);
  for (const request of ['completed', 'active']) {
    snapshot.status = 'running';
    admit(previous, request); // The previous host never created an in-process runs row.
    if (request === 'completed') { snapshot.status = 'idle'; previous.store.save(snapshot); }
  }
  await previous.close();

  const current = open();
  t.after(() => current.close());
  await current.recover();
  assert.equal(current.store.get(snapshot.id).status, 'interrupted');
  const row = current.store.db.prepare("SELECT * FROM runs WHERE run_id='run'").get()!;
  assert.equal(row.outcome, null);
  assert.equal(row.state, 'dead');
  assert.equal(typeof row.finished_at, 'number');
});

test('migration leaves completed runs without a reported outcome', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-migration-'));
  const old = new DatabaseSync(join(directory, 'host.sqlite'));
  // runs DDL from the protocol 6 store.
  old.exec(`CREATE TABLE runs(run_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, request_id TEXT NOT NULL, harness TEXT NOT NULL,
    state TEXT NOT NULL, last_seq INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, finished_at INTEGER);
    INSERT INTO runs VALUES ('old','thread','request','pi','finished',1,1,2)`);
  old.close();
  const runtime = new Runtime(directory, createModels(), new Set());
  runtime.store.db.exec(`INSERT INTO records(id,thread,run_request,kind,checkout,state,created_at)
    VALUES ('before','thread','request','before','/checkout','failed',0)`);
  try {
    assert.equal(runtime.store.db.prepare('SELECT outcome FROM runs').get()?.outcome, null);
    assert.equal('outcome' in runtime.records.page('thread', undefined, 1, 7).records[0], false);
  } finally { await runtime.close(); await rm(directory, { recursive: true, force: true }); }
});
