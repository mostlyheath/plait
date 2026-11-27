import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, mkdir, readFile, realpath, rename, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import { promisify } from 'node:util';
import { createModels } from '@earendil-works/pi-ai/models';
import { LinearService } from '../src/linear/service.ts';
import {
  LINEAR_CACHE_DETAIL_TTL_MS,
  LINEAR_CACHE_ISSUES_TTL_MS,
  LINEAR_CACHE_MAX_DETAILS,
  LINEAR_CACHE_MAX_ISSUES_PER_TEAM,
  LINEAR_CACHE_TEAMS_TTL_MS,
} from '../src/linear/types.ts';
import { Runtime } from '../src/runtime.ts';
import { Store } from '../src/store.ts';
import { FakeLinearServer } from './support/linear-server.ts';
import type { FakeIssue } from './support/linear-server.ts';

const execFileAsync = promisify(execFile);

async function makeRepo(parent: string, name: string): Promise<string> {
  const repo = join(parent, name);
  await mkdir(repo, { recursive: true });
  await execFileAsync('git', ['init', '-q', repo]);
  return realpath(repo);
}

async function setupHarness(t: test.TestContext) {
  const dir = await mkdtemp(join(tmpdir(), 'plait-linear-reads-'));
  const storeDir = join(dir, 'store');
  await mkdir(storeDir, { recursive: true, mode: 0o700 });
  const fake = new FakeLinearServer();
  const endpoint = await fake.start();
  let now = 1_763_300_000_000;
  const runtime = new Runtime(storeDir, createModels(), new Set());
  runtime.linear = new LinearService(runtime.store, storeDir, { endpoint, timeoutMs: 1000, now: () => now });
  t.after(async () => {
    await runtime.close();
    await fake.close();
    await rm(dir, { recursive: true, force: true });
  });
  const transcript: string[] = [];
  const call = async (input: unknown): Promise<any> => {
    const reply = await runtime.reply(input);
    transcript.push(JSON.stringify(reply));
    return reply;
  };
  return {
    dir, storeDir, fake, runtime, call, transcript,
    get now() { return now; },
    advance(ms: number) { now += ms; },
  };
}

async function assertKeyNotInStoreDb(storeDir: string, secret: string) {
  for (const suffix of ['', '-wal', '-shm']) {
    const file = join(storeDir, `host.sqlite${suffix}`);
    try {
      const bytes = await readFile(file);
      assert(!bytes.includes(secret), `${file} must never contain the Linear API key`);
    } catch (error) {
      if ((error as { code?: string }).code !== 'ENOENT') throw error;
    }
  }
}

test('Linear sign-in verifies key before storing in 0600 file, never in host.sqlite, and sign-out deletes it', async t => {
  const h = await setupHarness(t);

  // Initially signed out, no network calls made during startup.
  assert.equal(h.fake.requests.length, 0);
  const initial = await h.call({ op: 'linear_auth_status' });
  assert.equal(initial.ok, true);
  assert.deepEqual(initial.result, {
    signed_in: false, status: 'signed_out',
    workspace_id: null, workspace_name: null, workspace_url_key: null,
    viewer_id: null, viewer_name: null,
  });

  // Reading before sign-in fails cleanly.
  const unsigned = await h.call({ op: 'linear_list_teams' });
  assert.equal(unsigned.ok, false);
  assert.equal(unsigned.error.code, 'linear_not_signed_in');

  // Rejected key is not stored.
  const bad = await h.call({ op: 'linear_set_key', key: 'lin_api_BAD_KEY_SECRET_000' });
  assert.equal(bad.ok, false);
  assert.equal(bad.error.code, 'linear_key_rejected');
  await assert.rejects(() => stat(join(h.storeDir, 'linear-auth.json')), { code: 'ENOENT' });

  // Good key verifies, writes 0600 file, and never enters host.sqlite or replies.
  const good = await h.call({ op: 'linear_set_key', key: `  ${h.fake.validKey}\n` });
  assert.equal(good.ok, true);
  assert.deepEqual(good.result, {
    signed_in: true, status: 'signed_in',
    workspace_id: 'org-1', workspace_name: 'Acme Workspace', workspace_url_key: 'acme',
    viewer_id: 'viewer-1', viewer_name: 'Alex Tester',
  });
  const mode = (await stat(join(h.storeDir, 'linear-auth.json'))).mode & 0o777;
  assert.equal(mode, 0o600);
  assert.equal((await h.call({ op: 'linear_auth_status' })).result.workspace_name, 'Acme Workspace');

  // Populate cache, then verify sign-out deletes file and clears cache.
  assert.equal((await h.call({ op: 'linear_list_teams' })).ok, true);
  assert.equal((await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' })).ok, true);
  assert.equal((await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' })).ok, true);

  await assertKeyNotInStoreDb(h.storeDir, h.fake.validKey);
  await assertKeyNotInStoreDb(h.storeDir, 'lin_api_BAD_KEY_SECRET_000');

  const out = await h.call({ op: 'linear_sign_out' });
  assert.equal(out.ok, true);
  assert.equal(out.result.signed_in, false);
  assert.equal(out.result.status, 'signed_out');
  await assert.rejects(() => stat(join(h.storeDir, 'linear-auth.json')), { code: 'ENOENT' });
  assert.equal(h.runtime.store.linear.getTeams(), null);
  assert.equal(h.runtime.store.linear.getIssueDetail('issue-142', h.now), null);

  const joined = h.transcript.join('\n');
  assert(!joined.includes(h.fake.validKey), 'valid key must never appear in any reply');
  assert(!joined.includes('lin_api_BAD_KEY_SECRET_000'), 'bad key must never appear in any reply');
});

test('teams, project binding, and offline cache fallback work with TTLs and checkout identity checks', async t => {
  const h = await setupHarness(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });

  // First teams call hits Linear and caches for 15 min.
  const first = await h.call({ op: 'linear_list_teams' });
  assert.equal(first.ok, true);
  assert.equal(first.result.cached, false);
  assert.equal(first.result.teams.length, 2);
  assert.equal(first.result.teams[0].key, 'PLA');
  assert.equal(first.result.teams[0].states.length, 5);

  const reqCount = h.fake.requests.length;
  h.advance(60_000);
  const second = await h.call({ op: 'linear_list_teams' });
  assert.equal(second.ok, true);
  assert.equal(second.result.cached, true);
  assert.equal(second.result.stale, false);
  assert.equal(second.result.cache_age_seconds, 60);
  assert.equal(h.fake.requests.length, reqCount, 'fresh cache must not hit Linear');

  // Bind a real Git checkout to team-pla and read back.
  const repo = await makeRepo(h.dir, 'project-a');
  assert.equal((await h.call({ op: 'linear_get_project_binding', checkout: repo })).result.binding, null);
  const bound = await h.call({ op: 'linear_bind_project', checkout: repo, team_id: 'team-pla' });
  assert.equal(bound.ok, true);
  assert.equal(bound.result.checkout, repo);
  assert.equal(bound.result.binding.team_id, 'team-pla');
  assert.equal(bound.result.binding.team_key, 'PLA');
  assert.equal(bound.result.binding.team_name, 'Plait');
  assert.equal(bound.result.binding.workspace_id, 'org-1');

  // Replacing the checkout directory at the same path does not inherit the old binding.
  // The old directory is moved aside, not removed, so its inode stays in use: a
  // filesystem may hand a removed directory's inode straight to the next one.
  await rename(repo, join(h.dir, 'project-a-old'));
  const replaced = await makeRepo(h.dir, 'project-a');
  assert.equal((await h.call({ op: 'linear_get_project_binding', checkout: replaced })).result.binding, null);
  await h.call({ op: 'linear_bind_project', checkout: replaced, team_id: 'team-ops' });
  assert.equal((await h.call({ op: 'linear_get_project_binding', checkout: replaced })).result.binding.team_id, 'team-ops');
  assert.equal((await h.call({ op: 'linear_bind_project', checkout: replaced, team_id: null })).result.binding, null);

  // Expire teams cache and simulate offline -> serves stale cache with age.
  h.advance(LINEAR_CACHE_TEAMS_TTL_MS + 30_000);
  h.fake.enqueueFault((_req, res) => { res.writeHead(503); res.end(); return true; });
  const offline = await h.call({ op: 'linear_list_teams' });
  assert.equal(offline.ok, true);
  assert.equal(offline.result.cached, true);
  assert.equal(offline.result.stale, true);
  assert.equal(offline.result.cache_age_seconds, Math.floor((LINEAR_CACHE_TEAMS_TTL_MS + 90_000) / 1000));
  assert.equal(offline.result.teams.length, 2);
});

test('issue listing pages across 3 pages, respects filters/search, never fetches descriptions/comments, and caps cache at 500/team', async t => {
  const h = await setupHarness(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });

  // Populate 560 active issues in team-pla (exceeds 500/team cap and spans 12 pages of 50).
  const many: FakeIssue[] = [];
  for (let i = 1; i <= 560; i++) {
    many.push({
      id: `issue-bulk-${i}`,
      identifier: `PLA-${1000 + i}`,
      number: 1000 + i,
      title: i === 42 ? 'Needle retry issue' : `Issue number ${i}`,
      url: `https://linear.app/acme/issue/PLA-${1000 + i}`,
      priority: i % 5,
      updatedAt: new Date(1_763_000_000_000 + i * 1000).toISOString(),
      description: `SECRET_DESCRIPTION_${i}`,
      teamId: 'team-pla',
      stateId: i % 2 === 0 ? 'state-progress' : 'state-todo',
      assigneeName: i % 3 === 0 ? 'Alex Tester' : null,
      assignedToMe: i % 3 === 0,
      labels: [{ id: 'lbl-1', name: 'core' }],
      attachments: [],
      comments: [{ id: `c-${i}`, body: `SECRET_COMMENT_${i}`, createdAt: '2026-11-18T10:00:00.000Z', updatedAt: '2026-11-18T10:00:00.000Z', user: { name: 'Alex' } }],
    });
  }
  h.fake.issues = many;

  // Page across 3 pages (150 unique issues), confirming no descriptions or comments leak into list replies.
  const seen = new Set<string>();
  let cursor: string | undefined;
  for (let pageIndex = 0; pageIndex < 3; pageIndex++) {
    const reply = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active', ...(cursor ? { cursor } : {}) });
    assert.equal(reply.ok, true);
    assert.equal(reply.result.issues.length, 50);
    assert.equal(reply.result.has_more, true);
    assert.equal(typeof reply.result.next_cursor, 'string');
    const serialized = JSON.stringify(reply.result);
    assert(!serialized.includes('SECRET_DESCRIPTION_'), 'list must never return descriptions');
    assert(!serialized.includes('SECRET_COMMENT_'), 'list must never return comments');
    for (const issue of reply.result.issues) seen.add(issue.id);
    cursor = reply.result.next_cursor;
  }
  assert.equal(seen.size, 150);

  // Continue paging past 500 issues (all 12 pages) and assert SQLite cache stays <= 500 rows for team-pla.
  while (cursor) {
    const reply = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active', cursor });
    assert.equal(reply.ok, true);
    for (const issue of reply.result.issues) seen.add(issue.id);
    cursor = reply.result.next_cursor ?? undefined;
  }
  assert.equal(seen.size, 560);
  const cachedCount = Number(h.runtime.store.db.prepare('SELECT COUNT(*) AS n FROM linear_cache_issues WHERE team_id=?').get('team-pla')!.n);
  assert.equal(cachedCount, LINEAR_CACHE_MAX_ISSUES_PER_TEAM, 'cache must cap rows per team at 500');

  // Search by text and by identifier.
  const byTitle = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active', search: 'needle' });
  assert.equal(byTitle.result.issues.length, 1);
  assert.equal(byTitle.result.issues[0].identifier, 'PLA-1042');

  const byId = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active', search: 'PLA-1042' });
  assert.equal(byId.result.issues.length, 1);
  assert.equal(byId.result.issues[0].identifier, 'PLA-1042');

  // Offline fallback serves cached search page with age after TTL expiry.
  h.advance(LINEAR_CACHE_ISSUES_TTL_MS + 15_000);
  h.fake.enqueueFault((_req, res) => { res.writeHead(502); res.end(); return true; });
  const offlinePage = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active', search: 'needle' });
  assert.equal(offlinePage.ok, true);
  assert.equal(offlinePage.result.cached, true);
  assert.equal(offlinePage.result.stale, true);
  assert.equal(offlinePage.result.cache_age_seconds, Math.floor((LINEAR_CACHE_ISSUES_TTL_MS + 15_000) / 1000));
  assert.equal(offlinePage.result.issues[0].identifier, 'PLA-1042');
});

test('issue detail returns description, attachment metadata without fetching URLs, paged comments, LRU 200 cap, and offline cache', async t => {
  const h = await setupHarness(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });

  // Point attachment at our own loopback fake server to prove the host never fetches it.
  const attachUrl = h.fake.url.replace('/graphql', '/attachment/secret.png');
  const issue = h.fake.issues[0];
  issue.attachments = [{ id: 'att-1', title: 'Screenshot\u202e', url: attachUrl }];
  issue.comments = Array.from({ length: 30 }, (_, i) => ({
    id: `cmt-${i + 1}`,
    body: `Comment body ${i + 1}`,
    createdAt: '2026-11-18T11:00:00.000Z',
    updatedAt: '2026-11-18T11:00:00.000Z',
    user: { name: 'Alex' },
  }));

  const page1 = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' });
  assert.equal(page1.ok, true);
  assert.equal(page1.result.cached, false);
  assert.equal(page1.result.issue.description, issue.description);
  assert.deepEqual(page1.result.issue.attachments, [{ id: 'att-1', title: 'Screenshot\\u202e', url: attachUrl }]);
  assert.equal(page1.result.issue.comments.length, 25);
  assert.equal(page1.result.issue.comments_has_more, true);
  assert.equal(h.fake.attachmentHits, 0, 'host must never fetch attachment URLs');

  // Second comments page fetches remaining 5 comments.
  const page2 = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142', comments_cursor: page1.result.issue.comments_next_cursor });
  assert.equal(page2.ok, true);
  assert.equal(page2.result.issue.comments.length, 5);
  assert.equal(page2.result.issue.comments_has_more, false);
  assert.equal(h.fake.attachmentHits, 0);

  // Offline after TTL serves cached detail with age.
  h.advance(LINEAR_CACHE_DETAIL_TTL_MS + 10_000);
  h.fake.enqueueFault((_req, res) => { res.writeHead(500); res.end(); return true; });
  const offline = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' });
  assert.equal(offline.ok, true);
  assert.equal(offline.result.cached, true);
  assert.equal(offline.result.stale, true);
  assert.equal(offline.result.cache_age_seconds, Math.floor((LINEAR_CACHE_DETAIL_TTL_MS + 10_000) / 1000));

  // Missing issue returns not_found.
  const missing = await h.call({ op: 'linear_get_issue', issue_id: 'does-not-exist' });
  assert.equal(missing.ok, false);
  assert.equal(missing.error.code, 'not_found');

  // LRU detail cache caps at 200 entries.
  for (let i = 1; i <= LINEAR_CACHE_MAX_DETAILS + 15; i++) {
    h.runtime.store.linear.saveIssueDetail({
      ...page1.result.issue,
      id: `lru-issue-${i}`,
    }, h.now + i);
  }
  const detailCount = Number(h.runtime.store.db.prepare('SELECT COUNT(*) AS n FROM linear_cache_details').get()!.n);
  assert.equal(detailCount, LINEAR_CACHE_MAX_DETAILS);
});

test('async guards: superseded query does not overwrite newer cache and sign-out during in-flight request does not repopulate cache', async t => {
  const h = await setupHarness(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });

  // 1. Superseded query guard: start request 1 (held open), then request 2 completes with newer title,
  // then request 1 resolves with older title -> cache must still hold the newer title.
  let releaseFirst!: () => void;
  const firstStarted = new Promise<void>(resolve => {
    h.fake.enqueueFault(async (_req, res) => {
      resolve();
      await new Promise<void>(done => { releaseFirst = done; });
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({
        data: {
          issue: {
            id: 'issue-142', identifier: 'PLA-142', title: 'STALE_TITLE',
            url: 'https://linear.app/acme/issue/PLA-142', priority: 2,
            updatedAt: '2026-11-18T10:00:00.000Z', description: 'old',
            team: { id: 'team-pla' }, state: { id: 'state-progress', name: 'In Progress', type: 'started' },
            assignee: null, labels: { nodes: [] }, attachments: { nodes: [] },
            comments: { nodes: [], pageInfo: { hasNextPage: false, endCursor: null } },
          },
        },
      }));
      return true;
    });
  });

  const req1 = h.call({ op: 'linear_get_issue', issue_id: 'issue-142', refresh: true });
  await firstStarted;

  h.fake.issues[0].title = 'FRESH_NEWER_TITLE';
  const req2 = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142', refresh: true });
  assert.equal(req2.ok, true);
  assert.equal(req2.result.issue.title, 'FRESH_NEWER_TITLE');

  releaseFirst();
  await req1;
  const cachedAfter = h.runtime.store.linear.getIssueDetail('issue-142', h.now);
  assert.equal(cachedAfter?.issue.title, 'FRESH_NEWER_TITLE', 'superseded reply must not overwrite newer cache');

  // 2. Sign-out while a request is in flight: the in-flight reply must fail and NOT repopulate the cache.
  let releaseInFlight!: () => void;
  const inFlightStarted = new Promise<void>(resolve => {
    h.fake.enqueueFault(async (_req, res) => {
      resolve();
      await new Promise<void>(done => { releaseInFlight = done; });
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({
        data: {
          teams: {
            nodes: [{ id: 'team-pla', key: 'PLA', name: 'Plait', states: { nodes: [] } }],
            pageInfo: { hasNextPage: false, endCursor: null },
          },
        },
      }));
      return true;
    });
  });

  const teamsReq = h.call({ op: 'linear_list_teams', refresh: true });
  await inFlightStarted;
  assert.equal((await h.call({ op: 'linear_sign_out' })).ok, true);
  releaseInFlight();
  const teamsReply = await teamsReq;
  assert.equal(teamsReply.ok, false);
  assert.equal(teamsReply.error.code, 'linear_not_signed_in');
  assert.equal(h.runtime.store.linear.getTeams(), null, 'sign-out must prevent in-flight reply from repopulating cache');
});

test('migration: store written by current main gains Linear tables idempotently and remains readable by code ignoring them', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-linear-migration-'));
  t.after(() => rm(dir, { recursive: true, force: true }));

  // Create a pre-Linear host.sqlite matching current main's schema.
  const pre = new DatabaseSync(join(dir, 'host.sqlite'));
  pre.exec(`
    PRAGMA journal_mode=WAL;
    CREATE TABLE threads(id TEXT PRIMARY KEY, snapshot TEXT NOT NULL, head INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE requests(scope TEXT NOT NULL, id TEXT NOT NULL, hash TEXT NOT NULL, result TEXT NOT NULL, input TEXT, entry INTEGER, anchor INTEGER, PRIMARY KEY(scope,id));
    CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE roots(id TEXT PRIMARY KEY, identity TEXT NOT NULL);
    CREATE TABLE postimages(thread_id TEXT NOT NULL, path TEXT NOT NULL, image TEXT NOT NULL, PRIMARY KEY(thread_id,path));
    CREATE TABLE records(id TEXT PRIMARY KEY, thread TEXT NOT NULL, run_request TEXT NOT NULL, kind TEXT NOT NULL, checkout TEXT NOT NULL,
      before TEXT, commit_id TEXT, tree TEXT, head TEXT, head_ref TEXT, state TEXT NOT NULL, not_covered TEXT, not_covered_total INTEGER,
      after_restart INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
    CREATE TABLE record_garbage(id TEXT PRIMARY KEY);
    CREATE TABLE undo_operations(thread TEXT NOT NULL, request_id TEXT NOT NULL, target TEXT NOT NULL, leave_changed INTEGER NOT NULL,
      before_id TEXT NOT NULL, record_id TEXT NOT NULL, result TEXT, PRIMARY KEY(thread,request_id));
    CREATE TABLE record_paths(record TEXT NOT NULL, path TEXT NOT NULL, before_mode TEXT, before_oid TEXT, after_mode TEXT, after_oid TEXT,
      added INTEGER, removed INTEGER, PRIMARY KEY(record,path));
  `);
  pre.prepare('INSERT INTO threads(id, snapshot, head) VALUES (?, ?, 0)').run(
    'thread-legacy',
    JSON.stringify({ id: 'thread-legacy', checkout: '/repo', model: { provider: 'p', model_id: 'm' }, status: 'idle', messages: [], live_text: '', tools: [], pending_approval: null, error: null, truncated: false }),
  );
  // Also retain rows in the Linear tables that today's host already writes.
  pre.exec(`CREATE TABLE linear_thread_links(thread_id TEXT PRIMARY KEY, issue_id TEXT NOT NULL,
    identifier TEXT NOT NULL, url TEXT NOT NULL, title TEXT NOT NULL, linked_at INTEGER NOT NULL);
    CREATE TABLE linear_cache_meta(key TEXT PRIMARY KEY, cached_at INTEGER NOT NULL);
    INSERT INTO linear_thread_links VALUES ('thread-legacy','issue-142','PLA-142','https://linear.app/acme/issue/PLA-142','Legacy',1);
    INSERT INTO linear_cache_meta VALUES ('teams',1);`);
  pre.close();

  // Open with Store -> adds Linear tables without disturbing existing rows.
  const store1 = new Store(dir);
  const tables = store1.db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'linear_%'").all().map(r => String(r.name)).sort();
  assert.deepEqual(tables, ['linear_cache_aliases', 'linear_cache_details', 'linear_cache_images', 'linear_cache_issues', 'linear_cache_meta', 'linear_cache_pages', 'linear_cache_teams', 'linear_projects', 'linear_thread_links', 'linear_write_intents', 'linear_writes']);
  assert.equal(store1.get('thread-legacy').id, 'thread-legacy');
  assert.equal(store1.linear.getThreadLink('thread-legacy')?.workspace_id, null);
  assert.equal(store1.linear.getTeams()?.complete, null);
  const service = new LinearService(store1, dir);
  const historical = await service.getThreadLink('thread-legacy');
  assert.equal(historical.link?.historical, true);
  store1.close();

  // Re-opening is idempotent.
  const store2 = new Store(dir);
  assert.equal(store2.get('thread-legacy').id, 'thread-legacy');
  store2.close();

  // Code that only knows pre-Linear tables can still open and query the database cleanly.
  const legacyReader = new DatabaseSync(join(dir, 'host.sqlite'));
  const row = legacyReader.prepare('SELECT id, snapshot FROM threads WHERE id=?').get('thread-legacy');
  assert.equal(row?.id, 'thread-legacy');
  legacyReader.close();
});

test('all Linear read operations strictly refuse malformed inputs, unknown fields, and protocol_version', async t => {
  const h = await setupHarness(t);
  for (const bad of [
    { op: 'linear_auth_status', extra: true },
    { op: 'linear_auth_status', protocol_version: 6 },
    { op: 'linear_set_key' },
    { op: 'linear_set_key', key: '' },
    { op: 'linear_set_key', key: 'lin_api_ok', extra: 1 },
    { op: 'linear_sign_out', protocol_version: 5 },
    { op: 'linear_list_teams', refresh: 'yes' },
    { op: 'linear_get_project_binding', checkout: 'relative/path' },
    { op: 'linear_bind_project', checkout: '/abs', team_id: '../bad' },
    { op: 'linear_bind_project', checkout: '/abs', team_id: 'team-1', team_key: 'BAD KEY!' },
    { op: 'linear_list_issues', team_id: 'team-1' },
    { op: 'linear_list_issues', team_id: 'team-1', filter: 'bogus' },
    { op: 'linear_list_issues', team_id: '../bad', filter: 'active' },
    { op: 'linear_list_issues', team_id: 'team-1', filter: 'active', search: 'x'.repeat(201) },
    { op: 'linear_list_issues', team_id: 'team-1', filter: 'active', cursor: 'bad\ncursor' },
    { op: 'linear_list_issues', team_id: 'team-1', filter: 'active', refresh: 1 },
    { op: 'linear_get_issue', issue_id: 'bad/id' },
    { op: 'linear_get_issue', issue_id: 'issue-1', comments_cursor: 'x'.repeat(600) },
    { op: 'linear_get_issue', issue_id: 'issue-1', extra: true },
  ]) {
    const reply = await h.call(bad);
    assert.equal(reply.ok, false, `expected rejection for ${JSON.stringify(bad)}`);
    assert.equal(reply.error.code, 'invalid_request', JSON.stringify(bad));
  }
  for (const inherited of ['toString', 'constructor', '__proto__', 'hasOwnProperty']) {
    assert.equal(h.runtime.linear.handles({ op: inherited }), false);
  }
});

test('server echoing Authorization header in issue data never leaks key into replies or host.sqlite', async t => {
  const h = await setupHarness(t);
  const secret = h.fake.validKey;
  assert.equal((await h.call({ op: 'linear_set_key', key: secret })).ok, true);

  // Hostile server echoes the Authorization key in title, URL, cursor, description, and comments.
  h.fake.enqueueFault((req, res) => {
    const echoed = String(req.headers.authorization ?? '');
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      data: {
        issues: {
          nodes: [{
            id: 'issue-echo',
            identifier: 'PLA-77',
            title: `Echoed ${echoed}`,
            url: `https://linear.app/acme/issue/PLA-77?k=${encodeURIComponent(echoed)}`,
            priority: 1,
            updatedAt: '2026-11-18T12:00:00.000Z',
            team: { id: 'team-pla' },
            state: { id: 'state-todo', name: `State ${echoed}`, type: 'unstarted' },
            assignee: { name: `User ${echoed}` },
            labels: { nodes: [{ id: 'lbl-1', name: `Tag ${echoed}` }] },
          }],
          pageInfo: { hasNextPage: true, endCursor: `cur-${echoed}` },
        },
      },
    }));
    return true;
  });

  const listed = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' });
  assert.equal(listed.ok, true);
  assert(!JSON.stringify(listed).includes(secret), 'issue list reply must not contain echoed key');
  assert(!JSON.stringify(listed).includes(encodeURIComponent(secret)), 'issue list reply must not contain encoded key');
  assert.equal(listed.result.issues[0].url, '');
  assert.equal(listed.result.next_cursor, null);
  await assertKeyNotInStoreDb(h.storeDir, secret);
});

test('linear_sign_out or newer linear_set_key during in-flight linear_set_key prevents writing stale credential', async t => {
  const h = await setupHarness(t);

  // 1. Sign-out during in-flight set_key must prevent linear-auth.json from being written.
  let releaseFirst!: () => void;
  const firstStarted = new Promise<void>(resolve => {
    h.fake.enqueueFault(async () => {
      resolve();
      await new Promise<void>(done => { releaseFirst = done; });
      return false;
    });
  });

  const setKeyReq = h.call({ op: 'linear_set_key', key: h.fake.validKey });
  await firstStarted;
  assert.equal((await h.call({ op: 'linear_sign_out' })).ok, true);
  releaseFirst();
  const setKeyReply = await setKeyReq;
  assert.equal(setKeyReply.ok, false);
  assert.equal(setKeyReply.error.code, 'linear_not_signed_in');
  assert.equal((await h.call({ op: 'linear_auth_status' })).result.signed_in, false);
  await assert.rejects(() => stat(join(h.storeDir, 'linear-auth.json')), { code: 'ENOENT' });

  // 2. Newer set_key completing while an older set_key is still verifying must not be overwritten.
  let releaseOlder!: () => void;
  const olderStarted = new Promise<void>(resolve => {
    h.fake.enqueueFault(async () => {
      resolve();
      await new Promise<void>(done => { releaseOlder = done; });
      return false;
    });
  });
  const olderReq = h.call({ op: 'linear_set_key', key: h.fake.validKey });
  await olderStarted;
  h.fake.organization = { id: 'org-2', name: 'Second Workspace', urlKey: 'second' };
  const newerReply = await h.call({ op: 'linear_set_key', key: h.fake.readOnlyKey });
  assert.equal(newerReply.ok, true);
  assert.equal(newerReply.result.workspace_id, 'org-2');

  releaseOlder();
  const olderReply = await olderReq;
  assert.equal(olderReply.ok, false);
  assert.equal(olderReply.error.code, 'linear_not_signed_in');
  assert.equal((await h.call({ op: 'linear_auth_status' })).result.workspace_id, 'org-2');
});

test('project binding from a prior workspace is never served after sign-out or workspace switch', async t => {
  const h = await setupHarness(t);
  const repo = await makeRepo(h.dir, 'project-ws-isolation');

  // Sign in to workspace 1, load teams, and bind repo to team-pla.
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);
  assert.equal((await h.call({ op: 'linear_list_teams' })).ok, true);
  const bound = await h.call({ op: 'linear_bind_project', checkout: repo, team_id: 'team-pla' });
  assert.equal(bound.ok, true);
  assert.equal(bound.result.binding.workspace_id, 'org-1');

  // Sign out -> workspace 1 binding must not be served.
  assert.equal((await h.call({ op: 'linear_sign_out' })).ok, true);
  const afterSignOut = await h.call({ op: 'linear_get_project_binding', checkout: repo });
  assert.equal(afterSignOut.ok, true);
  assert.equal(afterSignOut.result.binding, null, 'project binding must not be served after sign-out');

  // Re-sign-in to workspace 1, bind again, then switch directly to workspace 2 without signing out.
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);
  assert.equal((await h.call({ op: 'linear_bind_project', checkout: repo, team_id: 'team-pla' })).ok, true);
  h.fake.organization = { id: 'org-2', name: 'Other Workspace', urlKey: 'other' };
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  const afterSwitch = await h.call({ op: 'linear_get_project_binding', checkout: repo });
  assert.equal(afterSwitch.ok, true);
  assert.equal(afterSwitch.result.binding, null, 'workspace 1 binding must not be served under workspace 2');
});

test('linear_list_teams stops on repeated cursor and deduplicates team IDs without crashing', async t => {
  const h = await setupHarness(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  const reqBefore = h.fake.requests.length;
  for (let i = 0; i < 15; i++) {
    h.fake.enqueueFault((_req, res) => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({
        data: {
          teams: {
            nodes: [
              { id: 'team-loop', key: 'LOOP', name: 'Loop Team', states: { nodes: [] } },
              { id: 'team-loop', key: 'LOOP', name: 'Loop Team Duplicate', states: { nodes: [] } },
            ],
            pageInfo: { hasNextPage: true, endCursor: 'STUCK_CURSOR' },
          },
        },
      }));
      return true;
    });
  }

  const reply = await h.call({ op: 'linear_list_teams' });
  assert.equal(reply.ok, true, JSON.stringify(reply));
  assert.equal(reply.result.teams.length, 1);
  assert.equal(reply.result.teams[0].id, 'team-loop');
  assert(h.fake.requests.length - reqBefore <= 2, `expected repeated cursor to stop early, got ${h.fake.requests.length - reqBefore} requests`);
});

test('partial GraphQL responses are not cached as complete data', async t => {
  const h = await setupHarness(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  h.fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      data: {
        teams: {
          nodes: [{ id: 'team-pla', key: 'PLA', name: 'Plait', states: { nodes: [] } }],
          pageInfo: { hasNextPage: false, endCursor: null },
        },
      },
      errors: [{ message: 'States resolver failed' }],
    }));
    return true;
  });

  const first = await h.call({ op: 'linear_list_teams' });
  assert.equal(first.ok, true);
  assert.equal(first.result.partial, true);
  assert.equal(first.result.teams[0].states.length, 0);

  // Next read must fetch fresh complete data rather than serving the incomplete partial response as complete.
  const second = await h.call({ op: 'linear_list_teams' });
  assert.equal(second.ok, true);
  assert.equal(second.result.cached, false);
  assert.equal(second.result.partial, false);
  assert.equal(second.result.teams[0].states.length, 5);
});

test('superseded listIssues for another filter does not overwrite newer issue summaries in cache', async t => {
  const h = await setupHarness(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  let releaseStale!: () => void;
  const staleStarted = new Promise<void>(resolve => {
    h.fake.enqueueFault(async (_req, res) => {
      resolve();
      await new Promise<void>(done => { releaseStale = done; });
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({
        data: {
          issues: {
            nodes: [{
              id: 'issue-142',
              identifier: 'PLA-142',
              title: 'STALE_CROSS_FILTER_TITLE',
              url: 'https://linear.app/acme/issue/PLA-142',
              priority: 2,
              updatedAt: '2026-11-18T10:00:00.000Z',
              team: { id: 'team-pla' },
              state: { id: 'state-progress', name: 'In Progress', type: 'started' },
              assignee: null,
              labels: { nodes: [] },
            }],
            pageInfo: { hasNextPage: false, endCursor: null },
          },
        },
      }));
      return true;
    });
  });

  const staleReq = h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' });
  await staleStarted;

  h.fake.issues[0].title = 'FRESH_CROSS_FILTER_TITLE';
  const freshReply = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'all' });
  assert.equal(freshReply.ok, true);
  assert.equal(freshReply.result.issues[0].title, 'FRESH_CROSS_FILTER_TITLE');

  releaseStale();
  await staleReq;

  const cachedAll = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'all' });
  assert.equal(cachedAll.ok, true);
  assert.equal(cachedAll.result.cached, true);
  assert.equal(cachedAll.result.issues[0].title, 'FRESH_CROSS_FILTER_TITLE');
});

test('multibyte issue description and comments honor UTF-8 KiB bounds and fit inside 1 MiB host frame', async t => {
  const h = await setupHarness(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  // Use 3-byte UTF-8 characters (65,536 chars = 192 KiB; 25 * 16,384 chars = 1.2 MiB if only UTF-16 bounded).
  // Keep raw JSON under LinearClient's 2 MiB response body cap while exceeding 64 KiB / 16 KiB in UTF-8 bytes.
  h.fake.issues[0].description = 'あ'.repeat(40_000);
  h.fake.issues[0].comments = Array.from({ length: 25 }, (_, i) => ({
    id: `mb-cmt-${i}`,
    body: 'あ'.repeat(16_384),
    createdAt: '2026-11-18T11:00:00.000Z',
    updatedAt: '2026-11-18T11:00:00.000Z',
    user: { name: 'Alex' },
  }));

  const reply = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' });
  assert.equal(reply.ok, true, JSON.stringify(reply));
  assert(Buffer.byteLength(reply.result.issue.description, 'utf8') <= 65_536, 'description must be <= 64 KiB UTF-8');
  for (const comment of reply.result.issue.comments) {
    assert(Buffer.byteLength(comment.body, 'utf8') <= 16_384, 'comment body must be <= 16 KiB UTF-8');
  }
  const frameBytes = Buffer.byteLength(JSON.stringify(reply) + '\n', 'utf8');
  assert(frameBytes <= 1_048_576, `reply frame must stay <= 1 MiB, got ${frameBytes} bytes`);
});

test('bounded text inputs reject oversize whitespace padding and trim team_name', async t => {
  const h = await setupHarness(t);
  const repo = await makeRepo(h.dir, 'project-trim');
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  // Oversize whitespace-padded search exceeding 200 chars must be rejected.
  const hugeSearch = await h.call({
    op: 'linear_list_issues',
    team_id: 'team-pla',
    filter: 'active',
    search: `${' '.repeat(500)}retry`,
  });
  assert.equal(hugeSearch.ok, false);
  assert.equal(hugeSearch.error.code, 'invalid_request');

  // Leading/trailing whitespace in a valid team_name must be trimmed before storing.
  const bound = await h.call({
    op: 'linear_bind_project',
    checkout: repo,
    team_id: 'team-pla',
    team_name: '  Plait Core  ',
  });
  assert.equal(bound.ok, true);
  assert.equal(bound.result.binding.team_name, 'Plait Core');
});

test('getIssuesPage fetches a 50-issue cached page in one SQLite query without N+1 selects', async t => {
  const h = await setupHarness(t);
  const issues = Array.from({ length: 50 }, (_, i) => ({
    id: `issue-q-${i}`,
    identifier: `PLA-${500 + i}`,
    title: `Cached issue ${i}`,
    url: `https://linear.app/acme/issue/PLA-${500 + i}`,
    priority: 2,
    updated_at: '2026-11-18T12:00:00.000Z',
    team_id: 'team-pla',
    state: { id: 'state-todo', name: 'Todo', type: 'unstarted' },
    assignee: null,
    labels: [],
  }));
  h.runtime.store.linear.saveIssuesPage('batch-page-key', 'team-pla', issues, true, 'next-1', h.now);

  let issueSelects = 0;
  const origPrepare = h.runtime.store.db.prepare.bind(h.runtime.store.db);
  h.runtime.store.db.prepare = ((sql: string) => {
    const stmt = origPrepare(sql);
    if (sql.includes('linear_cache_issues')) {
      const origGet = stmt.get.bind(stmt);
      const origAll = stmt.all.bind(stmt);
      stmt.get = (...args: unknown[]) => { issueSelects++; return origGet(...args as []); };
      stmt.all = (...args: unknown[]) => { issueSelects++; return origAll(...args as []); };
    }
    return stmt;
  }) as typeof h.runtime.store.db.prepare;

  const loaded = h.runtime.store.linear.getIssuesPage('batch-page-key', h.now + 1000);
  assert.notEqual(loaded, null);
  assert.equal(loaded!.issues.length, 50);
  assert.equal(loaded!.issues[0].id, 'issue-q-0');
  assert.equal(loaded!.issues[49].id, 'issue-q-49');
  assert.equal(issueSelects, 1, `expected 1 batched query for 50 issues, got ${issueSelects}`);
});

test('exact identifier and URL resolution is bound to the project team and workspace, with offline aliases', async t => {
  const h = await setupHarness(t);
  const root = await makeRepo(h.dir, 'exact-repo');
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const request = { op: 'linear_resolve_issue', checkout: root, reference: 'PLA-142' };
  assert.equal((await h.call(request)).error.code, 'invalid_request');
  await h.call({ op: 'linear_bind_project', checkout: root, team_id: 'team-pla' });
  // The same number in another team and a matching title must never influence resolution.
  h.fake.issues.unshift({ ...h.fake.issues[0], id: 'ops-142', identifier: 'OPS-142', teamId: 'team-ops',
    url: 'https://linear.app/acme/issue/OPS-142', stateId: 'state-ops-todo', title: 'PLA-142' });
  const resolved = await h.call(request);
  assert.equal(resolved.result.issue.id, 'issue-142');
  assert.equal(resolved.result.workspace_id, 'org-1');
  assert.equal(resolved.result.issue.description, undefined);
  assert.equal(h.fake.requests.at(-1)?.variables.id, 'PLA-142');
  assert.equal((await h.call({ ...request, reference: 'OPS-142' })).error.code, 'not_found');
  const before = h.fake.requests.length;
  const pasted = await h.call({ ...request, reference: 'https://linear.app/acme/issue/PLA-142/a-slug#comment' });
  assert.equal(pasted.result.cached, true);
  assert.equal(h.fake.requests.length, before);
  for (const reference of ['https://linear.app/other/issue/PLA-142', 'https://linear.app/acme/issue/OPS-142']) {
    const result = await h.call({ ...request, reference });
    assert.equal(result.ok, false);
  }
  for (const reference of ['142', '#142', 'https://evil.invalid/acme/issue/PLA-142', 'https://u:p@linear.app/acme/issue/PLA-142',
    'https://linear.app/acme/issue/PLA-142/slug/extra', 'https://linear.app/acme/issue/PLA%2D142', 'PLA-142\nBAD']) {
    assert.equal((await h.call({ ...request, reference })).error.code, 'invalid_request');
  }
  // A list-known issue also resolves offline even without a saved detail.
  await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'all' });
  h.advance(LINEAR_CACHE_ISSUES_TTL_MS + 1);
  await h.fake.close();
  const cached = await h.call(request);
  assert.equal(cached.result.issue.id, 'issue-142');
  assert.equal(cached.result.stale, true);
  assert.equal(cached.result.cached, true);
  assert.equal((await h.call({ ...request, reference: 'PLA-143' })).result.issue.id, 'issue-143');
  assert.equal((await h.call({ ...request, reference: 'PLA-999' })).error.code, 'linear_offline');
});

test('exact lookup validates returned identity, rejects stale bindings and auth, and persists aliases across restart', async t => {
  const h = await setupHarness(t);
  const root = await makeRepo(h.dir, 'guarded-repo');
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  await h.call({ op: 'linear_bind_project', checkout: root, team_id: 'team-pla' });
  const request = { op: 'linear_resolve_issue', checkout: root, reference: 'PLA-142', refresh: true };
  h.fake.issues[0].url = 'https://linear.app/other/issue/PLA-142';
  assert.equal((await h.call(request)).error.code, 'not_found');
  assert.equal(h.runtime.store.linear.getIssueAlias('org-1', 'team-pla', 'PLA-142'), null);
  h.fake.issues[0].url = 'https://linear.app/acme/issue/PLA-142';
  await h.call(request);
  h.runtime.store.close();
  h.runtime.store = new Store(h.storeDir);
  h.runtime.linear = new LinearService(h.runtime.store, h.storeDir, { endpoint: h.fake.url, now: () => h.now });
  const count = h.fake.requests.length;
  assert.equal((await h.call({ ...request, refresh: false })).result.cached, true);
  assert.equal(h.fake.requests.length, count);
  let release!: () => void;
  const started = new Promise<void>(resolve => h.fake.enqueueFault(async () => {
    resolve(); await new Promise<void>(done => { release = done; }); return false;
  }));
  const pending = h.call(request);
  await started;
  await h.call({ op: 'linear_bind_project', checkout: root, team_id: 'team-ops' });
  release();
  assert.equal((await pending).error.code, 'conflict');
  await h.call({ op: 'linear_bind_project', checkout: root, team_id: 'team-pla' });
  const authStarted = new Promise<void>(resolve => h.fake.enqueueFault(async () => {
    resolve(); await new Promise<void>(done => { release = done; }); return false;
  }));
  const authPending = h.call(request);
  await authStarted;
  await h.call({ op: 'linear_sign_out' });
  release();
  assert.equal((await authPending).error.code, 'linear_not_signed_in');
  assert.equal(h.runtime.store.linear.getIssueAlias('org-1', 'team-pla', 'PLA-142'), null);
  h.fake.organization = { id: 'org-2', name: 'Other', urlKey: 'other' };
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  assert.equal(h.runtime.store.linear.getIssueAlias('org-1', 'team-pla', 'PLA-142'), null);
  assert.equal((await h.call(request)).error.code, 'invalid_request');
  assert.equal((await h.call({ op: 'linear_get_issue', issue_id: 'issue-142', workspace_id: 'org-1' })).error.code, 'conflict');
});

test('Linear rate-limit replies carry bounded wait seconds without changing other failures', async t => {
  const h = await setupHarness(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  h.fake.enqueueFault((_req, res) => { res.writeHead(429, { 'Retry-After': '12' }); res.end(); return true; });
  const limited = await h.call({ op: 'linear_list_teams' });
  assert.deepEqual(limited, { ok: false, error: { code: 'linear_rate_limited', message: 'Linear rate limit reached. Retry in 12 seconds.', retry_after_seconds: 12 } });
  h.fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'X-RateLimit-Requests-Reset': String(h.now + 32_000) });
    res.end(JSON.stringify({ errors: [{ extensions: { code: 'RATELIMITED' } }] })); return true;
  });
  const gql = await h.call({ op: 'linear_list_teams' });
  assert.equal(gql.error.retry_after_seconds, 32);
  assert.deepEqual(await h.call({ op: 'nonexistent' }), { ok: false, error: { code: 'invalid_request', message: 'Unknown operation or field.' } });
  assert.equal((await h.call({ op: 'linear_get_issue', issue_id: 'absent' })).error.retry_after_seconds, undefined);
});

test('team and workflow completeness survives caching and reports caps honestly', async t => {
  const h = await setupHarness(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const complete = await h.call({ op: 'linear_list_teams' });
  assert.equal(complete.result.teams_complete, true);
  assert(complete.result.teams.every((team: any) => team.states_complete === true));
  h.fake.teams[0].states = Array.from({ length: 51 }, (_, i) => ({ id: `state-${i}`, name: `S${i}`, type: 'started', position: i }));
  h.fake.teams = Array.from({ length: 51 }, (_, i) => ({ ...h.fake.teams[0], id: `team-${i}`, key: `T${i}` }));
  const capped = await h.call({ op: 'linear_list_teams', refresh: true });
  assert.equal(capped.result.teams.length, 50);
  assert.equal(capped.result.teams_complete, false);
  assert.equal(capped.result.teams[0].states.length, 50);
  assert.equal(capped.result.teams[0].states_complete, false);
  const cached = await h.call({ op: 'linear_list_teams' });
  assert.equal(cached.result.teams_complete, false);
  assert.equal(cached.result.teams[0].states_complete, false);
  const owner = h.runtime.store.linear.getCacheOwner() ?? undefined;
  h.runtime.store.linear.saveTeams([{ id: 'old', key: 'OLD', name: 'Old cache', states: [] }], h.now, null, owner);
  const legacy = await h.call({ op: 'linear_list_teams' });
  assert.equal(legacy.result.teams_complete, null);
  assert.equal(legacy.result.teams[0].states_complete, null);
});

test('write invalidation marks cached aliases stale while preserving cached_at and age offline', async t => {
  const h = await setupHarness(t);
  const root = await makeRepo(h.dir, 'stale-alias-repo');
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  await h.call({ op: 'linear_bind_project', checkout: root, team_id: 'team-pla' });

  const initialAt = h.now;
  const resolved = await h.call({ op: 'linear_resolve_issue', checkout: root, reference: 'PLA-142' });
  assert.equal(resolved.ok, true);
  assert.equal(resolved.result.cached_at, initialAt);

  // Post a comment whose follow-up issue refresh fails with 503 so the alias remains invalidated.
  h.advance(10_000);
  h.fake.enqueueFault(() => false);
  h.fake.enqueueFault((_req, res) => { res.writeHead(503); res.end(); return true; });
  const posted = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Invalidate alias without losing cached_at',
    request_id: 'req-stale-alias',
  });
  assert.equal(posted.ok, true);
  assert.equal(posted.result.issue, null);

  // While online, a write-invalidated alias must refresh instead of serving stale data.
  await h.call({ op: 'linear_resolve_issue', checkout: root, reference: 'PLA-143' });
  h.fake.enqueueFault(() => false);
  h.fake.enqueueFault((_req, res) => { res.writeHead(503); res.end(); return true; });
  await h.call({ op: 'linear_post_comment', issue_id: 'issue-143', body: 'Invalidate PLA-143', request_id: 'req-stale-online' });
  h.fake.issues[1].title = 'Refreshed PLA-143';
  const refreshedOnline = await h.call({ op: 'linear_resolve_issue', checkout: root, reference: 'PLA-143' });
  assert.equal(refreshedOnline.ok, true);
  assert.equal(refreshedOnline.result.cached, false);
  assert.equal(refreshedOnline.result.issue.title, 'Refreshed PLA-143');

  // While Linear is offline, resolving from the invalidated alias must report the real cached_at and age.
  h.advance(5_000);
  await h.fake.close();
  const offline = await h.call({ op: 'linear_resolve_issue', checkout: root, reference: 'PLA-142' });
  assert.equal(offline.ok, true);
  assert.equal(offline.result.cached, true);
  assert.equal(offline.result.stale, true);
  assert.equal(offline.result.cached_at, initialAt);
  assert.equal(offline.result.cache_age_seconds, 15);
});

test('issue URL resolution accepts lowercase identifiers and rejects empty slug double slashes', async t => {
  const h = await setupHarness(t);
  const root = await makeRepo(h.dir, 'case-url-repo');
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  await h.call({ op: 'linear_bind_project', checkout: root, team_id: 'team-pla' });

  for (const reference of [
    'https://linear.app/acme/issue/pla-142',
    '  https://linear.app/acme/issue/pla-142/fix-retry  ',
  ]) {
    const resolved = await h.call({ op: 'linear_resolve_issue', checkout: root, reference, refresh: true });
    assert.equal(resolved.ok, true, JSON.stringify(resolved));
    assert.equal(resolved.result.issue.id, 'issue-142');
    assert.equal(resolved.result.issue.identifier, 'PLA-142');
  }

  const doubleSlash = await h.call({
    op: 'linear_resolve_issue',
    checkout: root,
    reference: 'https://linear.app/acme/issue/PLA-142//',
  });
  assert.equal(doubleSlash.ok, false);
  assert.equal(doubleSlash.error.code, 'invalid_request');
});

test('interrupted Linear account switch across host restart never serves the previous workspace cache (#212)', async t => {
  const h = await setupHarness(t);
  const { LinearAuthStore } = await import('../src/linear/auth.ts');

  // 1. Sign in to workspace A and populate teams, issue pages, and issue details caches.
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);
  const teamsA = await h.call({ op: 'linear_list_teams' });
  assert.equal(teamsA.ok, true);
  assert.equal(teamsA.result.cached, false);
  assert.equal(teamsA.result.teams[0].id, 'team-pla');
  assert.equal((await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' })).ok, true);
  assert.equal((await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' })).ok, true);

  // Confirm workspace A's teams are currently fresh in cache.
  const cachedA = await h.call({ op: 'linear_list_teams' });
  assert.equal(cachedA.ok, true);
  assert.equal(cachedA.result.cached, true);
  assert.equal(cachedA.result.teams[0].id, 'team-pla');

  // 2. Simulate an interrupted switch to workspace B: credential file is replaced with B
  //    before the SQLite cache-clear transaction commits, then the host restarts.
  const authStore = new LinearAuthStore(h.storeDir);
  await authStore.write({
    api_key: h.fake.validKey,
    workspace_id: 'org-2',
    workspace_name: 'Workspace B',
    workspace_url_key: 'ws-b',
    viewer_id: 'viewer-2',
    viewer_name: 'Blake',
    saved_at: h.now,
    credential_id: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  });
  h.fake.organization = { id: 'org-2', name: 'Workspace B', urlKey: 'ws-b' };
  h.fake.teams = [
    {
      id: 'team-b',
      key: 'WSB',
      name: 'Workspace B Team',
      states: [{ id: 'state-b-open', name: 'Open', type: 'unstarted', position: 0 }],
    },
  ];
  h.fake.issues = [
    {
      ...h.fake.issues[0],
      id: 'issue-b-1',
      identifier: 'WSB-1',
      title: 'Workspace B issue',
      url: 'https://linear.app/ws-b/issue/WSB-1',
      teamId: 'team-b',
      stateId: 'state-b-open',
    },
  ];

  // Restart the host without running clearAll().
  h.runtime.store.close();
  h.runtime.store = new Store(h.storeDir);
  h.runtime.linear = new LinearService(h.runtime.store, h.storeDir, {
    endpoint: h.fake.url,
    timeoutMs: 1000,
    now: () => h.now,
  });

  // 3. Immediately request linear_list_teams WITHOUT refresh: must NOT return workspace A's cached teams.
  h.fake.enqueueFault((_req, res) => { res.writeHead(503); res.end(); return true; });
  const offline = await h.call({ op: 'linear_list_teams' });
  assert.equal(offline.ok, false);
  assert.equal(offline.error.code, 'linear_offline');

  const teamsB = await h.call({ op: 'linear_list_teams', refresh: false });
  assert.equal(teamsB.ok, true, JSON.stringify(teamsB));
  assert.equal(teamsB.result.cached, false);
  assert.deepEqual(teamsB.result.teams.map((team: any) => team.id), ['team-b']);

  // Workspace A's cached detail and issue pages were also cleared and are never served under B.
  const detailA = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142', refresh: false });
  assert.equal(detailA.ok, false);
  assert.equal(detailA.error.code, 'not_found');

  // 4. Even an interrupted key replacement within the SAME workspace (same timestamp) invalidates cache.
  assert.equal((await h.call({ op: 'linear_list_teams', refresh: false })).result.cached, true);
  await authStore.write({
    api_key: h.fake.validKey,
    workspace_id: 'org-2',
    workspace_name: 'Workspace B',
    workspace_url_key: 'ws-b',
    viewer_id: 'viewer-2',
    viewer_name: 'Blake',
    saved_at: h.now,
    credential_id: 'cccccccccccccccccccccccccccccccc',
  });
  h.runtime.store.close();
  h.runtime.store = new Store(h.storeDir);
  h.runtime.linear = new LinearService(h.runtime.store, h.storeDir, {
    endpoint: h.fake.url,
    timeoutMs: 1000,
    now: () => h.now,
  });
  const afterKeyReplace = await h.call({ op: 'linear_list_teams', refresh: false });
  assert.equal(afterKeyReplace.ok, true);
  assert.equal(afterKeyReplace.result.cached, false);
});
