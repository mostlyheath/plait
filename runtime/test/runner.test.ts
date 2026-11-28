import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { copyFile, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { test } from 'node:test';

const execute = promisify(execFile);
test('an inherited Node test context cannot silently skip the runtime suite', async t => {
  const root = await fixtureDirectory(join(tmpdir(), 'plait-runner-fixture-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const directory = join(root, 'test'); await mkdir(directory);
  const marker = join(root, 'executed');
  await copyFile(new URL('./run.mjs', import.meta.url), join(directory, 'run.mjs'));
  await writeFile(join(directory, 'probe.test.ts'), `
    import { test } from 'node:test';
    import { writeFileSync } from 'node:fs';
    test('disposable runner fixture', () => writeFileSync(${JSON.stringify(marker)}, 'executed'));
  `);
  // The installed suite reports active tests through a helper this fixture lacks.
  const { PLAIT_ACTIVE_TESTS: _, ...inherited } = process.env;
  const { stdout, stderr } = await execute(process.execPath, [join(directory, 'run.mjs')], {
    env: { ...inherited, NODE_TEST_CONTEXT: 'child-v8' }, timeout: 10000,
  });
  assert.equal(await readFile(marker, 'utf8'), 'executed');
  assert(!stderr.includes('skipping running files'), stderr);
  assert.match(stdout, /disposable runner fixture/);
});

test('the suite fails and cleans up a fixture missed by its worker', async () => {
  const root = await fixtureDirectory(join(tmpdir(), 'plait-runner-fixture-'));
  const directory = join(root, 'test');
  await mkdir(directory);
  const marker = join(root, 'leaked-path');
  await copyFile(new URL('./run.mjs', import.meta.url), join(directory, 'run.mjs'));
  await writeFile(join(directory, 'leak.test.ts'), `
    import { mkdtempSync, writeFileSync } from 'node:fs';
    import { tmpdir } from 'node:os';
    import { join } from 'node:path';
    writeFileSync(${JSON.stringify(marker)}, mkdtempSync(join(tmpdir(), 'plait-unowned-')));
  `);
  const env = { ...process.env };
  delete env.PLAIT_ACTIVE_TESTS;
  await assert.rejects(execute(process.execPath, [join(directory, 'run.mjs')], { env, timeout: 30_000 }), { code: 1 });
  const leaked = await readFile(marker, 'utf8');
  await assert.rejects(stat(leaked), { code: 'ENOENT' });
});
