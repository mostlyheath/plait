import { issueSummary } from './identity.ts';
import { LinearWriteJournal } from './writes.ts';
import type { DatabaseSync } from 'node:sqlite';
import type { FileIdentity } from '../safety.ts';
import {
  LINEAR_CACHE_MAX_DETAILS,
  LINEAR_CACHE_MAX_ISSUES_PER_TEAM,
  LINEAR_CACHE_MAX_PAGES,
  LINEAR_MAX_RECENT_WRITES,
} from './types.ts';
import type {
  LinearIssueDetail,
  LinearIssueSummary,
  LinearProjectBinding,
  LinearTeam,
  LinearThreadLink,
} from './types.ts';

export type CachedTeams = { teams: LinearTeam[]; cached_at: number; complete: boolean | null };
export type CachedIssuesPage = {
  issues: LinearIssueSummary[];
  has_more: boolean;
  next_cursor: string | null;
  cached_at: number;
};
export type CachedIssueDetail = { issue: LinearIssueDetail; cached_at: number };
export type ThreadLinksPage = { links: LinearThreadLink[]; next_cursor: string | null };

const TEAMS_META_KEY = 'teams';
const LINKS_PAGE_BYTE_BUDGET = 750_000;

/**
 * Additive SQLite persistence for Linear project bindings and bounded API caches.
 * Credentials are never stored here; they live exclusively in `$STORE/linear-auth.json`.
 */
export class LinearCache {
  private readonly db: DatabaseSync;
  private touchSeq = 0;
  readonly writes: LinearWriteJournal;

  constructor(db: DatabaseSync) {
    this.db = db;
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS linear_projects(
        canonical_root TEXT NOT NULL,
        root_dev TEXT NOT NULL,
        root_ino TEXT NOT NULL,
        workspace_id TEXT,
        team_id TEXT NOT NULL,
        team_key TEXT,
        team_name TEXT,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY(canonical_root, root_dev, root_ino)
      );
      CREATE TABLE IF NOT EXISTS linear_cache_meta(
        key TEXT PRIMARY KEY,
        cached_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS linear_cache_teams(
        team_id TEXT PRIMARY KEY,
        payload TEXT NOT NULL,
        cached_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS linear_cache_issues(
        team_id TEXT NOT NULL,
        issue_id TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        cached_at INTEGER NOT NULL,
        touched_seq INTEGER NOT NULL DEFAULT 0,
        payload TEXT NOT NULL,
        PRIMARY KEY(team_id, issue_id)
      );
      CREATE INDEX IF NOT EXISTS linear_cache_issues_team_recent
        ON linear_cache_issues(team_id, cached_at DESC, touched_seq DESC, updated_at DESC);
      CREATE TABLE IF NOT EXISTS linear_cache_pages(
        cache_key TEXT PRIMARY KEY,
        team_id TEXT NOT NULL,
        issue_ids TEXT NOT NULL,
        has_more INTEGER NOT NULL,
        next_cursor TEXT,
        cached_at INTEGER NOT NULL,
        accessed_at INTEGER NOT NULL,
        touched_seq INTEGER NOT NULL DEFAULT 0
      );
      CREATE INDEX IF NOT EXISTS linear_cache_pages_team ON linear_cache_pages(team_id);
      CREATE TABLE IF NOT EXISTS linear_cache_details(
        issue_id TEXT PRIMARY KEY,
        team_id TEXT NOT NULL,
        payload TEXT NOT NULL,
        cached_at INTEGER NOT NULL,
        accessed_at INTEGER NOT NULL,
        touched_seq INTEGER NOT NULL DEFAULT 0
      );
      CREATE TABLE IF NOT EXISTS linear_cache_aliases(
        workspace_id TEXT NOT NULL,
        team_id TEXT NOT NULL,
        identifier TEXT NOT NULL,
        issue_id TEXT NOT NULL,
        payload TEXT NOT NULL,
        cached_at INTEGER NOT NULL,
        stale INTEGER NOT NULL DEFAULT 0,
        touched_seq INTEGER NOT NULL,
        PRIMARY KEY(workspace_id, team_id, identifier)
      );
      CREATE TABLE IF NOT EXISTS linear_thread_links(
        thread_id TEXT PRIMARY KEY,
        issue_id TEXT NOT NULL,
        identifier TEXT NOT NULL,
        url TEXT NOT NULL,
        title TEXT NOT NULL,
        linked_at INTEGER NOT NULL
      );
      CREATE INDEX IF NOT EXISTS linear_thread_links_order
        ON linear_thread_links(linked_at DESC, thread_id ASC);
      CREATE TRIGGER IF NOT EXISTS linear_thread_links_delete_thread
        AFTER DELETE ON threads
      BEGIN
        DELETE FROM linear_thread_links WHERE thread_id = OLD.id;
      END;
      CREATE TABLE IF NOT EXISTS linear_writes(
        request_id TEXT PRIMARY KEY,
        op TEXT NOT NULL,
        hash TEXT NOT NULL,
        result TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        touched_seq INTEGER NOT NULL DEFAULT 0
      );
      CREATE INDEX IF NOT EXISTS linear_writes_recent ON linear_writes(created_at DESC);
      CREATE TABLE IF NOT EXISTS linear_cache_images(
        url TEXT PRIMARY KEY,
        credential TEXT NOT NULL,
        bytes BLOB NOT NULL,
        cached_at INTEGER NOT NULL,
        used_at INTEGER NOT NULL
      );
    `);
    for (const table of ['linear_cache_pages', 'linear_cache_details', 'linear_writes']) {
      if (!this.db.prepare(`PRAGMA table_info(${table})`).all().some(row => row.name === 'touched_seq')) {
        this.db.exec(`ALTER TABLE ${table} ADD COLUMN touched_seq INTEGER NOT NULL DEFAULT 0`);
      }
    }
    if (!this.db.prepare('PRAGMA table_info(linear_cache_meta)').all().some(row => row.name === 'complete')) {
      this.db.exec('ALTER TABLE linear_cache_meta ADD COLUMN complete INTEGER');
    }
    if (!this.db.prepare('PRAGMA table_info(linear_thread_links)').all().some(row => row.name === 'workspace_id')) {
      this.db.exec('ALTER TABLE linear_thread_links ADD COLUMN workspace_id TEXT');
    }
    if (!this.db.prepare('PRAGMA table_info(linear_cache_aliases)').all().some(row => row.name === 'stale')) {
      this.db.exec('ALTER TABLE linear_cache_aliases ADD COLUMN stale INTEGER NOT NULL DEFAULT 0');
    }
    const maxSeq = this.db.prepare(`
      SELECT MAX(seq) AS m FROM (
        SELECT MAX(touched_seq) AS seq FROM linear_cache_issues
        UNION ALL SELECT MAX(touched_seq) FROM linear_cache_pages
        UNION ALL SELECT MAX(touched_seq) FROM linear_cache_details
        UNION ALL SELECT MAX(touched_seq) FROM linear_writes
        UNION ALL SELECT MAX(touched_seq) FROM linear_cache_aliases
      )
    `).get();
    this.touchSeq = Number(maxSeq?.m ?? 0);
    this.writes = new LinearWriteJournal(db);
  }

  private transaction<T>(work: () => T): T {
    this.db.exec('BEGIN IMMEDIATE');
    try {
      const result = work();
      this.db.exec('COMMIT');
      return result;
    } catch (error) {
      this.db.exec('ROLLBACK');
      throw error;
    }
  }

  getProjectBinding(root: string, identity: FileIdentity): LinearProjectBinding | null {
    const row = this.db.prepare(
      'SELECT workspace_id, team_id, team_key, team_name, updated_at FROM linear_projects WHERE canonical_root=? AND root_dev=? AND root_ino=?',
    ).get(root, identity.dev, identity.ino);
    if (!row) return null;
    return {
      workspace_id: typeof row.workspace_id === 'string' ? row.workspace_id : null,
      team_id: String(row.team_id),
      team_key: typeof row.team_key === 'string' ? row.team_key : null,
      team_name: typeof row.team_name === 'string' ? row.team_name : null,
      updated_at: Number(row.updated_at),
    };
  }

  setProjectBinding(root: string, identity: FileIdentity, binding: LinearProjectBinding | null): void {
    if (!binding) {
      this.db.prepare('DELETE FROM linear_projects WHERE canonical_root=? AND root_dev=? AND root_ino=?')
        .run(root, identity.dev, identity.ino);
      return;
    }
    this.db.prepare(`
      INSERT INTO linear_projects(canonical_root, root_dev, root_ino, workspace_id, team_id, team_key, team_name, updated_at)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(canonical_root, root_dev, root_ino) DO UPDATE SET
        workspace_id=excluded.workspace_id,
        team_id=excluded.team_id,
        team_key=excluded.team_key,
        team_name=excluded.team_name,
        updated_at=excluded.updated_at
    `).run(root, identity.dev, identity.ino, binding.workspace_id, binding.team_id, binding.team_key, binding.team_name, binding.updated_at);
  }

  getTeams(): CachedTeams | null {
    const meta = this.db.prepare('SELECT cached_at, complete FROM linear_cache_meta WHERE key=?').get(TEAMS_META_KEY);
    if (!meta) return null;
    const rows = this.db.prepare('SELECT payload FROM linear_cache_teams ORDER BY rowid ASC').all();
    return {
      teams: rows.map(row => JSON.parse(String(row.payload)) as LinearTeam),
      cached_at: Number(meta.cached_at),
      complete: meta.complete === null ? null : meta.complete === 1,
    };
  }

  getTeam(teamId: string): LinearTeam | null {
    const row = this.db.prepare('SELECT payload FROM linear_cache_teams WHERE team_id=?').get(teamId);
    return row ? JSON.parse(String(row.payload)) as LinearTeam : null;
  }

  saveTeams(teams: LinearTeam[], cachedAt: number, complete: boolean | null = null): void {
    this.transaction(() => {
      this.db.prepare('DELETE FROM linear_cache_teams').run();
      const insert = this.db.prepare(`
        INSERT INTO linear_cache_teams(team_id, payload, cached_at)
        VALUES (?, ?, ?)
        ON CONFLICT(team_id) DO UPDATE SET payload=excluded.payload, cached_at=excluded.cached_at
      `);
      for (const team of teams) insert.run(team.id, JSON.stringify(team), cachedAt);
      this.db.prepare('INSERT INTO linear_cache_meta(key, cached_at, complete) VALUES (?, ?, ?) ON CONFLICT(key) DO UPDATE SET cached_at=excluded.cached_at, complete=excluded.complete')
        .run(TEAMS_META_KEY, cachedAt, complete === null ? null : complete ? 1 : 0);
    });
  }

  getIssuesPage(cacheKey: string, now: number): CachedIssuesPage | null {
    const page = this.db.prepare(
      'SELECT team_id, issue_ids, has_more, next_cursor, cached_at FROM linear_cache_pages WHERE cache_key=?',
    ).get(cacheKey);
    if (!page) return null;
    const teamId = String(page.team_id);
    const idsJson = String(page.issue_ids);
    const ids = JSON.parse(idsJson) as string[];
    const issues: LinearIssueSummary[] = [];
    if (ids.length > 0) {
      const rows = this.db.prepare(
        'SELECT issue_id, payload FROM linear_cache_issues WHERE team_id=? AND issue_id IN (SELECT value FROM json_each(?))',
      ).all(teamId, idsJson);
      const byId = new Map<string, LinearIssueSummary>();
      for (const row of rows) {
        byId.set(String(row.issue_id), JSON.parse(String(row.payload)) as LinearIssueSummary);
      }
      for (const issueId of ids) {
        const issue = byId.get(issueId);
        // If any issue in the cached page was evicted by the per-team cap, do not serve a holey page.
        if (!issue) return null;
        issues.push(issue);
      }
    }
    this.db.prepare('UPDATE linear_cache_pages SET accessed_at=?, touched_seq=? WHERE cache_key=?').run(now, ++this.touchSeq, cacheKey);
    return {
      issues,
      has_more: page.has_more === 1,
      next_cursor: typeof page.next_cursor === 'string' ? page.next_cursor : null,
      cached_at: Number(page.cached_at),
    };
  }

  saveIssuesPage(
    cacheKey: string,
    teamId: string,
    issues: LinearIssueSummary[],
    hasMore: boolean,
    nextCursor: string | null,
    cachedAt: number,
    workspaceId?: string,
  ): void {
    this.transaction(() => {
      const seq = ++this.touchSeq;
      const upsertIssue = this.db.prepare(`
        INSERT INTO linear_cache_issues(team_id, issue_id, updated_at, cached_at, touched_seq, payload)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(team_id, issue_id) DO UPDATE SET
          updated_at=excluded.updated_at,
          cached_at=excluded.cached_at,
          touched_seq=excluded.touched_seq,
          payload=excluded.payload
      `);
      for (const issue of issues) {
        upsertIssue.run(teamId, issue.id, issue.updated_at, cachedAt, seq, JSON.stringify(issue));
        if (workspaceId) this.saveIssueAlias(workspaceId, issue, cachedAt);
      }
      // Strictly bound cached summaries to LINEAR_CACHE_MAX_ISSUES_PER_TEAM per team.
      this.db.prepare(`
        DELETE FROM linear_cache_issues
        WHERE team_id=? AND issue_id NOT IN (
          SELECT issue_id FROM linear_cache_issues
          WHERE team_id=?
          ORDER BY cached_at DESC, touched_seq DESC, updated_at DESC, issue_id ASC
          LIMIT ?
        )
      `).run(teamId, teamId, LINEAR_CACHE_MAX_ISSUES_PER_TEAM);

      this.db.prepare(`
        INSERT INTO linear_cache_pages(cache_key, team_id, issue_ids, has_more, next_cursor, cached_at, accessed_at, touched_seq)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(cache_key) DO UPDATE SET
          team_id=excluded.team_id,
          issue_ids=excluded.issue_ids,
          has_more=excluded.has_more,
          next_cursor=excluded.next_cursor,
          cached_at=excluded.cached_at,
          accessed_at=excluded.accessed_at,
          touched_seq=excluded.touched_seq
      `).run(cacheKey, teamId, JSON.stringify(issues.map(issue => issue.id)), hasMore ? 1 : 0, nextCursor, cachedAt, cachedAt, seq);

      this.db.prepare(`
        DELETE FROM linear_cache_pages
        WHERE cache_key NOT IN (
          SELECT cache_key FROM linear_cache_pages ORDER BY accessed_at DESC, touched_seq DESC, rowid DESC LIMIT ?
        )
      `).run(LINEAR_CACHE_MAX_PAGES);
    });
  }

  getIssueDetail(issueId: string, now: number): CachedIssueDetail | null {
    const row = this.db.prepare('SELECT payload, cached_at FROM linear_cache_details WHERE issue_id=?').get(issueId);
    if (!row) return null;
    this.db.prepare('UPDATE linear_cache_details SET accessed_at=?, touched_seq=? WHERE issue_id=?').run(now, ++this.touchSeq, issueId);
    return {
      issue: JSON.parse(String(row.payload)) as LinearIssueDetail,
      cached_at: Number(row.cached_at),
    };
  }

  saveIssueDetail(issue: LinearIssueDetail, cachedAt: number, workspaceId?: string): void {
    this.transaction(() => {
      if (workspaceId) this.saveIssueAlias(workspaceId, issueSummary(issue), cachedAt);
      const seq = ++this.touchSeq;
      this.db.prepare(`
        INSERT INTO linear_cache_details(issue_id, team_id, payload, cached_at, accessed_at, touched_seq)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(issue_id) DO UPDATE SET
          team_id=excluded.team_id,
          payload=excluded.payload,
          cached_at=excluded.cached_at,
          accessed_at=excluded.accessed_at,
          touched_seq=excluded.touched_seq
      `).run(issue.id, issue.team_id, JSON.stringify(issue), cachedAt, cachedAt, seq);

      // Keep LRU cap of LINEAR_CACHE_MAX_DETAILS issue details.
      this.db.prepare(`
        DELETE FROM linear_cache_details
        WHERE issue_id NOT IN (
          SELECT issue_id FROM linear_cache_details ORDER BY accessed_at DESC, touched_seq DESC, rowid DESC LIMIT ?
        )
      `).run(LINEAR_CACHE_MAX_DETAILS);

      // Keep matching summary row fresh if the issue belongs to a team.
      if (issue.team_id) {
        const summary: LinearIssueSummary = {
          id: issue.id,
          identifier: issue.identifier,
          title: issue.title,
          url: issue.url,
          priority: issue.priority,
          updated_at: issue.updated_at,
          team_id: issue.team_id,
          state: issue.state,
          assignee: issue.assignee,
          labels: issue.labels,
        };
        this.db.prepare(`
          UPDATE linear_cache_issues
          SET updated_at=?, cached_at=?, payload=?
          WHERE team_id=? AND issue_id=?
        `).run(summary.updated_at, cachedAt, JSON.stringify(summary), summary.team_id, summary.id);
      }
    });
  }

  getIssueAlias(workspaceId: string, teamId: string, identifier: string): { issue: LinearIssueSummary; cached_at: number; stale: boolean } | null {
    const row = this.db.prepare(`SELECT payload, cached_at, stale FROM linear_cache_aliases
      WHERE workspace_id=? AND team_id=? AND identifier=?`).get(workspaceId, teamId, identifier);
    return row ? { issue: JSON.parse(String(row.payload)), cached_at: Number(row.cached_at), stale: row.stale === 1 } : null;
  }

  saveIssueAlias(workspaceId: string, issue: LinearIssueSummary, cachedAt: number): void {
    this.db.prepare(`INSERT INTO linear_cache_aliases
      (workspace_id, team_id, identifier, issue_id, payload, cached_at, stale, touched_seq) VALUES (?, ?, ?, ?, ?, ?, 0, ?)
      ON CONFLICT(workspace_id, team_id, identifier) DO UPDATE SET issue_id=excluded.issue_id,
        payload=excluded.payload, cached_at=excluded.cached_at, stale=0, touched_seq=excluded.touched_seq`)
      .run(workspaceId, issue.team_id, issue.identifier, issue.id, JSON.stringify(issue), cachedAt, ++this.touchSeq);
    this.db.prepare(`DELETE FROM linear_cache_aliases WHERE workspace_id=? AND team_id=? AND identifier NOT IN (
      SELECT identifier FROM linear_cache_aliases WHERE workspace_id=? AND team_id=?
      ORDER BY cached_at DESC, touched_seq DESC LIMIT ?)`)
      .run(workspaceId, issue.team_id, workspaceId, issue.team_id, LINEAR_CACHE_MAX_ISSUES_PER_TEAM);
  }

  /** Looks up the cached team_id for an issue, checking detail cache then summary cache. */
  findIssueTeamId(issueId: string): string | null {
    const detail = this.db.prepare('SELECT team_id FROM linear_cache_details WHERE issue_id=?').get(issueId);
    if (detail && typeof detail.team_id === 'string' && detail.team_id) return detail.team_id;
    const summary = this.db.prepare('SELECT team_id FROM linear_cache_issues WHERE issue_id=? LIMIT 1').get(issueId);
    if (summary && typeof summary.team_id === 'string' && summary.team_id) return summary.team_id;
    return null;
  }

  /** Invalidates cached detail for an issue and cached pages for its team. */
  invalidateIssue(issueId: string, teamId?: string | null): void {
    this.transaction(() => {
      const resolvedTeam = teamId ?? this.findIssueTeamId(issueId);
      this.db.prepare('DELETE FROM linear_cache_details WHERE issue_id=?').run(issueId);
      // Keep exact identity and fetch timestamp for offline lookup, marking it stale until refreshed.
      this.db.prepare('UPDATE linear_cache_aliases SET stale=1 WHERE issue_id=?').run(issueId);
      if (resolvedTeam) {
        this.db.prepare('DELETE FROM linear_cache_pages WHERE team_id=?').run(resolvedTeam);
      } else {
        this.db.prepare('DELETE FROM linear_cache_pages').run();
      }
    });
  }

  /** Clears cached responses and full write receipts, retaining permanent retry identities (and optionally project bindings). */
  clearAll(options: { clearProjects?: boolean } = {}): void {
    this.transaction(() => {
      if (options.clearProjects) {
        this.db.prepare('DELETE FROM linear_projects').run();
      }
      this.db.prepare('DELETE FROM linear_cache_meta').run();
      this.db.prepare('DELETE FROM linear_cache_teams').run();
      this.db.prepare('DELETE FROM linear_cache_issues').run();
      this.db.prepare('DELETE FROM linear_cache_pages').run();
      this.db.prepare('DELETE FROM linear_cache_details').run();
      this.db.prepare('DELETE FROM linear_cache_aliases').run();
      this.db.prepare('DELETE FROM linear_writes').run();
      this.db.prepare('DELETE FROM linear_cache_images').run();
    });
  }

  /** A cached image's bytes from `offset`, at most `length` of them, or null.
   * Only an entry read with `credential`, the current sign-in, is returned. */
  readImage(url: string, credential: string, offset: number, length: number, now: number): { size: number; chunk: Buffer } | null {
    const row = this.db.prepare(
      'SELECT length(bytes) AS size, substr(bytes,?,?) AS chunk FROM linear_cache_images WHERE url=? AND credential=?',
    ).get(offset + 1, length, url, credential);
    if (!row) return null;
    this.db.prepare('UPDATE linear_cache_images SET used_at=? WHERE url=?').run(now, url);
    return { size: Number(row.size), chunk: Buffer.from(row.chunk as Uint8Array) };
  }

  /** Keeps at most `count` images and `bytes` in all, forgetting the least recently read. */
  saveImage(url: string, credential: string, bytes: Buffer, now: number, count: number, total: number): void {
    this.transaction(() => {
      // Another sign-in's images are never kept beside this one's.
      this.db.prepare('DELETE FROM linear_cache_images WHERE credential<>?').run(credential);
      this.db.prepare(`INSERT INTO linear_cache_images(url, credential, bytes, cached_at, used_at) VALUES(?,?,?,?,?)
        ON CONFLICT(url) DO UPDATE SET credential=excluded.credential, bytes=excluded.bytes,
          cached_at=excluded.cached_at, used_at=excluded.used_at`)
        .run(url, credential, bytes, now, now);
      const rows = this.db.prepare('SELECT url, length(bytes) AS size FROM linear_cache_images ORDER BY used_at DESC, url').all();
      let kept = 0; let used = 0;
      for (const row of rows) {
        const size = Number(row.size);
        if (kept < count && used + size <= total) { kept += 1; used += size; continue; }
        this.db.prepare('DELETE FROM linear_cache_images WHERE url=?').run(String(row.url));
      }
    });
  }

  getWrite(requestId: string): { op: string; hash: string; result: unknown } | null {
    const row = this.db.prepare('SELECT op, hash, result FROM linear_writes WHERE request_id=?').get(requestId);
    if (!row) return null;
    return { op: String(row.op), hash: String(row.hash), result: JSON.parse(String(row.result)) };
  }

  saveWrite(requestId: string, op: string, hash: string, result: unknown, createdAt: number): void {
    this.transaction(() => {
      const seq = ++this.touchSeq;
      this.db.prepare(`
        INSERT INTO linear_writes(request_id, op, hash, result, created_at, touched_seq)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(request_id) DO UPDATE SET
          result=excluded.result,
          created_at=excluded.created_at,
          touched_seq=excluded.touched_seq
      `).run(requestId, op, hash, JSON.stringify(result), createdAt, seq);

      this.db.prepare(`
        DELETE FROM linear_writes
        WHERE request_id NOT IN (
          SELECT request_id FROM linear_writes ORDER BY created_at DESC, touched_seq DESC, rowid DESC LIMIT ?
        )
      `).run(LINEAR_MAX_RECENT_WRITES);
    });
  }

  hasThread(threadId: string): boolean {
    return !!this.db.prepare('SELECT 1 FROM threads WHERE id=?').get(threadId);
  }

  getThreadLink(threadId: string): LinearThreadLink | null {
    const row = this.db.prepare(
      'SELECT thread_id, workspace_id, issue_id, identifier, url, title, linked_at FROM linear_thread_links WHERE thread_id=?',
    ).get(threadId);
    return row ? rowToThreadLink(row) : null;
  }

  setThreadLink(link: LinearThreadLink): LinearThreadLink {
    this.db.prepare(`
      INSERT INTO linear_thread_links(thread_id, workspace_id, issue_id, identifier, url, title, linked_at)
      VALUES (?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(thread_id) DO UPDATE SET
        workspace_id=excluded.workspace_id,
        issue_id=excluded.issue_id,
        identifier=excluded.identifier,
        url=excluded.url,
        title=excluded.title,
        linked_at=excluded.linked_at
    `).run(link.thread_id, link.workspace_id, link.issue_id, link.identifier, link.url, link.title, link.linked_at);
    return link;
  }

  removeThreadLink(threadId: string): boolean {
    const result = this.db.prepare('DELETE FROM linear_thread_links WHERE thread_id=?').run(threadId);
    return Number(result.changes) > 0;
  }

  /** Bulk-lists persisted thread links in stable order (linked_at DESC, thread_id ASC), bounded by count and frame bytes. */
  listThreadLinks(limit: number, cursor?: { linkedAt: number; threadId: string }): ThreadLinksPage {
    const rows = cursor
      ? this.db.prepare(`
          SELECT thread_id, workspace_id, issue_id, identifier, url, title, linked_at
          FROM linear_thread_links
          WHERE linked_at < ? OR (linked_at = ? AND thread_id > ?)
          ORDER BY linked_at DESC, thread_id ASC
          LIMIT ?
        `).all(cursor.linkedAt, cursor.linkedAt, cursor.threadId, limit + 1)
      : this.db.prepare(`
          SELECT thread_id, workspace_id, issue_id, identifier, url, title, linked_at
          FROM linear_thread_links
          ORDER BY linked_at DESC, thread_id ASC
          LIMIT ?
        `).all(limit + 1);

    const links: LinearThreadLink[] = [];
    let bytes = 1024;
    let truncatedByBytes = false;
    for (const row of rows.slice(0, limit)) {
      const link = rowToThreadLink(row);
      // Reserve the service's historical field before enforcing the page byte budget.
      const itemBytes = Buffer.byteLength(JSON.stringify(link)) + 32;
      if (links.length > 0 && bytes + itemBytes > LINKS_PAGE_BYTE_BUDGET) {
        truncatedByBytes = true;
        break;
      }
      links.push(link);
      bytes += itemBytes;
    }
    const hasMore = truncatedByBytes || rows.length > limit;
    const last = links.at(-1);
    return {
      links,
      next_cursor: hasMore && last ? `${last.linked_at}:${last.thread_id}` : null,
    };
  }
}

function rowToThreadLink(row: Record<string, unknown>): LinearThreadLink {
  return {
    thread_id: String(row.thread_id),
    workspace_id: typeof row.workspace_id === 'string' ? row.workspace_id : null,
    issue_id: String(row.issue_id),
    identifier: String(row.identifier),
    url: String(row.url),
    title: String(row.title),
    linked_at: Number(row.linked_at),
  };
}
