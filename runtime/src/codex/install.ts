import { managedPath } from '../agent-tools.ts';
import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { access, stat } from 'node:fs/promises';
import { delimiter, isAbsolute, join } from 'node:path';
import { hostAccount, loginEnvironment } from '../command/login-environment.ts';
import { CODEX_SCHEMA_VERSION } from './protocol.ts';
import type { GetAccountParams, GetAccountResponse, ModelListParams, ModelListResponse } from './protocol.ts';
import type { AppServer } from './server.ts';

export type CodexInstall = {
  state: 'ready' | 'not_installed' | 'unsupported_version' | 'signed_out' | 'error';
  path?: string; version?: string; untested?: boolean; managed?: boolean;
  /** One short sentence for the person, saying how to fix it when something is wrong. */
  message: string;
};
export type CodexModel = { id: string; name: string; efforts: string[]; defaultEffort: string | null; isDefault: boolean; input?: string[] };

const VERSION_TIMEOUT_MS = 5000;
const VERSION_OUTPUT_BYTES = 4096;
const MODEL_PAGES = 20;
const SUPPORTED = `${CODEX_SCHEMA_VERSION.split('.').slice(0, 2).join('.')}`;
const VERSION = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;

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

/** The version in `codex --version` output, such as `codex-cli 0.169.0`. */
export function parseVersionOutput(output: string): string | undefined {
  return /^codex-cli (\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\s*$/m.exec(output)?.[1];
}

async function executable(path: string) {
  try {
    if (!(await stat(path)).isFile()) return false;
    await access(path, constants.X_OK);
    return true;
  } catch { return false; }
}

/** The first `codex` on PATH. A per-user service starts with a minimal PATH, so the login shell's is used unless one is given. */
async function search(env: NodeJS.ProcessEnv | undefined): Promise<{ path?: string; env: NodeJS.ProcessEnv }> {
  const login = env?.PATH ? undefined : await loginEnvironment(hostAccount(), AbortSignal.timeout(15_000));
  const environment = env?.PATH ? env : login!.env;
  const directories = (environment.PATH ?? '').split(delimiter);
  // The fixed fallback PATH lacks the directory Codex's own installer uses.
  if (login?.source === 'fallback') directories.push(join(hostAccount().home, '.local/bin'));
  for (const directory of directories) {
    if (!isAbsolute(directory)) continue;
    const candidate = join(directory, 'codex');
    if (await executable(candidate)) return { path: candidate, env: environment };
  }
  return { env: environment };
}

function runVersion(path: string, env: NodeJS.ProcessEnv): Promise<string | undefined> {
  return new Promise(resolve => {
    execFile(path, ['--version'], { env, timeout: VERSION_TIMEOUT_MS, maxBuffer: VERSION_OUTPUT_BYTES, killSignal: 'SIGKILL' },
      (error, stdout) => resolve(error ? undefined : stdout));
  });
}

/** Grades a reported version against the schema Plait was checked against. */
export function judgeVersion(path: string, version: string): CodexInstall {
  if (compareVersions(version, CODEX_SCHEMA_VERSION) < 0) {
    return { state: 'unsupported_version', path, version, message: `Codex ${version} is too old. Install Codex ${SUPPORTED} or newer.` };
  }
  const [major, minor] = parse(version)!.parts;
  const tested = `${major}.${minor}` === SUPPORTED;
  return tested
    ? { state: 'ready', path, version, message: `Codex ${version} is ready.` }
    : { state: 'ready', path, version, untested: true, message: `Codex ${version} is newer than Plait has tested (${SUPPORTED}); it may not work as expected.` };
}

/** Finds the person's installed Codex and checks its version. Reads no credentials. */
export async function findCodex(options: { path?: string; env?: NodeJS.ProcessEnv; directory?: string }): Promise<CodexInstall> {
  let path = options.path; let env = options.env ?? process.env; let managed = false;
  if (path !== undefined) {
    if (!isAbsolute(path) || !(await executable(path))) {
      return { state: 'not_installed', path, message: 'No Codex program was found at the chosen path. Choose another path or install Codex.' };
    }
  } else {
    const found = await search(options.env);
    env = found.env; path = found.path ?? (options.directory ? await managedPath('codex', options.directory) : undefined); managed = !found.path && !!path;
    if (!path) return { state: 'not_installed', message: `Codex was not found. Install Codex ${SUPPORTED} or newer.` };
  }
  const output = await runVersion(path, env);
  const version = output === undefined ? undefined : parseVersionOutput(output);
  if (!version) return { state: 'error', path, message: 'Codex did not report its version. Reinstall Codex or choose another path.' };
  return { ...judgeVersion(path, version), managed };
}

/** Whether Codex can run turns. Asks the app-server, so no auth file is read. */
export async function codexAccount(server: AppServer): Promise<'signed_in' | 'signed_out'> {
  const params: GetAccountParams = { refreshToken: false };
  const response = await server.request<GetAccountResponse>('account/read', params);
  // A provider that needs no OpenAI auth has no account and still runs.
  return response.account === null && response.requiresOpenaiAuth ? 'signed_out' : 'signed_in';
}

/** The models Codex offers in its picker, across every page. */
export async function codexModels(server: AppServer): Promise<CodexModel[]> {
  const models: CodexModel[] = [];
  let cursor: string | null = null;
  for (let page = 0; page < MODEL_PAGES; page++) {
    const params: ModelListParams = { cursor };
    const response: ModelListResponse = await server.request<ModelListResponse>('model/list', params);
    for (const model of response.data) {
      if (model.hidden) continue;
      models.push({
        // `model` is the slug turn/start takes; `id` names the catalog entry (equal in 0.169.0).
        id: model.model || model.id, name: model.displayName || model.id,
        ...(model.inputModalities ? { input: model.inputModalities } : {}),
        efforts: model.supportedReasoningEfforts.map(option => option.reasoningEffort),
        defaultEffort: model.defaultReasoningEffort || null, isDefault: model.isDefault,
      });
    }
    cursor = response.nextCursor;
    if (!cursor) return models;
  }
  throw new Error('Codex returned too many model pages.');
}
