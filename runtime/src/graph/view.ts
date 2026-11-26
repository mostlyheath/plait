import { fail, type Summary } from '../protocol.ts';
import { layoutGraph, type LayoutEdge } from './layout.ts';
import { settings, type Storage, type Node, type Fact } from './storage.ts';
import { fitAnswer } from './text.ts';

export type Threads = Map<string, Summary>;
export function state(n: Fact, threads: Threads) {
  if (n.reason) return 'blocked';
  if (!n.started) return 'draft';
  const t = threads.get(n.thread!) ?? (n.summary ? JSON.parse(n.summary) as Summary : undefined);
  if (!t) return 'blocked';
  if (t.archived) return t.status === 'idle' && n.done ? 'done' : 'blocked';
  if (t.pending_approval || ['error', 'stopped', 'interrupted', 'awaiting_approval'].includes(t.status)) return 'needs_you';
  if (t.status === 'idle' && n.done) return 'done';
  if (t.status === 'running') return n.kind === 'discuss' ? 'discussing' : 'building';
  return 'draft';
}
/** Placement depends only on visible topology and stable node order. */
export class GraphView {
  private previous?: { topology: string; layout: ReturnType<typeof layoutGraph> };

  private layout(nodes: { id: string; order: number }[], edges: LayoutEdge[]) {
    const topology = JSON.stringify([nodes.map(n => [n.id, n.order]), edges]);
    if (this.previous?.topology === topology) return this.previous.layout;
    const layout = layoutGraph(nodes, edges);
    this.previous = { topology, layout };
    return layout;
  }
  read(store: Storage, project: string, identity: string, revision: unknown, page: { threads: Summary[]; next_cursor: string | null }) {
    if (revision !== undefined && (!Number.isSafeInteger(revision) || Number(revision) < 0)) fail('invalid_request', 'Invalid graph revision.');
    const db = store.db;
    const threads: Threads = new Map(page.threads.map(t => [t.id, t]));
    const signature = JSON.stringify([identity, !!page.next_cursor, page.threads.filter(t => t.checkout === project)]);
    const firstPoll = !db.prepare('SELECT 1 FROM pages WHERE project=?').get(project);
    db.prepare('INSERT INTO pages(project,summary) VALUES(?,?) ON CONFLICT(project) DO UPDATE SET summary=excluded.summary WHERE summary<>excluded.summary').run(project, signature);
    const graph_revision = store.revision();
    if (revision === graph_revision && !firstPoll) return { graph_revision, unchanged: true };
    const rows = db.prepare(`SELECT rowid AS ordering,id,thread,title,kind,started,reason,done,summary,settings,auto,requested,
    (SELECT count(*) FROM edges e JOIN nodes child ON child.id=e.dst WHERE e.src=nodes.id AND e.kind='fork' AND child.done) finished_children,
    (SELECT count(*) FROM edges e JOIN nodes child ON child.id=e.dst WHERE e.src=nodes.id AND e.kind='depends_on' AND NOT child.started) unblocks
    FROM nodes WHERE deleted=0 AND ((started=0 AND project_identity=?) OR (started=1 AND project=?)) AND
    (started=0 OR EXISTS(SELECT 1 FROM edges WHERE src=nodes.id OR dst=nodes.id)) ORDER BY rowid LIMIT 501`).all(identity, project) as (Node & { ordering: number; finished_children: number; unblocks: number })[];
    const selected = rows.slice(0, 500);
    for (const n of selected) if (n.thread && !threads.has(n.thread) && n.summary) threads.set(n.thread, JSON.parse(n.summary));
    const nodes = selected.map(n => {
      const t = threads.get(n.thread ?? '');
      const captured = n.settings ? settings(n) : undefined;
      return {
        id: n.id, order: n.ordering, thread_id: n.thread, title: fitAnswer(t?.title || n.title, 256), kind: n.kind,
        status: state(n, threads), reason: n.reason, archived: !!t?.archived, harness: t?.harness ?? captured?.harness,
        model_id: t?.model.model_id ?? captured?.model_id, lifecycle: n.started ? 'thread' : 'draft',
        start_when_ready: !!n.auto, waiting: { done: 0, total: 0 }, blocked_by: null as { id: string; title: string } | null,
        unblocks: n.unblocks, finished_children: n.finished_children
      };
    });
    const ids = JSON.stringify(nodes.map(n => n.id));
    const links = db.prepare(`SELECT src AS "from",dst AS "to",kind FROM edges WHERE dst IN (SELECT value FROM json_each(?))
    ORDER BY dst,src,kind LIMIT 2001`).all(ids) as LayoutEdge[];
    const indices = new Map(nodes.map((n, i) => [n.id, i]));
    const edges: LayoutEdge[] = [];
    const partial = new Set<string>();
    function count(target: typeof nodes[number], dep: Fact) {
      const status = state(dep, threads);
      target.waiting.total++;
      if (status === 'done') target.waiting.done++;
      else if (!target.blocked_by) {
        const t = threads.get(dep.thread ?? '') ?? (dep.summary ? JSON.parse(dep.summary) as Summary : undefined);
        if (status === 'blocked' || status === 'needs_you' && t?.status !== 'awaiting_approval')
          target.blocked_by = { id: dep.id, title: t?.title || dep.title || dep.id };
      }
    }
    for (const edge of links.slice(0, 2000)) {
      const src = indices.get(edge.from);
      if (src === undefined) { partial.add(edge.to); continue; }
      edges.push(edge);
      if (edge.kind === 'depends_on') count(nodes[indices.get(edge.to)!], selected[src]);
    }
    for (const [i, n] of nodes.entries()) {
      if (links.length > 2000 || partial.has(n.id)) {
        n.waiting = { done: 0, total: 0 }; n.blocked_by = null;
        for (const dep of store.dependencies(n.id, identity)) count(n, dep);
      }
      if (n.lifecycle === 'draft' && !n.reason) {
        if (n.blocked_by) n.status = 'blocked';
        else if (n.waiting.done < n.waiting.total) n.status = 'waiting';
        else if (n.waiting.total || selected[i].requested) n.status = 'ready';
      }
    }
    const candidates = page.threads.filter(t => t.checkout === project && !t.archived).map(t => t.id);
    const unlinked = Number(db.prepare(`SELECT count(*) n FROM json_each(?) candidate WHERE NOT EXISTS
    (SELECT 1 FROM nodes JOIN edges ON src=nodes.id OR dst=nodes.id WHERE thread=candidate.value)`).get(JSON.stringify(candidates))!.n);
    return { nodes, edges, layout: this.layout(nodes, edges), graph_revision, unlinked, truncated: !!page.next_cursor || rows.length > 500 || links.length > 2000 };
  }
}
