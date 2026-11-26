import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { createPiTurn } from '../src/runner/pi-runner.ts';
import { Runtime } from '../src/runtime.ts';
import type { TurnEvent } from '../src/turn-executor.ts';
import { acquireCheckout } from '../src/safety.ts';
import { exec, setup } from './support/safety.ts';
import type { ScriptedStep } from './support/runner-test-provider.ts';

const detached = process.env.PLAIT_DETACHED_RUNNERS === '1';

function toScriptedSteps(responses: Responses): ScriptedStep[] {
  return responses.map(step => {
    if (typeof step === 'function') throw new Error('Function response steps are not supported in detached script mode');
    if (step.stopReason === 'error') {
      return { stopReason: 'error', errorMessage: step.errorMessage ?? 'SCRIPTED_ERROR' };
    }
    const content = Array.isArray(step.content) ? step.content : [];
    const text = content.filter((part): part is { type: 'text'; text: string } => part.type === 'text').map(part => part.text).join('');
    const tool = content.find((part): part is { type: 'toolCall'; id: string; name: string; arguments: Record<string, string | number> } => part.type === 'toolCall');
    if (tool) return { ...(text ? { text } : {}), tool: { name: tool.name, args: tool.arguments, id: tool.id } };
    return { text };
  });
}

class TestRuntime extends Runtime {
  events: TurnEvent['type'][] = [];
  override createTurnSink(...args: Parameters<Runtime['createTurnSink']>) {
    const sink = super.createTurnSink(...args);
    return new Proxy(sink, { get: (target, key, receiver) => key === 'emit'
      ? (event: TurnEvent) => { this.events.push(event.type); return Reflect.apply(target.emit, target, [event]); }
      : Reflect.get(target, key, receiver) });
  }
  override async resources() { return { skills: [], templates: [], problems: [], mcpServers: [], mcpEntries: new Map() }; }
}
type Responses = Parameters<ReturnType<typeof fauxProvider>['setResponses']>[0];
async function fixture(t: Parameters<typeof setup>[0], responses: Responses, scriptedRunner = detached) {
  const { root, base } = await setup(t);
  await writeFile(join(root, 'source.txt'), 'base\n');
  await exec('/usr/bin/git', ['-C', root, 'add', '.']);
  await exec('/usr/bin/git', ['-C', root, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'base']);
  const store = join(base, 'store'); await mkdir(store, { mode: 0o700 });
  const prevScenario = process.env.PLAIT_TEST_SCENARIO;
  const prevTestTurns = process.env.PLAIT_RUNNER_TEST_TURNS;
  if (scriptedRunner) {
    process.env.PLAIT_TEST_SCENARIO = 'pi-script';
    process.env.PLAIT_RUNNER_TEST_TURNS = '1';
    await writeFile(join(store, 'pi-test-script.json'), JSON.stringify({
      steps: toScriptedSteps(responses),
      retry: { enabled: false, maxRetries: 0, baseDelayMs: 10 },
    }));
  }
  const models = createModels(); const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses(responses); models.setProvider(faux.provider);
  const runtime = new TestRuntime(store, models, new Set(['faux'])); runtime.retry.enabled = false;
  t.after(async () => {
    if (scriptedRunner) {
      if (prevScenario === undefined) delete process.env.PLAIT_TEST_SCENARIO;
      else process.env.PLAIT_TEST_SCENARIO = prevScenario;
      if (prevTestTurns === undefined) delete process.env.PLAIT_RUNNER_TEST_TURNS;
      else process.env.PLAIT_RUNNER_TEST_TURNS = prevTestTurns;
    }
    await runtime.close();
  });
  const call = async (request: Record<string, unknown>) => {
    const reply: any = await runtime.reply({ protocol_version: 6, ...request });
    assert.equal(reply.ok, true, JSON.stringify(reply)); return reply.result;
  };
  const created = await call({ op: 'create_thread', checkout: root, request_id: 'create', harness: 'pi', provider: 'faux', model_id: 'faux-1' });
  const id: string = created.thread.id;
  const statuses: string[] = []; const notices: string[] = [];
  runtime.subscribe(key => { notices.push(key); if (key === id) statuses.push(runtime.snapshot(id).status); });
  const submit = (request_id: string, text: string, extra = {}) => call({ op: 'submit', thread_id: id, request_id, text, approval: 'auto', ...extra });
  const records = async () => (await call({ op: 'turn_records', protocol_version: 7, thread_id: id })).records as any[];
  const until = async (predicate: () => boolean) => {
    const end = Date.now() + 20000;
    while (!predicate()) { assert.ok(Date.now() < end, JSON.stringify(runtime.snapshot(id))); await new Promise(resolve => setTimeout(resolve, 10)); }
  };
  const settled = async () => { await until(() => !runtime.sessions.has(id) && !runtime.runnerSessions.has(id)); return runtime.snapshot(id); };
  return { root, runtime, id, call, submit, records, until, settled, statuses, notices };
}
const write = (path: string, content: string) => fauxAssistantMessage([fauxToolCall('write', { path, content }, { id: `write-${path}` })], { stopReason: 'toolUse' });

test('plain streamed turn records its transcript and lifecycle', async t => {
  const h = await fixture(t, [fauxAssistantMessage('Hello from Pi.')]);
  assert.deepEqual(await h.records(), []);
  await h.submit('one', 'Hello'); const end = await h.settled();
  assert.equal(end.status, 'idle'); assert.equal(end.error, null);
  assert.deepEqual(end.messages.map(m => [m.role, m.text]), [['user', 'Hello'], ['assistant', 'Hello from Pi.']]);
  const records = await h.records(); assert.equal(records.length, 1); assert.equal(records[0].state, 'ready');
  assert.equal(records[0].outcome, 'finished');
  assert.deepEqual(records[0].request_ids, ['one']); assert.deepEqual(records[0].paths, []);
  assert.ok(h.statuses.includes('running')); assert.equal(h.statuses.at(-1), 'idle'); assert.ok(h.notices.includes('#'));
});

test('steering during approval joins the active run', async t => {
  const h = await fixture(t, [write('new.txt', 'written'), fauxAssistantMessage('Finished.'), fauxAssistantMessage('Steered.')]);
  await h.submit('one', 'Start', { approval: 'ask' });
  await h.until(() => !!h.runtime.snapshot(h.id).pending_approval);
  const pending = h.runtime.snapshot(h.id).pending_approval!;
  assert.equal(h.runtime.snapshot(h.id).status, 'awaiting_approval');
  assert.equal((await h.records())[0].state, 'recording');
  await h.submit('steer', 'Also do this', { when_busy: 'steer' });
  await h.call({ op: 'approve', thread_id: h.id, approval_id: pending.id, allow: true });
  const end = await h.settled(); assert.equal(end.status, 'idle');
  assert.ok(end.messages.some(m => m.role === 'user' && m.text === 'Also do this'));
  const records = await h.records(); assert.equal(records.length, 1); assert.deepEqual(records[0].request_ids, ['one', 'steer']);
  assert.deepEqual(records[0].paths.map((p: any) => p.path), ['new.txt']);
  assert.ok(h.statuses.includes('awaiting_approval')); assert.ok(h.notices.includes('#'));
});

test('write approval allows and denies separate runs', async t => {
  const h = await fixture(t, [write('allowed.txt', 'yes'), fauxAssistantMessage('Allowed.'), write('denied.txt', 'no'), fauxAssistantMessage('Denied.')]);
  for (const [request, allow] of [['allow', true], ['deny', false]] as const) {
    await h.submit(request, request, { approval: 'ask' });
    await h.until(() => !!h.runtime.snapshot(h.id).pending_approval);
    const pending = h.runtime.snapshot(h.id).pending_approval!; assert.equal(pending.kind, 'edit');
    await h.call({ op: 'approve', thread_id: h.id, approval_id: pending.id, allow });
    assert.equal((await h.settled()).status, 'idle');
  }
  assert.equal(await readFile(join(h.root, 'allowed.txt'), 'utf8'), 'yes');
  await assert.rejects(readFile(join(h.root, 'denied.txt')), { code: 'ENOENT' });
  const records = await h.records(); assert.equal(records.length, 2);
  assert.deepEqual(records.map(r => r.paths.map((p: any) => p.path)), [[], ['allowed.txt']]);
  assert.ok(h.runtime.snapshot(h.id).tools.some(tool => tool.name === 'write'));
  assert.ok(h.statuses.includes('awaiting_approval')); assert.ok(h.notices.includes('#'));
});

test('a dirty file remains protected from a write', async t => {
  const h = await fixture(t, [write('source.txt', 'overwrite'), fauxAssistantMessage('Done.')]);
  await writeFile(join(h.root, 'source.txt'), 'my changes\n');
  await h.submit('dirty', 'Write it'); const end = await h.settled();
  assert.equal(end.status, 'idle'); assert.equal(await readFile(join(h.root, 'source.txt'), 'utf8'), 'my changes\n');
  assert.ok(end.tools.some(tool => tool.name === 'write' && /protect|dirty/i.test(tool.output)));
  const records = await h.records(); assert.equal(records.length, 1); assert.deepEqual(records[0].paths, []);
  assert.ok(h.statuses.includes('running')); assert.ok(h.notices.includes('#'));
});

test('provider failure reports a fixed error and captures an unchanged after record', async t => {
  const h = await fixture(t, [fauxAssistantMessage([], { stopReason: 'error', errorMessage: 'SECRET_PROVIDER_DETAIL' })]);
  assert.deepEqual(await h.records(), []); await h.submit('fail', 'Fail'); const end = await h.settled();
  assert.equal(end.status, 'error'); assert.deepEqual(end.error, { code: 'runtime_error', message: 'Agent ended without an answer (model_error). Inspect the transcript before retrying.' });
  assert.ok(!JSON.stringify(end).includes('SECRET_PROVIDER_DETAIL'));
  const records = await h.records(); assert.equal(records.length, 1); assert.equal(records[0].state, 'ready'); assert.deepEqual(records[0].paths, []);
  assert.equal(records[0].outcome, 'failed');
  assert.ok(h.statuses.includes('running')); assert.equal(h.statuses.at(-1), 'error'); assert.ok(h.notices.includes('#'));
});

test('stop during approval settles the run and releases the checkout', async t => {
  const h = await fixture(t, [write('stopped.txt', 'never')]);
  await h.submit('stop', 'Start', { approval: 'ask' }); await h.until(() => !!h.runtime.snapshot(h.id).pending_approval);
  await h.call({ op: 'stop', thread_id: h.id }); const end = await h.settled();
  assert.equal(end.status, 'stopped'); assert.equal(end.pending_approval, null);
  await assert.rejects(readFile(join(h.root, 'stopped.txt')), { code: 'ENOENT' });
  const records = await h.records(); assert.equal(records.length, 1); assert.equal(records[0].state, 'ready'); assert.deepEqual(records[0].paths, []);
  assert.equal(records[0].outcome, 'stopped');
  const lease = await acquireCheckout(h.root); await lease.close();
  assert.ok(h.statuses.includes('awaiting_approval')); assert.equal(h.statuses.at(-1), 'stopped'); assert.ok(h.notices.includes('#'));
});

test('Pi sink preserves event order for plain and approval turns', async t => {
  const plain = await fixture(t, [fauxAssistantMessage('Hello.')]);
  await plain.submit('plain', 'Hello'); const plainEnd = await plain.settled();
  assert.equal(plainEnd.status, 'idle');
  if (!detached) {
    const plainEvents = plain.runtime.events;
    assert.equal(plainEvents[0], 'record_before');
    assert.ok(plainEvents.indexOf('phase') > plainEvents.indexOf('record_before'));
    assert.ok(plainEvents.includes('live'));
    for (let i = 0; i < plainEvents.length; i++) if (plainEvents[i] === 'live') assert.equal(plainEvents[i + 1], 'commit');
    assert.deepEqual(plainEvents.slice(-2), ['record_after', 'finished']);
    assert.ok(!plainEvents.includes('approval_needed'));
  }

  const approval = await fixture(t, [write('approved.txt', 'yes'), fauxAssistantMessage('Done.')]);
  await approval.submit('approval', 'Write', { approval: 'ask' });
  await approval.until(() => !!approval.runtime.snapshot(approval.id).pending_approval);
  const pending = approval.runtime.snapshot(approval.id).pending_approval!;
  await approval.call({ op: 'approve', thread_id: approval.id, approval_id: pending.id, allow: true });
  const approvalEnd = await approval.settled();
  assert.equal(approvalEnd.status, 'idle');
  if (!detached) {
    const events = approval.runtime.events;
    assert.equal(events[0], 'record_before');
    assert.ok(events.indexOf('phase') > events.indexOf('record_before'));
    assert.ok(events.indexOf('approval_needed') > events.indexOf('phase'));
    assert.equal(events.filter(event => event === 'approval_needed').length, 1);
    assert.ok(events.indexOf('approval_settled') > events.indexOf('approval_needed'));
    for (let i = 0; i < events.length; i++) if (events[i] === 'live') assert.equal(events[i + 1], 'commit');
    assert.deepEqual(events.slice(-2), ['record_after', 'finished']);
  }
});

test('a held steer starts separately with its saved approval after a fresh auto run', async t => {
  const h = await fixture(t, [fauxAssistantMessage('Fresh run finished.'), write('held.txt', 'held'), fauxAssistantMessage('Done.')]);
  // Recreate the durable queue left by startup recovery before the fresh submission.
  const snapshot = h.runtime.snapshot(h.id);
  h.runtime.store.enqueue('old', 'Old steer', {}, { mode: 'steer', approval: 'ask' }, snapshot);
  h.runtime.store.waiters.hold(h.id);
  await h.submit('fresh', 'Fresh request');
  await h.until(() => !!h.runtime.snapshot(h.id).pending_approval);
  const followUp = h.runtime.snapshot(h.id);
  assert.equal(followUp.approval_mode, 'ask');
  const pending = followUp.pending_approval!;
  await h.call({ op: 'approve', thread_id: h.id, approval_id: pending.id, allow: false });
  await h.settled();
  assert.deepEqual((await h.records()).map(record => record.request_ids), [['old'], ['fresh']]);
});

test('the detached Pi turn places only steers owned by its run', async t => {
  const h = await fixture(t, [fauxAssistantMessage('Started.'), fauxAssistantMessage('Steered.')], true);
  const snapshot = h.runtime.snapshot(h.id);
  for (const [id, steerRunId] of [['legacy', undefined], ['stale', 'previous'], ['current', 'active']] as const) {
    h.runtime.store.enqueue(id, id, {}, { mode: 'steer', approval: 'ask', steerRunId }, snapshot);
  }
  const lease = await acquireCheckout(h.root, 'writer');
  const placed = new Set<string>();
  try {
    const result = await createPiTurn().start({
      kind: 'pi', thread_id: h.id, request_id: 'fresh', text: 'Fresh request', checkout: h.root, vcs: 'git',
      model: { provider: 'faux', modelId: 'faux-1' }, approval_mode: 'auto', root_identity: lease.identity,
      max_turns: 10, retry: h.runtime.retry,
    }, event => {
      if (event.type === 'commit' && Array.isArray(event.placed)) for (const id of event.placed) placed.add(id);
    }, new AbortController().signal, {
      store: h.runtime.directory, lease,
      manifest: { run_id: 'active', thread_id: h.id, request_id: 'fresh', checkout: h.root, harness: 'pi', build_id: 'test' },
    });
    assert.deepEqual(result, { status: 'idle' });
    assert.deepEqual([...placed], ['current']);
  } finally { await lease.close(); }
});
