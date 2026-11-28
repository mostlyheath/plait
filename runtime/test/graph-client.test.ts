import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { rm, readdir, mkdir, realpath, rename, symlink, unlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { Runtime } from '../src/runtime.ts';
import { exec } from './support/safety.ts';
import { Storage } from '../src/graph/storage.ts';
import { HostTransportError } from '../src/host-client.ts';
import { Graph } from '../src/graph/host.ts';
import { randomUUID } from 'node:crypto';
import { excerpt } from '../src/graph/text.ts';
import type { Request } from '../src/graph/requests.ts';
import { rootIdentity } from '../src/safety.ts';
import { DatabaseSync } from 'node:sqlite';

const draft = { op: 'create_draft', checkout: '/p', title: 'Child', prompt: 'Research', harness: 'pi', provider: 'example', model_id: 'model' };
const thread = (id: string) => ({
  id, checkout: '/p', title: id, harness: { id: 'pi' }, model: { provider: 'example', model_id: 'model' },
  status: 'idle', archived: false, approval_mode: 'ask', error: null, messages: [{ role: 'assistant', text: 'done' }]
});
async function fixture(t: TestContext, probe?: 'fixed' | ((path: string) => Promise<{ project: string; identity: string }>)) {
  const dir = await realpath(await fixtureDirectory(join(tmpdir(), 'graph-client-')));
  const root = join(await realpath(dir), 'project');
  await mkdir(root);
  const calls: Request[] = [];
  const threads = new Map([['parent', { ...thread('parent'), checkout: root }]]);
  const identity = await rootIdentity(root);
  const queued = new Set<string>();
  const control = { admission: async () => {}, parentRoot: null as string | null, history: '\u0001😀'.repeat(12000), busy: false, refused: false, historyDelay: async () => {} };
  const graph = new Graph(dir, async r => {
    calls.push(r);
    switch (r.op) {
      case 'stored_thread': return { thread: threads.get(String(r.thread_id)) ?? assert.fail('unknown thread') };
      case 'list_threads': return { threads: [...threads.values()].map(t => ({ ...t, harness: t.harness.id })), next_cursor: null };
      case 'history': await control.historyDelay(); return { items: [{ id: 'm', type: 'message', role: 'assistant', text: control.history }], before: null };
      case 'create_admitted_thread': {
        await control.admission();
        if (control.busy && r.placement === 'project') return { admission: { state: 'choice' } };
        const id = 'thread-' + r.request_id;
        if (!threads.has(id)) threads.set(id, { ...thread(id), checkout: root, messages: [] });
        return { admission: { state: 'ready', checkout: root, worktree: control.parentRoot && { parent: { root: control.parentRoot, identity: { ...identity, ino: '0' } } }, thread_id: id } };
      }
      case 'submit_checkout': {
        queued.add(String(r.request_id));
        threads.get(String(r.thread_id))!.messages = [{ role: 'assistant', text: 'done' }];
        return { accepted: true };
      }
      default: assert.fail('unexpected ' + r.op);
    }
  }, async () => identity, (_, request) => queued.has(request) ? {
    state: control.refused ? 'failed' : control.busy ? 'waiting' : 'started',
    error: control.refused ? { message: 'Project policy changed' } : null
  } : undefined, () => identity, probe === 'fixed' ? async project => ({ project, identity: JSON.stringify(identity) }) : probe);
  t.after(async () => { await graph.close(); await rm(dir, { recursive: true, force: true }); });
  const create = (r: Request = {}) => graph.handle(r.op === 'fork_thread' ? { op_id: randomUUID(), title: draft.title, harness: draft.harness, provider: draft.provider, model_id: draft.model_id, ...r } : { ...draft, checkout: root, op_id: randomUUID(), ...r }) as Promise<{ node_id: string }>;
  const view = () => graph.handle({ op: 'graph', project: root }) as Promise<{ graph_revision: number; nodes: { id: string; status: string; reason: string; unblocks: number }[] }>;
  const refresh = () => graph.reconcile(root);
  return { dir, root, graph, threads, calls, control, create, view, refresh };
}
test('durable create intent rejects changed payloads, cycles and replay after delete', async t => {
  const h = await fixture(t);
  const first = await h.create({ op_id: 'one' });
  assert.equal((await h.create({ op_id: 'one' })).node_id, first.node_id);
  await assert.rejects(h.create({ op_id: 'one', title: 'Other' }), /different input/);
  const second = await h.create({ depends_on: [first.node_id] });
  await assert.rejects(h.graph.handle({ op: 'set_dependencies', node_id: first.node_id, depends_on: [second.node_id] }), /cycle/);
  await h.graph.handle({ op: 'delete_draft', node_id: first.node_id });
  await assert.rejects(h.create({ op_id: 'one' }), /deleted/);
  assert.deepEqual((await h.view()).nodes.map(n => n.id), [second.node_id]);
});
test('automatic start uses ordinary deterministic requests and waits for clean archived dependencies', async t => {
  // Identity is fixed in this admission test. The separate probe tests exercise slow disks.
  const h = await fixture(t, 'fixed');
  const schedule = h.graph.wake.bind(h.graph);
  // Trigger one ordinary scheduled scan once the dependency is clean.
  t.mock.method(h.graph, 'wake', () => {});
  const p = h.threads.get('parent')!;
  p.archived = true; p.status = 'interrupted';
  const { node_id } = await h.create({ depends_on: ['parent'] });
  await h.refresh();
  assert.equal((await h.view()).nodes.find(n => n.id === node_id)?.status, 'blocked');
  assert.equal(h.calls.some(r => r.op === 'submit_checkout'), false);
  p.status = 'idle';
  schedule(h.root);
  // The bound only diagnoses a stuck scheduler. Submission itself releases the wait.
  await t.waitFor(() => assert.ok(h.calls.some(r => r.op === 'submit_checkout')), { timeout: 30_000 });
  await h.refresh(); await h.refresh();
  assert.equal(h.calls.find(r => r.op === 'create_admitted_thread')?.request_id, 'create-' + node_id);
  const submit = h.calls.filter(r => r.op === 'submit_checkout');
  assert.equal(submit.length, 1); assert.equal(submit[0].request_id, 'submit-' + node_id);
  assert.deepEqual(submit[0].expect, { harness: 'pi', provider: 'example', model_id: 'model', mode: null, thinking: null, approval: 'ask' });
  assert.match(String(submit[0].text), /plait:\/\/thread\/parent/);
  const call = h.calls.length;
  const page = await h.view();
  assert.deepEqual(h.calls.slice(call).map(r => r.op), ['list_threads']);
  assert.deepEqual(await h.graph.handle({ op: 'graph', project: h.root, revision: page.graph_revision }), { unchanged: true, graph_revision: page.graph_revision });
});
test('a lost admission reply retries the persisted admission ID', async t => {
  const h = await fixture(t);
  t.mock.method(h.graph, 'wake', () => {}); // This test advances reconciliation explicitly.
  const { node_id } = await h.create({ depends_on: ['parent'] });
  h.control.admission = async () => { throw new HostTransportError('lost reply'); };
  await h.refresh();
  h.control.admission = async () => {};
  await h.refresh();
  assert.deepEqual(h.calls.filter(r => r.op === 'create_admitted_thread').map(r => r.request_id),
    ['create-' + node_id, 'create-' + node_id]);
  assert.deepEqual(h.calls.filter(r => r.op === 'submit_checkout').map(r => r.request_id), ['submit-' + node_id]);
});
test('a waiting queue receipt stays Ready and a policy refusal blocks the draft', async t => {
  const h = await fixture(t);
  h.control.busy = true;
  const { node_id } = await h.create({ depends_on: ['parent'] });
  await h.refresh();
  assert.equal((await h.view()).nodes.find(n => n.id === node_id)?.status, 'ready');
  assert.deepEqual(h.calls.filter(r => r.op === 'create_admitted_thread').map(r => r.placement), ['project', 'wait']);
  for (const edit of [{ op: 'set_node_kind', kind: 'discuss' }, { op: 'set_dependencies', depends_on: [] },
    { op: 'set_start_when_ready', start_when_ready: false }, { op: 'delete_draft' }]) {
    await assert.rejects(h.graph.handle({ ...edit, node_id }), { message: 'This draft is waiting to start. Cancel it first.' });
  }
  h.control.refused = true;
  h.control.busy = false;
  await h.refresh();
  const card = (await h.view()).nodes.find(n => n.id === node_id)!;
  assert.equal(card.status, 'blocked'); assert.equal(card.reason, 'Project policy changed');
  assert.equal(h.calls.filter(r => r.op === 'submit_checkout').length, 1);
});
test('Send results limits children and escaped bytes', async t => {
  const h = await fixture(t);
  for (let i = 0; i < 21; i++) {
    const { node_id } = await h.create({ op: 'fork_thread', parent_id: 'parent', seed_mode: 'brief', brief: 'go' } as Request);
    await h.graph.handle({ op: 'start_draft', node_id });
  }
  for (let i = 0; i < 10; i++) {
    await h.refresh();
    if ((await h.view()).nodes.filter(n => n.status === 'done').length === 22) break;
  }
  const call = h.calls.length;
  const { text } = await h.graph.handle({ op: 'send_results', parent_id: 'parent' }) as { text: string };
  assert.equal(h.calls.slice(call).filter(r => r.op === 'history').length, 20);
  assert.ok(Buffer.byteLength(JSON.stringify(text)) <= 64000);
  assert.match(text, /1 results left out/);
  let now = 0;
  t.mock.method(performance, 'now', () => now);
  h.control.historyDelay = async () => { now += 1100; };
  const partial = await h.graph.handle({ op: 'send_results', parent_id: 'parent' }) as { text: string };
  assert.equal(partial.text.match(/plait:\/\/thread/g)?.length, 3);
  assert.match(partial.text, /18 results left out/);
  assert.equal(now, 4400);

  // The fourth read never replies. Only the remaining 700 ms are available.
  now = 0;
  h.control.historyDelay = async () => {
    if (now === 3300) await new Promise<void>(() => {});
    now += 1100;
  };
  const pending = await h.graph.handle({ op: 'send_results', parent_id: 'parent' });
  assert.deepEqual(pending, partial);
});
test('fork stops at twenty pages', async t => {
  let count = 0;
  await assert.rejects(excerpt(async () => { count++; return { items: [], before: 'older' }; }, 'p', 'missing'), /twenty pages/);
  assert.equal(count, 20);

});
test('flag unset preserves ordinary runtime dispatch and opens no graph store', async t => {
  const previous = process.env.PLAIT_GRAPH;
  delete process.env.PLAIT_GRAPH;
  const dir = await realpath(await fixtureDirectory(join(tmpdir(), 'ordinary-host-')));
  const host = new Runtime(dir, createModels(), new Set());
  t.after(async () => {
    await host.close(); await rm(dir, { recursive: true, force: true });
    if (previous !== undefined) process.env.PLAIT_GRAPH = previous;
  });
  host.startBackground(request => host.handle(request));
  assert.deepEqual(await host.handle({ op: 'list_threads', protocol_version: 6 }), { threads: [], next_cursor: null });
  await assert.rejects(host.handle({ op: 'graph', project: '/p' }), { code: 'invalid_request' });
  assert.ok(!(await readdir(dir)).includes('graph.sqlite'));
});

test('feature-on runtime admits a draft and submits through the host handler', async t => {
  const previous = process.env.PLAIT_GRAPH;
  process.env.PLAIT_GRAPH = '1';
  const dir = await realpath(await fixtureDirectory(join(tmpdir(), 'graph-integration-')));
  const { mkdir } = await import('node:fs/promises');
  const root = join(dir, 'checkout'), store = join(dir, 'store');
  await mkdir(root); await mkdir(store);
  await exec('git', ['init', '-q', root]);
  const { fauxProvider, fauxAssistantMessage } = await import('@earendil-works/pi-ai/providers/faux');
  const models = createModels(), faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('finished')]); models.setProvider(faux.provider);
  const host = new Runtime(store, models, new Set(['faux']));
  t.after(async () => {
    await host.close(); await rm(dir, { recursive: true, force: true });
    if (previous === undefined) delete process.env.PLAIT_GRAPH;
    else process.env.PLAIT_GRAPH = previous;
  });
  let socketBusy = false;
  host.startBackground(async request => {
    socketBusy = true;
    try { return await host.handle(request); }
    finally { socketBusy = false; }
  });
  const info = host.admissions.info.bind(host.admissions);
  let entered!: () => void, release!: () => void;
  const probing = new Promise<void>(resolve => entered = resolve);
  const gate = new Promise<void>(resolve => release = resolve);
  host.admissions.info = async path => { entered(); await gate; return info(path); };
  const created = await host.reply({ ...draft, op_id: 'integration', checkout: await realpath(root), provider: 'faux', model_id: 'faux-1' });
  assert.ok(created.ok);
  const { node_id } = created.result as { node_id: string };
  assert.ok((await host.reply({ op: 'start_draft', node_id })).ok);
  try {
    await probing;
    assert.equal(socketBusy, false, 'initial graph placement held the host socket');
    assert.ok((await host.reply({ op: 'graph', project: root })).ok);
  } finally { release(); }
  const deadline = Date.now() + 15000;
  for (; ;) {
    const { threads } = await host.handle({ op: 'list_threads', protocol_version: 6 }) as { threads: { id: string }[] };
    if (threads.length) {
      const { thread } = await host.handle({ op: 'watch', protocol_version: 6, thread_id: threads[0].id }) as { thread: { status: string; messages: { text: string }[]; error: unknown } };
      assert.equal(thread.error, null);
      if (thread.status === 'idle' && thread.messages.at(-1)?.text === 'finished') {
        return;
      }
    }
    const page = await host.handle({ op: 'graph', project: root }) as { nodes: { reason: string | null }[] };
    for (const n of page.nodes) assert.equal(n.reason, null);
    assert.ok(Date.now() < deadline, 'Timed out waiting for the ordinary submit');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
});

test('500 visible drafts have stable layout and a bounded page', async t => {
  const h = await fixture(t);
  const first = await h.create();
  for (let i = 0; i < 500; i++) await h.create({ depends_on: [first.node_id], start_when_ready: false });
  const page = await h.graph.handle({ op: 'graph', project: h.root }) as {
    nodes: { id: string; unblocks: number }[]; edges: unknown[]; truncated: boolean; layout: { placed: unknown[] }
  };
  assert.equal(page.nodes.length, 500);
  assert.equal(page.edges.length, 499);
  assert.equal(page.nodes[0].unblocks, 500);
  assert.equal(page.truncated, true);
  assert.equal(page.layout.placed.length, 500);
  await h.graph.handle({ op: 'set_node_kind', node_id: first.node_id, kind: 'discuss' });
  assert.deepEqual((await h.graph.handle({ op: 'graph', project: h.root }) as typeof page).layout, page.layout);
});

test('reopening the graph preserves creation receipts without repeating the queued submission', async t => {
  const dir = await realpath(await fixtureDirectory(join(tmpdir(), 'graph-reopen-')));
  const submissions: Request[] = [];
  const identity = await rootIdentity(dir);
  const receipt = () => submissions.length ? { state: "started", error: null } : undefined;
  const send = async (r: Request) => {
    switch (r.op) {
      case 'stored_thread': return { thread: { ...thread('destination'), checkout: await realpath(dir), messages: [] } };
      case 'create_admitted_thread': return { admission: { state: 'ready', checkout: dir, thread_id: 'destination' } };
      case 'submit_checkout':
        submissions.push(r);
        return { accepted: true };
      default: assert.fail(String(r.op));
    }
  };
  let graph = new Graph(dir, send, async () => identity, receipt, () => identity);
  t.after(async () => { await graph.close(); await rm(dir, { recursive: true, force: true }); });
  const create = { ...draft, checkout: await realpath(dir), op_id: 'start-once', approval: 'auto' };
  const { node_id } = await graph.handle(create) as { node_id: string };
  await graph.handle({ op: 'start_draft', node_id });
  await graph.reconcile(dir); await graph.close();
  graph = new Graph(dir, send, async () => identity, receipt, () => identity);
  assert.equal((await graph.handle(create) as { node_id: string }).node_id, node_id);
  await graph.reconcile(dir); await graph.reconcile(dir);
  assert.equal(submissions.length, 1);
});

async function waitForCard(h: Awaited<ReturnType<typeof fixture>>, id: string, visible: boolean) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if ((await h.view()).nodes.some(n => n.id === id) === visible) return;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  assert.fail('Graph identity did not refresh');
}

test('graph polls and edits finish while a single identity refresh is stalled', { timeout: 3000 }, async t => {
  let release!: () => void;
  const stalled = new Promise<void>(resolve => { release = resolve; });
  const probe = t.mock.fn(async (project: string) => {
    await stalled;
    return { project, identity: JSON.stringify(await rootIdentity(project)) };
  });
  const h = await fixture(t, probe);
  const { node_id } = await h.create({ depends_on: ['parent'], start_when_ready: false });
  h.threads.set('elsewhere', thread('elsewhere'));
  try {
    const first = await h.view();
    assert.deepEqual(first.nodes.map(n => n.id), ['parent']);
    assert.ok(!JSON.stringify(first).includes(node_id));
    assert.equal((first.nodes[0] as { title?: string }).title, '');
    assert.equal(first.nodes[0].unblocks, 0);
    assert.deepEqual((first as { edges?: unknown }).edges, []);
    assert.equal((first as { unlinked?: number }).unlinked, 0);
    await new Promise(resolve => setImmediate(resolve));
    const next = await h.graph.handle({ op: 'graph', project: h.root, revision: first.graph_revision });
    assert.deepEqual(next, { unchanged: true, graph_revision: first.graph_revision });
    await h.create();
    assert.equal(probe.mock.callCount(), 1);
  } finally {
    release();
  }
  await waitForCard(h, node_id, true);
});

test('stalled graph probes leave a filesystem worker free in small pools', async t => {
  for (const [size, expectedProbes] of [['1', 0], ['2', 1], ['\u00a02', 0]] as const) {
    await t.test(`UV_THREADPOOL_SIZE=${JSON.stringify(size)}`, async t => {
      const previous = process.env.UV_THREADPOOL_SIZE;
      process.env.UV_THREADPOOL_SIZE = size;
      t.after(() => {
        if (previous === undefined) delete process.env.UV_THREADPOOL_SIZE;
        else process.env.UV_THREADPOOL_SIZE = previous;
      });
      let release!: () => void;
      const stalled = new Promise<void>(resolve => { release = resolve; });
      const probe = t.mock.fn(async (project: string) => {
        await stalled;
        return { project, identity: 'unused' };
      });
      const h = await fixture(t, probe);
      try {
        for (const name of ['a', 'b', 'c'])
          await h.graph.handle({ op: 'graph', project: join(h.dir, name) });
        assert.equal(probe.mock.callCount(), expectedProbes);
      } finally { release(); }
    });
  }
});

test('revision polls recover drafts when every identity check takes 75 ms', async t => {
  let completed!: () => void;
  const firstProbe = new Promise<void>(resolve => { completed = resolve; });
  const h = await fixture(t, async project => {
    await new Promise(resolve => setTimeout(resolve, 75));
    const identity = JSON.stringify(await rootIdentity(project));
    completed();
    return { project, identity };
  });
  h.threads.clear();
  const draft = await h.create();
  const fallback = await h.view();
  assert.deepEqual(fallback.nodes, []);
  await firstProbe;
  await new Promise(resolve => setImmediate(resolve));
  const db = new DatabaseSync(join(h.dir, 'graph.sqlite'), { readOnly: true });
  try {
    assert.ok(Number(db.prepare('SELECT revision FROM version').get()!.revision) > fallback.graph_revision);
  } finally { db.close(); }
  const next = await h.graph.handle({ op: 'graph', project: h.root, revision: fallback.graph_revision }) as typeof fallback;
  assert.notEqual(next.graph_revision, fallback.graph_revision);
  assert.deepEqual(next.nodes.map(n => n.id), [draft.node_id]);
  await rename(h.root, h.root + '-old');
  await mkdir(h.root);
  const replacement = await h.graph.handle({ op: 'graph', project: h.root, revision: next.graph_revision }) as typeof fallback;
  assert.deepEqual(replacement.nodes, []);
});

test('a failed refresh clears confirmed cards and recovery invalidates the fallback revision', async t => {
  let unavailable = false;
  const h = await fixture(t, async project => {
    if (unavailable) throw new Error('drive unavailable');
    return { project, identity: JSON.stringify(await rootIdentity(project)) };
  });
  h.threads.clear();
  const draft = await h.create();
  const confirmed = await h.view();
  assert.deepEqual(confirmed.nodes.map(n => n.id), [draft.node_id]);
  const poll = (revision: number) => h.graph.handle({ op: 'graph', project: h.root, revision }) as Promise<typeof confirmed>;
  unavailable = true;
  const fallback = await poll(confirmed.graph_revision);
  assert.deepEqual(fallback.nodes, []);
  unavailable = false;
  assert.deepEqual((await poll(fallback.graph_revision)).nodes.map(n => n.id), [draft.node_id]);
});

test('the first replacement poll hides old drafts even with two other probes stalled', async t => {
  let release!: () => void;
  const stalled = new Promise<void>(resolve => { release = resolve; });
  const calls: string[] = [];
  const h = await fixture(t, async path => {
    calls.push(path);
    if (path.endsWith('-stalled')) await stalled;
    return { project: path, identity: JSON.stringify(await rootIdentity(path)) };
  });
  const old = await h.create({ title: 'Private old title' });
  const before = await h.view();
  assert.equal(before.nodes[0].id, old.node_id);
  try {
    for (const name of ['one-stalled', 'two-stalled'])
      await h.graph.handle({ op: 'graph', project: join(h.dir, name) });
    await rename(h.root, h.root + '-old');
    await mkdir(h.root);
    const after = await h.graph.handle({ op: 'graph', project: h.root, revision: before.graph_revision }) as typeof before;
    assert.deepEqual(after.nodes.map(n => n.id), ['parent']);
    assert.deepEqual(calls, [h.root, join(h.dir, 'one-stalled'), join(h.dir, 'two-stalled')]);
  } finally { release(); }
});

test('retargeting an alias invalidates the revision even for a previously polled destination', async t => {
  const h = await fixture(t);
  const other = join(h.dir, 'other'), alias = join(h.dir, 'alias');
  await mkdir(other);
  const a = await h.create(), b = await h.create({ checkout: other });
  await h.graph.handle({ op: 'graph', project: other });
  await symlink(h.root, alias);
  const before = await h.graph.handle({ op: 'graph', project: alias }) as Awaited<ReturnType<typeof h.view>>;
  assert.deepEqual(before.nodes.map(n => n.id), [a.node_id]);
  await unlink(alias);
  await symlink(other, alias);
  const after = await h.graph.handle({ op: 'graph', project: alias, revision: before.graph_revision }) as typeof before;
  assert.deepEqual(after.nodes.map(n => n.id), [b.node_id]);
  assert.notEqual(after.graph_revision, before.graph_revision);
});

test('alias revisions expire across upgrade and rollback with a previously cached destination', async t => {
  const h = await fixture(t);
  h.threads.clear();
  const other = join(h.dir, 'other'), alias = join(h.dir, 'alias');
  await mkdir(other);
  const a = await h.create(), b = await h.create({ checkout: other });
  const db = new DatabaseSync(join(h.dir, 'graph.sqlite'));
  t.after(() => db.close());
  db.exec('DROP TRIGGER page_insert');
  // Reproduce the previous reader's canonical key, signature, and revision check.
  async function legacyPoll(revision?: number) {
    const project = await realpath(alias);
    const identity = JSON.stringify(await rootIdentity(project));
    const signature = JSON.stringify([identity, false, []]);
    const first = !db.prepare('SELECT 1 FROM pages WHERE project=?').get(project);
    db.prepare(`INSERT INTO pages VALUES(?,?) ON CONFLICT(project) DO UPDATE SET
      summary=excluded.summary WHERE summary<>excluded.summary`).run(project, signature);
    const current = Number(db.prepare('SELECT revision FROM version').get()!.revision);
    return { revision: current, unchanged: !first && current === revision };
  }
  async function retarget(project: string) { await unlink(alias); await symlink(project, alias); }
  await symlink(other, alias);
  await legacyPoll();
  await retarget(h.root);
  const legacy = await legacyPoll();
  const reopened = new Storage(h.dir);
  const migrationRevision = reopened.revision();
  reopened.db.close();
  await retarget(other);
  const poll = (revision?: number) => h.graph.handle({ op: 'graph', project: alias, revision }) as ReturnType<typeof h.view>;
  const upgraded = await poll();
  assert.ok(upgraded.graph_revision > migrationRevision);
  assert.deepEqual((await poll(legacy.revision)).nodes.map(n => n.id), [b.node_id]);
  await retarget(h.root);
  const beforeRollback = await poll(upgraded.graph_revision);
  assert.deepEqual(beforeRollback.nodes.map(n => n.id), [a.node_id]);
  await retarget(other);
  assert.ok((await legacyPoll()).revision > beforeRollback.graph_revision);
  assert.equal((await legacyPoll(beforeRollback.graph_revision)).unchanged, false);
});

test('a cross-project edge leaves the replacement thread unlinked', async t => {
  const h = await fixture(t);
  const old = await h.create();
  await rename(h.root, h.root + '-old');
  await mkdir(h.root);
  const identity = JSON.stringify(await rootIdentity(h.root));
  const db = new DatabaseSync(join(h.dir, 'graph.sqlite'));
  t.after(() => db.close());
  db.prepare('INSERT INTO nodes(id,thread,project,project_identity,started,title) VALUES(?,?,?,?,1,?)')
    .run('parent', 'parent', h.root, identity, 'parent');
  const before = await h.view() as Awaited<ReturnType<typeof h.view>> & { unlinked: number; edges: unknown[] };
  assert.equal(before.unlinked, 1);
  await h.graph.handle({ op: 'set_dependencies', node_id: old.node_id, depends_on: ['parent'] });
  const after = await h.view() as typeof before;
  assert.deepEqual(after.nodes, before.nodes);
  assert.deepEqual(after.edges, before.edges);
  assert.equal(after.unlinked, 1);
});

test('a replacement sees only its started nodes and dependency counts', async t => {
  const h = await fixture(t);
  const old = await h.create({ depends_on: ['parent'], start_when_ready: false });
  await rename(h.root, h.root + '-original');
  await mkdir(h.root);
  const current = await h.create();
  await h.graph.handle({ op: 'start_draft', node_id: current.node_id });
  await h.refresh();
  await h.create({ depends_on: [current.node_id], start_when_ready: false });
  // A historical edge can cross identities at the same path.
  await h.graph.handle({ op: 'set_dependencies', node_id: old.node_id, depends_on: ['parent', current.node_id] });
  const nodes = (await h.view()).nodes;
  assert.ok(!nodes.some(n => n.id === 'parent' || n.id === old.node_id));
  assert.equal(nodes.find(n => n.id === current.node_id)?.unblocks, 1);
});

test('reconciliation recovers the saved identity of previously imported threads', async t => {
  const h = await fixture(t);
  await h.create({ depends_on: ['parent'], start_when_ready: false });
  const db = new DatabaseSync(join(h.dir, 'graph.sqlite'));
  db.prepare('UPDATE nodes SET project_identity=NULL WHERE id=?').run('parent');
  db.close();
  await h.refresh();
  assert.equal((await h.view()).nodes.find(n => n.id === 'parent')?.unblocks, 1);
});

test('a directory with the same identity at a different path cannot see old drafts', async t => {
  const h = await fixture(t);
  const old = await h.create();
  const moved = h.root + '-moved';
  await rename(h.root, moved);
  const current = await h.create({ checkout: moved });
  const page = await h.graph.handle({ op: 'graph', project: moved }) as Awaited<ReturnType<typeof h.view>>;
  assert.deepEqual(page.nodes.map(n => n.id), [current.node_id]);
  assert.ok(!page.nodes.some(n => n.id === old.node_id));
});

test('a replacement checkout blocks admission', async t => {
  const h = await fixture(t);
  const { node_id } = await h.create();
  await rename(h.root, h.root + '-original');
  await mkdir(h.root);
  await h.graph.handle({ op: 'start_draft', node_id });
  await h.refresh();
  await waitForCard(h, node_id, false);
  await rename(h.root, h.root + '-replacement');
  await rename(h.root + '-original', h.root);
  await waitForCard(h, node_id, true);
  const card = (await h.view()).nodes.find(n => n.id === node_id)!;
  assert.equal(card.status, 'blocked');
  assert.equal(card.reason, 'The project at this path changed');
  assert.ok(!h.calls.some(r => r.op === 'create_admitted_thread'));
});

test('create and fork require operation IDs and refuse oversized text', async t => {
  const h = await fixture(t);
  const fork = { op: 'fork_thread', parent_id: 'parent', seed_mode: 'brief', brief: 'go' };
  for (const request of [{}, fork]) {
    await assert.rejects(h.create({ ...request, op_id: undefined }), { code: 'invalid_request' });
    const replay = { ...request, op_id: randomUUID() };
    assert.deepEqual(await h.create(replay), await h.create(replay));
  }
  const error = { code: 'invalid_request', message: 'Brief is too long' };
  await assert.rejects(h.create({ ...fork, brief: 'a'.repeat(30028) }), error);
  await assert.rejects(h.create({ ...fork, brief: 'a'.repeat(15013), prompt: 'b'.repeat(15013) }), error);
  await assert.rejects(h.create({ prompt: 'a'.repeat(30028) }), error);
});

test('replacement project cannot fork an old excerpt or import its dependency answer', async t => {
  const h = await fixture(t);
  await rename(h.root, h.root + '-original');
  await mkdir(h.root);
  await assert.rejects(h.create({ op: 'fork_thread', parent_id: 'parent', seed_mode: 'excerpt' }), /Source is from another project/);
  const { node_id } = await h.create({ depends_on: ['parent'] });
  await h.refresh();
  assert.equal((await h.view()).nodes.find(n => n.id === node_id)?.reason, 'Source is from another project');
  assert.ok(!h.calls.some(r => r.op === 'history' || r.op === 'submit_checkout'));
});

test('a worktree receipt for an enclosing checkout blocks submission', async t => {
  const h = await fixture(t);
  const { node_id } = await h.create();
  h.control.parentRoot = h.dir;
  await h.graph.handle({ op: 'start_draft', node_id });
  await h.refresh();
  assert.equal((await h.view()).nodes.find(n => n.id === node_id)?.reason, 'Admission belongs to another checkout');
  assert.ok(!h.calls.some(r => r.op === 'submit_checkout'));
});

test('a replacement worktree parent at the captured path blocks submission', async t => {
  const h = await fixture(t);
  h.control.history = 'Private excerpt';
  const { node_id } = await h.create({ op: 'fork_thread', parent_id: 'parent', seed_mode: 'excerpt' });
  h.control.parentRoot = h.root;
  await h.graph.handle({ op: 'start_draft', node_id });
  await h.refresh();
  assert.equal((await h.view()).nodes.find(n => n.id === node_id)?.reason, 'Admission belongs to another checkout');
  assert.ok(!h.calls.some(r => r.op === 'submit_checkout'));
});

test('waiting drafts yield between batches of 50 dependency reads', async t => {
  const h = await fixture(t);
  const db = new DatabaseSync(join(h.dir, 'graph.sqlite'));
  db.exec('BEGIN');
  const insert = db.prepare('INSERT INTO nodes(id,project,title,auto) VALUES(?,?,?,1)');
  insert.run('unfinished', h.root, 'Dependency');
  const edge = db.prepare("INSERT INTO edges VALUES('unfinished',?,'depends_on')");
  for (let i = 0; i < 101; i++) {
    insert.run(String(i), h.root, 'Waiting');
    edge.run(String(i));
  }
  db.exec('COMMIT'); db.close();
  const batches = t.mock.method(Storage.prototype, 'batchDependencies');
  const scan = h.refresh();
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(batches.mock.callCount(), 1);
  await scan;
  assert.deepEqual(batches.mock.calls.map(call => call.arguments[0].length), [50, 50, 1]);
  assert.equal(h.calls.length, 0);
});

test('5000 linked threads yield to a concurrent graph request after one batch', async t => {
  const h = await fixture(t);
  const db = new DatabaseSync(join(h.dir, 'graph.sqlite'));
  db.exec('BEGIN');
  const identity = JSON.stringify(await rootIdentity(h.root));
  const add = db.prepare('INSERT INTO nodes(id,project,thread,title,started,project_identity) VALUES(?,?,?,?,1,?)');
  const edge = db.prepare("INSERT INTO edges VALUES(?,?,'fork')");
  for (let i = 0; i <= 5000; i++) {
    const id = 'thread-' + i;
    const project = i === 5000 ? '/other' : h.root;
    h.threads.set(id, { ...thread(id), checkout: project });
    add.run(id, project, id, id, identity);
    if (i) edge.run('thread-' + (i - 1), id);
  }
  db.exec('COMMIT'); db.close();
  const scan = h.refresh();
  await new Promise<void>(resolve => setImmediate(resolve));
  const watched = () => h.calls.filter(r => r.op === 'stored_thread');
  assert.equal(watched().length, 50);
  await h.view();
  await scan;
  assert.equal(watched().length, 5000);
  assert.ok(!watched().some(r => r.thread_id === 'thread-5000'));
});

test('admission waiting for preparation leaves the graph request pipe available', async t => {
  const h = await fixture(t);
  let enter!: () => void, release!: () => void;
  const entered = new Promise<void>(resolve => { enter = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  h.control.admission = async () => { enter(); await gate; };
  const { node_id } = await h.create();
  await h.graph.handle({ op: 'start_draft', node_id });
  const scan = h.refresh();
  await entered;
  try {
    assert.equal((await h.view()).nodes.find(n => n.id === node_id)?.status, 'ready');
    assert.ok(!h.calls.some(r => r.op === 'submit_checkout'));
  } finally { release(); await scan; }
});

test('graph admissions from different projects share one local probe slot', async t => {
  const dir = await realpath(await fixtureDirectory(join(tmpdir(), 'graph-probes-')));
  const host = new Runtime(dir, createModels(), new Set());
  const old = process.env.PLAIT_GRAPH;
  process.env.PLAIT_GRAPH = '1';
  let polls = 0;
  try {
    host.startBackground(async () => { polls++; return {}; });
  } finally {
    if (old === undefined) delete process.env.PLAIT_GRAPH; else process.env.PLAIT_GRAPH = old;
  }
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const probes: string[] = [];
  host.admissions.info = async path => { probes.push(path); await gate; throw Error('end probe'); };
  const projects = ['/one', '/two', '/three'];
  const tasks = projects.map((checkout, i) => host['graph']!['send']({ op: 'create_admitted_thread',
    request_id: 'project-' + i, checkout, placement: 'project', harness: 'pi', provider: 'example',
    model_id: 'model', approval: 'ask' }));
  const finished = Promise.allSettled(tasks);
  try {
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(polls, projects.length);
    assert.deepEqual(probes, projects.slice(0, 1));
  } finally {
    release();
    await finished;
    await host.close();
    await rm(dir, { recursive: true, force: true });
  }
  assert.deepEqual(probes, projects);
});
