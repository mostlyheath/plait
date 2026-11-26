import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { mkdtemp, rm, readdir, mkdir, realpath, rename } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { Runtime } from '../src/runtime.ts';
import './support/safety.ts';
import { Storage } from '../src/graph/storage.ts';
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
async function fixture(t: TestContext) {
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'graph-client-')));
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
      case 'watch': return { thread: threads.get(String(r.thread_id)) ?? assert.fail('unknown thread') };
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
  } : undefined);
  t.after(async () => { await graph.close(); await rm(dir, { recursive: true, force: true }); });
  const create = (r: Request = {}) => graph.handle(r.op === 'fork_thread' ? { op_id: randomUUID(), title: draft.title, harness: draft.harness, provider: draft.provider, model_id: draft.model_id, ...r } : { ...draft, checkout: root, op_id: randomUUID(), ...r }) as Promise<{ node_id: string }>;
  const view = () => graph.handle({ op: 'graph', project: root }) as Promise<{ graph_revision: number; nodes: { id: string; status: string; reason: string }[] }>;
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
  const h = await fixture(t);
  const p = h.threads.get('parent')!;
  p.archived = true; p.status = 'interrupted';
  const { node_id } = await h.create({ depends_on: ['parent'] });
  await h.refresh();
  assert.equal((await h.view()).nodes.find(n => n.id === node_id)?.status, 'blocked');
  assert.equal(h.calls.some(r => r.op === 'submit_checkout'), false);
  p.status = 'idle';
  await h.refresh(); await h.refresh();
  assert.equal(h.calls.find(r => r.op === 'create_admitted_thread')?.request_id, 'create-' + node_id);
  const submit = h.calls.filter(r => r.op === 'submit_checkout');
  assert.equal(submit.length, 1); assert.equal(submit[0].request_id, 'submit-' + node_id);
  assert.match(String(submit[0].text), /plait:\/\/thread\/parent/);
  const call = h.calls.length;
  const page = await h.view();
  assert.deepEqual(h.calls.slice(call).map(r => r.op), ['list_threads']);
  assert.deepEqual(await h.graph.handle({ op: 'graph', project: h.root, revision: page.graph_revision }), { unchanged: true, graph_revision: page.graph_revision });
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
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'ordinary-host-')));
  const host = new Runtime(dir, createModels(), new Set());
  t.after(async () => {
    await host.close(); await rm(dir, { recursive: true, force: true });
    if (previous !== undefined) process.env.PLAIT_GRAPH = previous;
  });
  assert.deepEqual(await host.handle({ op: 'list_threads', protocol_version: 6 }), { threads: [], next_cursor: null });
  await assert.rejects(host.handle({ op: 'graph', project: '/p' }), { code: 'invalid_request' });
  assert.ok(!(await readdir(dir)).includes('graph.sqlite'));
});

test('feature-on runtime admits a draft and submits through the host handler', async t => {
  const previous = process.env.PLAIT_GRAPH;
  process.env.PLAIT_GRAPH = '1';
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'graph-integration-')));
  const { mkdir } = await import('node:fs/promises');
  const root = join(dir, 'checkout'), store = join(dir, 'store');
  await mkdir(root); await mkdir(store);
  const { fauxProvider, fauxAssistantMessage } = await import('@earendil-works/pi-ai/providers/faux');
  const models = createModels(), faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('finished')]); models.setProvider(faux.provider);
  const host = new Runtime(store, models, new Set(['faux']));
  t.after(async () => {
    await host.close(); await rm(dir, { recursive: true, force: true });
    if (previous === undefined) delete process.env.PLAIT_GRAPH;
    else process.env.PLAIT_GRAPH = previous;
  });
  const handle = host.handle.bind(host);
  let rename: ReturnType<typeof host.reply> | undefined;
  let submitting = false;
  t.mock.method(host, 'handle', async (input: Request) => {
    if (input.op === 'rename_thread') assert.equal(submitting, false);
    if (input.op !== 'submit_checkout') return handle(input);
    submitting = true;
    rename = host.reply({ op: 'rename_thread', protocol_version: 6, thread_id: input.thread_id, title: 'Desktop rename' });
    await new Promise<void>(resolve => setImmediate(resolve));
    try { return await handle(input); }
    finally { submitting = false; }
  });
  const created = await host.reply({ ...draft, op_id: 'integration', checkout: await realpath(root), provider: 'faux', model_id: 'faux-1' });
  assert.ok(created.ok);
  const { node_id } = created.result as { node_id: string };
  assert.ok((await host.reply({ op: 'start_draft', node_id })).ok);
  const deadline = Date.now() + 15000;
  for (; ;) {
    const { threads } = await host.handle({ op: 'list_threads', protocol_version: 6 }) as { threads: { id: string }[] };
    if (threads.length) {
      const { thread } = await host.handle({ op: 'watch', protocol_version: 6, thread_id: threads[0].id }) as { thread: { status: string; messages: { text: string }[]; error: unknown } };
      assert.equal(thread.error, null);
      if (thread.status === 'idle' && thread.messages.at(-1)?.text === 'finished') {
        assert.ok((await rename)?.ok);
        assert.equal((thread as typeof thread & { title: string }).title, 'Desktop rename');
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
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'graph-reopen-')));
  const submissions: Request[] = [];
  const identity = await rootIdentity(dir);
  const receipt = () => submissions.length ? { state: "started", error: null } : undefined;
  const send = async (r: Request) => {
    switch (r.op) {
      case 'watch': return { thread: { ...thread('destination'), checkout: await realpath(dir), messages: [] } };
      case 'create_admitted_thread': return { admission: { state: 'ready', checkout: dir, thread_id: 'destination' } };
      case 'submit_checkout':
        submissions.push(r);
        return { accepted: true };
      default: assert.fail(String(r.op));
    }
  };
  let graph = new Graph(dir, send, async () => identity, receipt);
  t.after(async () => { await graph.close(); await rm(dir, { recursive: true, force: true }); });
  const create = { ...draft, checkout: await realpath(dir), op_id: 'start-once', approval: 'auto' };
  const { node_id } = await graph.handle(create) as { node_id: string };
  await graph.handle({ op: 'start_draft', node_id });
  await graph.reconcile(dir); await graph.close();
  graph = new Graph(dir, send, async () => identity, receipt);
  assert.equal((await graph.handle(create) as { node_id: string }).node_id, node_id);
  await graph.reconcile(dir); await graph.reconcile(dir);
  assert.equal(submissions.length, 1);
});

test('a replacement checkout blocks admission', async t => {
  const h = await fixture(t);
  const { node_id } = await h.create();
  await rename(h.root, h.root + '-original');
  await mkdir(h.root);
  await h.graph.handle({ op: 'start_draft', node_id });
  await h.refresh();
  assert.ok(!(await h.view()).nodes.some(n => n.id === node_id));
  await rename(h.root, h.root + '-replacement');
  await rename(h.root + '-original', h.root);
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
  const add = db.prepare('INSERT INTO nodes(id,project,thread,title,started) VALUES(?,?,?,?,1)');
  const edge = db.prepare("INSERT INTO edges VALUES(?,?,'fork')");
  for (let i = 0; i <= 5000; i++) {
    const id = 'thread-' + i;
    const project = i === 5000 ? '/other' : h.root;
    h.threads.set(id, { ...thread(id), checkout: project });
    add.run(id, project, id, id);
    if (i) edge.run('thread-' + (i - 1), id);
  }
  db.exec('COMMIT'); db.close();
  const scan = h.refresh();
  await new Promise<void>(resolve => setImmediate(resolve));
  const watched = () => h.calls.filter(r => r.op === 'watch');
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
