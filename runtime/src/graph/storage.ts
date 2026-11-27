import { DatabaseSync } from 'node:sqlite';
import { join } from 'node:path';
import type { Start } from '../admission.ts';

export type Node = {
  id: string; project: string; thread: string | null; title: string; kind: 'build' | 'discuss';
  auto: number; started: number; requested: number; reason: string | null; done: number;
  project_identity: string | null; identity: string | null; observed: string | null; summary: string | null; settings: string | null; prompt: string; submission: string | null;
};
export class Storage {
  db: DatabaseSync;
  constructor(directory: string) {
    this.db = new DatabaseSync(join(directory, 'graph.sqlite'));
    this.db.exec(`PRAGMA foreign_keys=ON;
      CREATE TABLE IF NOT EXISTS version(revision INTEGER NOT NULL);
      INSERT INTO version SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM version);
      CREATE TABLE IF NOT EXISTS nodes(id TEXT PRIMARY KEY, project TEXT NOT NULL, thread TEXT UNIQUE,
        title TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'build', auto INTEGER NOT NULL DEFAULT 0,
        started INTEGER NOT NULL DEFAULT 0, requested INTEGER NOT NULL DEFAULT 0, reason TEXT,
        done INTEGER NOT NULL DEFAULT 0, settings TEXT, prompt TEXT NOT NULL DEFAULT '', submission TEXT,
        observed TEXT, summary TEXT, op_id TEXT UNIQUE, payload TEXT, deleted INTEGER NOT NULL DEFAULT 0);
      CREATE INDEX IF NOT EXISTS nodes_by_project ON nodes(project);
      CREATE TABLE IF NOT EXISTS pages(project TEXT PRIMARY KEY,summary TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS edges(src TEXT REFERENCES nodes(id),dst TEXT REFERENCES nodes(id),
        kind TEXT NOT NULL, PRIMARY KEY(src,dst,kind));
      CREATE INDEX IF NOT EXISTS incoming ON edges(dst,kind);
      CREATE TRIGGER IF NOT EXISTS node_insert AFTER INSERT ON nodes BEGIN UPDATE version SET revision=revision+1; END;
      CREATE TRIGGER IF NOT EXISTS node_update AFTER UPDATE ON nodes BEGIN UPDATE version SET revision=revision+1; END;
      CREATE TRIGGER IF NOT EXISTS link_add AFTER INSERT ON edges BEGIN UPDATE version SET revision=revision+1; END;
      CREATE TRIGGER IF NOT EXISTS link_remove AFTER DELETE ON edges BEGIN UPDATE version SET revision=revision+1; END;
      CREATE TRIGGER IF NOT EXISTS page_insert AFTER INSERT ON pages BEGIN UPDATE version SET revision=revision+1; END;
      CREATE TRIGGER IF NOT EXISTS page_update AFTER UPDATE ON pages BEGIN UPDATE version SET revision=revision+1; END;`);
    // The older runtime keys its three-field signatures by canonical path. Evict
    // those entries so its first poll after rollback also advances the revision.
    this.transaction(() => {
      const legacy = this.db.prepare('DELETE FROM pages WHERE json_array_length(summary)=3').run();
      if (legacy.changes) this.db.exec('UPDATE version SET revision=revision+1');
    });
    if (!this.db.prepare("SELECT 1 FROM pragma_table_info('nodes') WHERE name='identity'").get())
      this.db.exec('ALTER TABLE nodes ADD COLUMN identity TEXT');
    if (!this.db.prepare("SELECT 1 FROM pragma_table_info('nodes') WHERE name='project_identity'").get())
      this.transaction(() => this.db.exec(`ALTER TABLE nodes ADD COLUMN project_identity TEXT;
        UPDATE nodes SET project_identity=identity WHERE project=json_extract(settings,'$.checkout')`));
  }
  batchDependencies(ids: string[]) {
    const rows = this.db.prepare(`SELECT e.dst,n.id,n.title,n.started,n.thread,n.kind,n.reason,n.done,n.summary
      FROM edges e JOIN nodes n ON n.id=e.src WHERE e.dst IN (SELECT value FROM json_each(?))
      AND e.kind='depends_on' ORDER BY e.dst,n.rowid`).all(JSON.stringify(ids)) as (Fact & { dst: string })[];
    const groups = new Map<string, Fact[]>();
    for (const row of rows) {
      const group = groups.get(row.dst);
      if (group) group.push(row);
      else groups.set(row.dst, [row]);
    }
    return groups;
  }
  transaction<T>(fn: () => T) {
    this.db.exec('BEGIN IMMEDIATE');
    try { const result = fn(); this.db.exec('COMMIT'); return result; }
    catch (err) { this.db.exec('ROLLBACK'); throw err; }
  }
  updatePageIdentity(path: string, project: string, identity: string) {
    // The existing page-update trigger advances the revision for waiting clients.
    this.db.prepare(`UPDATE pages SET summary=json_set(summary,'$[0]',?,'$[1]',?)
      WHERE project=? AND (json_extract(summary,'$[0]') IS NOT ? OR json_extract(summary,'$[1]') IS NOT ?)`)
      .run(project, identity, path, project, identity);
  }
  revision() { return Number(this.db.prepare('SELECT revision FROM version').get()!.revision); }
  node(id: string) {
    return this.db.prepare('SELECT * FROM nodes WHERE deleted=0 AND (id=? OR thread=?)').get(id, id) as Node | undefined;
  }
  dependencies(id: string, project: string, identity: string) {
    return this.db.prepare(`SELECT n.id,n.title,n.started,n.thread,n.kind,n.reason,n.done,n.summary
      FROM edges e JOIN nodes n ON n.id=e.src WHERE e.dst=? AND e.kind='depends_on' AND n.project=? AND n.project_identity=? ORDER BY n.rowid`).all(id, project, identity) as Fact[];
  }
  resultChildren(id: string) {
    return this.db.prepare(`SELECT n.thread, count(*) OVER () AS total FROM edges e JOIN nodes n ON n.id=e.dst
      WHERE e.src=? AND e.kind='fork' AND n.done ORDER BY n.rowid LIMIT 20`).all(id) as { thread: string; total: number }[];
  }
}
export function settings(node: Node): Start { return JSON.parse(node.settings!); }

export type Fact = Pick<Node, 'id' | 'title' | 'started' | 'thread' | 'kind' | 'reason' | 'done' | 'summary'>;
