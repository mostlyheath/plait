import { resourceError } from './run-capacity.ts';
import { execFile, spawn } from 'node:child_process';
import { realpath, lstat, mkdir, open } from 'node:fs/promises';
import type { FileHandle } from 'node:fs/promises';
import { dirname, isAbsolute, join } from 'node:path';
import { homedir } from 'node:os';
import { constants } from 'node:fs';
import { promisify } from 'node:util';
import { fail, HostError } from './protocol.ts';
import type { CleanupProof } from './cleanup.ts';

const execute = promisify(execFile);
const gitEnv = { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: '/nonexistent', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_OPTIONAL_LOCKS: '0', GIT_TERMINAL_PROMPT: '0', LC_ALL: 'C' };
const gitOptions = ['-c', 'core.fsmonitor=false', '-c', 'core.untrackedCache=false', '-c', 'core.hooksPath=/dev/null', '-c', 'diff.external='];

async function git(root: string, args: string[], input?: string) {
  const operation = execute('/usr/bin/git', [...gitOptions, '-C', root, ...args], { env: gitEnv, timeout: 10000, maxBuffer: 1048576, encoding: 'buffer' });
  if (input !== undefined) operation.child.stdin!.end(input);
  const result = await operation;
  return new TextDecoder('utf-8', { fatal: true }).decode(result.stdout);
}
export type ProjectVcs = 'git' | 'folder';
export async function projectKind(path: string): Promise<{root: string; vcs: ProjectVcs}> {
  if (!isAbsolute(path)) fail('invalid_request', 'Checkout must be absolute.');
  try {
    const root = await realpath(path);
    if (!(await lstat(root)).isDirectory()) throw new Error('Not a directory');
    // Plain only when no repository marker exists here or above, as Git itself
    // decides: a `.git` file, or a `.git` folder with HEAD. A broken repository
    // with a marker stays a repository and keeps Git's refusal.
    let current = root;
    for (;;) {
      if (await repositoryMarker(current)) break;
      const parent = dirname(current);
      if (parent === current) return { root, vcs: 'folder' };
      current = parent;
    }
    return { root: await realpath((await git(root, ['rev-parse', '--show-toplevel'])).trim()), vcs: 'git' };
  } catch (error) { const classified = resourceError(error); if (classified !== error) throw classified; fail('invalid_request', 'Checkout must be an accessible Git working tree.'); }
}
async function repositoryMarker(folder: string): Promise<boolean> {
  try {
    const entry = await lstat(join(folder, '.git'));
    if (!entry.isDirectory()) return true;
    await lstat(join(folder, '.git', 'HEAD'));
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
    throw error;
  }
}
export async function checkout(path: string): Promise<string> { return (await projectKind(path)).root; }
export async function gitCheckout(path: string): Promise<void> {
  try {
    if (await realpath((await git(path, ['rev-parse', '--show-toplevel'])).trim()) !== path) throw new Error('Checkout root changed');
  }
  catch (error) { const classified = resourceError(error); if (classified !== error) throw classified; fail('invalid_request', 'Checkout must be an accessible Git working tree.'); }
}
/** A linked worktree keeps its objects and refs in the main repository's Git
 * directory, outside its own root. Returns that directory, or undefined when
 * it lies inside the root as usual. Fixed per root, so it is remembered. */
const sharedDirs = new Map<string, string | undefined>();
export async function sharedGitDir(root: string): Promise<string | undefined> {
  if ((await projectKind(root)).vcs === 'folder') return undefined;
  if (sharedDirs.has(root)) return sharedDirs.get(root);
  const dir = await realpath((await git(root, ['rev-parse', '--path-format=absolute', '--git-common-dir'])).trim());
  const shared = dir === root || dir.startsWith(root + '/') ? undefined : dir;
  sharedDirs.set(root, shared);
  return shared;
}
/** `vcs` is the thread's stored kind. Without it the checkout is treated as
 * Git, so a repository that lost its `.git` is refused, never unprotected. */
export async function dirtyPaths(root: string, vcs: ProjectVcs = 'git'): Promise<string[]> {
  if (vcs === 'folder') return [];
  let names = '';
  try { names = await git(root, ['config', '--name-only', '--get-regexp', '^filter\\..*\\.(clean|process)$']); }
  catch (error) { if ((error as { code?: number }).code !== 1) throw error; }
  const filters = names.trim().split('\n').filter(Boolean).flatMap(key => ['-c', `${key}=`]);
  const result = await git(root, [...filters, 'status', '--porcelain=v1', '-z', '--untracked-files=all', '--ignored=matching', '--ignore-submodules=all']);
  const entries = result.split('\0');
  const paths: string[] = [];
  for (let i = 0; i < entries.length; i++) {
    const item = entries[i];
    if (!item) continue;
    paths.push(item.slice(3).replace(/\/$/, ''));
    if (item[0] === 'R' || item[0] === 'C') paths.push(entries[++i]);
  }
  // Submodules are separate checkouts; never run their status hooks or edit through them.
  const tracked = await git(root, ['ls-files', '--stage', '-z']);
  for (const entry of tracked.split('\0')) if (entry.startsWith('160000 ')) paths.push(entry.slice(entry.indexOf('\t') + 1));
  if (paths.length > 500 || Buffer.byteLength(JSON.stringify(paths)) > 32768) fail('permission_denied', 'Dirty baseline exceeds the safety bound.');
  return [...new Set(paths)];
}
export type FileIdentity = { dev: string; ino: string };
export type Postimage = FileIdentity & { path: string; sha256: string; mode: string; size: string; mtime: string; ctime: string };
export type FileResult = { text: string; postimage: Postimage };
export function sameImage(left: Postimage, right: Postimage) {
  return (['path', 'sha256', 'dev', 'ino', 'mode', 'size', 'mtime', 'ctime'] as const).every(key => left[key] === right[key]);
}
export const pathKey = (path: string) => path.normalize('NFD').toLowerCase();
export async function ignoredPaths(root: string, paths: string[], vcs: ProjectVcs = 'git'): Promise<string[]> {
  if (vcs === 'folder') return [];
  if (!paths.length) return [];
  try { return (await git(root, ['check-ignore', '--no-index', '-z', '--stdin'], paths.join('\0') + '\0')).split('\0').filter(Boolean); }
  catch (error) { if ((error as { code?: number }).code === 1) return []; throw error; }
}
export async function rootIdentity(root: string): Promise<FileIdentity> {
  const info = await lstat(root, { bigint: true });
  if (!info.isDirectory() || await realpath(root) !== root) fail('permission_denied', 'Checkout root changed.');
  return { dev: String(info.dev), ino: String(info.ino) };
}
export async function verifyExpectedIdentity(root: string, expected: unknown): Promise<void> {
  if (expected === undefined) return;
  const current = await rootIdentity(root);
  const identity = expected as FileIdentity;
  if (current.dev !== identity.dev || current.ino !== identity.ino) fail('invalid_request', 'This folder changed since it was opened.');
}
export async function checkRoot(root: string, expected: FileIdentity) {
  const current = await rootIdentity(root);
  if (current.dev !== expected.dev || current.ino !== expected.ino) fail('permission_denied', 'Checkout root identity changed. Create a new thread for the replacement.');
}
export async function identities(root: string, paths: string[]): Promise<FileIdentity[]> {
  const result: FileIdentity[] = [];
  for (const path of paths) {
    try {
      const info = await lstat(join(root, path), { bigint: true });
      result.push({ dev: String(info.dev), ino: String(info.ino) });
    } catch (error) { if ((error as { code?: string }).code !== 'ENOENT') throw error; }
  }
  return result;
}
export function safePath(path: string, dirty: string[] = []) {
  const parts = path.split('/');
  if (!path || isAbsolute(path) || parts.some(part => !part || part === '.' || part === '..' || part.toLowerCase() === '.git') || path.includes('\0')) {
    fail('permission_denied', 'Path must stay inside the checkout and outside Git metadata.');
  }
  const folded = path.normalize('NFD').toLowerCase();
  if (dirty.some(raw => {
    const item = raw.normalize('NFD').toLowerCase();
    return folded === item || folded.startsWith(`${item}/`) || item.startsWith(`${folded}/`);
  })) {
    fail('permission_denied', 'Pre-existing dirty paths are protected.');
  }
}
export type LeaseClass = 'reader' | 'writer' | 'exclusive';
export type LeaseHandle = {
  cleanup?: CleanupProof;
  fd?: number;
  lifecycleFd?: number;
  close(): Promise<void>;
};
export type CheckoutLease = LeaseHandle & {
  fd: number | undefined;
  lifecycleFd: number;
  class: LeaseClass;
  root: string;
  identity: FileIdentity;
};
export type CommonMetadataLease = {
  fd: number;
  commonDir: string;
  identity: FileIdentity;
  close(): Promise<void>;
};
type PreparedLock = { root: string; dev: string; ino: string; lock_path: string; lock_dev: string; lock_ino: string };

async function prepareLease(kind: 'checkout' | 'common', path: string): Promise<PreparedLock> {
  const executable = process.env.PLAIT_HOST_EXE;
  if (!executable) fail('internal', 'Host lease helper is not configured.');
  return await new Promise<PreparedLock>((resolve, reject) => {
    const child = spawn(executable, ['lease-prepare', kind, path], { stdio: ['ignore', 'pipe', 'pipe'] });
    let out = ''; let err = '';
    child.stdout!.setEncoding('utf8').on('data', chunk => { out += chunk; });
    child.stderr!.setEncoding('utf8').on('data', chunk => { err = (err + chunk).slice(-4096); });
    child.once('error', reject);
    child.once('close', code => {
      if (code !== 0) {
        const reason = err.trim().replace(/^plait-host:\s*/, '');
        return reject(new HostError('permission_denied', reason || 'Checkout lease could not be prepared.'));
      }
      try { resolve(JSON.parse(out)); } catch { reject(new HostError('internal', 'Invalid lease helper output.')); }
    });
  });
}

function lockFailure(stderr: string, fallback: string): HostError {
  const line = stderr.trim().replace(/^plait-host:\s*/, '');
  if (line.startsWith('cleanup_uncertain:')) return new HostError('cleanup_uncertain', line.slice('cleanup_uncertain:'.length).trim());
  const busyIndex = line.indexOf('busy:');
  if (busyIndex >= 0) return new HostError('busy', line.slice(busyIndex + 5).trim() || fallback);
  if (line.includes('identity:')) return new HostError('permission_denied', 'Checkout changed while taking its lease.');
  return new HostError('permission_denied', line ? `Checkout lease could not be acquired: ${line}` : 'Checkout lease could not be acquired safely.');
}

/** The per-user lease directory, as `plait_core::lease::default_lease_dir` finds it. */
function leaseDirectory(): string {
  const configured = process.env.PLAIT_LEASE_DIR;
  if (configured === undefined) return join(homedir(), '.local/share/plait-leases');
  if (!isAbsolute(configured)) fail('permission_denied', 'PLAIT_LEASE_DIR must be an absolute path.');
  return configured;
}
const lockHex = (value: string) => BigInt(value).toString(16).padStart(16, '0');
/** `plait_core::lease::checkout_lock_name`; `lock-fd` refuses a descriptor for any other file. */
export const checkoutLockName = (identity: FileIdentity) => `checkout-${lockHex(identity.dev)}-${lockHex(identity.ino)}.lock`;

/**
 * Opens (creating if needed) this checkout's lifecycle lock file without a
 * helper process: one `lock-fd` per lease is the cost a run paid before
 * lifecycle locks. `lock-fd` still verifies the directory, the file's identity,
 * owner, mode and link count, so a disagreement here fails closed.
 */
async function openLifecycle(root: string): Promise<{ root: string; identity: FileIdentity; lifecycle: FileHandle }> {
  const canonical = await realpath(root).catch(() => fail('permission_denied', 'Checkout root changed.'));
  const identity = await rootIdentity(canonical);
  const directory = leaseDirectory();
  try {
    await mkdir(dirname(directory), { recursive: true });
    await mkdir(directory, { mode: 0o700 }).catch((error: NodeJS.ErrnoException) => { if (error.code !== 'EEXIST') throw error; });
    const lifecycle = await open(join(directory, checkoutLockName(identity)), constants.O_RDWR | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
    return { root: canonical, identity, lifecycle };
  } catch (error) {
    const classified = resourceError(error); if (classified !== error) throw classified;
    fail('permission_denied', `Checkout lease could not be prepared: ${(error as NodeJS.ErrnoException).code ?? 'error'}.`);
  }
}

export async function acquireCheckout(root: string, mode: LeaseClass = 'writer'): Promise<CheckoutLease> {
  // Lock both the per-user lifecycle file (keyed by verified dev/ino) and, for
  // writers/exclusive mutations, the checkout directory inode itself.
  const { root: canonical, identity: expected, lifecycle } = await openLifecycle(root);
  let writer: FileHandle | undefined;
  let owner: FileHandle | undefined;
  try {
    // Runs and undo save a restart marker; this lock tells readers it is live.
    if (mode !== 'reader') {
      const ownerName = `cleanup-owner-${lockHex(expected.dev)}-${lockHex(expected.ino)}.lock`;
      owner = await open(join(leaseDirectory(), ownerName), constants.O_RDWR | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
    }
    if (mode !== 'reader') {
      writer = await open(canonical, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
    }
    await new Promise<void>((resolve, reject) => {
      const stdio: ('ignore' | 'pipe' | number)[] = writer
        ? ['ignore', 'ignore', 'pipe', lifecycle.fd, writer.fd, owner?.fd ?? 'ignore']
        : ['ignore', 'ignore', 'pipe', lifecycle.fd];
      const child = spawn(process.env.PLAIT_HOST_EXE!, ['lock-fd', mode, canonical, ...(owner ? ['owner'] : [])], { stdio });
      let err = '';
      child.stderr!.setEncoding('utf8').on('data', chunk => { err = (err + chunk).slice(-4096); });
      child.once('error', reject);
      child.once('close', code => code === 0 ? resolve() : reject(lockFailure(err, 'This checkout already has a host writer.')));
    });
    if (writer) {
      const held = await writer.stat({ bigint: true });
      if (String(held.dev) !== expected.dev || String(held.ino) !== expected.ino) fail('permission_denied', 'Checkout changed while taking its lease.');
    }
    await checkRoot(canonical, expected);
    let closed = false;
    return {
      fd: writer?.fd,
      lifecycleFd: lifecycle.fd,
      class: mode,
      root: canonical,
      identity: expected,
      async close() {
        if (closed) return;
        closed = true;
        // Release in reverse acquisition order: writer first, then lifecycle.
        try { await writer?.close(); } finally { try { await owner?.close(); } finally { await lifecycle.close(); } }
      },
    };
  } catch (error) {
    try { await writer?.close(); } finally { try { await owner?.close(); } finally { await lifecycle.close(); } }
    throw error;
  }
}

export async function acquireCommonMetadata(root: string, timeoutMs = 5000): Promise<CommonMetadataLease> {
  const prepared = await prepareLease('common', root);
  const handle = await open(prepared.lock_path, constants.O_RDWR | constants.O_NOFOLLOW);
  try {
    await new Promise<void>((resolve, reject) => {
      const child = spawn(process.env.PLAIT_HOST_EXE!, ['lock-fd', 'common', prepared.root, String(Math.max(0, Math.trunc(timeoutMs)))], {
        stdio: ['ignore', 'ignore', 'pipe', handle.fd],
      });
      let err = '';
      child.stderr!.setEncoding('utf8').on('data', chunk => { err = (err + chunk).slice(-4096); });
      child.once('error', reject);
      child.once('close', code => code === 0 ? resolve() : reject(lockFailure(err, 'Another Plait operation is updating this repository\'s shared Git metadata.')));
    });
    let closed = false;
    return {
      fd: handle.fd,
      commonDir: prepared.root,
      identity: { dev: prepared.dev, ino: prepared.ino },
      async close() {
        if (closed) return;
        closed = true;
        await handle.close();
      },
    };
  } catch (error) { await handle.close(); throw error; }
}

export async function fileAction<T = FileResult>(root: string, action: string, path: string, input?: unknown, checkoutFd?: number, identity?: FileIdentity, lifecycleFd?: number) {
  safePath(path);
  const executable = process.env.PLAIT_HOST_EXE;
  if (!executable) fail('internal', 'Host file helper is not configured.');
  try {
    const child = spawn(executable, ['file', action, root, path, ...(identity ? [identity.dev, identity.ino] : [])], {
      stdio: ['pipe', 'pipe', 'pipe', process.env.PLAIT_LOCK_FD ? Number(process.env.PLAIT_LOCK_FD) : 'ignore', checkoutFd ?? 'ignore',
        ...(lifecycleFd !== undefined ? ['ignore', 'ignore', lifecycleFd] as const : [])],
    });
    child.stdin!.on('error', () => {});
    child.stdin!.end(input === undefined ? undefined : JSON.stringify(input));
    return await new Promise<T>((resolve, reject) => {
      let output = ''; let diagnostic = '';
      child.stdout!.setEncoding('utf8');
      child.stderr!.setEncoding('utf8');
      child.stderr!.on('data', data => { diagnostic = (diagnostic + data).slice(-4096); });
      const timer = setTimeout(() => child.kill('SIGKILL'), 10000);
      child.stdout!.on('data', data => { output += data; if (output.length > 1048576) child.kill('SIGKILL'); });
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('close', code => {
        clearTimeout(timer);
        if (code !== 0) {
          // Only fixed helper messages cross the boundary; never echo raw stderr.
          const reason = new Map([
            ['plait-host: invalid read offset', 'Read denied: offset must be a non-negative integer byte offset.'],
            ['plait-host: invalid read limit', 'Read denied: limit must be an integer from 4 to 12000 bytes.'],
            ['plait-host: read offset beyond EOF', 'Read denied: offset is beyond the end of the file.'],
            ['plait-host: read offset inside UTF-8 character', 'Read denied: offset is inside a UTF-8 character. Use the returned next offset.'],
            ['plait-host: file contents are not valid UTF-8', 'Read denied: file contents are not valid UTF-8.'],
            ['plait-host: edit must match exactly once', 'Edit denied: old_text must match exactly once.'],
            ['plait-host: file identity or metadata changed after approval', 'Edit denied: file identity or metadata changed after approval.'],
            ['plait-host: file changed after approval was requested', 'Edit denied: file contents changed after approval was requested.'],
          ]).get(diagnostic.trim());
          reject(new HostError('permission_denied', reason ?? 'File access denied, changed, non-unique edit, or unsafe path.'));
        }
        else { try { resolve(JSON.parse(output)); } catch { reject(new Error('Invalid helper output')); } }
      });
    });
  } catch (error) {
    if (error instanceof HostError) throw error;
    fail('permission_denied', 'File operation failed safely.');
  }
}

/** Metadata only: no working-tree filters, project hooks, or optional locks. */
export async function runHeader(root: string, vcs: ProjectVcs = 'git'): Promise<string> {
  if (vcs === 'folder') return `Plait run context (host supplied):\nWorking directory: ${JSON.stringify(root)}\nThis folder is not a Git repository. Nothing in it is protected or can be undone.\n`;
  const branch = (await git(root, ['symbolic-ref', '--quiet', '--short', 'HEAD']).catch(error => {
    if ((error as { code?: number }).code === 1) return '(detached)'; throw error;
  })).trim();
  const base = (await git(root, ['rev-parse', '--verify', 'HEAD']).catch(error => {
    if ((error as { code?: number }).code === 128) return '(unborn)'; throw error;
  })).trim();
  const shared = await sharedGitDir(root);
  return `Plait run context (host supplied):\nWorking directory: ${JSON.stringify(root)}\nBranch: ${JSON.stringify(branch)}\nBase at run start: ${base}\nCheckout: ${shared ? 'isolated linked worktree' : 'project checkout'}. Execution capability: writer; one Plait writer owns this checkout for this run. External tools and people may still change observations. Pre-existing dirty files belong to the person; do not present them as your work. Worktrees start from a committed base; original uncommitted files and build outputs are not copied. Use only the private build locations supplied by this harness; do not share mutable output with another checkout. A refused write is an error or approval request, never permission to bypass the harness. Plait manages worktrees; do not create, remove, or move them yourself. Return work by commits, push, and pull request. Project instructions remain in effect.\n`;
}
