// Host-owned placement decisions; a worktree is chosen before a run starts.
import { resourceFailure, type RunCapacity } from './run-capacity.ts';
import { cleanupHelper } from './cleanup.ts';
import { randomUUID } from 'node:crypto';
import { acquireCheckout, checkout, projectKind, checkRoot, rootIdentity, type FileIdentity } from './safety.ts';
import { fail, failure, HostError, HARNESSES, THINKING_LEVELS, type Snapshot, type Failure, type HarnessId, type ThinkingLevel, type ApprovalMode } from './protocol.ts';
import { worktreeHelper, type Worktrees, type Parent, type Managed, type Create } from './worktrees.ts';
import type { Store } from './store.ts';

export type Placement = 'project' | 'worktree' | 'wait';
export type Start = { op: 'create_admitted_thread'; request_id: string; checkout: string; placement: Placement; harness: HarnessId; provider: string; model_id: string; approval: ApprovalMode; mode?: string; thinking?: ThinkingLevel; source_thread_id?: string };
type Request = { op: 'run_preparation'; thread_id: string } | Start | { op: 'admission_info'; checkout: string } | { op: 'set_project_isolation'; checkout: string; identity: FileIdentity; auto_worktree: boolean } | { op: 'admission_status'; request_id: string } | { op: 'thread_origin'; thread_id: string };
export type Admission = { request_id: string; state: 'choice' | 'preparing' | 'ready' | 'failed'; thread_id: string | null; checkout: string; worktree: Managed | null; source_thread_id: string | null; error: Failure | null };
type Plan = { identity: FileIdentity; input: Start; worktree: Create | null; here_id: string };
const fields: Record<Request['op'], string[]> = {
  run_preparation: ['thread_id'], admission_info: ['checkout'], set_project_isolation: ['checkout', 'identity', 'auto_worktree'],
  create_admitted_thread: ['request_id', 'checkout', 'placement', 'harness', 'provider', 'model_id', 'approval'],
  admission_status: ['request_id'], thread_origin: ['thread_id'],
};
const optional = ['thinking', 'mode', 'source_thread_id'];
const id = (value: unknown) => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
const path = (value: unknown) => typeof value === 'string' && value.startsWith('/') && Buffer.byteLength(value) <= 4096 && !/[\u0000-\u001f\u007f]/u.test(value) && !value.includes('//') && !value.split('/').some(part => part === '.' || part === '..');
export function handlesAdmission(input: unknown): boolean { return !!input && typeof input === 'object' && Object.hasOwn(fields, String((input as { op?: unknown }).op)); }
export function validateAdmission(input: unknown): Request {
  if (!handlesAdmission(input) || Array.isArray(input)) fail('invalid_request', 'Unknown admission operation.');
  const value = input as Record<string, unknown>; const required = fields[value.op as Request['op']];
  if (Buffer.byteLength(JSON.stringify(value)) > 16384 || required.some(key => !Object.hasOwn(value, key)) || Object.keys(value).some(key => key !== 'op' && !required.includes(key) && !(value.op === 'create_admitted_thread' && optional.includes(key)))) fail('invalid_request', 'Unknown or missing admission field.');
  for (const key of ['request_id', 'thread_id', 'source_thread_id']) if (value[key] !== undefined && !id(value[key])) fail('invalid_request', 'Invalid admission request or thread ID.');
  if (value.checkout !== undefined && !path(value.checkout)) fail('invalid_request', 'Invalid admission checkout path.');
  if (value.op === 'set_project_isolation') {
    const identity = value.identity as FileIdentity | null;
    if (typeof value.auto_worktree !== 'boolean' || !identity || Object.keys(identity).sort().join(',') !== 'dev,ino' || !Object.values(identity).every(part => typeof part === 'string' && /^(0|[1-9]\d{0,19})$/.test(part) && BigInt(part) <= 18446744073709551615n)) fail('invalid_request', 'Invalid project isolation setting.');
  }
  if (value.op === 'create_admitted_thread') {
    if (!['project', 'worktree', 'wait'].includes(String(value.placement)) || !HARNESSES.includes(value.harness as HarnessId) || !['ask', 'auto'].includes(String(value.approval))) fail('invalid_request', 'Invalid thread placement or harness.');
    for (const key of ['provider', 'model_id']) if (typeof value[key] !== 'string' || !value[key].trim() || Buffer.byteLength(value[key]) > 256 || /[\u0000-\u001f\u007f]/u.test(value[key])) fail('invalid_request', 'Invalid admission model.');
    if (value.mode !== undefined && (typeof value.mode !== 'string' || !/^[a-z_]{1,32}$/.test(value.mode))) fail('invalid_request', 'Invalid admission mode.');
    if (value.thinking !== undefined && !THINKING_LEVELS.includes(value.thinking as ThinkingLevel)) fail('invalid_request', 'Invalid admission thinking level.');
    if (value.source_thread_id !== undefined && value.placement !== 'worktree') fail('invalid_request', 'A linked continuation requires an explicit worktree choice.');
  }
  return value as Request;
}
function canonical(value: unknown): string {
  if (value && typeof value === 'object' && !Array.isArray(value)) return JSON.stringify(Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, part]) => [key, part])));
  return JSON.stringify(value);
}
async function requireCleanup(root: string) {
  const state = await cleanupHelper('read', root);
  if (state.cleanup) fail('cleanup_uncertain', `Stop unconfirmed. ${state.cleanup.reason} New threads from this checkout are blocked until cleanup is confirmed.`);
}
const policyKey = (root: string, identity: FileIdentity) => `project_isolation:${JSON.stringify([root, identity.dev, identity.ino])}`;

export class Admissions {
  private pending = new Map<string, Promise<Admission>>();
  private admitting = new Map<string, Promise<Admission>>();
  private probes = new Map<string, Promise<boolean>>();
  private capacity: RunCapacity;
  private store: Store;
  private worktrees: Worktrees;
  private createHere: (input: Record<string, unknown>, identity: FileIdentity) => Promise<unknown>;
  private busy: (root: string) => boolean;
  private admit: () => void;
  constructor(store: Store, worktrees: Worktrees, capacity: RunCapacity,
    createHere: (input: Record<string, unknown>, identity: FileIdentity) => Promise<unknown>, busy: (root: string) => boolean, admit: () => void = () => {}) {
    this.capacity = capacity; this.store = store; this.worktrees = worktrees; this.createHere = createHere; this.busy = busy; this.admit = admit;
    store.db.exec('CREATE TABLE IF NOT EXISTS thread_admissions(request_id TEXT PRIMARY KEY,input TEXT NOT NULL,plan TEXT NOT NULL,result TEXT NOT NULL)');
    for (const row of store.db.prepare('SELECT request_id,result FROM thread_admissions').all()) {
      if ((JSON.parse(String(row.result)) as Admission).state === 'preparing') this.start(String(row.request_id));
    }
  }
  async handle(input: unknown) {
    const request = validateAdmission(input);
    if (request.op === 'run_preparation') { this.store.get(request.thread_id); return { schema: 1, thread_id: request.thread_id, phase: this.capacity.phases.get(request.thread_id) ?? null }; }
    if (request.op === 'admission_info') return this.info(request.checkout);
    if (request.op === 'set_project_isolation') {
      const info = await this.info(request.checkout);
      if (canonical(info.project_identity) !== canonical(request.identity)) fail('permission_denied', 'Project identity changed. Refresh its settings.');
      await checkRoot(info.project, info.project_identity);
      if (request.auto_worktree && (await projectKind(info.checkout)).vcs === 'folder') fail('worktree_unavailable', 'Worktrees need a Git repository.');
      this.store.setSetting(policyKey(info.project, info.project_identity), request.auto_worktree ? 'worktree' : 'ask');
      return { ...info, auto_worktree: request.auto_worktree };
    }
    if (request.op === 'thread_origin') {
      this.store.get(request.thread_id);
      const row = this.store.db.prepare("SELECT result FROM thread_admissions WHERE json_extract(result,'$.thread_id')=? ORDER BY rowid DESC LIMIT 1").get(request.thread_id);
      return { schema: 1, admission: row ? JSON.parse(String(row.result)) : null };
    }
    if (request.op === 'admission_status') return { schema: 1, admission: await this.status(request.request_id) };
    const earlier = this.admitting.get(request.request_id); if (earlier) await earlier;
    const admission = this.create(request); this.admitting.set(request.request_id, admission);
    try { return { schema: 1, admission: await admission }; }
    finally { if (this.admitting.get(request.request_id) === admission) this.admitting.delete(request.request_id); }
  }
  async info(path: string) {
    const root = await checkout(path); const identity = await rootIdentity(root);
    await requireCleanup(root);
    let parent: Parent | null = null; let refusal: string | null = null;
    if ((await projectKind(root)).vcs === 'folder') refusal = 'Worktrees need a Git repository.';
    else try { parent = await worktreeHelper<Parent>({ action: 'parent', root }); }
    catch (error) { refusal = error instanceof Error ? error.message : 'Worktree base is unavailable.'; }
    let busy = this.busy(root);
    if (!busy) {
      // Checked and registered in one step, so a local run start sees the probe.
      let probe = this.probes.get(root);
      if (!probe) {
        probe = this.writerElsewhere(root).finally(() => this.probes.delete(root));
        this.probes.set(root, probe);
      }
      busy = await probe;
    }
    await checkRoot(root, identity);
    if (parent && (parent.root !== root || canonical(parent.identity) !== canonical(identity))) fail('permission_denied', 'Checkout identity changed. Refresh before starting a thread.');
    const project = parent?.main ?? root; const project_identity = parent?.main_identity ?? identity;
    return { schema: 1, worktree_at_start_v1: true, linked_continuation_v1: true, queue: true, strict_reader: false,
      checkout: root, identity, project, project_identity, busy, auto_worktree: refusal === 'Worktrees need a Git repository.' ? false : this.store.setting(policyKey(project, project_identity)) === 'worktree', parent, refusal };
  }
  /** Settles when no placement probe holds this checkout's lease. */
  probing(root: string): Promise<void> { return (this.probes.get(root) ?? Promise.resolve(false)).then(() => {}, () => {}); }
  /** Another host's run holds the writer lock with a shared lifecycle lock. A Git
   * write or removal holds the lifecycle exclusively; it is not a run. */
  protected async writerElsewhere(root: string): Promise<boolean> {
    const busy = (error: unknown) => { if (error instanceof HostError && error.code === 'busy') return true; throw error; };
    await this.worktrees.builds.yieldToRun(root);
    try { await (await acquireCheckout(root)).close(); return false; } catch (error) { busy(error); }
    try { await (await acquireCheckout(root, 'reader')).close(); return true; } catch (error) { busy(error); return false; }
  }
  private get(id: string): Admission {
    const row = this.store.db.prepare('SELECT result FROM thread_admissions WHERE request_id=?').get(id);
    if (!row) fail('not_found', 'Thread admission not found.');
    return JSON.parse(String(row.result));
  }
  private put(result: Admission) { this.store.db.prepare('UPDATE thread_admissions SET result=? WHERE request_id=?').run(JSON.stringify(result), result.request_id); }
  private async create(request: Start): Promise<Admission> {
    const previous = this.store.db.prepare('SELECT input FROM thread_admissions WHERE request_id=?').get(request.request_id);
    if (previous) {
      if (previous.input !== canonical(request)) fail('invalid_request', 'This admission request ID has a different payload.');
      return this.status(request.request_id);
    }
    const info = await this.info(request.checkout);
    if (request.source_thread_id) {
      const source = this.store.get(request.source_thread_id);
      if (source.checkout !== info.checkout) fail('permission_denied', 'The linked thread belongs to another checkout.');
      await checkRoot(source.checkout, this.store.rootIdentity(source.id));
    }
    const isolated = request.placement === 'worktree' || request.placement === 'project' && info.busy && info.auto_worktree;
    const result: Admission = { request_id: request.request_id, state: 'preparing', thread_id: null, checkout: info.checkout, worktree: null, source_thread_id: request.source_thread_id ?? null, error: null };
    if (info.busy && !isolated && request.placement !== 'wait') return { ...result, state: 'choice' };
    if (isolated && !info.parent) fail('worktree_unavailable', info.refusal ?? 'Make a first commit before starting a worktree.');
    const { harness, provider, model_id, approval, mode, thinking } = request;
    const effectId = randomUUID();
    const worktree: Create | null = isolated ? { op: 'create_worktree_thread', request_id: effectId, parent: info.parent!, harness, provider, model_id, approval, ...(mode === undefined ? {} : { mode }), ...(thinking === undefined ? {} : { thinking }) } : null;
    const plan: Plan = { identity: info.identity, input: request, worktree, here_id: effectId };
    this.admit();
    this.store.db.prepare('INSERT INTO thread_admissions VALUES (?,?,?,?)').run(request.request_id, canonical(request), JSON.stringify(plan), JSON.stringify(result));
    this.start(request.request_id);
    return result;
  }
  private start(id: string) {
    if (this.pending.has(id)) return;
    const task = this.finish(id).finally(() => this.pending.delete(id));
    this.pending.set(id, task); void task.catch(() => {});
  }
  private async status(id: string) {
    const result = this.get(id);
    if (result.state === 'preparing') { this.start(id); return this.pending.get(id)!; }
    return result;
  }
  private async finish(id: string): Promise<Admission> {
    const row = this.store.db.prepare('SELECT plan FROM thread_admissions WHERE request_id=?').get(id)!;
    const plan = JSON.parse(String(row.plan)) as Plan; const result = this.get(id);
    try {
      if (plan.worktree) {
        const reply = await this.worktrees.handle({ op: 'worktree_status', request_id: plan.worktree.request_id }).catch(error => {
          if (error instanceof HostError && error.code === 'not_found') return requireCleanup(plan.input.checkout).then(() => this.worktrees.handle(plan.worktree, true));
          throw error;
        }) as { operation: { state: string; thread_id: string; worktree: Managed | null; error: Failure | null } };
        result.thread_id = reply.operation.thread_id; result.worktree = reply.operation.worktree;
        result.state = reply.operation.state === 'ready' ? 'ready' : reply.operation.state === 'failed' ? 'failed' : 'preparing';
        result.error = reply.operation.error;
        if (result.worktree) result.checkout = result.worktree.path;
      } else {
        await checkRoot(plan.input.checkout, plan.identity);
        await requireCleanup(plan.input.checkout);
        const { checkout, harness, provider, model_id, mode, thinking } = plan.input;
        const reply = await this.createHere({ op: 'create_thread', protocol_version: 6, request_id: plan.here_id, checkout, harness, provider, model_id, ...(mode === undefined ? {} : { mode }), ...(thinking === undefined ? {} : { thinking }) }, plan.identity) as { thread: Snapshot };
        result.thread_id = reply.thread.id; result.checkout = reply.thread.checkout; result.state = 'ready';
      }
    } catch (error) { result.state = 'failed'; result.error = resourceFailure(error) ?? failure(error); }
    this.put(result); return result;
  }
  async close() { await Promise.allSettled([...this.admitting.values()]); await Promise.allSettled([...this.pending.values()]); }
}
