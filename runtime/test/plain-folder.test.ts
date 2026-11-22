import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { projectKind } from '../src/safety.ts';

test('project kind requires no Git entry in any ancestor', async t => {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'plait-plain-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  const plain = join(base, 'plain'); await mkdir(plain);
  assert.deepEqual(await projectKind(plain), { root: plain, vcs: 'folder' });
  const repo = join(base, 'repo'); execFileSync('git', ['init', '-q', repo]);
  const child = join(repo, 'child'); await mkdir(child);
  assert.deepEqual(await projectKind(child), { root: repo, vcs: 'git' });
  // An empty stray `.git` folder is no repository, as Git itself decides.
  const stray = join(base, 'stray'); await mkdir(stray); await mkdir(join(stray, '.git'));
  assert.deepEqual(await projectKind(stray), { root: stray, vcs: 'folder' });
  // With HEAD it marks a repository, even a broken one: refused, never plain.
  const broken = join(base, 'broken'); await mkdir(join(broken, '.git'), { recursive: true });
  await writeFile(join(broken, '.git', 'HEAD'), 'garbage\n');
  await assert.rejects(projectKind(broken), /accessible Git working tree/);
  const file = join(base, 'file'); await mkdir(file); await writeFile(join(file, '.git'), 'gitdir: /missing\n');
  await assert.rejects(projectKind(file), /accessible Git working tree/);
});
