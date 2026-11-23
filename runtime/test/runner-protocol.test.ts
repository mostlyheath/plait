import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { COMMAND_LIMIT, EVENT_LIMIT, encodeFrame, EventBuffer, EventSequence, FrameDecoder, parseCommand, parseRunnerMessage, receiptEvents, type Event } from '../src/runner/protocol.ts';

test('frames split UTF-8 and validate command types and limits', () => {
  const decoder = new FrameDecoder(COMMAND_LIMIT, parseCommand);
  const frame = encodeFrame({ type: 'steer', request_id: 'r', text: 'é', images: [] }, COMMAND_LIMIT);
  assert.deepEqual([...decoder.push(frame.subarray(0, frame.length - 3)), ...decoder.push(frame.subarray(frame.length - 3))], [{ type: 'steer', request_id: 'r', text: 'é', images: [] }]);
  assert.throws(() => parseCommand({ type: 'unknown' }));
  assert.throws(() => encodeFrame({ type: 'start', payload: 'x'.repeat(COMMAND_LIMIT) }, COMMAND_LIMIT));
  assert.throws(() => new FrameDecoder(COMMAND_LIMIT, parseCommand).push(Buffer.alloc(COMMAND_LIMIT + 1)));
  assert.throws(() => encodeFrame({ type: 'live', seq: 1, text: 'x'.repeat(EVENT_LIMIT) }, EVENT_LIMIT));
});

test('events require increasing sequence numbers', () => {
  const seq = new EventSequence();
  const event = parseRunnerMessage({ type: 'phase', seq: 1, state: 'running' });
  if (event.type === 'attached') throw new Error('expected event');
  seq.accept(event);
  assert.throws(() => seq.accept(event));
  assert.throws(() => parseRunnerMessage({ type: 'unknown', seq: 2 }));
});

test('replay retains events until acknowledged and rejects gaps in retained history', () => {
  const buffer = new EventBuffer();
  buffer.publish({ type: 'phase', seq: 1, state: 'preparing' });
  buffer.publish({ type: 'phase', seq: 2, state: 'running' });
  assert.deepEqual(buffer.after(0).map(event => event.seq), [1, 2]);
  buffer.ack(1);
  buffer.ack(1);
  assert.deepEqual(buffer.after(1).map(event => event.seq), [2]);
  assert.throws(() => buffer.after(0));
  assert.throws(() => buffer.ack(3));
  buffer.ack(2);
  assert.throws(() => buffer.publish({ type: 'phase', seq: 2, state: 'running' }));
});

test('event payloads reject missing required fields', () => {
  for (const type of ['phase', 'live', 'commit', 'approval_needed', 'postimage', 'record_before', 'record_after', 'finished']) {
    assert.throws(() => parseRunnerMessage({ type, seq: 1 }));
  }
});

test('commands and events reject wrong field types', () => {
  const invalidCommands = [
    { type: 'attach', protocol: 1, last_seq: -1 }, { type: 'start', payload: null },
    { type: 'approve', approval_id: 'a', allow: 'yes' },
    { type: 'steer', request_id: 'r', text: '', images: [null] },
    { type: 'set_mode', mode: 1 }, { type: 'ack', seq: 1.5 },
  ];
  for (const command of invalidCommands) assert.throws(() => parseCommand(command));
  const invalidEvents = [
    { type: 'attached', seq: 1, state: 'running', snapshot: {}, pending_approval: null, head: 42, before_record_id: null },
    { type: 'phase', seq: 1, state: 'idle' }, { type: 'live', seq: 1, payload: null },
    { type: 'commit', seq: 1, item: {}, head: null, placed: [1], changed_paths: [], usage: null },
    { type: 'approval_needed', seq: 1, approval: { id: 'a' } },
    { type: 'postimage', seq: 1, postimage: 1 }, { type: 'record_before', seq: 1, record: null },
    { type: 'record_after', seq: 1, record: 'id' }, { type: 'finished', seq: 1, result: false },
  ];
  for (const event of invalidEvents) assert.throws(() => parseRunnerMessage(event));
  assert.deepEqual(parseCommand({ type: 'stop' }), { type: 'stop' });
});


test('recovery protocol additions validate and obey framing and sequence limits', () => {
  const ping = { type: 'ping', nonce: 'nonce' };
  assert.deepEqual(new FrameDecoder(COMMAND_LIMIT, parseCommand).push(encodeFrame(ping, COMMAND_LIMIT)), [ping]);
  assert.throws(() => parseCommand({ type: 'ping', nonce: 3 }));
  assert.throws(() => encodeFrame({ type: 'ping', nonce: 'x'.repeat(COMMAND_LIMIT) }, COMMAND_LIMIT));
  const events = [
    { type: 'approval_settled', seq: 1, id: 'approval' },
    { type: 'forget', seq: 2, path: 'file' },
    { type: 'pong', seq: 3, nonce: 'nonce' },
  ];
  const sequence = new EventSequence();
  const decoder = new FrameDecoder(EVENT_LIMIT, parseRunnerMessage);
  for (const event of events) {
    const [parsed] = decoder.push(encodeFrame(event, EVENT_LIMIT));
    assert.deepEqual(parsed, event);
    if (parsed.type === 'attached') throw new Error('expected event');
    sequence.accept(parsed);
    assert.throws(() => sequence.accept(parsed));
    const field = event.type === 'approval_settled' ? 'id' : event.type === 'forget' ? 'path' : 'nonce';
    assert.throws(() => parseRunnerMessage({ ...event, [field]: null }));
    assert.throws(() => encodeFrame({ ...event, [field]: 'x'.repeat(EVENT_LIMIT) }, EVENT_LIMIT));
  }
});

test('receiptEvents keeps only latest live and preserves boundary events when over bound', () => {
  const events: Event[] = [
    { type: 'phase', seq: 1, state: 'running' },
    { type: 'live', seq: 2, payload: { text: 'first' } },
    { type: 'pong', seq: 3, nonce: 'n' },
    { type: 'commit', seq: 4, item: { note: 'x'.repeat(200) }, head: '1', placed: [], changed_paths: [], usage: null },
    { type: 'live', seq: 5, payload: { text: 'latest' } },
    { type: 'postimage', seq: 6, postimage: { path: 'a.txt' } },
    { type: 'approval_settled', seq: 7, id: 'a1' },
  ];
  // Within bound: keeps non-live, non-pong events and only the latest live.
  assert.deepEqual(receiptEvents(events, 4096).map(e => e.seq), [1, 4, 5, 6, 7]);
  // Over bound: drops non-boundary commit/phase, never drops boundary events or latest live.
  assert.deepEqual(receiptEvents(events, 100).map(e => e.seq), [5, 6, 7]);
});
