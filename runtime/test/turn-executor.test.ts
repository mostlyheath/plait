import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { rm, mkdir, readFile, writeFile, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { createRegistry, defineTool, defineExtension } from '@earendil-works/pi-durable';
import type { ToolState } from '../src/tools.ts';
import type { CommandOutputs } from '../src/tool-output.ts';
import { rootIdentity } from '../src/safety.ts';
import './support/safety.ts';
import { TurnExecutor, toolStateFor, type TurnEvent, type TurnSink } from '../src/turn-executor.ts';

test('executor runs a plain Pi turn without Runtime', async () => {
  const models = createModels(); const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage('Hello from Pi.')]); models.setProvider(faux.provider);
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-executor-'));
  const executor = new TurnExecutor();
  const input = { sessionFile: join(directory, 'session.sqlite'), checkout: process.cwd(), model: { provider: 'faux', modelId: 'faux-1' }, maxTurns: 10, vcs: 'folder' as const, priorOwned: [], directory, threadId: 'thread', trusted: false, header: '', mcpSettings: {} };
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

test('executor includes discovered instructions with MCP disabled', async () => {
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-executor-resources-'));
  try {
    const agentDir = join(directory, 'agent');
    await mkdir(agentDir);
    await writeFile(join(agentDir, 'AGENTS.md'), 'Follow this instruction.');
    const input = { sessionFile: join(directory, 'session.sqlite'), checkout: directory,
      model: { provider: 'faux', modelId: 'faux-1' }, maxTurns: 10, vcs: 'folder' as const,
      priorOwned: [], directory, threadId: 'thread', trusted: false, header: '', mcpSettings: {}, agentDir };
    const state = { rootIdentity: { dev: '1', ino: '2' } } as ToolState;
    const { instructions } = await new TurnExecutor().resources(input, state, {
      observe() { assert.fail('disabled MCP must not observe'); },
      remember() { assert.fail('disabled MCP must not register labels'); },
    });
    assert.match(instructions, /Follow this instruction\./);
    assert.equal(state.mcp, undefined);
  } finally { await rm(directory, { recursive: true, force: true }); }
});


test('executor handles write approvals through commands without Runtime', async () => {
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-executor-approval-'));
  const executor = new TurnExecutor();
  const events: TurnEvent[] = []; const writes: string[] = [];
  let allow = true;
  const sink = { emit(event: TurnEvent) {
    events.push(event);
    if (event.type === 'approval_needed') queueMicrotask(() => executor.command({ type: 'approve', id: event.approval.id, allow }));
  } } as TurnSink;
  executor.connect(sink);
  const models = createModels(); const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([
    fauxAssistantMessage([fauxToolCall('write', { path: 'allowed' }, { id: 'first' })], { stopReason: 'toolUse' }), fauxAssistantMessage('done'),
    fauxAssistantMessage([fauxToolCall('write', { path: 'denied' }, { id: 'second' })], { stopReason: 'toolUse' }), fauxAssistantMessage('done'),
  ]); models.setProvider(faux.provider);
  const input = { sessionFile: join(directory, 'session.sqlite'), checkout: directory, model: { provider: 'faux', modelId: 'faux-1' }, maxTurns: 10, vcs: 'folder' as const, priorOwned: [], directory, threadId: 'thread', trusted: false, header: '', mcpSettings: {} };
  const state = { authorization: { active: true } } as ToolState;
  const { harness, root } = await executor.open(input, { models, state, retry: { enabled: false, maxRetries: 0, baseDelayMs: 0 }, registry: () => {
    const registry = createRegistry();
    registry.install(defineExtension({ name: 'test', tools: [defineTool({ name: 'write', description: 'Write a file', parameters: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] },
      async execute(value, api) {
        const path = (value as { path: string }).path;
        await executor.authorize({ toolId: api.callId, name: 'write', arguments: { path }, dirtyPaths: [] }, new AbortController().signal);
        writes.push(path); return { output: [{ type: 'text' as const, text: 'written' }] };
      } })] }));
    return registry;
  } });
  try {
    for (const requestId of ['one', 'two']) {
      await executor.run({ root, instructions: 'Use the tool.', model: input.model, thinkingLevel: null, maxTurns: 10, signal: new AbortController().signal }, sink, {
        activate() {}, async draft() { return { type: 'input', requestId, content: 'Write' }; }, started() {}, async steer() {},
      });
      allow = false;
    }
    assert.deepEqual(writes, ['allowed']);
    const approvals = events.filter(event => event.type === 'approval_needed');
    assert.equal(approvals.length, 2);
    assert.notEqual(approvals[0].approval.id, approvals[1].approval.id);
    assert.equal(events.filter(event => event.type === 'approval_settled').length, 2);
  } finally { await harness.close(context); await rm(directory, { recursive: true, force: true }); }
});


test('plain turn state sends a write postimage to its sink', async t => {
  // Resolved, as the desktop sends it (on macOS /var is a link to /private/var).
  const directory = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-executor-')));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const checkout = join(directory, 'plain');
  await mkdir(checkout);
  const executor = new TurnExecutor();
  const events: TurnEvent[] = [];
  const sink = { emit(event: TurnEvent) { events.push(event); } } as TurnSink;
  executor.connect(sink);
  const input = { sessionFile: join(directory, 'session.sqlite'), checkout,
    model: { provider: 'faux', modelId: 'faux-1' }, maxTurns: 10, vcs: 'folder' as const,
    priorOwned: [], rootIdentity: await rootIdentity(checkout), approvalMode: 'auto' as const,
    directory, threadId: 'thread', trusted: false, header: '', mcpSettings: {} };
  const state = toolStateFor(input, { buildCache: { store: directory, thread: 'thread' }, outputs: {} as CommandOutputs },
    { sink, approval: executor });
  const models = createModels(); const faux = fauxProvider({ tokensPerSecond: 1e8 });
  faux.setResponses([fauxAssistantMessage([fauxToolCall('write', { path: 'new.txt', content: 'hello' }, { id: 'write' })], { stopReason: 'toolUse' }), fauxAssistantMessage('done')]);
  models.setProvider(faux.provider);
  const { harness, root } = await executor.open(input, { models, state, retry: { enabled: false, maxRetries: 0, baseDelayMs: 0 } });
  try {
    await executor.run({ root, instructions: 'Write the file.', model: input.model, thinkingLevel: null, maxTurns: 10,
      signal: state.authorization.signal }, sink, {
      activate() { state.authorization.activate(); }, async draft() { return { type: 'input', requestId: 'one', content: 'Write' }; },
      started() {}, async steer() {},
    });
    const postimage = events.find(event => event.type === 'postimage');
    assert.equal(postimage?.image.path, 'new.txt');
    assert.equal(await readFile(join(checkout, 'new.txt'), 'utf8'), 'hello');
  } finally { await harness.close(context); }
});
