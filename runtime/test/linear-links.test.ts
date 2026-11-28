import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdir, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { LinearService } from '../src/linear/service.ts';
import { Runtime } from '../src/runtime.ts';
import { FakeLinearServer } from './support/linear-server.ts';

const execFileAsync = promisify(execFile);

async function setup(t: test.TestContext) {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-linear-links-'));
  const storeDir = join(dir, 'store');
  const repo = join(dir, 'repo');
  await mkdir(storeDir, { recursive: true, mode: 0o700 });
  await mkdir(repo, { recursive: true });
  await execFileAsync('git', ['init', '-q', repo]);
  const root = await realpath(repo);
  const fake = new FakeLinearServer();
  let runtime: Runtime;
  t.after(async () => {
    try { await runtime?.close(); }
    finally { await fake.close(); await rm(dir, { recursive: true, force: true }); }
  });
  const endpoint = await fake.start();
  let now = 1_763_400_000_000;
  const faux = fauxProvider();
  const models = createModels();
  models.setProvider(faux.provider);
  runtime = new Runtime(storeDir, models, new Set(['faux']));
  runtime.linear = new LinearService(runtime.store, storeDir, { endpoint, now: () => now });
  const call = (input: unknown): Promise<any> => runtime.reply(input);
  let seq = 0;
  const createThread = async () => {
    const reply = await call({
      op: 'create_thread', protocol_version: 6, checkout: root,
      provider: 'faux', model_id: faux.models[0].id, request_id: `create-${++seq}`,
    });
    assert.equal(reply.ok, true, JSON.stringify(reply));
    return String(reply.result.thread.id);
  };
  return {
    root, fake, runtime, call, createThread,
    get now() { return now; },
    advance(ms: number) { now += ms; },
  };
}

test('thread links are local to Plait, persist across operations, emit no notifications, and leave ThreadSummary/Snapshot unchanged', async t => {
  const h = await setup(t);
  const t1 = await h.createThread();
  const notifications: string[] = [];
  const unsub = h.runtime.subscribe(id => notifications.push(id));
  t.after(unsub);

  // Works while signed out and never contacts Linear.
  const linked = await h.call({
    op: 'linear_link_thread',
    thread_id: t1,
    issue_id: 'issue-142',
    identifier: 'PLA-142',
    url: 'https://linear.app/acme/issue/PLA-142/fix-retry',
    title: 'Fix retry\u202e backoff',
  });
  assert.equal(linked.ok, true);
  assert.deepEqual(linked.result.link, {
    thread_id: t1,
    workspace_id: null,
    historical: true,
    issue_id: 'issue-142',
    identifier: 'PLA-142',
    url: 'https://linear.app/acme/issue/PLA-142/fix-retry',
    title: 'Fix retry\\u202e backoff',
    linked_at: h.now,
  });
  assert.equal(h.fake.requests.length, 0, 'linking must not write to or contact Linear');
  assert.deepEqual(notifications, [], 'linking must not emit change notifications in this stage');

  // Read back single link.
  const read = await h.call({ op: 'linear_get_thread_link', thread_id: t1 });
  assert.equal(read.ok, true);
  assert.deepEqual(read.result, { thread_id: t1, link: linked.result.link });

  // Existing ThreadSummary and Snapshot wire shapes stay completely closed and unchanged.
  for (const version of [3, 4, 5, 6]) {
    const listed = await h.call({ op: 'list_threads', protocol_version: version });
    assert.equal(listed.ok, true);
    assert.equal('linear_issue' in listed.result.threads[0], false);
    const watched = await h.call({ op: 'watch', protocol_version: version, thread_id: t1 });
    assert.equal(watched.ok, true);
    assert.equal('linear_issue' in watched.result.thread, false);
  }
  // Extra flag on list_threads / watch_threads is still rejected.
  assert.equal((await h.call({ op: 'list_threads', protocol_version: 6, include_linear: true })).error?.code, 'invalid_request');

  // Replacing a thread's link updates it cleanly; unlinking removes it.
  h.advance(1000);
  const replaced = await h.call({
    op: 'linear_link_thread',
    thread_id: t1,
    issue_id: 'issue-143',
    identifier: 'PLA-143',
    url: 'https://linear.app/acme/issue/PLA-143/other',
    title: 'Other issue',
  });
  assert.equal(replaced.result.link.identifier, 'PLA-143');
  assert.equal((await h.call({ op: 'linear_get_thread_link', thread_id: t1 })).result.link.identifier, 'PLA-143');

  const unlinked = await h.call({ op: 'linear_unlink_thread', thread_id: t1 });
  assert.deepEqual(unlinked.result, { thread_id: t1, removed: true });
  assert.deepEqual((await h.call({ op: 'linear_unlink_thread', thread_id: t1 })).result, { thread_id: t1, removed: false });
  assert.deepEqual((await h.call({ op: 'linear_get_thread_link', thread_id: t1 })).result, { thread_id: t1, link: null });
  assert.equal(h.fake.requests.length, 0);
  assert.deepEqual(notifications, []);
});

test('linear_list_thread_links returns every link in stable order with bounded paging, and deleting a thread removes its link', async t => {
  const h = await setup(t);
  const threads: string[] = [];
  for (let i = 0; i < 5; i++) {
    const tid = await h.createThread();
    threads.push(tid);
    // Give two threads the same timestamp to verify deterministic tie-breaking by thread_id ASC.
    if (i !== 2) h.advance(1000);
    const res = await h.call({
      op: 'linear_link_thread',
      thread_id: tid,
      issue_id: `issue-${i}`,
      identifier: `PLA-${100 + i}`,
      url: `https://linear.app/acme/issue/PLA-${100 + i}`,
      title: `Issue ${i}`,
    });
    assert.equal(res.ok, true);
  }

  // Page 2 items at a time across all 5 links.
  const collected: Array<{ thread_id: string; identifier: string; linked_at: number }> = [];
  let cursor: string | undefined;
  let pages = 0;
  do {
    const page = await h.call({ op: 'linear_list_thread_links', limit: 2, ...(cursor ? { cursor } : {}) });
    assert.equal(page.ok, true);
    assert(page.result.links.length <= 2);
    collected.push(...page.result.links);
    cursor = page.result.next_cursor ?? undefined;
    pages++;
  } while (cursor);
  assert.equal(pages, 3);
  assert.equal(collected.length, 5);
  // Verify descending linked_at, ascending thread_id tie-breaker.
  for (let i = 1; i < collected.length; i++) {
    const prev = collected[i - 1];
    const curr = collected[i];
    assert(prev.linked_at > curr.linked_at || (prev.linked_at === curr.linked_at && prev.thread_id < curr.thread_id));
  }

  // Deleting a thread row removes its link automatically via the SQLite trigger.
  const deletedThread = threads[2];
  h.runtime.store.db.prepare('DELETE FROM threads WHERE id=?').run(deletedThread);
  const remaining = await h.call({ op: 'linear_list_thread_links' });
  assert.equal(remaining.result.links.length, 4);
  assert(!remaining.result.links.some((link: { thread_id: string }) => link.thread_id === deletedThread));
  assert.equal(h.runtime.store.linear.getThreadLink(deletedThread), null);
});

test('thread link operations validate inputs and reject unknown threads', async t => {
  const h = await setup(t);
  const t1 = await h.createThread();

  for (const op of [
    { op: 'linear_link_thread', thread_id: 'missing', issue_id: 'i1', identifier: 'PLA-1', url: 'https://linear.app/a/issue/PLA-1', title: 'Hi' },
    { op: 'linear_unlink_thread', thread_id: 'missing' },
    { op: 'linear_get_thread_link', thread_id: 'missing' },
  ]) {
    const reply = await h.call(op);
    assert.equal(reply.ok, false);
    assert.equal(reply.error.code, 'not_found');
  }

  for (const bad of [
    { op: 'linear_link_thread', thread_id: t1, issue_id: 'i1', identifier: 'not-an-id', url: 'https://linear.app/a', title: 'Hi' },
    { op: 'linear_link_thread', thread_id: t1, issue_id: 'i1', identifier: 'PLA-1', url: 'http://linear.app/a', title: 'Hi' },
    { op: 'linear_link_thread', thread_id: t1, issue_id: 'i1', identifier: 'PLA-1', url: 'javascript:alert(1)', title: 'Hi' },
    { op: 'linear_link_thread', thread_id: t1, issue_id: 'i1', identifier: 'PLA-1', url: 'https://u:p@linear.app/a', title: 'Hi' },
    { op: 'linear_link_thread', thread_id: t1, issue_id: 'i1', identifier: 'PLA-1', url: 'https://linear.app/a', title: '   ' },
    { op: 'linear_link_thread', thread_id: t1, issue_id: 'i1', identifier: 'PLA-1', url: 'https://linear.app/a', title: 'x'.repeat(600) },
    { op: 'linear_link_thread', thread_id: t1, issue_id: 'i1', identifier: 'PLA-1', url: 'https://linear.app/a', title: 'Hi', protocol_version: 6 },
    { op: 'linear_list_thread_links', limit: 0 },
    { op: 'linear_list_thread_links', limit: 501 },
    { op: 'linear_list_thread_links', cursor: 'bad-cursor' },
    { op: 'linear_list_thread_links', extra: true },
  ]) {
    const reply = await h.call(bad);
    assert.equal(reply.ok, false, JSON.stringify(bad));
    assert.equal(reply.error.code, 'invalid_request', JSON.stringify(bad));
  }
});

test('workspace-scoped links remain history on sign-out and never borrow another workspace state', async t => {
  const h = await setup(t);
  const thread = await h.createThread();
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const request = { op: 'linear_link_thread', thread_id: thread, issue_id: 'issue-142', identifier: 'PLA-142',
    url: 'https://linear.app/acme/issue/PLA-142', title: 'Fix retry', workspace_id: 'org-1' };
  const linked = await h.call(request);
  assert.equal(linked.result.link.workspace_id, 'org-1');
  assert.equal(linked.result.link.historical, false);
  assert.equal((await h.call({ op: 'linear_get_linked_issue', thread_id: thread })).result.issue.id, 'issue-142');
  assert.equal((await h.call({ ...request, workspace_id: 'org-2' })).error.code, 'conflict');
  assert.equal((await h.call({ ...request, url: 'https://linear.app/other/issue/PLA-142' })).error.code, 'conflict');
  await h.call({ op: 'linear_sign_out' });
  assert.equal((await h.call({ op: 'linear_get_thread_link', thread_id: thread })).result.link.historical, true);
  assert.equal((await h.call({ op: 'linear_list_thread_links' })).result.links.length, 1);
  let count = h.fake.requests.length;
  assert.equal((await h.call({ op: 'linear_get_linked_issue', thread_id: thread })).error.code, 'conflict');
  assert.equal(h.fake.requests.length, count);
  h.fake.organization = { id: 'org-2', name: 'Other', urlKey: 'other' };
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  count = h.fake.requests.length;
  assert.equal((await h.call({ op: 'linear_get_linked_issue', thread_id: thread })).error.code, 'conflict');
  assert.equal(h.fake.requests.length, count);
  assert.equal((await h.call({ op: 'linear_get_thread_link', thread_id: thread })).result.link.workspace_id, 'org-1');
  assert.equal((await h.call({ op: 'linear_unlink_thread', thread_id: thread })).result.removed, true);
});

test('legacy links are historical, old requests still work, and link changes fence live replies', async t => {
  const h = await setup(t);
  const thread = await h.createThread();
  // A row written by the old schema contains no workspace identity.
  h.runtime.store.db.prepare('INSERT INTO linear_thread_links(thread_id,issue_id,identifier,url,title,linked_at) VALUES (?,?,?,?,?,?)')
    .run(thread, 'issue-142', 'PLA-142', 'https://linear.app/acme/issue/PLA-142', 'Legacy', h.now);
  await h.call({ op: 'linear_set_key', key: h.fake.validKey });
  const legacy = (await h.call({ op: 'linear_get_thread_link', thread_id: thread })).result.link;
  assert.equal(legacy.workspace_id, null);
  assert.equal(legacy.historical, true);
  assert.equal((await h.call({ op: 'linear_get_linked_issue', thread_id: thread })).error.code, 'conflict');
  const request = { op: 'linear_link_thread', thread_id: thread, issue_id: 'issue-142', identifier: 'PLA-142',
    url: 'https://linear.app/acme/issue/PLA-142', title: 'Relinked' };
  assert.equal((await h.call(request)).result.link.workspace_id, 'org-1');
  let release!: () => void;
  const started = new Promise<void>(resolve => h.fake.enqueueFault(async () => {
    resolve(); await new Promise<void>(done => { release = done; }); return false;
  }));
  const pending = h.call({ op: 'linear_get_linked_issue', thread_id: thread, refresh: true });
  await started;
  await h.call({ op: 'linear_unlink_thread', thread_id: thread });
  release();
  assert.equal((await pending).error.code, 'conflict');
  assert.equal((await h.call({ ...request, url: 'https://example.invalid/old-url' })).result.link.workspace_id, null);
});
