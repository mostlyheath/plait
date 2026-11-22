import { randomUUID } from 'node:crypto';
import { isAbsolute, normalize } from 'node:path';
import { fail, THINKING_LEVELS, type ThinkingLevel } from './protocol.ts';
import { once } from './operations.ts';
import type { Store } from './store.ts';
import { historyPage, historyText, type HistorySource } from './history.ts';

type StatusInput = { lifecycle: string; kind: string; threadStatus?: string; pending?: boolean; errorCode?: string; autoStart?: string; blocked?: boolean; queueState?: string; queuePosition?: number; done?: number; total?: number; completed?: boolean };
export function deriveStatus(i: StatusInput) {
  let status: string;
  let waiting: { done: number; total: number } | null = null;
  let queue_position: number | null = null;
  if (i.pending || i.threadStatus === 'awaiting_approval' || i.errorCode === 'cleanup_uncertain' || i.autoStart === 'held' || i.queueState === 'held') status = 'needs_you';
  else if (i.blocked || i.threadStatus === 'error' || i.threadStatus === 'interrupted') status = 'failed';
  else if (i.threadStatus === 'running') status = i.kind === 'discuss' ? 'discussing' : 'building';
  else if (i.queueState === 'waiting') { status = 'queued'; queue_position = i.queuePosition ?? null; }
  else if (i.autoStart === 'armed' && i.lifecycle === 'draft' && (i.total ?? 0) > 0) { status = 'waiting'; waiting = { done: i.done ?? 0, total: i.total ?? 0 }; }
  else if (i.lifecycle === 'draft') status = 'draft';
  else if (i.threadStatus === 'stopped' || (i.threadStatus === 'idle' && i.completed)) status = 'done';
  else status = 'draft';
  return { status, waiting, queue_position };
}
const fields: Record<string, string[]> = {
  graph: ['project'], create_draft: ['project','checkout','title','prompt','kind','placement','harness','provider','model_id','thinking','mode','approval','seed_text','parent_id','depends_on','auto_start','op_id'],
  fork_thread: ['parent_id','at_message_id','seed_mode','brief','title','prompt','kind','placement','harness','provider','model_id','thinking','mode','approval','depends_on','auto_start','start_immediately','op_id'],
  delete_draft: ['node_id','op_id'], set_dependencies: ['node_id','depends_on','auto_start','op_id'], set_node_kind: ['node_id','kind','pinned_root','op_id'],
};
const identifier = (v: unknown): v is string => typeof v === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(v);
function str(v: unknown, name: string, limit = 4096): string {
  if (typeof v !== 'string' || !v || Buffer.byteLength(v) > limit || v.includes('\0')) fail('invalid_request', `Invalid ${name}.`);
  return v;
}
function path(v: unknown): string {
  const value = str(v, 'path');
  if (!isAbsolute(value) || /[\u0000-\u001f\u007f]/u.test(value)) fail('invalid_request', 'Invalid path.');
  return normalize(value);
}
function ids(v: unknown): string[] {
  if (!Array.isArray(v) || v.length > 500 || v.some(id => !identifier(id)) || new Set(v).size !== v.length) fail('invalid_request', 'Invalid dependencies.');
  return v as string[];
}
export class Graph {
  private store: Store;
  private changed: () => void;
  private readHistory?: <T>(id: string, work: (source: HistorySource) => Promise<T>) => Promise<T>;
  constructor(store: Store, changed: () => void = () => {}, readHistory?: <T>(id: string, work: (source: HistorySource) => Promise<T>) => Promise<T>) {
    this.store = store;
    this.store.db.exec('CREATE TABLE IF NOT EXISTS workspace_worktrees(folder TEXT PRIMARY KEY, main TEXT NOT NULL, branch TEXT NOT NULL, creation_id TEXT, identity TEXT)');
    this.changed = changed;
    this.readHistory = readHistory;
  }
  handles(input: unknown) { return !!input && typeof input === 'object' && typeof (input as { op?: unknown }).op === 'string' && Object.hasOwn(fields, (input as { op: string }).op); }
  async handle(input: unknown): Promise<unknown> {
    if (!input || typeof input !== 'object' || Array.isArray(input)) fail('invalid_request', 'Invalid graph request.');
    const r = input as Record<string, any>;
    if (typeof r.op !== 'string' || !Object.hasOwn(fields, r.op) || Object.keys(r).some(k => k !== 'op' && !fields[r.op].includes(k)) || Buffer.byteLength(JSON.stringify(r)) > 65000) fail('invalid_request', 'Unknown graph operation or field.');
    if (r.op === 'graph') return this.read(r.project === undefined ? undefined : path(r.project));
    const { op_id, ...intent } = r;
    return once(this.store, op_id, r.op, intent, async () => {
      let result: unknown;
      switch (r.op) {
        case 'create_draft': result = this.createDraft(r); break;
        case 'fork_thread': result = await this.forkThread(r); break;
        case 'delete_draft': result = this.deleteDraft(r.node_id); break;
        case 'set_dependencies': result = this.setDependencies(r.node_id, r.depends_on, r.auto_start); break;
        case 'set_node_kind': result = this.setNodeKind(r.node_id, r.kind, r.pinned_root); break;
        default: fail('invalid_request', 'Unknown graph operation.');
      }
      this.changed();
      return result;
    });
  }
  private transaction(run: () => void) {
    this.store.db.exec('BEGIN IMMEDIATE');
    try { run(); this.store.db.exec('COMMIT'); }
    catch (error) { this.store.db.exec('ROLLBACK'); throw error; }
  }
  private revision() { return Number(this.store.db.prepare("SELECT value FROM settings WHERE key='graph_revision'").get()?.value ?? 0); }
  private bump() { const n = this.revision() + 1; this.store.db.prepare("INSERT INTO settings(key,value) VALUES ('graph_revision',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").run(String(n)); return n; }
  private ensure(id: unknown): string {
    if (!identifier(id) || !this.store.db.prepare('SELECT id FROM graph_nodes WHERE id=? UNION SELECT id FROM threads WHERE id=?').get(id, id)) fail('invalid_request', 'Unknown graph node.');
    return id;
  }
  private materialize(id: string) { this.store.db.prepare("INSERT OR IGNORE INTO graph_nodes(id,project,checkout,lifecycle,created_at,updated_at) SELECT t.id,COALESCE(w.main,t.checkout),t.checkout,'thread',COALESCE(t.created_at,0),COALESCE(t.updated_at,0) FROM threads t LEFT JOIN workspace_worktrees w ON w.folder=t.checkout WHERE t.id=?").run(id); }
  private replace(id: string, deps: string[]) {
    for (const dep of deps) this.ensure(dep);
    const adjacent = new Map<string,string[]>();
    for (const row of this.store.db.prepare("SELECT from_node,to_node FROM graph_edges WHERE kind='depends_on' AND to_node<>?").all(id)) { const from = String(row.from_node); const list = adjacent.get(from) ?? []; list.push(String(row.to_node)); adjacent.set(from, list); }
    const seen = new Set<string>(); const stack = [id];
    while (stack.length) { const next = stack.pop()!; if (seen.has(next)) continue; seen.add(next); stack.push(...(adjacent.get(next) ?? [])); }
    if (deps.some(dep => seen.has(dep))) fail('invalid_request', 'Dependencies would create a cycle.');
    this.store.db.prepare("DELETE FROM graph_edges WHERE kind='depends_on' AND to_node=?").run(id);
    const insert = this.store.db.prepare("INSERT INTO graph_edges(id,kind,from_node,to_node,created_at) VALUES (?,'depends_on',?,?,?)");
    for (const dep of deps) insert.run(randomUUID(),dep,id,Date.now());
  }
  private validateDraft(r: Record<string, any>) {
    path(r.checkout); if (r.project !== undefined) path(r.project);
    for (const [key, limit] of [['title', 256], ['prompt', 64000], ['provider', 256], ['model_id', 256]] as const) str(r[key], key, limit);
    if (/[\u0000-\u001f\u007f]/u.test(r.title) || /[\u0000-\u001f\u007f]/u.test(r.provider) || /[\u0000-\u001f\u007f]/u.test(r.model_id)) fail('invalid_request', 'Invalid draft text.');
    if (!['build','discuss'].includes(r.kind ?? 'build') || !['project','worktree','wait'].includes(r.placement ?? 'project') || !['pi','codex','claude'].includes(r.harness) || !['ask','auto'].includes(r.approval ?? 'ask') || (r.auto_start !== undefined && typeof r.auto_start !== 'boolean')) fail('invalid_request', 'Invalid draft settings.');
    if (r.mode !== undefined && (typeof r.mode !== 'string' || !/^[a-z_]{1,32}$/.test(r.mode))) fail('invalid_request', 'Invalid mode.');
    if (r.thinking !== undefined && !THINKING_LEVELS.includes(r.thinking as ThinkingLevel)) fail('invalid_request', 'Invalid thinking.');
    if (r.depends_on !== undefined) for (const dep of ids(r.depends_on)) this.ensure(dep);
  }
  createDraft(r: Record<string, any>) {
    this.validateDraft(r);
    const checkout = path(r.checkout); const project = r.project === undefined ? checkout : path(r.project);
    const title = str(r.title,'title',256); const prompt = str(r.prompt,'prompt',64000); const kind = r.kind ?? 'build'; const placement = r.placement ?? 'project'; const approval = r.approval ?? 'ask';
    if (!['build','discuss'].includes(kind) || !['project','worktree','wait'].includes(placement) || !['pi','codex','claude'].includes(r.harness) || !['ask','auto'].includes(approval) || (r.auto_start !== undefined && typeof r.auto_start !== 'boolean')) fail('invalid_request','Invalid draft settings.');
    const provider = str(r.provider,'provider',256); const model = str(r.model_id,'model',256);
    for (const key of ['thinking','mode','seed_text']) if (r[key] != null) str(r[key],key,32768);
    const deps = r.depends_on === undefined ? [] : ids(r.depends_on); if (r.parent_id != null) this.ensure(r.parent_id);
    const id = randomUUID(); let revision = 0;
    this.transaction(() => { const now = Date.now();
      this.store.db.prepare("INSERT INTO graph_nodes(id,project,checkout,lifecycle,kind,auto_start,created_at,updated_at) VALUES (?,?,?,'draft',?,?,?,?)").run(id,project,checkout,kind,r.auto_start ? 'armed' : 'manual',now,now);
      this.store.db.prepare('INSERT INTO graph_drafts(node_id,title,prompt,placement,harness,provider,model_id,thinking,mode,approval,seed_text) VALUES (?,?,?,?,?,?,?,?,?,?,?)').run(id,title,prompt,placement,r.harness,provider,model,r.thinking ?? null,r.mode ?? null,approval,r.seed_text ?? null);
      this.replace(id,deps); if (r.parent_id != null) this.store.db.prepare("INSERT INTO graph_edges(id,kind,from_node,to_node,source_message_id,created_at) VALUES (?,'fork',?,?,?,?)").run(randomUUID(),r.parent_id,id,r.at_message_id ?? null,now); revision = this.bump(); });
    return { node: this.read(undefined, id).nodes.find(n => n.id === id), graph_revision: revision };
  }
  deleteDraft(value: unknown) {
    const id = this.ensure(value); let revision = 0;
    this.transaction(() => { if (!this.store.db.prepare("SELECT id FROM graph_nodes WHERE id=? AND lifecycle='draft'").get(id)) fail('invalid_request','Only drafts can be deleted.');
      this.store.db.prepare('DELETE FROM graph_edges WHERE from_node=? OR to_node=?').run(id,id); this.store.db.prepare('DELETE FROM graph_drafts WHERE node_id=?').run(id); this.store.db.prepare('DELETE FROM graph_nodes WHERE id=?').run(id); revision = this.bump(); });
    return { graph_revision: revision };
  }
  setDependencies(value: unknown, dependencies: unknown, autoStart?: unknown) {
    const id = this.ensure(value); const deps = ids(dependencies); if (autoStart !== undefined && typeof autoStart !== 'boolean') fail('invalid_request','Invalid auto start.'); let revision = 0;
    this.transaction(() => { this.materialize(id); this.replace(id,deps); if (autoStart !== undefined) this.store.db.prepare('UPDATE graph_nodes SET auto_start=?,updated_at=? WHERE id=?').run(autoStart ? 'armed' : 'manual',Date.now(),id); revision = this.bump(); });
    const graph = this.read(undefined, id); return { node: graph.nodes.find(n => n.id === id), edges: graph.edges.filter(e => e.kind === 'depends_on' && e.to_node === id), graph_revision: revision };
  }
  setNodeKind(value: unknown, kind: unknown, pinned: unknown) {
    const id = this.ensure(value); if (!['build','discuss'].includes(String(kind)) || typeof pinned !== 'boolean') fail('invalid_request','Invalid node kind or pinned root.'); let revision = 0;
    this.transaction(() => { const thread = this.store.db.prepare('SELECT status FROM threads WHERE id=?').get(id); if (thread && ['running','awaiting_approval'].includes(String(thread.status))) fail('busy','The thread is running.');
      this.materialize(id); this.store.db.prepare('UPDATE graph_nodes SET kind=?,pinned_root=?,updated_at=? WHERE id=?').run(String(kind),pinned ? 1 : 0,Date.now(),id); revision = this.bump(); });
    return { node: this.read(undefined, id).nodes.find(n => n.id === id), graph_revision: revision };
  }
  private async forkThread(r: Record<string, any>) {
    if (r.start_immediately !== undefined && typeof r.start_immediately !== 'boolean') fail('invalid_request', 'Invalid start immediately.');
    if (r.start_immediately) fail('invalid_request', 'Starting nodes needs a newer host.');
    const parent = this.ensure(r.parent_id);
    const row = this.store.db.prepare('SELECT t.checkout,COALESCE(w.main,t.checkout) AS project FROM threads t LEFT JOIN workspace_worktrees w ON w.folder=t.checkout WHERE t.id=?').get(parent);
    if (!row || !this.readHistory) fail('invalid_request', 'Unknown parent thread.');
    if (r.at_message_id !== undefined) str(r.at_message_id, 'message ID', 256);
    this.validateDraft({ ...r, checkout: String(row.checkout), project: String(row.project) });
    const mode = r.seed_mode ?? 'both';
    if (!['excerpt', 'brief', 'both'].includes(mode)) fail('invalid_request', 'Invalid seed mode.');
    if (r.brief !== undefined) str(r.brief, 'brief', 64000);
    if (mode === 'brief' && r.brief === undefined) fail('invalid_request', 'Brief is required.');
    let excerpt = '';
    if (mode !== 'brief' || r.at_message_id !== undefined) {
      excerpt = await this.readHistory(parent, async source => {
        const parts: string[] = []; let size = 0; let found = r.at_message_id === undefined;
        let before: string | undefined;
        do {
          const page = await historyPage(source, before, 32);
          for (const item of page.items.slice().reverse()) {
            if (item.type !== 'message') continue;
            if (!found) { if (item.id !== r.at_message_id) continue; found = true; }
            if (mode === 'brief') break;
            let text = item.text;
            const more = item.more?.find(part => part.field === 'text');
            if (more) {
              // Fetch only the tail that can survive the seed limit.
              let offset = Math.max(more.offset, more.length - 24 * 1024);
              let part;
              try { part = await historyText(source, more.ref, offset); }
              catch { part = await historyText(source, more.ref, offset + 1); }
              text = part.text;
            }
            const line = `${item.role}: ${text.replaceAll('\0', '')}`;
            parts.unshift(line); size += Buffer.byteLength(line) + 2;
            if (size >= 24 * 1024) break;
          }
          if (found && (mode === 'brief' || size >= 24 * 1024)) break;
          before = page.before ?? undefined;
        } while (before);
        if (!found) fail('invalid_request', 'Unknown message.');
        return parts.join('\n\n');
      });
    }
    const seed = [mode !== 'brief' ? excerpt : '', mode !== 'excerpt' ? (r.brief ?? '') : ''].filter(Boolean).join('\n\n');
    const seedText = clipSeed(seed);
    return this.createDraft({ ...r, checkout: String(row.checkout), project: String(row.project), seed_text: seedText || undefined });
  }

  read(project?: string, onlyId?: string) {
    const rows = this.store.db.prepare(`SELECT * FROM (
      SELECT g.id,CASE WHEN t.id IS NOT NULL THEN COALESCE(w.main,g.project) ELSE g.project END AS project,g.checkout,g.lifecycle,g.kind,g.pinned_root,g.auto_start,g.blocked_by_failure,g.created_at,g.updated_at,d.title AS draft_title,t.status AS thread_status,t.pending,t.title AS thread_title,t.has_assistant,t.error_code FROM graph_nodes g LEFT JOIN graph_drafts d ON d.node_id=g.id LEFT JOIN threads t ON t.id=g.id LEFT JOIN workspace_worktrees w ON w.folder=t.checkout WHERE t.id IS NULL OR t.archived=0
      UNION ALL SELECT t.id,COALESCE(w.main,t.checkout),t.checkout,'thread','build',0,'manual',NULL,COALESCE(t.created_at,0),COALESCE(t.updated_at,0),NULL,t.status,t.pending,t.title,t.has_assistant,t.error_code FROM threads t LEFT JOIN workspace_worktrees w ON w.folder=t.checkout WHERE t.archived=0 AND NOT EXISTS (SELECT 1 FROM graph_nodes g WHERE g.id=t.id)
      ) WHERE (? IS NULL OR project=?) AND (? IS NULL OR id=?) ORDER BY updated_at DESC,id LIMIT 501`).all(project ?? null, project ?? null, onlyId ?? null, onlyId ?? null);
    let truncated = rows.length > 500; if (truncated) rows.pop();
    const selected = new Set(rows.map(row => String(row.id)));
    // Read only edges touching selected nodes; the extra row signals truncation.
    const edgeRows = selected.size ? this.store.db.prepare(`SELECT * FROM graph_edges WHERE to_node IN (${[...selected].map(() => '?').join(',')}) ORDER BY created_at DESC,id LIMIT 2001`).all(...selected) : [];
    if (edgeRows.length > 2000) { truncated = true; edgeRows.pop(); }
    const edges = edgeRows.map(r => ({ id: String(r.id), kind: String(r.kind), from_node: String(r.from_node), to_node: String(r.to_node), source_message_id: r.source_message_id, target_request_id: r.target_request_id, metadata: r.metadata, created_at: Number(r.created_at) }));
    const incoming = new Map<string, string[]>();
    for (const edge of edges) {
      if (edge.kind !== 'depends_on') continue;
      const deps = incoming.get(edge.to_node) ?? [];
      deps.push(edge.from_node);
      incoming.set(edge.to_node, deps);
    }
    const positions = new Map<string, { state: string; position: number }>();
    const counts = new Map<string, number>();
    for (const row of this.store.db.prepare("SELECT thread,checkout,state,kind FROM checkout_waiters WHERE state IN ('waiting','held') ORDER BY ordering").all()) {
      const checkout = String(row.checkout);
      const thread = String(row.thread); const previous = positions.get(thread);
      const position = row.kind === 'checkout' ? (counts.get(checkout) ?? 0) + 1 : 0;
      if (position) counts.set(checkout, position);
      positions.set(thread, { state: row.state === 'held' || previous?.state === 'held' ? 'held' : 'waiting', position: previous?.position || position });
    }
    const status = (row: Record<string, unknown>, done = 0, total = 0) => {
      const queued = positions.get(String(row.id));
      return deriveStatus({ lifecycle: String(row.lifecycle), kind: String(row.kind), threadStatus: String(row.thread_status ?? ''), pending: row.pending === 1, errorCode: String(row.error_code ?? ''), autoStart: String(row.auto_start), blocked: row.blocked_by_failure != null, queueState: queued?.state, queuePosition: queued?.position, done, total, completed: row.has_assistant === 1 });
    };
    const base = new Map(rows.map(row => [String(row.id), status(row)]));
    const missing = [...new Set(edges.filter(edge => edge.kind === 'depends_on' && !base.has(edge.from_node)).map(edge => edge.from_node))];
    if (missing.length) {
      const dependencies = this.store.db.prepare(`SELECT t.id,COALESCE(g.lifecycle,'thread') AS lifecycle,COALESCE(g.kind,'build') AS kind,g.auto_start,g.blocked_by_failure,t.status AS thread_status,t.pending,t.error_code,t.has_assistant FROM threads t LEFT JOIN graph_nodes g ON g.id=t.id WHERE t.id IN (${missing.map(() => '?').join(',')})`).all(...missing);
      for (const row of dependencies) base.set(String(row.id), status(row));
    }
    const nodes = rows.filter(row => project === undefined || row.project === project).map(row => {
      const deps = incoming.get(String(row.id)) ?? [];
      const done = deps.filter(id => base.get(id)?.status === 'done').length;
      return { id: String(row.id), project: String(row.project), checkout: String(row.checkout), lifecycle: String(row.lifecycle), kind: String(row.kind), pinned_root: row.pinned_root === 1, title: String(row.draft_title ?? row.thread_title ?? ''), ...status(row, done, deps.length) };
    });
    const included = new Set(nodes.map(node => node.id));
    const result = { nodes, edges: onlyId ? edges : edges.filter(edge => included.has(edge.from_node) && included.has(edge.to_node)), graph_revision: this.revision(), truncated };
    while (Buffer.byteLength(JSON.stringify(result)) > 800000 && (result.edges.length || result.nodes.length)) {
      result.truncated = true;
      if (result.edges.length) result.edges.pop(); else { const removed = result.nodes.pop()!; result.edges = result.edges.filter(edge => edge.from_node !== removed.id && edge.to_node !== removed.id); }
    }
    return result;
  }
}

function clipSeed(text: string): string {
  const limit = 24 * 1024;
  if (Buffer.byteLength(text) <= limit) return text;
  const marker = '[Earlier text clipped]\n';
  const bytes = Buffer.from(text);
  let start = bytes.length - (limit - Buffer.byteLength(marker));
  while ((bytes[start] & 0xc0) === 0x80) start++;
  return marker + bytes.subarray(start).toString('utf8');
}
