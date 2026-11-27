import { fixtureDirectorySync } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { existsSync, mkdirSync, realpathSync, symlinkSync, renameSync, rmSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage } from '@earendil-works/pi-ai/providers/faux';
import { setTimeout as delay } from 'node:timers/promises';
import { exec, setup } from './support/safety.ts';
import { previousRuntime } from './support/previous-runtime.ts';
import { Automations } from '../src/automations/service.ts';
import { HostError } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';

const definition = {
  name: 'Review', instructions: 'Review recent changes', target: { kind: 'directory', path: '/project' },
  harness: 'pi', provider: 'faux', model_id: 'faux-1', approval: 'ask', command_policy: 'trusted',
  schedule: { kind: 'hourly', minute: 0, timeZone: 'UTC' }
};
function directory(t: TestContext) {
  const dir = realpathSync(fixtureDirectorySync(join(tmpdir(), 'automations-')));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
async function client(t: TestContext, respond: (r: Record<string, unknown>) => unknown = () => undefined, settings: { catch_up_ms?: number } & Omit<typeof definition, 'target'> & { target: { kind: string; path?: string; checkout?: string } } = definition) {
  const dir = directory(t), requests: Record<string, unknown>[] = [];
  const send = async (r: Record<string, unknown>) => {
    requests.push(r);
    const scripted = respond(r);
    if (scripted !== undefined) return scripted;
    switch (r.op) {
      case 'create_admitted_thread': return { admission: { state: 'ready', thread_id: requests.filter(request => request.op === 'create_admitted_thread').length === 1 ? 'thread' : 'second' } };
      case 'stored_thread': return { thread: { checkout: '/project', model: { provider: 'faux', model_id: 'faux-1' }, approval_mode: 'ask' } };
      case 'admission_info': return { busy: false };
      case 'checkout_queue': return { waiters: [] };
      case 'command_policy': return { policy: 'trusted' };
      case 'submit_checkout': return { accepted: true };
      default: throw Error('Unexpected ' + r.op);
    }
  };
  const accepted = new Set<string>();
  const hasReceipt = (_thread: string, request: string) => accepted.has(request);
  let service = new Automations(dir, send, hasReceipt);
  t.after(() => service.close());
  const handle = (op: string, fields = {}): Promise<any> => service.handle({ op, ...fields });
  const { automation } = await handle('create_automation', { definition: settings.target.kind === 'directory' ? { ...settings, target: { kind: 'directory', path: dir } } : settings, request_id: 'create' });
  await Promise.all(service['placements'].values());
  const due = (Math.floor(automation.cursor / 3600000) + 1) * 3600000;
  return {
    dir, accepted, checkSubmission: (request: Record<string, unknown>) => service.checkSubmission(String(request.thread_id),
      (request.expect as { claim: { id: string; revision: number } }).claim, String(request.approval)),
    requests, due, created: automation, id: automation.id, start: () => service.start(), tick: (now: number) => service.tick(now), handle,
    restart: async (Service = Automations) => { await service.close(); service = new Service(dir, send, hasReceipt); }
  };
}
test('lost submit reply retries the same host key after restart and catch-up runs once', async t => {
  let lost = true;
  const c = await client(t, r => { if (r.op === 'submit_checkout' && lost) { lost = false; throw Error('lost reply'); } });
  const now = c.due + 3 * 3600000;
  await assert.rejects(c.tick(now), /lost reply/);
  await c.restart();
  await c.tick(now); await c.tick(now);
  const submits = c.requests.filter(r => r.op === 'submit_checkout');
  assert.equal(submits.length, 2);
  assert.deepEqual(submits[0], submits[1]);
  assert.equal(submits[0].request_id, c.id + '-' + now);
  assert.ok(!('when_busy' in submits[0]));
  const { occurrences } = await c.handle('get_automation', { id: c.id });
  assert.deepEqual(occurrences.map((o: any) => o.outcome), ['queued', 'missed', 'missed', 'missed']);
});
test('restart reconciles an accepted claim even after its automation was edited', async t => {
  const c = await client(t, request => {
    if (request.op === 'submit_checkout') {
      c.accepted.add(String(request.request_id));
      throw new Error('lost reply');
    }
  });
  await assert.rejects(c.tick(c.due), /lost reply/);
  await c.handle('pause_automation', { id: c.id });
  await c.restart();
  await c.tick(c.due);
  const { occurrences } = await c.handle('get_automation', { id: c.id });
  assert.equal(occurrences[0].outcome, 'queued');
  assert.equal(c.requests.filter(request => request.op === 'submit_checkout').length, 1);
});
for (const [code, outcome] of [['busy', 'skipped'], ['checkout_missing', 'paused'], ['permission_denied', 'paused']]) {
  test('ordinary submit refusal ' + code, async t => {
    const c = await client(t, r => { if (r.op === 'submit_checkout') throw new HostError(code, 'Checkout root identity changed.'); });
    await c.tick(c.due);
    const { automation, occurrences } = await c.handle('get_automation', { id: c.id });
    assert.equal(occurrences[0].outcome, outcome);
    assert.equal(automation.pause_reason, outcome === 'paused' ? 'target missing' : null);
  });
}
test('policy drift pauses before submit, recursion is refused, deletion retains the thread', async t => {
  const c = await client(t, r => r.op === 'command_policy' ? { policy: 'sandboxed' } : undefined);
  await assert.rejects(c.handle('create_automation', { definition, request_id: 'new', caller: { thread_id: 'thread' } }), /cannot create/);
  await c.tick(c.due);
  const { automation } = await c.handle('get_automation', { id: c.id });
  assert.equal(automation.pause_reason, 'Settings changed outside the automation');
  await c.handle('delete_automation', { id: c.id });
  await c.restart(); await c.tick(c.due + 3600000);
  assert.deepEqual(await c.handle('list_automations'), { automations: [] });
  assert.deepEqual(c.requests.map(r => r.op), ['admission_info', 'create_admitted_thread', 'stored_thread', 'admission_info', 'command_policy']);
});
test('the startup settling delay counts against the catch-up window', async t => {
  t.mock.timers.enable({apis: ['Date','setTimeout'], now: Date.UTC(2025, 0, 1, 9, 59)});
  const c = await client(t);
  t.mock.timers.setTime(c.due + 30 * 60_000 - 1000);
  c.start();
  t.mock.timers.tick(59_999);
  let info = await c.handle('get_automation',{id:c.id});
  assert.equal(info.occurrences.length,0);
  t.mock.timers.tick(1);
  await new Promise<void>(resolve => setImmediate(resolve));
  info = await c.handle('get_automation',{id:c.id});
  assert.equal(info.occurrences[0].outcome,'missed');
  assert.equal(c.requests.filter(r => r.op === 'submit_checkout').length,0);
});
test('each thread setting independently prevents a drifted submit', async t => {
  const thread = {model:{provider:'faux',model_id:'faux-1'}, harness:{id:'pi',mode:null}, checkout:'/project',approval_mode:'ask'};
  for (const changed of [
    {model:{...thread.model,provider:'other'}}, {model:{...thread.model,model_id:'other'}},
    {harness:{...thread.harness,id:'codex'}}, {harness:{...thread.harness,mode:'other'}},
  ]) {
    const c = await client(t,r => r.op === 'stored_thread' ? {thread:{...thread,...changed}} : undefined);
    await c.tick(c.due);
    const {automation} = await c.handle('get_automation',{id:c.id});
    assert.equal(automation.state,'paused');
    assert.equal(automation.pause_reason,'Settings changed outside the automation');
    assert.ok(!c.requests.some(r => r.op === 'submit_checkout'));
  }
});
test('an edit before the host handles a scheduled submission invalidates its claim', async t => {
  const c = await client(t, request => {
    if (request.op !== 'submit_checkout') return;
    return (async () => {
      await c.handle('pause_automation', { id: c.id });
      c.checkSubmission(request);
      assert.fail('stale submission passed its revision check');
    })();
  });
  await c.tick(c.due);
  const { occurrences } = await c.handle('get_automation', { id: c.id });
  assert.equal(occurrences[0].detail, 'Automation changed before submission.');
});

test('real Runtime gates routing and storage on the flag', async t => {
  const dir = directory(t);
  const old = process.env.PLAIT_AUTOMATIONS;
  try {
    delete process.env.PLAIT_AUTOMATIONS;
    const runtime = new Runtime(dir, createModels(), new Set());
    try {
      await assert.rejects(runtime.handle({ op: 'list_automations' }), /Unknown operation/);
      await assert.rejects(runtime.handle({ op: 'admission_info', checkout: dir, poll_id: 'scheduled' }), /admission field/);
      assert.ok(!existsSync(join(dir, 'automations.sqlite')));
    } finally { await runtime.close(); }
    process.env.PLAIT_AUTOMATIONS = '1';
    const enabled = new Runtime(dir, createModels(), new Set());
    await assert.rejects(enabled.handle({ op: 'list_automations' }), /Unknown operation/);
    enabled.startBackground(request => enabled.handle(request));
    try { assert.deepEqual(await enabled.handle({ op: 'list_automations' }), { automations: [] }); }
    finally { await enabled.close(); }
  } finally { if (old === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = old; }
});

test('real placement creates a thread, Run now submits to it, and removal pauses the schedule', async t => {
  const dir = directory(t);
  const real = join(dir, 'folder'), store = join(dir, 'store');
  mkdirSync(real); mkdirSync(store);
  // Reach the folder through a link, as macOS reaches /private/var through /var.
  const path = join(dir, 'link'); symlinkSync(real, path);
  const old = process.env.PLAIT_AUTOMATIONS;
  process.env.PLAIT_AUTOMATIONS = '1';
  const models = createModels(), faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('Complete')]);
  models.setProvider(faux.provider);
  const runtime = new Runtime(store, models, new Set(['faux']));
  runtime.startBackground(async request => {
    const reply = await runtime.reply(request);
    if (reply.error) throw new HostError(reply.error.code, reply.error.message);
    return reply.result;
  });
  try {
    const { automation } = await runtime.handle({
      op: 'create_automation', request_id: 'create',
      definition: { ...definition, approval: 'auto', target: { kind: 'directory', path } }
    }) as { automation: { id: string; thread_id: string } };
    const run = async (request_id: string) => {
      const { occurrence } = await runtime.handle({ op: 'run_automation_now', id: automation.id, request_id }) as { occurrence: { request_id: string } };
      for (let attempt = 0; attempt < 500; attempt++) {
        const { occurrences } = await runtime.handle({ op: 'get_automation', id: automation.id }) as { occurrences: { request_id: string; outcome: string; detail: string }[] };
        const receipt = occurrences.find(row => row.request_id === occurrence.request_id);
        if (receipt) return { occurrence: receipt };
        await delay(10);
      }
      throw Error('Timed out waiting for the occurrence receipt');
    };
    for (let attempts = 0; ; attempts++) {
      const result = await runtime.handle({ op: 'get_automation', id: automation.id }) as { automation: typeof automation & { state: string; error?: string | null } };
      if (result.automation.state === 'active') { automation.thread_id = result.automation.thread_id; break; }
      assert.equal(result.automation.state, 'preparing', `Placement failed: ${result.automation.error}`);
      assert.ok(attempts < 500);
      await delay(10);
    }
    const started = await run('now');
    assert.equal(started.occurrence.outcome, 'queued', started.occurrence.detail);
    const until = Date.now() + 10000;
    while (!runtime.snapshot(automation.thread_id).messages.some(m => m.text === 'Complete') || runtime.snapshot(automation.thread_id).status === 'running') {
      assert.ok(Date.now() < until, 'Timed out waiting for ordinary turn completion');
      await delay(10);
    }
    const thread = runtime.snapshot(automation.thread_id);
    assert.ok(thread.messages.some(m => m.text === 'Complete'));

    renameSync(real, join(dir, 'moved'));
    assert.equal((await run('later')).occurrence.detail, 'target missing');
    await runtime.handle({ op: 'delete_automation', id: automation.id });
    assert.equal(runtime.snapshot(thread.id).id, thread.id);
  } finally {
    await runtime.close();
    if (old === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = old;
  }
});

test('restart settling never replays the occurrence handled by Run now', async t => {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout'], now: Date.UTC(2025, 0, 1, 8, 50) });
  const c = await client(t);
  t.mock.timers.setTime(c.due - 30_000);
  await c.restart();
  c.start();
  t.mock.timers.tick(30_000);
  await c.handle('run_automation_now', { id: c.id, request_id: 'click' });
  t.mock.timers.tick(30_000);
  await new Promise<void>(resolve => setImmediate(resolve));
  const { occurrences } = await c.handle('get_automation', { id: c.id });
  assert.equal(occurrences.length, 1);
  assert.equal(c.requests.filter(r => r.op === 'submit_checkout').length, 1);
});

test('pending admission survives restart and slow status never holds a request', async t => {
  let finish!: (value: unknown) => void;
  const pending = new Promise(resolve => { finish = resolve; });
  const c = await client(t, r => {
    if (r.op === 'create_admitted_thread') return { admission: { state: 'preparing' } };
    if (r.op === 'admission_status') return pending;
  });
  assert.equal(c.created.state, 'preparing');
  assert.equal(c.created.admission_request_id, 'automation-' + c.id);
  await c.restart();
  const status = () => c.handle('automation_status', { id: c.id });
  const first = await status();
  assert.equal(first.automation.state, 'preparing');
  assert.equal(first.automation.admission_request_id, 'automation-' + c.id);
  assert.equal((await c.handle('list_automations')).automations[0].state, 'preparing');
  assert.equal(c.requests.filter(r => r.op === 'admission_status').length, 1);
  finish({ admission: { state: 'ready', thread_id: 'placed' } });
  await delay(0);
  assert.equal((await status()).automation.state, 'active');
});

test('Git project gets a worktree and directory gets in-place admission', async t => {
  const folder = await client(t);
  assert.equal(folder.requests.find(r => r.op === 'create_admitted_thread')?.placement, 'wait');
  const project = await client(t, undefined, { ...definition, target: { kind: 'project', checkout: '/repo' } });
  assert.equal(project.requests.find(r => r.op === 'create_admitted_thread')?.placement, 'worktree');
});

test('list returns only card fields while get returns the saved instructions', async t => {
  const c = await client(t);
  await c.tick(c.due);
  const [card] = (await c.handle('list_automations')).automations;
  assert.deepEqual(Object.keys(card).sort(), ['id','last_result','name','next_run','schedule','state','target_path']);
  assert.equal(card.target_path, c.requests[0].checkout);
  assert.equal(card.next_run, c.due + 3600000);
  assert.equal(card.last_result.outcome, 'queued');
  assert.equal((await c.handle('get_automation', { id: c.id })).automation.definition.instructions, definition.instructions);
});

test('approval is supplied per submit even when the thread reports Ask', async t => {
  const c = await client(t, undefined, { ...definition, approval: 'auto' });
  await c.tick(c.due);
  assert.equal(c.requests.find(r => r.op === 'submit_checkout')?.approval, 'auto');
  assert.deepEqual(c.requests.find(r => r.op === 'submit_checkout')?.expect, { claim: { id: c.id + '-' + c.due, revision: 1 }, harness: 'pi', provider: 'faux', model_id: 'faux-1', mode: null, command_policy: 'trusted' });
});

test('reply handles a desktop request directly while a background client awaits its reply', async t => {
  const old = process.env.PLAIT_AUTOMATIONS;
  process.env.PLAIT_AUTOMATIONS = '1';
  const runtime = new Runtime(directory(t), createModels(), new Set());
  runtime.startBackground(request => runtime.handle(request));
  if (old === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = old;
  t.after(() => runtime.close());
  let begin!: () => void, release!: () => void;
  const entered = new Promise<void>(resolve => { begin = resolve; });
  const lease = new Promise<void>(resolve => { release = resolve; });
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: Date.UTC(2025, 0, 1, 8, 59) });
  runtime.admissions.info = async () => ({ busy: false }) as Awaited<ReturnType<typeof runtime.admissions.info>>;
  const handle = runtime.handle.bind(runtime);
  runtime.handle = async input => {
    switch ((input as { op: string }).op) {
      case 'create_admitted_thread': return { admission: { state: 'ready', thread_id: 'thread' } };
      case 'stored_thread': return { thread: { checkout: '/project', model: definition } };
      case 'command_policy': return { policy: 'trusted' };
      case 'admission_info': return { busy: false };
      case 'submit_checkout': begin(); setTimeout(release, 30_000); await lease; return {};
      default: return handle(input);
    }
  };
  const { automation } = await runtime.handle({ op: 'create_automation', definition: { ...definition, target: { kind: 'directory', path: directory(t) } }, request_id: 'create' }) as { automation: { id: string; cursor: number } };
  const due = (Math.floor(automation.cursor / 3600000) + 1) * 3600000;
  while ((await runtime.handle({ op: 'get_automation', id: automation.id }) as any).automation.state === 'preparing') await delay(5);
  t.mock.timers.setTime(due);
  const ticking = runtime['automations']!.tick(due);
  await entered;
  const click = runtime.reply({ op: 'run_automation_now', id: automation.id, request_id: 'overlap' });
  let answered = false;
  const reply = runtime.reply({ op: 'status' }).then(result => { answered = result.ok; });
  await new Promise<void>(resolve => setImmediate(resolve));
  try { assert.equal(answered, true); }
  finally { t.mock.timers.tick(30_000); await Promise.all([reply, ticking]); }
  assert.ok(answered);
  assert.equal(((await click).result as any).occurrence.request_id, automation.id + '-' + due);
});

test('creation refuses a directory inside a broader Git checkout', async t => {
  const { root } = await setup(t);
  const path = join(root, 'docs');
  mkdirSync(path);
  const c = await client(t);
  const nested = await c.handle('create_automation', {
    request_id: 'nested', definition: { ...definition, target: { kind: 'directory', path } }
  });
  let status = nested;
  while (status.automation.state === 'preparing') {
    await delay(5);
    status = await c.handle('automation_status', { id: nested.automation.id });
  }
  assert.match(status.automation.error, /inside a Git project/);
  await c.handle('create_automation', { request_id: 'root', definition: { ...definition, target: { kind: 'directory', path: root } } });
  while (c.requests.filter(r => r.op === 'create_admitted_thread').length < 2) await delay(5);
  assert.equal(c.requests.at(-1)?.placement, 'worktree');
});

test('catch-up deduplicates a due Run now accepted before its receipt was saved', async t => {
  t.mock.timers.enable({ apis: ['Date'], now: Date.UTC(2025, 0, 1, 8, 50) });
  const accepted = new Set();
  const c = await client(t, r => {
    if (r.op === 'submit_checkout' && !accepted.has(r.request_id)) {
      accepted.add(r.request_id);
      throw Error('lost reply');
    }
  });
  t.mock.timers.setTime(c.due);
  const click = () => c.handle('run_automation_now', { id: c.id, request_id: 'click' });
  assert.equal((await click()).occurrence.outcome, 'pending');
  await c.restart();
  await c.tick(c.due + 60_000);
  assert.equal((await click()).occurrence.request_id, c.id + '-' + c.due);
  assert.deepEqual([...accepted], [c.id + '-' + c.due]);
  assert.equal(c.requests.filter(r => r.op === 'submit_checkout').length, 2);
  assert.equal((await c.handle('get_automation', { id: c.id })).occurrences.length, 1);
});

test('a busy checkout skips before queue admission', async t => {
  const c = await client(t, r => r.op === 'admission_info' ? { busy: true } : undefined);
  await c.tick(c.due);
  assert.equal((await c.handle('get_automation', { id: c.id })).occurrences[0].outcome, 'skipped');
  assert.ok(!c.requests.some(r => r.op === 'submit_checkout'));
});

test('a click joins an in-flight timer claim', async t => {
  t.mock.timers.enable({ apis: ['Date'], now: Date.UTC(2025, 0, 1, 8, 50) });
  let begin!: () => void, finish!: () => void;
  const entered = new Promise<void>(resolve => { begin = resolve; });
  const release = new Promise<void>(resolve => { finish = resolve; });
  const c = await client(t, r => {
    if (r.op === 'submit_checkout') { begin(); return release; }
  });
  t.mock.timers.setTime(c.due);
  const timer = c.tick(c.due);
  await entered;
  try {
    const { occurrence } = await c.handle('run_automation_now', { id: c.id, request_id: 'click' });
    assert.equal(occurrence.request_id, c.id + '-' + c.due);
  } finally { finish(); await timer; }
  assert.equal(c.requests.filter(r => r.op === 'submit_checkout').length, 1);
});

test('retrying an old manual claim leaves the next due occurrence available', async t => {
  t.mock.timers.enable({ apis: ['Date'], now: Date.UTC(2025, 0, 1, 8, 15) });
  let lost = true;
  const c = await client(t, r => {
    if (r.op === 'submit_checkout' && lost) { lost = false; throw Error('lost reply'); }
  });
  const click = () => c.handle('run_automation_now', { id: c.id, request_id: 'old' });
  const first = await click();
  await c.restart();
  t.mock.timers.setTime(c.due + 60_000);
  assert.equal((await click()).occurrence.request_id, first.occurrence.request_id);
  await c.tick(Date.now());
  const { automation, occurrences } = await c.handle('get_automation', { id: c.id });
  assert.equal(automation.cursor, c.created.cursor);
  assert.deepEqual(occurrences.map((r: any) => r.request_id), [c.id + '-' + c.due, first.occurrence.request_id]);
});

test('a click received on the last millisecond joins the saved occurrence even after dispatch is delayed', async t => {
  t.mock.timers.enable({ apis: ['Date'], now: Date.UTC(2025, 0, 1, 8, 50) });
  const c = await client(t, undefined, { ...definition, target: { kind: 'project', checkout: '/project' }, catch_up_ms: 1000 });
  t.mock.timers.setTime(c.due);
  await c.tick(c.due);
  t.mock.timers.setTime(c.due + 21_200);
  await c.handle('update_automation', { id: c.id, definition: { ...definition, target: { kind: 'project', checkout: '/project' }, name: 'Renamed', catch_up_ms: 0 } });
  const { occurrence } = await c.handle('run_automation_now', { id: c.id, request_id: 'delayed-click', received_at_ms: c.due + 1000 });
  assert.equal(occurrence.outcome, 'queued');
  assert.equal(occurrence.request_id, c.id + '-' + c.due);
  assert.equal(c.requests.filter(r => r.op === 'submit_checkout').length, 1);
  t.mock.timers.setTime(c.due + 21_201);
  const later = await c.handle('run_automation_now', { id: c.id, request_id: 'later-click', received_at_ms: Date.now() });
  assert.match(later.occurrence.request_id, /^manual-/);
});

test('a timed-out original cannot enqueue after its retry skips and approval changes', async t => {
  const previous = process.env.PLAIT_AUTOMATIONS;
  process.env.PLAIT_AUTOMATIONS = '1';
  t.after(() => { if (previous === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = previous; });
  const models = createModels();
  models.setProvider(fauxProvider({ tokensPerSecond: 1e8 }).provider);
  const runtime = new Runtime(directory(t), models, new Set(['faux']));
  t.after(() => runtime.close());
  let original: Record<string, unknown> | undefined;
  runtime.startBackground(async request => {
    if (request.op === 'submit_checkout') { original = request; throw Error('timed out'); }

    return runtime.handle(request);
  });
  const settings = { ...definition, approval: 'auto', target: { kind: 'directory', path: directory(t) } };
  const { automation } = await runtime.handle({ op: 'create_automation', definition: settings, request_id: 'create' }) as any;
  const service = runtime['automations']!;
  const status = async () => (await service.handle({ op: 'automation_status', id: automation.id }) as any).automation;
  let active = await status();
  for (let attempt = 0; active.state === 'preparing' && attempt < 500; attempt++) {
    await delay(10);
    active = await status();
  }
  assert.equal(active.state, 'active', active.error);
  const getInfo = runtime.admissions.info.bind(runtime.admissions);
  const due = active.next_run;
  await assert.rejects(service.tick(due), /timed out/);
  assert.ok(original);
  assert.equal((await runtime.reply({ ...original, approval: 'ask' })).error?.code, 'conflict');
  runtime.admissions.info = async checkout => ({ ...await getInfo(checkout), busy: true });
  await service.tick(due);
  const saved = await service.handle({ op: 'get_automation', id: automation.id }) as any;
  assert.equal(saved.occurrences[0].outcome, 'skipped');
  assert.equal((await runtime.reply(original)).error?.code, 'conflict');
  await service.handle({ op: 'update_automation', id: automation.id, definition: { ...saved.automation.definition, approval: 'ask' } });
  assert.equal((await runtime.reply(original)).error?.code, 'conflict');
  assert.equal(runtime.store.waiters.get(String(original.thread_id), String(original.request_id)), undefined);
});

for (const window of [600_000, 7_200_000]) test(`restart runs only the current hour with a ${window} ms window`, async t => {
  let offline = true;
  const c = await client(t, r => {
    if (offline && r.op === 'stored_thread') throw Error('offline');
  }, { ...definition, catch_up_ms: window, target: { kind: 'project', checkout: '/project' } });
  await assert.rejects(c.tick(c.due), /offline/);
  await c.restart();
  offline = false;
  const hour = 3_600_000;
  await c.tick(c.due + hour + 300_000);
  await c.tick(c.due + hour + 300_001);
  assert.deepEqual(c.requests.filter(r => r.op === 'submit_checkout').map(r => r.request_id), [c.id + '-' + (c.due + hour)]);
  const { occurrences, automation } = await c.handle('get_automation', { id: c.id });
  assert.deepEqual(occurrences.map((r: any) => r.outcome), ['queued', 'missed']);
  assert.equal(automation.missed_count, 1);
});

test('a saved scheduled click retried before the timer only launches the newer hour', async t => {
  let offline = true;
  const c = await client(t, r => {
    if (offline && r.op === 'stored_thread') throw Error('offline');
  }, { ...definition, catch_up_ms: 7_200_000, target: { kind: 'project', checkout: '/project' } });
  const click = (now: number) => c.handle('run_automation_now', { id: c.id, request_id: 'retry', received_at_ms: now });
  await click(c.due);
  await c.restart();
  offline = false;
  const later = c.due + 3_900_000;
  await click(later);
  await c.tick(later);
  assert.deepEqual(c.requests.filter(r => r.op === 'submit_checkout').map(r => r.request_id),
    [c.id + '-' + (c.due + 3_600_000)]);
});

test('six-column migration saves the window and permits five-value startup inserts', async t => {
  const c = await client(t, undefined, { ...definition, catch_up_ms: 1000 });
  await c.tick(c.due);
  const db = new DatabaseSync(join(c.dir, 'automations.sqlite'));
  try {
    db.exec('ALTER TABLE automation_claims ADD COLUMN catch_up_ms INTEGER; UPDATE automation_claims SET catch_up_ms=1000; DROP TABLE automation_claim_windows');
    await c.restart();
    assert.equal(db.prepare('PRAGMA table_info(automation_claims)').all().length, 5);
    assert.equal(db.prepare('SELECT catch_up_ms FROM automation_claim_windows WHERE id=?').get(c.id + '-' + c.due)?.catch_up_ms, 1000);
    db.exec((await previousRuntime('automations/schema-five.ts')).schema);
  } finally { db.close(); }
});

for (const known of [false, true]) test(`rollback prevents a second submission with a ${known ? 'known' : 'missing'} window`, async t => {
  const settings = { ...definition, target: { kind: 'project', checkout: '/project' }, catch_up_ms: 1000 };
  const c = await client(t, undefined, settings);
  await c.tick(c.due);
  if (!known) {
    const db = new DatabaseSync(join(c.dir, 'automations.sqlite'));
    db.exec('DELETE FROM automation_claim_windows');
    db.close();
    await c.restart();
  }
  await c.handle('update_automation', { id: c.id, definition: { ...settings, catch_up_ms: 0 } });
  const click = () => c.handle('run_automation_now', { id: c.id, request_id: 'rollback', received_at_ms: c.due + 500 });
  if (!known) await assert.rejects(click(), /window.*unknown/);
  await c.restart((await previousRuntime('automations/service.ts')).Automations);
  if (!known) {
    const replay = await click();
    assert.equal(replay.occurrence.request_id, c.id + '-' + c.due);
    assert.equal(replay.occurrence.outcome, 'queued');
  }
  const fresh = await c.handle('run_automation_now', { id: c.id, request_id: 'after-rollback', received_at_ms: c.due + 500 });
  assert.equal(fresh.occurrence.outcome, 'skipped');
  assert.ok(fresh.occurrence.detail.includes(c.id + '-' + c.due));
  const repeated = await c.handle('run_automation_now', { id: c.id, request_id: 'after-rollback', received_at_ms: c.due + 501 });
  assert.equal(repeated.occurrence.request_id, c.id + '-' + c.due);
  assert.equal(repeated.occurrence.outcome, 'queued');
  await c.tick(c.due + 501);
  assert.equal(c.requests.filter(r => r.op === 'submit_checkout').length, 1);
});

test('rollback recovery reads a definitive receipt for a legacy claim without a window', async t => {
  const c = await client(t, r => { if (r.op === 'stored_thread') throw Error('offline'); });
  await assert.rejects(c.tick(c.due), /offline/);
  const db = new DatabaseSync(join(c.dir, 'automations.sqlite'));
  try {
    db.exec('DELETE FROM automation_claim_windows');
    await c.restart();
    await c.restart((await previousRuntime('automations/service.ts')).Automations);
    await c.tick(c.due + 500);
    const { occurrences } = await c.handle('get_automation', { id: c.id });
    assert.deepEqual(occurrences.map((r: any) => r.outcome), ['missed']);
    assert.equal(c.requests.filter(r => r.op === 'submit_checkout').length, 0);
  } finally { db.close(); }
});

test('a scheduled checkout check can stay busy while status is served', async t => {
  let begin!: () => void, release!: (info: { busy: boolean }) => void;
  const entered = new Promise<void>(resolve => begin = resolve);
  const gate = new Promise<{ busy: boolean }>(resolve => release = resolve);
  let held = false;
  const c = await client(t, r => {
    if (held && r.op === 'admission_info') { begin(); return gate; }
  });
  held = true;
  const tick = c.tick(c.due);
  try {
    await entered;
    assert.equal((await c.handle('automation_status', { id: c.id })).automation.state, 'active');
    assert.ok(!c.requests.some(r => r.op === 'submit_checkout'));
  } finally { release({ busy: true }); await tick; }
  assert.equal((await c.handle('get_automation', { id: c.id })).occurrences[0].outcome, 'skipped');
});

test('a pending folder probe survives flag-off main recovery without publishing a plan', async t => {
  const folder = directory(t), store = directory(t);
  const models = createModels();
  const runtime = new Runtime(store, models, new Set());
  const old = process.env.PLAIT_AUTOMATIONS, graph = process.env.PLAIT_GRAPH;
  delete process.env.PLAIT_GRAPH;
  process.env.PLAIT_AUTOMATIONS = '1';
  let release!: () => void, begin!: () => void;
  const held = new Promise<void>(resolve => release = resolve);
  const checking = new Promise<void>(resolve => begin = resolve);
  let starts = 0;
  runtime.admissions.info = async () => {
    begin();
    await held;
    return { busy: false } as Awaited<ReturnType<typeof runtime.admissions.info>>;
  };
  runtime.startBackground(async request => {
    if (request.op === 'create_admitted_thread') starts++;
    return runtime.handle(request);
  });
  try {
    const { automation } = await runtime.handle({ op: 'create_automation', request_id: 'folder-race',
      definition: { ...definition, target: { kind: 'directory', path: folder } } }) as any;
    await checking;
    assert.equal((await runtime.handle({ op: 'automation_status', id: automation.id }) as any).automation.state, 'preparing');
    delete process.env.PLAIT_AUTOMATIONS;
    const disabled = new Runtime(store, models, new Set());
    try {
      disabled.startBackground(async () => assert.fail('disabled background request'));
      await disabled.recover();
      await disabled.admissions.close();
      const { Admissions } = await previousRuntime('admission.ts');
      let effects = 0;
      const recordEffect = () => { effects++; throw Error('Unexpected placement'); };
      const recovered = new Admissions(disabled.store, { handle: recordEffect }, {}, recordEffect, () => false);
      await recovered.close();
      assert.equal(effects, 0);
      for (const table of ['thread_admissions', 'threads'])
        assert.equal(disabled.store.db.prepare('SELECT count(*) AS n FROM ' + table).get()?.n, 0);
      assert.equal(disabled.store.db.prepare("SELECT name FROM sqlite_master WHERE name='automation_admission_probes'").get(), undefined);
    } finally { await disabled.close(); }
    await exec('/usr/bin/git', ['init', '-q', folder]);
    release();
    await Promise.all(runtime['automations']!['placements'].values());
    const status = await runtime.handle({ op: 'automation_status', id: automation.id }) as any;
    assert.match(status.automation.error, /became a Git checkout/);
    assert.equal(starts, 0);
  } finally {
    release();
    await runtime.close();
    if (old === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = old;
    if (graph === undefined) delete process.env.PLAIT_GRAPH; else process.env.PLAIT_GRAPH = graph;
  }
});
