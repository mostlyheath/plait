import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { inputMessage, type Snapshot } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { rootIdentity } from '../src/safety.ts';

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

for (const mode of ['one steer', 'two steers', 'rowless follow-up'] as const) {
  test(`restart resolves the active turn with ${mode}`, async t => {
    const directory = await mkdtemp(join(tmpdir(), 'plait-recovery-'));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const open = () => new Runtime(directory, createModels(), new Set());
    const first = open();
    const { store } = first;
    const snapshot: Snapshot = { id: 'thread', checkout: directory, model: { provider: 'faux', model_id: 'model' },
      status: 'running', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
    const before = (request: string) => store.db.prepare(`INSERT INTO records(id,thread,run_request,kind,checkout,state,created_at)
      VALUES (?, 'thread', ?, 'before', ?, 'failed', 0)`).run(request, request, directory);
    store.admit('thread', 'original', 'first', { accepted: true }, snapshot, await rootIdentity(directory));
    store.createRun('run', 'thread', 'original', 'pi');
    before('original');
    store.place('thread', 'original', 1);
    for (let entry = 2; entry <= (mode === 'two steers' ? 3 : 2); entry++) {
      const id = `steer-${entry}`;
      store.enqueue(id, id, { accepted: true }, { mode: 'steer', approval: 'ask' }, snapshot);
      snapshot.messages.push(inputMessage(store.waitingInput('thread', id)));
      store.place('thread', id, entry);
      store.save(snapshot);
    }
    store.enqueue('next', 'next', { accepted: true }, { mode: 'follow_up', approval: 'ask' }, snapshot);
    if (mode === 'rowless follow-up') {
      store.dequeue(store.waitingInput('thread', 'next'), snapshot);
      before('next'); // The older host records the turn but creates no runs row.
      store.place('thread', 'next', 4);
    }
    await first.close();
    const current = open();
    t.after(() => current.close());
    await current.recover();
    assert.equal(current.store.get('thread').status, 'interrupted');
    const turns = current.records.page('thread', undefined, 2, 7).records;
    const original = turns.at(-1)!;
    assert.deepEqual(original.request_ids, mode === 'two steers' ? ['original', 'steer-2', 'steer-3'] : ['original', 'steer-2']);
    assert.equal(original.outcome, mode === 'rowless follow-up' ? undefined : 'interrupted');
    if (mode === 'rowless follow-up') assert.deepEqual(turns[0].request_ids, ['next']);
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

for (const boundary of ['missing', 'undone'] as const) {
  test(`rollback leaves the earlier outcome unset when the active boundary is ${boundary}`, async t => {
    const directory = await mkdtemp(join(tmpdir(), 'plait-rollback-'));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const open = () => new Runtime(directory, createModels(), new Set());
    const first = open();
    const snapshot: Snapshot = { id: 'thread', checkout: directory, model: { provider: 'faux', model_id: 'model' },
      status: 'running', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
    first.store.admit('thread', 'original', 'first', { accepted: true }, snapshot, await rootIdentity(directory));
    first.store.createRun('run', 'thread', 'original', 'pi');
    first.store.place('thread', 'original', 1);
    first.store.db.exec(`INSERT INTO records(id,thread,run_request,kind,checkout,state,created_at)
      VALUES ('original','thread','original','before','/checkout','failed',0)`);
    await first.close();

    const previous = open();
    if (boundary === 'missing') {
      previous.store.admit('thread', 'active', 'later', { accepted: true }, snapshot);
    } else {
      previous.store.enqueue('active', 'later', { accepted: true }, { mode: 'follow_up', approval: 'ask' }, snapshot);
      previous.store.dequeue(previous.store.waitingInput('thread', 'active'), snapshot);
    }
    previous.store.place('thread', 'active', 2);
    if (boundary === 'undone') {
      previous.store.db.exec(`INSERT INTO records(id,thread,run_request,kind,before,checkout,state,created_at) VALUES
        ('active','thread','active','before',NULL,'/checkout','ready',1),
        ('undo','thread','active','undo','active','/checkout','undone',2)`);
    }
    await previous.close();

    const current = open();
    t.after(() => current.close());
    await current.recover();
    assert.equal(current.store.get('thread').status, 'interrupted');
    assert.equal(current.store.db.prepare("SELECT outcome FROM runs WHERE run_id='run'").get()?.outcome, null);
  });
}

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
