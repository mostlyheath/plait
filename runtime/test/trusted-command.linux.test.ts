import assert from 'node:assert/strict';
import { mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { trustedCommand } from '../src/command/trusted.ts';
import { rootIdentity } from '../src/safety.ts';

 test('Linux runs trusted commands and Stop kills their process group', { timeout: 15000 }, async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'plait-linux-trusted-')));
  const previous = process.env.PLAIT_HOST_EXE;
  process.env.PLAIT_HOST_EXE = resolve('../target/debug/plait-host');
  t.after(async () => { if (previous === undefined) delete process.env.PLAIT_HOST_EXE; else process.env.PLAIT_HOST_EXE = previous; await rm(root, { recursive: true, force: true }); });
  const run = { snapshot: { checkout: root }, baseline: [], owned: new Map(), rootIdentity: await rootIdentity(root) };
  assert.match(await trustedCommand(run, 'printf trusted-output', new AbortController().signal, () => {}), /trusted-output/);
  const controller = new AbortController(); let group = 0;
  await assert.rejects(trustedCommand(run, 'sleep 120 & printf "READY %s\n" "$$"; wait', controller.signal, chunk => {
    const match = chunk.match(/READY (\d+)/); if (match) { group = Number(match[1]); controller.abort(); }
  }));
  assert(group > 0);
  // A group can briefly retain orphaned zombies; it must contain no live processes.
  const { stdout } = await import('node:child_process').then(({ execFileSync }) => ({ stdout: execFileSync('ps', ['-eo', 'pgid=,stat='], { encoding: 'utf8' }) })).catch(() => ({ stdout: '' }));
  assert(stdout.trim().split('\n').every(line => { const [pgid, state] = line.trim().split(/\s+/); return Number(pgid) !== group || state.startsWith('Z'); }));
});
