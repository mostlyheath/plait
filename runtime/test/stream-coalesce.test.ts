import assert from 'node:assert/strict';
import { test } from 'node:test';
import { InProcessSink, type TurnEvent } from '../src/turn-executor.ts';
import type { Snapshot } from '../src/protocol.ts';
import type { Store } from '../src/store.ts';

function fixture() {
  const snapshot: Snapshot = { id: 't', checkout: '/repo', model: { provider: 'p', model_id: 'm' }, status: 'running', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
  const saves: Snapshot[] = [];
  const sink = new InProcessSink(snapshot, { setting: () => null } as unknown as Store,
    () => saves.push(structuredClone(snapshot)), () => {}, () => saves.push(structuredClone(snapshot)));
  function delta(text: string, tool = false) {
    const projection = { entries: tool ? [{ id: '1', model: [{ role: 'assistant', content: [{ type: 'toolCall', id: 'call', name: 'read', arguments: {} }] }] }] : [],
      docs: { 'pi.live': { generation: { message: { role: 'assistant', content: text } } } } } as unknown as Extract<TurnEvent, { type: 'live' }>['projection'];
    sink.emit({ type: 'live', projection, inputs: [], contextWindow: null });
    sink.emit({ type: 'commit', head: tool ? 1 : 0 });
  }
  return { sink, snapshot, saves, delta };
}

test('stream bursts persist the trailing text and publish within the window', async () => {
  const { sink, snapshot, saves, delta } = fixture();
  for (let i = 0; i < 100; i++) delta(String(i));
  assert.equal(snapshot.live_text, '99');
  await new Promise(resolve => setTimeout(resolve, 75));
  assert.ok(saves.length <= 2, `saves: ${saves.length}`);
  assert.equal(saves.at(-1)?.live_text, '99');
  sink.emit({ type: 'finished' });
});

test('tool start and stop flush pending text immediately; finish cancels the timer', async () => {
  const { sink, saves, delta } = fixture();
  delta('first'); delta('pending');
  const beforeTool = saves.length;
  delta('tool', true);
  assert.equal(saves.length, beforeTool + 1);
  assert.equal(saves.at(-1)?.tools[0].status, 'running');
  delta('latest', true);
  sink.flush(); // Runtime flushes before marking a stop or interruption.
  assert.equal(saves.at(-1)?.live_text, 'latest');
  delta('last', true);
  sink.emit({ type: 'finished' });
  const finished = saves.length;
  await new Promise(resolve => setTimeout(resolve, 75));
  assert.equal(saves.length, finished);
});
