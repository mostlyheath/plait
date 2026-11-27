import assert from 'node:assert/strict';
import { getEventListeners } from 'node:events';
import { test } from 'node:test';
import { InteractiveApproval } from '../src/approval.ts';
import { RunAuthorization } from '../src/authorization.ts';
import type { EffectCall } from '../src/authorization.ts';
import type { Snapshot } from '../src/protocol.ts';

const denied = { code: 'permission_denied' };
const stale = { code: 'not_found' };
const call: EffectCall = {
  toolId: 'tool-1', name: 'edit', arguments: { path: 'notes.txt', old_text: 'before', new_text: 'after 🙂' },
  dirtyPaths: ['user.txt'],
};

function fixture(onSave?: (snapshot: Snapshot) => void) {
  const snapshot: Snapshot = {
    id: 'thread-1', checkout: '/fixture', model: { provider: 'faux', model_id: 'faux-1' },
    status: 'running', messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false,
  };
  const saved: Snapshot[] = [];
  const approvals = new InteractiveApproval(snapshot, () => {
    onSave?.(snapshot);
    saved.push(structuredClone(snapshot));
  });
  return { snapshot, saved, approvals, authorization: new RunAuthorization(approvals) };
}

// A permissive fixture policy must not bypass the run's recovery and Stop guards.
test('run authorization starts disabled and revocation cannot be undone', async () => {
  let decisions = 0;
  const authorization = new RunAuthorization({ async authorize() { decisions++; } });
  await assert.rejects(authorization.authorize(call), denied);
  assert.throws(() => authorization.check(), denied);
  assert.equal(decisions, 0);
  authorization.activate();
  await authorization.authorize(call);
  assert.equal(decisions, 1);
  authorization.revoke(); authorization.revoke();
  assert.equal(authorization.active, false);
  assert.equal(authorization.signal.aborted, true);
  assert.throws(() => authorization.activate(), denied);
  await assert.rejects(authorization.authorize(call), denied);
  assert.equal(decisions, 1);
});

test('policy rejection cannot authorize an effect', async () => {
  const error = new Error('Scripted denial');
  const authorization = new RunAuthorization({ async authorize() { throw error; } });
  authorization.activate();
  await assert.rejects(authorization.authorize(call), value => value === error);
});

test('revocation wins even if a pending policy later allows the call', async () => {
  let finish!: () => void;
  let signal!: AbortSignal;
  const authorization = new RunAuthorization({ authorize(_call, current) {
    signal = current;
    return new Promise<void>(resolve => { finish = resolve; });
  } });
  authorization.activate();
  const pending = authorization.authorize(call);
  const rejected = assert.rejects(pending, denied);
  authorization.revoke();
  assert.equal(signal.aborted, true);
  finish();
  await rejected;
});

test('tool cancellation does not grant permission or revoke another call in the run', async () => {
  const f = fixture(); f.authorization.activate();
  const context = new AbortController();
  const pending = f.authorization.authorize(call, context.signal);
  const id = f.snapshot.pending_approval!.id;
  const rejected = assert.rejects(pending, denied);
  context.abort();
  await rejected;
  assert.equal(f.authorization.active, true);
  assert.equal(f.snapshot.pending_approval, null);
  assert.throws(() => f.approvals.answer(id, true), stale);
  const next = f.authorization.authorize({ ...call, toolId: 'tool-2' });
  f.approvals.answer(f.snapshot.pending_approval!.id, true);
  await next;
});

test('an already cancelled call never reaches the policy', async () => {
  let decisions = 0;
  const authorization = new RunAuthorization({ async authorize() { decisions++; } });
  authorization.activate();
  await assert.rejects(authorization.authorize(call, AbortSignal.abort()), denied);
  assert.equal(decisions, 0);
});

test('interactive approval publishes exact call details and consumes one answer', async () => {
  const f = fixture(); f.authorization.activate();
  const pending = f.authorization.authorize(call);
  const approval = f.snapshot.pending_approval!;
  assert.equal(f.snapshot.status, 'awaiting_approval');
  assert.equal(approval.tool_id, call.toolId);
  assert.equal(approval.kind, 'edit');
  assert.equal(approval.summary, 'Change notes.txt');
  assert.equal(approval.details, JSON.stringify(call.arguments));
  assert.deepEqual(approval.dirty_paths, ['user.txt']);
  assert.match(approval.risk, /Only this exact call is approved/);
  assert.deepEqual(f.saved.map(state => state.status), ['awaiting_approval']);
  assert.throws(() => f.approvals.answer('wrong-id', true), stale);
  assert.equal(f.snapshot.pending_approval, approval);
  f.approvals.answer(approval.id, true);
  assert.equal(f.snapshot.pending_approval, null);
  assert.equal(f.snapshot.status, 'running');
  assert.throws(() => f.approvals.answer(approval.id, true), stale);
  await pending;
  assert.deepEqual(f.saved.map(state => state.status), ['awaiting_approval', 'running']);
});

for (const name of ['write', 'command'] as const) test(`${name} keeps the existing approval shape`, async () => {
  const f = fixture(); f.authorization.activate();
  const args: Record<string, string> = name === 'write' ? { path: 'new.txt', content: 'new' } : { command: 'printf hello' };
  const pending = f.authorization.authorize({ ...call, name, arguments: args });
  const approval = f.snapshot.pending_approval!;
  assert.equal(approval.kind, name === 'command' ? 'command' : 'edit');
  assert.equal(approval.summary, name === 'command' ? 'Run sandboxed command' : 'Change new.txt');
  assert.equal(approval.details, JSON.stringify(args));
  f.approvals.answer(approval.id, true);
  await pending;
});

test('denial clears the prompt without granting permission', async () => {
  const f = fixture(); f.authorization.activate();
  const pending = f.authorization.authorize(call);
  const id = f.snapshot.pending_approval!.id;
  const rejected = assert.rejects(pending, denied);
  f.approvals.answer(id, false);
  await rejected;
  assert.equal(f.snapshot.pending_approval, null);
  assert.equal(f.snapshot.status, 'running');
  assert.throws(() => f.approvals.answer(id, true), stale);
});

for (const status of ['stopped', 'interrupted'] as const) test(`revocation preserves ${status} state`, async () => {
  const f = fixture(); f.authorization.activate();
  const pending = f.authorization.authorize(call);
  const id = f.snapshot.pending_approval!.id;
  const rejected = assert.rejects(pending, denied);
  f.snapshot.status = status;
  f.authorization.revoke();
  await rejected;
  assert.equal(f.snapshot.status, status);
  assert.equal(f.snapshot.pending_approval, null);
  assert.throws(() => f.approvals.answer(id, true), stale);
});

test('Stop immediately after Allow still prevents authorization', async () => {
  const f = fixture(); f.authorization.activate();
  const pending = f.authorization.authorize(call);
  const rejected = assert.rejects(pending, denied);
  f.approvals.answer(f.snapshot.pending_approval!.id, true);
  f.authorization.revoke();
  await rejected;
});

test('concurrent prompts cannot replace the pending decision', async () => {
  const f = fixture(); f.authorization.activate();
  const first = f.authorization.authorize(call);
  const approval = f.snapshot.pending_approval!;
  await assert.rejects(f.authorization.authorize({ ...call, toolId: 'tool-2' }), { code: 'busy' });
  assert.equal(f.snapshot.pending_approval, approval);
  f.approvals.answer(approval.id, true);
  await first;
});

test('approval ownership is not inferred from another run or a copied snapshot', async () => {
  const f = fixture(); f.authorization.activate();
  const pending = f.authorization.authorize(call);
  const id = f.snapshot.pending_approval!.id;
  const other = new InteractiveApproval(structuredClone(f.snapshot), () => {});
  assert.throws(() => other.answer(id, true), stale);
  f.approvals.answer(id, true);
  await pending;
  const next = f.authorization.authorize({ ...call, toolId: 'tool-2' });
  assert.notEqual(f.snapshot.pending_approval!.id, id);
  assert.throws(() => f.approvals.answer(id, true), stale);
  f.approvals.answer(f.snapshot.pending_approval!.id, true);
  await next;
});

for (const outcome of ['allow', 'deny', 'cancel'] as const) test(`${outcome} removes the approval abort listener`, async () => {
  const f = fixture();
  const controller = new AbortController();
  const pending = f.approvals.authorize(call, controller.signal);
  const result = outcome === 'allow' ? pending : assert.rejects(pending, denied);
  assert.equal(getEventListeners(controller.signal, 'abort').length, 1);
  if (outcome === 'cancel') controller.abort();
  else f.approvals.answer(f.snapshot.pending_approval!.id, outcome === 'allow');
  await result;
  assert.equal(getEventListeners(controller.signal, 'abort').length, 0);
});

test('cancellation of a settled call cannot clear the next prompt', async () => {
  const f = fixture();
  const oldContext = new AbortController();
  const first = f.approvals.authorize(call, oldContext.signal);
  f.approvals.answer(f.snapshot.pending_approval!.id, true);
  await first;
  const next = f.approvals.authorize({ ...call, toolId: 'tool-2' }, new AbortController().signal);
  const approval = f.snapshot.pending_approval!;
  oldContext.abort();
  assert.equal(f.snapshot.pending_approval, approval);
  assert.equal(f.snapshot.status, 'awaiting_approval');
  f.approvals.answer(approval.id, true);
  await next;
});

test('cancellation during publication cannot strand a prompt', async () => {
  const controller = new AbortController();
  const f = fixture(snapshot => { if (snapshot.pending_approval) controller.abort(); });
  await assert.rejects(f.approvals.authorize(call, controller.signal), denied);
  assert.equal(f.snapshot.pending_approval, null);
  assert.equal(f.snapshot.status, 'running');
  assert.equal(getEventListeners(controller.signal, 'abort').length, 0);
});

test('an already cancelled interactive request never publishes a prompt', async () => {
  const f = fixture();
  await assert.rejects(f.approvals.authorize(call, AbortSignal.abort()), denied);
  assert.equal(f.saved.length, 0);
  assert.equal(f.snapshot.pending_approval, null);
});

test('failed prompt publication leaves no pending decision or abort listener', async () => {
  const error = new Error('Scripted store failure');
  const f = fixture(() => { throw error; });
  const controller = new AbortController();
  await assert.rejects(f.approvals.authorize(call, controller.signal), value => value === error);
  assert.equal(f.snapshot.pending_approval, null);
  assert.equal(f.snapshot.status, 'running');
  assert.equal(getEventListeners(controller.signal, 'abort').length, 0);
});

for (const outcome of ['allow', 'cancel'] as const) test(`failed ${outcome} persistence rejects the waiter safely`, async () => {
  const error = new Error('Scripted settlement failure');
  const f = fixture(snapshot => { if (!snapshot.pending_approval) throw error; });
  const controller = new AbortController();
  const pending = f.approvals.authorize(call, controller.signal);
  const id = f.snapshot.pending_approval!.id;
  const rejected = assert.rejects(pending, value => value === error);
  if (outcome === 'allow') assert.throws(() => f.approvals.answer(id, true), value => value === error);
  else assert.doesNotThrow(() => controller.abort());
  await rejected;
  assert.equal(f.snapshot.pending_approval, null);
  assert.equal(getEventListeners(controller.signal, 'abort').length, 0);
  assert.throws(() => f.approvals.answer(id, true), stale);
});

for (const kind of ['command', 'mcp'] as const) test(`${kind} approval escapes layout controls while preserving the exact execution value`, async () => {
  const f = fixture(); f.authorization.activate();
  const text = 'safe\u202edelete\u200b\u0085\u2028\u2066\ufeff\u{e0001}';
  const effect: EffectCall = { toolId: 'layout', name: 'command', arguments: { command: text }, dirtyPaths: [],
    ...(kind === 'mcp' ? { mcp: { server: 'docs', tool: 'echo' } } : {}) };
  const pending = f.authorization.authorize(effect);
  assert.equal(f.snapshot.pending_approval!.details, '{"command":"safe\\u202edelete\\u200b\\u0085\\u2028\\u2066\\ufeff\\udb40\\udc01"}');
  assert.equal(effect.arguments.command, text);
  f.approvals.answer(f.snapshot.pending_approval!.id, true); await pending;
});

test('a large dirty edit can be saved and approved within the legacy frame', async () => {
  const { boundSnapshot } = await import('../src/transcript.ts');
  const f = fixture(boundSnapshot);
  const paths = Array.from({ length: 6000 }, (_, i) => i + '雪"'.repeat(40));
  const pending = f.approvals.ask({ id: 'many', tool_id: 'edit', kind: 'edit', summary: 'Edit files',
    details: '', risk: 'Review these edits.', dirty_paths: paths }, new AbortController().signal);
  const shown = f.saved[0].pending_approval!;
  assert.ok(Buffer.byteLength(JSON.stringify(shown.dirty_paths)) <= 32 * 1024);
  assert.ok(shown.dirty_paths.length > 0);
  assert.match(shown.risk, /of 6000/);
  assert.equal(paths.length, 6000);
  f.approvals.answer('many', true);
  assert.equal(await pending, true);
});
