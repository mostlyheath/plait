import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdir, mkdtemp, readFile, realpath, rename, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { acquireCheckout, projectKind } from '../src/safety.ts';
import { Runtime } from '../src/runtime.ts';
import { threadKind } from '../src/store.ts';
import type { ToolState } from '../src/tools.ts';
import { exec } from './support/safety.ts';

test('project kind requires no Git entry in any ancestor', async t => {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'plait-plain-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  const plain = join(base, 'plain'); await mkdir(plain);
  assert.deepEqual(await projectKind(plain), { root: plain, vcs: 'folder' });
  const repo = join(base, 'repo'); await exec('/usr/bin/git', ['init', '-q', repo]);
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

test('a missing project folder is reported as missing, not inaccessible', async t => {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'plait-missing-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  await assert.rejects(projectKind(join(base, 'gone')), (error: any) => error.code === 'checkout_missing' && /no longer exists/.test(error.message));
});

for (const deferred of [false, true]) test(
  `Git initialized before ${deferred ? 'deferred' : 'immediate'} lease acquisition protects dirty files and records the turn`, { timeout: 20000 }, async t => {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'plait-plain-git-')));
  const plain = join(base, 'plain');
  const store = join(base, 'store');
  await mkdir(plain);
  await mkdir(store, { mode: 0o700 });
  const models = createModels();
  const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([
    fauxAssistantMessage([fauxToolCall('edit', { path: 'notes.txt', old_text: 'dirty edit', new_text: 'overwritten' }, { id: 'edit-notes' })], { stopReason: 'toolUse' }),
    fauxAssistantMessage('Done.'),
  ]);
  models.setProvider(faux.provider);
  let observedBaseline: string[] = [];
  class PlainTestRuntime extends Runtime {
    override async resources() { return { skills: [], templates: [], problems: [], mcpServers: [], mcpEntries: new Map() }; }
    override registry(state: ToolState) {
      observedBaseline = [...state.baseline];
      return super.registry(state);
    }
  }
  const runtime = new PlainTestRuntime(store, models, new Set(['faux']));
  runtime.retry.enabled = false;
  t.after(async () => {
    await runtime.close();
    await rm(base, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 });
  });
  const call = async (request: Record<string, unknown>) => {
    const reply = await runtime.reply({ protocol_version: 6, ...request }) as { ok: boolean; result: any; error?: unknown };
    assert.equal(reply.ok, true, JSON.stringify(reply));
    return reply.result;
  };

  const created = await call({ op: 'create_thread', checkout: plain, request_id: 'create', harness: 'pi', provider: 'faux', model_id: 'faux-1' });
  const id: string = created.thread.id;
  assert.equal(threadKind(runtime.store, id), 'folder');

  const gitEnv = { PATH: '/usr/bin:/bin', HOME: '/nonexistent', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
  const git = (...args: string[]) => exec('/usr/bin/git', ['-C', plain, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', '-c', 'commit.gpgsign=false', ...args], { env: gitEnv });
  const initializeGit = async () => {
    const writer = await acquireCheckout(plain, 'writer');
    try {
      await writeFile(join(plain, 'notes.txt'), 'committed\n');
      await exec('/usr/bin/git', ['init', '-q', plain], { env: gitEnv });
      await git('add', 'notes.txt');
      await git('commit', '-qm', 'initial');
      await writeFile(join(plain, 'notes.txt'), 'dirty edit\n');
    } finally { await writer.close(); }
  };
  if (deferred) {
    t.mock.method(runtime.records, 'busy', () => true, { times: 1 });
    t.mock.method(runtime.records, 'settled', initializeGit, { times: 1 });
  } else {
    t.mock.method(runtime.admissions, 'probing', initializeGit, { times: 1 });
  }

  await call({ op: 'submit', thread_id: id, request_id: 'submit-1', text: 'Edit notes.txt', approval: 'auto' });
  const deadline = Date.now() + 20000;
  while (runtime.sessions.has(id) || runtime.runnerSessions.has(id)) {
    assert.ok(Date.now() < deadline, JSON.stringify(runtime.snapshot(id)));
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  const end = runtime.snapshot(id);
  assert.equal(end.status, 'idle');
  assert.equal(threadKind(runtime.store, id), 'git');
  assert.ok(observedBaseline.includes('notes.txt'), JSON.stringify(observedBaseline));
  assert.equal(await readFile(join(plain, 'notes.txt'), 'utf8'), 'dirty edit\n');
  assert.ok(end.tools.some(tool => tool.name === 'edit' && /protected|dirty/i.test(tool.output)), JSON.stringify(end.tools));
  const records = (await call({ op: 'turn_records', protocol_version: 7, thread_id: id })).records as { state: string }[];
  assert.equal(records.length, 1);
  assert.equal(records[0].state, 'ready');
});

for (const replacement of [false, true]) test(replacement
  ? 'a replacement Git directory cannot promote the saved folder thread'
  : 'a plain-folder thread whose ancestor becomes a Git repo refuses submission', { timeout: 20000 }, async t => {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'plait-plain-ancestor-')));
  const sub = join(base, 'sub');
  const store = join(base, 'store');
  await mkdir(sub);
  await mkdir(store, { mode: 0o700 });
  const models = createModels();
  const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('Done.')]);
  models.setProvider(faux.provider);
  const runtime = new Runtime(store, models, new Set(['faux']));
  runtime.retry.enabled = false;
  t.after(async () => {
    await runtime.close();
    await rm(base, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 });
  });

  const created = await runtime.reply({ protocol_version: 6, op: 'create_thread', checkout: sub, request_id: 'create', harness: 'pi', provider: 'faux', model_id: 'faux-1' }) as { ok: boolean; result: { thread: { id: string } } };
  assert.equal(created.ok, true);
  const id = created.result.thread.id;
  assert.equal(threadKind(runtime.store, id), 'folder');

  const gitEnv = { PATH: '/usr/bin:/bin', HOME: '/nonexistent', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
  if (replacement) await rename(sub, join(base, 'original'));
  await exec('/usr/bin/git', ['init', '-q', replacement ? sub : base], { env: gitEnv });

  const reply = await runtime.reply({ protocol_version: 6, op: 'submit', thread_id: id, request_id: 'submit-1', text: 'Run', approval: 'auto' }) as { ok: boolean; error?: { code: string; message: string } };
  assert.equal(reply.ok, false);
  assert.equal(reply.error?.code, replacement ? 'permission_denied' : 'invalid_request');
  assert.match(reply.error?.message ?? '', replacement ? /identity changed/ : /accessible Git working tree/);
  assert.equal(threadKind(runtime.store, id), 'folder');
  if (replacement) {
    await rename(sub, join(base, 'replacement'));
    await rename(join(base, 'original'), sub);
    const retry = await runtime.reply({ protocol_version: 6, op: 'submit', thread_id: id, request_id: 'submit-1', text: 'Run', approval: 'auto' });
    assert.equal(retry.ok, true, JSON.stringify(retry));
    await runtime.sessions.get(id)?.finished;
    assert.equal(runtime.snapshot(id).status, 'idle');
  }
});
