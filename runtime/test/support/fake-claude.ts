// A scripted stand-in for `claude`, for tests. It never contacts a model.
//   fake-claude --version              prints `$FAKE_CLAUDE_VERSION (Claude Code)` (default 2.1.326)
//   fake-claude auth status --json     prints sign-in state (`signed-out` scenario: logged out)
//   fake-claude --output-format=…      speaks the stream-json and control protocol the Agent SDK uses
// FAKE_CLAUDE_SCENARIO picks the behavior; FAKE_CLAUDE_LOG, when set, receives argv and every stdin line as JSON lines.
// Like Claude Code, Auto and Bypass never ask, Accept edits asks only for commands, and the mode is read when the tool is called.
// `hold` waits before the tool until the mode changes (or 5 s), so a test can switch modes mid-turn.
// FAKE_CLAUDE_WRITE, when set, makes each turn write `claude-N.txt` in its working directory as it starts.
import { appendFileSync, existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { stdinLines } from './stdin-lines.ts';

type Frame = Record<string, any>;
const scenario = process.env.FAKE_CLAUDE_SCENARIO ?? 'turn';
const log = process.env.FAKE_CLAUDE_LOG;
const version = process.env.FAKE_CLAUDE_VERSION ?? '2.1.326';
const KNOWN_SESSION = '6f1c2a4e-0000-4000-8000-000000000001';
const record = (value: object) => { if (log) appendFileSync(log, `${JSON.stringify(value)}\n`); };
const argv = process.argv.slice(2);
record({ argv });
/** Exits once stdout is flushed; pipe writes are asynchronous on macOS. */
const exit = (code: number): never => { process.stdout.write('', () => process.exit(code)); return undefined as never; };

if (argv[0] === '--version') {
  process.stdout.write(scenario === 'no-version' ? 'Claude Code (unknown build)\n' : `${version} (Claude Code)\n`);
  exit(0);
} else if (argv[0] === 'auth' && argv[1] === 'status') {
  const signedIn = scenario !== 'signed-out';
  process.stdout.write(`${JSON.stringify({ loggedIn: signedIn, authMethod: signedIn ? 'claude.ai' : 'none', apiProvider: 'firstParty', email: 'person@example.invalid' })}\n`);
  exit(signedIn ? 0 : 1);
} else serve();

/** The stream-json session the SDK starts. */
function serve() {
  // `--flag=value`, `--flag value`, and bare `--flag`, as the SDK writes them.
  const flags = new Map<string, string | true>();
  for (let index = 0; index < argv.length; index++) {
    const match = /^--([^=]+)(?:=(.*))?$/s.exec(argv[index]);
    if (!match) continue;
    const following = argv[index + 1];
    if (match[2] !== undefined) flags.set(match[1], match[2]);
    else if (following !== undefined && !following.startsWith('--')) { flags.set(match[1], following); index++; }
    else flags.set(match[1], true);
  }
  if (flags.get('input-format') !== 'stream-json' || flags.get('output-format') !== 'stream-json') {
    process.stderr.write('fake claude: expected stream-json input and output\n');
    return exit(2);
  }
  const replay = flags.has('replay-user-messages') && scenario !== 'no-replay';
  const partial = flags.has('include-partial-messages');
  const model = typeof flags.get('model') === 'string' ? flags.get('model') as string : 'claude-fake-1';
  const MODES = ['default', 'acceptEdits', 'plan', 'auto', 'bypassPermissions', 'dontAsk'];
  let mode = typeof flags.get('permission-mode') === 'string' ? flags.get('permission-mode') as string : 'default';
  const resume = flags.get('resume');
  const session = typeof resume === 'string' ? resume : typeof flags.get('session-id') === 'string' ? flags.get('session-id') as string : randomUUID();
  const cwd = process.cwd();

  const write = (frame: object) => process.stdout.write(`${JSON.stringify(frame)}\n`);
  const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

  // Sessions this fake started are remembered in FAKE_CLAUDE_STATE, so a later run can resume them.
  const state = process.env.FAKE_CLAUDE_STATE;
  const remembered = (id: string) => !!state && existsSync(join(state, id));
  if (typeof resume !== 'string' && state) writeFileSync(join(state, session), '');
  if (typeof resume === 'string' && resume !== KNOWN_SESSION && !remembered(resume)) {
    // Like Claude Code: a result naming the failure, then exit 1.
    write({ type: 'result', subtype: 'error_during_execution', is_error: true, errors: [`No conversation found with session ID: ${resume}`],
      duration_ms: 0, duration_api_ms: 0, num_turns: 0, stop_reason: null, total_cost_usd: 0, usage: {}, modelUsage: {}, permission_denials: [],
      uuid: randomUUID(), session_id: resume });
    return exit(1);
  }

  let nextRequest = 1;
  const answers = new Map<string, (frame: Frame) => void>();
  /** Asks the SDK a control request and waits for the answer. */
  const ask = (request: object) => new Promise<Frame>(resolve => {
    const id = `fake-${nextRequest++}`;
    answers.set(id, resolve);
    write({ type: 'control_request', request_id: id, request });
  });
  const respond = (id: string, response: object) => write({ type: 'control_response', response: { subtype: 'success', request_id: id, response } });
  const refuse = (id: string, error: string) => write({ type: 'control_response', response: { subtype: 'error', request_id: id, error } });

  type Sent = { uuid: string; text: string; message: Frame };
  type Turn = { interrupted: boolean; wake?: () => void; approval?: string; consumed: string[] };
  const queue: Sent[] = [];
  let active: Turn | undefined;
  let turns = 0;
  const base = () => ({ uuid: randomUUID(), session_id: session });
  const usage = { input_tokens: 1000, output_tokens: 50, cache_read_input_tokens: 200, cache_creation_input_tokens: 10 };
  const stream = (event: object, extra: object = {}) => { if (partial) write({ type: 'stream_event', event, parent_tool_use_id: null, ...base(), ...extra }); };
  const assistant = (id: string, block: object) => write({
    type: 'assistant', parent_tool_use_id: null, ...base(),
    message: { id, type: 'message', role: 'assistant', model, content: [block], stop_reason: null, stop_sequence: null, usage },
  });
  const echo = (sent: Sent) => { if (replay) write({ type: 'user', message: sent.message, parent_tool_use_id: null, isReplay: true, uuid: sent.uuid, session_id: session }); };
  const textOf = (message: Frame) => typeof message?.content === 'string' ? message.content
    : Array.isArray(message?.content) ? message.content.map((block: Frame) => block?.text ?? '').join('') : '';

  /** Waits `ms`, or less when the turn is interrupted. */
  function pause(turn: Turn, ms: number) {
    return new Promise<void>(resolve => {
      const timer = setTimeout(() => { turn.wake = undefined; resolve(); }, ms);
      turn.wake = () => { clearTimeout(timer); resolve(); };
    });
  }

  function streamText(id: string, index: number, type: 'text' | 'thinking', parts: string[], stamp: object = {}) {
    stream({ type: 'content_block_start', index, content_block: type === 'text' ? { type: 'text', text: '' } : { type: 'thinking', thinking: '', signature: '' } }, stamp);
    for (const part of parts) stream({ type: 'content_block_delta', index, delta: type === 'text' ? { type: 'text_delta', text: part } : { type: 'thinking_delta', thinking: part } });
    stream({ type: 'content_block_stop', index });
    assistant(id, type === 'text' ? { type: 'text', text: parts.join('') } : { type: 'thinking', thinking: parts.join(''), signature: 'sig' });
  }

  function result(turn: Turn, extra: object) {
    write({
      type: 'result', duration_ms: 5, duration_api_ms: 4, num_turns: 1, stop_reason: 'end_turn', total_cost_usd: 0,
      usage, modelUsage: { [model]: { inputTokens: 1000, outputTokens: 50, cacheReadInputTokens: 200, cacheCreationInputTokens: 10, webSearchRequests: 0, costUSD: 0, contextWindow: 200000, maxOutputTokens: 64000 } },
      permission_denials: [], queued_turn_count: queue.length, user_message_uuid: turn.consumed.at(-1), user_message_uuids: turn.consumed, ...base(), ...extra,
    });
  }

  /** Plays one turn: thinking, a tool needing approval, its result, any steering folded in, an answer, the result. */
  async function play(first: Sent) {
    const turn: Turn = { interrupted: false, consumed: [first.uuid] };
    active = turn;
    const n = ++turns;
    if (process.env.FAKE_CLAUDE_WRITE) writeFileSync(join(cwd, `claude-${n}.txt`), `turn ${n}\n`);
    write({
      type: 'system', subtype: 'init', cwd, tools: ['Bash', 'Edit', 'Read'], mcp_servers: [], model, permissionMode: mode, slash_commands: [],
      output_style: 'default', skills: [], plugins: [], apiKeySource: 'none', claude_code_version: version,
      capabilities: ['interrupt_receipt_v1', 'interrupt_cancel_queued_v1'], ...base(),
    });
    echo(first);
    const stamp = { user_message_uuid: first.uuid, user_message_uuids: [first.uuid] };
    const message = `msg-${n}`;
    stream({ type: 'message_start', message: { id: message, type: 'message', role: 'assistant', model, content: [], usage } }, stamp);
    streamText(message, 0, 'thinking', ['Looking at ', 'the files.']);
    const tool = scenario === 'edit' ? { name: 'Edit', input: { file_path: `${cwd}/a.txt`, old_string: 'old', new_string: 'new' } }
      : scenario === 'ask' ? { name: 'AskUserQuestion', input: { questions: [{ question: 'Which file?', header: 'File', options: [], multiSelect: false }] } }
      : { name: 'Bash', input: { command: 'ls -la', description: 'List files' } };
    if (scenario === 'hold') await pause(turn, 5000);
    if (turn.interrupted) return;
    const toolId = `toolu_${n}`;
    stream({ type: 'content_block_start', index: 1, content_block: { type: 'tool_use', id: toolId, name: tool.name, input: {} } });
    stream({ type: 'content_block_delta', index: 1, delta: { type: 'input_json_delta', partial_json: JSON.stringify(tool.input) } });
    stream({ type: 'content_block_stop', index: 1 });
    assistant(message, { type: 'tool_use', id: toolId, name: tool.name, input: tool.input });
    if (scenario === 'exit-mid-turn') return exit(3);
    const asks = !(mode === 'auto' || mode === 'bypassPermissions' || (mode === 'acceptEdits' && tool.name === 'Edit'));
    const request = !asks ? Promise.resolve({ response: { subtype: 'success', response: { behavior: 'allow', updatedInput: tool.input } } }) : ask({
      subtype: 'can_use_tool', tool_name: tool.name, input: tool.input, tool_use_id: toolId,
      permission_suggestions: [{ type: 'addRules', rules: [{ toolName: tool.name }], behavior: 'allow', destination: 'session' }],
      decision_reason: tool.name === 'Bash' ? 'This command needs approval' : undefined,
    });
    turn.approval = asks ? [...answers.keys()].at(-1) : undefined;
    const answer = await request;
    turn.approval = undefined;
    if (turn.interrupted) return;
    if (scenario === 'aborted_tools' || scenario === 'aborted_streaming') {
      result(turn, { subtype: 'error_during_execution', is_error: true, errors: [], terminal_reason: scenario });
      active = undefined;
      return;
    }
    const decision = answer.response?.response ?? {};
    record({ observed: asks ? 'permission' : 'unasked', tool: tool.name, decision, mode });
    const allowed = answer.response?.subtype === 'success' && decision.behavior === 'allow';
    const output = allowed ? (tool.name === 'Edit' ? `The file ${cwd}/a.txt has been updated.` : 'a.txt\nb.txt') : String(decision.message ?? 'Denied');
    write({ type: 'user', parent_tool_use_id: null, ...base(), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: toolId, content: output, is_error: !allowed }] } });
    if (scenario === 'slow') await pause(turn, 5000);
    if (turn.interrupted) return;
    // Messages sent during the turn join it here, before the answer.
    while (queue.length) { const sent = queue.shift()!; turn.consumed.push(sent.uuid); echo(sent); }
    const reply = `msg-${n}-b`;
    stream({ type: 'message_start', message: { id: reply, type: 'message', role: 'assistant', model, content: [], usage } });
    streamText(reply, 0, 'text', turn.consumed.length > 1 ? ['Two files, ', 'and noted.'] : ['Two ', 'files.']);
    result(turn, { subtype: 'success', is_error: false, result: 'Two files.', terminal_reason: 'completed' });
    active = undefined;
    next();
  }

  /** Starts the next queued message as its own turn, as Claude Code does after a result. */
  function next() {
    if (active || !queue.length) return;
    void play(queue.shift()!);
  }

  function control(frame: Frame) {
    const id = frame.request_id; const request = frame.request ?? {};
    switch (request.subtype) {
      case 'initialize':
        return respond(id, {
          commands: [], agents: [], output_style: 'default', available_output_styles: ['default'], claude_code_version: version,
          current_permission_mode: mode, capabilities: [],
          account: { apiProvider: 'firstParty' },
          models: [
            { value: 'default', resolvedModel: 'claude-fake-1', displayName: 'Default (recommended)', description: 'Fake default', supportsEffort: true, supportedEffortLevels: ['low', 'medium', 'high', 'xhigh', 'max'], supportsAdaptiveThinking: true },
            { value: 'fake-small', resolvedModel: 'claude-fake-small', displayName: 'Fake Small', description: 'Fake older model', supportsEffort: true, supportedEffortLevels: ['low', 'medium', 'high', 'max'], supportsAdaptiveThinking: true },
            { value: 'fake-budget', displayName: 'Fake Budget', description: 'Takes a token budget' },
          ],
        });
      case 'set_permission_mode':
        if (!MODES.includes(request.mode)) return refuse(id, `Cannot set permission mode: must be one of ${MODES.join(', ')}`);
        mode = request.mode;
        respond(id, {});
        active?.wake?.();
        return write({ type: 'system', subtype: 'status', status: null, permissionMode: mode, ...base() });
      case 'interrupt': {
        const turn = active;
        const cancelled = request.cancel_queued === true ? queue.splice(0).map(sent => sent.uuid) : [];
        respond(id, { still_queued: queue.map(sent => sent.uuid), ...(request.cancel_queued === true ? { cancelled } : {}) });
        if (!turn) return;
        turn.interrupted = true;
        turn.wake?.();
        if (turn.approval) {
          // Claude Code withdraws the permission prompt it was waiting on.
          write({ type: 'control_cancel_request', request_id: turn.approval });
          answers.get(turn.approval)?.({ response: { subtype: 'error', error: 'cancelled' } });
          answers.delete(turn.approval);
        }
        result(turn, { subtype: 'error_during_execution', is_error: true, errors: [], terminal_reason: 'aborted_streaming', stop_reason: null });
        active = undefined;
        return next();
      }
      default:
        return refuse(id, `Unsupported control request: ${request.subtype}`);
    }
  }

  stdinLines(line => {
    record({ stdin: line });
    let frame: Frame;
    try { frame = JSON.parse(line); } catch { return; }
    if (frame.type === 'control_request') return control(frame);
    if (frame.type === 'control_response') {
      const id = frame.response?.request_id;
      answers.get(id)?.(frame); answers.delete(id);
      return;
    }
    if (frame.type === 'user') {
      const sent: Sent = { uuid: typeof frame.uuid === 'string' ? frame.uuid : randomUUID(), text: textOf(frame.message), message: frame.message };
      queue.push(sent);
      next();
    }
  }, () => { void wait(10).then(() => process.exit(0)); });
}
