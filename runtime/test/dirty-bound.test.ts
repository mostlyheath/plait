import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { approvalDirtyPaths, dirtyPaths, identities } from '../src/safety.ts';
import { exec, setup } from './support/safety.ts';

const git = (root: string, ...args: string[]) => exec('/usr/bin/git', ['-C', root, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@invalid', ...args],
  { env: { PATH: '/usr/bin:/bin', HOME: '/nonexistent', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' } });

async function fill(directory: string, count: number, name = (index: number) => `file-${index}`) {
  await mkdir(directory, { recursive: true });
  for (let index = 0; index < count; index++) await writeFile(join(directory, name(index)), '');
}

// In-place installs and builds leave thousands of files. The 500-entry dirty
// bound must not count them when the project ignores their directory.
test('an ignored directory is one dirty entry however many files it holds', { timeout: 60000 }, async t => {
  const { root } = await setup(t);
  await writeFile(join(root, '.gitignore'), 'node_modules/\n/target\n__pycache__/\n*.pyc\n');
  await git(root, 'add', '.gitignore'); await git(root, 'commit', '-qm', 'fixture');
  assert.deepEqual(await dirtyPaths(root), []);
  await fill(join(root, 'node_modules', 'package', 'lib'), 3000);
  await fill(join(root, 'target', 'debug', 'deps'), 3000);
  await fill(join(root, 'packages', 'nested', 'node_modules', 'package'), 600);
  await fill(join(root, 'tools', '__pycache__'), 600, index => `module-${index}.pyc`);
  const dirty = await dirtyPaths(root);
  assert.deepEqual(dirty.sort(), ['node_modules', 'packages/nested/node_modules', 'target', 'tools/__pycache__']);
  assert.equal((await identities(root, dirty)).length, 4);
});

test('files ignored one by one, and untracked files, still count individually', { timeout: 60000 }, async t => {
  const { root } = await setup(t);
  await writeFile(join(root, '.gitignore'), '*.o\n');
  await git(root, 'add', '.gitignore'); await git(root, 'commit', '-qm', 'fixture');
  await fill(join(root, 'objects'), 40, index => `unit-${index}.o`);
  await fill(join(root, 'generated'), 40);
  assert.equal((await dirtyPaths(root)).length, 80);
  await fill(join(root, 'objects'), 501, index => `unit-${index}.o`);
  await assert.rejects(dirtyPaths(root), /Dirty baseline exceeds the safety bound/);
});

test('external edit approvals keep the dirty baseline for 501 modified tracked files', async t => {
  const { root } = await setup(t);
  await fill(root, 501);
  await writeFile(join(root, 'clean'), '');
  await git(root, 'add', '.');
  await git(root, 'commit', '-qm', 'fixture');
  for (let index = 0; index < 501; index++) await writeFile(join(root, `file-${index}`), 'changed');
  const baseline = await approvalDirtyPaths(root, 'git');
  assert.ok(baseline);
  assert.equal(baseline.size, 501);
  assert.ok(baseline.has('file-500'));
  assert.ok(!baseline.has('clean'));
});
