import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Store } from '../src/store.ts';

test('graph storage survives a restart and enforces edge uniqueness', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  let store = new Store(directory);
  store.db.prepare("INSERT INTO graph_nodes(id,project,checkout,lifecycle,created_at,updated_at) VALUES (?,?,?,'draft',?,?)").run('a', '/p', '/p', 1, 1);
  store.db.prepare('INSERT INTO graph_drafts(node_id,title,prompt,placement,harness,provider,model_id,seed_text) VALUES (?,?,?,?,?,?,?,?)').run('a', 'Title', 'Prompt', 'project', 'pi', 'provider', 'model', 'seed');
  const edge = store.db.prepare("INSERT INTO graph_edges(id,kind,from_node,to_node,created_at) VALUES (?,'depends_on','a','b',1)");
  edge.run('e');
  assert.throws(() => edge.run('duplicate'));
  store.close();
  store = new Store(directory);
  assert.equal(store.db.prepare('SELECT seed_text FROM graph_drafts WHERE node_id=?').get('a')?.seed_text, 'seed');
  assert.equal(store.db.prepare('SELECT kind FROM graph_nodes WHERE id=?').get('a')?.kind, 'build');
  assert.equal(store.db.prepare('SELECT count(*) AS n FROM graph_edges').get()?.n, 1);
  store.close();
});

import { Graph, deriveStatus } from '../src/graph.ts';

test('graph mutations, cycles, idempotency and projected threads', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const store = new Store(directory); t.after(() => store.close());
  let changes = 0; const graph = new Graph(store, () => changes++);
  const draft = { op: 'create_draft', checkout: '/p', title: 'A', prompt: 'Work', harness: 'pi', provider: 'p', model_id: 'm' };
  const first = await graph.handle({ ...draft, op_id: 'one' }) as any;
  assert.deepEqual(await graph.handle({ ...draft, op_id: 'one' }), first);
  assert.equal(changes, 1);
  const second = await graph.handle({ ...draft, title: 'B' }) as any;
  const a = first.node.id, b = second.node.id;
  await graph.handle({ op: 'set_dependencies', node_id: b, depends_on: [a], auto_start: true });
  assert.deepEqual(graph.read().nodes.find(n => n.id === b)?.waiting, { done: 0, total: 1 });
  await assert.rejects(graph.handle({ op: 'set_dependencies', node_id: a, depends_on: [b] }), /cycle/);
  await graph.handle({ op: 'set_dependencies', node_id: b, depends_on: [] });
  assert.equal(graph.read().edges.length, 0);
  const snapshot = { id: 'thread', checkout: '/p', model: { provider: 'p', model_id: 'm' }, status: 'running' as const, messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false };
  store.save(snapshot);
  assert.equal(graph.read().nodes.find(n => n.id === 'thread')?.kind, 'build');
  await assert.rejects(graph.handle({ op: 'set_node_kind', node_id: 'thread', kind: 'discuss', pinned_root: true }), /running/);
  await graph.handle({ op: 'delete_draft', node_id: a });
  assert.equal(graph.read().nodes.some(n => n.id === a), false);
  assert.equal(graph.read().graph_revision, 5);
});

test('graph status priority and details', () => {
  const base = { lifecycle: 'thread', kind: 'build' };
  const cases: [Parameters<typeof deriveStatus>[0], string][] = [
    [{ ...base, pending: true, threadStatus: 'error' }, 'needs_you'],
    [{ ...base, queueState: 'held' }, 'needs_you'],
    [{ ...base, threadStatus: 'interrupted' }, 'failed'],
    [{ ...base, kind: 'discuss', threadStatus: 'running' }, 'discussing'],
    [{ ...base, threadStatus: 'running' }, 'building'],
    [{ ...base, queueState: 'waiting', queuePosition: 2 }, 'queued'],
    [{ ...base, lifecycle: 'draft', autoStart: 'armed', done: 1, total: 2 }, 'waiting'],
    [{ ...base, lifecycle: 'draft' }, 'draft'],
    [{ ...base, threadStatus: 'idle', completed: true }, 'done'],
  ];
  for (const [input, expected] of cases) assert.equal(deriveStatus(input).status, expected);
  assert.equal(deriveStatus({ ...base, queueState: 'waiting', queuePosition: 2 }).queue_position, 2);
  assert.deepEqual(deriveStatus({ ...base, lifecycle: 'draft', autoStart: 'armed', done: 1, total: 2 }).waiting, { done: 1, total: 2 });
});

test('fork seeds use paged history, clip, validate and replay', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const store = new Store(directory); t.after(() => store.close());
  store.save({ id: 'parent', checkout: '/p', model: { provider: 'p', model_id: 'm' }, status: 'idle', messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false });
  const source = {
    entries: async () => [{ id: 1, kind: 'test', model: [{ role: 'user', content: 'Hello' }, { role: 'assistant', content: 'World' }] }] as any,
    pending: () => [], placed: () => new Map(), typed: () => undefined, journal: () => undefined, active: false,
  };
  const graph = new Graph(store, () => {}, async (_id, work) => work(source));
  const request = { op: 'fork_thread', parent_id: 'parent', title: 'Child', prompt: 'Work', harness: 'pi', provider: 'p', model_id: 'm', brief: 'Brief' };
  for (const [mode, expected] of [['excerpt', 'user: Hello\n\nassistant: World'], ['brief', 'Brief'], ['both', 'user: Hello\n\nassistant: World\n\nBrief']]) {
    const result = await graph.handle({ ...request, seed_mode: mode }) as any;
    assert.equal(store.db.prepare('SELECT seed_text FROM graph_drafts WHERE node_id=?').get(result.node.id)?.seed_text, expected);
  }
  const first = await graph.handle({ ...request, at_message_id: '1-0', op_id: 'fork' }) as any;
  assert.deepEqual(await graph.handle({ ...request, at_message_id: '1-0', op_id: 'fork' }), first);
  assert.equal(store.db.prepare('SELECT seed_text FROM graph_drafts WHERE node_id=?').get(first.node.id)?.seed_text, 'user: Hello\n\nBrief');
  assert.equal(store.db.prepare("SELECT source_message_id FROM graph_edges WHERE to_node=? AND kind='fork'").get(first.node.id)?.source_message_id, '1-0');
  await assert.rejects(graph.handle({ ...request, at_message_id: 'missing' }), /Unknown message/);
  const clipped = await graph.handle({ ...request, seed_mode: 'brief', brief: 'é'.repeat(15000) }) as any;
  const seed = String(store.db.prepare('SELECT seed_text FROM graph_drafts WHERE node_id=?').get(clipped.node.id)?.seed_text);
  assert.ok(seed.startsWith('[Earlier text clipped]\n'));
  assert.ok(Buffer.byteLength(seed) <= 24 * 1024);
});

test('graph reads hundreds of nodes and thousands of edges promptly', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const store = new Store(directory); t.after(() => store.close());
  store.db.exec('BEGIN');
  const node = store.db.prepare("INSERT INTO graph_nodes(id,project,checkout,lifecycle,created_at,updated_at) VALUES (?,'/p','/p','draft',0,0)");
  const edge = store.db.prepare("INSERT INTO graph_edges(id,kind,from_node,to_node,created_at) VALUES (?,'depends_on',?,?,0)");
  for (let i = 0; i < 500; i++) node.run(`n${i}`);
  for (let i = 0; i < 2000; i++) edge.run(`e${i}`, `n${i % 500}`, `n${(i + 1 + Math.floor(i / 500)) % 500}`);
  store.db.exec('COMMIT');
  const graph = new Graph(store);
  const start = performance.now();
  const result = graph.read();
  assert.equal(result.nodes.length, 500);
  assert.equal(result.edges.length, 2000);
  assert.ok(performance.now() - start < 50);
});

test('backfill preserves thread timestamps and ordering', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  let store = new Store(directory);
  const snapshot = { id: 'old', checkout: '/p', model: { provider: 'p', model_id: 'm' }, status: 'idle' as const, messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false };
  store.save(snapshot);
  store.save({ ...snapshot, id: 'new' });
  const { updated_at: _updatedAt, ...legacy } = snapshot as typeof snapshot & { updated_at?: number };
  store.db.prepare('UPDATE threads SET snapshot=?,updated_at=1,has_assistant=NULL WHERE id=?').run(JSON.stringify(legacy), 'old');
  store.db.prepare('UPDATE threads SET updated_at=2 WHERE id=?').run('new');
  store.close();
  store = new Store(directory); t.after(() => store.close());
  assert.deepEqual(store.db.prepare('SELECT id,updated_at FROM threads ORDER BY updated_at DESC').all().map(row => [row.id, row.updated_at]), [['new', 2], ['old', 1]]);
});

test('graph changes notify workspace subscribers with a revision', async t => {
  const { Workspace } = await import('../src/workspace.ts');
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const store = new Store(directory); t.after(() => store.close());
  const workspace = new Workspace(store, () => {});
  const events: ReturnType<typeof workspace.view>[] = [];
  const graph = new Graph(store, () => events.push(workspace.view()));
  assert.deepEqual(await workspace.handle({ op: 'workspace_subscribe' }), workspace.view());
  assert.equal(workspace.view().graph_revision, 0);
  await graph.handle({ op: 'create_draft', checkout: '/p', title: 'A', prompt: 'Work', harness: 'pi', provider: 'p', model_id: 'm' });
  assert.equal(events.length, 1);
  assert.equal(events[0].graph_revision, 1);
  assert.equal('nodes' in events[0], false);
  assert.equal(graph.read().graph_revision, events[0].graph_revision);
});

test('graph bounds, archived threads, worktrees and queue state', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const store = new Store(directory); t.after(() => store.close());
  const graph = new Graph(store);
  store.db.prepare('INSERT INTO workspace_worktrees(folder,main,branch) VALUES (?,?,?)').run('/wt', '/p', 'branch');
  const snapshot = { id: 'worktree', checkout: '/wt', model: { provider: 'p', model_id: 'm' }, status: 'idle' as const, messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false };
  store.save(snapshot); store.save({ ...snapshot, id: 'archived' });
  store.db.prepare('UPDATE threads SET archived=1 WHERE id=?').run('archived');
  assert.deepEqual(graph.read('/p').nodes.map(n => n.id), ['worktree']);
  const insert = store.db.prepare("INSERT INTO graph_nodes(id,project,checkout,lifecycle,created_at,updated_at) VALUES (?,'/p','/p','draft',0,0)");
  for (let i = 0; i < 501; i++) insert.run(`bound${i}`);
  const view = graph.read('/p');
  assert.equal(view.nodes.length, 500); assert.equal(view.truncated, true);
  assert.ok(Buffer.byteLength(JSON.stringify(view)) < 800000);
  assert.equal(graph.handles({ op: ['graph'] }), false);
  await assert.rejects(graph.handle({ op: ['graph'] }), /Unknown graph/);
  assert.equal(deriveStatus({ lifecycle: 'thread', kind: 'build', threadStatus: 'idle', completed: true, autoStart: 'armed', total: 1 }).status, 'done');
  assert.equal(deriveStatus({ lifecycle: 'draft', kind: 'build', autoStart: 'armed', total: 0 }).status, 'draft');
});

test('fork reads only the needed history and skips it for a brief', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const store = new Store(directory); t.after(() => store.close());
  store.save({ id: 'parent', checkout: '/p', model: { provider: 'p', model_id: 'm' }, status: 'idle', messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false });
  let reads = 0;
  const source = { entries: async ({ maxEntryId }: { maxEntryId?: number }) => { reads++; const max = Math.min(maxEntryId ?? 1000, 1000); return Array.from({ length: Math.min(max, 64) }, (_, i) => ({ id: max - i, kind: 'test', model: [{ role: 'user', content: 'x'.repeat(2000) }] })) as any; }, pending: () => [], placed: () => new Map(), typed: () => undefined, journal: () => undefined, active: false };
  const graph = new Graph(store, () => {}, async (_id, work) => work(source));
  const request = { op: 'fork_thread', parent_id: 'parent', title: 'Child', prompt: 'Work', harness: 'pi', provider: 'p', model_id: 'm' };
  await graph.handle({ ...request, seed_mode: 'brief', brief: 'Brief' });
  assert.equal(reads, 0);
  const result = await graph.handle({ ...request, seed_mode: 'excerpt' }) as any;
  assert.ok(reads <= 2);
  assert.ok(Buffer.byteLength(String(store.db.prepare('SELECT seed_text FROM graph_drafts WHERE node_id=?').get(result.node.id)?.seed_text)) <= 24 * 1024);
});

test('held follow-ups preserve queue state and the first checkout position', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'plait-graph-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const store = new Store(directory); t.after(() => store.close());
  const graph = new Graph(store);
  store.save({ id: 'thread', checkout: '/p', model: { provider: 'p', model_id: 'm' }, status: 'idle', messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false });
  const insert = store.db.prepare("INSERT INTO checkout_waiters VALUES (?,?,'/p','{}',?,?,?,'{}','trusted','ask',0,NULL)");
  insert.run('thread', 'a', 'checkout', 'waiting', 1);
  insert.run('thread', 'b', 'follow_up', 'held', 2);
  insert.run('thread', 'c', 'checkout', 'waiting', 3);
  assert.equal(graph.read().nodes.find(n => n.id === 'thread')?.status, 'needs_you');
  store.db.prepare("UPDATE checkout_waiters SET state='waiting' WHERE request='b'").run();
  assert.equal(graph.read().nodes.find(n => n.id === 'thread')?.queue_position, 1);
});
