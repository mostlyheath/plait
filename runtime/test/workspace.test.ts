import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdir, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { HostError } from '../src/protocol.ts';
import { Store } from '../src/store.ts';
import { Workspace, validateWorkspace } from '../src/workspace.ts';

const code = (expected: string) => (error: unknown) => error instanceof HostError && error.code === expected;

async function setup(t: { after: (fn: () => unknown) => void }) {
  const root = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-workspace-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'store'));
  const store = new Store(join(root, 'store'));
  let notified = 0;
  const workspace = new Workspace(store, () => { notified++; });
  const repo = join(root, 'repo');
  execFileSync('git', ['init', '-q', repo]);
  return { root, store, workspace, repo, notified: () => notified };
}

test('the workspace names its host and starts empty', async t => {
  const { workspace } = await setup(t);
  const view: any = await workspace.handle({ op: 'workspace_view' });
  assert.equal(view.schema, 1);
  assert.match(view.host.id, /^[0-9a-f-]{36}$/);
  assert.equal(view.host.platform, process.platform);
  assert.deepEqual([view.revision, view.projects, view.worktrees, view.seen], [0, [], [], []]);
  const again: any = await workspace.handle({ op: 'workspace_view' });
  assert.equal(again.host.id, view.host.id, 'the identity is stable');
  const named: any = await workspace.handle({ op: 'host_set_name', name: 'workstation' });
  assert.equal(named.host.name, 'workstation');
});

test('a checkout opens at its top folder, and each change moves the revision', async t => {
  const { workspace, repo, root, notified } = await setup(t);
  const opened: any = await workspace.handle({ op: 'workspace_open_project', path: join(repo, '.') });
  assert.equal(opened.path, repo);
  assert.equal(opened.revision, 1);
  assert.equal(notified(), 1);
  await assert.rejects(workspace.handle({ op: 'workspace_open_project', path: join(root, 'missing') }), code('checkout_missing'));
  await assert.rejects(workspace.handle({ op: 'workspace_open_project', path: 'relative' }), code('invalid_request'));
  await workspace.handle({ op: 'workspace_hide_project', path: repo, hidden: true });
  let view: any = await workspace.handle({ op: 'workspace_view' });
  assert.deepEqual(view.projects, [{ path: repo, hidden: true, vcs: 'git' }]);
  assert.equal(view.revision, 2);
  // Opening a hidden project shows it again.
  await workspace.handle({ op: 'workspace_open_project', path: repo });
  view = await workspace.handle({ op: 'workspace_view' });
  assert.deepEqual(view.projects, [{ path: repo, hidden: false, vcs: 'git' }]);
});

test('worktree ownership and seen state round-trip; an older observation never wins', async t => {
  const { workspace } = await setup(t);
  const tree = { folder: '/w/topic', main: '/w/main', branch: 'plait/topic', creation_id: 'c1', identity: { dev: '1', ino: '18446744073709551615' } };
  await workspace.handle({ op: 'workspace_set_worktree', ...tree });
  await workspace.handle({ op: 'workspace_mark_seen', entries: [{ thread_id: 't1', updated_at: 20, state: 'idle', unread: true }] });
  await workspace.handle({ op: 'workspace_mark_seen', entries: [{ thread_id: 't1', updated_at: 10, state: 'running', unread: false }] });
  let view: any = await workspace.handle({ op: 'workspace_view' });
  assert.deepEqual(view.worktrees, [tree]);
  assert.deepEqual(view.seen, [{ thread_id: 't1', updated_at: 20, state: 'idle', unread: true }]);
  await workspace.handle({ op: 'workspace_mark_seen', entries: [{ thread_id: 't1', updated_at: 20, state: 'idle', unread: false }] });
  await workspace.handle({ op: 'workspace_forget_worktree', folder: '/w/topic' });
  view = await workspace.handle({ op: 'workspace_view' });
  assert.deepEqual([view.worktrees, view.seen[0].unread], [[], false]);
});

test('a device imports its lists once', async t => {
  const { workspace } = await setup(t);
  const lists = { device: 'mac-1', projects: ['/a', '/b'], hidden: ['/c'], worktrees: [], seen: [{ thread_id: 't', updated_at: 1, state: 'idle', unread: true }] };
  assert.deepEqual(await workspace.handle({ op: 'workspace_import', ...lists }), { imported: true, revision: 1 });
  assert.deepEqual(await workspace.handle({ op: 'workspace_import', ...lists, projects: ['/z'] }), { imported: false, revision: 1 });
  const view: any = await workspace.handle({ op: 'workspace_view' });
  assert.deepEqual(view.projects, [{ path: '/a', hidden: false, vcs: 'git' }, { path: '/b', hidden: false, vcs: 'git' }, { path: '/c', hidden: true, vcs: 'git' }]);
  assert.equal(view.seen.length, 1);
});

test('requests are checked strictly', () => {
  for (const input of [null, [], { op: 'workspace_view', extra: 1 }, { op: 'workspace_hide_project', path: '/a' },
    { op: 'workspace_open_project' }, { op: 'nope' }]) assert.throws(() => validateWorkspace(input), code('invalid_request'));
});

test('bad values are refused without changing anything', async t => {
  const { workspace } = await setup(t);
  for (const input of [
    { op: 'workspace_hide_project', path: '/a\nb', hidden: true },
    { op: 'workspace_hide_project', path: '/a', hidden: 'yes' },
    { op: 'workspace_mark_seen', entries: [{ thread_id: '../x', updated_at: 1, state: 'idle', unread: true }] },
    { op: 'workspace_set_worktree', folder: '/w', main: '/m', branch: 'b', identity: { dev: 'x', ino: '1' } },
    { op: 'workspace_import', device: 'd', projects: ['relative'], hidden: [], worktrees: [], seen: [] },
    { op: 'host_set_name', name: '' },
  ]) await assert.rejects(workspace.handle(input), code('invalid_request'));
  const view: any = await workspace.handle({ op: 'workspace_view' });
  assert.equal(view.revision, 0);
});

test('workspace write retries replay the first revision without notifying again', async t => {
  const { workspace, notified } = await setup(t);
  const request = { op: 'host_set_name', name: 'desk', op_id: 'same' };
  const first = await workspace.handle(request);
  assert.deepEqual(await workspace.handle(request), first);
  assert.equal(notified(), 1);
  await assert.rejects(workspace.handle({ ...request, name: 'other' }), code('conflict'));
});

test('opens a plain folder', async t => {
  const { workspace, root } = await setup(t);
  const folder = join(root, 'plain'); await mkdir(folder);
  await workspace.handle({ op: 'workspace_open_project', path: folder });
  assert.deepEqual(workspace.view().projects, [{ path: folder, hidden: false, vcs: 'folder' }]);
});

test('summaries refresh, persist fingerprints, and avoid unchanged revisions', async t => {
  const { store, repo, root } = await setup(t);
  let branch = 'main';
  const fingerprint = { roots: ['abc'], remote: 'example.com/repo' };
  const read = async (folder: string) => folder === repo
    ? { vcs: 'git', fingerprint, branch: { branch, named: true, tracking: null } }
    : { vcs: 'folder' };
  const workspace = new Workspace(store, () => {}, read);
  const settle = () => new Promise(resolve => setTimeout(resolve, 20));
  await workspace.handle({ op: 'workspace_open_project', path: repo });
  await settle();
  assert.deepEqual(workspace.view().projects[0].fingerprint, fingerprint);
  const revision = workspace.view().revision;
  workspace.refreshSummary(repo); await settle();
  assert.equal(workspace.view().revision, revision);
  branch = 'next'; workspace.refreshSummary(repo); await settle();
  assert.equal(workspace.view().revision, revision + 1);
  assert.equal(workspace.view().projects[0].branch?.branch, 'next');
  const restarted = new Workspace(store, () => {}, () => new Promise(() => {}));
  assert.deepEqual(restarted.view().projects[0].fingerprint, fingerprint);
  assert.equal(restarted.view().projects[0].branch, undefined);
  const folder = join(root, 'plain-summary'); await mkdir(folder);
  await workspace.handle({ op: 'workspace_open_project', path: folder }); await settle();
  const plain = workspace.view().projects.find(project => project.path === folder)!;
  assert.equal(plain.vcs, 'folder'); assert.equal(plain.fingerprint, undefined);
  await rm(repo, { recursive: true });
  const missing = new Workspace(store, () => {}, async () => { throw new Error('missing'); });
  await settle();
  assert.deepEqual(missing.view().projects.find(project => project.path === repo)?.fingerprint, fingerprint);
  assert.equal(missing.view().projects.find(project => project.path === repo)?.branch, undefined);
});

test('summary reads are bounded and repeated changes are coalesced', async t => {
  const { store } = await setup(t);
  let active = 0; let maximum = 0;
  const releases: (() => void)[] = [];
  const workspace = new Workspace(store, () => {}, async () => {
    active++; maximum = Math.max(maximum, active);
    await new Promise<void>(resolve => releases.push(resolve));
    active--; return { vcs: 'git' };
  });
  for (const folder of ['/a', '/b', '/c', '/d']) {
    store.db.prepare('INSERT INTO workspace_projects(path,hidden,added_at) VALUES (?,0,0)').run(folder);
    workspace.refreshSummary(folder);
  }
  workspace.refreshSummary('/a'); workspace.refreshSummary('/a');
  while (releases.length || active) {
    releases.splice(0).forEach(release => release());
    await new Promise(resolve => setTimeout(resolve, 0));
  }
  assert.equal(maximum, 2);
});

test('watcher summaries reuse persisted roots and refresh unborn roots', async t => {
  const { store, repo } = await setup(t);
  const fingerprint = { roots: ['abc'], remote: 'example.com/repo' };
  store.db.prepare('INSERT INTO workspace_projects(path,hidden,added_at,vcs,fingerprint) VALUES (?,0,0,?,?)').run(repo, 'git', JSON.stringify(fingerprint));
  const requests: boolean[] = [];
  const workspace = new Workspace(store, () => {}, async (_folder, roots) => {
    requests.push(roots);
    return { vcs: 'git', fingerprint: roots ? fingerprint : null, branch: { branch: 'next', named: true, tracking: null } };
  });
  const settle = () => new Promise(resolve => setTimeout(resolve, 20));
  await settle();
  workspace.refreshSummary(repo); await settle();
  assert.deepEqual(requests, [false, false]);
  assert.deepEqual(workspace.view().projects[0].fingerprint, fingerprint);
  assert.equal(workspace.view().projects[0].branch?.branch, 'next');

  const unborn = '/unborn';
  store.db.prepare('INSERT INTO workspace_projects(path,hidden,added_at,vcs,fingerprint) VALUES (?,0,0,?,?)').run(unborn, 'git', JSON.stringify({ roots: [], remote: null }));
  const unbornRequests: boolean[] = [];
  const restarted = new Workspace(store, () => {}, async (folder, roots) => {
    if (folder === unborn) unbornRequests.push(roots);
    return { vcs: 'git', fingerprint: { roots: [], remote: null } };
  });
  await settle();
  restarted.refreshSummary(unborn); await settle();
  assert.deepEqual(unbornRequests, [true, true]);
});

test('pending root refresh is not downgraded by a watcher tick', async t => {
  const { store } = await setup(t);
  const folder = '/queued';
  // A saved fingerprint means startup itself requests no root walk.
  new Workspace(store, () => {});
  store.db.prepare("INSERT INTO workspace_projects(path,hidden,added_at,fingerprint) VALUES (?,0,0,'{\"roots\":[\"old\"],\"remote\":null}')").run(folder);
  const requests: boolean[] = [];
  const releases: (() => void)[] = [];
  const workspace = new Workspace(store, () => {}, async (_folder, roots) => {
    requests.push(roots);
    await new Promise<void>(resolve => releases.push(resolve));
    return { vcs: 'git', fingerprint: roots ? { roots: ['abc'], remote: null } : null };
  });
  workspace.refreshSummary(folder, false);
  workspace.refreshSummary(folder, true);
  workspace.refreshSummary(folder, false);
  releases.shift()!();
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.deepEqual(requests, [false, true]);
  releases.shift()!();
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.deepEqual(workspace.view().projects[0].fingerprint?.roots, ['abc']);
});

test('invalid hidden fingerprints are backfilled and folder summaries clear fingerprints', async t => {
  const { store, repo } = await setup(t);
  store.db.prepare('INSERT INTO workspace_projects(path,hidden,added_at,fingerprint) VALUES (?,1,0,?)').run(repo, '{"roots":"bad","remote":null}');
  const requests: boolean[] = [];
  let folder = false;
  const workspace = new Workspace(store, () => {}, async (_path, roots) => {
    requests.push(roots);
    return folder ? { vcs: 'folder' } : { vcs: 'git', fingerprint: { roots: ['root'], remote: null } };
  });
  assert.equal(workspace.view().projects[0].fingerprint, undefined);
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(requests, [true]);
  workspace.refreshSummary(repo);
  assert.deepEqual(requests, [true]);
  await workspace.handle({ op: 'workspace_hide_project', path: repo, hidden: false });
  await new Promise(resolve => setTimeout(resolve, 20));
  folder = true;
  workspace.refreshSummary(repo, false);
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(workspace.view().projects[0].fingerprint, undefined);
  assert.equal(store.db.prepare('SELECT fingerprint FROM workspace_projects WHERE path=?').get(repo)?.fingerprint, null);
});

test('failed roots read keeps roots on a pending refresh', async t => {
  const { store, repo } = await setup(t);
  store.db.prepare('INSERT INTO workspace_projects(path,hidden,added_at) VALUES (?,0,0)').run(repo);
  const requests: boolean[] = [];
  let rejectRead!: (error: Error) => void;
  const workspace = new Workspace(store, () => {}, async (_path, roots) => {
    requests.push(roots);
    if (requests.length === 1) return await new Promise((_resolve, reject) => { rejectRead = reject; });
    return { vcs: 'git', fingerprint: { roots: ['new'], remote: null } };
  });
  workspace.refreshSummary(repo, false);
  rejectRead(new Error('transient'));
  await new Promise(resolve => setTimeout(resolve, 30));
  assert.deepEqual(requests, [true, true]);
});

test('a failed summary notification does not stop the queue or reject unhandled', async t => {
  const { store } = await setup(t);
  for (const folder of ['/one', '/two', '/three']) store.db.prepare('INSERT INTO workspace_projects(path,hidden,added_at) VALUES (?,0,0)').run(folder);
  const releases: (() => void)[] = [];
  const requests: string[] = [];
  new Workspace(store, () => { throw new Error('subscriber failed'); }, async folder => {
    requests.push(folder);
    await new Promise<void>(resolve => releases.push(resolve));
    return { vcs: 'folder' };
  });
  releases.shift()!();
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.ok(requests.includes('/three'));
  releases.splice(0).forEach(release => release());
  await new Promise(resolve => setTimeout(resolve, 20));
});
