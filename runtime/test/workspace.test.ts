import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { HostError } from '../src/protocol.ts';
import { Store } from '../src/store.ts';
import { Workspace, validateWorkspace } from '../src/workspace.ts';

const code = (expected: string) => (error: unknown) => error instanceof HostError && error.code === expected;

async function setup(t: { after: (fn: () => unknown) => void }) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'plait-workspace-')));
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

test('only a Git checkout opens, at its top folder, and each change moves the revision', async t => {
  const { workspace, repo, root, notified } = await setup(t);
  const opened: any = await workspace.handle({ op: 'workspace_open_project', path: join(repo, '.') });
  assert.equal(opened.path, repo);
  assert.equal(opened.revision, 1);
  assert.equal(notified(), 1);
  await assert.rejects(workspace.handle({ op: 'workspace_open_project', path: join(root, 'missing') }), code('invalid_request'));
  await assert.rejects(workspace.handle({ op: 'workspace_open_project', path: 'relative' }), code('invalid_request'));
  await workspace.handle({ op: 'workspace_hide_project', path: repo, hidden: true });
  let view: any = await workspace.handle({ op: 'workspace_view' });
  assert.deepEqual(view.projects, [{ path: repo, hidden: true }]);
  assert.equal(view.revision, 2);
  // Opening a hidden project shows it again.
  await workspace.handle({ op: 'workspace_open_project', path: repo });
  view = await workspace.handle({ op: 'workspace_view' });
  assert.deepEqual(view.projects, [{ path: repo, hidden: false }]);
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
  assert.deepEqual(view.projects, [{ path: '/a', hidden: false }, { path: '/b', hidden: false }, { path: '/c', hidden: true }]);
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
