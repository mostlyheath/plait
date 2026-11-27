import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { mkdir, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createModels } from '@earendil-works/pi-ai/models';
import { LinearService } from '../src/linear/service.ts';
import { LINEAR_COMMENT_BODY_MAX_BYTES } from '../src/linear/types.ts';
import { Runtime } from '../src/runtime.ts';
import { FakeLinearServer } from './support/linear-server.ts';

async function setup(t: test.TestContext) {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-linear-writes-'));
  const storeDir = join(dir, 'store');
  await mkdir(storeDir, { recursive: true, mode: 0o700 });
  const fake = new FakeLinearServer();
  let runtime: Runtime;
  t.after(async () => {
    try { await runtime?.close(); }
    finally { await fake.close(); await rm(dir, { recursive: true, force: true }); }
  });
  const endpoint = await fake.start();
  let now = 1_763_500_000_000;
  runtime = new Runtime(storeDir, createModels(), new Set());
  const attach = (target: Runtime) => {
    target.linear = new LinearService(target.store, storeDir, { endpoint, timeoutMs: 1000, now: () => now });
  };
  attach(runtime);
  const transcript: string[] = [];
  const call = async (input: unknown): Promise<any> => {
    const reply = await runtime.reply(input);
    transcript.push(JSON.stringify(reply));
    return reply;
  };
  const restart = async () => {
    await runtime.close();
    runtime = new Runtime(storeDir, createModels(), new Set());
    attach(runtime);
  };
  return {
    storeDir, fake, call, transcript, restart,
    get runtime() { return runtime; },
    advance(ms: number) { now += ms; },
  };
}

test('linear_post_comment posts once, invalidates cache, deduplicates retries (including after restart and failed refresh), and rejects conflicts', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  // Prime list and detail caches first.
  assert.equal((await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' })).ok, true);
  assert.equal((await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' })).result.issue.comments.length, 1);

  const mutationsBefore = h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length;
  const posted = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Investigated the backoff behavior.\nReady for review.',
    request_id: 'cmt-req-1',
  });
  assert.equal(posted.ok, true, JSON.stringify(posted));
  assert.equal(typeof posted.result.comment_id, 'string');
  assert.equal(posted.result.issue.comments.length, 2);
  assert.equal(posted.result.issue.comments[1].body, 'Investigated the backoff behavior.\nReady for review.');
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, mutationsBefore + 1);

  // Team issue list page cache was invalidated, so next list hits Linear fresh.
  const listAfter = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' });
  assert.equal(listAfter.result.cached, false);

  // Retrying the same request_id returns the saved result without posting a second comment.
  const retry1 = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Investigated the backoff behavior.\nReady for review.',
    request_id: 'cmt-req-1',
  });
  assert.deepEqual(retry1, posted);
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, mutationsBefore + 1);

  // Survives host restart without posting again.
  await h.restart();
  const retryAfterRestart = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Investigated the backoff behavior.\nReady for review.',
    request_id: 'cmt-req-1',
  });
  assert.deepEqual(retryAfterRestart, posted);
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, mutationsBefore + 1);

  // Same request_id with different body returns conflict and does not post.
  const conflict = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Different body',
    request_id: 'cmt-req-1',
  });
  assert.equal(conflict.ok, false);
  assert.equal(conflict.error.code, 'conflict');
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, mutationsBefore + 1);

  // If the post-write refresh fails (e.g. transient 503), retrying the same request_id
  // refreshes the issue WITHOUT posting a second comment.
  const beforeSecond = h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length;
  let sawComment = false;
  h.fake.enqueueFault(() => { sawComment = true; return false; });
  h.fake.enqueueFault((_req, res) => { res.writeHead(503); res.end(); return true; });
  const partialPost = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Second comment with flaky refresh',
    request_id: 'cmt-req-2',
  });
  assert.equal(sawComment, true);
  assert.equal(partialPost.ok, true);
  assert.equal(partialPost.result.issue, null);
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, beforeSecond + 1);

  const retrySecond = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Second comment with flaky refresh',
    request_id: 'cmt-req-2',
  });
  assert.equal(retrySecond.ok, true);
  assert.equal(retrySecond.result.comment_id, partialPost.result.comment_id);
  assert.equal(retrySecond.result.issue.comments.length, 3);
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, beforeSecond + 1);

  // Concurrent identical retries with the same request_id send only one mutation.
  let releaseConcurrent!: () => void;
  const concurrentStarted = new Promise<void>(resolve => {
    h.fake.enqueueFault(async () => {
      resolve();
      await new Promise<void>(done => { releaseConcurrent = done; });
      return false;
    });
  });
  const beforeConcurrent = h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length;
  const p1 = h.call({ op: 'linear_post_comment', issue_id: 'issue-142', body: 'Once only', request_id: 'cmt-req-3' });
  await concurrentStarted;
  const p2 = h.call({ op: 'linear_post_comment', issue_id: 'issue-142', body: 'Once only', request_id: 'cmt-req-3' });
  releaseConcurrent();
  const [r1, r2] = await Promise.all([p1, p2]);
  assert.deepEqual(r1, r2);
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, beforeConcurrent + 1);
});

test('linear_update_issue_state checks team states before sending, invalidates cache, and deduplicates retries', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);
  assert.equal((await h.call({ op: 'linear_list_teams' })).ok, true);
  assert.equal((await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' })).ok, true);
  assert.equal((await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' })).ok, true);

  // State from another team (team-ops) is refused BEFORE sending any mutation to Linear.
  const wrongTeam = await h.call({
    op: 'linear_update_issue_state',
    issue_id: 'issue-142',
    state_id: 'state-ops-done',
    request_id: 'state-req-bad',
  });
  assert.equal(wrongTeam.ok, false);
  assert.equal(wrongTeam.error.code, 'invalid_request');
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitIssueUpdateState')).length, 0);

  // Valid state belonging to team-pla succeeds, invalidates caches, and returns refreshed issue.
  const moved = await h.call({
    op: 'linear_update_issue_state',
    issue_id: 'issue-142',
    state_id: 'state-done',
    request_id: 'state-req-1',
  });
  assert.equal(moved.ok, true, JSON.stringify(moved));
  assert.equal(moved.result.issue.state.id, 'state-done');
  assert.equal(moved.result.issue.state.name, 'Done');
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitIssueUpdateState')).length, 1);

  // Active list cache was invalidated and now reflects that PLA-142 left active states.
  const activeAfter = await h.call({ op: 'linear_list_issues', team_id: 'team-pla', filter: 'active' });
  assert.equal(activeAfter.result.cached, false);
  assert.equal(activeAfter.result.issues.length, 0);

  // Retry with same request_id returns saved reply without repeating the mutation.
  const retry = await h.call({
    op: 'linear_update_issue_state',
    issue_id: 'issue-142',
    state_id: 'state-done',
    request_id: 'state-req-1',
  });
  assert.deepEqual(retry, moved);
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitIssueUpdateState')).length, 1);

  // Different state with same request_id returns conflict.
  const conflict = await h.call({
    op: 'linear_update_issue_state',
    issue_id: 'issue-142',
    state_id: 'state-todo',
    request_id: 'state-req-1',
  });
  assert.equal(conflict.ok, false);
  assert.equal(conflict.error.code, 'conflict');
});

test('read-only key refuses both writes safely and never leaks the key into replies or host.sqlite', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.readOnlyKey })).ok, true);
  assert.equal((await h.call({ op: 'linear_list_teams' })).ok, true);

  h.fake.readOnlyMode = 'graphql_forbidden';
  const commentRefused = await h.call({
    op: 'linear_post_comment',
    issue_id: 'issue-142',
    body: 'Should fail on read-only key',
    request_id: 'ro-1',
  });
  assert.equal(commentRefused.ok, false);
  assert.equal(commentRefused.error.code, 'linear_read_only_key');

  h.fake.readOnlyMode = 'http403';
  const stateRefused = await h.call({
    op: 'linear_update_issue_state',
    issue_id: 'issue-142',
    state_id: 'state-done',
    request_id: 'ro-2',
  });
  assert.equal(stateRefused.ok, false);
  assert.equal(stateRefused.error.code, 'linear_read_only_key');

  // Verify the key never appears in any reply or in host.sqlite bytes.
  assert(!h.transcript.join('\n').includes(h.fake.readOnlyKey));
  for (const suffix of ['', '-wal', '-shm']) {
    try {
      const bytes = await readFile(join(h.storeDir, `host.sqlite${suffix}`));
      assert(!bytes.includes(h.fake.readOnlyKey));
    } catch (error) {
      if ((error as { code?: string }).code !== 'ENOENT') throw error;
    }
  }
});

test('write operations enforce bounds, reject malformed inputs, and are not exposed as agent tools', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  // Exact 16 KiB body is accepted; 16 KiB + 1 byte is rejected.
  const exact = 'a'.repeat(LINEAR_COMMENT_BODY_MAX_BYTES);
  const okMax = await h.call({ op: 'linear_post_comment', issue_id: 'issue-142', body: exact, request_id: 'max-ok' });
  assert.equal(okMax.ok, true);
  const tooBig = 'a'.repeat(LINEAR_COMMENT_BODY_MAX_BYTES + 1);
  const badMax = await h.call({ op: 'linear_post_comment', issue_id: 'issue-142', body: tooBig, request_id: 'max-bad' });
  assert.equal(badMax.ok, false);
  assert.equal(badMax.error.code, 'invalid_request');

  for (const bad of [
    { op: 'linear_post_comment', issue_id: 'issue-142', body: '   ', request_id: 'r1' },
    { op: 'linear_post_comment', issue_id: 'issue-142', body: 'hi\u0000there', request_id: 'r2' },
    { op: 'linear_post_comment', issue_id: 'bad/id', body: 'hi', request_id: 'r3' },
    { op: 'linear_post_comment', issue_id: 'issue-142', body: 'hi', request_id: 'bad id' },
    { op: 'linear_post_comment', issue_id: 'issue-142', body: 'hi', request_id: 'r4', protocol_version: 6 },
    { op: 'linear_update_issue_state', issue_id: 'issue-142', state_id: 'bad/state', request_id: 'r5' },
    { op: 'linear_update_issue_state', issue_id: 'issue-142', state_id: 'state-done', request_id: 'r6', extra: true },
  ]) {
    const reply = await h.call(bad);
    assert.equal(reply.ok, false, JSON.stringify(bad));
    assert.equal(reply.error.code, 'invalid_request', JSON.stringify(bad));
  }

  // No agent tool in Runtime's tool registry touches Linear.
  const toolNames = h.runtime.registry({} as never).snapshot().tools().map(({ tool }) => tool.name);
  assert.deepEqual(toolNames.sort(), ['command', 'edit', 'read', 'read_output', 'write']);
});

test('in-flight write is never joined after sign-out or workspace switch', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);

  let releaseFirst!: () => void;
  const firstStarted = new Promise<void>(resolve => {
    h.fake.enqueueFault(async () => {
      resolve();
      await new Promise<void>(done => { releaseFirst = done; });
      return false;
    });
  });

  const p1 = h.call({ op: 'linear_post_comment', issue_id: 'issue-142', body: 'WS1 comment', request_id: 'cross-session-id' });
  await firstStarted;
  assert.equal((await h.call({ op: 'linear_sign_out' })).ok, true);

  // While signed out, reusing the same request_id must fail with linear_not_signed_in, not join p1.
  const p2 = h.call({ op: 'linear_post_comment', issue_id: 'issue-142', body: 'WS1 comment', request_id: 'cross-session-id' });
  releaseFirst();
  const [, whileSignedOut] = await Promise.all([p1, p2]);
  assert.equal(whileSignedOut.ok, false);
  assert.equal(whileSignedOut.error.code, 'linear_not_signed_in');
});

test('same-millisecond boundary eviction retains newest write, detail, and page entries', async t => {
  const h = await setup(t);
  const now = 1_763_500_000_000;

  // 1. linear_writes: 500 writes a-0000..a-0499 at `now`, then z-newest at the exact same `now`.
  for (let i = 0; i < 500; i++) {
    h.runtime.store.linear.saveWrite(`a-${String(i).padStart(4, '0')}`, 'linear_post_comment', 'h', { comment_id: `c-${i}` }, now);
  }
  h.runtime.store.linear.saveWrite('z-newest', 'linear_post_comment', 'h', { comment_id: 'c-new' }, now);
  assert.notEqual(h.runtime.store.linear.getWrite('z-newest'), null, 'newest write must not be evicted on same-ms tie');
  assert.equal(h.runtime.store.linear.getWrite('a-0000'), null, 'oldest write must be evicted');

  // 2. linear_cache_details: 200 details a-0000..a-0199 at `now`, touch a-0000, then insert z-newest at `now`.
  const template = {
    id: 'x', identifier: 'PLA-1', title: 'T', url: 'https://linear.app/a', priority: 0,
    updated_at: '2026-11-18T00:00:00Z', team_id: 't1', state: { id: 's1', name: 'S', type: 'started' },
    assignee: null, labels: [], description: null, attachments: [], comments: [],
    comments_has_more: false, comments_next_cursor: null,
  };
  for (let i = 0; i < 200; i++) {
    h.runtime.store.linear.saveIssueDetail({ ...template, id: `a-${String(i).padStart(4, '0')}` }, now);
  }
  // Touch a-0000 so it becomes MRU; a-0001 becomes LRU.
  assert.notEqual(h.runtime.store.linear.getIssueDetail('a-0000', now), null);
  h.runtime.store.linear.saveIssueDetail({ ...template, id: 'z-newest' }, now);
  assert.notEqual(h.runtime.store.linear.getIssueDetail('z-newest', now), null, 'newest detail must survive same-ms eviction');
  assert.notEqual(h.runtime.store.linear.getIssueDetail('a-0000', now), null, 'recently touched detail must survive LRU eviction');
  assert.equal(h.runtime.store.linear.getIssueDetail('a-0001', now), null, 'least-recently-used detail must be evicted');

  // 3. linear_cache_pages: 100 pages a-0000..a-0099 at `now`, then z-newest at `now`.
  for (let i = 0; i < 100; i++) {
    h.runtime.store.linear.saveIssuesPage(`a-${String(i).padStart(4, '0')}`, 't1', [], false, null, now);
  }
  h.runtime.store.linear.saveIssuesPage('z-newest', 't1', [], false, null, now);
  assert.notEqual(h.runtime.store.linear.getIssuesPage('z-newest', now), null, 'newest page must survive same-ms eviction');
  assert.equal(h.runtime.store.linear.getIssuesPage('a-0000', now), null, 'oldest page must be evicted');
});

test('linear_update_issue_state refreshes stale or missing team states before validating', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);
  assert.equal((await h.call({ op: 'linear_list_teams' })).ok, true);
  assert.equal((await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' })).ok, true);

  // 1. Add a new workflow state to team-pla and advance past the 15-minute teams TTL.
  h.fake.teams[0].states.push({ id: 'state-review', name: 'In Review', type: 'started', position: 5 });
  h.advance(16 * 60 * 1000);
  const movedNewState = await h.call({
    op: 'linear_update_issue_state',
    issue_id: 'issue-142',
    state_id: 'state-review',
    request_id: 'req-new-state',
  });
  assert.equal(movedNewState.ok, true, JSON.stringify(movedNewState));
  assert.equal(movedNewState.result.issue.state.id, 'state-review');

  // 2. Add a brand-new team and issue while the teams cache is fresh -> must refresh and discover it.
  h.fake.teams.push({
    id: 'team-new',
    key: 'NEW',
    name: 'New Team',
    states: [{ id: 'state-new-open', name: 'Open', type: 'unstarted', position: 0 }],
  });
  h.fake.issues.push({
    ...h.fake.issues[0],
    id: 'issue-new-team',
    identifier: 'NEW-1',
    teamId: 'team-new',
    stateId: 'state-new-open',
  });
  const movedNewTeam = await h.call({
    op: 'linear_update_issue_state',
    issue_id: 'issue-new-team',
    state_id: 'state-new-open',
    request_id: 'req-new-team',
  });
  assert.equal(movedNewTeam.ok, true, JSON.stringify(movedNewTeam));
  assert.equal(movedNewTeam.result.issue.state.id, 'state-new-open');
});

for (const op of ['linear_post_comment', 'linear_update_issue_state']) {
  test(`${op}: accepted with lost reply remains unknown through every retry path`, async t => {
    const h = await setup(t);
    await h.call({ op: 'linear_set_key', key: h.fake.validKey });
    const request = { op, issue_id: 'issue-142', request_id: 'lost-reply',
      ...(op === 'linear_post_comment' ? { body: 'Exactly once' } : { state_id: 'state-done' }) };
    h.fake.dropNextMutationReply = true;
    const first = await h.call(request);
    assert.equal(first.error.code, 'linear_write_unknown');
    assert.equal(first.error.outcome, 'unknown');
    assert.equal((await h.call({ op: 'linear_get_write', request_id: request.request_id })).result.write.outcome, 'unknown');
    assert.equal(h.runtime.store.linear.writes.get(request.request_id)?.payload, 'body' in request ? request.body : request.state_id);
    const mutations = () => h.fake.requests.filter(r => r.query.startsWith('mutation')).length;
    assert.equal(mutations(), 1);
    // Another person changes the state after our uncertain move.
    h.fake.issues[0].stateId = 'state-todo';
    for (const action of [
      async () => {},
      () => h.restart(),
      () => h.call({ op: 'linear_set_key', key: h.fake.readOnlyKey }),
      async () => {
        await h.call({ op: 'linear_sign_out' });
        const history = await h.call({ op: 'linear_get_write', request_id: request.request_id });
        assert.equal(history.result.write.outcome, 'unknown');
        await h.call({ op: 'linear_set_key', key: h.fake.validKey });
      },
      async () => { h.advance(365 * 24 * 60 * 60 * 1000); await h.restart(); },
    ]) {
      await action();
      assert.equal((await h.call(request)).error.outcome, 'unknown');
      assert.equal(mutations(), 1);
      assert.equal(h.fake.issues[0].stateId, 'state-todo');
    }
    const changed = { ...request, ...('body' in request ? { body: 'Changed' } : { state_id: 'state-progress' }) };
    assert.equal((await h.call(changed)).error.code, 'conflict');
    h.fake.organization = { id: 'org-2', name: 'Other', urlKey: 'other' };
    await h.call({ op: 'linear_set_key', key: h.fake.validKey });
    assert.equal((await h.call(request)).error.code, 'conflict');
    assert.equal(mutations(), 1);
  });
}

for (const crash of ['before-send', 'before-receipt', 'after-receipt']) {
  test(`reopening the store after a crash ${crash} preserves the write boundary`, async t => {
    const h = await setup(t);
    await h.call({ op: 'linear_set_key', key: h.fake.validKey });
    const request = { op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'crash', body: 'Crash window' };
    const journal = h.runtime.store.linear.writes;
    if (crash === 'before-send') {
      journal.claim = () => { throw new Error('Simulated process death'); };
    } else {
      const finish = journal.finish.bind(journal);
      journal.finish = (...args) => {
        if (crash === 'after-receipt') finish(...args);
        throw new Error('Simulated process death');
      };
    }
    assert.equal((await h.call(request)).error.code, 'internal');
    await h.restart();
    const history = (await h.call({ op: 'linear_get_write', request_id: 'crash' })).result.write;
    assert.equal(history.outcome, crash === 'before-send' ? 'not_sent' : crash === 'before-receipt' ? 'unknown' : 'accepted');
    const retry = await h.call(request);
    if (crash === 'before-receipt') assert.equal(retry.error.outcome, 'unknown');
    else assert.equal(retry.result.outcome, 'accepted');
    assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, 1);
    assert.equal(h.fake.issues[0].comments.length, 2);
  });
}

test('accepted tombstones survive 501 newer writes, sign-out, key replacement and restart', async t => {
  const h = await setup(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const request = { op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'oldest', body: 'Retain this receipt' };
  const first = await h.call(request);
  for (let i = 0; i < 501; i++) {
    assert.equal((await h.call({ ...request, request_id: `new-${i}` })).result.outcome, 'accepted');
  }
  assert.equal(h.runtime.store.linear.getWrite('oldest'), null);
  assert.equal((await h.call(request)).result.comment_id, first.result.comment_id);
  await h.call({ op: 'linear_set_key', key: h.fake.readOnlyKey });
  assert.equal((await h.call(request)).result.comment_id, first.result.comment_id);
  await h.call({ op: 'linear_sign_out' });
  const saved = (await h.call({ op: 'linear_get_write', request_id: 'oldest' })).result.write;
  assert.equal(saved.outcome, 'accepted');
  assert.equal(saved.receipt.comment_id, first.result.comment_id);
  assert.equal(saved.receipt.issue, null);
  await h.restart();
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  assert.equal((await h.call(request)).result.comment_id, first.result.comment_id);
  assert.equal((await h.call({ ...request, body: 'Changed' })).error.code, 'conflict');
  assert.equal(h.fake.requests.filter(r => r.query.includes('PlaitCommentCreate')).length, 502);
});

test('rejected and not-sent writes can retry; unknown responses and ambiguous errors cannot', async t => {
  const h = await setup(t);
  await h.call({ op: 'linear_set_key', key: h.fake.readOnlyKey });
  const request = { op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'rejected', body: 'Later allowed' };
  assert.equal((await h.call(request)).error.outcome, 'rejected');
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  assert.equal((await h.call(request)).result.outcome, 'accepted');
  const move = { op: 'linear_update_issue_state', issue_id: 'issue-142', request_id: 'not-sent', state_id: 'new-state' };
  assert.equal((await h.call(move)).error.outcome, 'not_sent');
  h.fake.teams[0].states.push({ id: 'new-state', name: 'New', type: 'started', position: 6 });
  assert.equal((await h.call(move)).result.outcome, 'accepted');
  for (const [i, envelope] of [
    {}, { data: { commentCreate: {} } },
    { errors: [{ extensions: { code: 'INTERNAL_SERVER_ERROR' } }] },
    { data: { commentCreate: null }, errors: [{ extensions: { code: 'FORBIDDEN' } }] },
  ].entries()) {
    h.fake.enqueueFault((_req, res) => { res.end(JSON.stringify(envelope)); return true; });
    const uncertain = { ...request, request_id: `uncertain-${i}` };
    assert.equal((await h.call(uncertain)).error.outcome, 'unknown');
    const before = h.fake.requests.length;
    assert.equal((await h.call(uncertain)).error.outcome, 'unknown');
    assert.equal(h.fake.requests.length, before);
  }
});

test('acceptance with failed selected fields or follow-up read is still accepted', async t => {
  const h = await setup(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  h.fake.enqueueFault((_req, res) => {
    res.end(JSON.stringify({ data: { commentCreate: { success: true, comment: null } }, errors: [{ extensions: { code: 'FORBIDDEN' } }] }));
    return true;
  });
  h.fake.enqueueFault((_req, res) => { res.writeHead(503); res.end(); return true; });
  const reply = await h.call({ op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'partial-accept', body: 'Accept' });
  assert.equal(reply.result.outcome, 'accepted');
  assert.equal(reply.result.issue, null);
  assert.match(reply.result.comment_id, /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/);
  assert.equal((await h.call({ op: 'linear_get_write', request_id: 'partial-accept' })).result.write.outcome, 'accepted');
});

test('history cap fails closed and the read-only capability never makes a request', async t => {
  const h = await setup(t);
  const info = await h.call({ op: 'linear_info' });
  assert.equal(info.result.features.durable_write_outcomes, true);
  assert.equal(h.fake.requests.length, 0);
  assert.deepEqual((await h.call({ op: 'linear_get_write', request_id: 'absent' })).result, { request_id: 'absent', write: null });
  assert.equal((await h.call({ op: 'linear_get_write', request_id: 'bad id' })).error.code, 'invalid_request');
  assert.equal((await h.call({ op: 'linear_info', protocol_version: 6 })).error.code, 'invalid_request');
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  h.runtime.store.db.exec(`WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<10000)
    INSERT INTO linear_write_intents(request_id,op,hash,outcome,created_at,updated_at)
    SELECT 'filled-' || i,'linear_post_comment','hash','unknown',0,0 FROM n`);
  const reply = await h.call({ op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'over-cap', body: 'Full' });
  assert.equal(reply.error.code, 'linear_write_history_full');
  assert.equal(h.fake.requests.filter(r => r.query.startsWith('mutation')).length, 0);
});

test('today\'s write table migrates without replaying existing receipts', async t => {
  const h = await setup(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const request = { op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'legacy', body: 'Legacy' };
  const first = await h.call(request);
  h.runtime.store.db.exec('DROP TABLE linear_write_intents');
  await h.restart();
  const saved = (await h.call({ op: 'linear_get_write', request_id: 'legacy' })).result.write;
  assert.equal(saved.outcome, 'accepted');
  assert.equal(saved.workspace_id, null);
  assert.equal((await h.call(request)).result.comment_id, first.result.comment_id);
  assert.equal((await h.call({ ...request, body: 'Different' })).error.code, 'conflict');
  assert.equal(h.fake.requests.filter(r => r.query.startsWith('mutation')).length, 1);
});

test('a crash after claiming but before sending remains unknown after reopen', async t => {
  const h = await setup(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const journal = h.runtime.store.linear.writes;
  const claim = journal.claim.bind(journal);
  journal.claim = (...args) => { claim(...args); throw new Error('Simulated death before fetch'); };
  const request = { op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'claim-gap', body: 'Uncertain' };
  await h.call(request);
  await h.restart();
  assert.equal((await h.call(request)).error.outcome, 'unknown');
  assert.equal(h.fake.requests.filter(r => r.query.startsWith('mutation')).length, 0);
});

test('a key change in flight cannot cause another mutation or lose its eventual receipt', async t => {
  const h = await setup(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  let release!: () => void;
  const started = new Promise<void>(resolve => {
    h.fake.enqueueFault(async () => { resolve(); await new Promise<void>(done => { release = done; }); return false; });
  });
  const request = { op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'key-race', body: 'Once' };
  const pending = h.call(request);
  await started;
  await h.call({ op: 'linear_set_key', key: h.fake.readOnlyKey });
  assert.equal((await h.call(request)).error.outcome, 'unknown');
  assert.equal((await h.call({ ...request, body: 'Different' })).error.code, 'conflict');
  release();
  assert.equal((await pending).result.outcome, 'accepted');
  await h.restart();
  assert.equal((await h.call(request)).result.outcome, 'accepted');
  assert.equal(h.fake.requests.filter(r => r.query.startsWith('mutation')).length, 1);
});

test('exact intent storage refuses credential-bearing write payloads', async t => {
  const h = await setup(t);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const result = await h.call({ op: 'linear_post_comment', issue_id: 'issue-142', request_id: 'secret', body: h.fake.validKey });
  assert.equal(result.error.code, 'invalid_request');
  assert.equal(result.error.outcome, 'not_sent');
  assert.equal(h.runtime.store.linear.writes.get('secret'), null);
  assert(!JSON.stringify(result).includes(h.fake.validKey));
});
