import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage } from '@earendil-works/pi-ai/providers/faux';
import { setTimeout as delay } from 'node:timers/promises';
import { setup } from './support/safety.ts';
import { Automations } from '../src/automations/service.ts';
import { HostError } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';

const definition = {
  name: 'Review', instructions: 'Review recent changes', target: { kind: 'directory', path: '/project' },
  harness: 'pi', provider: 'faux', model_id: 'faux-1', approval: 'ask', command_policy: 'trusted',
  schedule: { kind: 'hourly', minute: 0, timeZone: 'UTC' }
};
function directory(t: TestContext) {
  const dir = mkdtempSync(join(tmpdir(), 'automations-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
async function client(t: TestContext, respond: (r: Record<string, unknown>) => unknown = () => undefined, settings: Omit<typeof definition, 'target'> & { target: { kind: string; path?: string; checkout?: string } } = definition) {
  const dir = directory(t), requests: Record<string, unknown>[] = [];
  const send = async (r: Record<string, unknown>) => {
    requests.push(r);
    const scripted = respond(r);
    if (scripted !== undefined) return scripted;
    switch (r.op) {
      case 'create_admitted_thread': return { admission: { state: 'ready', thread_id: requests.filter(request => request.op === 'create_admitted_thread').length === 1 ? 'thread' : 'second' } };
      case 'watch': return { thread: { checkout: '/project', model: { provider: 'faux', model_id: 'faux-1' }, approval_mode: 'ask' } };
      case 'admission_info': return { busy: false };
      case 'checkout_queue': return { waiters: [] };
      case 'command_policy': return { policy: 'trusted' };
      case 'submit_checkout': return { accepted: true };
      default: throw Error('Unexpected ' + r.op);
    }
  };
  let service = new Automations(dir, send);
  t.after(() => service.close());
  const handle = (op: string, fields = {}): Promise<any> => service.handle({ op, ...fields });
  const { automation } = await handle('create_automation', { definition: settings.target.kind === 'directory' ? { ...settings, target: { kind: 'directory', path: dir } } : settings, request_id: 'create' });
  await Promise.all(service['placements'].values());
  const due = (Math.floor(automation.cursor / 3600000) + 1) * 3600000;
  return {
    requests, due, created: automation, id: automation.id, start: () => service.start(), tick: (now: number) => service.tick(now), handle,
    restart: async () => { await service.close(); service = new Automations(dir, send); }
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
  assert.deepEqual(c.requests.map(r => r.op), ['create_admitted_thread', 'watch', 'admission_info', 'command_policy']);
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
    const c = await client(t,r => r.op === 'watch' ? {thread:{...thread,...changed}} : undefined);
    await c.tick(c.due);
    const {automation} = await c.handle('get_automation',{id:c.id});
    assert.equal(automation.state,'paused');
    assert.equal(automation.pause_reason,'Settings changed outside the automation');
    assert.ok(!c.requests.some(r => r.op === 'submit_checkout'));
  }
});
test('real Runtime gates routing and storage on the flag', async t => {
  const dir = directory(t);
  const old = process.env.PLAIT_AUTOMATIONS;
  try {
    delete process.env.PLAIT_AUTOMATIONS;
    const runtime = new Runtime(dir, createModels(), new Set());
    try {
      await assert.rejects(runtime.handle({ op: 'list_automations' }), /Unknown operation/);
      assert.ok(!existsSync(join(dir, 'automations.sqlite')));
    } finally { await runtime.close(); }
    process.env.PLAIT_AUTOMATIONS = '1';
    const enabled = new Runtime(dir, createModels(), new Set());
    try { assert.deepEqual(await enabled.handle({ op: 'list_automations' }), { automations: [] }); }
    finally { await enabled.close(); }
  } finally { if (old === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = old; }
});

test('real placement creates a thread, Run now submits to it, and removal pauses the schedule', async t => {
  const dir = directory(t);
  const path = join(dir, 'folder'), store = join(dir, 'store');
  mkdirSync(path); mkdirSync(store);
  const old = process.env.PLAIT_AUTOMATIONS;
  process.env.PLAIT_AUTOMATIONS = '1';
  const models = createModels(), faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('Complete')]);
  models.setProvider(faux.provider);
  const runtime = new Runtime(store, models, new Set(['faux']));
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
      const result = await runtime.handle({ op: 'get_automation', id: automation.id }) as { automation: typeof automation & { state: string } };
      if (result.automation.state === 'active') { automation.thread_id = result.automation.thread_id; break; }
      assert.equal(result.automation.state, 'preparing');
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

    renameSync(path, join(dir, 'moved'));
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
  assert.equal(folder.requests[0].placement, 'wait');
  const project = await client(t, undefined, { ...definition, target: { kind: 'project', checkout: '/repo' } });
  assert.equal(project.requests[0].placement, 'worktree');
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
});

test('desktop requests wait their turn behind a scheduled host request', async t => {
  const old = process.env.PLAIT_AUTOMATIONS;
  process.env.PLAIT_AUTOMATIONS = '1';
  const runtime = new Runtime(directory(t), createModels(), new Set());
  if (old === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = old;
  t.after(() => runtime.close());
  let begin!: () => void, release!: () => void;
  const entered = new Promise<void>(resolve => { begin = resolve; });
  const lease = new Promise<void>(resolve => { release = resolve; });
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: Date.UTC(2025, 0, 1, 8, 59) });
  const handle = runtime.handle.bind(runtime);
  runtime.handle = async input => {
    switch ((input as { op: string }).op) {
      case 'create_admitted_thread': return { admission: { state: 'ready', thread_id: 'thread' } };
      case 'watch': return { thread: { checkout: '/project', model: definition } };
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
  try { assert.equal(answered, false); }
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
