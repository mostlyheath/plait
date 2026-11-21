import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { homedir, userInfo } from 'node:os';
import { basename, isAbsolute } from 'node:path';
import { fail } from '../protocol.ts';

export type Account = { home: string; shell: string; user: string };
export type LoginEnvironment = { env: Record<string, string>; source: 'login-shell' | 'fallback' };

export const LOGIN_ENVIRONMENT_TIMEOUT_MS = 10_000;
const OUTPUT_LIMIT = 1_048_576;
const SYSTEM_PATH = '/usr/bin:/bin:/usr/sbin:/sbin';
// Session facts a per-user service legitimately holds. Nothing else is inherited.
const INHERITED = ['TMPDIR', 'LANG', 'LC_ALL', 'LC_CTYPE', 'SSH_AUTH_SOCK', '__CF_USER_TEXT_ENCODING'];
// Host plumbing must never reach a command that runs with full user access.
const HOST_PRIVATE = ['PLAIT_HOST_EXE', 'PLAIT_LOCK_FD', 'PLAIT_NOTIFY_FD'];
// The capturing shell's own state would be stale in the command's shell.
const SHELL_STATE = ['PWD', 'OLDPWD', 'SHLVL', '_'];

const quote = (text: string) => `'${text.replaceAll("'", "'\\''")}'`;

/** The account the host serves. `run-user` sets HOME from the OS account. */
export function hostAccount(): Account {
  const info = userInfo();
  const shell = info.shell && isAbsolute(info.shell) ? info.shell : process.platform === 'linux' ? '/bin/sh' : '/bin/zsh';
  return { home: homedir(), shell, user: info.username };
}

function baseEnvironment(account: Account): Record<string, string> {
  const env: Record<string, string> = {
    HOME: account.home, USER: account.user, LOGNAME: account.user, SHELL: account.shell, PATH: SYSTEM_PATH,
  };
  for (const key of INHERITED) {
    const value = process.env[key];
    if (value) env[key] = value;
  }
  return env;
}

function fallbackEnvironment(account: Account): LoginEnvironment {
  const env = baseEnvironment(account);
  const homebrew = process.platform === 'darwin' ? '/opt/homebrew/bin:/opt/homebrew/sbin:' : '';
  env.PATH = `${account.home}/.cargo/bin:${homebrew}/usr/local/bin:${SYSTEM_PATH}`;
  return { env, source: 'fallback' };
}

function loginFlags(shell: string) {
  // Interactive startup files are where nvm and similar tools extend PATH.
  return ['zsh', 'bash', 'fish'].includes(basename(shell)) ? '-i -l -c' : '-l -c';
}

function parse(output: string, marker: string): Record<string, string> | undefined {
  const start = output.indexOf(marker); const end = output.lastIndexOf(marker);
  if (start < 0 || end <= start) return undefined;
  try {
    const value: unknown = JSON.parse(output.slice(start + marker.length, end));
    if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
    const entries = Object.entries(value);
    if (!entries.every(([, item]) => typeof item === 'string')) return undefined;
    return Object.fromEntries(entries) as Record<string, string>;
  } catch { return undefined; }
}

/** Runs the login shell under the guardian so it also ends with Stop or host death. */
function capture(account: Account, signal: AbortSignal, timeoutMs: number): Promise<Record<string, string> | undefined> {
  const executable = process.env.PLAIT_HOST_EXE;
  const node = process.execPath;
  // Startup files may print anything; only text between the markers is read.
  const marker = `plait-env-${randomUUID()}`;
  if (!executable || /['\\]/.test(node) || /['\\]/.test(account.shell)) return Promise.resolve(undefined);
  const print = `process.stdout.write("${marker}"+JSON.stringify(process.env)+"${marker}")`;
  const script = `exec ${quote(account.shell)} ${loginFlags(account.shell)} ${quote(`${quote(node)} -e ${quote(print)}`)}`;
  return new Promise(resolve => {
    const child = spawn(executable, ['trusted-command', account.home, script], {
      cwd: account.home, env: baseEnvironment(account), stdio: ['pipe', 'pipe', 'ignore'],
    });
    let output = ''; let settled = false;
    const cancel = () => { child.stdin!.end(); };
    const timeout = setTimeout(cancel, timeoutMs);
    const finish = (value?: Record<string, string>) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      signal.removeEventListener('abort', cancel);
      cancel();
      resolve(value);
    };
    child.stdin!.on('error', () => {});
    signal.addEventListener('abort', cancel, { once: true });
    child.stdout!.setEncoding('utf8').on('data', (chunk: string) => {
      output += chunk;
      if (output.length > OUTPUT_LIMIT) { output = ''; cancel(); }
    });
    child.once('error', () => finish());
    child.once('close', code => finish(code === 0 ? parse(output, marker) : undefined));
  });
}

/**
 * The environment a trusted command receives. A per-user service starts with a
 * minimal PATH, so ask the account's login shell, as a new terminal would. If
 * that fails, use a fixed fallback instead of blocking every command.
 */
export async function loginEnvironment(account: Account, signal: AbortSignal, timeoutMs = LOGIN_ENVIRONMENT_TIMEOUT_MS): Promise<LoginEnvironment> {
  if (signal.aborted) fail('permission_denied', 'Run stopped.');
  const captured = await capture(account, signal, timeoutMs);
  if (signal.aborted) fail('permission_denied', 'Run stopped.');
  if (!captured?.PATH) return fallbackEnvironment(account);
  const env = { ...baseEnvironment(account), ...captured };
  for (const key of [...HOST_PRIVATE, ...SHELL_STATE]) delete env[key];
  return { env, source: 'login-shell' };
}
