import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxText } from '@earendil-works/pi-ai/providers/faux';
import { Runtime } from '../src/runtime.ts';
import type { Snapshot } from '../src/protocol.ts';
import { setup } from './support/safety.ts';

async function until(done: () => boolean) {
  const deadline = Date.now() + 8000;
  while (!done()) { assert(Date.now() < deadline, 'Naming or coding run did not settle'); await delay(10); }
}

async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const { root, base } = await setup(t);
  const directory = join(base, 'host'); await mkdir(directory);
  const coding = fauxProvider({ provider: 'coding' });
  coding.setResponses(Array.from({ length: 8 }, () => fauxAssistantMessage([fauxText('Task response.')])));
  const naming = fauxProvider({ provider: 'naming' });
  const models = createModels(); models.setProvider(coding.provider); models.setProvider(naming.provider);
  const runtime = new Runtime(directory, models, new Set(['coding', 'naming']));
  t.after(() => runtime.close());
  const choice = { provider: 'naming', model_id: naming.models[0].id };
  await runtime.handle({ op: 'set_thread_naming', enabled: true, model: choice });
  const create = async (request: string) => (await runtime.handle({ op: 'create_thread', protocol_version: 7,
    checkout: root, provider: 'coding', model_id: coding.models[0].id, request_id: request }) as { thread: Snapshot }).thread;
  return { root, runtime, naming, create, choice };
}

test('a new submission names once, publishes its summary, and leaves coding history alone', async t => {
  const h = await fixture(t);
  let finish!: () => void;
  const released = new Promise<void>(resolve => { finish = resolve; });
  const prompt = 'Could you take a look at how requests fail and add HTTP 429 retries?';
  h.naming.setResponses([async (context, options) => {
    assert.equal(context.messages.length, 2);
    const system = context.messages[0];
    assert.equal(system.role, 'system');
    if (system.role === 'system') assert.deepEqual(system.toolsAdded ?? [], []);
    assert.equal(context.messages[1].role, 'user');
    assert.equal(context.messages[1].content, prompt);
    assert.equal(options?.maxRetries, 0);
    await released;
    return fauxAssistantMessage([fauxText('Add HTTP 429 retries')]);
  }]);
  const thread = await h.create('create');
  const events: string[] = []; h.runtime.subscribe(id => events.push(id));
  const input = { op: 'submit', protocol_version: 7, thread_id: thread.id, request_id: 'first', text: prompt };
  assert.deepEqual(await h.runtime.handle(input), { thread_id: thread.id, request_id: 'first', accepted: true });
  await until(() => !h.runtime.sessions.has(thread.id));
  assert.equal(h.runtime.snapshot(thread.id).title, prompt);
  assert.equal(h.runtime.snapshot(thread.id).status, 'idle');
  finish(); await until(() => h.runtime.snapshot(thread.id).title === 'Add HTTP 429 retries');
  assert(events.includes('#'));
  const page = await h.runtime.handle({ op: 'list_threads', protocol_version: 7 }) as { threads: Snapshot[] };
  assert.equal(page.threads[0].title, 'Add HTTP 429 retries');
  assert.deepEqual(h.runtime.snapshot(thread.id).messages.map(message => message.text), [prompt, 'Task response.']);
  await h.runtime.handle(input);
  await h.runtime.handle({ ...input, request_id: 'second', text: 'Keep going' });
  await until(() => !h.runtime.sessions.has(thread.id));
  assert.equal(h.naming.state.callCount, 1);
  assert.equal(h.runtime.snapshot(thread.id).title, 'Add HTTP 429 retries');
  assert.equal(h.runtime.store.get(thread.id).title, 'Add HTTP 429 retries');
});

test('manual rename wins over a naming reply while the real runtime finishes', async t => {
  const h = await fixture(t);
  let finish!: () => void;
  const released = new Promise<void>(resolve => { finish = resolve; });
  h.naming.setResponses([async () => { await released; return fauxAssistantMessage([fauxText('Generated name')]); }]);
  const thread = await h.create('create');
  await h.runtime.handle({ op: 'submit', protocol_version: 7, thread_id: thread.id, request_id: 'first', text: 'Fix retries' });
  await until(() => h.naming.state.callCount === 1);
  await h.runtime.handle({ op: 'rename_thread', protocol_version: 7, thread_id: thread.id, title: 'My name' });
  finish(); await until(() => !h.runtime.sessions.has(thread.id)); await delay(20);
  assert.equal(h.runtime.snapshot(thread.id).title, 'My name');
  assert.equal(h.runtime.store.get(thread.id).title, 'My name');
});

test('a first message waiting for a checkout is named once without starting a coding run', async t => {
  const h = await fixture(t);
  h.naming.setResponses([fauxAssistantMessage([fauxText('Fix queued retry handling')])]);
  const thread = await h.create('create');
  h.runtime.active.set(h.root, 'another-writer');
  try {
    const request = { op: 'submit_checkout', thread_id: thread.id, request_id: 'first', text: 'Please fix how queued retries are handled.', approval: 'ask' };
    await h.runtime.handle(request);
    await h.runtime.handle(request);
    await until(() => h.naming.state.callCount === 1);
    await delay(20);
    assert(!h.runtime.sessions.has(thread.id));
    assert.equal(h.runtime.store.get(thread.id).title, 'Fix queued retry handling');
    assert.equal(h.naming.state.callCount, 1);
    await h.runtime.handle({ op: 'cancel_checkout_wait', thread_id: thread.id, request_id: 'first' });
  } finally { h.runtime.active.delete(h.root); }
});

test('existing conversations are not renamed when naming is enabled later', async t => {
  const h = await fixture(t);
  await h.runtime.handle({ op: 'set_thread_naming', enabled: false, model: h.choice });
  const thread = await h.create('create');
  await h.runtime.handle({ op: 'submit', protocol_version: 7, thread_id: thread.id, request_id: 'first', text: 'Old conversation' });
  await until(() => !h.runtime.sessions.has(thread.id));
  await h.runtime.handle({ op: 'set_thread_naming', enabled: true, model: h.choice });
  await h.runtime.handle({ op: 'submit', protocol_version: 7, thread_id: thread.id, request_id: 'second', text: 'New message' });
  await until(() => !h.runtime.sessions.has(thread.id));
  assert.equal(h.naming.state.callCount, 0);
  assert.equal(h.runtime.snapshot(thread.id).title, 'Old conversation');
});
