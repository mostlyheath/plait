import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createModels } from '@earendil-works/pi-ai/models';
import { LINEAR_IMAGE_CHUNK, LinearService } from '../src/linear/service.ts';
import { LINEAR_DESCRIPTION_MAX_BYTES } from '../src/linear/types.ts';
import { Runtime } from '../src/runtime.ts';
import { FakeLinearServer, TINY_PNG } from './support/linear-server.ts';

const IMAGE = 'https://uploads.linear.app/org-1/png';

async function setup(t: test.TestContext) {
  const dir = await mkdtemp(join(tmpdir(), 'plait-linear-issue-writes-'));
  const storeDir = join(dir, 'store');
  await mkdir(storeDir, { recursive: true, mode: 0o700 });
  const fake = new FakeLinearServer();
  const endpoint = await fake.start();
  const imageOrigin = endpoint.replace(/\/graphql$/, '');
  let runtime = new Runtime(storeDir, createModels(), new Set());
  const attach = (target: Runtime) => {
    target.linear = new LinearService(target.store, storeDir, { endpoint, imageOrigin, timeoutMs: 1000 });
  };
  attach(runtime);
  t.after(async () => {
    await runtime.close();
    await fake.close();
    await rm(dir, { recursive: true, force: true });
  });
  const call = (input: unknown): Promise<any> => runtime.reply(input);
  assert.equal((await call({ op: 'linear_set_key', key: fake.validKey })).ok, true);
  const restart = async () => {
    await runtime.close();
    runtime = new Runtime(storeDir, createModels(), new Set());
    attach(runtime);
  };
  return { storeDir, fake, call, restart };
}

const mutations = (fake: FakeLinearServer, name: string) =>
  fake.requests.filter(r => r.query.includes(name)).length;

test('linear_info reports issue writes and images', async t => {
  const h = await setup(t);
  const info = await h.call({ op: 'linear_info' });
  assert.equal(info.result.features.issue_writes, true);
  assert.equal(info.result.features.images, true);
});

test('linear_create_issue creates once in the team, reads it back, and replays its receipt', async t => {
  const h = await setup(t);
  const request = { op: 'linear_create_issue', team_id: 'team-pla', title: '  Show images in issues  ',
    description: '## Why\n\nThey carry the context.', request_id: 'create-1' };
  const created = await h.call(request);
  assert.equal(created.ok, true, JSON.stringify(created));
  assert.equal(created.result.outcome, 'accepted');
  assert.equal(created.result.issue.title, 'Show images in issues');
  assert.equal(created.result.issue.description, '## Why\n\nThey carry the context.');
  assert.equal(created.result.issue.team_id, 'team-pla');
  assert.equal(created.result.issue_id, created.result.issue.id);
  assert.match(created.result.issue_id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  assert.equal(mutations(h.fake, 'PlaitIssueCreate'), 1);

  // The same request again, even after a restart, returns the receipt and sends nothing.
  await h.restart();
  const again = await h.call(request);
  assert.equal(again.ok, true);
  assert.equal(again.result.issue_id, created.result.issue_id);
  assert.equal(mutations(h.fake, 'PlaitIssueCreate'), 1);

  const conflict = await h.call({ ...request, title: 'Something else' });
  assert.equal(conflict.ok, false);
  assert.equal(conflict.error.code, 'conflict');
});

test('linear_create_issue refuses another team\'s state before sending', async t => {
  const h = await setup(t);
  const reply = await h.call({ op: 'linear_create_issue', team_id: 'team-pla', title: 'Wrong state',
    state_id: 'state-ops-done', request_id: 'create-bad-state' });
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'invalid_request');
  assert.equal(mutations(h.fake, 'PlaitIssueCreate'), 0);
});

test('a lost create reply is unknown and is never sent again', async t => {
  const h = await setup(t);
  h.fake.dropNextMutationReply = true;
  const request = { op: 'linear_create_issue', team_id: 'team-pla', title: 'Lost', request_id: 'create-lost' };
  const first = await h.call(request);
  assert.equal(first.ok, false);
  assert.equal(first.error.code, 'linear_write_unknown');
  const second = await h.call(request);
  assert.equal(second.error.code, 'linear_write_unknown');
  assert.equal(mutations(h.fake, 'PlaitIssueCreate'), 1);
});

test('linear_update_issue edits title and description when the issue is unchanged', async t => {
  const h = await setup(t);
  const read = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' });
  const expected = read.result.issue.updated_at;
  const edited = await h.call({ op: 'linear_update_issue', issue_id: 'issue-142', expected_updated_at: expected,
    title: 'Retry 429s with backoff', description: 'Use **Retry-After**.', request_id: 'edit-1' });
  assert.equal(edited.ok, true, JSON.stringify(edited));
  assert.equal(edited.result.verification, 'matched');
  assert.equal(edited.result.issue.title, 'Retry 429s with backoff');
  assert.equal(edited.result.issue.description, 'Use **Retry-After**.');
  assert.notEqual(edited.result.issue.updated_at, expected);
  assert.equal(mutations(h.fake, 'PlaitIssueUpdateText'), 1);

  // Only the description: the title is left as it is.
  const described = await h.call({ op: 'linear_update_issue', issue_id: 'issue-142',
    expected_updated_at: edited.result.issue.updated_at, description: 'Shorter.', request_id: 'edit-2' });
  assert.equal(described.ok, true);
  assert.equal(described.result.issue.title, 'Retry 429s with backoff');
});

test('an accepted edit compares the live text and replays the discrepancy without writing again', async t => {
  const h = await setup(t);
  const issue = h.fake.issues.find(i => i.id === 'issue-142')!;
  const request = { op: 'linear_update_issue', issue_id: issue.id, expected_updated_at: issue.updatedAt,
    title: 'My title', request_id: 'edit-race' };
  h.fake.enqueueFault(() => false); // Timestamp check.
  h.fake.enqueueFault(() => false); // Mutation.
  h.fake.enqueueFault((_req, _res, parsed) => {
    assert.match(parsed.query, /PlaitIssueDetail/);
    issue.title = 'Another title';
    return false;
  });
  const result = (await h.call(request)).result;
  assert.equal(result.outcome, 'accepted');
  assert.equal(result.verification, 'different');
  assert.equal(result.issue.title, 'Another title');
  assert.match(result.warning, /someone else/i);
  await h.restart();
  assert.deepEqual((await h.call(request)).result, result);
  assert.equal(mutations(h.fake, 'PlaitIssueUpdateText'), 1);
});

test('a failed post-edit refresh leaves the save accepted', async t => {
  const h = await setup(t);
  const issue = h.fake.issues.find(i => i.id === 'issue-142')!;
  h.fake.enqueueFault(() => false);
  h.fake.enqueueFault(() => false);
  h.fake.enqueueFault((_req, res) => {
    res.writeHead(503).end();
    return true;
  });
  const reply = await h.call({ op: 'linear_update_issue', issue_id: issue.id,
    expected_updated_at: issue.updatedAt, description: 'Mine', request_id: 'edit-unverified' });
  assert.equal(reply.result.outcome, 'accepted');
  assert.equal(reply.result.issue, null);
  assert.equal(reply.result.verification, 'unverified');
  assert.match(reply.result.warning, /could not/i);
  assert.equal(issue.description, 'Mine');
  assert.equal(mutations(h.fake, 'PlaitIssueUpdateText'), 1);
});

test('linear_update_issue refuses an issue that changed since it was read', async t => {
  const h = await setup(t);
  const reply = await h.call({ op: 'linear_update_issue', issue_id: 'issue-142',
    expected_updated_at: '2020-01-01T00:00:00.000Z', title: 'Stale edit', request_id: 'edit-stale' });
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'linear_issue_changed');
  assert.equal(reply.error.outcome, 'not_sent');
  assert.equal(mutations(h.fake, 'PlaitIssueUpdateText'), 0);
});

test('write bounds: titles, descriptions, empty edits, and unknown fields are refused', async t => {
  const h = await setup(t);
  for (const bad of [
    { op: 'linear_create_issue', team_id: 'team-pla', title: '   ', request_id: 'b1' },
    { op: 'linear_create_issue', team_id: 'team-pla', title: 'x'.repeat(513), request_id: 'b2' },
    { op: 'linear_create_issue', team_id: 'team-pla', title: 'ok', description: 'a\u0000b', request_id: 'b3' },
    { op: 'linear_create_issue', team_id: 'team-pla', title: 'ok', description: 'a'.repeat(LINEAR_DESCRIPTION_MAX_BYTES + 1), request_id: 'b4' },
    { op: 'linear_create_issue', team_id: 'bad/team', title: 'ok', request_id: 'b5' },
    { op: 'linear_update_issue', issue_id: 'issue-142', expected_updated_at: 'x', request_id: 'b6' },
    { op: 'linear_update_issue', issue_id: 'issue-142', expected_updated_at: 'x', title: 'ok', request_id: 'b7', extra: 1 },
  ]) {
    const reply = await h.call(bad);
    assert.equal(reply.ok, false, JSON.stringify(bad).slice(0, 120));
    assert.equal(reply.error.code, 'invalid_request', JSON.stringify(bad).slice(0, 120));
  }
  assert.equal(mutations(h.fake, 'PlaitIssueCreate') + mutations(h.fake, 'PlaitIssueUpdateText'), 0);
});

test('linear_read_image reads Linear storage with the key, in chunks, from the cache after the first read', async t => {
  const h = await setup(t);
  const big = Buffer.concat([TINY_PNG, Buffer.alloc(LINEAR_IMAGE_CHUNK + 10)]);
  h.fake.images.set('big', big);
  const first = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/big', offset: 0 });
  assert.equal(first.ok, true, JSON.stringify(first).slice(0, 200));
  assert.equal(first.result.size, big.length);
  assert.equal(Buffer.from(first.result.data, 'base64').length, LINEAR_IMAGE_CHUNK);
  const second = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/big', offset: LINEAR_IMAGE_CHUNK });
  assert.deepEqual(Buffer.concat([Buffer.from(first.result.data, 'base64'), Buffer.from(second.result.data, 'base64')]), big);
  assert.deepEqual(h.fake.imageAuthorizations, [h.fake.validKey], 'one fetch, with the key');
  // The bytes are never written as text into host.sqlite's request log or replies beyond the image.
  const sqlite = await readFile(join(h.storeDir, 'host.sqlite'));
  assert.equal(sqlite.includes(Buffer.from(h.fake.validKey)), false);
});

test('linear_read_image follows one redirect without the key', async t => {
  const h = await setup(t);
  const reply = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/redirect', offset: 0 });
  assert.equal(reply.ok, true, JSON.stringify(reply));
  assert.deepEqual(Buffer.from(reply.result.data, 'base64'), TINY_PNG);
  assert.deepEqual(h.fake.imageAuthorizations, [h.fake.validKey, undefined]);
});

test('linear_read_image refuses other hosts, non-images, and missing files without marking the key rejected', async t => {
  const h = await setup(t);
  for (const url of ['https://example.com/a.png', 'http://uploads.linear.app/a', 'https://uploads.linear.app:444/a',
    'https://user:pw@uploads.linear.app/a', 'https://uploads.linear.app/']) {
    const reply = await h.call({ op: 'linear_read_image', url, offset: 0 });
    assert.equal(reply.error.code, 'invalid_request', url);
  }
  h.fake.images.set('gif', Buffer.from('GIF89a\u0001\u0000\u0001\u0000', 'latin1'));
  const gif = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/gif', offset: 0 });
  assert.equal(gif.ok, false);
  assert.equal(gif.error.code, 'invalid_request');
  const missing = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/none', offset: 0 });
  assert.equal(missing.error.code, 'not_found');
  const status = await h.call({ op: 'linear_auth_status' });
  assert.equal(status.result.status, 'signed_in');
  const beyond = await h.call({ op: 'linear_read_image', url: IMAGE, offset: TINY_PNG.length });
  assert.equal(beyond.error.code, 'invalid_request');
});

test('sign-out clears cached images', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_read_image', url: IMAGE, offset: 0 })).ok, true);
  await h.call({ op: 'linear_sign_out' });
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);
  assert.equal((await h.call({ op: 'linear_read_image', url: IMAGE, offset: 0 })).ok, true);
  assert.equal(h.fake.imageAuthorizations.length, 2, 'read again after sign-out');
});

test('an image redirect to any other host is refused before connecting', async t => {
  const h = await setup(t);
  let awayHits = 0;
  const away = createServer((_req, res) => { awayHits++; res.end(); });
  await new Promise<void>(resolve => away.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise<void>(resolve => away.close(() => resolve())));
  h.fake.awayOrigin = `http://127.0.0.1:${(away.address() as AddressInfo).port}`;
  const reply = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/redirect-away', offset: 0 });
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'linear_offline');
  assert.equal(awayHits, 0, 'the other host received nothing');
});

test('a description-only edit leaves the title out of the request', async t => {
  const h = await setup(t);
  const read = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142' });
  const reply = await h.call({ op: 'linear_update_issue', issue_id: 'issue-142',
    expected_updated_at: read.result.issue.updated_at, description: 'Only this.', request_id: 'desc-only' });
  assert.equal(reply.ok, true, JSON.stringify(reply));
  const sent = h.fake.requests.filter(r => r.query.includes('PlaitIssueUpdateText')).at(-1)!;
  assert.deepEqual(Object.keys(sent.variables).sort(), ['description', 'id']);
});

test('linear_get_issue_text returns the exact text an edit starts from', async t => {
  const h = await setup(t);
  const long = '漢'.repeat(200);
  h.fake.issues.find(issue => issue.id === 'issue-142')!.title = long;
  const display = await h.call({ op: 'linear_get_issue', issue_id: 'issue-142', refresh: true });
  assert.ok(display.result.issue.title.length < 200, 'the display read is bounded');
  const text = await h.call({ op: 'linear_get_issue_text', issue_id: 'issue-142' });
  assert.equal(text.ok, true, JSON.stringify(text));
  assert.equal(text.result.editable, true);
  assert.equal(text.result.title, long);
  assert.equal(text.result.description, 'Honor Retry-After and reset headers when rate limited.');

  // Text Plait could not write back unchanged is offered for editing in Linear only.
  h.fake.issues.find(issue => issue.id === 'issue-142')!.description = 'x'.repeat(LINEAR_DESCRIPTION_MAX_BYTES + 1);
  const tooLong = await h.call({ op: 'linear_get_issue_text', issue_id: 'issue-142' });
  assert.equal(tooLong.result.editable, false);
  assert.equal(tooLong.result.title, null);
  assert.equal(tooLong.result.description, null);
});

test('cached images need the current sign-in, survive nothing else, and follow the key', async t => {
  const h = await setup(t);
  assert.equal((await h.call({ op: 'linear_read_image', url: IMAGE, offset: 0 })).ok, true);
  // A crash after the key file is removed, before the cache is cleared.
  await rm(join(h.storeDir, 'linear-auth.json'));
  await h.restart();
  const signedOut = await h.call({ op: 'linear_read_image', url: IMAGE, offset: 0 });
  assert.equal(signedOut.ok, false);
  assert.equal(signedOut.error.code, 'linear_not_signed_in');
  // A new key never reads the earlier key's images from the cache.
  assert.equal((await h.call({ op: 'linear_set_key', key: h.fake.validKey })).ok, true);
  assert.equal((await h.call({ op: 'linear_read_image', url: IMAGE, offset: 0 })).ok, true);
  assert.equal(h.fake.imageAuthorizations.length, 2);
});

test('image limits: streamed and decompressed size, pixels, rate limits, and the cache bound', async t => {
  const h = await setup(t);
  for (const name of ['streamed-huge', 'gzip-bomb']) {
    const reply = await h.call({ op: 'linear_read_image', url: `https://uploads.linear.app/org-1/${name}`, offset: 0 });
    assert.equal(reply.ok, false, name);
    assert.equal(reply.error.code, 'linear_offline', name);
  }
  const huge = Buffer.from(TINY_PNG);
  huge.writeUInt32BE(9000, 16);
  huge.writeUInt32BE(9000, 20);
  h.fake.images.set('pixels', huge);
  const pixels = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/pixels', offset: 0 });
  assert.equal(pixels.error.code, 'invalid_request');
  const limited = await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/limited', offset: 0 });
  assert.equal(limited.error.code, 'linear_rate_limited');
  assert.equal(limited.error.retry_after_seconds, 7);

  // 65 images: the first read falls out of the 64-image cache and is read again.
  h.fake.imageAuthorizations = [];
  for (let i = 0; i < 65; i++) {
    h.fake.images.set(`many-${i}`, Buffer.concat([TINY_PNG, Buffer.from([i])]));
    assert.equal((await h.call({ op: 'linear_read_image', url: `https://uploads.linear.app/org-1/many-${i}`, offset: 0 })).ok, true);
  }
  assert.equal(h.fake.imageAuthorizations.length, 65);
  await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/many-64', offset: 0 });
  assert.equal(h.fake.imageAuthorizations.length, 65, 'the newest is cached');
  await h.call({ op: 'linear_read_image', url: 'https://uploads.linear.app/org-1/many-0', offset: 0 });
  assert.equal(h.fake.imageAuthorizations.length, 66, 'the oldest was evicted');
});

test('an incomplete exact read is never taken as empty text', async t => {
  const h = await setup(t);
  for (const fault of ['errors', 'no_description'] as const) {
    h.fake.issueTextFault = fault;
    const reply = await h.call({ op: 'linear_get_issue_text', issue_id: 'issue-142' });
    assert.equal(reply.ok, true, JSON.stringify(reply));
    assert.equal(reply.result.editable, false, fault);
    assert.equal(reply.result.description, null, fault);
  }
  h.fake.issueTextFault = null;
  h.fake.issues.find(issue => issue.id === 'issue-142')!.description = null;
  const empty = await h.call({ op: 'linear_get_issue_text', issue_id: 'issue-142' });
  assert.equal(empty.result.editable, true, 'Linear stores an empty description as null');
  assert.equal(empty.result.description, '');
});
