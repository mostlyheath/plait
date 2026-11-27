import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmod, mkdir, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Folders } from '../src/folders.ts';
import { HostError } from '../src/protocol.ts';

const error = (code: string, message: string) => (e: unknown) => e instanceof HostError && e.code === code && e.message === message;

test('browse folders and validate requests', async t => {
  const root = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-folders-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  const folders = new Folders(root);
  await Promise.all(['beta', 'Alpha', '.hidden', 'worktree'].map(name => mkdir(join(root, name))));
  await mkdir(join(root, 'Alpha', '.git'));
  await writeFile(join(root, 'worktree', '.git'), 'gitdir: elsewhere');
  await writeFile(join(root, 'file'), 'file');
  await symlink(join(root, 'Alpha'), join(root, 'link'));
  const result = await folders.handle({ op: 'browse_folder' });
  assert.equal(result.path, root);
  assert.equal(result.home, root);
  assert.deepEqual(result.entries, [{ name: 'Alpha', git: true }, { name: 'beta', git: false }, { name: 'link', git: true }, { name: 'worktree', git: true }]);
  assert.equal((await folders.handle({ op: 'browse_folder', path: join(root, 'Alpha') })).git, true);
  assert.equal((await folders.handle({ op: 'browse_folder', hidden: true })).entries[0].name, '.hidden');
  assert.equal((await folders.handle({ op: 'browse_folder', path: '/' })).parent, null);
  await assert.rejects(folders.handle({ op: 'browse_folder', path: join(root, 'missing') }), error('not_found', 'That folder does not exist.'));
  await assert.rejects(folders.handle({ op: 'browse_folder', path: join(root, 'file') }), error('invalid_request', 'That is not a folder.'));
  for (const path of ['relative', '/bad\npath', `/${'x'.repeat(4096)}`]) await assert.rejects(folders.handle({ op: 'browse_folder', path }), error('invalid_request', 'Invalid path.'));
  await assert.rejects(folders.handle({ op: 'browse_folder', protocol_version: 1 }), error('invalid_request', 'Unknown operation or field.'));
  if (process.getuid?.() !== 0) {
    const locked = join(root, 'locked');
    await mkdir(locked);
    await chmod(locked, 0);
    try { await assert.rejects(folders.handle({ op: 'browse_folder', path: locked }), error('permission_denied', 'Plait cannot read that folder.')); }
    finally { await chmod(locked, 0o700); }
  }
});

test('limits listings to 1000 folders', async t => {
  const root = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-folders-limit-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  await Promise.all(Array.from({ length: 1001 }, (_, i) => mkdir(join(root, `folder-${String(i).padStart(4, '0')}`))));
  const listing = await new Folders(root).handle({ op: 'browse_folder' });
  assert.equal(listing.entries.length, 1000);
  assert.equal(listing.truncated, true);
  assert.equal(listing.entries[999].name, 'folder-0999');
});
