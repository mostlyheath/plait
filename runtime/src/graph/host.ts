import { HostTransportError } from '../host-client.ts';
import { realpath } from 'node:fs/promises';
import { randomUUID, createHash } from 'node:crypto';
import type { HarnessEntry } from '../harness.ts';
import { validateAdmission, type Admission, type Start } from '../admission.ts';
import { fail, failure, type Summary, type Snapshot } from '../protocol.ts';
import { GraphView, state } from './view.ts';
import { Storage, settings, type Node, type Fact } from './storage.ts';
import { fields, graphRequest, SETTINGS_CHANGED, type Request, type Send } from './requests.ts';
import { rootIdentity, type FileIdentity } from '../safety.ts';
import { Deadline, ReadTimeout, fitAnswer, excerpt } from './text.ts';

function string(v: unknown, name: string, max = 128) {
  if (typeof v !== 'string' || !v.trim() || v.includes('\0') || Buffer.byteLength(v) > max)
    fail('invalid_request', 'Invalid ' + name + '.');
  return v;
}
function checkPromptSize(value: unknown) {
  if (typeof value === 'string' && Buffer.byteLength(JSON.stringify(value)) - 2 > 24576)
    fail('invalid_request', 'Brief is too long');
}
function kind(v: unknown) {
  if (v !== 'discuss' && v !== 'build') fail('invalid_request', 'Invalid thread kind.');
  return v;
}
function flag(v: unknown) {
  if (typeof v !== 'boolean') fail('invalid_request', 'Expected Start when ready setting.');
  return Number(v);
}
function clean(t: Snapshot) {
  return t.status === 'idle' && !t.error && !t.pending_approval && !t.queued?.length && t.messages.at(-1)?.role === 'assistant';
}
function matchesSettings(start: Start, t: Snapshot, baseline: string | null) {
  const previous = baseline ? JSON.parse(baseline) as { mode: string | null; thinking: string | null; approval: string } : undefined;
  // For an in-place admission the host applies approval on submit. Until the
  // first turn, watch reports its initial default. Detect changes from that baseline.
  let approval = start.approval;
  if (!t.messages.length && t.checkout === start.checkout && start.approval === 'auto') {
    approval = (previous?.approval ?? t.approval_mode) === 'auto' ? 'auto' : 'ask';
  }
  return (t.harness?.id ?? 'pi') === start.harness && t.model.provider === start.provider && t.model.model_id === start.model_id &&
    (t.harness?.mode ?? null) === (start.mode ?? (previous ? previous.mode : t.harness?.mode ?? null)) &&
    (t.thinking_level ?? null) === (start.thinking ?? (previous ? previous.thinking : t.thinking_level ?? null)) &&
    (t.approval_mode ?? 'ask') === approval;
}
// An invalid explicit size may leave libuv with just one worker.
function probeLimit() {
  const size = process.env.UV_THREADPOOL_SIZE ?? '4';
  if (!/^[0-9]+$/.test(size)) return 0;
  const workers = Number(size);
  if (workers > 1024) return 0;
  return Math.max(0, Math.min(2, workers - 1));
}

export class Graph {
  private store: Storage;
  private view = new GraphView();
  private send: Send;
  private tail: Promise<unknown> = Promise.resolve();
  private timer?: NodeJS.Timeout;
  private closing = false;
  private edits = 0;
  private starting = new Set<string>();
  private projects = new Set<string>();
  private readonly sourceIdentity: (id: string) => Promise<FileIdentity>;
  private readonly receipt: (id: string, request: string) => { state: string; error: { message: string } | null } | undefined;
  private readonly probeLimit = probeLimit();
  private readonly slowPollBudget = new Map<string, number>();
  private readonly probing = new Set<string>();
  private readonly savedIdentity: (id: string) => FileIdentity;
  private readonly probe: (path: string) => Promise<{ project: string; identity: string }>;
  constructor(directory: string, send: Send, sourceIdentity: Graph['sourceIdentity'], receipt: Graph['receipt'],
    savedIdentity: (id: string) => FileIdentity,
    probe = async (path: string) => {
      const project = await realpath(path);
      return { project, identity: JSON.stringify(await rootIdentity(project)) };
    }) {
    this.savedIdentity = savedIdentity;
    this.probe = probe;
    this.sourceIdentity = sourceIdentity;
    this.receipt = receipt;
    this.store = new Storage(directory);
    this.send = send;
  }
  private async pollIdentity(path: string) {
    // Timed-out reads keep their slots until the filesystem operation settles.
    if (this.probing.size >= this.probeLimit || this.probing.has(path)) return null;
    this.probing.add(path);
    const started = performance.now();
    let timedOut = false;
    let timer: NodeJS.Timeout | undefined;
    const pending = new Promise<null>(resolve => {
      timer = setTimeout(() => {
        timedOut = true;
        resolve(null);
      }, this.slowPollBudget.get(path) ?? 40);
    });
    const read = this.probe(path).catch(() => null).then(current => {
      this.probing.delete(path);
      if (this.closing) return current;
      const elapsed = performance.now() - started;
      if (!current || elapsed < 40) this.slowPollBudget.delete(path);
      if (timedOut && current) {
        // Give the next check time to verify even a replaced directory. Keep
        // the queue bounded if a previously responsive volume stalls.
        this.slowPollBudget.delete(path);
        if (this.slowPollBudget.size >= 128)
          this.slowPollBudget.delete(this.slowPollBudget.keys().next().value!);
        this.slowPollBudget.set(path, Math.min(250, Math.ceil(elapsed * 2)));
        this.store.updatePageIdentity(path, current.project, current.identity);
      }
      return current;
    });
    try { return await Promise.race([read, pending]); }
    finally { clearTimeout(timer); }
  }
  handles(input: unknown): input is Request {
    return !!input && typeof input === 'object' && graphRequest((input as Request).op);
  }
  private serialize<T>(work: () => Promise<T>) {
    const result = this.tail.then(work);
    this.tail = result.catch(() => { });
    return result;
  }
  ownsSubmission(thread: string, request: string) {
    const node = this.store.node(thread);
    return node?.thread === thread && request === 'submit-' + node.id;
  }
  summaryChanged(thread: string) {
    if (this.closing) return;
    const node = this.store.node(thread);
    if (node) this.wake(node.project);
  }
  wake(project?: string, delay = 25) {
    if (project) this.projects.add(project);
    else for (const row of this.store.db.prepare('SELECT DISTINCT project FROM nodes WHERE deleted=0').all()) this.projects.add(String(row.project));
    if (this.closing || this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      const projects = [...this.projects];
      this.projects.clear();
      for (const project of projects) void this.reconcile(project).catch(() => this.wake(project, 1000));
    }, delay);
    this.timer.unref();
  }
  async close() { this.closing = true; clearTimeout(this.timer); await Promise.allSettled(this.scans.values()); await this.tail; this.store.db.close(); }
  private watch(thread_id: string, send = this.send) {
    return send({ op: 'stored_thread', thread_id, protocol_version: 6 }) as Promise<{ thread: Snapshot }>;
  }
  private list(send: Send) {
    return send({ op: 'list_threads', archived: true, limit: 200, protocol_version: 6 }) as Promise<{ threads: Summary[]; next_cursor: string | null }>;
  }
  private async resolve(value: unknown, send: Send) {
    const id = string(value, 'node');
    const existing = this.store.node(id);
    if (existing) return existing;
    const { thread } = await this.watch(id, send);
    this.store.db.prepare('INSERT INTO nodes(id,project,thread,title,started,done,project_identity) VALUES(?,?,?,?,1,?,?)')
      .run(id, thread.checkout, id, thread.title ?? '', Number(clean(thread)), JSON.stringify(this.savedIdentity(id)));
    return this.store.node(id)!;
  }
  private async dependencies(input: unknown, project: string, send: Send) {
    if (!Array.isArray(input) || input.length > 500) fail('invalid_request', 'Invalid dependencies.');
    const ids = [];
    for (const value of input) {
      const dep = await this.resolve(value, send);
      if (dep.project !== project) fail('invalid_request', 'Dependencies must belong to the same project.');
      ids.push(dep.id);
    }
    if (new Set(ids).size !== ids.length) fail('invalid_request', 'Duplicate dependency.');
    return ids;
  }
  private replace(id: string, ids: string[]) {
    const db = this.store.db;
    const cycle = db.prepare(`WITH RECURSIVE children(id) AS (VALUES(?) UNION SELECT dst FROM edges JOIN children ON src=children.id)
      SELECT 1 FROM children WHERE id IN (SELECT value FROM json_each(?)) LIMIT 1`).get(id, JSON.stringify(ids));
    if (cycle) fail('invalid_request', 'Dependency would create a cycle.');
    db.prepare("DELETE FROM edges WHERE dst=? AND kind='depends_on'").run(id);
    for (const src of ids) db.prepare("INSERT INTO edges VALUES(?,?,'depends_on')").run(src, id);
  }
  private reply(id: string) { return { node_id: id, graph_revision: this.store.revision() }; }
  handle(r: Request, foreground = this.send): Promise<unknown> {
    const deadline = new Deadline();
    const send: Send = request => deadline.read(() => foreground(request));
    const ready = deadline.read(() => this.tail);
    // A timed-out waiter must never execute its mutation later.
    const result = this.serialize(async () => {
      await ready;
      const op = String(r.op);
      if (op === 'create_draft' || op === 'fork_thread') {
        checkPromptSize(r.prompt);
        checkPromptSize(r.brief);
      }
      if (!graphRequest(op) || Object.keys(r).some(k => k !== 'op' && k !== 'op_id' && !fields[op].includes(k)) || Buffer.byteLength(JSON.stringify(r)) > 65000)
        fail('invalid_request', 'Unknown graph operation or field.');
      if (r.op_id !== undefined && (typeof r.op_id !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(r.op_id))) fail('invalid_request', 'Invalid operation ID.');
      if (op === 'graph') {
        const named = string(r.project, 'project', 4096);
        if (!named.startsWith('/')) fail('invalid_request', 'Expected an absolute project path.');
        const [current, page] = await Promise.all([this.pollIdentity(named), this.list(send)]);
        return this.view.read(this.store, named, current, r.revision, page);
      }
      if (op === 'create_draft' || op === 'fork_thread') return this.create(r, send, deadline);
      if (op === 'send_results') {
        const parent = await this.resolve(r.parent_id, send);
        const children = this.store.resultChildren(parent.id);
        return { text: await this.results(children.map(child => child.thread), children[0]?.total ?? 0, 62000, send) };
      }
      const n = await this.resolve(r.node_id, send), db = this.store.db;
      const receipt = n.thread && this.receipt(n.thread, 'submit-' + n.id);
      if (!n.started && n.submission && op !== 'start_draft' && (!receipt || !['cancelled', 'failed', 'started'].includes(receipt.state)))
        fail('conflict', 'This draft is waiting to start. Cancel it first.');
      if (this.starting.has(n.id)) fail('busy', 'The draft is being queued.');
      if (op === 'set_node_kind') {
        if (n.thread && ['running', 'awaiting_approval'].includes((await this.watch(n.thread, send)).thread.status)) fail('busy', 'The thread is running.');
        if ((n.started || receipt && receipt.state === 'started') && r.kind === 'discuss') fail('unsupported', 'Read-only Discuss execution is unavailable.');
        db.prepare('UPDATE nodes SET kind=? WHERE id=?').run(kind(r.kind), n.id);
      } else {
        if (n.started || receipt && receipt.state === 'started' && op !== 'start_draft') fail('conflict', 'Only unstarted drafts can be changed.');
        if (op === 'delete_draft') this.store.transaction(() => {
          db.prepare('DELETE FROM edges WHERE src=? OR dst=?').run(n.id, n.id);
          db.prepare("UPDATE nodes SET deleted=1,prompt='',submission=NULL,settings=NULL,title='' WHERE id=?").run(n.id);
        });
        if (op === 'set_start_when_ready') db.prepare('UPDATE nodes SET auto=? WHERE id=?').run(flag(r.start_when_ready), n.id);
        if (op === 'set_dependencies') {
          const deps = await this.dependencies(r.depends_on, n.project, send);
          this.store.transaction(() => this.replace(n.id, deps));
        }
        if (op === 'start_draft') db.prepare('UPDATE nodes SET requested=1,reason=NULL WHERE id=?').run(n.id);
      }
      this.edits++;
      this.wake(n.project);
      return this.reply(n.id);
    });
    return ready.then(() => result);
  }
  private async create(r: Request, send: Send, deadline: Deadline) {
    const db = this.store.db;
    if (typeof r.op_id !== 'string') fail('invalid_request', 'Operation ID is required.');
    const payload = createHash('sha256').update(JSON.stringify(Object.entries(r).sort(([a], [b]) => a.localeCompare(b)))).digest('hex');
    const prior = db.prepare('SELECT id,payload,deleted FROM nodes WHERE op_id=?').get(r.op_id);
    if (prior) {
      if (prior.payload !== payload || prior.deleted) fail('conflict', 'Operation ID already used for different input or a deleted draft.');
      return this.reply(String(prior.id));
    }
    const parent = r.op === 'fork_thread' ? await this.resolve(r.parent_id, send) : undefined;
    if (parent && !parent.started) fail('invalid_request', 'Only a thread can be forked.');
    // Admission compares canonical roots (on macOS /var is a link to /private/var).
    const given = parent ? undefined : string(r.checkout, 'checkout', 4096);
    const project = parent?.project ?? await realpath(given!).catch(() => given!);
    if (r.project !== undefined && r.project !== project) fail('invalid_request', 'Checkout belongs to another project.');
    const id = randomUUID();
    const checkout = parent ? (await this.watch(parent.thread!, send)).thread.checkout : project;
    const start = validateAdmission({
      op: 'create_admitted_thread', request_id: 'create-' + id, placement: 'project', checkout,
      harness: r.harness, provider: r.provider, model_id: r.model_id, approval: r.approval ?? 'ask',
      ...(r.mode !== undefined && { mode: r.mode }), ...(r.thinking !== undefined && { thinking: r.thinking })
    }) as Start;
    const identity = await deadline.read(() => rootIdentity(checkout));
    const projectIdentity = checkout === project ? identity : await deadline.read(() => rootIdentity(project));
    if (parent) await deadline.read(() => this.checkSource(parent.thread!, identity));
    if (parent?.project_identity && parent.project_identity !== JSON.stringify(projectIdentity))
      fail('conflict', 'The project at this path changed');
    if (start.harness !== 'pi' && start.mode === undefined) {
      const { harnesses } = await send({ op: 'harnesses', protocol_version: 6 }) as { harnesses: HarnessEntry[] };
      const mode = harnesses.find(entry => entry.id === start.harness)?.default_mode;
      if (!mode) fail('unavailable', 'Cannot read the selected harness mode.');
      start.mode = mode;
    }
    const title = string(r.title, 'title', 256);
    let prompt = r.prompt === undefined ? '' : string(r.prompt, 'prompt', 64000);
    if (parent) {
      const seed = r.seed_mode ?? 'both';
      if (!['excerpt', 'both', 'brief'].includes(String(seed))) fail('invalid_request', 'Invalid seed mode.');
      const at = r.at_message_id === undefined ? undefined : string(r.at_message_id, 'message ID', 256);
      const text = seed !== 'brief' || at ? await excerpt(send, parent.thread!, at, seed === 'brief' ? 'locate' : 'conversation') : '';
      const brief = r.brief === undefined && seed !== 'brief' ? '' : string(r.brief, 'brief', 64000);
      prompt = [text, seed === 'excerpt' ? '' : brief, prompt].filter(Boolean).join('\n\n');
    }
    checkPromptSize(prompt);
    string(prompt, 'prompt', 64000);
    const deps = await this.dependencies(r.depends_on ?? [], project, send);
    this.store.transaction(() => {
      db.prepare('INSERT INTO nodes(id,project,title,kind,auto,settings,prompt,op_id,payload,identity,project_identity) VALUES(?,?,?,?,?,?,?,?,?,?,?)')
        .run(id, project, title, kind(r.kind ?? 'build'), flag(r.start_when_ready ?? true), JSON.stringify(start), prompt, r.op_id as string, payload, JSON.stringify(identity), JSON.stringify(projectIdentity));
      if (parent) db.prepare("INSERT INTO edges VALUES(?,?,'fork')").run(parent.id, id);
      this.replace(id, deps);
    });
    this.wake(project); return this.reply(id);
  }
  private async checkSource(thread: string, identity: FileIdentity) {
    const saved = await this.sourceIdentity(thread).catch(() => null);
    if (!saved || saved.dev !== identity.dev || saved.ino !== identity.ino)
      fail('permission_denied', 'Source is from another project');
  }
  private async results(threads: string[], total: number, budget: number, send: Send) {
    const parts: string[] = [];
    for (const thread of threads) {
      try {
        if (!clean((await this.watch(thread, send)).thread)) continue;
        const link = `[Thread](plait://thread/${thread})\n`;
        const allowance = Math.floor(budget / threads.length) - Buffer.byteLength(link) - 8;
        parts.push(link + fitAnswer(await excerpt(send, thread, undefined, 'answer'), allowance));
      } catch (error) {
        if (error instanceof ReadTimeout) break;
        throw error;
      }
    }
    if (total > parts.length) parts.push(`[${total - parts.length} results left out]`);
    return parts.join('\n\n');
  }
  private dirty = new Set<string>();
  private scans = new Map<string, Promise<void>>();
  reconcile(project: string): Promise<void> {
    const active = this.scans.get(project);
    if (active) { this.dirty.add(project); return active; }
    const scan = this.refresh(project).finally(() => {
      this.scans.delete(project);
      if (this.dirty.delete(project)) this.wake(project);
    });
    this.scans.set(project, scan);
    return scan;
  }
  private batchTail: Promise<unknown> = Promise.resolve();
  private async refresh(project: string) {
    const db = this.store.db;
    let cursor = 0;
    while (!this.closing) {
      const rows = db.prepare(`SELECT rowid,id,thread, (project_identity IS NULL AND settings IS NULL) AS imported FROM nodes WHERE project=? AND rowid>? AND deleted=0 AND started=1
        AND EXISTS(SELECT 1 FROM edges WHERE src=nodes.id OR dst=nodes.id) ORDER BY rowid LIMIT 50`).all(project,cursor) as {rowid:number; id:string; thread:string; imported:number}[];
      const batch = this.batchTail.then(async () => {
        for (const n of rows) {
          try {
            const { thread: t } = await this.watch(n.thread);
            // Older imported threads have no admission settings or captured graph identity.
            if (n.imported)
              db.prepare('UPDATE nodes SET project_identity=? WHERE id=?').run(JSON.stringify(this.savedIdentity(n.thread)), n.id);
            const summary = {
              id: t.id, checkout: t.checkout, title: t.title ?? '', model: t.model, status: t.status, archived: !!t.archived,
              harness: t.harness?.id ?? 'pi', pending_approval: !!t.pending_approval, created_at: t.created_at ?? 0, updated_at: t.updated_at ?? 0
            };

            const done = Number(clean(t)), encoded = JSON.stringify(summary);
            db.prepare('UPDATE nodes SET done=?,summary=? WHERE id=? AND (done<>? OR summary IS NOT ?)')
              .run(done, encoded, n.id, done, encoded);
          } catch (error) {
            // One unavailable thread must not prevent other projects from starting.
            if (failure(error).code !== 'not_found') this.wake(project, 1000);
            db.prepare('UPDATE nodes SET done=0,summary=NULL WHERE id=? AND (done<>0 OR summary IS NOT NULL)').run(n.id);
          }
        }
      });
      this.batchTail = batch.catch(() => {}).then(() => new Promise<void>(resolve => setImmediate(resolve)));
      await batch;
      if (rows.length < 50) break;
      cursor = rows[rows.length - 1].rowid;
    }
    cursor = 0;
    while (!this.closing) {
      const drafts = db.prepare(`SELECT rowid,* FROM nodes WHERE project=? AND rowid>? AND deleted=0 AND started=0 AND reason IS NULL
        AND (requested OR auto AND EXISTS(SELECT 1 FROM edges WHERE dst=nodes.id AND kind='depends_on'))
        ORDER BY rowid LIMIT 50`).all(project, cursor) as (Node & { rowid: number })[];
      if (!drafts.length) break;
      const dependencies = this.store.batchDependencies(drafts.map(n => n.id));
      const edits = this.edits;
      for (const n of drafts) {
        if (this.closing) return;
        // A client edit during an admission can invalidate the remaining batch.
        if (edits !== this.edits) { this.wake(project); return; }
        const deps = dependencies.get(n.id) ?? [];
        if (!n.submission && deps.some(dep => state(dep, new Map()) !== 'done')) continue;
        const claimed = await this.serialize(async () => {
          if (edits !== this.edits || this.closing) return false;
          this.starting.add(n.id);
          return true;
        });
        if (!claimed) { this.wake(project); return; }
        try { await this.start(n, deps); }
        catch (error) {
          if (error instanceof HostTransportError) { this.wake(project, 1000); return; }
          db.prepare('UPDATE nodes SET reason=? WHERE id=?').run(failure(error).message, n.id);
        } finally { this.starting.delete(n.id); }
      }
      if (drafts.length < 50) break;
      cursor = drafts[drafts.length - 1].rowid;
      await new Promise<void>(resolve => setImmediate(resolve));
    }
  }
  private async start(n: Node, deps: Fact[]) {
    if (n.kind === 'discuss') fail('unsupported', 'Read-only Discuss execution is unavailable.');
    const start = settings(n), db = this.store.db;
    let thread = n.thread;
    const request = 'submit-' + n.id;
    const queued = thread && this.receipt(thread, request);
    if (queued) {
      if (queued.state === 'started') {
        db.prepare('UPDATE nodes SET started=1 WHERE id=?').run(n.id);
        this.wake(n.project);
      } else if (queued.state === 'failed' || queued.state === 'cancelled') {
        fail('conflict', queued.error?.message ?? 'Queued start was cancelled');
      }
      return;
    }
    const current = await rootIdentity(start.checkout).catch(() => null);
    if (!current || JSON.stringify(current) !== n.identity) fail('conflict', 'The project at this path changed');
    for (const dep of deps) await this.checkSource(dep.thread!, current);
    if (!thread) {
      let { admission } = await this.send(start) as { admission: Admission };
      if (admission.state === 'choice') {
        const wait: Start = { ...start, placement: 'wait', request_id: start.request_id + '-wait' };
        db.prepare('UPDATE nodes SET settings=? WHERE id=?').run(JSON.stringify(wait), n.id);
        ({ admission } = await this.send(wait) as { admission: Admission });
      }
      if (admission.state === 'preparing') {
        ({ admission } = await this.send({ op: 'admission_status', request_id: admission.request_id }) as { admission: Admission });
      }
      if (admission.state === 'failed') fail(admission.error?.code ?? 'conflict', admission.error?.message ?? 'Admission failed.');
      if (!admission.thread_id || admission.state !== 'ready') { this.wake(n.project, 1000); return; }
      const parent = admission.worktree?.parent;
      if ((parent?.root ?? admission.checkout) !== start.checkout ||
          parent && (parent.identity?.dev !== current.dev || parent.identity?.ino !== current.ino))
        fail('conflict', 'Admission belongs to another checkout');
      thread = admission.thread_id;
      db.prepare('UPDATE nodes SET thread=? WHERE id=?').run(thread, n.id);
    }
    let text = n.submission;
    if (!text) {
      const deadline = new Deadline();
      const answers = await this.results(deps.map(dep => dep.thread!), deps.length, 31000, request => deadline.read(() => this.send(request)));
      text = answers ? n.prompt + '\n\n' + fitAnswer(answers, 31000) : n.prompt;
    }
    for (const dep of deps) {
      if (!clean((await this.watch(dep.thread!)).thread)) { this.wake(n.project); return; }
    }
    const { thread: destination } = await this.watch(thread);
    if (!matchesSettings(start, destination, n.observed)) fail('conflict', SETTINGS_CHANGED);
    if (!n.observed) {
      const observed = JSON.stringify({
        mode: destination.harness?.mode ?? null, thinking: destination.thinking_level ?? null,
        approval: destination.approval_mode ?? 'ask'
      });
      db.prepare('UPDATE nodes SET observed=? WHERE id=?').run(observed, n.id);
    }
    if (this.closing) return;
    if (!n.submission) db.prepare('UPDATE nodes SET submission=? WHERE id=?').run(text, n.id);
    const identity = destination.checkout === start.checkout ? current : await this.sourceIdentity(thread);
    await this.send({ op: 'submit_checkout', identity, thread_id: thread, request_id: request, text, approval: start.approval,
      expect: { harness: start.harness, provider: start.provider, model_id: start.model_id,
        mode: destination.harness?.mode ?? null, thinking: destination.thinking_level ?? null, approval: destination.approval_mode ?? 'ask' }
    });
    this.wake(n.project);
  }
}
