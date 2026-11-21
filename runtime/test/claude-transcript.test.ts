import test from 'node:test';
import assert from 'node:assert/strict';
import type { ConversationView } from '@earendil-works/pi-durable';
import { update } from '../src/transcript.ts';
import type { InputRecord, Snapshot } from '../src/protocol.ts';
import { ClaudeTranscript, isTurnEnd, toolCall } from '../src/claude/transcript.ts';
import type { Entry, Placed } from '../src/external/entries.ts';

const session = '6f1c2a4e-0000-4000-8000-000000000009';
const U1 = '00000000-0000-4000-8000-0000000000a1';
const U2 = '00000000-0000-4000-8000-0000000000a2';
const usage = { input_tokens: 1000, output_tokens: 50, cache_read_input_tokens: 200, cache_creation_input_tokens: 10 };
const blank = (): Snapshot => ({
  id: 't', checkout: '/work', model: { provider: 'claude', model_id: 'opus' }, status: 'running',
  messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false,
});
function render(transcript: ClaudeTranscript, inputs: InputRecord[] = []) {
  const snapshot = blank();
  update(snapshot, transcript.view() as unknown as ConversationView, inputs, null);
  return snapshot;
}
let counter = 0;
const base = (extra: object = {}) => ({ uuid: `00000000-0000-4000-8000-${String(++counter).padStart(12, '0')}`, session_id: session, ...extra });
const init = (mode = 'default') => ({ type: 'system', subtype: 'init', permissionMode: mode, model: 'claude-opus-5-5', capabilities: ['interrupt_cancel_queued_v1'], ...base() });
const replay = (uuid: string, text: string) => ({ type: 'user', message: { role: 'user', content: text }, parent_tool_use_id: null, isReplay: true, uuid, session_id: session });
const event = (value: object, extra: object = {}) => ({ type: 'stream_event', event: value, parent_tool_use_id: null, ...base(extra) });
const delta = (kind: 'text' | 'thinking', text: string) => event({ type: 'content_block_delta', index: 0, delta: kind === 'text' ? { type: 'text_delta', text } : { type: 'thinking_delta', thinking: text } });
const assistant = (block: object, extra: object = {}) => ({
  type: 'assistant', parent_tool_use_id: null, ...base(extra),
  message: { id: 'msg_1', type: 'message', role: 'assistant', model: 'claude-opus-5-5', content: [block], stop_reason: null, usage },
});
const toolResult = (id: string, content: unknown, isError = false, parent: string | null = null) => ({
  type: 'user', parent_tool_use_id: parent, ...base(), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content, is_error: isError }] },
});
const result = (extra: object = {}) => ({
  type: 'result', subtype: 'success', is_error: false, result: 'done', num_turns: 1, duration_ms: 1, duration_api_ms: 1, stop_reason: 'end_turn',
  total_cost_usd: 0, usage, permission_denials: [], terminal_reason: 'completed', queued_turn_count: 0,
  modelUsage: { 'claude-opus-5-5': { inputTokens: 1, outputTokens: 1, cacheReadInputTokens: 0, cacheCreationInputTokens: 0, webSearchRequests: 0, costUSD: 0, contextWindow: 1000000, maxOutputTokens: 128000 } },
  ...base(), ...extra,
});

function driver(transcript = new ClaudeTranscript([], 1)) {
  const persisted: Entry[] = []; const placed: Placed[] = [];
  const step = (message: object) => {
    const applied = transcript.apply(message as never);
    persisted.push(...applied.append); placed.push(...applied.placed);
    return applied;
  };
  return { transcript, persisted, placed, step };
}

test('a scripted turn renders through update() like a Pi run', () => {
  const { transcript, persisted, placed, step } = driver();
  transcript.sent(U1, 'req-1', 'Run the tests');
  step(init('auto'));
  assert.equal(transcript.sessionId(), session);
  assert.equal(transcript.permissionMode(), 'auto');
  assert.deepEqual(transcript.capabilityList(), ['interrupt_cancel_queued_v1']);
  step(replay(U1, 'Run the tests'));
  // A replay of the same message, or an unknown one, places nothing more.
  step(replay(U1, 'Run the tests'));
  step(replay(U2, 'Not ours'));
  assert.deepEqual(placed, [{ clientId: 'req-1', entry: 1 }]);
  assert.deepEqual(transcript.unplaced(), []);

  step(event({ type: 'message_start', message: {} }));
  step(event({ type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: '' } }));
  step(delta('thinking', 'Planning '));
  assert.equal(step(delta('thinking', 'the run.')).changed, true);
  assert.equal(render(transcript).live_reasoning, 'Planning the run.');
  step(assistant({ type: 'thinking', thinking: 'Planning the run.', signature: 's' }));
  // Thinking waits for the message it led to.
  assert.equal(persisted.length, 1);
  assert.equal(render(transcript).live_reasoning, 'Planning the run.');

  assert.equal(step(assistant({ type: 'tool_use', id: 'toolu_1', name: 'Bash', input: { command: 'npm test', description: 'Run tests' } })).append.length, 1);
  let snapshot = render(transcript, [{ request_id: 'req-1', text: 'Run the tests', entry: 1, anchor: 0 }]);
  assert.equal(snapshot.live_reasoning, '');
  assert.deepEqual(snapshot.messages, [
    { id: 'request-req-1', role: 'user', text: 'Run the tests' },
    { id: '2-0', role: 'assistant', text: '', reasoning: 'Planning the run.' },
  ]);
  assert.deepEqual(snapshot.tools, [{ id: 'toolu_1', name: 'command', status: 'running', input: '{"command":"npm test","description":"Run tests"}', output: '', after: 'request-req-1', after3: '2-0' }]);

  step(toolResult('toolu_1', [{ type: 'text', text: 'ok 1' }, { type: 'text', text: 'ok 2' }]));
  snapshot = render(transcript);
  assert.equal(snapshot.tools[0].status, 'done');
  assert.equal(snapshot.tools[0].output, 'ok 1\nok 2');

  step(event({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }));
  step(delta('text', 'All '));
  step(delta('text', 'passed.'));
  assert.equal(render(transcript).live_text, 'All passed.');
  step(assistant({ type: 'text', text: 'All passed.' }));
  snapshot = render(transcript);
  assert.equal(snapshot.live_text, '');
  assert.equal(snapshot.messages.at(-1)?.text, 'All passed.');

  const end = result();
  assert.deepEqual(isTurnEnd(end as never), { status: 'completed', queued: 0 });
  step(end);
  assert.deepEqual(transcript.usage(null), {
    input: 1000, output: 50, cache_read: 200, cache_write: 10, reasoning: null, total: 1260, context_used: 1260, context_window: 1000000,
  });
  assert.deepEqual(persisted.map(entry => entry.id), [1, 2, 3, 4]);
});

test('steering messages are placed where Claude Code took them', () => {
  const { transcript, placed, step } = driver();
  transcript.sent(U1, 'req-1', 'Start');
  step(init()); step(replay(U1, 'Start'));
  step(assistant({ type: 'tool_use', id: 'toolu_1', name: 'Read', input: { file_path: '/work/a.ts' } }));
  transcript.sent(U2, 'req-2', 'Also check b.ts');
  assert.deepEqual(transcript.unplaced(), ['req-2']);
  step(toolResult('toolu_1', 'contents'));
  step(replay(U2, 'Also check b.ts'));
  step(assistant({ type: 'text', text: 'Both look fine.' }));
  step(result({ user_message_uuids: [U1, U2] }));
  assert.deepEqual(placed, [{ clientId: 'req-1', entry: 1 }, { clientId: 'req-2', entry: 4 }]);
  const snapshot = render(transcript, [
    { request_id: 'req-1', text: 'Start', entry: 1, anchor: 0 }, { request_id: 'req-2', text: 'Also check b.ts', entry: 4, anchor: 0 },
  ]);
  assert.deepEqual(snapshot.messages.map(message => message.id), ['request-req-1', 'request-req-2', '5-0']);
  assert.equal(snapshot.tools[0].name, 'read');
  assert.equal(snapshot.tools[0].after, 'request-req-1');
});

test('without echoes, reply frames that name a send place it', () => {
  const { transcript, placed, step } = driver();
  transcript.sent(U1, 'req-1', 'Hello');
  step(init());
  step(event({ type: 'message_start', message: {} }, { user_message_uuid: U1, user_message_uuids: [U1] }));
  assert.deepEqual(placed, [{ clientId: 'req-1', entry: 1 }]);
  transcript.sent(U2, 'req-2', 'And this');
  step(assistant({ type: 'text', text: 'Hi.' }));
  // A message folded in without an echo is placed by the result at the latest.
  step(result({ user_message_uuids: [U1, U2] }));
  assert.deepEqual(placed.map(item => item.clientId), ['req-1', 'req-2']);
  assert.deepEqual(transcript.unplaced(), []);
});

test('turn ends: interrupted, failed, queued, and a tool left open', () => {
  assert.deepEqual(isTurnEnd(result({ subtype: 'error_during_execution', is_error: true, errors: [], terminal_reason: 'aborted_streaming' }) as never), { status: 'interrupted', queued: 0 });
  assert.deepEqual(isTurnEnd(result({ subtype: 'error_during_execution', is_error: true, errors: ['Boom', 'again'], terminal_reason: 'model_error' }) as never),
    { status: 'failed', queued: 0, error: 'Boom; again' });
  assert.deepEqual(isTurnEnd(result({ is_error: true, result: 'API Error: 529' }) as never), { status: 'failed', queued: 0, error: 'API Error: 529' });
  assert.deepEqual(isTurnEnd(result({ queued_turn_count: 2 }) as never), { status: 'completed', queued: 2 });
  assert.equal(isTurnEnd(assistant({ type: 'text', text: 'x' }) as never), undefined);
  assert.ok(isTurnEnd(result({ subtype: 'error_max_turns', is_error: true, errors: ['x'.repeat(5000)] }) as never)!.error!.length <= 1024);

  const { transcript, step } = driver();
  step(assistant({ type: 'thinking', thinking: 'Hmm.' }));
  step(assistant({ type: 'tool_use', id: 'toolu_9', name: 'Bash', input: { command: 'sleep 100' } }));
  step(delta('text', 'Partial'));
  const ended = step(result({ subtype: 'error_during_execution', is_error: true, terminal_reason: 'aborted_tools' }));
  assert.equal(ended.append.length, 1);
  const snapshot = render(transcript);
  assert.equal(snapshot.tools[0].status, 'error');
  assert.equal(snapshot.tools[0].output, 'The turn ended before this finished.');
  assert.equal(snapshot.live_text, '');

  // An exit without a result ends the turn the same way, and thinking with no message still persists.
  const exited = driver();
  exited.step(assistant({ type: 'thinking', thinking: 'Only thinking.' }));
  exited.step(assistant({ type: 'tool_use', id: 'toolu_x', name: 'Edit', input: { file_path: '/work/a.ts', old_string: 'a', new_string: 'b' } }));
  exited.step(assistant({ type: 'thinking', thinking: 'Then this.' }));
  const finished = exited.transcript.finish();
  assert.equal(finished.append.length, 2);
  assert.equal(render(exited.transcript).messages.at(-1)?.reasoning, 'Then this.');
  assert.equal(render(exited.transcript).tools[0].status, 'error');
});

test('subagent frames, synthetic text, and empty thinking leave no rows', () => {
  const { transcript, persisted, step } = driver();
  step({ ...assistant({ type: 'tool_use', id: 'toolu_sub', name: 'Bash', input: { command: 'ls' } }), parent_tool_use_id: 'toolu_agent' });
  step(toolResult('toolu_sub', 'x', false, 'toolu_agent'));
  step({ ...delta('text', 'sub text'), parent_tool_use_id: 'toolu_agent' });
  step({ type: 'user', parent_tool_use_id: null, isSynthetic: true, ...base(), message: { role: 'user', content: [{ type: 'text', text: 'expanded skill' }] } });
  step(assistant({ type: 'thinking', thinking: '', signature: 's' }));
  step(assistant({ type: 'redacted_thinking', data: 'opaque' }));
  assert.equal(persisted.length, 0);
  assert.equal(render(transcript).live_text, '');
  step({ type: 'system', subtype: 'status', status: null, permissionMode: 'plan', ...base() });
  assert.equal(transcript.permissionMode(), 'plan');
  step({ type: 'system', subtype: 'local_command_output', content: 'Total cost: $0.00', ...base() });
  assert.equal(render(transcript).messages.at(-1)?.text, 'Total cost: $0.00');
});

test('tool rows use the names the desktop reads, with bounded arguments and results', () => {
  assert.deepEqual(toolCall('Edit', { file_path: '/w/a.ts', old_string: 'x'.repeat(100), new_string: 'y' }), { name: 'edit', arguments: { path: '/w/a.ts', tool: 'Edit' } });
  assert.deepEqual(toolCall('Write', { file_path: '/w/b.ts', content: 'héllo' }), { name: 'write', arguments: { path: '/w/b.ts', bytes: 6 } });
  assert.deepEqual(toolCall('Read', { file_path: '/w/c.ts', offset: 10 }), { name: 'read', arguments: { path: '/w/c.ts', offset: 10 } });
  assert.deepEqual(toolCall('WebSearch', { query: 'q' }), { name: 'web_search', arguments: { query: 'q' } });
  assert.equal(toolCall('mcp__github__create_issue', { title: 't' }).name, 'mcp:github/create_issue');
  assert.equal((toolCall('Grep', { pattern: 'z'.repeat(20000) }).arguments as { pattern: string }).pattern.length, 8193);

  const { transcript, step } = driver();
  step(assistant({ type: 'tool_use', id: 'toolu_big', name: 'Bash', input: { command: 'cat big' } }));
  step(toolResult('toolu_big', `${'a'.repeat(300_000)}END`, true));
  const stored = transcript.view().entries.at(-1)!.model[0];
  const text = (stored.content as { text: string }[])[0].text;
  assert.ok(text.startsWith('[Earlier output omitted]\n'));
  assert.ok(text.endsWith('END'));
  assert.ok(text.length <= 262144 + 30);
  assert.equal(stored.isError, true);
  assert.equal(render(transcript).tools[0].status, 'error');
});

test('live text keeps a bounded tail however long the stream runs', () => {
  const { transcript, step } = driver();
  const chunk = 'x'.repeat(1000);
  for (let index = 0; index < 300; index++) step(delta('text', chunk));
  step(delta('text', 'TAIL'));
  const live = transcript.view().docs['pi.live'] as { generation: { message: { content: { text: string }[] } } };
  const text = live.generation.message.content[0].text;
  assert.ok(text.length <= 65536);
  assert.ok(text.endsWith('TAIL'));
  assert.equal(render(transcript).live_text.length, 16384);
});

test('a restarted transcript keeps answered tools answered', () => {
  const first = driver();
  first.step(assistant({ type: 'tool_use', id: 'toolu_1', name: 'Bash', input: { command: 'ls' } }));
  first.step(toolResult('toolu_1', 'a'));
  const restarted = driver(new ClaudeTranscript(first.persisted, 3));
  restarted.step(assistant({ type: 'tool_use', id: 'toolu_1', name: 'Bash', input: { command: 'ls' } }));
  restarted.step(toolResult('toolu_1', 'again'));
  assert.equal(restarted.persisted.length, 0);
  restarted.step(assistant({ type: 'text', text: 'Next.' }));
  assert.deepEqual(restarted.persisted.map(entry => entry.id), [3]);
  assert.deepEqual(restarted.transcript.drop([U1]), []);
  restarted.transcript.sent(U1, 'req-9', 'x');
  assert.deepEqual(restarted.transcript.drop([U1, U2]), ['req-9']);
});

test('translation stays well under the per-event budget on a replayed stream', () => {
  // A stream shaped like a long Claude Code turn: many text and thinking deltas, tool calls and results.
  const events: object[] = [init()];
  for (let round = 0; round < 40; round++) {
    events.push(event({ type: 'message_start', message: {} }));
    for (let index = 0; index < 40; index++) events.push(delta('thinking', 'reasoning words '));
    events.push(assistant({ type: 'thinking', thinking: 'reasoning words '.repeat(40) }));
    events.push(assistant({ type: 'tool_use', id: `toolu_${round}`, name: 'Bash', input: { command: `cargo test ${round}` } }));
    events.push(toolResult(`toolu_${round}`, 'ok\n'.repeat(200)));
    for (let index = 0; index < 60; index++) events.push(delta('text', 'answer text '));
    events.push(assistant({ type: 'text', text: 'answer text '.repeat(60) }));
  }
  events.push(result());
  const transcript = new ClaudeTranscript([], 1);
  const started = performance.now();
  for (const message of events) transcript.apply(message as never);
  const applyUs = (performance.now() - started) * 1000 / events.length;
  // The runtime renders at most every 16 ms while streaming and at once on each finished item; time the worst case, a render per event.
  const second = new ClaudeTranscript([], 1);
  const snapshot = blank();
  const renderStarted = performance.now();
  for (const message of events) {
    second.apply(message as never);
    update(snapshot, second.view() as unknown as ConversationView, [], null);
  }
  const renderUs = (performance.now() - renderStarted) * 1000 / events.length;
  console.log(`claude transcript: ${events.length} events, apply ${applyUs.toFixed(2)} µs/event, apply+update ${renderUs.toFixed(1)} µs/event`);
  assert.ok(events.length > 4000);
  assert.ok(applyUs < 5000 && renderUs < 5000, 'under the 5 ms per event target');
});
