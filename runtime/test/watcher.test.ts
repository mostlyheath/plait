import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import { test } from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import { Store } from '../src/store.ts';
import { CheckoutWatcher } from '../src/watcher.ts';
import { Workspace } from '../src/workspace.ts';

test('workspace watches changes, filters Git objects, and stops hidden projects and forgotten worktrees', async t => {
  const root = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-watch-')));
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
  const root = await fixtureDirectory(join(tmpdir(), 'plait-watch-limit-'));
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
  const root = await fixtureDirectory(join(tmpdir(), 'plait-watch-rate-'));
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
  const root = await fixtureDirectory(join(tmpdir(), 'plait-watch-refs-'));
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

function git(cwd: string, ...args: string[]): string {
  return execFileSync(
    'git',
    ['-C', cwd, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', '-c', 'commit.gpgsign=false', ...args],
    { encoding: 'utf8' },
  );
}

test('staging, unstaging, and HEAD changes in a linked worktree advance its counter', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'plait-watch-worktree-')));
  const repo = join(root, 'repo');
  const worktree = join(root, 'linked');
  await mkdir(repo);
  git(repo, 'init', '-q');
  await writeFile(join(repo, 'file'), 'initial\n');
  git(repo, 'add', 'file');
  git(repo, 'commit', '-qm', 'initial');
  git(repo, 'worktree', 'add', '-qb', 'feature', worktree);
  // Edit before starting the watcher so staging and unstaging touch only the linked worktree index.
  await writeFile(join(worktree, 'file'), 'edited\n');

  const events: Array<{ folder?: string; refs: boolean }> = [];
  const watcher = new CheckoutWatcher((folder, refs) => { events.push({ folder, refs: refs === true }); });
  t.after(async () => { watcher.close(); await rm(root, { recursive: true, force: true }); });
  watcher.sync([repo, worktree]);
  assert.equal(watcher.status()[repo], 0);
  assert.equal(watcher.status()[worktree], 0);

  const until = async (done: () => boolean, what: string) => {
    const deadline = Date.now() + 10_000;
    while (!done()) {
      assert.ok(Date.now() < deadline, `timed out waiting for ${what}: status=${JSON.stringify(watcher.status())} events=${JSON.stringify(events)}`);
      await delay(20);
    }
  };

  // macOS starts its file event stream shortly after the watch; retry staging if the first write was missed.
  await delay(600);
  const repoBefore = watcher.status()[repo];
  const beforeStage = watcher.status()[worktree];
  for (let attempt = 0; watcher.status()[worktree] === beforeStage; attempt++) {
    assert.ok(attempt < 5, 'staging in linked worktree did not advance counter');
    if (attempt > 0) git(worktree, 'restore', '--staged', '--', 'file');
    git(worktree, 'add', 'file');
    const wait = Date.now() + 2000;
    while (watcher.status()[worktree] === beforeStage && Date.now() < wait) await delay(20);
  }
  assert.equal(watcher.status()[repo], repoBefore);
  assert.ok(events.filter(event => event.folder === worktree).every(event => !event.refs));

  await delay(350);
  const beforeUnstage = watcher.status()[worktree];
  git(worktree, 'restore', '--staged', '--', 'file');
  await until(() => watcher.status()[worktree] > beforeUnstage, 'counter advance after unstaging in linked worktree');
  assert.equal(watcher.status()[repo], repoBefore);
  assert.ok(events.filter(event => event.folder === worktree).every(event => !event.refs));

  const beforeHead = watcher.status()[worktree];
  const seenBeforeHead = events.length;
  git(worktree, 'checkout', '-q', '--detach', 'HEAD');
  await until(
    () => watcher.status()[worktree] > beforeHead && events.slice(seenBeforeHead).some(event => event.folder === worktree && event.refs),
    'HEAD change in linked worktree',
  );
  assert.equal(watcher.status()[repo], repoBefore);
});

test('relative .git pointers watch linked metadata, filter lock and log files, and close on removal', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'plait-watch-rel-worktree-')));
  const repo = join(root, 'repo');
  const worktree = join(root, 'linked');
  await mkdir(repo);
  git(repo, 'init', '-q');
  await writeFile(join(repo, 'file'), 'initial\n');
  git(repo, 'add', 'file');
  git(repo, 'commit', '-qm', 'initial');
  git(repo, 'worktree', 'add', '-qb', 'rel', worktree);
  const gitDir = git(worktree, 'rev-parse', '--absolute-git-dir').trim();
  await writeFile(join(worktree, '.git'), `gitdir: ${relative(worktree, gitDir)}\n`);
  await writeFile(join(worktree, 'file'), 'edited\n');

  let notifications = 0;
  const watcher = new CheckoutWatcher(() => { notifications++; });
  t.after(async () => { watcher.close(); await rm(root, { recursive: true, force: true }); });
  watcher.sync([worktree]);

  const until = async (done: () => boolean, what: string) => {
    const deadline = Date.now() + 10_000;
    while (!done()) {
      assert.ok(Date.now() < deadline, `timed out waiting for ${what}: status=${JSON.stringify(watcher.status())}`);
      await delay(20);
    }
  };

  await delay(600);
  const beforeStage = watcher.status()[worktree];
  for (let attempt = 0; watcher.status()[worktree] === beforeStage; attempt++) {
    assert.ok(attempt < 5, 'staging via relative gitdir pointer did not advance counter');
    if (attempt > 0) git(worktree, 'restore', '--staged', '--', 'file');
    git(worktree, 'add', 'file');
    const wait = Date.now() + 2000;
    while (watcher.status()[worktree] === beforeStage && Date.now() < wait) await delay(20);
  }

  // Let any trailing event settle past the minimum announcement interval before testing ignored files.
  await delay(1200);
  const beforeIgnored = watcher.status()[worktree];
  await mkdir(join(gitDir, 'logs'), { recursive: true });
  await writeFile(join(gitDir, 'logs', 'HEAD'), 'ignored\n');
  await writeFile(join(gitDir, 'COMMIT_EDITMSG'), 'ignored\n');
  await writeFile(join(gitDir, 'index.lock'), 'ignored\n');
  await rm(join(gitDir, 'index.lock'));
  await delay(500);
  assert.equal(watcher.status()[worktree], beforeIgnored);

  git(worktree, 'restore', '--staged', '--', 'file');
  await until(() => watcher.status()[worktree] > beforeIgnored, 'counter advance after unstaging via relative gitdir');

  watcher.sync([]);
  assert.equal(watcher.status()[worktree], undefined);
  const countAfterRemove = notifications;
  git(worktree, 'add', 'file');
  await delay(400);
  assert.equal(notifications, countAfterRemove);
});

test('external metadata gets a shallow watch only when its backlink matches', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'plait-watch-backlink-')));
  const checkout = join(root, 'checkout');
  const other = join(root, 'other');
  const admin = join(other, '.git');
  const pointer = join(checkout, '.git');
  const watcher = new CheckoutWatcher(() => {});
  const watch = t.mock.method(fs, 'watch');
  syncBuiltinESMExports();
  t.after(async () => {
    watcher.close();
    watch.mock.restore();
    syncBuiltinESMExports();
    await rm(root, { recursive: true, force: true });
  });
  await mkdir(checkout);
  git(root, 'init', '-q', other);
  await writeFile(pointer, `gitdir: ${admin}\n`);
  const adminWatches = () => watch.mock.calls.filter(call => call.arguments[0] === admin);
  watcher.sync([checkout]);
  assert.equal(adminWatches().length, 0);
  await writeFile(join(admin, 'gitdir'), join(other, '.git'));
  watcher.sync([checkout]);
  assert.equal(adminWatches().length, 0);
  await writeFile(join(admin, 'gitdir'), relative(admin, pointer));
  watch.mock.mockImplementationOnce(() => { throw new Error('metadata watch unavailable'); });
  watcher.sync([checkout]);
  assert.equal(watcher.status()[checkout], 0);
  watcher.sync([checkout]);
  assert.equal(adminWatches().length, 2);
  const attached = adminWatches()[1];
  assert.deepEqual(attached.arguments[1], { recursive: false });
  const metadata = attached.result!;
  const close = metadata.close.bind(metadata);
  t.mock.method(metadata, 'close', () => { close(); throw new Error('metadata watch already closed'); });
  await rm(pointer);
  watcher.sync([checkout]);
  assert.equal(watcher.status()[checkout], 0);
});

test('the pointer read stays bounded when the open file grows', async t => {
  const checkout = await realpath(await mkdtemp(join(tmpdir(), 'plait-watch-read-')));
  const pointer = join(checkout, '.git');
  await writeFile(pointer, 'gitdir: admin\n');
  const watcher = new CheckoutWatcher(() => {});
  const open = t.mock.method(fs, 'openSync');
  const originalRead = fs.readSync;
  let bytes = 0;
  const read = t.mock.method(fs, 'readSync', (fd: number, buffer: Buffer, offset: number, length: number, position: null) => {
    if (bytes === 0) fs.appendFileSync(pointer, Buffer.alloc(32768));
    // Force short reads so the limit must hold across multiple calls.
    const count = originalRead(fd, buffer, offset, Math.min(length, 2048), position);
    bytes += count;
    return count;
  });
  syncBuiltinESMExports();
  try {
    watcher.sync([checkout]);
    const opens = open.mock.calls.filter(call => call.arguments[0] === pointer && typeof call.arguments[1] === 'number');
    assert.equal(opens.length, 1);
    const required = fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK;
    const flags = opens[0].arguments[1];
    assert.ok(typeof flags === 'number');
    assert.equal(flags & required, required);
    assert.equal(bytes, 8193);
  } finally {
    watcher.close();
    open.mock.restore();
    read.mock.restore();
    syncBuiltinESMExports();
    await rm(checkout, { recursive: true, force: true });
  }
});

test('plain folders and ordinary repositories skip the linked metadata reader on every sync', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-watch-ordinary-'));
  const dotGit = join(root, '.git');
  const watcher = new CheckoutWatcher(() => {});
  const open = t.mock.method(fs, 'openSync');
  syncBuiltinESMExports();
  try {
    watcher.sync([root]);
    await mkdir(dotGit);
    watcher.sync([root]);
    assert.equal(watcher.status()[root], 0);
    assert.equal(open.mock.calls.some(call => call.arguments[0] === dotGit), false);
  } finally {
    watcher.close();
    open.mock.restore();
    syncBuiltinESMExports();
    await rm(root, { recursive: true, force: true });
  }
});
