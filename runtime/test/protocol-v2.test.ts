import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import type { ConversationView } from '@earendil-works/pi-durable';
import { validate, view } from '../src/protocol.ts';
import type { Snapshot } from '../src/protocol.ts';
import { update } from '../src/transcript.ts';
import { request, setup, until } from './support/host.ts';

const empty = (): Snapshot => ({ id: 'x', checkout: '/repo', model: { provider: 'faux', model_id: 'faux-1' }, status: 'idle',
  messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false });

test('version 2 fields are opt-in and approval needs version 2', () => {
  assert.deepEqual(validate({ op: 'status', protocol_version: 2 }), { op: 'status', protocol_version: 2 });
  validate({ op: 'submit', thread_id: 't', request_id: 'r', text: 'hi', protocol_version: 2, approval: 'auto' });
  validate({ op: 'watch', thread_id: 't', protocol_version: 1 });
  for (const input of [
    { op: 'status', protocol_version: 8 },
    { op: 'status', protocol_version: '2' },
    { op: 'submit', thread_id: 't', request_id: 'r', text: 'hi', approval: 'auto' },
    { op: 'submit', thread_id: 't', request_id: 'r', text: 'hi', protocol_version: 2, approval: 'always' },
    { op: 'watch', thread_id: 't', protocol_version: 2, approval: 'auto' },
  ]) assert.throws(() => validate(input), JSON.stringify(input));
});

test('tools record the message they followed, and version 1 views omit new fields', () => {
  const state = empty();
  // This fixture supplies only the fields read by the transcript adapter.
  const conversation = { entries: [{ id: 1, model: [
    { role: 'assistant', content: [{ type: 'toolCall', id: 'early', name: 'read', arguments: { path: 'a' } }] },
    { role: 'user', content: 'Fix it' },
    { role: 'assistant', content: [{ type: 'text', text: 'Looking.' }, { type: 'toolCall', id: 'read-1', name: 'read', arguments: { path: 'a' } }] },
    { role: 'toolResult', toolCallId: 'read-1', content: 'ok' },
    { role: 'assistant', content: [{ type: 'toolCall', id: 'edit-1', name: 'edit', arguments: { path: 'a' } }] },
    { role: 'toolResult', toolCallId: 'edit-1', content: 'ok' },
    { role: 'assistant', content: [{ type: 'text', text: 'Done.' }] },
  ] }], docs: {} } as unknown as ConversationView;
  update(state, conversation);
  assert.deepEqual(state.messages.map(message => message.text), ['Fix it', 'Looking.', 'Done.']);
  assert.deepEqual(state.tools.map(tool => [tool.id, tool.after]), [['early', null], ['read-1', '1-2'], ['edit-1', '1-2']]);

  const legacy = view(state, 1) as Record<string, unknown> & { tools: Record<string, unknown>[] };
  assert(!('approval_mode' in legacy));
  assert(legacy.tools.every(tool => !('after' in tool)));
  const current = view({ ...state, tools: [{ ...state.tools[0], after: undefined }] }, 2) as Snapshot;
  assert.equal(current.approval_mode, 'ask');
  assert(!('after' in JSON.parse(JSON.stringify(current.tools[0]))));
});

test('status and watch answer in the version the client asked for', { timeout: 30000 }, async t => {
  const f = await setup(t);
  const thread = await f.create();
  assert.equal(((await request(f.store, { op: 'status' })).result as unknown as { protocol_version: number }).protocol_version, 1);
  assert.equal(((await request(f.store, { op: 'status', protocol_version: 2 })).result as unknown as { protocol_version: number }).protocol_version, 2);
  assert(!('approval_mode' in (await request(f.store, { op: 'watch', thread_id: thread.id })).result.thread));
  const current = (await request(f.store, { op: 'watch', thread_id: thread.id, protocol_version: 2 })).result.thread;
  assert.equal(current.approval_mode, 'ask');
});

test('automatic approval runs edits and commands without prompts for that run only', { timeout: 30000 }, async t => {
  const f = await setup(t, 'separate-edits');
  const thread = await f.create();
  const submit = (request_id: string, approval?: string) => request(f.store, {
    op: 'submit', thread_id: thread.id, request_id, text: 'Do the scripted task.', protocol_version: 2, ...(approval ? { approval } : {}),
  });
  const watch = async () => (await request(f.store, { op: 'watch', thread_id: thread.id, protocol_version: 2 })).result.thread;
  assert.equal((await submit('auto-1', 'auto')).ok, true);
  const done = await until(watch, value => value.status === 'idle');
  assert.equal(done.approval_mode, 'auto');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');
  assert(done.tools.every(tool => tool.status === 'done' && typeof tool.after === 'string'), JSON.stringify(done.tools));

  // The grant ends with the run: an unspecified mode asks again.
  assert.equal((await submit('ask-2')).ok, true);
  const pending = await until(watch, value => !!value.pending_approval);
  assert.equal(pending.approval_mode, 'ask');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'after\n');
  assert.equal((await f.approve(thread.id, pending.pending_approval!.id)).ok, true);
  await until(watch, value => value.status === 'idle');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'followup\n');
});

test('automatic approval still protects dirty paths and the read-only checkout', { timeout: 30000 }, async t => {
  const f = await setup(t, 'dirty');
  await writeFile(join(f.repo, 'notes.txt'), 'dirty\n');
  const thread = await f.create();
  const reply = await request(f.store, { op: 'submit', thread_id: thread.id, request_id: 'auto-dirty', text: 'Overwrite it.', protocol_version: 2, approval: 'auto' });
  assert.equal(reply.ok, true);
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(done.pending_approval, null);
  assert.equal(done.tools.length, 2);
  assert(done.tools.every(tool => tool.status === 'error'), JSON.stringify(done.tools));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'dirty\n');
});
