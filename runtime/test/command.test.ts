import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { commandPlatform } from '../src/command/platform.ts';
import { runGuardian, COMMAND_TIMEOUT_MS } from '../src/command/supervisor.ts';

const quote = (text: string) => `'${text.replaceAll("'", "'\\''")}'`;

async function setup(t: { after: (fn: () => Promise<void>) => void }) {
  const root = await fixtureDirectory(join(tmpdir(), 'plait-guardian-'));
  const helper = join(root, 'helper');
  const fixture = fileURLToPath(new URL('./support/guardian.mjs', import.meta.url));
  await writeFile(helper, `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(fixture)} "$@"\n`, { mode: 0o700 });
  const previous = process.env.PLAIT_HOST_EXE;
  const lock = process.env.PLAIT_LOCK_FD;
  process.env.PLAIT_HOST_EXE = helper;
  delete process.env.PLAIT_LOCK_FD;
  t.after(async () => {
    if (previous === undefined) delete process.env.PLAIT_HOST_EXE;
    else process.env.PLAIT_HOST_EXE = previous;
    if (lock === undefined) delete process.env.PLAIT_LOCK_FD;
    else process.env.PLAIT_LOCK_FD = lock;
    await rm(root, { recursive: true, force: true });
  });
  const run = (text: string, signal = new AbortController().signal, output: (chunk: string) => void = () => {}, timeout = COMMAND_TIMEOUT_MS) =>
    runGuardian({ root, text, policy: 'scripted lifecycle fixture, not a sandbox', env: process.env }, signal, output, timeout);
  return { root, run };
}

test('only the validated platform supplies command policy', () => {
  assert.equal(commandPlatform('darwin').scratchPrefix, '/private/tmp/plait-command-');
  for (const platform of ['linux', 'win32', 'freebsd'] as const) {
    assert.throws(() => commandPlatform(platform), /containment is not enabled/);
  }
  assert.equal(COMMAND_TIMEOUT_MS, 900000);
});

test('guardian drains final output and bounds retained and streamed text', async t => {
  const { run } = await setup(t);
  const chunks: string[] = [];
  const result = await run('output', undefined, chunk => chunks.push(chunk));
  assert(result.endsWith('FINAL_OUTPUT'));
  assert.equal(result.length, 16384);
  assert(chunks.every(chunk => chunk.length <= 4096));
});

test('guardian decodes split UTF-8 independently on stdout and stderr', { timeout: 5000 }, async t => {
  const { root, run } = await setup(t);
  const chunks: string[] = [];
  const result = await run('unicode', undefined, chunk => {
    chunks.push(chunk);
    for (const marker of ['STDOUT_READY', 'STDERR_READY']) {
      if (chunk.includes(marker)) writeFileSync(join(root, marker), 'SCRIPTED_FIXTURE_ONLY');
    }
  });
  assert(result.includes('€')); assert(result.includes('🚀'));
  assert(!result.includes('�'));
  assert.equal(chunks.join(''), result);
});

test('guardian bounds do not create unpaired Unicode surrogates', async t => {
  const { run } = await setup(t);
  const chunks: string[] = [];
  const result = await run('unicode-bounds', undefined, chunk => chunks.push(chunk));
  assert(result.length <= 16384);
  assert.equal(Buffer.from(result).toString(), result);
  for (const chunk of chunks) {
    assert(chunk.length <= 4096);
    assert.equal(Buffer.from(chunk).toString(), chunk);
  }
});

test('guardian flushes incomplete UTF-8 at normal end of output', async t => {
  const { run } = await setup(t);
  const chunks: string[] = [];
  assert.equal(await run('incomplete-utf8', undefined, chunk => chunks.push(chunk)), '�');
  assert.equal(chunks.join(''), '�');
});

test('guardian errors retain bounded diagnostics', async t => {
  const { run } = await setup(t);
  await assert.rejects(run('failure'), /Command ended without success \(3\).*FIXTURE_FAILURE/);
});

test('Stop closes the lifetime pipe and waits for guardian exit', { timeout: 5000 }, async t => {
  const { root, run } = await setup(t);
  const controller = new AbortController();
  await assert.rejects(run('wait', controller.signal, chunk => {
    if (chunk.includes('READY')) controller.abort();
  }), /EOF_SEEN/);
  const pid = Number(await readFile(join(root, 'started'), 'utf8'));
  assert.throws(() => process.kill(pid, 0));
});

test('deadline cancellation uses the same lifetime pipe', { timeout: 5000 }, async t => {
  const { root, run } = await setup(t);
  await assert.rejects(run('wait', undefined, undefined, 500), /EOF_SEEN/);
  const pid = Number(await readFile(join(root, 'started'), 'utf8'));
  assert.throws(() => process.kill(pid, 0));
});

test('already-cancelled commands do not start the guardian', async t => {
  const { root, run } = await setup(t);
  const controller = new AbortController(); controller.abort();
  await assert.rejects(run('output', controller.signal), /Run stopped/);
  await assert.rejects(readFile(join(root, 'started')), { code: 'ENOENT' });
});

test('spawn failure settles without leaving the command deadline active', { timeout: 5000 }, async t => {
  const { root, run } = await setup(t);
  process.env.PLAIT_HOST_EXE = join(root, 'missing');
  await assert.rejects(run('output'), /guardian could not start/);
});

test('detached fixture output cannot hold the subscription open', { timeout: 5000 }, async t => {
  const { run } = await setup(t);
  let pid = 0;
  t.after(async () => {
    if (pid) { try { process.kill(pid, 'SIGKILL'); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error; } }
  });
  const result = await run('detached', undefined, chunk => {
    const match = chunk.match(/DETACHED (\d+)/);
    if (match) pid = Number(match[1]);
  });
  assert(pid > 0);
  assert.match(result, /DETACHED/);
});
