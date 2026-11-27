// Claude Code threads through the real runtime, against the scripted fake
// `claude` and the dev copy of the Agent SDK. Nothing here downloads anything,
// starts a real Claude Code, or reaches a model.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { mkdir, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { gzipSync } from 'node:zlib';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { ClaudeHarness } from '../src/claude/harness.ts';
import { CodexHarness } from '../src/harness.ts';
import type { ClaudeDependencies } from '../src/claude/harness.ts';
import { sdkDirectory } from '../src/claude/sdk.ts';
import type { Snapshot } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { DEV_SDK, FAKE_CLAUDE, fakeEnv } from './support/fake-claude-env.ts';
import { setup } from './support/safety.ts';

type Context = { after(fn: () => Promise<void>): void };
type Reply = { ok: boolean; result?: any; error?: { code: string; message: string } };

async function host(t: Context, scenario = 'turn', options: { store?: string; repo?: string; deps?: Partial<ClaudeDependencies> } = {}) {
  const { root, base } = options.repo ? { root: options.repo, base: '' } : await setup(t);
  const store = options.store ?? join(base, 'store');
  const sessions = join(store, 'fake-sessions');
  await mkdir(sessions, { recursive: true });
  const log = join(store, `claude-${scenario}.jsonl`);
  const models = createModels(); models.setProvider(fauxProvider().provider);
  const runtime = new Runtime(store, models, new Set(['faux']));
  noCodex(runtime);
  runtime.claude = new ClaudeHarness(runtime.store, {
    sdk: DEV_SDK, cwd: store,
    environment: async () => fakeEnv({ FAKE_CLAUDE_SCENARIO: scenario, FAKE_CLAUDE_LOG: log, FAKE_CLAUDE_STATE: sessions }),
    find: async () => ({ state: 'ready', path: FAKE_CLAUDE, version: '2.1.326', message: 'ready' }),
    fetch: (async () => { throw new Error('No test downloads anything.'); }) as unknown as typeof fetch,
    ...options.deps,
  });
  let closed = false;
  const close = async () => { if (!closed) { closed = true; await runtime.close(); } };
  t.after(close);
  const call = (request: Record<string, unknown>) => runtime.reply({ protocol_version: 4, ...request }) as Promise<Reply>;
  const create = async (mode?: string, model = 'default') => {
    const reply = await call({
      op: 'create_thread', checkout: root, provider: 'claude', model_id: model, request_id: `c${Date.now()}${Math.random()}`.replace('.', ''), harness: 'claude', thinking: 'high',
      ...(mode ? { mode } : {}),
    });
    assert.equal(reply.ok, true, JSON.stringify(reply));
    return reply.result.thread as Snapshot;
  };
  const until = async (id: string, done: (snapshot: Snapshot) => boolean, ms = 8000) => {
    const deadline = Date.now() + ms;
    for (;;) {
      const snapshot = runtime.snapshot(id);
      if (done(snapshot)) return structuredClone(snapshot);
      if (Date.now() > deadline) assert.fail(`Timed out; last snapshot: ${JSON.stringify(snapshot)}`);
      await new Promise(resolve => setTimeout(resolve, 10));
    }
  };
  const idle = (id: string) => until(id, snapshot => !runtime.sessions.has(id) && !['running', 'awaiting_approval'].includes(snapshot.status));
  const frames = async () => (await readFile(log, 'utf8').catch(() => '')).trim().split('\n').filter(Boolean).map(line => JSON.parse(line));
  /** Claude Code sessions started (the model check counts as one). */
  const starts = async () => (await frames()).filter(frame => frame.argv?.includes('stream-json')).length;
  const sent = async () => (await frames()).filter(frame => frame.stdin).map(frame => JSON.parse(frame.stdin));
  return { runtime, root, store, sessions, call, create, until, idle, frames, starts, sent, close };
}
const submit = (h: Awaited<ReturnType<typeof host>>, id: string, request: string, text: string, extra: Record<string, unknown> = {}) =>
  h.call({ op: 'submit', thread_id: id, request_id: request, text, ...extra });
/** Keeps the harness list from looking for a real Codex. */
function noCodex(runtime: Runtime) {
  runtime.codex = new CodexHarness(runtime.store, {
    environment: async () => ({ PATH: '/nonexistent' }), find: async () => ({ state: 'not_installed', message: 'Codex was not found.' }),
  });
}
const users = (snapshot: Snapshot) => snapshot.messages.filter(message => message.role === 'user').map(message => message.id);

test('before setup Claude Code is listed as needing it, refuses threads, and starts nothing', async t => {
  const { root, base } = await setup(t);
  const store = join(base, 'store');
  await mkdir(store, { recursive: true });
  const runtime = new Runtime(store, createModels(), new Set());
  noCodex(runtime);
  t.after(() => runtime.close());
  let found = 0;
  runtime.claude = new ClaudeHarness(runtime.store, { sdk: sdkDirectory(store), cwd: store, find: async () => { found++; return { state: 'ready', path: FAKE_CLAUDE, version: '2.1.326', message: 'ready' }; } });
  const listed = await runtime.reply({ op: 'harnesses', protocol_version: 4 }) as Reply;
  const claude = listed.result.harnesses.find((entry: { id: string }) => entry.id === 'claude');
  assert.deepEqual(listed.result.harnesses.map((entry: { id: string }) => entry.id), ['pi', 'codex', 'claude']);
  assert.equal(claude.state, 'setup_required');
  assert.match(claude.note, /Agent SDK 0\.3\.326/);
  assert.equal(claude.default_mode, 'auto');
  assert.deepEqual(claude.modes.map((mode: { id: string }) => mode.id), ['ask', 'accept_edits', 'plan', 'auto', 'bypass']);
  assert.deepEqual([claude.version, claude.path, claude.models], [null, null, []]);
  assert.equal(found, 0, 'nothing is looked for before setup');
  const created = await runtime.reply({ op: 'create_thread', protocol_version: 4, checkout: root, provider: 'claude', model_id: 'default', request_id: 'c1', harness: 'claude' }) as Reply;
  assert.equal(created.error?.code, 'model_unavailable');
  assert.match(created.error!.message, /Set up Claude Code threads/);
});

test('set_up_harness downloads once through the given fetch, then the normal checks decide', async t => {
  const { base } = await setup(t);
  const store = join(base, 'store');
  await mkdir(store, { recursive: true });
  const runtime = new Runtime(store, createModels(), new Set());
  noCodex(runtime);
  t.after(() => runtime.close());
  // An archive with the dev copy's module, so the pinned module hash holds; the tarball hash is this archive's own.
  const tar = (name: string, data: Buffer) => {
    const header = Buffer.alloc(512);
    header.write(name, 0); header.write('0000644\0', 100); header.write(`${data.length.toString(8).padStart(11, '0')}\0`, 124);
    header.write('0', 156); header.write('ustar\0', 257); header.write('00', 263); header.fill(0x20, 148, 156);
    let sum = 0; for (const byte of header) sum += byte;
    header.write(`${sum.toString(8).padStart(6, '0')}\0 `, 148);
    return Buffer.concat([header, data, Buffer.alloc((512 - (data.length % 512)) % 512)]);
  };
  const archive = gzipSync(Buffer.concat([tar('package/sdk.mjs', readFileSync(join(DEV_SDK, 'sdk.mjs'))), tar('package/LICENSE.md', Buffer.from('License')), Buffer.alloc(1024)]));
  let downloads = 0;
  const harness = (integrity: string) => new ClaudeHarness(runtime.store, {
    sdk: sdkDirectory(store), cwd: store, environment: async () => fakeEnv(),
    find: async () => ({ state: 'ready', path: FAKE_CLAUDE, version: '2.1.326', message: 'ready' }),
    fetch: (async () => { downloads++; return new Response(new Uint8Array(archive)); }) as unknown as typeof fetch,
    tarball: { url: 'https://registry.invalid/sdk.tgz', integrity },
  });
  const claudeOf = (reply: Reply) => reply.result.harnesses.find((entry: { id: string }) => entry.id === 'claude');
  // A mismatched download installs nothing and says so.
  runtime.claude = harness(`sha512-${createHash('sha512').update('other').digest('base64')}`);
  const refused = claudeOf(await runtime.reply({ op: 'set_up_harness', protocol_version: 4, harness: 'claude' }) as Reply);
  assert.equal(refused.state, 'setup_required');
  assert.match(refused.note, /does not match the pinned hash.*Nothing was installed/);
  runtime.claude = harness(`sha512-${createHash('sha512').update(archive).digest('base64')}`);
  const ready = claudeOf(await runtime.reply({ op: 'set_up_harness', protocol_version: 4, harness: 'claude' }) as Reply);
  assert.equal(ready.state, 'ready', JSON.stringify(ready));
  assert.deepEqual(ready.models.map((model: { model_id: string; provider: string }) => [model.provider, model.model_id]),
    [['claude', 'default'], ['claude', 'fake-small'], ['claude', 'fake-budget']]);
  assert.deepEqual(ready.models[0].thinking_levels, ['low', 'medium', 'high', 'xhigh', 'max']);
  assert.deepEqual(ready.models[2].thinking_levels, ['off', 'low', 'medium', 'high']);
  assert.deepEqual([ready.version, ready.supported], ['2.1.326', { min: '2.1.290', tested: '2.1.326' }]);
  // Already set up: listing or setting up again downloads nothing.
  await runtime.reply({ op: 'set_up_harness', protocol_version: 4, harness: 'claude' });
  await runtime.reply({ op: 'harnesses', protocol_version: 4 });
  assert.equal(downloads, 2);
  for (const harness of ['pi', 'codex']) {
    assert.equal((await runtime.reply({ op: 'set_up_harness', protocol_version: 4, harness }) as Reply).error?.code, 'invalid_request');
  }
});

test('an Ask turn shows its approval, finishes, stores its transcript, and reads back after a restart without Claude Code', async t => {
  const h = await host(t);
  const thread = await h.create('ask');
  assert.deepEqual(thread.harness, { id: 'claude', version: null, mode: 'ask', reported: null });
  assert.equal(thread.thinking_level, 'high');
  assert.equal((await submit(h, thread.id, 'r1', 'List the files')).ok, true);
  const asked = await h.until(thread.id, snapshot => !!snapshot.pending_approval);
  assert.equal(asked.status, 'awaiting_approval');
  assert.deepEqual([asked.pending_approval!.kind, asked.pending_approval!.summary, asked.pending_approval!.tool_id], ['command', 'ls -la', 'toolu_1']);
  assert.equal(asked.harness?.reported, 'ask');
  assert.equal((await h.call({ op: 'approve', thread_id: thread.id, approval_id: asked.pending_approval!.id, allow: true })).ok, true);
  const done = await h.idle(thread.id);
  assert.equal(done.status, 'idle', JSON.stringify(done.error));
  assert.deepEqual(done.harness, { id: 'claude', version: '2.1.326', mode: 'ask', reported: 'ask' });
  assert.deepEqual(users(done), ['request-r1']);
  assert.equal(done.messages.at(-1)?.text, 'Two files.');
  assert.ok(done.messages.some(message => message.reasoning === 'Looking at the files.'));
  assert.deepEqual(done.tools.map(tool => [tool.id, tool.name, tool.status, tool.output]), [['toolu_1', 'command', 'done', 'a.txt\nb.txt']]);
  assert.equal(done.usage?.context_window, 200000);
  assert.equal(done.usage?.input, 1000);
  // What Claude Code was started with: the thread's mode, the effort, and no prompt beyond the person's message.
  const argv: string[] = (await h.frames()).filter(frame => frame.argv?.includes('stream-json')).at(-1).argv;
  assert.ok(argv.includes('--permission-mode=default') && argv.includes('--effort=high'), argv.join(' '));
  const sessionId = argv.find(arg => arg.startsWith('--session-id='));
  assert.ok(sessionId, 'a new session is started under a known ID');
  const history = await h.call({ op: 'history', thread_id: thread.id });
  assert.equal(history.ok, true);
  assert.ok(history.result.items.some((item: { id: string }) => item.id === 'request-r1'));
  const started = await h.starts();
  await h.close();
  const again = await host(t, 'turn', { store: h.store, repo: h.root });
  const reread = await again.call({ op: 'history', thread_id: thread.id });
  assert.deepEqual(reread.result.items, history.result.items);
  assert.equal(await again.starts(), started, 'reading history starts no Claude Code');
});

test('Auto, the default, never asks; a second run resumes the same Claude session', async t => {
  const h = await host(t);
  const thread = await h.create();
  assert.equal(thread.harness?.mode, 'auto');
  await submit(h, thread.id, 'r1', 'First');
  const first = await h.idle(thread.id);
  assert.equal(first.status, 'idle', JSON.stringify(first.error));
  assert.equal(first.pending_approval, null);
  assert.equal(first.harness?.reported, 'auto');
  assert.ok((await h.frames()).some(frame => frame.observed === 'unasked' && frame.mode === 'auto'));
  await submit(h, thread.id, 'r2', 'Second');
  const second = await h.idle(thread.id);
  assert.equal(second.status, 'idle', JSON.stringify(second.error));
  assert.deepEqual(users(second), ['request-r1', 'request-r2']);
  const runs = (await h.frames()).filter(frame => frame.argv?.includes('stream-json') && frame.argv.some((arg: string) => arg.startsWith('--permission-mode=auto')));
  const created = runs[0].argv.find((arg: string) => arg.startsWith('--session-id=')).slice('--session-id='.length);
  assert.ok(runs[1].argv.includes(`--resume=${created}`));
  assert.ok(!runs[1].argv.some((arg: string) => arg.startsWith('--session-id')));
});

test('a mode change during a run reaches Claude Code at once, and reported follows', async t => {
  const h = await host(t, 'hold');
  const thread = await h.create();
  await submit(h, thread.id, 'r1', 'List the files');
  await h.until(thread.id, snapshot => snapshot.harness?.reported === 'auto');
  const changed = await h.call({ op: 'configure_thread', thread_id: thread.id, mode: 'ask' });
  assert.equal(changed.ok, true, JSON.stringify(changed));
  assert.deepEqual([changed.result.thread.harness.mode, changed.result.thread.harness.reported], ['ask', null]);
  // Claude Code now asks for the call it was about to make in Auto.
  const asked = await h.until(thread.id, snapshot => !!snapshot.pending_approval);
  assert.equal(asked.harness?.reported, 'ask');
  assert.ok((await h.sent()).some(frame => frame.request?.subtype === 'set_permission_mode' && frame.request.mode === 'default'));
  assert.equal((await h.call({ op: 'configure_thread', thread_id: thread.id, mode: 'plan', thinking: 'low' })).error?.code, 'busy');
  await h.call({ op: 'approve', thread_id: thread.id, approval_id: asked.pending_approval!.id, allow: true });
  const done = await h.idle(thread.id);
  assert.equal(done.status, 'idle', JSON.stringify(done.error));
  // The default mode is a host setting; Pi threads have none.
  const listed = await h.call({ op: 'set_harness_mode', harness: 'claude', mode: 'accept_edits' });
  assert.equal(listed.result.harnesses.find((entry: { id: string }) => entry.id === 'claude').default_mode, 'accept_edits');
  assert.equal((await h.create()).harness?.mode, 'accept_edits');
  assert.equal((await h.call({ op: 'set_harness_mode', harness: 'claude', mode: 'full_access' })).error?.code, 'invalid_request');
  assert.equal((await h.call({ op: 'configure_thread', thread_id: thread.id, mode: 'full_access' })).error?.code, 'invalid_request');
});

test('a steering message joins the running turn and is placed once', async t => {
  const h = await host(t);
  const thread = await h.create('ask');
  await submit(h, thread.id, 'r1', 'List the files');
  const asked = await h.until(thread.id, snapshot => !!snapshot.pending_approval);
  assert.equal((await submit(h, thread.id, 'r2', 'Also count them', { when_busy: 'steer' })).ok, true);
  // Sent to Claude Code at once, so it can no longer be withdrawn.
  await h.until(thread.id, () => h.runtime.sessions.get(thread.id)?.steered.has('r2') === true);
  assert.equal((await h.call({ op: 'cancel_queued', thread_id: thread.id, request_id: 'r2' })).error?.code, 'not_found');
  await h.call({ op: 'approve', thread_id: thread.id, approval_id: asked.pending_approval!.id, allow: true });
  const done = await h.idle(thread.id);
  assert.equal(done.status, 'idle', JSON.stringify(done.error));
  assert.deepEqual(users(done), ['request-r1', 'request-r2']);
  assert.deepEqual(done.queued, []);
  assert.equal(done.messages.at(-1)?.text, 'Two files, and noted.');
  assert.equal(await h.starts(), 2, 'the model check, then one process for both messages');
});

test('Stop ends a running turn as stopped and cancels what waits behind it', async t => {
  const h = await host(t, 'slow');
  const thread = await h.create();
  await submit(h, thread.id, 'r1', 'Slow work');
  await h.until(thread.id, snapshot => snapshot.tools.some(tool => tool.status === 'done'));
  await submit(h, thread.id, 'r2', 'Then more', { when_busy: 'steer' });
  const stopped = await h.call({ op: 'stop', thread_id: thread.id });
  assert.equal(stopped.result.thread.status, 'stopped');
  const done = await h.idle(thread.id);
  assert.equal(done.status, 'stopped');
  assert.deepEqual(done.queued, []);
  assert.deepEqual(users(done), ['request-r1']);
  assert.ok((await h.sent()).some(frame => frame.request?.subtype === 'interrupt' && frame.request.cancel_queued === true));
  // Nothing runs after Stop.
  await new Promise(resolve => setTimeout(resolve, 100));
  assert.equal(h.runtime.sessions.has(thread.id), false);
});

test('an edit in Accept edits runs without asking and lists the file as changed', async t => {
  const h = await host(t, 'edit');
  const thread = await h.create('accept_edits');
  await submit(h, thread.id, 'r1', 'Fix a.txt');
  const done = await h.idle(thread.id);
  assert.equal(done.status, 'idle', JSON.stringify(done.error));
  assert.equal(done.tools[0].name, 'edit');
  assert.deepEqual((await h.call({ op: 'changed_files', thread_id: thread.id })).result.files, ['a.txt']);
});

test('Claude Code exiting mid-turn fails the run with a fixed message; a lost session asks for a new thread', async t => {
  const h = await host(t, 'exit-mid-turn');
  const thread = await h.create();
  await submit(h, thread.id, 'r1', 'List the files');
  const failed = await h.idle(thread.id);
  assert.equal(failed.status, 'error');
  assert.deepEqual(failed.error, { code: 'runtime_error', message: 'Claude Code stopped unexpectedly. Inspect the checkout before a new submission.' });
  assert.ok(failed.tools.every(tool => tool.status !== 'running'));
  // The fake forgets every session, as if Claude Code's files were removed.
  await rm(h.sessions, { recursive: true, force: true }); await mkdir(h.sessions);
  await submit(h, thread.id, 'r2', 'Again');
  const lost = await h.idle(thread.id);
  assert.deepEqual(lost.error, { code: 'runtime_error', message: 'Claude Code could not continue this thread. Its session may have been removed; start a new thread.' });
});

test('a model Claude Code no longer offers, or a signed-out Claude Code, is refused', async t => {
  const h = await host(t, 'signed-out');
  const entry = (await h.call({ op: 'harnesses' })).result.harnesses.find((candidate: { id: string }) => candidate.id === 'claude');
  assert.equal(entry.state, 'signed_out');
  assert.match(entry.note, /Sign in from Settings → Agents, or run `claude auth login`/);
  const refused = await h.call({ op: 'create_thread', checkout: h.root, provider: 'claude', model_id: 'default', request_id: 'c1', harness: 'claude' });
  assert.equal(refused.error?.code, 'model_unavailable');
  const ok = await host(t);
  assert.equal((await ok.call({ op: 'create_thread', checkout: ok.root, provider: 'claude', model_id: 'gone', request_id: 'c2', harness: 'claude' })).error?.code, 'model_unavailable');
  // A model from another harness is refused, as is a mode for Pi.
  assert.equal((await ok.call({ op: 'create_thread', checkout: ok.root, provider: 'codex', model_id: 'gpt', request_id: 'c3', harness: 'claude' })).error?.code, 'invalid_request');
  assert.equal((await ok.call({ op: 'create_thread', checkout: ok.root, provider: 'claude', model_id: 'default', request_id: 'c4', harness: 'claude', mode: 'full_access' })).error?.code, 'invalid_request');
});

test('a Claude Code thread refuses to run once the SDK is gone, at once when a check says so', async t => {
  const h = await host(t);
  const thread = await h.create();
  // As if the person cleared the host's data: the set-up copy is missing.
  h.runtime.claude = new ClaudeHarness(h.runtime.store, { sdk: sdkDirectory(h.store), cwd: h.store, environment: async () => fakeEnv() });
  // Without a recent check the run finds out as it starts, and starts nothing.
  const before = await h.starts();
  assert.equal((await submit(h, thread.id, 'r1', 'Hello')).ok, true);
  const failed = await h.idle(thread.id);
  assert.equal(failed.error?.code, 'model_unavailable');
  assert.match(failed.error!.message, /Set up Claude Code threads/);
  assert.equal(await h.starts(), before);
  // After a check, the submission itself is refused.
  await h.call({ op: 'harnesses' });
  const refused = await submit(h, thread.id, 'r2', 'Hello again');
  assert.equal(refused.error?.code, 'model_unavailable');
});

for (const reason of ['aborted_tools', 'aborted_streaming']) {
  test(`Claude Code reporting ${reason} holds the queued follow-up`, async t => {
    const h = await host(t, reason);
    const { id } = await h.create('ask');
    assert.ok((await submit(h, id, 'first', 'Start')).ok);
    const asked = await h.until(id, snapshot => !!snapshot.pending_approval);
    assert.ok((await submit(h, id, 'next', 'Continue', { when_busy: 'follow_up' })).ok);
    assert.ok((await h.call({ op: 'approve', thread_id: id, approval_id: asked.pending_approval!.id, allow: true })).ok);
    assert.equal((await h.idle(id)).status, 'interrupted');
    assert.equal(h.runtime.store.waiters.line(h.root).find(waiter => waiter.request_id === 'next')?.state, 'held');
  });
}
