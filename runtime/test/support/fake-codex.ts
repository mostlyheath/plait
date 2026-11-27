// A scripted stand-in for `codex`, for tests. It never contacts a model.
//   node fake-codex.ts --version     prints `codex-cli $FAKE_CODEX_VERSION` (default 0.169.0)
//   node fake-codex.ts app-server    speaks the app-server JSON line protocol on stdio
// FAKE_CODEX_SCENARIO picks the behavior; FAKE_CODEX_LOG, when set, receives every client frame as a JSON line.
// FAKE_CODEX_WRITE, when set, names a directory where each turn writes `codex-<message ID>.txt` as it starts.
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { stdinLines } from './stdin-lines.ts';

type Frame = { id?: string | number; method?: string; params?: any; result?: any; error?: any };
const scenario = process.env.FAKE_CODEX_SCENARIO ?? 'turn';
const log = process.env.FAKE_CODEX_LOG;

if (process.argv[2] === '--version') {
  if (scenario === 'no-version') process.stdout.write('codex (unknown build)\n');
  else process.stdout.write(`codex-cli ${process.env.FAKE_CODEX_VERSION ?? '0.169.0'}\n`);
  process.exit(0);
}
if (process.argv[2] !== 'app-server') {
  process.stderr.write('fake codex: expected --version or app-server\n');
  process.exit(2);
}

let initialized = false;
let nextServerId = 1000;
let turns = 0;
const replies = new Map<string | number, (frame: Frame) => void>();
const write = (frame: object) => process.stdout.write(`${JSON.stringify(frame)}\n`);
const notify = (method: string, params: object) => write({ method, params, emittedAtMs: Date.now() });
const reply = (id: string | number, result: unknown) => write({ id, result });
const failRequest = (id: string | number, message: string) => write({ id, error: { code: -32600, message } });
/** Sends a server request and waits for the client's answer. */
const ask = (method: string, params: object) => new Promise<Frame>(resolve => {
  const id = nextServerId++;
  replies.set(id, resolve);
  write({ id, method, params });
});
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

const threadId = 'thread-fake-1';
const thread = (id = threadId) => ({
  id, sessionId: id, forkedFromId: null, parentThreadId: null, preview: '', ephemeral: false, section: null, sectionEnteredAt: null,
  projectId: null, historyMode: 'full', modelProvider: 'openai', model: 'gpt-fake', reasoningEffort: 'medium', createdAt: 1, updatedAt: 1,
  recencyAt: null, status: { type: 'idle' }, path: null, cwd: '/work', cliVersion: '0.169.0', originator: null, source: 'appServer',
  threadSource: null, agentNickname: null, agentRole: null, gitInfo: null, name: null, turns: [],
});
const SANDBOXES: Record<string, object> = {
  'read-only': { type: 'readOnly', networkAccess: false },
  'workspace-write': { type: 'workspaceWrite', writableRoots: [], networkAccess: false, excludeTmpdirEnvVar: false, excludeSlashTmp: false },
  'danger-full-access': { type: 'dangerFullAccess' },
};
/** Echoes the settings asked for, as Codex does; `pinned` stands for a Codex whose requirements hold it read-only. */
const threadResponse = (params: any, id?: string) => {
  const pinned = scenario === 'pinned';
  return {
    thread: thread(id), model: 'gpt-fake', modelProvider: 'openai', serviceTier: null, disabledPluginIds: [], cwd: '/work', instructionSources: [],
    approvalPolicy: pinned ? 'on-request' : params.approvalPolicy ?? 'on-request', approvalsReviewer: pinned ? 'user' : params.approvalsReviewer ?? 'user',
    sandbox: SANDBOXES[pinned ? 'read-only' : params.sandbox ?? 'read-only'], reasoningEffort: 'medium',
  };
};
/** The approval policy of the turn that is playing; with `never`, Codex runs everything without asking. */
let policy = 'on-request';
const turnOf = (id: string, status = 'inProgress', error: unknown = null) => ({
  id, rootTurnId: id, items: [], itemsView: 'full', status, error, startedAt: 1, completedAt: status === 'inProgress' ? null : 2, durationMs: null,
});
const usage = (total: number) => ({
  totalTokens: total, inputTokens: total - 50, cachedInputTokens: 200, cacheWriteInputTokens: 0, outputTokens: 50, reasoningOutputTokens: 20,
});
const command = (status: string, extra: object = {}) => ({
  type: 'commandExecution', id: 'cmd-1', pluginId: null, scriptPath: null, command: 'ls -la', cwd: '/work', processId: null,
  source: 'agent', status, commandActions: [], aggregatedOutput: null, exitCode: null, durationMs: null, ...extra,
});
const edit = (status: string) => ({
  type: 'fileChange', id: 'edit-1', status,
  changes: [{ path: join(process.env.FAKE_CODEX_EDIT_ROOT ?? '/work', 'a.txt'), kind: { type: 'update', move_path: null }, diff: '@@ -1 +1 @@\n-old\n+new\n' }],
});
const item = (method: string, turnId: string, value: object) => notify(method, { item: value, threadId, turnId, startedAtMs: 1, completedAtMs: 2 });
const user = (turnId: string, text: string, clientId: string | null, id: string) => {
  const value = { type: 'userMessage', id, clientId, content: [{ type: 'text', text, text_elements: [] }] };
  item('item/started', turnId, value);
  item('item/completed', turnId, value);
};
let active: { id: string; interrupted: boolean } | undefined;

/** Plays one turn: reasoning, a command needing approval, an answer, usage, completion. */
async function playTurn(turnId: string) {
  notify('turn/started', { threadId, turn: turnOf(turnId) });
  const delta = (method: string, itemId: string, text: string, extra: object = {}) => notify(method, { threadId, turnId, itemId, delta: text, ...extra });
  item('item/started', turnId, { type: 'reasoning', id: 'rs-1', summary: [], content: [] });
  delta('item/reasoning/summaryTextDelta', 'rs-1', 'Looking at ', { summaryIndex: 0 });
  delta('item/reasoning/summaryTextDelta', 'rs-1', 'the files.', { summaryIndex: 0 });
  item('item/completed', turnId, { type: 'reasoning', id: 'rs-1', summary: ['Looking at the files.'], content: [] });
  if (scenario === 'edit') {
    item('item/started', turnId, edit('inProgress'));
    const answer = policy === 'never' ? { result: { decision: 'accept' } }
      : await ask('item/fileChange/requestApproval', { threadId, turnId, itemId: 'edit-1', startedAtMs: 1, reason: 'Update a.txt' });
    item('item/completed', turnId, edit(answer.result?.decision === 'accept' ? 'completed' : 'declined'));
  } else {
    item('item/started', turnId, command('inProgress'));
    const answer = policy === 'never' ? { result: { decision: 'accept' } } : await ask('item/commandExecution/requestApproval', {
      kind: 'command', threadId, turnId, itemId: 'cmd-1', startedAtMs: 1, environmentId: null, command: 'ls -la', cwd: '/work', reason: 'List files',
    });
    if (answer.result?.decision === 'accept') {
      delta('item/commandExecution/outputDelta', 'cmd-1', 'a.txt\n');
      delta('item/commandExecution/outputDelta', 'cmd-1', 'b.txt\n');
      item('item/completed', turnId, command('completed', { aggregatedOutput: 'a.txt\nb.txt\n', exitCode: 0, durationMs: 5 }));
    } else item('item/completed', turnId, command('declined'));
  }
  if (scenario === 'user-input') {
    const answer = await ask('item/tool/requestUserInput', { threadId, turnId, itemId: 'ask-1', questions: [], isBlocking: true, autoResolutionMs: null });
    if (log) appendFileSync(log, `${JSON.stringify({ observed: 'user-input', answer })}\n`);
  }
  if (scenario === 'interrupt-ignored') return;
  if (scenario === 'slow' || scenario === 'start-delayed') await wait(5000);
  if (active?.interrupted) return;
  item('item/started', turnId, { type: 'agentMessage', id: 'msg-1', text: '', phase: null, moderation: null, memoryCitation: null, delivery: null, questions: null });
  for (const part of ['Two ', 'files.']) delta('item/agentMessage/delta', 'msg-1', part);
  item('item/completed', turnId, { type: 'agentMessage', id: 'msg-1', text: 'Two files.', phase: null, moderation: null, memoryCitation: null, delivery: null, questions: null });
  notify('thread/tokenUsage/updated', { threadId, turnId, tokenUsage: { total: usage(1500), last: usage(1000), modelContextWindow: 272000 } });
  notify('turn/completed', { threadId, turn: turnOf(turnId, 'completed') });
  active = undefined;
}

async function handle(frame: Frame) {
  const { id, method, params } = frame;
  if (method === undefined) {
    if (id !== undefined) { replies.get(id)?.(frame); replies.delete(id); }
    return;
  }
  if (id === undefined) return;
  if (params === undefined) return failRequest(id, 'Invalid request: missing field `params`');
  if (method === 'initialize') {
    if (initialized) return failRequest(id, 'Already initialized');
    initialized = true;
    return reply(id, { userAgent: 'fake/0.169.0', codexHome: '/nonexistent', platformFamily: 'unix', platformOs: 'macos' });
  }
  if (!initialized) return failRequest(id, 'Not initialized');
  switch (method) {
    case 'account/read':
      return reply(id, scenario === 'signed-out'
        ? { account: null, userId: null, requiresOpenaiAuth: true }
        : { account: { type: 'apiKey' }, userId: null, requiresOpenaiAuth: true });
    case 'model/list': {
      if (scenario === 'oversized') return process.stdout.write(`${JSON.stringify({ id, result: { data: [], nextCursor: null, pad: 'x'.repeat(9 * 1024 * 1024) } })}\n`);
      const model = (name: string, hidden = false) => ({
        id: name, model: name, displayName: name.toUpperCase(), description: '', hidden, isDefault: name === 'gpt-fake',
        supportedReasoningEfforts: [{ reasoningEffort: 'low', description: '' }, { reasoningEffort: 'high', description: '' }],
        defaultReasoningEffort: 'low',
        ...(scenario === 'text-only' ? { inputModalities: ['text'] } : {}),
      });
      return params.cursor === 'page-2'
        ? reply(id, { data: [model('gpt-fake-mini'), model('gpt-hidden', true)], nextCursor: null })
        : reply(id, { data: [model('gpt-fake')], nextCursor: 'page-2' });
    }
    case 'thread/start':
      reply(id, threadResponse(params));
      return notify('thread/started', { thread: thread() });
    case 'thread/resume':
      return params.threadId === threadId ? reply(id, threadResponse(params)) : failRequest(id, `no rollout found for thread id ${params.threadId}`);
    case 'turn/start': {
      if (scenario === 'start-refused') return failRequest(id, 'Turn cannot start.');
      const turnId = `turn-${++turns}`;
      // Stands for the work a turn does in the checkout; steering joins a turn and writes nothing.
      // Named for the message, as a later run may start a new app-server whose turns count from 1 again.
      const writeRoot = process.env.FAKE_CODEX_WRITE_CWD ? params.cwd : process.env.FAKE_CODEX_WRITE;
      if (writeRoot) writeFileSync(join(writeRoot, `codex-${params.clientUserMessageId ?? turnId}.txt`), `${turnId}\n`);
      if (scenario === 'exit-on-turn') process.exit(3);
      policy = params.approvalPolicy ?? policy;
      active = { id: turnId, interrupted: false };
      if (scenario === 'start-delayed') await wait(200);
      reply(id, { turn: turnOf(turnId) });
      const text = params.input?.map((input: { text?: string }) => input.text ?? '').join('') ?? '';
      // Codex reports the user message as items of the turn it starts.
      // `no-client-id` plays a Codex that does not echo the request ID.
      const clientId = scenario === 'no-client-id' ? null : params.clientUserMessageId ?? null;
      setImmediate(() => { user(turnId, text, clientId, `user-${turns}`); void playTurn(turnId); });
      return;
    }
    case 'turn/steer': {
      if (!active || active.id !== params.expectedTurnId) return failRequest(id, 'expected turn is not active');
      reply(id, { turnId: active.id });
      return user(active.id, params.input.map((input: { text?: string }) => input.text ?? '').join(''), params.clientUserMessageId ?? null, `steer-${turns}`);
    }
    case 'fake/finish': {
      reply(id, {});
      if (active) notify('turn/completed', { threadId, turn: turnOf(active.id, 'interrupted') });
      active = undefined; return;
    }
    case 'turn/interrupt': {
      if (scenario === 'interrupt-ignored') return reply(id, {});
      if (!active || active.id !== params.turnId) return failRequest(id, 'no active turn');
      active.interrupted = true;
      reply(id, {});
      notify('turn/completed', { threadId, turn: turnOf(active.id, 'interrupted') });
      active = undefined;
      return;
    }
    case 'fake/hang':
      return;
    default:
      return failRequest(id, `Invalid request: unknown variant \`${method}\``);
  }
}

stdinLines(line => {
  if (log) appendFileSync(log, `${line}\n`);
  let frame: Frame;
  try { frame = JSON.parse(line); } catch { return; }
  handle(frame);
}, () => process.exit(0));
if (scenario === 'stderr') process.stderr.write(Array.from({ length: 40 }, (_, index) => `line ${index}`).join('\n') + '\n');
