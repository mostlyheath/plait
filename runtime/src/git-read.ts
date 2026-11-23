// Additive Git and file reads, and Git writes, for a remote desktop. The Rust helper runs the
// desktop's own hardened reads; this keeps its result and serves it in pages,
// because one reply is limited to 1 MiB.
import { spawn, execFile } from 'node:child_process';
import { realpath, stat } from 'node:fs/promises';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';
import { once } from './operations.ts';
import type { Store } from './store.ts';
import { fail } from './protocol.ts';

/** Raw bytes per page; base64 makes that 800 KiB, under the reply limit. */
export const PAGE = 600 * 1024;
/** Matches the client's reassembly limit; a larger result is refused, not cut. */
const RESULT_LIMIT = 64 * 1024 * 1024;
const TIMEOUT_MS = 60_000;
/** A write may wait for the checkout's lease, then run a 120-second network command. */
const WRITE_TIMEOUT_MS = 240_000;
const KEEP_MS = 120_000;
const DOCUMENTS = 4;

const reads: Record<string, string[]> = {
  status: [], checkout_summary: ['roots'], files: [], branches: [], main_checkout: [], identity: [],
  diff: ['change', 'mode'],
  log: ['tip', 'skip', 'limit'],
  show: ['id'],
  line_counts: ['paths'],
  graph: ['heads'],
  file: ['path'],
  file_stamp: ['path'],
  worktree_size: ['worktree', 'worktree_identity', 'thread', 'removed', 'others', 'build_directories'],
};
const operations = new Set(['git_read', 'git_read_page', 'git_write']);
const digits = (value: unknown) => typeof value === 'string' && /^[0-9]{1,20}$/.test(value);
const base64 = (value: unknown) => typeof value === 'string' && value.length > 0 && value.length <= 8192 && value.length % 4 === 0 && /^[A-Za-z0-9+/]+={0,2}$/.test(value);

export type CheckoutSummary = { vcs: string; fingerprint?: { roots: string[]; remote: string | null } | null; branch?: { branch: string; named: boolean; tracking: unknown } | null; main_checkout?: string | null };

type Document = { bytes: Buffer; touched: number };
export type Helper = (input: unknown, operation: 'git-read' | 'git-write') => Promise<Buffer>;

export function validateGitRead(input: unknown): Record<string, unknown> & { op: string } {
  if (!input || typeof input !== 'object' || Array.isArray(input)) fail('invalid_request', 'Invalid Git read.');
  const request = input as Record<string, unknown> & { op: string };
  if (Buffer.byteLength(JSON.stringify(request)) > 65_000) fail('invalid_request', 'Unknown operation or field.');
  if (request.op === 'git_read_page') {
    if (Object.keys(request).some(key => !['op', 'token', 'offset'].includes(key))) fail('invalid_request', 'Unknown operation or field.');
    if (typeof request.token !== 'string' || !/^[0-9a-f-]{36}$/.test(request.token)) fail('invalid_request', 'Invalid token.');
    if (!Number.isSafeInteger(request.offset) || Number(request.offset) < 0) fail('invalid_request', 'Invalid page offset.');
    return request;
  }
  if (request.op === 'git_write') {
    if (Object.keys(request).some(key => !['op', 'checkout', 'identity', 'write', 'op_id'].includes(key))) fail('invalid_request', 'Unknown operation or field.');
    if (!base64(request.checkout)) fail('invalid_request', 'Invalid checkout.');
    if (!identityValid(request.identity)) fail('invalid_request', 'Invalid checkout identity.');
    if (request.write === undefined || request.write === null) fail('invalid_request', 'Missing write.');
    return request;
  }
  const kind = typeof request.read === 'string' && Object.hasOwn(reads, request.read) ? reads[request.read] : undefined;
  const allowed = ['op', 'read', 'checkout', 'identity', ...(kind ?? [])];
  if (!kind || Object.keys(request).some(key => !allowed.includes(key))) fail('invalid_request', 'Unknown operation or field.');
  if (!base64(request.checkout)) fail('invalid_request', 'Invalid checkout.');
  if (request.read === 'checkout_summary' && typeof request.roots !== 'boolean') fail('invalid_request', 'Invalid roots.');
  if (request.identity != null && !identityValid(request.identity)) fail('invalid_request', 'Invalid checkout identity.');
  return request;
}

function identityValid(value: unknown) {
  const identity = value as { dev?: unknown; ino?: unknown } | null;
  return !!identity && typeof identity === 'object' && !Array.isArray(identity)
    && Object.keys(identity).every(key => key === 'dev' || key === 'ino') && digits(identity.dev) && digits(identity.ino);
}

/** Tokens belong to this owner-authenticated runtime, across socket exchanges. */
export class GitReads {
  private documents = new Map<string, Document>();
  private helper: Helper;
  private now: () => number;
  /** Base64 of the host store; only the runtime names it, for a worktree's retained build output. */
  private store: string | null;
  private operationStore?: Store;
  constructor(helper: Helper = runHelper, now = Date.now, store: string | null = null, operationStore?: Store) {
    this.operationStore = operationStore; this.helper = helper; this.now = now; this.store = store === null ? null : Buffer.from(store).toString('base64');
  }
  async checkoutSummary(path: string, roots: boolean): Promise<CheckoutSummary> {
    const bytes = await this.helper({ read: 'checkout_summary', roots, checkout: Buffer.from(path).toString('base64') }, 'git-read');
    const reply = JSON.parse(bytes.toString()) as { ok?: { result?: CheckoutSummary } };
    if (!reply.ok?.result || !['git', 'folder'].includes(reply.ok.result.vcs)) throw new Error('Invalid checkout summary.');
    return reply.ok.result;
  }
  handles(input: unknown) { return !!input && typeof input === 'object' && operations.has(String((input as { op?: unknown }).op)); }
  async handle(input: unknown) {
    const request = validateGitRead(input);
    this.expire();
    if (request.op === 'git_read_page') return this.page(String(request.token), Number(request.offset));
    const { op, op_id, ...forward } = request;
    if (op === 'git_write' && this.operationStore) return once(this.operationStore, op_id, op, forward, () => this.execute(op, forward));
    if (op_id !== undefined) fail('invalid_request', 'Operation IDs require a host store.');
    return this.execute(op, forward);
  }
  private async execute(op: string, forward: Record<string, unknown>) {
    if (forward.read === 'worktree_size') forward.store = this.store;
    const write = op === 'git_write';
    let bytes: Buffer;
    try { bytes = forward.read === 'identity' || forward.read === 'main_checkout' ? await localRead(forward) : await this.helper(forward, write ? 'git-write' : 'git-read'); } catch {
      fail(write ? 'git_write_failed' : 'git_read_failed', write ? 'The host could not finish this change. Refresh to see the checkout as it is.' : 'The host could not read this checkout.');
    }
    if (bytes.length > RESULT_LIMIT) fail('git_read_limit', 'This read is too large to show.');
    if (bytes.length <= PAGE) return { total: bytes.length, data: bytes.toString('base64'), token: null };
    const token = randomUUID();
    this.documents.set(token, { bytes, touched: this.now() });
    // A few large reads at a time; the oldest gives way.
    while (this.documents.size > DOCUMENTS) this.documents.delete(this.documents.keys().next().value!);
    return this.page(token, 0);
  }
  private page(token: string, offset: number) {
    const document = this.documents.get(token);
    if (!document) fail('not_found', 'This read expired. Refresh to read it again.');
    if (offset >= document.bytes.length) fail('invalid_request', 'Invalid page offset.');
    document.touched = this.now();
    const end = Math.min(offset + PAGE, document.bytes.length);
    if (end === document.bytes.length) this.documents.delete(token);
    return { total: document.bytes.length, data: document.bytes.subarray(offset, end).toString('base64'), token: end < document.bytes.length ? token : null };
  }
  private expire() {
    for (const [token, document] of this.documents) if (this.now() - document.touched > KEEP_MS) this.documents.delete(token);
  }
}

function runHelper(input: unknown, operation: 'git-read' | 'git-write'): Promise<Buffer> {
  const executable = process.env.PLAIT_HOST_EXE;
  if (!executable) return Promise.reject(new Error('Host helper is not configured.'));
  return new Promise<Buffer>((resolve, reject) => {
    // The store lock travels with the helper, as with every helper. Its own
    // process group lets a timeout stop Git as well.
    const child = spawn(executable, [operation], {
      stdio: ['pipe', 'pipe', 'ignore', process.env.PLAIT_LOCK_FD ? Number(process.env.PLAIT_LOCK_FD) : 'ignore'],
      detached: true,
    });
    const chunks: Buffer[] = []; let size = 0;
    const stop = () => { try { process.kill(-child.pid!, 'SIGKILL'); } catch { child.kill('SIGKILL'); } };
    const timer = setTimeout(stop, operation === 'git-write' ? WRITE_TIMEOUT_MS : TIMEOUT_MS);
    child.stdin!.on('error', () => {});
    child.stdin!.end(JSON.stringify(input));
    child.stdout!.on('data', (chunk: Buffer) => { size += chunk.length; if (size > RESULT_LIMIT + 1) stop(); else chunks.push(chunk); });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', code => {
      clearTimeout(timer);
      stop(); // Reclaim a stray Git descendant too.
      if (code !== 0) return reject(new Error(`plait-host ${operation} failed`));
      resolve(Buffer.concat(chunks));
    });
  });
}

// These reads are implemented here until the host helper supports their wire types.
async function localRead(request: Record<string, unknown>): Promise<Buffer> {
  try {
    const path = Buffer.from(String(request.checkout), 'base64');
    if (path[0] !== 47 || path.includes(0)) throw new Error('Invalid path');
    const root = await realpath(path, { encoding: 'buffer' });
    const info = await stat(root, { bigint: true });
    if (!info.isDirectory()) throw new Error('Not a directory');
    const identity = { dev: String(info.dev), ino: String(info.ino) };
    const expected = request.identity as typeof identity | null | undefined;
    // A remounted volume can change dev after a restart; the same canonical path and inode is still this folder.
    if (expected && expected.ino !== identity.ino) throw new Error('Checkout identity changed.');
    if (expected && expected.dev !== identity.dev && !root.equals(path)) throw new Error('Checkout identity changed.');
    let result: unknown = { root: root.toString('base64'), identity };
    if (request.read === 'main_checkout') {
      const cwd = new TextDecoder('utf-8', { fatal: true }).decode(root);
      const { stdout } = await promisify(execFile)('/usr/bin/git', ['-c', 'core.fsmonitor=false', '-c', 'core.untrackedCache=false', '-c', 'diff.external=', '-c', 'core.hooksPath=/dev/null', 'worktree', 'list', '--porcelain', '-z'], {
        env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: '/nonexistent', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_OPTIONAL_LOCKS: '0', GIT_TERMINAL_PROMPT: '0', LC_ALL: 'C' }, cwd, encoding: 'buffer', timeout: 10_000, maxBuffer: 1048576,
      });
      const end = stdout.indexOf(0);
      if (end < 10 || !stdout.subarray(0, 9).equals(Buffer.from('worktree '))) throw new Error('Invalid worktree list');
      result = stdout.subarray(9, end).toString('base64');
    }
    const after = await stat(root, { bigint: true });
    if (after.dev !== info.dev || after.ino !== info.ino || !(await realpath(path, { encoding: 'buffer' })).equals(root)) throw new Error('Checkout identity changed.');
    return Buffer.from(JSON.stringify({ ok: { identity, result } }));
  } catch {
    return Buffer.from(JSON.stringify({ error: { message: 'The host could not read this checkout.' } }));
  }
}
