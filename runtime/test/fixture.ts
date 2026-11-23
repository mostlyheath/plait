import { loadMcp } from '../src/mcp/config.ts';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxText, fauxThinking, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import type { FauxResponseStep } from '@earendil-works/pi-ai/providers/faux';
import { Runtime } from '../src/runtime.ts';
import { defineExtension, hook, ToolTask } from '@earendil-works/pi-durable';
import { appendFile } from 'node:fs/promises';
import { appendFileSync, writeFileSync, readFileSync } from 'node:fs';
import childProcess from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { join } from 'node:path';
import type { ToolState } from '../src/tools.ts';
import { RunAuthorization } from '../src/authorization.ts';
import { serve } from '../src/stdio.ts';

writeFileSync(join(process.argv[2], 'fixture-processes.json'), JSON.stringify({ node: process.pid, host: process.ppid, store: process.argv[2] }));
const models = createModels();
const mode = process.env.PLAIT_TEST_SCENARIO;
if (mode?.startsWith('mcp-')) {
  const spawn = childProcess.spawn;
  childProcess.spawn = ((...args: Parameters<typeof spawn>) => {
    appendFileSync(join(process.argv[2], 'mcp-spawns.jsonl'), JSON.stringify({ operation: Array.isArray(args[1]) ? args[1][0] : null }) + '\n');
    return spawn(...args);
  }) as typeof spawn;
  syncBuiltinESMExports();
}
const compacting = mode?.startsWith('self-host-compaction');
const fast = compacting || mode === 'v3-long' || mode?.startsWith('mcp-');
// Records what the model is told (instructions and tool declarations) for one request.
const recordContext = (context: { messages: unknown }) => {
  appendFileSync(join(process.argv[2], 'model-contexts.jsonl'), JSON.stringify(context.messages) + '\n');
  return fauxAssistantMessage([fauxText('Recorded the scripted model context.')]);
};
// Version 3 scenarios add a reasoning model; the long one streams fast enough for a large reply.
const faux = fauxProvider({
  tokensPerSecond: fast ? 1e8 : 120, tokenSize: fast ? { min: 2048, max: 2048 } : { min: 1, max: 1 },
  ...(mode === 'images' ? { models: [{id:'faux-1',name:'Images',input:['text','image'] as ('text'|'image')[]}] } : {}),
  ...(compacting ? { models: [{ id: 'faux-1', name: 'Faux Model', contextWindow: 65536 }] } : {}),
  ...(mode?.startsWith('v3') ? { models: [{ id: 'faux-1', name: 'Faux Model' }, { id: 'faux-think', name: 'Faux Thinking', reasoning: true, contextWindow: 64000 }] } : {}),
});
const call = (name: string, args: Record<string, string | number>, id: string) => fauxAssistantMessage([fauxToolCall(name, args, { id })], { stopReason: 'toolUse' });
const overloaded = () => fauxAssistantMessage([], { stopReason: 'error', errorMessage: 'overloaded_error: SECRET_PROVIDER_TOKEN_DO_NOT_LEAK' });
const responses: FauxResponseStep[] = mode?.startsWith('mcp-') ? [
  context => { appendFileSync(join(process.argv[2], 'mcp-context.jsonl'), JSON.stringify(context) + '\n'); return call('tool_search', { query: 'echo fixture', limit: 20 }, 'search-mcp'); },
  context => {
    appendFileSync(join(process.argv[2], 'mcp-context.jsonl'), JSON.stringify(context) + '\n');
    if (mode === 'mcp-search-only') return fauxAssistantMessage('Search complete.');
    const tool = context.messages.flatMap(message => message.role === 'system' ? message.toolsAdded ?? [] : [])
      .find(tool => tool.description.startsWith('mcp:docs/echo:'));
    return tool ? call(tool.name, { text: 'fixture echo' }, 'call-mcp') : fauxAssistantMessage('No MCP tool was offered.');
  },
  context => { appendFileSync(join(process.argv[2], 'mcp-context.jsonl'), JSON.stringify(context) + '\n'); return fauxAssistantMessage('Fixture MCP run complete.'); },
] : mode === 'images' ? [recordContext] : compacting ? Array.from({ length: 40 }, () => async (context: { messages: unknown }) => {
  const text = JSON.stringify(context.messages);
  const summary = text.includes('You are a context summarization assistant');
  appendFileSync(join(process.argv[2], 'compaction-observed.jsonl'), JSON.stringify({ summary, goal: text.includes('KEEP_SELF_HOST_GOAL'), length: text.length }) + '\n');
  if (summary) {
    if (mode === 'self-host-compaction-crash') {
      writeFileSync(join(process.argv[2], 'summary-started'), 'ready');
      await new Promise(() => {});
    }
    if (mode === 'self-host-compaction-failure') return fauxAssistantMessage([], { stopReason: 'error', errorMessage: 'Fixture summary failed.' });
    return fauxAssistantMessage('KEEP_SELF_HOST_GOAL: finish the project. Preserve notes.txt and continue the review.');
  }
  return fauxAssistantMessage('Development record. '.repeat(2500));
}) : mode === 'self-host-script'
  ? (JSON.parse(readFileSync(join(process.argv[2], 'script.json'), 'utf8')) as { name?: string; args?: Record<string, string | number>; id?: string; text?: string }[])
    .map(step => step.name ? call(step.name, step.args!, step.id!) : fauxAssistantMessage([fauxText(step.text!)]))
  : mode === 'v3-thinking' ? [
  fauxAssistantMessage([fauxThinking('Plan: read the notes first. '.repeat(20).trim()), fauxToolCall('read', { path: 'notes.txt' }, { id: 'think-read' })], { stopReason: 'toolUse' }),
  fauxAssistantMessage([fauxThinking('The file says before.'), fauxText('The notes say before.')]),
] : mode === 'v3-long' ? [
  fauxAssistantMessage([fauxText('BEGIN ' + '0123456789'.repeat(20000) + ' END')]),
] : mode === 'v3-retry' ? [
  call('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, 'retry-edit'),
  overloaded(),
  fauxAssistantMessage([fauxText('Recovered after a transient provider error.')]),
] : mode === 'v3-retry-exhausted' ? [overloaded(), overloaded(), overloaded(), overloaded()]
  : mode === 'v3-retry-slow' ? [overloaded()]
  : mode === 'v3-turns' ? Array.from({ length: 6 }, (_, index) => call('read', { path: 'notes.txt' }, `turn-${index}`))
  : mode === 'unicode-read' ? [
  call('read', { path: 'unicode.txt' }, 'unicode-read-1'),
] : mode === 'edit' ? [
  call('read', { path: 'notes.txt' }, 'read-1'),
  call('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, 'edit-1'),
] : mode === 'separate-edits' ? [
  call('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, 'first-edit'),
  fauxAssistantMessage([fauxText('First scripted turn finished.')]),
  call('edit', { path: 'notes.txt', old_text: 'after', new_text: 'followup' }, 'second-edit'),
] : mode === 'permissive-policy' ? [
  call('write', { path: '../outside.txt', content: 'DESTROYED' }, 'outside-write'),
  call('write', { path: '.git/automation.txt', content: 'DESTROYED' }, 'git-write'),
  call('edit', { path: 'notes.txt', old_text: 'dirty', new_text: 'DESTROYED' }, 'dirty-edit'),
  call('command', { command: 'printf DESTROYED > notes.txt' }, 'checkout-command'),
] : mode === 'edit-command' ? [
  call('read', { path: 'notes.txt' }, 'read-1'),
  call('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, 'edit-1'),
  call('command', { command: 'test "$(cat notes.txt)" = after; printf passed' }, 'command-1'),
] : mode === 'crash' ? [call('command', { command: 'printf "READY %s\\n" "$$"; sleep 30 & wait' }, 'command-1')]
  : mode === 'followup' ? [call('edit', { path: 'notes.txt', old_text: 'after', new_text: 'followup' }, 'followup-1')]
  : mode === 'effect' || mode === 'unrecorded' ? [call('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, 'effect-1')]
  : mode === 'dirty' ? [call('edit', { path: 'notes.txt', old_text: 'dirty', new_text: 'DESTROYED' }, 'dirty-1'), call('command', { command: 'printf DESTROYED > notes.txt' }, 'dirty-2')]
  : mode === 'trusted-work' ? [
    call('command', { command: 'printf "made by command\\n" > generated.txt && git add generated.txt && git -c user.name=Fixture -c user.email=fixture@invalid commit -qm "trusted commit" && printf COMMITTED' }, 'commit-1'),
    call('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, 'own-edit'),
    call('command', { command: 'printf "appended\\n" >> notes.txt && printf "by command\\n" > scratch.txt && printf APPENDED' }, 'append-1'),
    call('edit', { path: 'notes.txt', old_text: 'after', new_text: 'again' }, 'later-edit'),
    call('edit', { path: 'scratch.txt', old_text: 'by command', new_text: 'DESTROYED' }, 'unowned-edit'),
    call('edit', { path: 'dirty.txt', old_text: 'dirty', new_text: 'DESTROYED' }, 'dirty-edit'),
    call('write', { path: 'dirty.txt', content: 'DESTROYED' }, 'dirty-write'),
  ] : mode === 'trusted-followup' ? [call('edit', { path: 'notes.txt', old_text: 'again', new_text: 'followup' }, 'followup-1')]
  : mode === 'trusted-install' ? [
    call('command', { command: 'mkdir -p node_modules/package/lib target/debug/deps && i=0; while [ $i -lt 3000 ]; do : > node_modules/package/lib/f$i; : > target/debug/deps/f$i; i=$((i+1)); done; printf INSTALLED' }, 'install-1'),
    call('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, 'after-install-edit'),
    call('write', { path: 'created.txt', content: 'new\n' }, 'after-install-write'),
  ] : mode === 'trusted-stop' ? [call('command', { command: 'printf started > started.txt; sleep 300 & printf "READY %s %s\\n" "$$" "$!"; wait' }, 'stop-1')]
  : mode === 'trusted-twice' ? [
    call('command', { command: 'printf one > first.txt && printf "FIRST %s" "$HOME"' }, 'first-1'),
    fauxAssistantMessage([fauxText('First scripted turn finished.')]),
    call('command', { command: 'printf two > second.txt && printf SECOND' }, 'second-1'),
  ] : mode === 'record-context' ? [recordContext, recordContext]
  : mode === 'error' ? [fauxAssistantMessage([], { stopReason: 'error', errorMessage: 'SECRET_PROVIDER_TOKEN_DO_NOT_LEAK' })]
  : [];
responses.push(...Array.from({ length: 8 }, () => fauxAssistantMessage([fauxText('Scripted test response with incremental real runtime events. This is not a live model.')] )));
faux.setResponses(responses);
models.setProvider(faux.provider);
class FixtureRuntime extends Runtime {
  // Scenarios must not depend on the skills or instructions of the machine running them.
  override async resources(checkout: string, trusted: boolean, describe = false) {
    const mcp = await loadMcp(checkout, trusted, join(this.directory, 'test-pi'), true, describe);
    return { skills: [], templates: [], problems: mcp.problems, mcpServers: mcp.servers, mcpEntries: mcp.entries };
  }
  override async open(...args: Parameters<Runtime['open']>) {
    await super.open(...args);
    if (mode === 'cleanup') {
      const harness = args[0].harness!;
      const close = harness.close.bind(harness);
      harness.close = async context => { await new Promise(resolve => setTimeout(resolve, 200)); await close(context); };
    }
  }
  override async run(...args: Parameters<Runtime['run']>) {
    if (mode === 'before-submit') {
      writeFileSync(join(process.argv[2], 'accepted.marker'), 'accepted before durable submit');
      process.kill(process.pid, 'SIGKILL');
      await new Promise(() => {});
    }
    await super.run(...args);
    // Stop acknowledgement precedes cleanup; tests need a separate completion barrier.
    writeFileSync(join(process.argv[2], `${args[0].snapshot.id}.settled`), JSON.stringify({
      fixture: 'SCRIPTED_FIXTURE_ONLY', request_id: args[1], status: args[0].snapshot.status,
    }));
  }
  override registry(state: ToolState) {
    if (mode === 'mcp-consent') faux.setResponses(responses);
    if (mode?.startsWith('mcp-')) state.loginEnvironment = Promise.resolve({ env: { HOME: this.directory, PATH: '/usr/bin:/bin' }, source: 'login-shell' });
    if (mode === 'permissive-policy') {
      // This test-only policy proves authorization cannot replace file or sandbox checks.
      state.authorization = new RunAuthorization({ async authorize() {} });
    }
    const result = super.registry(state);
    if (mode === 'unrecorded') state.record = () => {
      writeFileSync(join(state.snapshot.checkout, 'before-record.marker'), 'effect committed, provenance absent');
      process.kill(process.pid, 'SIGKILL');
    };
    if (mode === 'effect') result.install(defineExtension({ name: 'effect-crash-window', hooks: [hook(ToolTask, {
      async afterTool(call, result) {
        if (call.name !== 'edit' || result.isError) return;
        await appendFile(join(state.snapshot.checkout, 'attempts.txt'), 'effect\n');
        await new Promise<void>(resolve => {
          state.authorization.signal.addEventListener('abort', () => resolve(), { once: true });
          if (state.authorization.signal.aborted) resolve();
        });
        return result;
      },
    })] }));
    // Counts completed edits outside the checkout, so a retried model request cannot hide a repeated effect.
    if (mode === 'v3-retry') result.install(defineExtension({ name: 'effect-counter', hooks: [hook(ToolTask, {
      async afterTool(call, result) {
        if (call.name === 'edit' && !result.isError) await appendFile(join(process.argv[2], 'attempts.txt'), 'effect\n');
        return result;
      },
    })] }));
    return result;
  }
}
const runtime = new FixtureRuntime(process.argv[2], models, new Set(['faux']));
// Short backoff and a small turn limit keep these scenarios fast; the policies are the production ones.
if (mode?.startsWith('v3-retry')) runtime.retry = { enabled: true, maxRetries: 2, baseDelayMs: mode === 'v3-retry-slow' ? 20000 : 40 };
if (mode === 'v3-turns') runtime.maxTurns = 2;
try { await runtime.recover(); await serve(runtime); }
finally { await runtime.close(); }
