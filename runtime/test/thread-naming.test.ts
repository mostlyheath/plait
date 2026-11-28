import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setImmediate as tick, setTimeout as delay } from 'node:timers/promises';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxText, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { Store } from '../src/store.ts';
import { generatedTitle, nameThread, ThreadNaming, type NamingCall } from '../src/thread-naming.ts';
import type { Snapshot } from '../src/protocol.ts';

const model = { provider: 'faux', model_id: 'faux-1' };
const prompt = 'I have been looking at this for a while. Please fix retry handling for HTTP 429 responses.';
const answer = 'Fix HTTP 429 retry handling';
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

async function fixture(t: { after(fn: () => Promise<void>): void }, call: NamingCall = async () => answer, deadline = 45_000) {
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-naming-'));
  const store = new Store(directory);
  const models = createModels(); models.setProvider(fauxProvider().provider);
  const authenticated = new Set(['faux']);
  const owner = { models, authenticated, snapshot: (id: string) => store.get(id), save: (snapshot: Snapshot) => { store.save(snapshot); } };
  const naming = new ThreadNaming(store, owner, call, deadline);
  t.after(async () => { await naming.close(); store.close(); await rm(directory, { recursive: true, force: true }); });
  const enable = () => naming.handle({ op: 'set_thread_naming', enabled: true, model });
  const admit = (id = 'thread', text = prompt, request = 'r1') => {
    const snapshot: Snapshot = { id, checkout: '/repo', model, status: 'running', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
    store.admit(id, request, text, { accepted: true }, snapshot);
    naming.accepted(id, request, text);
    return snapshot;
  };
  return { directory, store, models, authenticated, naming, enable, admit };
}

test('names are plain, bounded Unicode text, never multi-line explanations or controls', () => {
  assert.equal(generatedTitle('  “Fix HTTP 429 retries.”  '), 'Fix HTTP 429 retries');
  assert.equal(generatedTitle('`修复请求重试`'), '修复请求重试');
  assert.equal(generatedTitle('Fix  HTTP   retries'), 'Fix HTTP retries');
  for (const text of ['', '  ', 'Title: Fix retries', '# Fix retries', '**Fix retries**', '{"title":"x"}', 'Fix retries\nExplanation', 'Fix\u202eretires', 'Fix\u0007retries', '🙂']) {
    assert.equal(generatedTitle(text), undefined, JSON.stringify(text));
  }
  assert.equal(Array.from(generatedTitle('界'.repeat(90))!).length, 72);
  assert(!generatedTitle('Handle temporary network errors and retry failed requests without losing data')!.endsWith('losi'));
});

test('settings default off, validate exact models, persist, and do not affect the coding default', async t => {
  const h = await fixture(t);
  assert.deepEqual(h.naming.get(), { enabled: false, model: null });
  h.store.setDefault(model);
  for (const request of [
    { enabled: true, model: null }, { enabled: 'yes', model },
    { enabled: true, model: { ...model, extra: true } },
    { enabled: true, model: { provider: 'faux', model_id: 'bad\nmodel' } },
    { enabled: true, model, extra: true },
  ]) assert.throws(() => h.naming.handle({ op: 'set_thread_naming', ...request }));
  assert.throws(() => h.naming.handle({ op: 'set_thread_naming', enabled: true, model: { ...model, provider: 'codex' } }), /unavailable/);
  h.enable();
  const reopened = new Store(h.directory);
  try { assert.deepEqual(JSON.parse(reopened.setting('thread_naming_v1')!), { enabled: true, model }); }
  finally { reopened.close(); }
  assert.deepEqual(h.store.defaultModel(), model);
  h.authenticated.clear();
  assert.throws(h.enable, /unavailable/);
  assert.doesNotThrow(() => h.naming.handle({ op: 'set_thread_naming', enabled: false, model }));
});

test('naming is nonblocking, keeps run state, and persists one title without changing activity age', async t => {
  const result = deferred<string>();
  let calls = 0;
  const h = await fixture(t, async (_models, choice, input) => {
    calls++; assert.deepEqual(choice, model); assert.equal(input, prompt); return result.promise;
  });
  h.enable();
  const first = h.admit();
  assert.equal(h.store.get(first.id).title, prompt.slice(0, 72));
  await tick();
  assert.equal(calls, 1);
  result.resolve(answer);
  await tick();
  const current = h.store.get(first.id);
  assert.equal(current.title, answer);
  assert.equal(current.status, 'running');
  assert.equal(current.updated_at, first.updated_at);
  assert.deepEqual(current.messages, first.messages);
  h.store.admit(first.id, 'r2', 'Second request', {}, current);
  h.naming.accepted(first.id, 'r2', 'Second request');
  await tick();
  assert.equal(calls, 1);
});

for (const change of ['rename', 'settings', 'disable', 'close'] as const) {
  test(`a late name cannot overwrite ${change}`, async t => {
    const result = deferred<string>();
    let signal: AbortSignal | undefined;
    const h = await fixture(t, async (_models, _choice, _input, abort) => { signal = abort; return result.promise; });
    h.enable(); const first = h.admit(); await tick();
    if (change === 'rename') {
      const snapshot = h.store.get(first.id); snapshot.title = 'My chosen name'; snapshot.renamed = true; h.store.save(snapshot);
    } else if (change === 'settings') h.enable();
    else if (change === 'disable') h.naming.handle({ op: 'set_thread_naming', enabled: false, model });
    else await h.naming.close();
    result.resolve(answer); await tick();
    assert.equal(h.store.get(first.id).title, change === 'rename' ? 'My chosen name' : first.title);
    if (change !== 'rename') assert.equal(signal?.aborted, true);
  });
}

test('disabled, old, renamed, and image-only threads make no call', async t => {
  let calls = 0;
  const h = await fixture(t, async () => { calls++; return answer; });
  const old = h.admit('old');
  h.enable();
  h.naming.accepted(old.id, 'r2', 'A later message');
  h.admit('image-only', '');
  const renamed = { ...old, id: 'renamed', title: 'Chosen', renamed: true };
  h.store.admit(renamed.id, 'r1', prompt, {}, renamed);
  h.naming.accepted(renamed.id, 'r1', prompt);
  await tick(); assert.equal(calls, 0);
});

test('timeouts and provider errors keep the fallback and release the queue without a retry', async t => {
  let calls = 0;
  let firstSignal: AbortSignal | undefined;
  const h = await fixture(t, async (_models, _choice, _text, signal) => {
    calls++;
    if (calls === 1) { firstSignal = signal; return new Promise(() => {}); }
    if (calls === 2) throw new Error('Provider details must not enter the transcript');
    return answer;
  }, 20);
  h.enable(); const first = h.admit('first'); const second = h.admit('second'); h.admit('third');
  await delay(80);
  assert.equal(firstSignal?.aborted, true);
  assert.equal(calls, 3);
  assert.equal(h.store.get('first').title, first.title);
  assert.equal(h.store.get('second').title, second.title);
  assert.equal(h.store.get('third').title, answer);
  assert.equal(h.store.get('second').error, null);
});

test('queue and prompt bounds keep burst traffic small and skip unavailable credentials', async t => {
  const wait = deferred<string>(); let calls = 0;
  const h = await fixture(t, async (_models, _choice, text) => {
    calls++; assert(text.length <= 8000); assert(!/[\uD800-\uDBFF]$/u.test(text)); return wait.promise;
  });
  h.enable();
  for (let i = 0; i < 40; i++) h.admit(`thread-${i}`, '🙂'.repeat(6000));
  await tick(); assert.equal(calls, 1);
  h.authenticated.clear(); wait.resolve(answer); await tick();
  assert.equal(calls, 1);
  assert.notEqual(h.store.get('thread-39').title, answer);
});

test('the pinned model API produces a title and rejects tool calls or exhausted output', async () => {
  const faux = fauxProvider(); const models = createModels(); models.setProvider(faux.provider);
  faux.setResponses([
    fauxAssistantMessage([fauxText(answer)]),
    fauxAssistantMessage([fauxToolCall('edit', { path: 'file' })], { stopReason: 'toolUse' }),
    fauxAssistantMessage([fauxText(answer)], { stopReason: 'length' }),
  ]);
  const signal = new AbortController().signal;
  assert.equal(await nameThread(models, model, prompt, signal), answer);
  assert.equal(await nameThread(models, model, prompt, signal), undefined);
  assert.equal(await nameThread(models, model, prompt, signal), undefined);
});
