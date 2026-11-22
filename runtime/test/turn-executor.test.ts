import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { createRegistry, Harness, MemoryStorage } from '@earendil-works/pi-durable';
import { TurnExecutor, type TurnEvent, type TurnSink } from '../src/turn-executor.ts';

test('executor runs a plain Pi turn without Runtime', async () => {
  const models = createModels(); const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('Hello from Pi.')]); models.setProvider(faux.provider);
  const harness = await Harness.open(new MemoryStorage(), { models, registry: createRegistry() }, context);
  try {
    const root = await harness.root(context, { agent: { model: { provider: 'faux', modelId: 'faux-1' }, cwd: process.cwd(), maxTurns: 10 } });
    const events: TurnEvent[] = [];
    const sink = { emit(event: TurnEvent) { events.push(event); } } as TurnSink;
    let started = false;
    await new TurnExecutor().run({ root, instructions: 'Be helpful.', model: { provider: 'faux', modelId: 'faux-1' },
      thinkingLevel: null, maxTurns: 10, signal: new AbortController().signal }, sink, {
      activate() {}, async draft() { return { type: 'input', requestId: 'one', content: 'Hello' }; },
      started() { started = true; }, async steer() {},
    });
    const page = await root.entries({ order: 'ascending' }, 100, undefined, context);
    assert.match(JSON.stringify(page), /Hello from Pi/);
    assert.equal(started, true); assert.deepEqual(events, [{ type: 'phase', phase: 'running' }]);
  } finally { await harness.close(context); }
});
