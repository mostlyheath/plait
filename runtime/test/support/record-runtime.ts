// Shared scripted harnesses for recording and undo tests. No models are contacted.
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { ClaudeHarness } from '../../src/claude/harness.ts';
import { AppServer } from '../../src/codex/server.ts';
import { CodexHarness } from '../../src/harness.ts';
import type { Snapshot } from '../../src/protocol.ts';
import { Runtime } from '../../src/runtime.ts';
import { DEV_SDK, FAKE_CLAUDE, fakeEnv as claudeEnv } from './fake-claude-env.ts';
import { FAKE_CODEX, fakeEnv as codexEnv } from './fake-codex-env.ts';
import { setup } from './safety.ts';

type Context = { after(fn: () => Promise<void>): void };
type Reply = { ok: boolean; result?: any; error?: { code: string; message: string } };
type Row = Record<string, unknown> & { id: string; kind: string; state: string; run_request: string; before: string | null; paths: string[] };
/** A run's two records, as the store links them. */
export type Pair = { run: string; before: Row; after?: Row };

function rows(db: DatabaseSync): Row[] {
  // One read transaction. The runtime commits a record's state and its paths
  // together; two separate reads could see the new state with the old paths.
  db.exec('BEGIN');
  try {
    const paths = db.prepare('SELECT record,path FROM record_paths ORDER BY path').all();
    return db.prepare('SELECT * FROM records ORDER BY rowid').all().map(row => ({
      ...row, paths: paths.filter(path => path.record === row.id).map(path => String(path.path)),
    }) as Row);
  } finally {
    db.exec('COMMIT');
  }
}
export function pairs(db: DatabaseSync): Pair[] {
  const all = rows(db);
  return all.filter(row => row.kind === 'before').map(before => ({
    run: before.run_request, before, after: all.find(row => row.kind === 'after' && row.before === before.id),
  }));
}
export const models = () => { const models = createModels(); models.setProvider(fauxProvider().provider); return models; };

/** A runtime whose Codex is the fake, writing a file in the checkout as each turn starts. */
export async function codex(t: Context, scenario = 'turn', writeCwd = false) {
  const { root, base } = await setup(t);
  const store = join(base, 'store');
  await mkdir(store, { recursive: true });
  const log = join(store, 'frames.jsonl');
  const open = async () => {
    const runtime = new Runtime(store, models(), new Set(['faux'])); await runtime.recover();
    runtime.codex = new CodexHarness(runtime.store, {
      environment: async () => codexEnv({ FAKE_CODEX_SCENARIO: scenario, FAKE_CODEX_LOG: log, ...(writeCwd ? { FAKE_CODEX_WRITE_CWD: '1' } : { FAKE_CODEX_WRITE: root }) }),
      find: async () => ({ state: 'ready', path: FAKE_CODEX, version: '0.169.0', message: 'ready' }),
      start: (path, env) => AppServer.start(path, { env, requestTimeoutMs: 10_000 }),
      idleMs: 50,
    });
    return harnessed(t, runtime, root, store, { provider: 'codex', model_id: 'gpt-fake', harness: 'codex', mode: 'ask' });
  };
  // Ask mode, so a run waits on its first command until the test allows it.
  const h = await open();
  /** Turns Codex was asked to start. */
  const turns = async () => (await readFile(log, 'utf8').catch(() => '')).trim().split('\n').filter(Boolean)
    .filter(line => JSON.parse(line).method === 'turn/start').length;
  return { ...h, turns, reopen: open };
}

/** A runtime whose Claude Code is the fake, writing a file in the checkout as each turn starts. */
export async function claude(t: Context, scenario = 'turn', mode = 'auto') {
  const { root, base } = await setup(t);
  const store = join(base, 'store');
  const sessions = join(store, 'fake-sessions');
  await mkdir(sessions, { recursive: true });
  const open = async () => {
    const runtime = new Runtime(store, models(), new Set(['faux'])); await runtime.recover();
    runtime.codex = new CodexHarness(runtime.store, {
      environment: async () => ({ PATH: '/nonexistent' }), find: async () => ({ state: 'not_installed', message: 'Codex was not found.' }),
    });
    runtime.claude = new ClaudeHarness(runtime.store, {
      sdk: DEV_SDK, cwd: store,
      environment: async () => claudeEnv({ FAKE_CLAUDE_SCENARIO: scenario, FAKE_CLAUDE_LOG: join(store, 'claude-images.jsonl'), FAKE_CLAUDE_STATE: sessions, FAKE_CLAUDE_WRITE: '1' }),
      find: async () => ({ state: 'ready', path: FAKE_CLAUDE, version: '2.1.326', message: 'ready' }),
      fetch: (async () => { throw new Error('No test downloads anything.'); }) as unknown as typeof fetch,
    });
    return harnessed(t, runtime, root, store, { provider: 'claude', model_id: 'default', harness: 'claude', mode });
  };
  return { ...await open(), reopen: open };
}

function harnessed(t: Context, runtime: Runtime, root: string, store: string, thread: Record<string, string>) {
  let closed = false;
  const close = async () => { if (!closed) { closed = true; await runtime.close(); } };
  t.after(close);
  const call = (request: Record<string, unknown>, protocol = 4) => runtime.reply({ protocol_version: protocol, ...request }) as Promise<Reply>;
  const create = async () => {
    const reply = await call({ op: 'create_thread', checkout: root, request_id: `c${Date.now()}${Math.random()}`.replace('.', ''), thinking: 'high', ...thread });
    assert.equal(reply.ok, true, JSON.stringify(reply));
    return reply.result.thread as Snapshot;
  };
  const until = async (id: string, done: (snapshot: Snapshot) => boolean, ms = 10000) => {
    const deadline = Date.now() + ms;
    for (;;) {
      const snapshot = runtime.snapshot(id);
      if (done(snapshot)) return structuredClone(snapshot);
      if (Date.now() > deadline) assert.fail(`Timed out; last snapshot: ${JSON.stringify(snapshot)}`);
      await new Promise(resolve => setTimeout(resolve, 10));
    }
  };
  const idle = (id: string) => until(id, snapshot => !runtime.sessions.has(id) && !(snapshot.queued ?? []).length && !['running', 'awaiting_approval'].includes(snapshot.status));
  const asked = (id: string) => until(id, snapshot => !!snapshot.pending_approval && (snapshot.queued ?? []).length === 0);
  const approve = async (id: string) => {
    const snapshot = await until(id, snapshot => !!snapshot.pending_approval);
    assert.equal((await call({ op: 'approve', thread_id: id, approval_id: snapshot.pending_approval!.id, allow: true })).ok, true);
  };
  const submit = async (id: string, request: string, extra: Record<string, unknown> = {}) => {
    const reply = await call({ op: 'submit', thread_id: id, request_id: request, text: `Message ${request}`, ...extra });
    assert.equal(reply.ok, true, JSON.stringify(reply));
  };
  /** Allows whatever the run asks until it ends. */
  const finish = async (id: string) => {
    for (;;) {
      const snapshot = await until(id, snapshot => !!snapshot.pending_approval || (!runtime.sessions.has(id) && !(snapshot.queued ?? []).length && !['running', 'awaiting_approval'].includes(snapshot.status)));
      if (!snapshot.pending_approval) return snapshot;
      await call({ op: 'approve', thread_id: id, approval_id: snapshot.pending_approval.id, allow: true });
    }
  };
  return { runtime, root, store, call, create, until, idle, asked, approve, submit, finish, close, pairs: () => pairs(runtime.store.db) };
}
