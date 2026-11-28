import { fixtureDirectory } from './temp.ts';
import { execFile } from 'node:child_process';
import { mkdir, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import './leases.ts';

export const exec = promisify(execFile);
process.env.PLAIT_HOST_EXE = process.env.PLAIT_HOST_BINARY ?? resolve('../target/debug/plait-host');

export async function setup(t: { after: (fn: () => Promise<void>) => void }) {
  const base = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-sandbox-')));
  // This runs before a test's own hook closes its runtime, and that runtime may still have a helper writing to its
  // store (a build seed is checked after a run), so the removal retries instead of failing on a folder being filled.
  t.after(async () => { await rm(base, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 }); });
  const root = join(base, 'repo'); await mkdir(root);
  await exec('/usr/bin/git', ['init', '-q', root]);
  return { root, base };
}
