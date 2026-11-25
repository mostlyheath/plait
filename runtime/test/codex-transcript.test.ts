import test from 'node:test';
import assert from 'node:assert/strict';
import type { ConversationView } from '@earendil-works/pi-durable';
import { update } from '../src/transcript.ts';
import type { InputRecord, Snapshot } from '../src/protocol.ts';
import { CodexTranscript, isTerminalTurn } from '../src/codex/transcript.ts';
import type { Entry } from '../src/external/entries.ts';
import { threadOf } from '../src/codex/protocol.ts';

const threadId = 'thread-1';
const turnId = 'turn-1';
const blank = (): Snapshot => ({
  id: 't', checkout: '/work', model: { provider: 'codex', model_id: 'gpt-fake' }, status: 'running',
  messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false,
});
function render(transcript: CodexTranscript, inputs: InputRecord[] = []) {
  const snapshot = blank();
  update(snapshot, transcript.view() as unknown as ConversationView, inputs, null);
  return snapshot;
}
const started = (item: object) => ['item/started', { threadId, turnId, item, startedAtMs: 1 }] as const;
const completed = (item: object) => ['item/completed', { threadId, turnId, item, completedAtMs: 2 }] as const;
const delta = (method: string, itemId: string, text: string, extra: object = {}) => [method, { threadId, turnId, itemId, delta: text, ...extra }] as const;
const turn = (status: string, error: object | null = null) => ({ id: turnId, items: [], status, error, startedAt: 1, completedAt: null, durationMs: null });
const userItem = (text: string, clientId: string | null, id = 'user-1') => ({ type: 'userMessage', id, clientId, content: [{ type: 'text', text, text_elements: [] }] });
const command = (status: string, extra: object = {}) => ({
  type: 'commandExecution', id: 'cmd-1', command: 'npm test', cwd: '/work', status, aggregatedOutput: null, exitCode: null, durationMs: null, ...extra,
});
const breakdown = (total: number) => ({ totalTokens: total, inputTokens: total - 100, cachedInputTokens: 300, cacheWriteInputTokens: 10, outputTokens: 100, reasoningOutputTokens: 40 });

test('a scripted turn renders through update() like a Pi run', () => {
  const transcript = new CodexTranscript([], 1);
  const persisted: Entry[] = [];
  const placed: { clientId: string; entry: number }[] = [];
  const step = ([method, params]: readonly [string, object]) => {
    const result = transcript.apply(method, params);
    persisted.push(...result.append); placed.push(...result.placed);
    return result;
  };
  step(['turn/started', { threadId, turn: turn('inProgress') }]);
  assert.equal(transcript.activeTurn(), turnId);
  // A user message arrives as started and completed; it is persisted once.
  step(started(userItem('Run the tests', 'req-1')));
  step(completed(userItem('Run the tests', 'req-1')));
  assert.equal(persisted.length, 1);
  assert.deepEqual(placed, [{ clientId: 'req-1', entry: 1 }]);

  step(started({ type: 'reasoning', id: 'rs-1', summary: [], content: [] }));
  step(delta('item/reasoning/summaryTextDelta', 'rs-1', 'Planning ', { summaryIndex: 0 }));
  assert.equal(step(delta('item/reasoning/summaryTextDelta', 'rs-1', 'the run.', { summaryIndex: 0 })).changed, true);
  assert.equal(render(transcript).live_reasoning, 'Planning the run.');
  step(completed({ type: 'reasoning', id: 'rs-1', summary: ['Planning the run.'], content: [] }));
  // Completed reasoning stays live until a message carries it.
  assert.equal(persisted.length, 1);
  assert.equal(render(transcript).live_reasoning, 'Planning the run.');

  assert.equal(step(started(command('inProgress'))).append.length, 1);
  let snapshot = render(transcript);
  assert.equal(snapshot.live_reasoning, '');
  assert.deepEqual(snapshot.tools, [{ id: 'cmd-1', name: 'command', status: 'running', input: '{"command":"npm test","cwd":"/work"}', output: '', after: '1-0', after3: '2-0' }]);
  snapshot = render(transcript, [{ request_id: 'req-1', text: 'Run the tests', entry: 1, anchor: 0 }]);
  assert.deepEqual(snapshot.messages, [
    { id: 'request-req-1', role: 'user', text: 'Run the tests' },
    { id: '2-0', role: 'assistant', text: '', reasoning: 'Planning the run.' },
  ]);
  assert.equal(snapshot.tools[0].after, 'request-req-1');

  step(delta('item/commandExecution/outputDelta', 'cmd-1', 'ok 1\n'));
  step(delta('item/commandExecution/outputDelta', 'cmd-1', 'ok 2\n'));
  assert.equal(render(transcript).tools[0].output, 'ok 1\nok 2\n');
  assert.equal(render(transcript).tools[0].status, 'running');
  step(completed(command('completed', { aggregatedOutput: 'ok 1\nok 2\n', exitCode: 0 })));
  snapshot = render(transcript);
  assert.equal(snapshot.tools[0].status, 'done');
  assert.equal(snapshot.tools[0].output, 'ok 1\nok 2\n');

  step(started({ type: 'agentMessage', id: 'msg-1', text: '' }));
  step(delta('item/agentMessage/delta', 'msg-1', 'All '));
  step(delta('item/agentMessage/delta', 'msg-1', 'passed.'));
  assert.equal(render(transcript).live_text, 'All passed.');
  step(completed({ type: 'agentMessage', id: 'msg-1', text: 'All passed.' }));
  assert.equal(render(transcript).live_text, '');
  assert.equal(step(['thread/tokenUsage/updated', { threadId, turnId, tokenUsage: { total: breakdown(5000), last: breakdown(2000), modelContextWindow: 272000 } }]).changed, true);
  step(['turn/completed', { threadId, turn: turn('completed') }]);
  assert.equal(transcript.activeTurn(), null);

  snapshot = render(transcript);
  assert.deepEqual(snapshot.messages.map(({ role, text }) => [role, text]), [['user', 'Run the tests'], ['assistant', ''], ['assistant', 'All passed.']]);
  assert.equal(snapshot.usage, undefined, 'update() leaves usage to the adapter');
  assert.deepEqual(transcript.usage(null), {
    input: 1590, output: 100, cache_read: 300, cache_write: 10, reasoning: 40, total: 2000, context_used: 2000, context_window: 272000,
  });
  // Entries are append-only: call, result, and each message once, in order.
  assert.deepEqual(persisted.map(entry => entry.id), [1, 2, 3, 4]);
  assert.deepEqual(persisted.map(entry => entry.model[0].role), ['user', 'assistant', 'toolResult', 'assistant']);
});

test('reasoning before an answer is stored in the same assistant message', () => {
  const transcript = new CodexTranscript([], 10);
  transcript.apply(...completed({ type: 'reasoning', id: 'rs', summary: ['First.', 'Second.'], content: ['raw'] }));
  const { append } = transcript.apply(...completed({ type: 'agentMessage', id: 'm', text: 'Done.' }));
  assert.deepEqual(append, [{ id: 10, kind: 'message', model: [{ role: 'assistant', stopReason: 'stop', content: [
    { type: 'thinking', thinking: 'First.\n\nSecond.' }, { type: 'text', text: 'Done.' },
  ] }] }]);
  const [message] = render(transcript).messages;
  assert.deepEqual(message, { id: '10-0', role: 'assistant', text: 'Done.', reasoning: 'First.\n\nSecond.' });
});

test('raw reasoning text is used when there is no summary, and trailing reasoning is kept at turn end', () => {
  const transcript = new CodexTranscript([], 1);
  transcript.apply(...delta('item/reasoning/textDelta', 'rs', 'thinking hard', { contentIndex: 0 }));
  assert.equal(render(transcript).live_reasoning, 'thinking hard');
  transcript.apply(...completed({ type: 'reasoning', id: 'rs', summary: [], content: [] }));
  const { append } = transcript.apply('turn/completed', { threadId, turn: turn('interrupted') });
  assert.deepEqual(append[0].model, [{ role: 'assistant', content: [{ type: 'thinking', thinking: 'thinking hard' }] }]);
  assert.equal(render(transcript).live_reasoning, '');
});

test('failed, declined, and unfinished commands read as errors', () => {
  const transcript = new CodexTranscript([], 1);
  transcript.apply(...started(command('inProgress')));
  transcript.apply(...completed(command('failed', { aggregatedOutput: 'boom\n', exitCode: 2 })));
  transcript.apply(...started({ ...command('inProgress'), id: 'cmd-2' }));
  transcript.apply(...completed({ ...command('declined'), id: 'cmd-2' }));
  transcript.apply(...started({ ...command('inProgress'), id: 'cmd-3' }));
  transcript.apply(...delta('item/commandExecution/outputDelta', 'cmd-3', 'partial'));
  const end = transcript.apply('turn/completed', { threadId, turn: turn('interrupted') });
  assert.equal(end.append.length, 1);
  // A late completion after the turn closed it is not persisted again.
  assert.deepEqual(transcript.apply(...completed({ ...command('completed'), id: 'cmd-3' })).append, []);
  const tools = render(transcript).tools.map(({ id, status, output }) => ({ id, status, output }));
  assert.deepEqual(tools, [
    { id: 'cmd-1', status: 'error', output: 'boom\n\n[exit code 2]' },
    { id: 'cmd-2', status: 'error', output: 'Declined.' },
    { id: 'cmd-3', status: 'error', output: 'The turn ended before this finished.' },
  ]);
});

test('edits, MCP calls, and web searches become named tools', () => {
  const transcript = new CodexTranscript([], 1);
  const edit = (status: string) => ({ type: 'fileChange', id: 'e1', status, changes: [
    { path: '/work/a.ts', kind: { type: 'update', move_path: null }, diff: '@@\n-a\n+b\n+c\n' },
    { path: '/work/b.ts', kind: { type: 'add' }, diff: '+new\n' },
  ] });
  transcript.apply(...started(edit('inProgress')));
  assert.equal(transcript.edit('e1')?.changes.length, 2);
  transcript.apply(...completed(edit('completed')));
  assert.equal(transcript.edit('e1'), undefined);
  transcript.apply(...started({ type: 'mcpToolCall', id: 'm1', server: 'docs', tool: 'search', status: 'inProgress', arguments: { q: 'x' }, result: null, error: null }));
  transcript.apply(...completed({ type: 'mcpToolCall', id: 'm1', server: 'docs', tool: 'search', status: 'completed', arguments: { q: 'x' },
    result: { content: [{ type: 'text', text: 'found it' }], structuredContent: null }, error: null }));
  transcript.apply(...completed({ type: 'mcpToolCall', id: 'm2', server: 'docs', tool: 'fetch', status: 'failed', arguments: {}, result: null, error: { message: 'offline' } }));
  transcript.apply(...started({ type: 'webSearch', id: 'w1', query: '', action: null }));
  transcript.apply(...completed({ type: 'webSearch', id: 'w1', query: 'gpui docs', action: { type: 'search', query: 'gpui docs', queries: null } }));
  const tools = render(transcript).tools.map(({ id, name, status, input, output }) => ({ id, name, status, input, output }));
  assert.deepEqual(tools, [
    { id: 'e1', name: 'edit', status: 'done', input: '{"paths":["/work/a.ts","/work/b.ts"],"summary":"2 files, +3 -1"}', output: '@@\n-a\n+b\n+c\n\n+new\n' },
    { id: 'm1', name: 'mcp:docs/search', status: 'done', input: '{"q":"x"}', output: 'found it' },
    { id: 'm2', name: 'mcp:docs/fetch', status: 'error', input: '{}', output: 'offline' },
    { id: 'w1', name: 'web_search', status: 'done', input: '{"query":"gpui docs"}', output: 'Searched for gpui docs' },
  ]);
});

test('a resumed transcript does not persist known tools again', () => {
  const entries: Entry[] = [
    { id: 1, kind: 'message', model: [{ role: 'assistant', content: [{ type: 'toolCall', id: 'cmd-1', name: 'command', arguments: {} }] }] },
  ];
  const transcript = new CodexTranscript(entries, 2);
  assert.deepEqual(transcript.apply(...started(command('inProgress'))).append, []);
  const done = transcript.apply(...completed(command('completed', { aggregatedOutput: 'x', exitCode: 0 })));
  assert.deepEqual(done.append.map(entry => entry.model[0].role), ['toolResult']);
  assert.deepEqual(transcript.apply(...completed(command('completed', { aggregatedOutput: 'x', exitCode: 0 }))).append, []);
});

test('the view keeps the last 100 entries and steered messages are placed', () => {
  const transcript = new CodexTranscript([], 1);
  for (let index = 0; index < 130; index++) transcript.apply(...completed({ type: 'agentMessage', id: `m${index}`, text: `reply ${index}` }));
  const { entries } = transcript.view();
  assert.equal(entries.length, 100);
  assert.equal(entries[0].id, 31);
  const steered = transcript.apply(...completed(userItem('also lint', 'req-2', 'user-2')));
  assert.deepEqual(steered.placed, [{ clientId: 'req-2', entry: 131 }]);
  assert.deepEqual(transcript.apply(...completed(userItem('typed elsewhere', null, 'user-3'))).placed, []);
});

test('unknown items and notifications change nothing', () => {
  const transcript = new CodexTranscript([], 1);
  assert.deepEqual(transcript.apply(...completed({ type: 'plan', id: 'p', text: 'x' })), { append: [], changed: false, placed: [] });
  assert.deepEqual(transcript.apply('thread/status/changed', { threadId }), { append: [], changed: false, placed: [] });
  assert.deepEqual(transcript.apply('item/agentMessage/delta', null), { append: [], changed: false, placed: [] });
  assert.equal(transcript.usage(128000), null);
  assert.deepEqual(transcript.view(), { entries: [], docs: {} });
});

test('usage falls back to the given context window', () => {
  const transcript = new CodexTranscript([], 1);
  transcript.apply('thread/tokenUsage/updated', { threadId, turnId, tokenUsage: { total: breakdown(900), last: breakdown(900), modelContextWindow: null } });
  assert.equal(transcript.usage(128000)?.context_window, 128000);
});

test('isTerminalTurn and threadOf read notification params', () => {
  assert.equal(isTerminalTurn('turn/started', { threadId, turn: turn('inProgress') }), undefined);
  assert.deepEqual(isTerminalTurn('turn/completed', { threadId, turn: turn('completed') }), { turnId, status: 'completed' });
  assert.deepEqual(isTerminalTurn('turn/completed', { threadId, turn: turn('failed', { message: 'Usage limit reached.', additionalDetails: null }) }),
    { turnId, status: 'failed', error: 'Usage limit reached.' });
  assert.equal(isTerminalTurn('turn/completed', {}), undefined);
  assert.equal(threadOf({ threadId, turnId }), threadId);
  assert.equal(threadOf({ thread: { id: threadId } }), undefined);
  assert.equal(threadOf(null), undefined);
});
