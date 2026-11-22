import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { createRegistry } from '@earendil-works/pi-durable';
import type { ToolState } from '../src/tools.ts';
import { TurnExecutor, type TurnEvent, type TurnSink } from '../src/turn-executor.ts';

test('executor runs a plain Pi turn without Runtime', async () => {
  const models = createModels(); const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('Hello from Pi.')]); models.setProvider(faux.provider);
  const directory = await mkdtemp(join(tmpdir(), 'plait-executor-'));
  const executor = new TurnExecutor();
  const input = { sessionFile: join(directory, 'session.sqlite'), checkout: process.cwd(), model: { provider: 'faux', modelId: 'faux-1' }, maxTurns: 10, vcs: 'folder' as const, priorOwned: [] };
  const state = { authorization: { active: true } } as ToolState;
  const { harness, root } = await executor.open(input, { models, state, retry: { enabled: false, maxRetries: 0, baseDelayMs: 0 }, registry: () => createRegistry() });
  try {
    const events: TurnEvent[] = [];
    const sink = { emit(event: TurnEvent) { events.push(event); } } as TurnSink;
    let started = false;
    await executor.run({ root, instructions: 'Be helpful.', model: { provider: 'faux', modelId: 'faux-1' },
      thinkingLevel: null, maxTurns: 10, signal: new AbortController().signal }, sink, {
      activate() {}, async draft() { return { type: 'input', requestId: 'one', content: 'Hello' }; },
      started() { started = true; }, async steer() {},
    });
    const page = await root.entries({ order: 'ascending' }, 100, undefined, context);
    assert.match(JSON.stringify(page), /Hello from Pi/);
    assert.equal(started, true); assert.deepEqual(events, [{ type: 'phase', phase: 'running' }]);
  } finally { await harness.close(context); await rm(directory, { recursive: true, force: true }); }
});
