import { managedPath } from '../agent-tools.ts';
// Finding the person's installed Claude Code, checking its version, and asking
// it which models it offers and whether it is signed in, without a prompt.
//
// Plait reads none of Claude Code's files or credentials. Sign-in state comes
// from `claude auth status --json`, of which only `loggedIn` and `apiProvider`
// are kept; models come from the answer to the SDK's `initialize` request.
import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { access, stat } from 'node:fs/promises';
import { delimiter, isAbsolute, join } from 'node:path';
import type { ModelInfo, SDKControlInitializeResponse, SDKUserMessage } from '@anthropic-ai/claude-agent-sdk';
import { hostAccount, loginEnvironment } from '../command/login-environment.ts';
import { THINKING_LEVELS } from '../protocol.ts';
import type { ThinkingLevel } from '../protocol.ts';
import type { QueryFunction } from './sdk.ts';

/** The pinned Agent SDK, and the Claude Code release its message types match (its package.json `claudeCodeVersion`). */
export const CLAUDE_SDK_VERSION = '0.3.326';
export const CLAUDE_TESTED_VERSION = '2.1.326';
/**
 * The oldest Claude Code Plait runs. 2.1.290 is the first release whose
 * `--replay-user-messages` holds a message's echo until a turn takes it, which
 * is how Plait places steering messages (SDK changelog 0.3.290), and it is
 * after 2.1.269, from which plan mode sends writes through `canUseTool` even
 * when bypass can be switched on later (0.3.269). Everything else Plait uses
 * (stream-json input, `can_use_tool`, `interrupt`, `set_permission_mode`,
 * `--resume`, partial messages, effort levels in `initialize`) is older.
 */
export const CLAUDE_MIN_VERSION = '2.1.290';

export type ClaudeInstall = {
  state: 'ready' | 'not_installed' | 'unsupported_version' | 'error';
  path?: string; version?: string; untested?: boolean; managed?: boolean;
  /** One short sentence for the person, saying how to fix it when something is wrong. */
  message: string;
};
export type ClaudeModel = {
  /** What `--model` takes: an alias such as `opus`, or a full model ID. `default` means Claude Code's own choice. */
  id: string; name: string; description: string; resolved: string | null;
  /** Plait's thinking levels this model offers, in Plait's order. */
  levels: ThinkingLevel[];
  adaptive: boolean; isDefault: boolean;
};
export type ClaudeCatalog = {
  state: 'ready' | 'signed_out' | 'unavailable';
  version: string | null; models: ClaudeModel[];
  /** `firstParty` for an Anthropic sign-in; anything else signs in outside Claude Code (cloud credentials, a gateway). */
  provider: string | null;
  note: string | null;
  /** How long the catalog took, for the latency budget. */
  ms: number;
};

const VERSION_TIMEOUT_MS = 5000;
const VERSION_OUTPUT_BYTES = 4096;
const AUTH_TIMEOUT_MS = 10_000;
const AUTH_OUTPUT_BYTES = 65536;
const CATALOG_TIMEOUT_MS = 30_000;
const MAX_MODELS = 64;
const VERSION = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;
const TESTED_LINE = CLAUDE_TESTED_VERSION.split('.').slice(0, 2).join('.');

type Parsed = { parts: [number, number, number]; pre?: string };
function parse(version: string): Parsed | undefined {
  const match = VERSION.exec(version);
  return match ? { parts: [Number(match[1]), Number(match[2]), Number(match[3])], pre: match[4] } : undefined;
}

/** Orders `X.Y.Z[-pre]` versions; a prerelease sorts before its release. Throws on anything else. */
export function compareVersions(a: string, b: string): -1 | 0 | 1 {
  const left = parse(a); const right = parse(b);
  if (!left || !right) throw new Error(`Not a version: ${left ? b : a}`);
  for (let index = 0; index < 3; index++) {
    if (left.parts[index] !== right.parts[index]) return left.parts[index] < right.parts[index] ? -1 : 1;
  }
  if (left.pre === right.pre) return 0;
  if (left.pre === undefined) return 1;
  if (right.pre === undefined) return -1;
  return left.pre < right.pre ? -1 : 1;
}

/** The version in `claude --version` output, such as `2.1.326 (Claude Code)`. */
export function parseVersionOutput(output: string): string | undefined {
  return /^(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?) \(Claude Code\)\s*$/m.exec(output)?.[1];
}

/**
 * Grades a reported version. Older than the minimum is refused. Claude Code
 * ships a 2.1.x release most days, so any release on the tested line runs
 * without a note; a later line runs with an untested note.
 */
export function judgeVersion(path: string, version: string): ClaudeInstall {
  if (compareVersions(version, CLAUDE_MIN_VERSION) < 0) {
    return { state: 'unsupported_version', path, version, message: `Claude Code ${version} is too old. Run \`claude update\`, or install ${CLAUDE_MIN_VERSION} or newer.` };
  }
  const [major, minor] = parse(version)!.parts;
  return `${major}.${minor}` === TESTED_LINE
    ? { state: 'ready', path, version, message: `Claude Code ${version} is ready.` }
    : { state: 'ready', path, version, untested: true, message: `Claude Code ${version} is newer than Plait has tested (${CLAUDE_TESTED_VERSION}); it may not work as expected.` };
}

async function executable(path: string) {
  try {
    if (!(await stat(path)).isFile()) return false;
    await access(path, constants.X_OK);
    return true;
  } catch { return false; }
}

/** The first `claude` on PATH. A per-user service starts with a minimal PATH, so the login shell's is used unless one is given. */
async function search(env: NodeJS.ProcessEnv | undefined): Promise<{ path?: string; env: NodeJS.ProcessEnv }> {
  const login = env?.PATH ? undefined : await loginEnvironment(hostAccount(), AbortSignal.timeout(15_000));
  const environment = env?.PATH ? env : login!.env;
  const directories = (environment.PATH ?? '').split(delimiter);
  // The fixed fallback PATH lacks the directory Claude Code's native installer uses.
  if (login?.source === 'fallback') directories.push(join(hostAccount().home, '.local/bin'));
  for (const directory of directories) {
    if (!isAbsolute(directory)) continue;
    const candidate = join(directory, 'claude');
    if (await executable(candidate)) return { path: candidate, env: environment };
  }
  return { env: environment };
}

function run(path: string, args: string[], env: NodeJS.ProcessEnv, timeout: number, maxBuffer: number): Promise<{ stdout: string; code: number } | undefined> {
  return new Promise(resolve => {
    execFile(path, args, { env, timeout, maxBuffer, killSignal: 'SIGKILL' }, (error, stdout) => {
      // A non-zero exit still reports; a timeout, kill, or overflow does not.
      if (error && (typeof error.code !== 'number' || error.killed)) return resolve(undefined);
      resolve({ stdout: String(stdout), code: typeof error?.code === 'number' ? error.code : 0 });
    });
  });
}

/** Finds the person's installed Claude Code and checks its version. Reads no credentials. */
export async function findClaude(options: { path?: string; env?: NodeJS.ProcessEnv; directory?: string }): Promise<ClaudeInstall> {
  let path = options.path; let env = options.env ?? process.env; let managed = false;
  if (path !== undefined) {
    if (!isAbsolute(path) || !(await executable(path))) {
      return { state: 'not_installed', path, message: 'No Claude Code program was found at the chosen path. Choose another path or install Claude Code.' };
    }
  } else {
    const found = await search(options.env);
    env = found.env; path = found.path ?? (options.directory ? await managedPath('claude', options.directory) : undefined); managed = !found.path && !!path;
    if (!path) return { state: 'not_installed', message: `Claude Code was not found. Install Claude Code ${CLAUDE_MIN_VERSION} or newer.` };
  }
  const output = await run(path, ['--version'], env, VERSION_TIMEOUT_MS, VERSION_OUTPUT_BYTES);
  const version = output?.code === 0 ? parseVersionOutput(output.stdout) : undefined;
  if (!version) return { state: 'error', path, message: 'Claude Code did not report its version. Reinstall Claude Code or choose another path.' };
  return { ...judgeVersion(path, version), managed };
}

/**
 * Plait's levels for one model. Claude Code's effort levels map by name
 * (`low` … `max`). Models with adaptive thinking cannot turn thinking off, and
 * Plait's `minimal` has no Claude equivalent, so neither is offered for them.
 * A model with neither effort nor adaptive thinking takes a token budget, so
 * it offers off, low, medium, and high.
 */
export function claudeLevels(model: Pick<ModelInfo, 'supportsEffort' | 'supportedEffortLevels' | 'supportsAdaptiveThinking'>): ThinkingLevel[] {
  if (model.supportsEffort && model.supportedEffortLevels?.length) {
    const offered = new Set<string>(model.supportedEffortLevels);
    return THINKING_LEVELS.filter(level => offered.has(level));
  }
  return model.supportsAdaptiveThinking ? [] : ['off', 'low', 'medium', 'high'];
}

const bounded = (text: unknown, limit: number) => typeof text === 'string' ? text.slice(0, limit) : '';

export function catalogModel(info: ModelInfo): ClaudeModel {
  return {
    id: bounded(info.value, 256), name: bounded(info.displayName, 256) || bounded(info.value, 256),
    description: bounded(info.description, 512), resolved: typeof info.resolvedModel === 'string' ? bounded(info.resolvedModel, 256) : null,
    levels: claudeLevels(info), adaptive: info.supportsAdaptiveThinking === true, isDefault: info.value === 'default',
  };
}

/**
 * Sign-in state from `claude auth status --json`. Only two fields are read;
 * the rest (which names the account) is dropped unparsed into anything kept.
 * Undefined when Claude Code did not answer in the expected form.
 */
export async function claudeAuth(path: string, env: NodeJS.ProcessEnv): Promise<{ signedIn: boolean; provider: string | null } | undefined> {
  const output = await run(path, ['auth', 'status', '--json'], env, AUTH_TIMEOUT_MS, AUTH_OUTPUT_BYTES);
  if (!output) return undefined;
  try {
    const status = JSON.parse(output.stdout) as { loggedIn?: unknown; apiProvider?: unknown };
    if (typeof status.loggedIn !== 'boolean') return undefined;
    const provider = typeof status.apiProvider === 'string' ? status.apiProvider.slice(0, 64) : null;
    // Cloud providers and gateways authenticate outside Claude Code's own sign-in.
    return { signedIn: status.loggedIn || (provider !== null && provider !== 'firstParty'), provider };
  } catch { return undefined; }
}

/** An input stream that sends nothing and ends when `end` is called. */
function silentInput(): { stream: AsyncIterable<SDKUserMessage>; end(): void } {
  let end = () => {};
  const done = new Promise<IteratorResult<SDKUserMessage>>(resolve => { end = () => resolve({ done: true, value: undefined }); });
  return { stream: { [Symbol.asyncIterator]: () => ({ next: () => done }) }, end };
}

/**
 * The answer to `initialize` from a Claude Code that is started and closed
 * without any user message, so no model request is made. Hooks are off and
 * only the (empty) given MCP configuration loads, so the person's SessionStart
 * hooks do not run and no MCP server starts just to list models; the session
 * is not saved.
 */
export async function claudeInitialize(query: QueryFunction, path: string, env: NodeJS.ProcessEnv, cwd: string): Promise<SDKControlInitializeResponse> {
  const input = silentInput();
  const session = query({
    prompt: input.stream,
    options: {
      pathToClaudeCodeExecutable: path, env, cwd, persistSession: false, strictMcpConfig: true,
      settings: { disableAllHooks: true }, permissionMode: 'default', stderr: () => {},
    },
  });
  // Messages must be read for the SDK to see the control responses.
  void (async () => { try { for await (const _ of session) { /* nothing is expected */ } } catch { /* reported by initialize */ } })();
  const timer = setTimeout(() => session.close(), CATALOG_TIMEOUT_MS);
  try {
    return await session.initializationResult();
  } finally {
    clearTimeout(timer);
    session.close(); input.end();
  }
}

type Auth = Awaited<ReturnType<typeof claudeAuth>>;

/** The catalog from a sign-in check and an `initialize` answer (or the error starting Claude Code). */
export function catalogFrom(auth: Auth, init: SDKControlInitializeResponse | Error, ms: number): ClaudeCatalog {
  if (auth && !auth.signedIn) {
    return { state: 'signed_out', version: null, models: [], provider: auth.provider, note: 'Sign in from Settings → Agents, or run `claude auth login` on this machine.', ms };
  }
  if (init instanceof Error) {
    // Claude Code's own error text can quote the machine or the account, so it is not passed on.
    return { state: 'unavailable', version: null, models: [], provider: auth?.provider ?? null, note: 'Claude Code did not start. Run `claude` in a terminal to see why.', ms };
  }
  const models = (Array.isArray(init.models) ? init.models : []).slice(0, MAX_MODELS)
    .filter(model => typeof model?.value === 'string' && model.value).map(catalogModel);
  const version = typeof init.claude_code_version === 'string' ? init.claude_code_version.slice(0, 64) : null;
  return { state: 'ready', version, models, provider: auth?.provider ?? init.account?.apiProvider ?? null, note: null, ms };
}

/** Models and sign-in state, without sending a prompt. Starts Claude Code once, briefly. */
export async function claudeCatalog(options: { query: QueryFunction; path: string; env: NodeJS.ProcessEnv; cwd?: string }): Promise<ClaudeCatalog> {
  const started = performance.now();
  const cwd = options.cwd ?? options.env.HOME ?? hostAccount().home;
  const [auth, init] = await Promise.all([
    claudeAuth(options.path, options.env),
    claudeInitialize(options.query, options.path, options.env, cwd).catch(error => error as Error),
  ]);
  return catalogFrom(auth, init, Math.round(performance.now() - started));
}
