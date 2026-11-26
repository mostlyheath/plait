import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { test } from 'node:test';

const execute = promisify(execFile);
test('an inherited Node test context cannot silently skip the runtime suite', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-runner-fixture-'));
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
