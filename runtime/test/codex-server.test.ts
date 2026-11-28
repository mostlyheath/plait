import { fixtureDirectory } from './support/temp.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AppServer, CodexConnectionError, CodexRpcError } from '../src/codex/server.ts';
import { FAKE_CODEX, fakeEnv } from './support/fake-codex-env.ts';

async function withLog(run: (log: string) => Promise<void>) {
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-codex-server-'));
  try { await run(join(directory, 'frames.jsonl')); } finally { await rm(directory, { recursive: true, force: true }); }
}
const frames = async (log: string) => (await readFile(log, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
const until = async (check: () => boolean | Promise<boolean>, what: string) => {
  for (let tries = 0; tries < 200; tries++) { if (await check()) return; await new Promise(resolve => setTimeout(resolve, 10)); }
  assert.fail(`Timed out waiting for ${what}`);
};

test('start sends initialize, then the initialized notification', () => withLog(async log => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_LOG: log }), clientVersion: '1.2.3' });
  await server.close();
  const [initialize, initialized] = await frames(log);
  assert.equal(initialize.method, 'initialize');
  assert.deepEqual(initialize.params, {
    clientInfo: { name: 'plait', title: 'Plait', version: '1.2.3' }, capabilities: { experimentalApi: false, requestAttestation: false },
  });
  assert.deepEqual(initialized, { method: 'initialized' });
}));

test('requests resolve with results and reject with JSON-RPC errors', async () => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv() });
  try {
    const started = await server.request<{ thread: { id: string } }>('thread/start', { cwd: '/work' });
    assert.equal(started.thread.id, 'thread-fake-1');
    const error = await server.request('thread/resume', { threadId: 'missing' }).catch(error => error);
    assert.ok(error instanceof CodexRpcError);
    assert.equal(error.code, -32600);
    assert.match(error.message, /no rollout found/);
    // Several requests in flight are matched by id.
    const [a, b] = await Promise.all([server.request<{ data: unknown[] }>('model/list', {}), server.request<{ account: unknown }>('account/read', {})]);
    assert.equal(a.data.length, 1);
    assert.deepEqual(b.account, { type: 'apiKey' });
  } finally {
    await server.close();
  }
});

test('notifications reach every handler until it unsubscribes', async () => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv() });
  try {
    const seen: string[] = []; const other: string[] = [];
    const stop = server.onNotification(method => seen.push(method));
    server.onNotification(method => other.push(method));
    await server.request('thread/start', {});
    await until(() => seen.includes('thread/started'), 'thread/started');
    stop();
    await server.request('thread/start', {});
    await until(() => other.filter(method => method === 'thread/started').length === 2, 'second thread/started');
    assert.equal(seen.filter(method => method === 'thread/started').length, 1);
  } finally {
    await server.close();
  }
});

test('server requests are answered by the handler, or refused without one', () => withLog(async log => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_LOG: log }) });
  try {
    const asked: string[] = [];
    let completed = false;
    server.onNotification(method => { if (method === 'turn/completed') completed = true; });
    server.onRequest(async (method, params) => {
      asked.push(`${method}:${params.itemId}`);
      return { decision: 'decline' };
    });
    await server.request('thread/start', {});
    await server.request('turn/start', { threadId: 'thread-fake-1', input: [{ type: 'text', text: 'hi', text_elements: [] }] });
    await until(() => completed, 'turn/completed');
    assert.deepEqual(asked, ['item/commandExecution/requestApproval:cmd-1']);
    const reply = (await frames(log)).find(frame => frame.id === 1000);
    assert.deepEqual(reply, { id: 1000, result: { decision: 'decline' } });
  } finally {
    await server.close();
  }
  await withLog(async second => {
    const bare = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_LOG: second }) });
    let completed = false;
    bare.onNotification(method => { if (method === 'turn/completed') completed = true; });
    await bare.request('turn/start', { threadId: 'thread-fake-1', input: [] });
    await until(() => completed, 'turn/completed');
    await bare.close();
    const refusal = (await frames(second)).find(frame => frame.id === 1000);
    assert.equal(refusal.error.code, -32601);
  });
}));

test('a handler that throws answers with an error', () => withLog(async log => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_LOG: log }) });
  let completed = false;
  server.onNotification(method => { if (method === 'turn/completed') completed = true; });
  server.onRequest(async () => { throw new Error('No approval is possible now.'); });
  await server.request('turn/start', { threadId: 'thread-fake-1', input: [] });
  await until(() => completed, 'turn/completed');
  await server.close();
  const reply = (await frames(log)).find(frame => frame.id === 1000);
  assert.deepEqual(reply.error, { code: -32603, message: 'No approval is possible now.' });
}));

test('the process exiting fails pending and later requests', async () => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_SCENARIO: 'exit-on-turn' }) });
  const pending = server.request('turn/start', { threadId: 'thread-fake-1', input: [] });
  await assert.rejects(pending, (error: Error) => error instanceof CodexConnectionError && /exited \(code 3\)/.test(error.message));
  assert.deepEqual(await server.exited, { code: 3, signal: null });
  await assert.rejects(server.request('model/list', {}), CodexConnectionError);
  await server.close();
});

test('an oversized line closes the connection', async () => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_SCENARIO: 'oversized' }) });
  await assert.rejects(server.request('model/list', {}), /larger than 8 MiB/);
  assert.equal((await server.exited).signal, 'SIGKILL');
});

test('an aborted or timed-out request stops waiting', async () => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv(), requestTimeoutMs: 50 });
  try {
    const controller = new AbortController();
    controller.abort();
    await assert.rejects(server.request('model/list', {}, controller.signal), /Stopped waiting for model\/list/);
    // The fake never answers this method, so only the timeout ends the wait.
    await assert.rejects(server.request('fake/hang', {}), /did not answer fake\/hang in time/);
    assert.equal((await server.request<{ data: unknown[] }>('model/list', {})).data.length, 1);
  } finally {
    await server.close();
  }
});

test('start fails clearly when the binary cannot run', async () => {
  await assert.rejects(AppServer.start('/nonexistent/codex', { env: fakeEnv() }), /Could not start Codex: ENOENT/);
});

test('diagnostics keep only the last stderr lines', async () => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_SCENARIO: 'stderr' }) });
  await until(() => server.diagnostics().includes('line 39'), 'stderr');
  const lines = server.diagnostics().split('\n');
  assert.equal(lines.length, 10);
  assert.equal(lines[0], 'line 30');
  await server.close();
  assert.equal((await server.exited).code, 0);
});
