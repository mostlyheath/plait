import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, writeFile, readFile, rm, symlink, link, chmod, stat, rename, lstat, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { test } from 'node:test';
import { fileAction, dirtyPaths, identities, safePath, acquireCheckout, acquireCommonMetadata, checkoutLockName, rootIdentity, checkRoot } from '../src/safety.ts';
import { Store } from '../src/store.ts';
import { prepareBuildCache } from '../src/build-cache.ts';
import { validate } from '../src/protocol.ts';
import type { Snapshot } from '../src/protocol.ts';
import { boundSnapshot } from '../src/transcript.ts';
import { restoreOwned } from '../src/provenance.ts';
import { createHash, randomUUID } from 'node:crypto';
import { exec, setup } from './support/safety.ts';

test('snapshot budget counts UTF-8 and JSON escapes without clipping approval arguments', () => {
  const snapshot: Snapshot = { id: 'x', checkout: '/repo', model: { provider: 'faux', model_id: 'faux-1' }, status: 'idle',
    messages: Array.from({ length: 100 }, (_, index) => ({ id: String(index), role: 'assistant', text: '─'.repeat(4096) })),
    tools: Array.from({ length: 50 }, (_, index) => ({ id: String(index), name: 'read', status: 'done', input: '\u0000'.repeat(4096), output: '─'.repeat(4096) })),
    pending_approval: { id: 'a', tool_id: 't', kind: 'edit', summary: 'change', details: '─'.repeat(4000), dirty_paths: [], risk: 'exact' }, error: null, truncated: false, live_text: '─'.repeat(16384) };
  const details = snapshot.pending_approval!.details;
  boundSnapshot(snapshot);
  assert(Buffer.byteLength(JSON.stringify(snapshot)) <= 524288);
  assert(snapshot.truncated); assert.equal(snapshot.pending_approval!.details, details);
});

test('strict request admission', () => {
  for (const input of [null, [], {}, { op: 'watch' }, { op: 'status', unknown: 1 }, { op: 'submit', thread_id: '../bad', request_id: 'x', text: 'hello' }]) {
    assert.throws(() => validate(input));
  }
  assert.deepEqual(validate({ op: 'status' }), { op: 'status' });
});

test('cache preparation rejects symlink roots, insecure ownership modes and path IDs', async t => {
  const { base } = await setup(t);
  const store = join(base, 'store'); await mkdir(store, { mode: 0o700 });
  const cache = { store, thread: randomUUID() };
  await assert.rejects(prepareBuildCache({ store, thread: '../other' }));
  const { target } = await prepareBuildCache(cache);
  await rm(target, { recursive: true }); await symlink(base, target);
  await assert.rejects(prepareBuildCache(cache), /symlink/);
  await rm(target); await mkdir(target, { mode: 0o755 });
  // The rejection needs an insecure fixture even under a private umask.
  await chmod(target, 0o755);
  await assert.rejects(prepareBuildCache(cache), /private/);
  await symlink(store, join(base, 'alias'));
  await assert.rejects(prepareBuildCache({ store: join(base, 'alias'), thread: randomUUID() }), /symlink/);
});

test('file helpers reject hard links, symlink parents, duplicate replacement and overwrite', async t => {
  const { root, base } = await setup(t);
  await writeFile(join(root, 'file.txt'), 'same same');
  await assert.rejects(fileAction(root, 'edit', 'file.txt', { expected: 'same', content: 'bad' }));
  await assert.rejects(fileAction(root, 'write', 'file.txt', { content: 'bad' }));
  await link(join(root, 'file.txt'), join(base, 'hard.txt'));
  await assert.rejects(fileAction(root, 'read', 'file.txt'));
  await symlink(base, join(root, 'parent'));
  await assert.rejects(fileAction(root, 'read', 'parent/hard.txt'));
  assert.equal(await readFile(join(root, 'file.txt'), 'utf8'), 'same same');
});

test('case and Unicode aliases cannot bypass dirty identities or Git metadata checks', async t => {
  const { root } = await setup(t);
  for (const alias of ['.GIT/config', '.Git/config', '.git/config']) await assert.rejects(fileAction(root, 'read', alias));
  await writeFile(join(root, 'notes.txt'), 'dirty');
  const protectedFiles = await identities(root, ['notes.txt']);
  assert.throws(() => safePath('NOTES.TXT', ['notes.txt']));
  await assert.rejects(fileAction(root, 'edit', 'NOTES.TXT', { expected: 'dirty', content: 'bad', protected: protectedFiles }));
  const nfc = 'caf\u00e9.txt'; const nfd = nfc.normalize('NFD');
  await writeFile(join(root, nfc), 'dirty');
  assert.throws(() => safePath(nfd, [nfc]));
  await assert.rejects(fileAction(root, 'edit', nfd, { expected: 'dirty', content: 'bad', protected: await identities(root, [nfc]) }));
  assert.equal(await readFile(join(root, nfc), 'utf8'), 'dirty');
  assert.equal(await readFile(join(root, 'notes.txt'), 'utf8'), 'dirty');
});

test('atomic edits preserve mode and reject changed approval content; replaced checkout is rejected', async t => {
  const { root, base } = await setup(t);
  await writeFile(join(root, 'code.sh'), 'before'); await chmod(join(root, 'code.sh'), 0o755);
  const before = await stat(join(root, 'code.sh'));
  const result = await fileAction(root, 'edit', 'code.sh', { expected: 'before', content: 'after', expected_file: 'before' });
  assert.equal(result.postimage.sha256, createHash('sha256').update('after').digest('hex'));
  assert.equal(result.postimage.path, 'code.sh');
  assert(!Object.hasOwn(result.postimage, 'atime'));
  const after = await stat(join(root, 'code.sh'));
  assert.notEqual(before.ino, after.ino); assert.equal(after.mode & 0o777, 0o755);
  await assert.rejects(fileAction(root, 'edit', 'code.sh', { expected: 'after', content: 'bad', expected_file: 'wrong' }));
  assert.equal(await readFile(join(root, 'code.sh'), 'utf8'), 'after');
  const saved = await rootIdentity(root);
  await rename(root, join(base, 'old')); await mkdir(root);
  await writeFile(join(root, 'code.sh'), 'replacement');
  await assert.rejects(checkRoot(root, saved), /identity changed/);
  await assert.rejects(fileAction(root, 'read', 'code.sh', undefined, undefined, saved));
});

test('saved checkout identity refreshes after a device change and refuses replacements', async t => {
  const { root, base } = await setup(t);
  const store = new Store(base); t.after(() => store.close());
  const current = await rootIdentity(root);
  const saved = { ...current, dev: current.dev === '0' ? '1' : '0' };
  store.db.prepare('INSERT INTO roots VALUES (?,?)').run('thread', JSON.stringify(saved));
  assert.deepEqual(await store.refreshRootIdentity('thread', root), current);
  assert.deepEqual(store.rootIdentity('thread'), current);
  store.db.prepare('UPDATE roots SET identity=? WHERE id=?').run(JSON.stringify({ ...saved, ino: '0' }), 'thread');
  await assert.rejects(store.refreshRootIdentity('thread', root), /identity changed/);
  store.db.prepare('UPDATE roots SET identity=? WHERE id=?').run(JSON.stringify(saved), 'thread');
  await rename(root, join(base, 'original')); await symlink(join(base, 'original'), root);
  await assert.rejects(store.refreshRootIdentity('thread', root));
  assert.deepEqual(store.rootIdentity('thread'), saved);
});

test('provenance never exempts ignored parents and never adopts a later unrelated file state', async t => {
  const { root } = await setup(t); await mkdir(join(root, 'generated'));
  const identity = await rootIdentity(root);
  const result = await fileAction(root, 'write', 'generated/new.txt', { content: 'known' }, undefined, identity);
  let restored = await restoreOwned(root, await dirtyPaths(root), [result.postimage], identity);
  assert.equal(restored.size, 1);
  await writeFile(join(root, '.gitignore'), 'generated/\n');
  restored = await restoreOwned(root, await dirtyPaths(root), [result.postimage], identity);
  assert.equal(restored.size, 0);
  await rm(join(root, '.gitignore'));
  await writeFile(join(root, 'generated/new.txt'), 'unknown later state');
  restored = await restoreOwned(root, await dirtyPaths(root), [result.postimage], identity);
  assert.equal(restored.size, 0);
});

test('checkout writer lease excludes different hosts and releases on close', async t => {
  const { root } = await setup(t);
  const first = await acquireCheckout(root);
  await assert.rejects(acquireCheckout(root), /already has a host writer/);
  await first.close();
  const second = await acquireCheckout(root); await second.close();
});

test('Git dirty baseline includes staged, untracked, rename source without running filters', async t => {
  const { root, base } = await setup(t);
  await writeFile(join(root, 'original.txt'), 'before');
  await exec('/usr/bin/git', ['-C', root, 'add', '.']);
  await exec('/usr/bin/git', ['-C', root, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', 'commit', '-qm', 'fixture']);
  await exec('/usr/bin/git', ['-C', root, 'mv', 'original.txt', 'renamed.txt']);
  await writeFile(join(root, 'untracked.txt'), 'keep');
  await writeFile(join(root, '.gitattributes'), '*.txt filter=evil\n');
  await writeFile(join(root, '.gitignore'), '.env\n'); await writeFile(join(root, '.env'), 'private');
  await exec('/usr/bin/git', ['-C', root, 'config', 'filter.evil.clean', `touch '${join(base, 'BAD')}'`]);
  const dirty = await dirtyPaths(root);
  assert(dirty.includes('original.txt')); assert(dirty.includes('renamed.txt')); assert(dirty.includes('untracked.txt')); assert(dirty.includes('.env'));
  await assert.rejects(readFile(join(base, 'BAD')));
});

test('read-only originals retain mode and identity on a rejected edit', async t => {
  const { root } = await setup(t);
  const path = join(root, 'readonly.txt');
  await writeFile(path, 'before\n');
  await chmod(path, 0o444);
  const before = await stat(path, { bigint: true });
  const image = (await fileAction(root, 'read', 'readonly.txt')).postimage;
  await assert.rejects(fileAction(root, 'edit', 'readonly.txt', { expected: 'not present', content: 'bad', expected_image: image }), /old_text must match exactly once/);
  assert.equal((await stat(path, { bigint: true })).ctimeNs, before.ctimeNs);
  await fileAction(root, 'edit', 'readonly.txt', { expected: 'before', content: 'after', expected_image: image });
  assert.equal(await readFile(path, 'utf8'), 'after\n');
  assert.equal((await stat(path)).mode & 0o777, 0o444);
});

test('reader lifecycle lease coexists with writer and blocks exclusive checkout mutations', async t => {
  const { root } = await setup(t);
  const readerA = await acquireCheckout(root, 'reader');
  const readerB = await acquireCheckout(root, 'reader');
  const writer = await acquireCheckout(root, 'writer');
  await assert.rejects(acquireCheckout(root, 'writer'), /already has a host writer/);
  await assert.rejects(acquireCheckout(root, 'exclusive'), { code: 'busy' });
  await writer.close();
  // Readers alone still block exclusive checkout mutations (Git writes, undo, worktree removal).
  await assert.rejects(acquireCheckout(root, 'exclusive'), { code: 'busy' });
  await readerA.close();
  await assert.rejects(acquireCheckout(root, 'exclusive'), { code: 'busy' });
  await readerB.close();
  // Exclusive blocks readers, writers, and other exclusive operations.
  const exclusive = await acquireCheckout(root, 'exclusive');
  await assert.rejects(acquireCheckout(root, 'reader'), { code: 'busy' });
  await assert.rejects(acquireCheckout(root, 'writer'), { code: 'busy' });
  await assert.rejects(acquireCheckout(root, 'exclusive'), { code: 'busy' });
  await exclusive.close();
  const after = await acquireCheckout(root, 'writer');
  await after.close();
});

test('checkout aliases and canonical paths contend on the same lifecycle and writer leases', async t => {
  const { root, base } = await setup(t);
  const alias = join(base, 'repo-alias');
  await symlink(root, alias);
  const held = await acquireCheckout(root, 'writer');
  await assert.rejects(acquireCheckout(alias, 'writer'), { code: 'busy' });
  await assert.rejects(acquireCheckout(alias, 'exclusive'), { code: 'busy' });
  await held.close();
  const exclusiveAlias = await acquireCheckout(alias, 'exclusive');
  assert.equal(exclusiveAlias.root, root);
  await assert.rejects(acquireCheckout(root, 'writer'), { code: 'busy' });
  await assert.rejects(acquireCheckout(root, 'reader'), { code: 'busy' });
  await exclusiveAlias.close();
});

test('shared common-dir metadata lease serializes linked worktrees while unrelated checkouts progress', async t => {
  const { root, base } = await setup(t);
  await writeFile(join(root, 'a.txt'), 'init\n');
  await exec('/usr/bin/git', ['-C', root, 'add', '.']);
  await exec('/usr/bin/git', ['-C', root, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', 'commit', '-qm', 'init']);
  const wt = join(base, 'wt1');
  await exec('/usr/bin/git', ['-C', root, 'worktree', 'add', '-q', '-b', 'wt-branch', wt, 'HEAD']);
  const other = join(base, 'unrelated');
  await mkdir(other);
  await exec('/usr/bin/git', ['init', '-q', other]);

  const common = await acquireCommonMetadata(root, 0);
  // Linked worktree shares common_dir, so a zero-wait metadata lock is busy.
  await assert.rejects(acquireCommonMetadata(wt, 20), { code: 'busy' });
  // Unrelated repository has its own common_dir and progresses immediately.
  const otherCommon = await acquireCommonMetadata(other, 0);
  await otherCommon.close();
  // Bounded wait serializes cleanly once the holder releases.
  const started = Date.now();
  const releaser = setTimeout(() => void common.close(), 120);
  t.after(() => clearTimeout(releaser));
  const next = await acquireCommonMetadata(wt, 2000);
  assert(Date.now() - started >= 80, 'expected linked worktree metadata lease to wait');
  await next.close();
});

test('process crash releases leases while an uncertain descendant holding lease descriptors keeps them held', async t => {
  const { root, base } = await setup(t);
  const host = process.env.PLAIT_HOST_EXE!;

  // 1. Plain holder process killed with SIGKILL releases both leases immediately.
  const plainMarker = join(base, 'plain-ready');
  const lease = await acquireCheckout(root, 'writer');
  const lockPath = await (async () => {
    const out = await exec(host, ['lease-prepare', 'checkout', root]);
    return JSON.parse(out.stdout).lock_path as string;
  })();
  await lease.close();

  const plain = spawn('/bin/sh', ['-c', `exec 3<>'${lockPath}' 4<'${root}'; '${host}' lock-fd writer '${root}' || exit 1; echo $$ > '${plainMarker}'; exec sleep 30`], { stdio: 'ignore' });
  t.after(() => { if (plain.exitCode === null) plain.kill('SIGKILL'); });
  for (let i = 0; i < 100 && !(await stat(plainMarker).catch(() => null)); i++) await new Promise(r => setTimeout(r, 20));
  await assert.rejects(acquireCheckout(root, 'exclusive'), { code: 'busy' });
  plain.kill('SIGKILL');
  await new Promise(resolve => plain.once('exit', resolve));
  const afterCrash = await acquireCheckout(root, 'exclusive');
  await afterCrash.close();

  // 2. Detached descendant inheriting lease descriptors keeps the lease held after the parent closes them.
  const held = await acquireCheckout(root, 'writer');
  const descendant = spawn('/bin/sh', ['-c', 'exec sleep 30'], {
    detached: true,
    stdio: ['ignore', 'ignore', 'ignore', held.lifecycleFd, held.fd!],
  });
  t.after(() => { try { process.kill(-descendant.pid!, 'SIGKILL'); } catch {} });
  // Closing the parent's handles must NOT release the lock while the descendant stays alive.
  await held.close();
  await assert.rejects(acquireCheckout(root, 'writer'), { code: 'busy' });
  await assert.rejects(acquireCheckout(root, 'exclusive'), { code: 'busy' });
  process.kill(-descendant.pid!, 'SIGKILL');
  await new Promise(resolve => descendant.once('exit', resolve));
  const recovered = await acquireCheckout(root, 'exclusive');
  await recovered.close();
});

test('a checkout lease starts one host helper, as before lifecycle locks', async t => {
  const { root, base } = await setup(t);
  const log = join(base, 'helpers.log');
  const wrapper = join(base, 'plait-host');
  const real = process.env.PLAIT_HOST_EXE!;
  await writeFile(wrapper, `#!/bin/sh\necho "$1" >> '${log}'\nexec '${real}' "$@"\n`, { mode: 0o700 });
  process.env.PLAIT_HOST_EXE = wrapper;
  try {
    for (const mode of ['writer', 'exclusive', 'reader'] as const) {
      await writeFile(log, '');
      const lease = await acquireCheckout(root, mode);
      await lease.close();
      assert.deepEqual((await readFile(log, 'utf8')).trim().split('\n'), ['lock-fd'], mode);
    }
  } finally { process.env.PLAIT_HOST_EXE = real; }
  // The lock file the runtime opened is the one plait-host verifies and reports.
  const prepared = JSON.parse((await exec(real, ['lease-prepare', 'checkout', root])).stdout);
  assert.equal(prepared.lock_path.split('/').at(-1), checkoutLockName({ dev: prepared.dev, ino: prepared.ino }));
});

test('tests keep lock files out of the account lease directory', async t => {
  const { root } = await setup(t);
  const lease = await acquireCheckout(root, 'writer');
  await lease.close();
  const name = checkoutLockName(lease.identity);
  assert.notEqual(process.env.PLAIT_LEASE_DIR, undefined);
  assert.equal((await stat(join(process.env.PLAIT_LEASE_DIR!, name))).isFile(), true);
  await assert.rejects(stat(join(homedir(), '.local/share/plait-leases', name)), { code: 'ENOENT' });
});

test('cleanup markers survive lease loss, cover aliases, and reject stale resolution', async t => {
  const { cleanupHelper } = await import('../src/cleanup.ts');
  const { root, base } = await setup(t);
  const alias = join(base, 'alias'); await symlink(root, alias);
  const lease = await acquireCheckout(root);
  await cleanupHelper('begin', root, 'run-one', 'The command group did not stop.', lease);
  await lease.close();
  try {
    for (const mode of ['writer', 'exclusive', 'reader'] as const) {
      await assert.rejects(acquireCheckout(alias, mode), { code: 'cleanup_uncertain' });
    }
    await assert.rejects(cleanupHelper('resolve', root, 'old-run'), /changed/);
    assert.equal((await cleanupHelper('read', root)).cleanup?.token, 'run-one');
  } finally { await cleanupHelper('resolve', root, 'run-one'); }
  const next = await acquireCheckout(root); await next.close();
});

test('cleanup requests require exact fields and explicit confirmation', async () => {
  const { validateCleanup } = await import('../src/cleanup.ts');
  assert.deepEqual(validateCleanup({ op: 'checkout_cleanup', checkout: '/repo' }), { op: 'checkout_cleanup', checkout: '/repo' });
  assert.equal(validateCleanup({ op: 'stop_checkout_cleanup', checkout: '/repo', token: 'git-abc', confirm_stop: true }).op, 'stop_checkout_cleanup');
  for (const bad of [
    { op: 'stop_checkout_cleanup', checkout: '/repo', token: 'git-abc', confirm_stop: false },
    { op: 'stop_checkout_cleanup', checkout: '/repo', token: 'git-abc', confirm_stopped: true },
    { op: 'checkout_cleanup', checkout: 'relative' },
    { op: 'checkout_cleanup', checkout: '/repo', extra: true },
    { op: 'resolve_checkout_cleanup', checkout: '/repo', token: 'run' },
    { op: 'resolve_checkout_cleanup', checkout: '/repo', token: 'run', confirm_stopped: false },
    { op: 'resolve_checkout_cleanup', checkout: '/repo', token: '../bad', confirm_stopped: true },
  ]) assert.throws(() => validateCleanup(bad), { code: 'invalid_request' });
});

test('a live run permits parent lifecycle readers but abandoned and uncertain markers refuse them', async t => {
  const { cleanupHelper, RESTART_REASON } = await import('../src/cleanup.ts');
  const { root } = await setup(t);
  const writer = await acquireCheckout(root);
  await cleanupHelper('begin', root, 'live-run', RESTART_REASON, writer);
  assert.equal((await cleanupHelper('read', root)).cleanup, null, 'a separate reader sees a live owner, not an unconfirmed stop');
  const reader = await acquireCheckout(root, 'reader'); await reader.close();
  await cleanupHelper('mark', root, 'live-run', 'The command group did not stop.', writer);
  assert.equal((await cleanupHelper('read', root)).cleanup?.token, 'live-run');
  await assert.rejects(acquireCheckout(root, 'reader'), { code: 'cleanup_uncertain' });
  await cleanupHelper('mark', root, 'live-run', RESTART_REASON, writer);
  await writer.close();
  try { await assert.rejects(acquireCheckout(root, 'reader'), { code: 'cleanup_uncertain' }); }
  finally { await cleanupHelper('resolve', root, 'live-run'); }
});

test('a live undo is not reported as a host that ended before confirming cleanup', async t => {
  const { cleanupHelper, RESTART_REASON } = await import('../src/cleanup.ts');
  const { root } = await setup(t);
  const undo = await acquireCheckout(root, 'exclusive');
  try {
    await cleanupHelper('begin', root, 'live-undo', RESTART_REASON, undo);
    assert.equal((await cleanupHelper('read', root)).cleanup, null, 'a live undo holds its liveness lock');
    await cleanupHelper('finish', root, 'live-undo', undefined, undo);
  } finally { await undo.close(); }
  // A crashed undo still leaves the marker visible.
  const crashed = await acquireCheckout(root, 'exclusive');
  await cleanupHelper('begin', root, 'crashed-undo', RESTART_REASON, crashed);
  await crashed.close();
  try { assert.equal((await cleanupHelper('read', root)).cleanup?.token, 'crashed-undo'); }
  finally { await cleanupHelper('resolve', root, 'crashed-undo'); }
});

/** Racing creators must not widen the lease directory or any lock file. */
async function assertPrivateLeases(directory: string) {
  assert.equal((await lstat(directory)).mode & 0o777, 0o700);
  const names = await readdir(directory);
  assert(names.length >= 2, 'lifecycle and cleanup lock files');
  for (const name of names) {
    const meta = await lstat(join(directory, name));
    assert(meta.isFile(), name); assert.equal(meta.nlink, 1, name); assert.equal(meta.mode & 0o777, 0o600, name);
  }
}
/** Even callers read and odd ones write; a writer may be refused as busy, and nothing else may fail. */
const unexpected = (results: PromiseSettledResult<unknown>[]) => results.flatMap((result, index) =>
  result.status === 'rejected' && !(index % 2 && (result.reason as { code?: string }).code === 'busy')
    ? [`${(result.reason as { code?: string }).code}: ${(result.reason as Error).message}`] : []);

// The lease directory is new the first time a build runs. On macOS, two
// O_CREAT opens of one new name at once through a directory descriptor, as
// the host helper opens lock files, can fail the slower with ENOENT.
test('leases taken at once in a fresh lease directory are all admitted', async t => {
  const { base } = await setup(t);
  const saved = process.env.PLAIT_LEASE_DIR;
  t.after(() => { process.env.PLAIT_LEASE_DIR = saved; });
  for (let round = 0; round < 20; round++) {
    const leases = join(base, `fresh-${round}`, 'leases');
    process.env.PLAIT_LEASE_DIR = leases;
    const root = join(base, `repo-${round}`); await mkdir(root);
    const taken = await Promise.allSettled(Array.from({ length: 8 }, (_, index) => acquireCheckout(root, index % 2 ? 'writer' : 'reader')));
    for (const result of taken) if (result.status === 'fulfilled') await result.value.close();
    assert.deepEqual(unexpected(taken), [], `round ${round}`);
    await assertPrivateLeases(leases);
  }
});

test('leases taken at once from separate runtimes in a fresh lease directory are all admitted', async t => {
  const { base } = await setup(t);
  const safety = new URL('../src/safety.ts', import.meta.url).href;
  // Each child waits at a gate so all of them open the new files together.
  const child = `import { writeFileSync, existsSync } from 'node:fs';
const { acquireCheckout } = await import(${JSON.stringify(safety)});
const [root, gate, ready, mode] = process.argv.slice(1);
writeFileSync(ready, '');
while (!existsSync(gate));
try { await (await acquireCheckout(root, mode)).close(); } catch (error) { process.stdout.write(JSON.stringify({ code: error.code, message: error.message })); }`;
  for (let round = 0; round < 8; round++) {
    const leases = join(base, `fresh-${round}`, 'leases');
    const root = join(base, `repo-${round}`); await mkdir(root);
    const gate = join(base, `gate-${round}`);
    const runs = Array.from({ length: 6 }, (_, index) => {
      const ready = join(base, `ready-${round}-${index}`);
      const run = spawn(process.execPath, ['--input-type=module', '-e', child, root, gate, ready, index % 2 ? 'writer' : 'reader'],
        { env: { ...process.env, PLAIT_LEASE_DIR: leases }, stdio: ['ignore', 'pipe', 'inherit'] });
      let out = ''; run.stdout!.setEncoding('utf8').on('data', chunk => { out += chunk; });
      const done = new Promise<void>((resolve, reject) => run.once('close', code => {
        if (code !== 0) return reject(new Error(`runtime ${index} exited ${code}`));
        if (!out) return resolve();
        const failure = JSON.parse(out) as { code: string; message: string };
        reject(Object.assign(new Error(failure.message), { code: failure.code }));
      }));
      return { ready, done };
    });
    t.after(() => writeFile(gate, '').catch(() => {}));
    for (let waited = 0; (await Promise.all(runs.map(run => stat(run.ready).then(() => true, () => false)))).includes(false); waited++) {
      assert(waited < 3000, 'runtimes did not start');
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    await writeFile(gate, '');
    assert.deepEqual(unexpected(await Promise.allSettled(runs.map(run => run.done))), [], `round ${round}`);
    await assertPrivateLeases(leases);
  }
});

// Opening a lock file can fail for reasons of its own. That is not evidence
// that a cleanup is unconfirmed, so it must not hold the checkout.
test('a lock file that cannot be opened fails the lease with its reason and holds nothing', async t => {
  const { cleanupHelper, RESTART_REASON } = await import('../src/cleanup.ts');
  const { root } = await setup(t);
  const name = checkoutLockName(await rootIdentity(root));
  const record = join(process.env.PLAIT_LEASE_DIR!, name.replace(/^checkout-/, 'cleanup-'));
  const gitRecord = join(process.env.PLAIT_LEASE_DIR!, name.replace(/^checkout-/, 'git-cleanup-'));
  const unopenable = (code: string) => (error: { code?: string; message: string }) => {
    assert.equal(error.code, code, error.message);
    if (code !== 'busy') assert.match(error.message, /Cannot open lease lock file/);
    return true;
  };
  // A directory where a record's lock file belongs cannot be opened. An earlier
  // test's removed folder can have had this folder's inode, and so left this name.
  await rm(record, { force: true });
  await mkdir(record);
  try {
    for (const mode of ['writer', 'exclusive', 'reader'] as const) await assert.rejects(acquireCheckout(root, mode), unopenable('permission_denied'));
    await assert.rejects(cleanupHelper('read', root), unopenable('permission_denied'));
  } finally { await rm(record, { recursive: true }); }
  const writer = await acquireCheckout(root, 'writer');
  try {
    // Saving a run's record when it starts can fail the same way.
    await rm(record); await mkdir(record);
    await assert.rejects(cleanupHelper('begin', root, 'run-one', RESTART_REASON, writer), unopenable('permission_denied'));
    await rm(record, { recursive: true });
    // A busy checkout stays busy when its Git record cannot be read.
    await rm(gitRecord); await mkdir(gitRecord);
    await assert.rejects(acquireCheckout(root, 'writer'), unopenable('busy'));
  } finally { await rm(gitRecord, { recursive: true, force: true }); await writer.close(); }
  // The refusals released their locks and saved nothing that holds the checkout.
  assert.equal((await cleanupHelper('read', root)).cleanup, null);
  await (await acquireCheckout(root, 'exclusive')).close();
});
