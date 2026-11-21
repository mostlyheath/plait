// Durable, explicit worktree operations. Existing protocol reply shapes stay unchanged.
import { BuildOutputs } from './build-output.ts';
import { commandPolicy, setCommandPolicy } from './command/policy.ts';
import { randomUUID } from 'node:crypto';
import { spawn } from 'node:child_process';
import { fail, failure, HostError, HARNESSES, THINKING_LEVELS } from './protocol.ts';
import type { Snapshot, HarnessId, ThinkingLevel, ApprovalMode, Failure } from './protocol.ts';
import type { Store } from './store.ts';
import type { FileIdentity } from './safety.ts';

export type Parent = { root: string; identity: FileIdentity; main: string; main_identity: FileIdentity; common: string; common_identity: FileIdentity; base: string };
export type Create = { op: 'create_worktree_thread'; request_id: string; parent: Parent; harness: HarnessId; provider: string; model_id: string; thinking?: ThinkingLevel; mode?: string; approval: ApprovalMode };
export type Managed = { thread_id: string; parent: Parent; path: string; branch: string; identity: FileIdentity | null; stage: 'reserved' | 'creating' | 'ready' | 'failed' | 'removing' | 'removed'; error: string | null };
export type Operation = { request_id: string; thread_id: string; state: 'preparing' | 'ready' | 'failed' | 'removing' | 'removed'; worktree: Managed | null; error: Failure | null };
type Request = Create | { op: 'project_worktrees'; checkout: string; only?: string } | { op: 'worktree_info'; checkout: string } | { op: 'worktree_status'; request_id: string } | { op: 'list_worktrees' } | { op: 'remove_worktree'; request_id: string; creation_id: string; identity: FileIdentity; allow_unpushed: boolean };
const fields = {
  project_worktrees: ['checkout'], worktree_info: ['checkout'], create_worktree_thread: ['request_id', 'parent', 'harness', 'provider', 'model_id', 'approval'],
  worktree_status: ['request_id'], list_worktrees: [], remove_worktree: ['request_id', 'creation_id', 'identity', 'allow_unpushed'],
};
export function handlesWorktrees(input: unknown): boolean {
  return !!input && typeof input === 'object' && Object.hasOwn(fields, String((input as { op?: unknown }).op));
}
function object(value: unknown, keys: string[]) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).length !== keys.length || keys.some(key => !Object.hasOwn(value, key))) fail('invalid_request', 'Invalid worktree identity.');
}
function identity(value: unknown) {
  object(value, ['dev', 'ino']);
  for (const part of Object.values(value as FileIdentity)) if (typeof part !== 'string' || !/^(0|[1-9]\d{0,19})$/.test(part) || BigInt(part) > 18446744073709551615n) fail('invalid_request', 'Invalid filesystem identity.');
}
function path(value: unknown) {
  if (typeof value !== 'string' || !value.startsWith('/') || Buffer.byteLength(value) > 4096 || /[\u0000-\u001f\u007f]/u.test(value) || value.split('/').some(part => part === '.' || part === '..') || value.includes('//')) fail('invalid_request', 'Invalid absolute worktree path.');
}
export function validateWorktree(input: unknown): Request {
  if (!handlesWorktrees(input)) fail('invalid_request', 'Unknown worktree operation.');
  const value = input as Record<string, unknown>;
  const required: string[] = fields[value.op as keyof typeof fields];
  const optional = value.op === 'create_worktree_thread' ? ['thinking', 'mode'] : value.op === 'project_worktrees' ? ['only'] : [];
  if (required.some(key => !Object.hasOwn(value, key)) || Object.keys(value).some(key => key !== 'op' && !required.includes(key) && !optional.includes(key)) || Buffer.byteLength(JSON.stringify(value)) > 65000) fail('invalid_request', 'Unknown or missing worktree field.');
  for (const field of ['request_id', 'creation_id']) if (value[field] !== undefined && (typeof value[field] !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(value[field]))) fail('invalid_request', 'Invalid worktree request ID.');
  if (value.op === 'worktree_info' || value.op === 'project_worktrees') path(value.checkout);
  if (value.op === 'project_worktrees' && value.only !== undefined) path(value.only);
  if (value.op === 'remove_worktree') {
    identity(value.identity);
    if (typeof value.allow_unpushed !== 'boolean') fail('invalid_request', 'allow_unpushed must be boolean.');
  }
  if (value.op === 'create_worktree_thread') {
    object(value.parent, ['root', 'identity', 'main', 'main_identity', 'common', 'common_identity', 'base']);
    const parent = value.parent as Parent;
    for (const item of [parent.root, parent.main, parent.common]) path(item);
    for (const item of [parent.identity, parent.main_identity, parent.common_identity]) identity(item);
    if (typeof parent.base !== 'string' || !/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(parent.base)) fail('invalid_request', 'Invalid base commit.');
    if (!HARNESSES.includes(value.harness as HarnessId)) fail('invalid_request', 'Invalid worktree harness.');
    for (const key of ['provider', 'model_id']) if (typeof value[key] !== 'string' || !value[key].trim() || Buffer.byteLength(value[key]) > 256 || /[\u0000-\u001f\u007f]/u.test(value[key])) fail('invalid_request', 'Invalid worktree model.');
    if (value.thinking !== undefined && !THINKING_LEVELS.includes(value.thinking as ThinkingLevel)) fail('invalid_request', 'Invalid thinking level.');
    if (value.mode !== undefined && (typeof value.mode !== 'string' || !/^[a-z_]{1,32}$/.test(value.mode))) fail('invalid_request', 'Invalid harness mode.');
    if (value.approval !== 'ask' && value.approval !== 'auto') fail('invalid_request', 'Invalid approval mode.');
  }
  return value as Request;
}
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, part]) => `${JSON.stringify(key)}:${canonical(part)}`).join(',')}}`;
  return JSON.stringify(value);
}
export function worktreeHelper<T>(input: unknown): Promise<T> {
  const executable = process.env.PLAIT_HOST_EXE;
  if (!executable) fail('internal', 'Host worktree helper is not configured.');
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ['worktree'], { detached: true, stdio: ['pipe', 'pipe', 'pipe', process.env.PLAIT_LOCK_FD ? Number(process.env.PLAIT_LOCK_FD) : 'ignore'] });
    let output = '';
    const stop = () => { try { process.kill(-child.pid!, 'SIGKILL'); } catch { child.kill('SIGKILL'); } };
    const timer = setTimeout(stop, 160_000);
    child.stdin!.on('error', () => {}); child.stdin!.end(JSON.stringify(input));
    child.stdout!.setEncoding('utf8').on('data', data => { output += data; if (Buffer.byteLength(output) > 750_000) stop(); });
    child.stderr!.resume(); // Git diagnostics are returned in the bounded JSON reply.
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', code => {
      clearTimeout(timer); stop();
      if (code !== 0) return reject(new HostError('worktree_failed', 'Worktree helper stopped. The recorded operation remains recoverable; existing work was preserved.'));
      try {
        const reply = JSON.parse(output) as { value: T; error: string | null };
        if (reply.error) reject(new HostError('worktree_refused', reply.error.slice(0, 4096)));
        else resolve(reply.value);
      } catch { reject(new HostError('worktree_failed', 'Invalid worktree helper reply. Query the operation before retrying.')); }
    });
  });
}

export class Worktrees {
  readonly builds: BuildOutputs;
  private pending = new Map<string, Promise<void>>();
  private admitting = new Map<string, Promise<unknown>>();
  private closing = false;
  private store: Store;
  private directory: string;
  private prepare: (request: Create, id: string) => Promise<Snapshot>;
  private changed: () => void;
  private busy: (root: string) => boolean;
  private admit: () => void;
  constructor(store: Store, directory: string,
    prepare: (request: Create, id: string) => Promise<Snapshot>,
    changed: () => void, busy: (root: string) => boolean, admit: () => void = () => {}) {
    this.store = store; this.directory = directory; this.prepare = prepare;
    this.changed = changed; this.busy = busy; this.admit = admit;
    this.builds = new BuildOutputs(store, directory, changed, busy);
    store.db.exec(`CREATE TABLE IF NOT EXISTS worktree_operations(request_id TEXT PRIMARY KEY,input TEXT NOT NULL,thread_id TEXT NOT NULL,snapshot TEXT NOT NULL,result TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS worktree_removals(request_id TEXT PRIMARY KEY,input TEXT NOT NULL,creation_id TEXT NOT NULL,result TEXT);`);
    // An admitted request resumes with its saved UUID, base, model and journal.
    for (const row of store.db.prepare('SELECT request_id FROM worktree_operations').all()) {
      const op = this.get(String(row.request_id));
      if (op.state === 'preparing') this.start(op.request_id);
    }
    for (const row of store.db.prepare('SELECT input FROM worktree_removals WHERE result IS NULL').all()) this.startRemoval(JSON.parse(String(row.input)));
  }
  handles(input: unknown) { return handlesWorktrees(input) || this.buildRequest(input); }
  private buildRequest(input: unknown): boolean {
    return !!input && typeof input === 'object' && ['build_output_info', 'set_build_output', 'watch_build_output', 'skip_build_output', 'project_worktrees_build'].includes(String((input as {op?: unknown}).op));
  }
  private async handleBuild(input: unknown) {
    const value = input as Record<string, unknown>;
    if (Array.isArray(value)) fail('invalid_request', 'Invalid build output request.');
    const fields: Record<string, string[]> = { build_output_info: ['checkout'], set_build_output: ['checkout','identity','revision','paths'], watch_build_output: ['request_id','kind'], skip_build_output: ['request_id','kind'], project_worktrees_build: ['checkout'] };
    const required = fields[String(value.op)];
    if (Buffer.byteLength(JSON.stringify(value)) > 40000 || required.some(key => !Object.hasOwn(value,key)) || Object.keys(value).some(key => key !== 'op' && !required.includes(key) && !(value.op === 'project_worktrees_build' && key === 'only') && !(value.op === 'build_output_info' && key === 'refresh'))) fail('invalid_request','Unknown or missing build output field.');
    if (value.checkout !== undefined) path(value.checkout);
    if (value.op === 'build_output_info') {
      if (value.refresh !== undefined && value.refresh !== true) fail('invalid_request', 'Invalid build output refresh.');
      return this.builds.info(value.checkout as string, value.refresh === true);
    }
    if (value.op === 'set_build_output') {
      identity(value.identity);
      if (!Number.isSafeInteger(value.revision) || Number(value.revision) < 0 || !Array.isArray(value.paths) || value.paths.length > 32 || value.paths.some(p => typeof p !== 'string' || Buffer.byteLength(p) > 1024)) fail('invalid_request','Invalid build output approval.');
      return this.builds.set(value.checkout as string, value.identity as FileIdentity, Number(value.revision), value.paths as string[]);
    }
    if (value.op === 'project_worktrees_build') {
      if (value.only !== undefined) path(value.only);
      return this.inventory(value.checkout as string, value.only as string | undefined, true);
    }
    if (typeof value.request_id !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(value.request_id) || !['worktree','admission'].includes(String(value.kind))) fail('invalid_request','Invalid build output operation identity.');
    let id = value.request_id;
    if (value.kind === 'admission') {
      const row = this.store.db.prepare('SELECT plan FROM thread_admissions WHERE request_id=?').get(id);
      if (!row && value.op === 'watch_build_output') return { schema: 1, worktree_build_output_v1: true, request_id: value.request_id, thread_id: null, phase: 'waiting', files: 0, outcome: null };
      if (!row) fail('not_found','Thread admission not found.');
      const plan = JSON.parse(String(row.plan)) as { worktree: Create | null };
      if (!plan.worktree) return { schema: 1, worktree_build_output_v1: true, request_id: value.request_id, thread_id: null, phase: 'cold', files: 0, outcome: null };
      id = plan.worktree.request_id;
    }
    if (!this.store.db.prepare('SELECT 1 FROM worktree_operations WHERE request_id=?').get(id) && value.op === 'watch_build_output') return { schema: 1, worktree_build_output_v1: true, request_id: value.request_id, thread_id: null, phase: 'waiting', files: 0, outcome: null };
    const operation = this.get(id);
    const state = value.op === 'skip_build_output' ? this.builds.skip(operation.thread_id) : this.builds.state(operation.thread_id);
    return { schema: 1, worktree_build_output_v1: true, request_id: value.request_id, thread_id: operation.thread_id, ...state, ...(operation.state === 'failed' ? { phase: 'cold', outcome: null } : {}) };
  }
  async handle(input: unknown, accepted = false) {
    if (this.buildRequest(input)) return this.handleBuild(input);
    const request = validateWorktree(input);
    if (request.op === 'worktree_info') return { schema: 1, worktree_threads_v1: true, parent: await worktreeHelper<Parent>({ action: 'parent', root: request.checkout }) };
    if (request.op === 'list_worktrees') return this.list();
    if (request.op === 'project_worktrees') return this.inventory(request.checkout, request.only);
    if (request.op === 'worktree_status') {
      const removed = this.store.db.prepare('SELECT result,creation_id FROM worktree_removals WHERE request_id=?').get(request.request_id);
      return { schema: 1, operation: removed ? (removed.result ? JSON.parse(String(removed.result)) : this.get(String(removed.creation_id))) : this.get(request.request_id) };
    }
    if (this.closing) fail('busy', 'The host is closing. Retry the same request after reconnecting.');
    if (request.op === 'remove_worktree') return this.remove(request);
    const earlier = this.admitting.get(request.request_id);
    if (earlier) await earlier;
    const admission = this.create(request, accepted);
    this.admitting.set(request.request_id, admission);
    try { return await admission; } finally { if (this.admitting.get(request.request_id) === admission) this.admitting.delete(request.request_id); }
  }
  // With `only`, exactly that checkout's row: a run ending re-reads its own checkout.
  private async inventory(checkout: string, only?: string, build = false) {
    const operations: Operation[] = []; let bytes = 0;
    for (const row of this.store.db.prepare('SELECT result FROM worktree_operations').iterate()) {
      const raw = String(row.result); bytes += Buffer.byteLength(raw);
      if (operations.length === 4096 || bytes > 900_000) fail('too_large', 'Worktree records exceed the inventory read limit. Use Git to inspect this repository.');
      operations.push(JSON.parse(raw) as Operation);
    }
    const records = operations.flatMap(op => op.worktree ? [op.worktree] : []);
    type Rows = Array<{ path: string; thread_id: string | null; preservation: { refusal: string | null; can_keep_local: boolean } }>;
    const reply = await worktreeHelper<Rows | { checkouts: Rows; seed_bytes: number | null }>({ action: 'inventory', root: checkout, store: this.directory, records, ...(only === undefined ? {} : { only }) });
    const checkouts = Array.isArray(reply) ? reply : reply.checkouts;
    const seedBytes = Array.isArray(reply) ? null : reply.seed_bytes;
    if (only !== undefined && (checkouts.length !== 1 || checkouts[0].path !== only)) fail('invalid_request', 'That folder is not one of this project’s checkouts.');
    const rows = checkouts.map(row => {
      const operation = operations.find(op => op.thread_id === row.thread_id);
      if (this.busy(row.path) || this.store.db.prepare("SELECT 1 FROM requests JOIN threads ON threads.id=requests.scope WHERE threads.checkout=? AND requests.state IN ('queued','checkout_queued') LIMIT 1").get(row.path)) {
        row.preservation.refusal = 'This worktree still has active, queued, or recovering work.';
        row.preservation.can_keep_local = false;
      }
      if (operation && ['preparing', 'removing'].includes(operation.state)) {
        row.preservation.refusal = 'A worktree operation is still in progress.';
        row.preservation.can_keep_local = false;
      }
      const thread = row.thread_id ? this.store.db.prepare('SELECT updated_at FROM threads WHERE id=?').get(row.thread_id) : this.store.db.prepare('SELECT MAX(updated_at) AS updated_at FROM threads WHERE checkout=?').get(row.path);
      const output = row as typeof row & { build_directories?: string[]; build_output?: unknown };
      if (build) output.build_output = row.thread_id ? this.builds.state(row.thread_id).outcome : null;
      else { delete output.build_directories; delete (row.preservation as { build_output?: unknown }).build_output; }
      return { ...output, creation_id: operation?.request_id ?? null, last_used: thread?.updated_at != null ? Number(thread.updated_at) : null };
    });
    const result = { schema: 1, worktree_inventory_v1: true, ...(build ? { worktree_build_output_v1: true, seed_bytes: seedBytes } : {}), checkouts: rows };
    if (Buffer.byteLength(JSON.stringify(result)) > 750_000) fail('too_large', 'Worktree inventory is too large. Use Git to inspect it.');
    return result;
  }
  private list() {
    const operations: Operation[] = []; let bytes = 1024;
    for (const row of this.store.db.prepare('SELECT result FROM worktree_operations ORDER BY rowid DESC LIMIT 256').all()) {
      const result = String(row.result); bytes += Buffer.byteLength(result);
      if (bytes > 750_000) break;
      operations.push(JSON.parse(result));
    }
    return { schema: 1, operations };
  }
  private get(id: string): Operation {
    const row = this.store.db.prepare('SELECT result FROM worktree_operations WHERE request_id=?').get(id);
    if (!row) fail('not_found', 'Worktree operation not found.');
    return JSON.parse(String(row.result));
  }
  private put(operation: Operation) {
    this.store.db.prepare('UPDATE worktree_operations SET result=? WHERE request_id=?').run(JSON.stringify(operation), operation.request_id);
  }
  private async create(request: Create, accepted: boolean) {
    if (this.store.db.prepare('SELECT 1 FROM worktree_removals WHERE request_id=?').get(request.request_id)) fail('invalid_request', 'This request ID belongs to a removal.');
    const previous = this.store.db.prepare('SELECT input FROM worktree_operations WHERE request_id=?').get(request.request_id);
    if (previous) {
      if (previous.input !== canonical(request)) fail('invalid_request', 'This worktree request ID has a different payload.');
      if (this.get(request.request_id).state === 'failed') {
        const retry = this.get(request.request_id); retry.state = 'preparing'; retry.error = null; this.put(retry); this.start(request.request_id);
      }
      return { schema: 1, operation: this.get(request.request_id) };
    }
    const id = randomUUID();
    const snapshot = await this.prepare(request, id);
    if (!accepted) this.admit();
    this.store.setSetting(`worktree_policy:${id}`, commandPolicy(this.store, request.parent.root, request.parent.identity));
    const operation: Operation = { request_id: request.request_id, thread_id: id, state: 'preparing', worktree: null, error: null };
    this.store.db.prepare('INSERT INTO worktree_operations VALUES (?,?,?,?,?)').run(request.request_id, canonical(request), id, JSON.stringify(snapshot), JSON.stringify(operation));
    this.start(request.request_id);
    return { schema: 1, operation };
  }
  private start(id: string) {
    if (this.pending.has(id)) return;
    const task = this.finish(id).catch(() => {}).finally(() => this.pending.delete(id));
    this.pending.set(id, task);
  }
  private async finish(id: string) {
    const operation = this.get(id);
    const row = this.store.db.prepare('SELECT input,snapshot FROM worktree_operations WHERE request_id=?').get(id)!;
    const request = JSON.parse(String(row.input)) as Create;
    try {
      const tree = await worktreeHelper<Managed>({ action: 'create', store: this.directory, thread_id: operation.thread_id, parent: request.parent });
      operation.worktree = tree;
      if (tree.stage !== 'ready' || !tree.identity) fail('worktree_failed', tree.error ?? 'Worktree creation did not finish. Existing work was preserved.');
      const snapshot = JSON.parse(String(row.snapshot)) as Snapshot;
      await this.builds.warm(tree, request.harness === 'pi' && this.store.setting(`worktree_policy:${snapshot.id}`) === 'sandboxed');
      snapshot.checkout = tree.path;
      // Thread admission is idempotent even if the host died before updating result.
      const duplicate = this.store.duplicate('worktree_create', id, request);
      if (!duplicate) {
        setCommandPolicy(this.store, tree.path, tree.identity, this.store.setting(`worktree_policy:${snapshot.id}`) === 'sandboxed' ? 'sandboxed' : 'trusted');
        this.store.admit('worktree_create', id, request, { thread_id: snapshot.id }, snapshot, tree.identity);
      }
      operation.state = 'ready'; operation.error = null; this.put(operation); this.changed();
      // After admission, never before it: an old seed is refreshed in the background.
      this.builds.scheduleSeed(tree.parent.main);
    } catch (error) {
      operation.state = 'failed'; operation.error = failure(error);
      operation.worktree ??= await worktreeHelper<Managed | null>({ action: 'read', store: this.directory, thread_id: operation.thread_id }).catch(() => null);
      this.put(operation);
    }
  }
  private remove(request: Extract<Request, { op: 'remove_worktree' }>) {
    if (this.store.db.prepare('SELECT 1 FROM worktree_operations WHERE request_id=?').get(request.request_id)) fail('invalid_request', 'This request ID belongs to a creation.');
    const previous = this.store.db.prepare('SELECT input,result FROM worktree_removals WHERE request_id=?').get(request.request_id);
    if (previous) {
      if (previous.input !== canonical(request)) fail('invalid_request', 'This removal request ID has a different payload.');
      return { schema: 1, operation: previous.result ? JSON.parse(String(previous.result)) : this.get(request.creation_id) };
    }
    const operation = this.get(request.creation_id);
    if (!operation.worktree?.identity || canonical(operation.worktree.identity) !== canonical(request.identity)) fail('permission_denied', 'No matching host-owned worktree identity.');
    if (operation.state === 'preparing' || operation.state === 'removing') fail('busy', 'A worktree operation is still in progress.');
    this.store.db.prepare('INSERT INTO worktree_removals VALUES (?,?,?,NULL)').run(request.request_id, canonical(request), request.creation_id);
    this.startRemoval(request);
    // An immediate refusal is already in the removal receipt.
    const done = this.store.db.prepare('SELECT result FROM worktree_removals WHERE request_id=?').get(request.request_id);
    return { schema: 1, operation: done?.result ? JSON.parse(String(done.result)) : this.get(request.creation_id) };
  }
  private startRemoval(request: Extract<Request, { op: 'remove_worktree' }>) {
    const key = `remove:${request.request_id}`;
    const task = this.finishRemoval(request).catch(() => {}).finally(() => this.pending.delete(key));
    this.pending.set(key, task);
  }
  private async finishRemoval(request: Extract<Request, { op: 'remove_worktree' }>) {
    const operation = this.get(request.creation_id);
    let receipt = operation;
    try {
      const root = operation.worktree!.path;
      // UI summaries are advisory. In-process work and OS leases both exclude removal.
      if (this.busy(root) || this.store.db.prepare("SELECT 1 FROM requests JOIN threads ON threads.id=requests.scope WHERE threads.checkout=? AND requests.state IN ('queued','checkout_queued') LIMIT 1").get(root)) fail('busy', 'This worktree still has active, queued, or recovering work.');
      operation.state = 'removing'; this.put(operation);
      operation.worktree = await worktreeHelper<Managed>({ action: 'remove', store: this.directory, thread_id: operation.thread_id, identity: request.identity, allow_unpushed: request.allow_unpushed });
      operation.state = 'removed'; operation.error = null; this.put(operation);
    } catch (error) {
      // The refusal answers the removal. The creation keeps its own state and
      // error: ready only when its worktree is ready and its thread was admitted.
      const admitted = this.store.db.prepare('SELECT 1 FROM threads WHERE id=?').get(operation.thread_id);
      operation.state = operation.worktree?.stage === 'ready' && admitted ? 'ready' : 'failed';
      this.put(operation);
      receipt = { ...operation, error: failure(error) };
    }
    this.store.db.prepare('UPDATE worktree_removals SET result=? WHERE request_id=?').run(JSON.stringify(receipt), request.request_id);
  }
  async close() { this.closing = true; await this.builds.close(); await Promise.allSettled([...this.admitting.values()]); await Promise.allSettled([...this.pending.values()]); }
}
