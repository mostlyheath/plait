import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import { realpath } from 'node:fs/promises';
import { projectKind } from '../safety.ts';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { validateAdmission, type Start, type Admission } from '../admission.ts';
import { fail, HostError, validate, type Snapshot } from '../protocol.ts';
import { defaultCatchUpMs, dueOccurrences, nextOccurrences, validateSchedule, type Schedule } from './schedule.ts';
import { schema } from './schema.ts';

type Send = (request: Record<string, unknown>, guard?: () => void) => Promise<unknown>;
type Definition = Pick<Start, 'harness' | 'provider' | 'model_id' | 'mode' | 'thinking' | 'approval'> & {
  name: string; instructions: string; target: { kind: 'project' | 'worktree'; checkout: string } | { kind: 'directory'; path: string };
  command_policy: 'sandboxed' | 'trusted'; schedule: Schedule; catch_up_ms: number;
};
type ScheduledClaim = { request_id: string; due: number; revision: number };
type Automation = { id: string; thread_id: string; definition: Definition; revision: number; state: string; pause_reason: string | null; cursor: number; missed_count: number };
const fields: Record<string, string[]> = {
  list_automations: [], automation_status: ['id'], get_automation: ['id'], create_automation: ['definition', 'request_id', 'caller'],
  update_automation: ['id', 'definition'], pause_automation: ['id'], resume_automation: ['id'],
  delete_automation: ['id'], run_automation_now: ['id', 'request_id'],
};
const settle = 60_000;
const drift = 'Settings changed outside the automation';
function key(value: unknown) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(value)) fail('invalid_request', 'Invalid ID.');
  return value;
}
function placement(d: Definition, request_id: string): Start {
  const { target, harness, provider, model_id, approval, thinking, mode } = d;
  return {
    op: 'create_admitted_thread', request_id, checkout: target.kind === 'directory' ? target.path : target.checkout,
    placement: target.kind === 'directory' ? 'wait' : 'worktree', harness, provider, model_id, approval,
    ...(mode === undefined ? {} : { mode }), ...(thinking === undefined ? {} : { thinking })
  };
}
function definition(input: unknown): Definition {
  const d = input as Definition;
  const allowed = ['name', 'instructions', 'target', 'harness', 'provider', 'model_id', 'approval', 'thinking', 'mode', 'command_policy', 'schedule', 'catch_up_ms'];
  if (!d || typeof d !== 'object' || Object.keys(d).some(k => !allowed.includes(k)))
    fail('invalid_request', 'Unknown automation definition field.');
  if (typeof d.name !== 'string' || !d.name.trim() || !['project', 'directory', 'worktree'].includes(d.target?.kind)
    || !['trusted', 'sandboxed'].includes(d.command_policy)) fail('invalid_request', 'Invalid automation definition.');
  const targetPath = d.target.kind === 'directory' ? 'path' : 'checkout';
  if (Object.keys(d.target).some(k => k !== 'kind' && k !== targetPath))
    fail('invalid_request', 'Unknown target field.');
  if (d.harness !== 'pi' && d.mode === undefined) fail('invalid_request', 'Select a harness mode.');
  validateAdmission(placement(d, 'validate'));
  validate({ op: 'submit', thread_id: 'validate', request_id: 'validate', text: d.instructions });
  let schedule: Schedule;
  try { schedule = validateSchedule(d.schedule); } catch (error) { fail('invalid_request', String(error)); }
  const catch_up_ms = d.catch_up_ms ?? defaultCatchUpMs(schedule);
  if (!Number.isSafeInteger(catch_up_ms) || catch_up_ms < 0) fail('invalid_request', 'Invalid catch-up window.');
  return { ...d, schedule, catch_up_ms };
}
function decode(row: Record<string, unknown>) {
  return { ...row, definition: JSON.parse(String(row.definition)) } as Automation;
}
export class Automations {
  private db: DatabaseSync;
  private send: Send;
  private runs = new Map<string, Promise<unknown>>();
  private timer?: NodeJS.Timeout;
  private running = false;
  private ticking?: Promise<void>;
  private readyAt = 0;
  constructor(directory: string, send: Send) {
    this.send = send;
    this.db = new DatabaseSync(join(directory, 'automations.sqlite'));
    this.db.exec('PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL');
    this.db.exec(schema);
  }
  handles(input: unknown) { return Object.hasOwn(fields, (input as { op?: string } | null)?.op ?? ''); }
  private list() { return this.db.prepare('SELECT * FROM automations ORDER BY rowid').all().map(decode); }
  private get(id: string) {
    const row = this.db.prepare('SELECT * FROM automations WHERE id=?').get(id);
    if (!row) fail('not_found', 'Automation not found.');
    return decode(row);
  }
  private placements = new Map<string, Promise<void>>();
  private pending() {
    return this.db.prepare(`SELECT p.*, c.definition FROM automation_placements p
      JOIN automation_creations c USING(id) ORDER BY p.rowid`).all().map(row => ({
      id: String(row.id), definition: JSON.parse(String(row.definition)) as Definition,
      cursor: Number(row.cursor), submitted: Boolean(row.submitted),
      error: row.error === null ? null : String(row.error),
    }));
  }
  private progress(p: ReturnType<Automations['pending']>[number]) {
    const request_id = 'automation-' + p.id;
    if (!p.error && !this.placements.has(p.id)) {
      const task = (async () => {
        try {
          const request = p.submitted ? { op: 'admission_status', request_id } : placement(p.definition, request_id);
          const target = p.definition.target;
          if (!p.submitted && target.kind === 'directory') {
            const location = await projectKind(target.path);
            if (location.vcs === 'git') {
              if (location.root !== await realpath(target.path))
                fail('invalid_request', 'This folder is inside a Git project. Choose the project instead.');
              Object.assign(request, { placement: 'worktree', checkout: location.root });
            }
          }
          const { admission } = await this.send(request) as { admission: Admission };
          if (admission.state === 'preparing') {
            this.db.prepare('UPDATE automation_placements SET submitted=1 WHERE id=?').run(p.id);
          } else if (admission.state === 'ready' && admission.thread_id) {
            this.transaction(() => {
              this.db.prepare("INSERT INTO automations(id,thread_id,definition,revision,state,cursor) VALUES (?,?,?,1,'active',?)")
                .run(p.id, admission.thread_id!, JSON.stringify(p.definition), p.cursor);
              this.db.prepare('DELETE FROM automation_placements WHERE id=?').run(p.id);
            });
          } else fail(admission.error?.code ?? 'busy', admission.error?.message ?? 'Placement failed.');
        } catch (error) {
          if (error instanceof HostError) this.db.prepare('UPDATE automation_placements SET error=? WHERE id=?').run(error.message, p.id);
          else console.error('Automation placement:', error);
        }
      })().finally(() => { this.placements.delete(p.id); this.arm(); });
      this.placements.set(p.id, task);
    }
    return { id: p.id, state: p.error ? 'failed' : 'preparing', admission_request_id: request_id, cursor: p.cursor, error: p.error };
  }
  private summary(a: Automation) {
    const { name, schedule, target } = a.definition;
    const last = this.db.prepare('SELECT due,outcome,detail FROM automation_occurrences WHERE automation_id=? ORDER BY due DESC,rowid DESC LIMIT 1').get(a.id);
    return { id: a.id, name, schedule, target_path: target.kind === 'directory' ? target.path : target.checkout,
      state: a.state, next_run: a.state === 'active' ? nextOccurrences(schedule, this.boundary(a), 1)[0] ?? null : null, last_result: last ?? null };
  }
  private create(d: Definition, id: string) {
    const saved = this.db.prepare('SELECT definition FROM automation_creations WHERE id=?').get(id);
    if (saved) {
      if (saved.definition === null) fail('not_found', 'This automation was deleted.');
      if (!isDeepStrictEqual(JSON.parse(String(saved.definition)), d)) fail('conflict', 'Creation request already used.');
    } else this.transaction(() => {
      this.db.prepare('INSERT INTO automation_creations VALUES (?,?)').run(id, JSON.stringify(d));
      this.db.prepare('INSERT INTO automation_placements(id,cursor) VALUES (?,?)').run(id, Date.now());
    });
    const pending = this.pending().find(p => p.id === id);
    return pending ? this.progress(pending) : this.summary(this.get(id));
  }
  private transaction<T>(work: () => T) {
    this.db.exec('BEGIN IMMEDIATE');
    try { const result = work(); this.db.exec('COMMIT'); return result; }
    catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  private state(id: string, state: string, reason: string | null) {
    this.db.prepare('UPDATE automations SET state=?,pause_reason=?,revision=revision+1,cursor=CASE WHEN ? THEN ? ELSE cursor END WHERE id=?').run(state, reason, Number(state === 'active'), Date.now(), id);
    return this.get(id);
  }
  handle(input: unknown, send: Send = this.send) {
    const r = input as Record<string, unknown>;
    const dispatch = async () => {
      const allowed = fields[String(r.op)];
      if (!this.handles(r) || Object.keys(r).some(k => k !== 'op' && !allowed.includes(k)))
        fail('invalid_request', 'Unknown automation operation or field.');
      if (r.op === 'list_automations') return { automations: [...this.list().map(a => this.summary(a)), ...this.pending().map(p => {
        const { name, schedule, target } = p.definition;
        return { id: p.id, name, schedule, target_path: target.kind === 'directory' ? target.path : target.checkout,
          state: this.progress(p).state, next_run: null, last_result: null };
      })] };
      if (r.op === 'create_automation') {
        if (r.caller !== undefined) {
          const caller = key((r.caller as { thread_id?: unknown } | null)?.thread_id);
          if (this.db.prepare('SELECT 1 FROM automations WHERE thread_id=?').get(caller))
            fail('permission_denied', 'An automation thread cannot create automations.');
        }
        const d = definition(r.definition);
        const request = key(r.request_id);
        const id = createHash('sha256').update(request).digest('hex');
        return { automation: this.create(d, id) };
      }
      const id = key(r.id);
      const pending = this.pending().find(p => p.id === id);
      if (pending) {
        if (r.op === 'delete_automation' && pending.error) {
          this.transaction(() => {
            this.db.prepare('DELETE FROM automation_placements WHERE id=?').run(id);
            this.db.prepare('UPDATE automation_creations SET definition=NULL WHERE id=?').run(id);
          });
          return { deleted: true };
        }
        const status = this.progress(pending);
        if (r.op === 'automation_status') return { automation: status };
        if (r.op === 'get_automation') return { automation: { ...status, definition: pending.definition }, occurrences: [] };
        fail('conflict', pending.error ?? 'Automation is preparing.');
      }
      const a = this.get(id);
      switch (r.op) {
        case 'automation_status': return { automation: this.summary(a) };
        case 'get_automation': return { automation: a, occurrences: this.db.prepare('SELECT * FROM automation_occurrences WHERE automation_id=? ORDER BY due DESC').all(a.id) };
        case 'delete_automation':
          this.transaction(() => {
            this.db.prepare('DELETE FROM automations WHERE id=?').run(a.id);
            this.db.prepare('UPDATE automation_creations SET definition=NULL WHERE id=?').run(a.id);
          });
          return { deleted: true };
        case 'pause_automation': return { automation: this.summary(this.state(a.id, 'paused', a.pause_reason)) };
        case 'resume_automation': return { automation: this.summary(this.state(a.id, 'active', null)) };
        case 'update_automation': {
          const d = definition(r.definition);
          const old = a.definition;
          if (!isDeepStrictEqual(d.target, old.target) || d.harness !== old.harness)
            fail('conflict', 'An existing thread is bound to its checkout and harness.');
          if (d.mode !== old.mode || d.model_id !== old.model_id || d.provider !== old.provider || d.thinking !== old.thinking) {
            await send({
              op: 'configure_thread', protocol_version: 6, thread_id: a.thread_id, provider: d.provider, model_id: d.model_id,
              ...(d.mode === undefined ? {} : { mode: d.mode }), ...(d.thinking === undefined ? {} : { thinking: d.thinking })
            });
          }
          this.db.prepare('UPDATE automations SET definition=?,cursor=?,revision=revision+1 WHERE id=?').run(JSON.stringify(d), Date.now(), a.id);
          return { automation: this.summary(this.get(a.id)) };
        }
        case 'run_automation_now': {
          const click = key(r.request_id);
          const now = Date.now();
          const claim = this.claimClick(a, click, now, this.arrivals.get(r));
          const receipt = this.receipt(id, String(claim.request_id));
          if (receipt) return { occurrence: receipt };
          if ('revision' in claim && claim.revision !== a.revision) return { occurrence: { ...claim, outcome: 'pending' } };
          this.launch(a, Number(claim.due), String(claim.request_id)).catch(error => console.error('Automation run:', error));
          return { occurrence: { ...claim, outcome: 'pending' } };
        }
        default: fail('invalid_request', 'Unknown automation operation.');
      }
    };
    return dispatch().finally(() => {
      if (r.op !== 'list_automations' && r.op !== 'get_automation') this.arm();
    });
  }
  private arrivals = new WeakMap<object, ScheduledClaim>();
  private scheduled = new Map<string, ScheduledClaim>();
  // Remember overlap at socket arrival, even if the receipt commits before the FIFO reaches the click.
  observeArrival(input: unknown) {
    const r = input as { op?: string; id?: string };
    if (r?.op !== 'run_automation_now' || typeof r.id !== 'string') return;
    const claim = this.scheduled.get(r.id);
    if (claim) this.arrivals.set(r, claim);
  }
  private claimClick(a: Automation, click: string, now: number, arrival?: ScheduledClaim) {
    const id = a.id;
    return this.transaction(() => {
      const saved = this.db.prepare('SELECT c.request_id,c.due,k.revision FROM automation_clicks c JOIN automation_claims k ON k.id=c.request_id WHERE c.automation_id=? AND click=?').get(id, click);
      if (saved) return saved;
      let pending;
      if (a.state === 'active') {
        pending = this.claimDue(a, now, true);
        if (!pending && arrival?.revision === a.revision && arrival.due <= now && now - arrival.due <= a.definition.catch_up_ms)
          pending = arrival;
        pending ??= this.db.prepare(`SELECT id AS request_id,due FROM automation_claims c
          WHERE automation_id=? AND occurrence BETWEEN ? AND ? AND revision=?
          AND NOT EXISTS (SELECT 1 FROM automation_occurrences WHERE automation_id=c.automation_id AND request_id=c.id)
          ORDER BY occurrence DESC LIMIT 1`).get(id, now - a.definition.catch_up_ms, now, a.revision);
      }
      const claim = pending ?? this.insertClaim(a, null, now, 'manual-' + createHash('sha256').update(id + '-' + click).digest('hex'));
      this.db.prepare('INSERT INTO automation_clicks VALUES (?,?,?,?)').run(id, click, claim.request_id, claim.due);
      return claim;
    });
  }
  private receipt(automation: string, id: string) {
    return this.db.prepare('SELECT * FROM automation_occurrences WHERE automation_id=? AND request_id=?').get(automation, id);
  }
  private launch(a: Automation, due: number, id: string) {
    const active = this.runs.get(id);
    if (active) return active;
    if (id === a.id + '-' + due) this.scheduled.set(a.id, { request_id: id, due, revision: a.revision });
    const task = this.run(a, due, id).finally(() => {
      this.runs.delete(id);
      if (this.scheduled.get(a.id)?.request_id === id) this.scheduled.delete(a.id);
    });
    this.runs.set(id, task);
    return task;
  }
  private async run(a: Automation, due: number, request_id: string) {
    const d = a.definition;
    let outcome = 'queued', detail: string | null = null;
    let checkout: string | undefined;
    try {
      const { thread } = await this.send({ op: 'watch', protocol_version: 6, thread_id: a.thread_id }) as { thread: Snapshot };
      checkout = thread.checkout;
      const { busy } = await this.send({ op: 'admission_info', checkout }) as { busy: boolean };
      if (busy || thread.status === 'running' || thread.status === 'awaiting_approval') {
        // A retry can find its own accepted message occupying the checkout.
        const { waiters } = await this.send({ op: 'checkout_queue', thread_id: a.thread_id }) as { waiters: { request_id: string }[] };
        if (!waiters.some(waiter => waiter.request_id === request_id)) {
          outcome = 'skipped'; detail = 'Checkout is busy.';
        }
      } else {
        const { policy } = await this.send({ op: 'command_policy', checkout }) as { policy: string };
        if ((thread.harness?.id ?? 'pi') !== d.harness || thread.model.provider !== d.provider || thread.model.model_id !== d.model_id
          || (thread.harness?.mode ?? null) !== (d.mode ?? null)
          || (d.thinking !== undefined && thread.thinking_level !== d.thinking) || policy !== d.command_policy) {
          detail = drift; outcome = 'paused';
        } else {
          await this.send({ op: 'submit_checkout', thread_id: a.thread_id, request_id, text: d.instructions, approval: d.approval }, () => {
            if (this.get(a.id).revision !== a.revision) fail('conflict', 'Automation changed before submission.');
          });
        }
      }
    } catch (error) {
      // An unknown transport failure has no definitive receipt. Retry with the same submit ID.
      const missing = (error as NodeJS.ErrnoException)?.code === 'ENOENT' &&
        (error as NodeJS.ErrnoException).syscall === 'lstat' && (error as NodeJS.ErrnoException).path === checkout;
      if (!(error instanceof HostError) && !missing) throw error;
      detail = (error as Error).message;
      if (error instanceof HostError && error.code === 'busy') outcome = 'skipped';
      else if (missing || error instanceof HostError && (error.code === 'checkout_missing' || error.code === 'permission_denied' && /checkout (?:root )?(?:identity )?changed/i.test(detail))) {
        outcome = 'paused'; detail = 'target missing';
      } else outcome = 'failed';
    }
    this.transaction(() => {
      const current = this.db.prepare('SELECT revision FROM automations WHERE id=?').get(a.id);
      if (!current) return;
      if (outcome === 'paused' && current.revision === a.revision) this.state(a.id, 'paused', detail);
      this.db.prepare('INSERT INTO automation_occurrences VALUES (?,?,?,?,?,?)').run(a.id, request_id, a.revision, due, outcome, detail);
    });
    return this.receipt(a.id, request_id);
  }
  private boundary(a: Automation) {
    const row = this.db.prepare('SELECT max(occurrence) AS latest FROM automation_claims WHERE automation_id=?').get(a.id);
    return Math.max(a.cursor, Number(row?.latest ?? a.cursor));
  }
  private insertClaim(a: Automation, occurrence: number | null, due: number, id: string) {
    this.db.prepare('INSERT INTO automation_claims VALUES (?,?,?,?,?)').run(id, a.id, occurrence, due, a.revision);
    return { request_id: id, due };
  }
  private claimDue(a: Automation, now: number, forClick = false) {
    const due = dueOccurrences(a.definition.schedule, this.boundary(a), now, a.definition.catch_up_ms);
    if (forClick && due.run === null) return null;
    for (const time of due.missed) {
      const claim = this.insertClaim(a, time, time, a.id + '-' + time);
      this.db.prepare("INSERT INTO automation_occurrences VALUES (?,?,?,?,'missed',NULL)").run(a.id, claim.request_id, a.revision, time);
    }
    if (due.missedTotal) this.db.prepare('UPDATE automations SET missed_count=missed_count+? WHERE id=?').run(due.missedTotal, a.id);
    return due.run === null ? null : this.insertClaim(a, due.run, due.run, a.id + '-' + due.run);
  }
  async tick(now = Date.now()) {
    const tasks = [];
    for (const a of this.list()) {
      if (a.state === 'active') this.transaction(() => this.claimDue(a, now));
      const pending = this.db.prepare(`SELECT id,due,revision FROM automation_claims c WHERE automation_id=?
        AND NOT EXISTS (SELECT 1 FROM automation_occurrences WHERE automation_id=c.automation_id AND request_id=c.id) ORDER BY due`).all(a.id);
      for (const claim of pending) {
        if (this.runs.has(String(claim.id))) tasks.push(this.runs.get(String(claim.id))!);
        else if (claim.revision !== a.revision) {
          this.db.prepare("INSERT OR IGNORE INTO automation_occurrences VALUES (?,?,?,?,'skipped','Automation changed before submission.')")
            .run(a.id, claim.id, claim.revision, claim.due);
        } else tasks.push(this.launch(a, Number(claim.due), String(claim.id)));
      }
    }
    const results = await Promise.allSettled(tasks);
    for (const result of results) if (result.status === 'rejected') throw result.reason;
  }
  start() { this.running = true; this.readyAt = Date.now() + settle; this.arm(); }
  private arm() {
    if (!this.running || this.ticking) return;
    clearTimeout(this.timer);
    const now = Date.now();
    let at = Infinity;
    for (const a of this.list()) if (a.state === 'active') {
      const next = nextOccurrences(a.definition.schedule, this.boundary(a), 1)[0];
      if (next !== undefined) at = Math.min(at, next);
    }
    if (this.db.prepare(`SELECT 1 FROM automation_claims c WHERE NOT EXISTS
      (SELECT 1 FROM automation_occurrences WHERE automation_id=c.automation_id AND request_id=c.id) LIMIT 1`).get()) at = Math.min(at, now + settle);
    // Periodically revisit a distant deadline so wake and wall-clock changes are observed.
    if (Number.isFinite(at)) at = Math.max(now, Math.min(at, now + settle), this.readyAt);
    if (this.pending().some(p => !p.error)) at = Math.min(at, now + 500);
    if (!Number.isFinite(at)) return;
    this.timer = setTimeout(async () => {
      for (const p of this.pending()) this.progress(p);
      if (Date.now() - at > 5000) this.readyAt = Date.now() + settle;
      if (Date.now() >= this.readyAt) {
        this.ticking = this.tick();
        try { await this.ticking; } catch (error) { console.error('Automation timer:', error); this.readyAt = Date.now() + settle; }
        finally { this.ticking = undefined; }
      }
      this.arm();
    }, at - now).unref();
  }
  async close() { this.running = false; clearTimeout(this.timer); await this.ticking?.catch(() => {}); await Promise.allSettled(this.runs.values()); await Promise.all(this.placements.values()); this.db.close(); }
}
