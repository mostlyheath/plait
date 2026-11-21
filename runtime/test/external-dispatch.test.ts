import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { Runtime } from '../src/runtime.ts';
import { ExternalHarnesses } from '../src/external/registry.ts';
import { ExternalEntries, transcriptPath } from '../src/external/entries.ts';
import { setup } from './support/safety.ts';

test('unknown saved harness refuses work while its transcript remains readable', async t => {
  const { root, base } = await setup(t);
  const directory = join(base, 'store'); await mkdir(directory);
  const models = createModels(); models.setProvider(fauxProvider().provider);
  const host = new Runtime(directory, models, new Set(['faux']));
  t.after(async () => { await host.close(); });
  const created = await host.handle({ op: 'create_thread', protocol_version: 4, checkout: root, provider: 'faux', model_id: 'faux-1', request_id: 'create' }) as { thread: { id: string } };
  const id = created.thread.id;
  const row = host.store.db.prepare('SELECT snapshot FROM threads WHERE id=?').get(id)!;
  const snapshot = JSON.parse(String(row.snapshot));
  snapshot.harness = { id: 'antigravity', version: null, mode: null, reported: null };
  host.store.db.prepare('UPDATE threads SET snapshot=?, harness=? WHERE id=?').run(JSON.stringify(snapshot), 'antigravity', id);
  const entries = new ExternalEntries(transcriptPath(directory, id, 'antigravity'));
  entries.append([{ id: 1, kind: 'message', model: [{ role: 'assistant', content: 'saved transcript' }] }]);
  entries.close();
  for (const request of [
    { op: 'submit', thread_id: id, request_id: 'one', text: 'hello' },
    { op: 'submit', thread_id: id, request_id: 'two', text: 'steer', when_busy: 'steer' },
    { op: 'configure_thread', thread_id: id, provider: 'faux', model_id: 'faux-1' },
    { op: 'submit_checkout', thread_id: id, request_id: 'three', text: 'wait' },
  ]) {
    const result = await host.reply({ ...request, protocol_version: 6 }) as { ok: boolean; error?: { code: string } };
    assert.equal(result.ok, false); assert.equal(result.error?.code, 'invalid_request');
  }
  assert.equal(existsSync(join(directory, `${id}.sqlite`)), false);
  const history = await host.handle({ op: 'history', protocol_version: 6, thread_id: id });
  assert.match(JSON.stringify(history), /saved transcript/);
  const watch = await host.handle({ op: 'watch', protocol_version: 6, thread_id: id });
  assert.match(JSON.stringify(watch), /antigravity/);
  const registry = new ExternalHarnesses(host);
  for (const harness of [undefined, 'pi', 'codex', 'claude', 'antigravity']) {
    const selected = registry.resolveHarness({ ...snapshot, harness: harness ? { id: harness } : undefined });
    assert.equal(selected.kind, harness === 'antigravity' ? 'unknown' : harness === 'codex' || harness === 'claude' ? 'external' : 'pi');
    if (selected.kind === 'external') assert.equal(selected.harness, host[selected.id]);
  }
});

test('a harness without steering refuses before accepting or queueing', async t => {
  const { root, base } = await setup(t);
  const directory = join(base, 'store'); await mkdir(directory);
  const models = createModels(); models.setProvider(fauxProvider().provider);
  const host = new Runtime(directory, models, new Set(['faux']));
  t.after(async () => { await host.close(); });
  const created = await host.handle({ op: 'create_thread', protocol_version: 4, checkout: root, provider: 'faux', model_id: 'faux-1', request_id: 'create' }) as { thread: { id: string } };
  const id = created.thread.id;
  const row = host.store.db.prepare('SELECT snapshot FROM threads WHERE id=?').get(id)!;
  const snapshot = JSON.parse(String(row.snapshot));
  snapshot.harness = { id: 'codex', version: null, mode: null, reported: null };
  host.store.db.prepare('UPDATE threads SET snapshot=?, harness=? WHERE id=?').run(JSON.stringify(snapshot), 'codex', id);
  Object.defineProperty(host.codex, 'supportsSteer', { value: false });
  const result = await host.reply({ op: 'submit', protocol_version: 4, thread_id: id, request_id: 'steer', text: 'hello', when_busy: 'steer' }) as { ok: boolean; error?: { code: string; message: string } };
  assert.equal(result.ok, false);
  assert.equal(result.error?.code, 'invalid_request');
  assert.equal(result.error?.message, 'This harness cannot take a message while it runs. Queue it instead.');
  assert.deepEqual(host.store.queue(id), []);
  assert.equal(host.store.duplicate(id, 'steer', 'hello'), undefined);
});
