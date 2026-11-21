import { lstat } from 'node:fs/promises';
import { join } from 'node:path';
import { fail, HostError } from '../protocol.ts';
import { checkRoot, fileAction, ignoredPaths, pathKey, safePath } from '../safety.ts';
import type { FileIdentity, LeaseHandle, Postimage } from '../safety.ts';
import { clipHead, clipTail } from '../text.ts';
import { trustedPlatform } from './platform.ts';
import { runGuardian, COMMAND_TIMEOUT_MS } from './supervisor.ts';
import { hostAccount, loginEnvironment } from './login-environment.ts';
import type { Account, LoginEnvironment } from './login-environment.ts';

/** The parts of a run that a trusted command reads and reconciles. */
export type TrustedRun = {
  snapshot: { checkout: string };
  baseline: string[];
  owned: Map<string, Postimage>;
  rootIdentity: FileIdentity;
  lease?: LeaseHandle;
  record?: (image: Postimage) => void;
  forget?: (path: string) => void;
  loginEnvironment?: Promise<LoginEnvironment>;
};
export type TrustedOptions = { account?: Account; timeoutMs?: number; capture?: (chunk: string) => void };

const RESULT_LIMIT = 16384;
const LISTED_PATHS = 20;
const NOTICE_LIMIT = 2048;

/** Identity and change times of the paths that were dirty before the run. */
async function dirtyStamps(root: string, paths: string[]) {
  const stamps = new Map<string, string>();
  for (const path of paths) {
    try {
      const info = await lstat(join(root, path), { bigint: true });
      // Ignored build directories change with every build; report files only.
      stamps.set(path, info.isDirectory() ? 'directory'
        : [info.dev, info.ino, info.mode, info.size, info.mtimeNs, info.ctimeNs].join(':'));
    } catch { stamps.set(path, 'absent'); }
  }
  return stamps;
}

function changedDirtyNotice(before: Map<string, string>, after: Map<string, string>) {
  const changed = [...before.keys()].filter(path => before.get(path) !== after.get(path));
  if (!changed.length) return '';
  const listed = changed.slice(0, LISTED_PATHS).join(', ');
  const more = changed.length > LISTED_PATHS ? `, and ${changed.length - LISTED_PATHS} more` : '';
  return clipHead(`Plait: paths that were dirty before this run changed during this command: ${listed}${more}`, NOTICE_LIMIT) + '.\n';
}

const stamp = (nanoseconds: bigint) => `${nanoseconds / 1_000_000_000n}:${nanoseconds % 1_000_000_000n}`;

/**
 * Whether a file still carries its record's identity, size, mode, and times.
 * This only decides which records to drop or refresh; an edit still compares
 * the content hash through the checked file helper.
 */
async function unchanged(root: string, record: Postimage) {
  try {
    const info = await lstat(join(root, record.path), { bigint: true });
    return info.isFile() && String(info.dev) === record.dev && String(info.ino) === record.ino
      && String(info.mode) === record.mode && String(info.size) === record.size
      && stamp(info.mtimeNs) === record.mtime && stamp(info.ctimeNs) === record.ctime;
  } catch { return false; }
}

function drop(run: TrustedRun, record: Postimage) {
  run.owned.delete(pathKey(record.path));
  run.forget?.(record.path);
}

/**
 * Before a command starts, drop every record whose file already changed. That
 * change came from something else, so the command must not launder it into
 * this thread's ownership.
 */
export async function dropDrifted(run: TrustedRun) {
  for (const record of [...run.owned.values()]) {
    if (!await unchanged(run.snapshot.checkout, record)) drop(run, record);
  }
}

/**
 * After the command, a file this thread still owned at its start and that
 * changed during it gets a fresh record of its new state, so the file tools
 * keep working on the thread's own files. Files that are gone, unreadable,
 * ignored, or otherwise unsafe lose their record instead.
 */
export async function refreshChanged(run: TrustedRun) {
  const root = run.snapshot.checkout;
  const changed: Postimage[] = [];
  for (const record of run.owned.values()) if (!await unchanged(root, record)) changed.push(record);
  if (!changed.length) return;
  let ignored: string[] | undefined;
  try { ignored = await ignoredPaths(root, changed.map(record => record.path)); } catch { /* No record survives a failed check. */ }
  for (const record of changed) {
    drop(run, record);
    if (!ignored) continue;
    try {
      safePath(record.path, ignored);
      safePath(record.path, run.baseline);
      const current = (await fileAction(root, 'inspect', record.path, undefined, run.lease?.fd, run.rootIdentity, run.lease?.lifecycleFd)).postimage;
      if (pathKey(current.path) !== pathKey(record.path)) continue;
      // Durable first, as for a file-tool write: memory never outlives the store.
      run.record?.(current);
      run.owned.set(pathKey(current.path), current);
    } catch { /* The file keeps ordinary dirty-path protection. */ }
  }
}

/**
 * Runs one command as the user. Authorization happened in the tool; this owns
 * the environment, the guardian request, and the bookkeeping afterwards.
 */
export async function trustedCommand(run: TrustedRun, text: string, signal: AbortSignal, output: (chunk: string) => void, options: TrustedOptions = {}): Promise<string> {
  trustedPlatform();
  if (signal.aborted) fail('permission_denied', 'Run stopped.');
  const root = run.snapshot.checkout;
  await checkRoot(root, run.rootIdentity);
  // One login-shell read per run keeps later commands fast and consistent.
  const pending = run.loginEnvironment ??= loginEnvironment(options.account ?? hostAccount(), signal);
  let environment: LoginEnvironment;
  try { environment = await pending; }
  catch (error) {
    if (run.loginEnvironment === pending) run.loginEnvironment = undefined;
    throw error;
  }
  let notices = environment.source === 'fallback'
    ? 'Plait: the login-shell environment could not be read; using a minimal PATH.\n' : '';
  const before = await dirtyStamps(root, run.baseline);
  await dropDrifted(run);
  let result = ''; let failure: unknown;
  try {
    result = await runGuardian({ root, text, cleanup: run.lease?.cleanup, env: environment.env, checkoutFd: run.lease?.fd, lifecycleFd: run.lease?.lifecycleFd, trusted: true },
      signal, output, options.timeoutMs ?? COMMAND_TIMEOUT_MS, options.capture);
  } catch (error) { failure = error ?? new Error('Command failed.'); }
  // Reconcile after success, failure, and Stop alike: a killed command still ran.
  try {
    await refreshChanged(run);
    notices += changedDirtyNotice(before, await dirtyStamps(root, run.baseline));
  } catch (error) { failure ??= error; }
  if (failure instanceof HostError) throw new HostError(failure.code, notices + failure.message);
  if (failure) throw failure;
  return notices + clipTail(result, RESULT_LIMIT - notices.length);
}
