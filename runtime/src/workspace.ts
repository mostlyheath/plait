// The person's workspace on this host: which projects are open or hidden,
// which worktrees belong to which project, and what has been seen. Every client
// reads it from here, so another machine's window shows this host's projects.
import { randomUUID } from 'node:crypto';
import { hostname } from 'node:os';
import { isAbsolute } from 'node:path';
import { fail } from './protocol.ts';
import { once } from './operations.ts';
import { checkout } from './safety.ts';
import type { Store } from './store.ts';
import type { CheckoutWatcher } from './watcher.ts';

/** The reserved notification name: the workspace changed. */
export const WORKSPACE_TOPIC = '%';
/** Seen entries kept, newest first; older ones read as seen. */
const SEEN_LIMIT = 1000;
const ENTRY_LIMIT = 1000;

const operations: Record<string, string[]> = {
  workspace_view: [],
  workspace_subscribe: [],
  workspace_open_project: ['path'],
  workspace_hide_project: ['path', 'hidden'],
  workspace_set_worktree: ['folder', 'main', 'branch', 'creation_id', 'identity'],
  workspace_forget_worktree: ['folder'],
  workspace_mark_seen: ['entries'],
  workspace_import: ['device', 'projects', 'hidden', 'worktrees', 'seen'],
  host_set_name: ['name'],
};
const optional = new Set(['creation_id', 'identity', 'op_id']);
for (const op of Object.keys(operations)) if (!['workspace_view', 'workspace_subscribe'].includes(op)) operations[op].push('op_id');

type Worktree = { folder: string; main: string; branch: string; creation_id: string | null; identity: { dev: string; ino: string } | null };
type Seen = { thread_id: string; updated_at: number; state: string; unread: boolean };

const id = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
const controls = /[\u0000-\u001f\u007f]/u;

function path(value: unknown, name = 'path'): string {
  if (typeof value !== 'string' || !isAbsolute(value) || Buffer.byteLength(value) > 4096 || controls.test(value)) fail('invalid_request', `Invalid ${name}.`);
  return value;
}
function text(value: unknown, name: string, limit = 256): string {
  if (typeof value !== 'string' || !value || Buffer.byteLength(value) > limit || controls.test(value)) fail('invalid_request', `Invalid ${name}.`);
  return value;
}
function list(value: unknown, name: string): unknown[] {
  if (!Array.isArray(value) || value.length > ENTRY_LIMIT) fail('invalid_request', `Invalid ${name}.`);
  return value;
}
function worktree(value: unknown): Worktree {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail('invalid_request', 'Invalid worktree.');
  const row = value as Record<string, unknown>;
  const identity = row.identity as { dev?: unknown; ino?: unknown } | null | undefined;
  const digits = (v: unknown) => typeof v === 'string' && /^[0-9]{1,20}$/.test(v);
  if (identity != null && (typeof identity !== 'object' || !digits(identity.dev) || !digits(identity.ino))) fail('invalid_request', 'Invalid worktree identity.');
  return {
    folder: path(row.folder, 'folder'), main: path(row.main, 'main'), branch: text(row.branch, 'branch'),
    creation_id: row.creation_id == null ? null : (id(row.creation_id) ? row.creation_id : fail('invalid_request', 'Invalid creation_id.')),
    identity: identity == null ? null : { dev: String(identity.dev), ino: String(identity.ino) },
  };
}
function seen(value: unknown): Seen {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail('invalid_request', 'Invalid seen entry.');
  const row = value as Record<string, unknown>;
  if (!id(row.thread_id) || !Number.isSafeInteger(row.updated_at) || Number(row.updated_at) < 0 || typeof row.unread !== 'boolean') fail('invalid_request', 'Invalid seen entry.');
  return { thread_id: row.thread_id, updated_at: Number(row.updated_at), state: text(row.state, 'state', 64), unread: row.unread };
}

export function validateWorkspace(input: unknown): Record<string, unknown> & { op: string } {
  if (!input || typeof input !== 'object' || Array.isArray(input)) fail('invalid_request', 'Invalid workspace request.');
  const request = input as Record<string, unknown> & { op: string };
  const fields = Object.hasOwn(operations, request.op) ? operations[request.op] : undefined;
  if (!fields || Object.keys(request).some(key => key !== 'op' && !fields.includes(key))
      || fields.some(key => !optional.has(key) && request[key] === undefined)
      || Buffer.byteLength(JSON.stringify(request)) > 65_000) fail('invalid_request', 'Unknown operation or field.');
  return request;
}

export class Workspace {
  private store: Store;
  private changed: () => void;
  private watcher?: CheckoutWatcher;
  setWatcher(watcher: CheckoutWatcher) { this.watcher = watcher; this.syncWatcher(); }
  private syncWatcher() {
    const view = this.view();
    this.watcher?.sync([...view.projects.filter(project => !project.hidden).map(project => project.path), ...view.worktrees.map(tree => tree.folder)]);
  }
  constructor(store: Store, changed: () => void) {
    this.store = store; this.changed = changed;
    store.db.exec(`
      CREATE TABLE IF NOT EXISTS workspace_projects(path TEXT PRIMARY KEY, hidden INTEGER NOT NULL DEFAULT 0, added_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS workspace_worktrees(folder TEXT PRIMARY KEY, main TEXT NOT NULL, branch TEXT NOT NULL, creation_id TEXT, identity TEXT);
      CREATE TABLE IF NOT EXISTS workspace_seen(thread_id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL, state TEXT NOT NULL, unread INTEGER NOT NULL);`);
  }
  handles(input: unknown) { return !!input && typeof input === 'object' && Object.hasOwn(operations, String((input as { op?: unknown }).op)); }

  /** This host's stable identity. The ID predates the workspace: turn review made it first. */
  identity() {
    let host = this.store.setting('review_host_id');
    if (!host) { host = randomUUID(); this.store.setSetting('review_host_id', host); }
    return { id: host, name: this.store.setting('host_name') ?? hostname(), platform: process.platform };
  }

  view() {
    const db = this.store.db;
    const projects = db.prepare('SELECT path,hidden FROM workspace_projects ORDER BY added_at,path').all()
      .map(row => ({ path: String(row.path), hidden: Number(row.hidden) === 1 }));
    const worktrees = db.prepare('SELECT * FROM workspace_worktrees ORDER BY folder').all().map(row => ({
      folder: String(row.folder), main: String(row.main), branch: String(row.branch),
      creation_id: row.creation_id == null ? null : String(row.creation_id),
      identity: row.identity == null ? null : JSON.parse(String(row.identity)),
    }));
    const seen = db.prepare('SELECT * FROM workspace_seen ORDER BY updated_at DESC, thread_id').all().map(row => ({
      thread_id: String(row.thread_id), updated_at: Number(row.updated_at), state: String(row.state), unread: Number(row.unread) === 1,
    }));
    return { schema: 1, host: this.identity(), revision: Number(this.store.setting('workspace_revision') ?? 0), projects, worktrees, seen, status: this.watcher?.status() ?? {} };
  }

  async handle(input: unknown) {
    const request = validateWorkspace(input);
    if (request.op === 'workspace_view' || request.op === 'workspace_subscribe') return this.view();
    const { op_id, ...inputWithoutId } = request;
    return once(this.store, op_id, request.op, inputWithoutId, () => this.writeRequest(request));
  }

  private async writeRequest(request: Record<string, unknown> & { op: string }) {
    switch (request.op) {
      case 'workspace_view': case 'workspace_subscribe': return this.view();
      case 'workspace_open_project': {
        // Only a Git checkout on this host can be opened, so a typo never becomes a project.
        const root = await checkout(path(request.path));
        this.write(() => this.store.db.prepare('INSERT INTO workspace_projects VALUES (?,0,?) ON CONFLICT(path) DO UPDATE SET hidden=0').run(root, Date.now()));
        return { path: root, revision: this.revision() };
      }
      case 'workspace_hide_project': {
        if (typeof request.hidden !== 'boolean') fail('invalid_request', 'Invalid hidden.');
        const target = path(request.path);
        this.write(() => this.store.db.prepare('INSERT INTO workspace_projects VALUES (?,?,?) ON CONFLICT(path) DO UPDATE SET hidden=excluded.hidden').run(target, request.hidden ? 1 : 0, Date.now()));
        return { revision: this.revision() };
      }
      case 'workspace_set_worktree': {
        const row = worktree(request);
        this.write(() => this.putWorktree(row));
        return { revision: this.revision() };
      }
      case 'workspace_forget_worktree': {
        const folder = path(request.folder, 'folder');
        this.write(() => this.store.db.prepare('DELETE FROM workspace_worktrees WHERE folder=?').run(folder));
        return { revision: this.revision() };
      }
      case 'workspace_mark_seen': {
        const entries = list(request.entries, 'entries').map(seen);
        this.write(() => { for (const entry of entries) this.putSeen(entry); this.pruneSeen(); });
        return { revision: this.revision() };
      }
      case 'workspace_import': return this.import(request);
      case 'host_set_name': {
        const name = text(request.name, 'name', 64);
        this.write(() => this.store.setSetting('host_name', name));
        return { host: this.identity(), revision: this.revision() };
      }
    }
    return fail('invalid_request', 'Unknown operation or field.');
  }

  /** One device's lists, merged once. A repeat from that device changes nothing. */
  private import(request: Record<string, unknown>) {
    if (!id(request.device)) fail('invalid_request', 'Invalid device.');
    const projects = list(request.projects, 'projects').map(p => path(p));
    const hidden = list(request.hidden, 'hidden').map(p => path(p));
    const worktrees = list(request.worktrees, 'worktrees').map(worktree);
    const entries = list(request.seen, 'seen').map(seen);
    const key = `workspace_import:${request.device}`;
    if (this.store.setting(key)) return { imported: false, revision: this.revision() };
    this.write(() => {
      const now = Date.now();
      const add = this.store.db.prepare('INSERT INTO workspace_projects VALUES (?,?,?) ON CONFLICT(path) DO NOTHING');
      projects.forEach((p, i) => add.run(p, 0, now + i));
      const hide = this.store.db.prepare('INSERT INTO workspace_projects VALUES (?,1,?) ON CONFLICT(path) DO UPDATE SET hidden=1');
      hidden.forEach((p, i) => hide.run(p, now + projects.length + i));
      for (const row of worktrees) this.putWorktree(row, false);
      for (const entry of entries) this.putSeen(entry, false);
      this.pruneSeen();
      this.store.setSetting(key, String(now));
    });
    return { imported: true, revision: this.revision() };
  }

  private putWorktree(row: Worktree, replace = true) {
    this.store.db.prepare(`INSERT INTO workspace_worktrees VALUES (?,?,?,?,?) ON CONFLICT(folder) DO ${replace ? 'UPDATE SET main=excluded.main,branch=excluded.branch,creation_id=excluded.creation_id,identity=excluded.identity' : 'NOTHING'}`)
      .run(row.folder, row.main, row.branch, row.creation_id, row.identity === null ? null : JSON.stringify(row.identity));
  }
  /** A newer observation wins; an older one never marks a thread unread again. */
  private putSeen(entry: Seen, replace = true) {
    this.store.db.prepare(`INSERT INTO workspace_seen VALUES (?,?,?,?) ON CONFLICT(thread_id) DO ${replace ? 'UPDATE SET updated_at=excluded.updated_at,state=excluded.state,unread=excluded.unread WHERE excluded.updated_at>=workspace_seen.updated_at' : 'NOTHING'}`)
      .run(entry.thread_id, entry.updated_at, entry.state, entry.unread ? 1 : 0);
  }
  private pruneSeen() {
    this.store.db.prepare('DELETE FROM workspace_seen WHERE thread_id NOT IN (SELECT thread_id FROM workspace_seen ORDER BY updated_at DESC, thread_id LIMIT ?)').run(SEEN_LIMIT);
  }
  /** A worktree this host made belongs to the project it was made from. */
  madeWorktree(folder: string, main: string, branch: string, creationId: string, identity: { dev: string; ino: string } | null) {
    this.write(() => this.putWorktree({ folder, main, branch, creation_id: creationId, identity }));
  }
  removedWorktree(folder: string) {
    this.write(() => this.store.db.prepare('DELETE FROM workspace_worktrees WHERE folder=?').run(folder));
  }
  private revision() { return Number(this.store.setting('workspace_revision') ?? 0); }
  /** Each change commits with a new revision, then tells subscribers. */
  private write(change: () => void) {
    this.store.db.exec('BEGIN IMMEDIATE');
    try {
      change();
      this.store.setSetting('workspace_revision', String(this.revision() + 1));
      this.store.db.exec('COMMIT');
    } catch (error) { this.store.db.exec('ROLLBACK'); throw error; }
    this.syncWatcher();
    this.changed();
  }
}
