import { fixtureDirectory } from './support/temp.ts';
// End to end through the pinned Agent SDK against the scripted fake Claude Code.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { SDKMessage } from '@anthropic-ai/claude-agent-sdk';
import { canUseToolFrom, DECLINED } from '../src/claude/approval.ts';
import type { ClaudeApproval } from '../src/claude/approval.ts';
import { ClaudeSession } from '../src/claude/session.ts';
import type { ClaudeModeId } from '../src/claude/session.ts';
import { ClaudeTranscript, isTurnEnd } from '../src/claude/transcript.ts';
import type { TurnEnd } from '../src/claude/transcript.ts';
import type { Placed } from '../src/external/entries.ts';
import { loadSdk } from '../src/claude/sdk.ts';
import { DEV_SDK, FAKE_CLAUDE, FAKE_SESSION, fakeEnv } from './support/fake-claude-env.ts';

type Run = {
  session: ClaudeSession; transcript: ClaudeTranscript; messages: SDKMessage[]; placed: Placed[]; asked: ClaudeApproval[];
  ends: TurnEnd[]; done: Promise<Error | undefined>; log: string;
  send(text: string, requestId: string): string;
  until(check: () => boolean, label: string): Promise<void>;
  frames(): Promise<Record<string, any>[]>;
};

async function start(t: test.TestContext, options: { scenario?: string; mode?: ClaudeModeId; resume?: string; allow?: (approval: ClaudeApproval) => boolean | Promise<boolean> } = {}): Promise<Run> {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-claude-e2e-'));
  const log = join(dir, 'log');
  const asked: ClaudeApproval[] = [];
  const session = new ClaudeSession({
    query: (await loadSdk(DEV_SDK)).query, path: FAKE_CLAUDE, env: fakeEnv({ FAKE_CLAUDE_LOG: log, FAKE_CLAUDE_SCENARIO: options.scenario ?? 'turn' }), cwd: dir,
    model: 'default', level: 'high', mode: options.mode ?? 'default', resume: options.resume,
    canUseTool: canUseToolFrom(async approval => { asked.push(approval); return options.allow ? options.allow(approval) : true; }),
  });
  const transcript = new ClaudeTranscript([], 1);
  const messages: SDKMessage[] = []; const placed: Placed[] = []; const ends: TurnEnd[] = [];
  const done = (async () => {
    try {
      for await (const message of session.messages()) {
        messages.push(message);
        placed.push(...transcript.apply(message).placed);
        const end = isTurnEnd(message);
        if (end) ends.push(end);
      }
      return undefined;
    } catch (error) { return error as Error; }
  })();
  t.after(async () => { session.close(); await done; await rm(dir, { recursive: true, force: true }); });
  const until = async (check: () => boolean, label: string) => {
    const deadline = Date.now() + 8000;
    while (!check()) {
      if (Date.now() > deadline) throw new Error(`Timed out waiting for ${label}`);
      await new Promise(resolve => setTimeout(resolve, 5));
    }
  };
  const frames = async () => (await readFile(log, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  const send = (text: string, requestId: string) => {
    const uuid = session.send(text, requestId);
    transcript.sent(uuid, requestId, text);
    return uuid;
  };
  return { session, transcript, messages, placed, asked, ends, done, log, send, until, frames };
}

const entries = (run: Run) => run.transcript.view().entries.map(entry => entry.model[0]);

test('a turn asks for a Bash command, runs it, and answers', async t => {
  const run = await start(t);
  const init = await run.session.initialized;
  assert.equal(init.claude_code_version, '2.1.326');
  assert.equal(run.session.permissionMode(), 'default');
  run.send('List the files', 'req-1');
  await run.until(() => run.ends.length === 1, 'the turn to end');
  assert.deepEqual(run.ends, [{ status: 'completed', queued: 0 }]);
  assert.deepEqual(run.placed, [{ clientId: 'req-1', entry: 1 }]);
  assert.equal(run.asked.length, 1);
  assert.equal(run.asked[0].summary, 'ls -la');
  assert.equal(run.asked[0].details, 'List files\nReason: This command needs approval');
  assert.deepEqual(entries(run).map(message => message.role), ['user', 'assistant', 'toolResult', 'assistant']);
  assert.equal(entries(run)[3].content?.[0] && (entries(run)[3].content as { text: string }[])[0].text, 'Two files.');
  assert.equal(run.transcript.usage(null)?.context_window, 200000);
  assert.equal(run.transcript.sessionId(), run.session.sessionId);

  const frames = await run.frames();
  const argv: string[] = frames.find(frame => frame.argv?.includes('stream-json'))!.argv;
  for (const flag of ['--replay-user-messages', '--include-partial-messages', '--permission-mode=default', '--allow-dangerously-skip-permissions',
    '--permission-prompt-tool=stdio', '--effort=high', '--thinking=adaptive', '--thinking-display=summarized', `--session-id=${run.session.sessionId}`]) {
    assert.ok(argv.includes(flag), `${flag} in ${argv.join(' ')}`);
  }
  // The person's settings, CLAUDE.md, skills, hooks, and MCP load as `claude` loads them, and `default` picks Claude Code's model.
  assert.ok(!argv.some(arg => arg.startsWith('--setting-sources') || arg.startsWith('--model') || arg.startsWith('--resume')));
  const decision = frames.find(frame => frame.observed === 'permission')!.decision;
  assert.deepEqual(decision, { behavior: 'allow', updatedInput: { command: 'ls -la', description: 'List files' }, toolUseID: 'toolu_1' });
});

test('a declined edit tells Claude Code, and the turn goes on', async t => {
  const run = await start(t, { scenario: 'edit', allow: () => false });
  run.send('Fix a.txt', 'req-1');
  await run.until(() => run.ends.length === 1, 'the turn to end');
  assert.equal(run.asked[0].kind, 'edit');
  assert.ok(run.asked[0].paths[0].endsWith('/a.txt'));
  const result = entries(run).find(message => message.role === 'toolResult')!;
  assert.equal(result.isError, true);
  assert.equal((result.content as { text: string }[])[0].text, DECLINED);
  assert.equal((await run.frames()).find(frame => frame.observed === 'permission')!.decision.behavior, 'deny');
  assert.equal(run.ends[0].status, 'completed');
});

test('a question is declined without asking the person', async t => {
  const run = await start(t, { scenario: 'ask' });
  run.send('Pick one', 'req-1');
  await run.until(() => run.ends.length === 1, 'the turn to end');
  assert.equal(run.asked.length, 0);
  const decision = (await run.frames()).find(frame => frame.observed === 'permission')!.decision;
  assert.equal(decision.behavior, 'deny');
  assert.match(decision.message, /Ask it in your reply instead/);
});

test('a message sent during the turn steers it and is placed where it joined', async t => {
  let release = () => {};
  const gate = new Promise<boolean>(resolve => { release = () => resolve(true); });
  const run = await start(t, { allow: () => gate });
  run.send('List the files', 'req-1');
  await run.until(() => run.asked.length === 1, 'the approval');
  run.send('Also count them', 'req-2');
  // Give the SDK time to write the message before the tool finishes.
  await new Promise(resolve => setTimeout(resolve, 50));
  release();
  await run.until(() => run.ends.length === 1, 'the turn to end');
  assert.deepEqual(run.ends, [{ status: 'completed', queued: 0 }]);
  assert.deepEqual(run.placed.map(item => item.clientId), ['req-1', 'req-2']);
  assert.deepEqual(entries(run).map(message => message.role), ['user', 'assistant', 'toolResult', 'user', 'assistant']);
  assert.equal(entries(run)[3].content, 'Also count them');
  const result = run.messages.find(message => message.type === 'result') as { user_message_uuids?: string[] };
  assert.equal(result.user_message_uuids?.length, 2);
});

test('Stop interrupts the turn and cancels what is still queued', async t => {
  const run = await start(t, { scenario: 'slow' });
  run.send('List the files', 'req-1');
  await run.until(() => entries(run).some(message => message.role === 'toolResult'), 'the tool result');
  const queued = run.send('Then do more', 'req-2');
  await new Promise(resolve => setTimeout(resolve, 50));
  const receipt = await run.session.interrupt();
  assert.deepEqual(receipt, { cancelled: [queued], stillQueued: [] });
  assert.deepEqual(run.transcript.drop(receipt.cancelled), ['req-2']);
  await run.until(() => run.ends.length === 1, 'the turn to end');
  assert.equal(run.ends[0].status, 'interrupted');
  // Nothing runs after Stop.
  await new Promise(resolve => setTimeout(resolve, 100));
  assert.equal(run.ends.length, 1);
  assert.deepEqual(run.placed.map(item => item.clientId), ['req-1']);
});

test('Stop during an approval withdraws it', async t => {
  const run = await start(t, { allow: () => new Promise<boolean>(() => {}) });
  run.send('List the files', 'req-1');
  await run.until(() => run.asked.length === 1, 'the approval');
  await run.session.interrupt();
  await run.until(() => run.ends.length === 1, 'the turn to end');
  assert.equal(run.ends[0].status, 'interrupted');
  run.transcript.finish();
  assert.equal(entries(run).find(message => message.role === 'toolResult')?.isError, true);
});

test('the mode can change during a run, and the reported mode follows', async t => {
  const run = await start(t, { mode: 'plan', scenario: 'slow' });
  await run.session.initialized;
  assert.equal(run.session.permissionMode(), 'plan');
  run.send('List the files', 'req-1');
  await run.until(() => run.asked.length === 1, 'the approval');
  await run.session.setPermissionMode('acceptEdits');
  await run.until(() => run.session.permissionMode() === 'acceptEdits', 'the accept edits status');
  assert.equal(run.transcript.permissionMode(), 'acceptEdits');
  await run.session.setPermissionMode('bypassPermissions');
  await run.until(() => run.session.permissionMode() === 'bypassPermissions', 'the bypass status');
  await assert.rejects(run.session.setPermissionMode('nonsense' as ClaudeModeId), /must be one of/);
  await run.session.interrupt();
});

test('Claude Code exiting mid-turn ends the messages with an error', async t => {
  const run = await start(t, { scenario: 'exit-mid-turn' });
  run.send('List the files', 'req-1');
  const error = await run.done;
  assert.ok(error instanceof Error);
  assert.match(error.message, /exited with code 3/);
  assert.equal(run.ends.length, 0);
  const finished = run.transcript.finish();
  assert.equal(finished.append.length, 1);
  assert.equal(entries(run).at(-1)?.role, 'toolResult');
  assert.throws(() => run.session.send('more', 'req-2'), /closed/);
});

test('a run resumes the thread\'s session, and an unknown one fails', async t => {
  const run = await start(t, { resume: FAKE_SESSION });
  assert.equal(run.session.sessionId, FAKE_SESSION);
  run.send('Continue', 'req-1');
  await run.until(() => run.ends.length === 1, 'the turn to end');
  assert.equal(run.transcript.sessionId(), FAKE_SESSION);
  const argv: string[] = (await run.frames()).find(frame => frame.argv?.includes('stream-json'))!.argv;
  assert.ok(argv.includes(`--resume=${FAKE_SESSION}`));
  assert.ok(!argv.some(arg => arg.startsWith('--session-id')));

  const missing = await start(t, { resume: '6f1c2a4e-0000-4000-8000-0000000000ff' });
  missing.send('Continue', 'req-1');
  const error = await missing.done;
  assert.ok(error instanceof Error);
  assert.equal(missing.ends[0]?.status, 'failed');
});
