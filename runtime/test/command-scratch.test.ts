import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmod, mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { removeScratch, sweepStaleScratch } from '../src/command/scratch.ts';

test('scratch removal recovers owner access to folders a command locked', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-scratch-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const scratch = join(root, 'plait-command-locked');
  await mkdir(join(scratch, 'inner', 'deep'), { recursive: true });
  await writeFile(join(scratch, 'inner', 'deep', 'file'), 'x');
  await chmod(join(scratch, 'inner', 'deep'), 0o000);
  await chmod(join(scratch, 'inner'), 0o500);
  await removeScratch(scratch);
  assert.deepEqual(await readdir(root), []);
});

test('scratch removal never throws for a missing folder', async () => {
  await removeScratch(join(tmpdir(), 'plait-command-does-not-exist-' + process.pid));
});

test('the startup sweep removes only stale command scratch with the prefix', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-scratch-sweep-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const name of ['plait-command-old', 'plait-command-new', 'other-old']) await mkdir(join(root, name));
  await writeFile(join(root, 'plait-command-file'), 'not a folder');
  // Birth time can't be set portably, so judge "now" relative to the folders' creation instead.
  const removed = await sweepStaleScratch(join(root, 'plait-command-'), 60_000, Date.now() + 120_000);
  assert.equal(removed, 2);
  assert.deepEqual((await readdir(root)).sort(), ['other-old', 'plait-command-file']);
  const none = await sweepStaleScratch(join(root, 'plait-command-'), 60_000);
  assert.equal(none, 0);
});
