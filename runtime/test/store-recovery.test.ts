import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { realpath, rm } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { inputMessage, type Snapshot } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { rootIdentity } from '../src/safety.ts';

// A checkout's identity is read by its real path, and macOS's temp folder is a
// symlink, so every folder made here is resolved first.

for (const status of ['running', 'awaiting_approval'] as const) {
  test(`startup interrupts ${status} only during recovery`, async () => {
    const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-recovery-')));
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
    const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-recovery-')));
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
  const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-rollback-')));
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
    const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-rollback-')));
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
  const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-migration-')));
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

// Request writes as every build before `requests.consumed` made them (preview.40 through efefb79).
const older = {
  dequeue: (store: Runtime['store'], id: string) => store.db.prepare(`UPDATE requests SET state=NULL,queue=NULL,anchor=0
    WHERE scope='thread' AND id=? AND state IN ('queued','checkout_queued')`).run(id),
  place: (store: Runtime['store'], id: string, entry: number) => store.db.prepare(`UPDATE requests SET entry=?,state=NULL,queue=NULL
    WHERE scope='thread' AND id=?`).run(entry, id),
};
/** Returns the store to how it was before this change; the next open migrates it again. */
const unmigrate = (store: Runtime['store']) => store.db.exec(`DROP TRIGGER requests_consumed_insert;
  DROP TRIGGER requests_consumed_update; ALTER TABLE requests DROP COLUMN consumed`);

async function consumedFixture(t: { after(fn: () => unknown): void }) {
  const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-consumed-')));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const snapshot: Snapshot = { id: 'thread', checkout: directory, model: { provider: 'faux', model_id: 'model' },
    status: 'running', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
  const open = () => new Runtime(directory, createModels(), new Set());
  const identity = await rootIdentity(directory);
  /** Run A: a direct admission with a runs row, optionally with its before record, placed at `entry`. */
  const startA = (host: Runtime, entry: number, before = true, root = true) => {
    host.store.admit('thread', 'original', 'first', { accepted: true }, snapshot, root ? identity : undefined);
    host.store.createRun('run', 'thread', 'original', 'pi');
    if (before) host.store.db.prepare(`INSERT INTO records(id,thread,run_request,kind,checkout,state,created_at)
      VALUES ('original','thread','original','before',?,'failed',0)`).run(directory);
    host.store.place('thread', 'original', entry);
  };
  /** Shows a queued input in the transcript, as the run that takes it does. */
  const show = (host: Runtime, id: string) => {
    snapshot.messages.push(inputMessage(host.store.waitingInput('thread', id)));
    host.store.save(snapshot);
  };
  /** Queues an input; a steering input is shown at once, as the active run takes it, unless `shown` is false. */
  const queue = (host: Runtime, id: string, mode: 'steer' | 'follow_up', shown = mode === 'steer') => {
    host.store.enqueue(id, id, { accepted: true }, { mode, approval: 'ask' }, snapshot);
    if (shown) show(host, id);
  };
  /** An older build starts a queued follow-up as its own run: no runs row, no before record. */
  const olderStart = (host: Runtime, id: string, entry: number) => {
    show(host, id);
    older.dequeue(host.store, id);
    older.place(host.store, id, entry);
  };
  /** Restart on this build and return run A's row. */
  const recover = async () => {
    const current = open();
    t.after(() => current.close());
    await current.recover();
    assert.equal(current.store.get('thread').status, 'interrupted');
    lastHost = current;
    return current.store.db.prepare("SELECT outcome,state FROM runs WHERE run_id='run'").get()!;
  };
  let lastHost: Runtime | undefined;
  /** The request IDs of each retained turn record, newest first, after `recover`. */
  const turns = () => lastHost!.records.page('thread', undefined, 10, 7).records.map(record => record.request_ids);
  /** A run's outcome, after `recover`. */
  const outcome = (run: string) => lastHost!.store.db.prepare('SELECT outcome FROM runs WHERE run_id=?').get(run)?.outcome;
  return { snapshot, open, identity, startA, queue, olderStart, recover, turns, outcome };
}

test('a follow-up an older build starts as its own run ends the earlier turn', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  f.queue(host, 'next', 'follow_up');
  f.olderStart(host, 'next', 2); // After a rollback.
  await host.close();
  const row = await f.recover();
  assert.deepEqual({ ...row }, { outcome: null, state: 'dead' });
  // The rowless follow-up has no record of its own, and it is not part of A's turn.
  assert.deepEqual(f.turns(), [['original']]);
});

test('a steer placed before its snapshot was saved still resolves to the run the snapshot names', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  // Codex and Claude Code commit the placement first; the host stops before the snapshot shows it.
  f.queue(host, 'next', 'steer', false);
  host.store.place('thread', 'next', 2);
  await host.close();
  assert.equal((await f.recover()).outcome, 'interrupted');
  assert.deepEqual(f.turns(), [['original', 'next']]);
});

test('a second steer placed before the snapshot showed it resolves through the first steer', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  f.queue(host, 'b', 'steer'); // Saved: the snapshot names b.
  host.store.place('thread', 'b', 2);
  f.queue(host, 'c', 'steer', false); // Placed, but the host stops before the snapshot shows it.
  host.store.place('thread', 'c', 3);
  await host.close();
  assert.equal((await f.recover()).outcome, 'interrupted');
});

test('a run started after an unsaved steer is anchored above it across two restarts', async t => {
  const f = await consumedFixture(t);
  const first = f.open();
  f.startA(first, 1);
  first.store.save(f.snapshot, 1); // The snapshot and its head flush with entry 1.
  f.queue(first, 'b', 'steer', false);
  first.store.place('thread', 'b', 2); // Stored, but the head still says 1.
  await first.close();
  const second = f.open();
  await second.recover();
  assert.equal(second.store.db.prepare("SELECT outcome FROM runs WHERE run_id='run'").get()?.outcome, 'interrupted');
  const snapshot = second.store.get('thread');
  snapshot.status = 'running';
  second.store.admit('thread', 'd', 'next task', { accepted: true }, snapshot);
  second.store.createRun('run-d', 'thread', 'd', 'pi');
  await second.close(); // Stopped again before d was placed.
  await f.recover();
  assert.equal(f.outcome('run-d'), 'interrupted');
});

test('a submission that reuses an undo request ID still ends the earlier turn', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  // An undo request 'b' records its own before and undo pair under that ID.
  host.store.db.exec(`INSERT INTO records(id,thread,run_request,kind,before,checkout,state,created_at) VALUES
    ('undo-before','thread','b','before',NULL,'/checkout','ready',1), ('undo','thread','b','undo','undo-before','/checkout','undone',2)`);
  f.snapshot.status = 'running';
  host.store.admit('thread', 'b', 'later', { accepted: true }, f.snapshot); // A run of its own, without a before record.
  host.store.place('thread', 'b', 2);
  await host.close();
  await f.recover();
  assert(f.turns().some(ids => JSON.stringify(ids) === '["original"]'), JSON.stringify(f.turns()));
});

test('reopening replaces an earlier receipt trigger definition', async t => {
  const f = await consumedFixture(t);
  let host = f.open();
  // The broader insert trigger an intermediate revision of this change created.
  host.store.db.exec(`DROP TRIGGER requests_consumed_insert; CREATE TRIGGER requests_consumed_insert AFTER INSERT ON requests
    WHEN NEW.consumed IS NULL AND NEW.state IS NULL AND NEW.input IS NOT NULL
    BEGIN UPDATE requests SET consumed='started' WHERE rowid=NEW.rowid; END;`);
  await host.close();
  host = f.open();
  t.after(() => host.close());
  host.store.admit('create', 'create-request', [f.snapshot.checkout, 'faux', 'model'], { thread: f.snapshot }, f.snapshot, f.identity, [], 'git');
  assert.equal(host.store.db.prepare("SELECT consumed FROM requests WHERE scope='create'").get()?.consumed, null);
});

test('a steer whose snapshot names another request is left unresolved', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  f.queue(host, 'next', 'steer', false);
  host.store.place('thread', 'next', 2);
  // The snapshot names a request that is neither the steer nor its run.
  f.snapshot.messages.push(inputMessage({ request_id: 'elsewhere', text: 'elsewhere', entry: null, anchor: 0 }));
  host.store.save(f.snapshot);
  await host.close();
  assert.equal((await f.recover()).outcome, null);
});

test('create and worktree receipts are not marked as run starts', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  t.after(() => host.close());
  const thread = (id: string): Snapshot => ({ ...f.snapshot, id, messages: [] });
  // The shapes the host stores: create's argument list, and the worktree request object.
  host.store.admit('create', 'create-request', [f.snapshot.checkout, 'faux', 'model'], { thread: thread('created') }, thread('created'), f.identity, [], 'git');
  host.store.admit('worktree_create', 'worktree-request', { parent: f.snapshot.checkout, harness: 'pi' }, { thread_id: 'tree' }, thread('tree'), f.identity, [], 'git');
  host.store.admit('created', 'first', 'first message', { accepted: true }, thread('created'));
  const consumed = (scope: string) => host.store.db.prepare('SELECT consumed FROM requests WHERE scope=?').get(scope)?.consumed;
  assert.deepEqual([consumed('create'), consumed('worktree_create'), consumed('created')], [null, null, 'started']);
});

test('a message an older build steers into the active run keeps that run its outcome', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  f.queue(host, 'next', 'steer');
  older.place(host.store, 'next', 2);
  await host.close();
  assert.equal((await f.recover()).outcome, 'interrupted');
});

test('a follow-up queued before the migration and started by an older build after it ends the turn', async t => {
  const f = await consumedFixture(t);
  let host = f.open();
  host.store.admit('thread', 'earlier', 'earlier', { accepted: true }, f.snapshot, f.identity);
  host.store.place('thread', 'earlier', 1);
  f.queue(host, 'next', 'follow_up');
  unmigrate(host.store); // `next` was queued by a build without the column.
  await host.close();
  host = f.open();
  f.startA(host, 2, true, false);
  f.olderStart(host, 'next', 3); // Rolled back again.
  await host.close();
  assert.equal((await f.recover()).outcome, null);
});

test('a steering message cancelled after its run took it is still steering', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  f.queue(host, 'next', 'steer');
  assert.equal(host.store.cancel('thread', 'next'), 1);
  host.store.place('thread', 'next', 2); // The run's placement arrives after the cancellation.
  assert.equal(host.store.db.prepare("SELECT consumed FROM requests WHERE id='next'").get()?.consumed, 'steered');
  await host.close();
  assert.equal((await f.recover()).outcome, 'interrupted');
});

test('marked steering in a plain folder belongs to the nearest marked start', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1, false); // No before records: a folder project.
  f.queue(host, 'next', 'steer');
  host.store.place('thread', 'next', 2);
  await host.close();
  assert.equal((await f.recover()).outcome, 'interrupted');
});

test('inputs consumed before the column existed keep the earlier inference', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  f.startA(host, 1);
  f.queue(host, 'next', 'follow_up');
  f.olderStart(host, 'next', 2);
  unmigrate(host.store);
  await host.close();
  // Without before records, a placed follow-up waiter was inferred to be steering, as before this change.
  assert.equal((await f.recover()).outcome, 'interrupted');
});

test('consumption is recorded as inputs start runs or steer them', async t => {
  const f = await consumedFixture(t);
  const host = f.open();
  t.after(() => host.close());
  const { store } = host;
  store.admit('thread', 'direct', 'first', { accepted: true }, f.snapshot, f.identity);
  store.place('thread', 'direct', 1);
  f.queue(host, 'steer', 'steer');
  store.place('thread', 'steer', 2);
  f.queue(host, 'later', 'follow_up');
  store.dequeue(store.waitingInput('thread', 'later'), f.snapshot);
  store.place('thread', 'later', 3);
  f.queue(host, 'dropped', 'follow_up');
  store.cancel('thread', 'dropped');
  f.queue(host, 'waiting', 'follow_up');
  const consumed = Object.fromEntries(store.db.prepare("SELECT id,consumed FROM requests WHERE scope='thread'").all()
    .map(row => [String(row.id), row.consumed]));
  assert.deepEqual(consumed, { direct: 'started', steer: 'steered', later: 'started', dropped: null, waiting: null });
});

test('migration leaves existing requests unknown and marks what older builds write later', async () => {
  const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-consumed-')));
  try {
    const old = new DatabaseSync(join(directory, 'host.sqlite'));
    // The requests table and a row as a build before this column wrote them.
    old.exec(`CREATE TABLE requests(scope TEXT NOT NULL, id TEXT NOT NULL, hash TEXT NOT NULL, result TEXT NOT NULL, input TEXT, entry INTEGER, anchor INTEGER,
      state TEXT, queue TEXT, PRIMARY KEY(scope,id));
      INSERT INTO requests (scope,id,hash,result,input,anchor) VALUES ('thread','a','h','{}','"a"',0)`);
    old.close();
    for (let open = 0; open < 2; open++) {
      const runtime = new Runtime(directory, createModels(), new Set());
      const { store } = runtime;
      const consumed = (id: string) => store.db.prepare('SELECT consumed FROM requests WHERE id=?').get(id)?.consumed;
      assert.equal(store.db.prepare("SELECT COUNT(*) AS n FROM sqlite_master WHERE type='trigger' AND name LIKE 'requests_consumed_%'").get()?.n, 2);
      if (open === 0) {
        assert.equal(consumed('a'), null, 'consumed before the column existed');
        // An older build's own statements, after the migration. It saves the thread before admitting.
        store.db.prepare(`INSERT INTO threads (id,snapshot,checkout) VALUES ('thread','{}','/checkout')`).run();
        store.db.prepare(`INSERT INTO requests (scope,id,hash,result,input,anchor) VALUES ('thread','b','h','{}','"b"',0)`).run();
        store.db.prepare(`INSERT INTO requests (scope,id,hash,result,input,anchor,state,queue) VALUES
          ('thread','queued','h','{}','"q"',0,'queued','{}'),('thread','steer','h','{}','"s"',0,'queued','{}')`).run();
        assert.equal(consumed('queued'), null, 'still queued');
        older.dequeue(store, 'queued');
        older.place(store, 'steer', 4);
        store.db.prepare(`INSERT INTO requests (scope,id,hash,result) VALUES ('other','dedup','h','{}')`).run();
      }
      assert.deepEqual([consumed('b'), consumed('queued'), consumed('steer'), consumed('dedup')], ['started', 'started', 'steered', null]);
      await runtime.close();
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});
