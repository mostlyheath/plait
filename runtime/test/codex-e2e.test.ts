import { fixtureDirectory } from './support/temp.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ConversationView } from '@earendil-works/pi-durable';
import { update } from '../src/transcript.ts';
import type { InputRecord, Snapshot } from '../src/protocol.ts';
import { approvalFor, approvalReply, declineUnsupported } from '../src/codex/approval.ts';
import type { CodexApproval } from '../src/codex/approval.ts';
import { threadOf } from '../src/codex/protocol.ts';
import type { ThreadStartResponse, TurnStartParams, TurnStartResponse } from '../src/codex/protocol.ts';
import { AppServer } from '../src/codex/server.ts';
import { CodexTranscript, isTerminalTurn } from '../src/codex/transcript.ts';
import type { Entry } from '../src/external/entries.ts';
import { FAKE_CODEX, fakeEnv } from './support/fake-codex-env.ts';

/** A minimal stand-in for the runtime's adapter loop: route by thread, answer approvals, persist entries. */
async function session(scenario: string, decide: (approval: CodexApproval) => boolean) {
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-codex-e2e-'));
  const log = join(directory, 'frames.jsonl');
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_SCENARIO: scenario, FAKE_CODEX_LOG: log }), requestTimeoutMs: 5000 });
  const { thread } = await server.request<ThreadStartResponse>('thread/start', { cwd: '/work', approvalPolicy: 'on-request', sandbox: 'read-only' });
  const transcript = new CodexTranscript([], 1);
  const persisted: Entry[] = []; const placed = new Map<string, number>(); const asked: CodexApproval[] = [];
  let ended: ReturnType<typeof isTerminalTurn>; let notified = () => {};
  server.onNotification((method, params) => {
    if (threadOf(params) !== thread.id) return;
    const result = transcript.apply(method, params);
    persisted.push(...result.append);
    for (const { clientId, entry } of result.placed) placed.set(clientId, entry);
    ended = isTerminalTurn(method, params) ?? ended;
    notified();
  });
  server.onRequest(async (method, params) => {
    const approval = approvalFor(method, params, transcript.edit(params.itemId));
    if (approval) { asked.push(approval); return approvalReply(method, decide(approval)); }
    return declineUnsupported(method);
  });
  const until = (check: () => boolean) => new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Timed out waiting for the fake turn')), 5000);
    notified = () => { if (check()) { clearTimeout(timer); resolve(); } };
    notified();
  });
  const render = (inputs: InputRecord[]) => {
    const snapshot: Snapshot = { id: 't', checkout: '/work', model: { provider: 'codex', model_id: 'gpt-fake' }, status: 'running',
      messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false };
    update(snapshot, transcript.view() as unknown as ConversationView, inputs, null);
    snapshot.usage = transcript.usage(null) ?? snapshot.usage;
    return snapshot;
  };
  const start = (text: string, requestId: string) => {
    const params: TurnStartParams = { threadId: thread.id, input: [{ type: 'text', text, text_elements: [] }], clientUserMessageId: requestId, effort: 'low' };
    return server.request<TurnStartResponse>('turn/start', params);
  };
  const close = async () => { await server.close(); await rm(directory, { recursive: true, force: true }); };
  const frames = async () => (await readFile(log, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  return { server, thread, transcript, persisted, placed, asked, until, render, start, close, frames, ended: () => ended };
}

test('an approved command turn ends in a complete snapshot', async () => {
  const run = await session('turn', () => true);
  try {
    const { turn } = await run.start('List the files', 'req-1');
    await run.until(() => run.ended() !== undefined);
    assert.deepEqual(run.ended(), { turnId: turn.id, status: 'completed' });
    assert.equal(run.asked[0].summary, 'ls -la');
    assert.equal(run.asked[0].details, 'In /work\nReason: List files');
    const input: InputRecord = { request_id: 'req-1', text: 'List the files', entry: run.placed.get('req-1')!, anchor: 0 };
    const snapshot = run.render([input]);
    assert.deepEqual(snapshot.messages, [
      { id: 'request-req-1', role: 'user', text: 'List the files' },
      { id: '2-0', role: 'assistant', text: '', reasoning: 'Looking at the files.' },
      { id: '4-0', role: 'assistant', text: 'Two files.' },
    ]);
    assert.deepEqual(snapshot.tools.map(({ name, status, output, after }) => ({ name, status, output, after })),
      [{ name: 'command', status: 'done', output: 'a.txt\nb.txt\n', after: 'request-req-1' }]);
    assert.equal(snapshot.live_text, '');
    assert.equal(snapshot.usage?.context_window, 272000);
    assert.equal(snapshot.usage?.context_used, 1000);
    assert.equal(run.persisted.length, 4);
    const reply = (await run.frames()).find(frame => frame.id === 1000);
    assert.deepEqual(reply.result, { decision: 'accept' });
  } finally {
    await run.close();
  }
});

test('a declined edit is shown with its files and ends as an error tool', async () => {
  const run = await session('edit', () => false);
  try {
    await run.start('Change a.txt', 'req-1');
    await run.until(() => run.ended() !== undefined);
    assert.equal(run.asked[0].summary, 'Edit 1 file');
    assert.deepEqual(run.asked[0].paths, ['/work/a.txt']);
    const [tool] = run.render([]).tools;
    assert.deepEqual({ name: tool.name, status: tool.status, output: tool.output }, { name: 'edit', status: 'error', output: 'Declined.' });
  } finally {
    await run.close();
  }
});

test('a user-input request is refused and the turn continues', async () => {
  const run = await session('user-input', () => true);
  try {
    await run.start('Go', 'req-1');
    await run.until(() => run.ended() !== undefined);
    const observed = (await run.frames()).find(frame => frame.observed === 'user-input');
    assert.deepEqual(observed.answer.result, { answers: {} });
  } finally {
    await run.close();
  }
});

test('a running turn can be steered and interrupted', async () => {
  const run = await session('slow', () => true);
  try {
    const { turn } = await run.start('Start', 'req-1');
    // The command finished; the fake now pauses before its answer.
    await run.until(() => run.render([]).tools[0]?.status === 'done');
    assert.equal(run.transcript.activeTurn(), turn.id);
    const steer = await run.server.request<{ turnId: string }>('turn/steer', {
      threadId: run.thread.id, input: [{ type: 'text', text: 'Also count them', text_elements: [] }], clientUserMessageId: 'req-2', expectedTurnId: turn.id,
    });
    assert.equal(steer.turnId, turn.id);
    await run.until(() => run.placed.has('req-2'));
    await run.server.request('turn/interrupt', { threadId: run.thread.id, turnId: turn.id });
    await run.until(() => run.ended() !== undefined);
    assert.deepEqual(run.ended(), { turnId: turn.id, status: 'interrupted' });
    assert.equal(run.transcript.activeTurn(), null);
    const texts = run.render([]).messages.filter(message => message.role === 'user').map(message => message.text);
    assert.deepEqual(texts, ['Start', 'Also count them']);
  } finally {
    await run.close();
  }
});
