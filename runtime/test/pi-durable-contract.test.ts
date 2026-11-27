import assert from 'node:assert/strict';
import { fork } from 'node:child_process';
import { appendFile, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxText, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { Harness, createRegistry, defineExtension, defineTool, hook, ToolTask } from '@earendil-works/pi-durable';
import { openNodeSqliteStorage } from '@earendil-works/pi-durable/storage/sqlite/node';

import { NodeExecutionEnv } from '@earendil-works/pi-durable/env/node';
import { createReadTool, createEditTool } from '@earendil-works/pi-durable/tools';

type Replay = 'safe' | 'unsafe';
type Phase = 'crash' | 'recover';
type EffectWritten = { type: 'effect_written', threadId: number, submissionId: number };
type WorkerMessage = EffectWritten | Awaited<ReturnType<typeof runRuntime>>;

const request = { type: 'input' as const, requestId: 'recovery-probe', content: 'Apply the test effect once.' };

async function runRuntime(phase: Phase, replay: Replay, directory: string) {
  assert(['crash', 'recover'].includes(phase));
  assert(['safe', 'unsafe'].includes(replay));
  const faux = fauxProvider();
  faux.setResponses(phase === 'crash' ? [
    fauxAssistantMessage([fauxToolCall('effect', {}, { id: 'effect-1' })], { stopReason: 'toolUse' }),
  ] : [fauxAssistantMessage([fauxText('Recovery complete.')])]);
  const models = createModels();
  models.setProvider(faux.provider);
  let admit!: (id: number) => void;
  const admitted = new Promise<number>(resolve => { admit = resolve; });
  let threadId: number;
  const registry = createRegistry();
  registry.install(defineExtension({
    name: 'recovery-probe',
    tools: [defineTool({
      name: 'effect',
      description: 'Write the test result.',
      parameters: { type: 'object', properties: {}, additionalProperties: false },
      replay,
      async execute() {
        const submissionId = phase === 'crash' ? await admitted : undefined;
        // The overwrite is idempotent; the attempt log measures whether the runtime replays it.
        await writeFile(join(directory, 'result.txt'), 'durable\n');
        await appendFile(join(directory, 'attempts.txt'), 'attempt\n');
        if (phase === 'crash') {
          process.send!({ type: 'effect_written', threadId, submissionId });
          // Keep the effect outside a completed tool checkpoint until the parent kills this process.
          await new Promise(() => {});
        }
        return { output: [{ type: 'text', text: 'Result written.' }] };
      },
    })],
  }));
  const storage = await openNodeSqliteStorage(join(directory, 'session.sqlite'));
  const harness = await Harness.open(storage, { models, registry }, context);
  try {
    const root = await harness.root(context, {
      agent: { model: { provider: 'faux', modelId: 'faux-1' }, maxTurns: 3 },
    });
    threadId = root.id;
    if (phase === 'crash') {
      const submission = await root.submit(request, context);
      admit(submission.id);
      await submission.wait(context);
      throw new Error('The parent should kill this process before the tool finishes.');
    }
    harness.resume();
    await harness.waitForIdle(context);
    const duplicate = await root.submit(request, context);
    assert.equal((await duplicate.wait(context)).status, 'done');
    await harness.waitForIdle(context);
    const { items } = await root.entries({}, 100, undefined, context);
    const results = items.filter(entry => entry.kind === 'pi.tool-result');
    assert.equal(results.length, 1);
    const data = results[0].data;
    const diagnostics = data && typeof data === 'object' && 'diagnostics' in data ? data.diagnostics : undefined;
    const message = results[0].model?.[0];
    assert(message?.role === 'toolResult');
    return {
      type: 'recovered' as const, threadId, submissionId: duplicate.id,
      userEntries: items.filter(entry => entry.kind === 'pi.user').length,
      interrupted: Array.isArray(diagnostics) && diagnostics.some(diagnostic => diagnostic && typeof diagnostic === 'object' && 'code' in diagnostic && diagnostic.code === 'interrupted'),
      toolError: message.isError,
    };
  } finally {
    await harness.close(context);
  }
}

function startWorker(phase: Phase, replay: Replay, directory: string) {
  const child = fork(fileURLToPath(import.meta.url), ['--worker', phase, replay, directory], {
    execArgv: [], stdio: ['ignore', 'ignore', 'pipe', 'ipc'],
  });
  let stderr = '';
  child.stderr!.on('data', data => { stderr = (stderr + data).slice(-8192); });
  const exited = new Promise<{ code: number | null, signal: NodeJS.Signals | null, stderr: string }>((resolve, reject) => {
    child.once('exit', (code, signal) => resolve({ code, signal, stderr }));
    child.once('error', reject);
  });
  const message = new Promise<WorkerMessage>((resolve, reject) => {
    child.once('message', resolve);
    child.once('error', reject);
    child.once('exit', (code, signal) => reject(new Error(`Worker exited before reporting: ${code}/${signal}\n${stderr}`)));
  });
  return { child, exited, message };
}

if (process.argv[2] === '--worker') {
  // A pending tool promise alone does not keep a Node process alive.
  process.channel!.ref();
  const [phase, replay, directory] = process.argv.slice(3);
  assert(phase === 'crash' || phase === 'recover');
  assert(replay === 'safe' || replay === 'unsafe');
  const result = await runRuntime(phase, replay, directory);
  process.send!(result);
  process.disconnect!();
} else {
  test('scripted edits survive observer detach and SQLite reopen', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'plait-durable-probe-'));
    t.after(() => rm(directory, { recursive: true, force: true }));
    await writeFile(join(directory, 'notes.txt'), 'hello world\n');
    const models = createModels();
    const provider = 'faux';
    const modelId = 'faux-1';
    const faux = fauxProvider({ tokensPerSecond: 80, tokenSize: { min: 1, max: 1 } });
    faux.setResponses([
      fauxAssistantMessage([fauxToolCall('read', { path: 'notes.txt' }, { id: 'r' })], { stopReason: 'toolUse' }),
      fauxAssistantMessage([fauxToolCall('edit', { path: 'notes.txt', edits: [{ oldText: 'world', newText: 'durable' }] }, { id: 'e' })], { stopReason: 'toolUse' }),
      fauxAssistantMessage([fauxText('The file now greets durable. This answer is deliberately streamed in small parts.')]),
    ]);
    models.setProvider(faux.provider);
    assert(models.getModel(provider, modelId), 'Configured model is missing');
    const registry = createRegistry();
    registry.install(defineExtension({
      name: 'probe',
      tools: [createReadTool(), createEditTool()],
      hooks: [hook(ToolTask, {
        beforeTool: call => call.arguments?.path === 'notes.txt' ? undefined : { block: 'This probe only permits notes.txt.' },
      })],
    }));
    const options = {
      models, registry,
      env: () => new NodeExecutionEnv({ cwd: directory }),
      settings: { retry: { maxRetries: 0 }, stream: { timeoutMs: 60000 } },
    };
    const open = () => openNodeSqliteStorage(join(directory, 'session.sqlite')).then(storage => Harness.open(storage, options, context));
    let harness = await open();
    const deadline = setTimeout(() => { void harness.close(context); }, 120000);
    try {
      const root = await harness.root(context, { agent: {
        model: { provider, modelId }, cwd: directory, maxTurns: 4, thinkingLevel: 'low',
        instructions: 'This is a Plait integration probe. Only inspect and edit notes.txt in the working directory. Do not access any other path. Keep the answer short.',
      } });
      const watch = await root.watch(context);
      let partials = 0;
      let firstUpdate!: () => void;
      const changed = new Promise<void>(resolve => { firstUpdate = resolve; });
      watch.start(async call => {
        if (call.kind !== 'value') return;
        const generation = call.value.docs['pi.live']?.generation;
        if (generation && typeof generation === 'object' && 'message' in generation && generation.message) {
          partials++;
          firstUpdate();
        }
      });
      const request = { type: 'input' as const, requestId: 'plait-probe-1', content: 'Read notes.txt, then use edit to replace world with durable. Reply with a short confirmation.' };
      const submission = await root.submit(request, context);
      const finished = submission.wait(context);
      await Promise.race([changed, finished]);
      // A detached observer must not own or cancel the agent run.
      await watch.stop();
      const settled = await finished;
      assert(partials > 0, 'No incremental model state was observed');
      assert.equal(settled.status, 'done', 'The scripted model did not finish');
      await harness.waitForIdle(context);
      assert.equal(await readFile(join(directory, 'notes.txt'), 'utf8'), 'hello durable\n');
      const reattached = await root.watch(context);
      const entries = reattached.value.entries;
      assert(entries.some(entry => entry.kind === 'pi.tool-result'));
      assert(entries.some(entry => entry.kind === 'pi.assistant'));
      await reattached.stop();
      await harness.close(context);
      harness = await open();
      const reopened = await harness.root(context);
      assert.equal(reopened.id, root.id);
      const duplicate = await reopened.submit(request, context);
      assert.equal(duplicate.id, submission.id, 'Retry submitted a second input');
      assert.equal((await duplicate.wait(context)).status, 'done');
      const persisted = await reopened.entries({}, 100, undefined, context);
      // Entry pages are newest first. The watch presents chronological order.
      assert.deepEqual(persisted.items.toReversed(), entries);
    } finally {
      clearTimeout(deadline);
      await harness.close(context);
    }
  });

  for (const replay of ['unsafe', 'safe'] as const) {
    test(`SIGKILL recovery follows the ${replay} tool replay policy`, { timeout: 20000 }, async t => {
      const directory = await mkdtemp(join(tmpdir(), 'plait-recovery-'));
      const workers: ReturnType<typeof startWorker>[] = [];
      t.after(async () => {
        for (const worker of workers) {
          if (worker.child.exitCode === null && worker.child.signalCode === null) worker.child.kill('SIGKILL');
          await worker.exited;
        }
        await rm(directory, { recursive: true, force: true });
      });
      const first = startWorker('crash', replay, directory);
      workers.push(first);
      const before = await first.message;
      assert.equal(before.type, 'effect_written');
      assert.equal(await readFile(join(directory, 'attempts.txt'), 'utf8'), 'attempt\n');
      first.child.kill('SIGKILL');
      assert.equal((await first.exited).signal, 'SIGKILL');

      const second = startWorker('recover', replay, directory);
      workers.push(second);
      const after = await second.message;
      const exit = await second.exited;
      assert.equal(exit.code, 0, JSON.stringify(exit));
      assert.equal(after.type, 'recovered');
      assert.equal(after.threadId, before.threadId);
      assert.equal(after.submissionId, before.submissionId);
      assert.equal(after.userEntries, 1);
      assert.equal(after.interrupted, replay === 'unsafe');
      assert.equal(after.toolError, replay === 'unsafe');
      assert.equal(await readFile(join(directory, 'result.txt'), 'utf8'), 'durable\n');
      assert.equal(await readFile(join(directory, 'attempts.txt'), 'utf8'),
        replay === 'unsafe' ? 'attempt\n' : 'attempt\nattempt\n');
    });
  }
}
