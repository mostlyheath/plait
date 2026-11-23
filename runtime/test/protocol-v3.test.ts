import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { ConversationView, EntryRecord } from '@earendil-works/pi-durable';
import { legacy, readsAsTyped, validate, view } from '../src/protocol.ts';
import type { Snapshot } from '../src/protocol.ts';
import { boundSnapshot, update } from '../src/transcript.ts';
import { historyPage, historyText, INLINE_UNITS } from '../src/history.ts';
import type { HistoryItem, HistorySource } from '../src/history.ts';
import type { PendingInput, Placed } from '../src/store.ts';
import { clipTail, customTitle, title } from '../src/text.ts';

const empty = (): Snapshot => ({ id: 'x', checkout: '/repo', model: { provider: 'faux', model_id: 'faux-1' }, status: 'idle',
  messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false });
const bytes = (value: unknown) => Buffer.byteLength(JSON.stringify(value));

test('version 3 operations and fields validate only with version 3', () => {
  const t = { thread_id: 't', protocol_version: 3 };
  for (const input of [
    { op: 'status', protocol_version: 3 },
    { op: 'list_threads', protocol_version: 3 },
    { op: 'list_threads', protocol_version: 3, checkout: '/repo', limit: 200, cursor: '1700000000000:12', archived: true },
    { op: 'watch_threads', protocol_version: 3 }, { op: 'watch_threads', protocol_version: 3, archived: true },
    { op: 'history', ...t }, { op: 'history', ...t, before: '12:0:0', limit: 500 },
    { op: 'history_text', ...t, ref: 'e12.0.text', offset: 0 }, { op: 'history_text', ...t, ref: 'e12.3.input.2', offset: 65536 },
    { op: 'history_text', ...t, ref: 'q7', offset: 1 },
    { op: 'rename_thread', ...t, title: 'New name' }, { op: 'archive_thread', ...t, archived: false },
    { op: 'configure_thread', ...t, provider: 'p', model_id: 'm' }, { op: 'configure_thread', ...t, thinking: 'high' },
    { op: 'configure_thread', ...t, provider: 'p', model_id: 'm', thinking: 'off' },
    { op: 'cancel_queued', ...t, request_id: 'r' },
    { op: 'submit', ...t, request_id: 'r', text: 'hi', when_busy: 'follow_up' },
    { op: 'submit', ...t, request_id: 'r', text: 'hi', when_busy: 'steer', approval: 'auto' },
  ]) assert.doesNotThrow(() => validate(input), JSON.stringify(input));
  for (const input of [
    { op: 'status', protocol_version: 8 },
    { op: 'history', thread_id: 't' }, { op: 'history', thread_id: 't', protocol_version: 2 },
    { op: 'list_threads' }, { op: 'list_threads', protocol_version: 2, limit: 5 },
    { op: 'list_threads', protocol_version: 3, limit: 0 }, { op: 'list_threads', protocol_version: 3, limit: 201 },
    { op: 'list_threads', protocol_version: 3, limit: '5' }, { op: 'list_threads', protocol_version: 3, cursor: 'x' },
    { op: 'list_threads', protocol_version: 3, archived: 'yes' }, { op: 'list_threads', protocol_version: 3, checkout: '' },
    { op: 'list_threads', protocol_version: 3, extra: 1 },
    { op: 'history', ...t, before: '12' }, { op: 'history', ...t, limit: 501 }, { op: 'history', ...t, cursor: '1:1' },
    { op: 'history_text', ...t, ref: 'e12.0.text' }, { op: 'history_text', ...t, ref: 'e12.0.secret', offset: 0 },
    { op: 'history_text', ...t, ref: 'q7', offset: -1 }, { op: 'history_text', ...t, ref: 'q7', offset: 1.5 },
    { op: 'rename_thread', ...t }, { op: 'rename_thread', ...t, title: '   ' }, { op: 'archive_thread', ...t, archived: 1 },
    { op: 'configure_thread', ...t }, { op: 'configure_thread', ...t, provider: 'p' }, { op: 'configure_thread', ...t, thinking: 'extreme' },
    { op: 'cancel_queued', ...t }, { op: 'cancel_queued', ...t, request_id: 'bad id' },
    { op: 'submit', ...t, request_id: 'r', text: 'hi', when_busy: 'reject' },
    { op: 'submit', thread_id: 't', request_id: 'r', text: 'hi', when_busy: 'steer' },
    { op: 'submit', thread_id: 't', request_id: 'r', text: 'hi', protocol_version: 2, when_busy: 'steer' },
  ]) assert.throws(() => validate(input), JSON.stringify(input));
});

test('titles follow the desktop rule', () => {
  assert.equal(title('  Fix\n the\tparser 🙂  '), 'Fix the parser 🙂');
  assert.equal(Array.from(title('🙂'.repeat(100))).length, 72);
  assert.equal(title('a\u0007b\u0085c\u00a0d\u2028e'), 'ab c d e');
  // U+FEFF is not whitespace in Rust, so it stays, as it does on the desktop.
  assert.equal(title('a\ufeffb'), 'a\ufeffb');
  assert.equal(Array.from(customTitle('x'.repeat(300))).length, 200);
  assert.equal(customTitle(' \n\t '), '');
});

// This fixture supplies only the fields read by the transcript adapter.
const thinking = () => ({ entries: [
  { id: 1, model: [{ role: 'user', content: 'Fix it' }] },
  { id: 2, model: [{ role: 'assistant', stopReason: 'toolUse', usage: { input: 10, output: 5, cacheRead: 100, cacheWrite: 20, totalTokens: 135 }, content: [
    { type: 'thinking', thinking: 'Read first.' }, { type: 'thinking', thinking: 'opaque', redacted: true },
    { type: 'toolCall', id: 'read-1', name: 'read', arguments: { path: 'a' } }] }] },
  { id: 3, model: [{ role: 'toolResult', toolCallId: 'read-1', content: 'ok' }] },
  { id: 4, model: [{ role: 'assistant', stopReason: 'stop', usage: { input: 30, output: 7, cacheRead: 0, cacheWrite: 0, reasoning: 3, totalTokens: 37 }, content: [
    { type: 'thinking', thinking: 'Now answer.' }, { type: 'text', text: 'Done.' }, { type: 'toolCall', id: 'late', name: 'read', arguments: {} }] }] },
], docs: { 'pi.live': { generation: { attempt: 2, retry: { at: 1234, error: 'SECRET' }, message: { role: 'assistant', content: [{ type: 'thinking', thinking: 'Live thought' }, { type: 'text', text: 'Live text' }] } } } } }) as unknown as ConversationView;

test('reasoning, usage, and retry reach version 3 while older views are unchanged', () => {
  const state = empty();
  update(state, thinking(), [], 64000);
  const current = view(state, 3) as Snapshot & Record<string, unknown>;
  assert.deepEqual(current.messages, [
    { id: '1-0', role: 'user', text: 'Fix it', reasoning: '' },
    { id: '2-0', role: 'assistant', text: '', reasoning: 'Read first.' },
    { id: '4-0', role: 'assistant', text: 'Done.', reasoning: 'Now answer.' },
  ]);
  assert.deepEqual(current.tools.map(tool => [tool.id, tool.after, tool.status]), [['read-1', '2-0', 'done'], ['late', '4-0', 'running']]);
  assert(current.tools.every(tool => !('after3' in tool)));
  assert.deepEqual(current.usage, { input: 30, output: 7, cache_read: 0, cache_write: 0, reasoning: 3, total: 37, context_used: 37, context_window: 64000 });
  assert.equal(current.live_reasoning, 'Live thought');
  assert.deepEqual(current.retry, { attempt: 2, at: 1234 });
  assert(!JSON.stringify(current).includes('SECRET') && !JSON.stringify(current).includes('opaque'));
  assert.deepEqual([current.title, current.archived, current.thinking_level, current.queued], ['', false, 'off', []]);

  // Older clients see exactly the text messages and the order a version 2 host kept.
  const second = view(state, 2) as Snapshot;
  assert.deepEqual(second.messages, [{ id: '1-0', role: 'user', text: 'Fix it' }, { id: '4-0', role: 'assistant', text: 'Done.' }]);
  assert.deepEqual(second.tools.map(tool => [tool.id, tool.after]), [['read-1', '1-0'], ['late', '4-0']]);
  assert.deepEqual(Object.keys(second).sort(), ['approval_mode', 'checkout', 'error', 'id', 'live_text', 'messages', 'model', 'pending_approval', 'status', 'tools', 'truncated']);
  const first = view(state, 1) as Snapshot;
  assert.deepEqual(Object.keys(first).sort(), ['checkout', 'error', 'id', 'live_text', 'messages', 'model', 'pending_approval', 'status', 'tools', 'truncated']);
  assert(first.tools.every(tool => !('after' in tool)) && first.messages.every(message => !('reasoning' in message)));
});

test('the turn limit is a distinct version 3 error and the earlier generic one below it', () => {
  const state = empty(); state.status = 'running';
  update(state, { entries: [], docs: { 'pi.live': { runFailure: { reason: 'turn_limit' } } } } as unknown as ConversationView);
  assert.equal((view(state, 3) as Snapshot).error?.code, 'turn_limit');
  for (const protocol of [1, 2] as const) assert.deepEqual((view(state, protocol) as Snapshot).error,
    { code: 'runtime_error', message: 'Agent ended without an answer (turn_limit). Inspect the transcript before retrying.' });
});

// The bound exactly as a version 2 host applied it, on the version 2 shape.
function boundBefore(snapshot: ReturnType<typeof legacy>) {
  while (bytes(snapshot) > 524288) {
    snapshot.truncated = true;
    if (snapshot.messages.length > 1) snapshot.messages.shift();
    else if (snapshot.tools.length) snapshot.tools.shift();
    else if (snapshot.live_text.length) snapshot.live_text = clipTail(snapshot.live_text, Math.floor(snapshot.live_text.length / 2));
    else if (snapshot.messages[0]?.text.length) snapshot.messages[0].text = clipTail(snapshot.messages[0].text, Math.floor(snapshot.messages[0].text.length / 2));
    else throw new Error('Non-transcript snapshot exceeds bound');
  }
  return snapshot;
}
test('oversized snapshots keep the content a version 2 host kept and stay inside the frame', () => {
  const state = empty();
  // Control characters cost six bytes each once escaped.
  for (let index = 0; index < 100; index++) {
    state.messages.push({ id: `m${index}`, role: 'assistant', text: '\u0001'.repeat(1500) + index, reasoning: '\u0002'.repeat(4096) });
    state.messages.push({ id: `r${index}`, role: 'assistant', text: '', reasoning: '\u0003'.repeat(4096) });
    state.tools.push({ id: `t${index}`, name: 'read', status: 'done', input: '{}', output: 'x'.repeat(500), after: `m${index}`, after3: `r${index}` });
  }
  state.live_reasoning = '\u0004'.repeat(16384);
  state.queued = [{ id: 'request-q', request_id: 'q', text: 'y'.repeat(4096), mode: 'follow_up', approval: 'ask' }];
  const expected = boundBefore(structuredClone(legacy(state)));
  boundSnapshot(state);
  assert.deepEqual(legacy(state), expected);
  assert(expected.truncated && expected.messages.length < 100);
  assert(bytes(state) <= 786432, String(bytes(state)));
  assert(bytes({ event: 'snapshot', thread: view(state, 3) }) < 1048576);
  // A tool never points at a reasoning message that was dropped.
  const ids = new Set(state.messages.map(message => message.id));
  assert(state.tools.every(tool => tool.after3 === undefined || ids.has(tool.after3)));
  assert(state.messages.some(message => message.reasoning));
});

function source(entries: unknown[], pending: PendingInput[] = [], placed: [number, string | Placed][] = [], active = false): HistorySource {
  const inputs = placed.map(([entry, input]) => [entry, typeof input === 'string' ? { request_id: input } : input] as [number, Placed]);
  const records = entries as EntryRecord[];
  return {
    async entries(query, limit) {
      const matched = records.filter(entry => Number(entry.id) >= (query.minEntryId ?? 0) && Number(entry.id) <= (query.maxEntryId ?? Infinity));
      return (query.order === 'descending' ? matched.reverse() : matched).slice(0, limit);
    },
    pending: (anchor, rowid, limit) => pending.filter(input => input.anchor < anchor || (input.anchor === anchor && input.rowid < rowid))
      .sort((a, b) => b.anchor - a.anchor || b.rowid - a.rowid).slice(0, limit),
    placed: (low, high) => new Map(inputs.filter(([entry]) => entry >= low && entry <= high)),
    typed: entry => inputs.find(([placedAt]) => placedAt === entry)?.[1].text,
    journal: rowid => pending.find(input => input.rowid === rowid)?.text,
    active,
  };
}
async function all(from: HistorySource, limit: number) {
  const pages: HistoryItem[][] = []; let before: string | undefined;
  for (;;) {
    const page = await historyPage(from, before, limit);
    assert(bytes({ ok: true, result: page }) < 1048576);
    pages.unshift(page.items);
    if (page.before === null) return pages;
    before = page.before;
  }
}
const turns = (count: number) => Array.from({ length: count }, (_, turn) => [
  { id: turn * 4 + 1, model: [{ role: 'user', content: `Question ${turn}` }] },
  { id: turn * 4 + 2, model: [{ role: 'assistant', content: [{ type: 'thinking', thinking: `Think ${turn}` }, { type: 'toolCall', id: `call-${turn}`, name: 'read', arguments: { turn } }] }] },
  { id: turn * 4 + 3, model: [{ role: 'toolResult', toolCallId: `call-${turn}`, content: `Result ${turn}`, isError: turn === 3 }] },
  { id: turn * 4 + 4, model: [{ role: 'assistant', content: [{ type: 'text', text: `Answer ${turn}` }] }] },
]).flat();

test('history pages backward through every message and tool in conversation order', async () => {
  const pending: PendingInput[] = [
    { rowid: 1, request_id: 'never-ran', text: 'Before everything', anchor: 0 },
    { rowid: 9, request_id: 'failed-a', text: 'Failed preflight A', anchor: 20 },
    { rowid: 10, request_id: 'failed-b', text: 'Failed preflight B', anchor: 20 },
    { rowid: 30, request_id: 'last', text: 'Accepted, not placed', anchor: 40 },
  ];
  const from = source(turns(10), pending, [[5, 'second']]);
  const whole = (await historyPage(from, undefined, 500));
  assert.equal(whole.before, null);
  assert.equal(whole.items.length, 10 * 4 + 4);
  assert.deepEqual(whole.items.slice(0, 5), [
    { type: 'message', id: 'request-never-ran', role: 'user', text: 'Before everything', reasoning: '' },
    { type: 'message', id: '1-0', role: 'user', text: 'Question 0', reasoning: '' },
    { type: 'message', id: '2-0', role: 'assistant', text: '', reasoning: 'Think 0' },
    { type: 'tool', id: 'call-0', name: 'read', status: 'done', input: '{"turn":0}', output: 'Result 0' },
    { type: 'message', id: '4-0', role: 'assistant', text: 'Answer 0', reasoning: '' },
  ]);
  const ids = whole.items.map(item => item.id);
  assert.equal(ids[5], 'request-second');
  assert.deepEqual(ids.slice(ids.indexOf('20-0') + 1, ids.indexOf('20-0') + 4), ['request-failed-a', 'request-failed-b', '21-0']);
  assert.equal(ids.at(-1), 'request-last');
  assert.equal(whole.items.find(item => item.id === 'call-3')?.type === 'tool' && (whole.items.find(item => item.id === 'call-3') as { status: string }).status, 'error');
  // Any page size yields the same conversation, including tools whose result is on a newer page.
  for (const limit of [1, 2, 3, 7, 50]) {
    const pages = await all(from, limit);
    assert.deepEqual(pages.flat(), whole.items, `limit ${limit}`);
    if (limit === 1) assert(pages.length > 30);
  }
  const older = await historyPage(from, '9:0:0', 500);
  assert.deepEqual(older.items.map(item => item.id), ids.slice(0, ids.indexOf('9-0')));
});

test('a thread that never ran still pages its accepted inputs', async () => {
  const pending = Array.from({ length: 150 }, (_, index) => ({ rowid: index + 1, request_id: `r${index}`, text: `Input ${index}`, anchor: 0 }));
  const from: HistorySource = { ...source([], pending), entries: undefined };
  const pages = await all(from, 40);
  assert.equal(pages.length, 4);
  assert.deepEqual(pages.flat().map(item => item.id), pending.map(input => `request-${input.request_id}`));
});

test('unfinished tool calls are running only while their run is', async () => {
  const entries = [{ id: 1, model: [{ role: 'assistant', content: [{ type: 'toolCall', id: 'open', name: 'command', arguments: {} }] }] }];
  const status = async (active: boolean) => ((await historyPage(source(entries, [], [], active), undefined, 10)).items[0] as { status: string }).status;
  assert.equal(await status(false), 'interrupted');
  assert.equal(await status(true), 'running');
});

test('oversized items are delivered in bounded parts through history_text', async () => {
  const long = 'BEGIN ' + '🙂ab'.repeat(60000) + ' END';
  const control = '\u0001'.repeat(70000);
  const entries = [
    { id: 1, model: [{ role: 'user', content: 'Short' }] },
    { id: 2, model: [{ role: 'assistant', content: [{ type: 'thinking', thinking: control }, { type: 'text', text: long }, { type: 'toolCall', id: 'big', name: 'write', arguments: { content: long } }] }] },
    { id: 3, model: [{ role: 'toolResult', toolCallId: 'big', content: control }] },
    // Many items at the inline bound in one entry still fit one frame.
    { id: 4, model: Array.from({ length: 6 }, () => ({ role: 'assistant', content: [{ type: 'text', text: control }] })) },
  ];
  const from = source(entries, [{ rowid: 5, request_id: 'huge', text: long, anchor: 4 }]);
  const pages = await all(from, 100);
  const items = pages.flat();
  const read = async (item: HistoryItem, field: 'text' | 'reasoning' | 'input' | 'output') => {
    let value = (item as unknown as Record<string, string>)[field];
    const more = item.more?.find(part => part.field === field);
    if (!more) return value;
    assert.equal(more.offset, value.length);
    for (let offset: number | null = more.offset; offset !== null;) {
      const part = await historyText(from, more.ref, offset);
      assert(bytes({ ok: true, result: part }) < 1048576 && part.text.length <= INLINE_UNITS && part.length === more.length);
      value += part.text; offset = part.next_offset;
    }
    assert.equal(value.length, more.length);
    return value;
  };
  const message = items.find(item => item.id === '2-0')!;
  assert(message.type === 'message' && message.text.length <= INLINE_UNITS && message.text.startsWith('BEGIN '));
  assert.equal(await read(message, 'text'), long);
  assert.equal(await read(message, 'reasoning'), control);
  const tool = items.find(item => item.id === 'big')!;
  assert.equal(await read(tool, 'input'), JSON.stringify({ content: long }));
  assert.equal(await read(tool, 'output'), '\\u0001'.repeat(70000));
  assert.equal(await read(items.find(item => item.id === 'request-huge')!, 'text'), long);
  for (const item of items.filter(item => item.id.startsWith('4-'))) assert.equal(await read(item, 'text'), control);
  assert.equal(items.filter(item => item.id.startsWith('4-')).length, 6);

  await assert.rejects(historyText(from, 'e9.0.text', 0), /not found/);
  await assert.rejects(historyText(from, 'e3.0.text', 0), /not found/);
  await assert.rejects(historyText(from, 'q99', 0), /not found/);
  // Offset 7 falls inside the first emoji of the text.
  await assert.rejects(historyText(from, 'e2.0.text', 7), /splits a character/);
  await assert.rejects(historyText(from, 'e2.0.text', long.length + 1), /outside the text/);
  assert.deepEqual(await historyText(from, 'e2.0.text', long.length), { text: '', offset: long.length, next_offset: null, length: long.length });
});

test('a placed input reads as typed in history, though the model received it expanded', async () => {
  const expanded = `<skill name="notes" location="/s/notes/SKILL.md">\n${'Long skill body. '.repeat(600)}\n</skill>\n\nfor v1.2`;
  const from = source([{ id: 1, model: [{ role: 'user', content: expanded }] }], [], [[1, { request_id: 'typed', text: '/skill:notes for v1.2' }]]);
  const [item] = (await historyPage(from, undefined, 10)).items;
  assert.equal(item.id, 'request-typed');
  assert.equal(item.type === 'message' && item.text, '/skill:notes for v1.2');
  assert.equal((await historyText(from, 'p1', 0)).text, '/skill:notes for v1.2');
});

test('a /skill: input reads back as typed; a template reads back as the model received it', () => {
  assert.ok(readsAsTyped('/skill:notes for v1.2'));
  for (const text of ['/review src/main.rs', 'plain', '', undefined]) assert.ok(!readsAsTyped(text), String(text));
});
