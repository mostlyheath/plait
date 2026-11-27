import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fixtureDirectory } from './support/temp.ts';

test('temporary fixtures are removed when setup throws before teardown is installed', async () => {
  const parent = await fixtureDirectory(join(tmpdir(), 'plait-fixture-test-'));
  const file = join(parent, 'failure.mjs');
  const helper = new URL('./support/temp.ts', import.meta.url).href;
  await writeFile(file, `
    import { test } from 'node:test';
    import { writeFile } from 'node:fs/promises';
    import { fixtureDirectory, fixtureDirectorySync } from ${JSON.stringify(helper)};
    test('failed setup', async () => {
      await fixtureDirectory(${JSON.stringify(join(parent, 'async-'))});
      fixtureDirectorySync(${JSON.stringify(join(parent, 'sync-'))});
      await writeFile(${JSON.stringify(join(parent, 'created'))}, '');
      throw new Error('intentional setup failure');
    });
  `);
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  await assert.rejects(promisify(execFile)(process.execPath, [file], { env, timeout: 30_000 }), { code: 1 });
  assert.deepEqual((await readdir(parent)).sort(), ['created', 'failure.mjs']);
});
