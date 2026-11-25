import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import { Store } from '../src/store.ts';
import { CheckoutWatcher } from '../src/watcher.ts';
import { Workspace } from '../src/workspace.ts';

test('workspace watches changes, filters Git objects, and stops hidden projects and forgotten worktrees', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'plait-watch-')));
  const repo = join(root, 'repo');
  execFileSync('git', ['init', '-q', repo]);
  await mkdir(join(root, 'store'));
  const store = new Store(join(root, 'store'));
  let notifications = 0;
  const watcher = new CheckoutWatcher(() => { notifications++; });
  const workspace = new Workspace(store, () => {});
  workspace.setWatcher(watcher);
  t.after(async () => { watcher.close(); store.close(); await rm(root, { recursive: true, force: true }); });
  await workspace.handle({ op: 'workspace_open_project', path: repo });
  assert.equal(workspace.view().status[repo], 0);
  // macOS starts its file event stream shortly after the watch; earlier writes can be missed.
  await delay(600);
  await writeFile(join(repo, 'file'), 'change');
  const deadline = Date.now() + 5000;
  while (!notifications && Date.now() < deadline) await delay(20);
  assert.ok(notifications > 0);
  assert.ok(workspace.view().status[repo] > 0);
  await delay(350);
  const before = workspace.view().status[repo];
  const count = notifications;
  await writeFile(join(repo, '.git', 'objects', 'ignored'), 'object');
  await delay(500);
  assert.equal(workspace.view().status[repo], before);
  assert.equal(notifications, count);
  await workspace.handle({ op: 'workspace_hide_project', path: repo, hidden: true });
  assert.equal(workspace.view().status[repo], undefined);
  await workspace.handle({ op: 'workspace_set_worktree', folder: repo, main: repo, branch: 'test' });
  assert.equal(workspace.view().status[repo], 0);
  await workspace.handle({ op: 'workspace_forget_worktree', folder: repo });
  assert.equal(workspace.view().status[repo], undefined);
});

test('at most 64 folders are watched, dropping the oldest', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-watch-limit-'));
  const watcher = new CheckoutWatcher(() => {});
  t.after(async () => { watcher.close(); await rm(root, { recursive: true, force: true }); });
  const folders = Array.from({ length: 65 }, (_, i) => join(root, String(i)));
  await Promise.all(folders.map(folder => mkdir(folder)));
  watcher.sync(folders);
  assert.equal(Object.keys(watcher.status()).length, 64);
  assert.equal(watcher.status()[folders[0]], undefined);
  assert.equal(watcher.status()[folders[64]], 0);
  watcher.sync([join(root, 'missing')]);
  assert.deepEqual(watcher.status(), {});
});

test('a busy folder is announced at most once a second, and its last change always is', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-watch-rate-'));
  let notifications = 0;
  const watcher = new CheckoutWatcher(() => { notifications++; });
  t.after(async () => { watcher.close(); await rm(root, { recursive: true, force: true }); });
  watcher.sync([root]);
  await delay(600);
  const started = Date.now();
  for (let i = 0; i < 8; i++) { await writeFile(join(root, `out-${i}`), 'x'); await delay(300); }
  const elapsed = Date.now() - started;
  await delay(2500);
  assert.ok(notifications >= 2, `expected the last change to be announced, saw ${notifications}`);
  assert.ok(notifications <= Math.ceil(elapsed / 1000) + 1, `too many announcements: ${notifications} in ${elapsed} ms`);
  assert.equal(watcher.status()[root], notifications);
});

test('working files and Git ref changes are classified separately', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-watch-refs-'));
  await mkdir(join(root, '.git'));
  const events: boolean[] = [];
  const watcher = new CheckoutWatcher((_folder, refs) => { events.push(refs === true); });
  t.after(async () => { watcher.close(); await rm(root, { recursive: true, force: true }); });
  watcher.sync([root]);
  // Wait on events, not a clock: macOS may start its stream late, deliver a write twice, or late.
  const until = async (done: () => boolean, what: string) => {
    const deadline = Date.now() + 10_000;
    while (!done()) { assert.ok(Date.now() < deadline, `timed out waiting for ${what}: ${JSON.stringify(events)}`); await delay(20); }
  };
  // A write before the stream starts can be missed, so repeat it until one is seen.
  for (let attempt = 0; !events.length; attempt++) {
    assert.ok(attempt < 10, 'no working-file event after ten writes');
    await writeFile(join(root, 'file'), String(attempt));
    const wait = Date.now() + 2000;
    while (!events.length && Date.now() < wait) await delay(20);
  }
  assert.ok(events.every(refs => !refs), `working files alone must not mark refs: ${JSON.stringify(events)}`);
  for (const name of ['packed-refs', 'config']) {
    const seen = events.length;
    await writeFile(join(root, '.git', name), 'x');
    await until(() => events.slice(seen).includes(true), `a refs change after .git/${name}`);
  }
});
