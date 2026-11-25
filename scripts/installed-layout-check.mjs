import { spawnSync } from 'node:child_process';
import { cp, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { installedFixture } from './installed-fixture.mjs';

const root = resolve(import.meta.dirname, '..');
const base = await mkdtemp(join(tmpdir(), 'plait-installed-layout-'));
try {
  const runtime = join(base, 'Host Release', 'runtime');
  await mkdir(runtime, { recursive: true });
  for (const name of ['src', 'package.json', 'node_modules']) {
    await cp(join(root, 'runtime', name), join(runtime, name), { recursive: true, verbatimSymlinks: true });
  }
  const fixture = await installedFixture(runtime, base);
  const result = spawnSync(process.execPath, ['--test', 'test/host-unicode.test.ts'], {
    cwd: join(root, 'runtime'), stdio: 'inherit',
    env: { ...process.env, PLAIT_TEST_RUNTIME: fixture,
      PLAIT_HOST_BINARY: resolve(process.env.PLAIT_HOST_BINARY ?? join(root, 'target/debug/plait-host')) },
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} finally {
  await rm(base, { recursive: true, force: true });
}
