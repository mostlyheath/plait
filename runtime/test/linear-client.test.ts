import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { chmod, readFile, rm, stat, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { LinearAuthStore, formatAuthStatus, validateLinearKey } from '../src/linear/auth.ts';
import { LinearClient, parseRetryAfterSeconds } from '../src/linear/client.ts';
import { FIELD_LIMITS, LINEAR_GRAPHQL_URL, LinearError } from '../src/linear/types.ts';
import { FakeLinearServer } from './support/linear-server.ts';

test('LinearAuthStore writes mode 0600, repairs loose modes, refuses symlinks, and never leaks the key', async t => {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-linear-auth-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const authStore = new LinearAuthStore(dir);
  const secret = 'lin_api_SUPER_SECRET_NEVER_LEAK_42';

  assert.equal(await authStore.read(), null);
  assert.deepEqual(formatAuthStatus(null), {
    signed_in: false, status: 'signed_out',
    workspace_id: null, workspace_name: null, workspace_url_key: null,
    viewer_id: null, viewer_name: null,
  });

  await authStore.write({
    api_key: secret,
    workspace_id: 'org-1',
    workspace_name: 'Acme',
    workspace_url_key: 'acme',
    viewer_id: 'viewer-1',
    viewer_name: 'Alex',
    saved_at: 1000,
  });

  const filePath = join(dir, 'linear-auth.json');
  const mode = (await stat(filePath)).mode & 0o777;
  assert.equal(mode, 0o600, `expected 0600, got ${mode.toString(8)}`);

  const loaded = await authStore.read();
  assert.equal(loaded?.api_key, secret);
  const status = formatAuthStatus(loaded);
  assert.equal(status.signed_in, true);
  assert.equal(status.workspace_name, 'Acme');
  assert.equal(status.viewer_name, 'Alex');
  assert(!JSON.stringify(status).includes(secret), 'status must never contain the key');

  // If permissions are loosened externally, reading restores 0600.
  await chmod(filePath, 0o644);
  await authStore.read();
  assert.equal((await stat(filePath)).mode & 0o777, 0o600);

  await authStore.remove();
  assert.equal(await authStore.read(), null);
  // Second remove is idempotent.
  await authStore.remove();

  // Symlink at linear-auth.json is refused without disclosing path.
  await symlink(join(dir, 'other'), filePath);
  await assert.rejects(() => authStore.read(), (error: Error) => !error.message.includes(dir));
  await assert.rejects(() => authStore.write({
    api_key: secret, workspace_id: 'org-1', workspace_name: 'Acme',
    workspace_url_key: 'acme', viewer_id: 'viewer-1', viewer_name: 'Alex', saved_at: 1000,
  }), (error: Error) => !error.message.includes(dir));
});

test('validateLinearKey trims and rejects malformed keys without echoing them', () => {
  const secret = 'lin_api_SECRET_WITH_\u0007_BELL';
  assert.equal(validateLinearKey('  lin_api_ok_123 \n'), 'lin_api_ok_123');
  for (const bad of ['', '   ', 42, null, 'has space', secret, 'x'.repeat(5000)]) {
    assert.throws(() => validateLinearKey(bad), (error: Error) => !error.message.includes('SECRET_WITH'));
  }
});

test('LinearClient sends raw Authorization key (no Bearer), checks identity, and escapes/bounds strings', async t => {
  const fake = new FakeLinearServer();
  const endpoint = await fake.start();
  t.after(() => fake.close());

  // Inject bidi control characters and oversized strings from Linear.
  fake.viewer.name = `Alex \u202eSpoof\u0000 ${'A'.repeat(400)}`;
  fake.organization.name = `Acme\u2028Corp`;

  const client = new LinearClient({ endpoint });
  assert.equal(LINEAR_GRAPHQL_URL, 'https://api.linear.app/graphql');
  const identity = await client.viewer(fake.validKey);
  assert.equal(fake.requests.length, 1);
  assert.equal(fake.requests[0].authorization, fake.validKey);
  assert(!fake.requests[0].authorization!.startsWith('Bearer '), 'API key must not use Bearer prefix');
  assert.equal(identity.data.workspace_id, 'org-1');
  assert.equal(identity.data.workspace_name, 'Acme\\u2028Corp');
  assert(identity.data.viewer_name.includes('\\u202e'));
  assert(identity.data.viewer_name.includes('\\u0000'));
  assert(identity.data.viewer_name.length <= FIELD_LIMITS.name);
  assert(!JSON.stringify(identity).includes(fake.validKey));
});

test('LinearClient maps rejected keys, read-only keys, 429 headers, timeouts, redirects, oversized bodies, and partial GraphQL data', async t => {
  const fake = new FakeLinearServer();
  const endpoint = await fake.start();
  t.after(() => fake.close());

  let now = 1_700_000_000_000;
  const client = new LinearClient({ endpoint, timeoutMs: 250, maxResponseBytes: 4096, now: () => now });

  // 1. Rejected key (401)
  await assert.rejects(() => client.viewer('lin_api_WRONG_KEY_999'), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'key_rejected');
    assert.equal(error.code, 'linear_key_rejected');
    assert(!error.message.includes('lin_api_WRONG_KEY_999'));
    return true;
  });

  // 2. Rejected key via GraphQL AUTHENTICATION_ERROR on HTTP 200
  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ data: null, errors: [{ message: 'RAW_AUTH_SECRET', extensions: { code: 'AUTHENTICATION_ERROR' } }] }));
    return true;
  });
  await assert.rejects(() => client.viewer(fake.validKey), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'key_rejected');
    assert(!error.message.includes('RAW_AUTH_SECRET'));
    return true;
  });

  // 3. Read-only key refusing write (GraphQL FORBIDDEN and HTTP 403)
  fake.readOnlyMode = 'graphql_forbidden';
  await assert.rejects(() => client.createComment(fake.readOnlyKey, 'issue-142', 'hello'), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'read_only_key');
    assert.equal(error.code, 'linear_read_only_key');
    assert(!error.message.includes('RAW_BODY_SECRET'));
    assert(!error.message.includes(fake.readOnlyKey));
    return true;
  });
  fake.readOnlyMode = 'http403';
  await assert.rejects(() => client.updateIssueState(fake.readOnlyKey, 'issue-142', 'state-done'), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'read_only_key');
    assert(!error.message.includes('SECRET_DIAGNOSTIC'));
    return true;
  });

  // 4. 429 with Retry-After and X-RateLimit-Complexity-Reset headers
  fake.enqueueFault((_req, res) => {
    res.writeHead(429, {
      'content-type': 'application/json',
      'retry-after': '12',
      'x-ratelimit-complexity-reset': String(now + 45_000),
    });
    res.end(JSON.stringify({ errors: [{ message: 'RATE_RAW_BODY', extensions: { code: 'RATELIMITED' } }] }));
    return true;
  });
  await assert.rejects(() => client.viewer(fake.validKey), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'rate_limited');
    assert.equal(error.code, 'linear_rate_limited');
    assert.equal(error.retryAfterSeconds, 45);
    assert.match(error.message, /45 seconds/);
    assert(!error.message.includes('RATE_RAW_BODY'));
    return true;
  });

  // 5. Redirect is refused and target never receives the API key
  fake.enqueueFault((_req, res) => {
    res.writeHead(302, { location: endpoint.replace('/graphql', '/redirect-target') });
    res.end();
    return true;
  });
  await assert.rejects(() => client.viewer(fake.validKey), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'offline');
    return true;
  });
  assert.equal(fake.redirectTargetHits, 0, 'redirect target must never be contacted');

  // 6. Hung response hits timeout
  fake.enqueueFault(() => true);
  await assert.rejects(() => client.viewer(fake.validKey), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'offline');
    assert.match(error.message, /timed out/i);
    return true;
  });

  // 7. Oversized body (chunked without Content-Length and with Content-Length)
  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ data: { filler: 'x'.repeat(5000) } }));
    return true;
  });
  await assert.rejects(() => client.viewer(fake.validKey), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'offline');
    assert.match(error.message, /size limit/);
    return true;
  });

  // 8. HTTP 200 with partial data and GraphQL errors returns usable data + partial flag
  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      data: {
        issues: {
          nodes: [{
            id: 'issue-partial',
            identifier: 'PLA-99',
            title: 'Usable partial issue',
            url: 'https://linear.app/acme/issue/PLA-99',
            priority: 1,
            updatedAt: '2026-11-18T12:00:00.000Z',
            team: { id: 'team-pla' },
            state: { id: 'state-todo', name: 'Todo', type: 'unstarted' },
            assignee: null,
            labels: { nodes: [] },
          }],
          pageInfo: { hasNextPage: false, endCursor: null },
        },
      },
      errors: [{ message: 'INTERNAL_PARTIAL_DETAIL_DO_NOT_LEAK' }],
    }));
    return true;
  });
  const partial = await client.listIssues(fake.validKey, { teamId: 'team-pla', filter: 'active' });
  assert.equal(partial.partial, true);
  assert.equal(partial.warning, 'Some Linear fields could not be loaded.');
  assert.equal(partial.data.issues.length, 1);
  assert.equal(partial.data.issues[0].identifier, 'PLA-99');
  assert(!JSON.stringify(partial).includes('INTERNAL_PARTIAL_DETAIL_DO_NOT_LEAK'));
});

test('parseRetryAfterSeconds handles Retry-After, reset epoch ms, and missing headers', () => {
  const now = 1_700_000_000_000;
  assert.equal(parseRetryAfterSeconds(new Headers({ 'retry-after': '17' }), now), 17);
  assert.equal(parseRetryAfterSeconds(new Headers({ 'x-ratelimit-requests-reset': String(now + 23_400) }), now), 24);
  assert.equal(parseRetryAfterSeconds(new Headers({}), now), 60);
});

test('LinearClient listIssues never requests descriptions or comments in its query document', async t => {
  const fake = new FakeLinearServer();
  const endpoint = await fake.start();
  t.after(() => fake.close());
  const client = new LinearClient({ endpoint });
  const result = await client.listIssues(fake.validKey, { teamId: 'team-pla', filter: 'active' });
  assert.equal(result.data.issues.length, 1);
  assert.equal(result.data.issues[0].identifier, 'PLA-142');
  const query = fake.requests[0].query;
  assert(!query.includes('description'), 'list query must not request description');
  assert(!query.includes('comments'), 'list query must not request comments');
  assert(!query.includes('attachments'), 'list query must not request attachments');
  assert.equal(fake.attachmentHits, 0);
});

test('LinearAuthStore does not write the key into any sibling file or readable backup', async t => {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-linear-key-check-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const secret = 'lin_api_UNIQUE_BYTE_CHECK_987654';
  const authStore = new LinearAuthStore(dir);
  await authStore.write({
    api_key: secret, workspace_id: 'org-1', workspace_name: 'Acme',
    workspace_url_key: 'acme', viewer_id: 'viewer-1', viewer_name: 'Alex', saved_at: 1000,
  });
  const bytes = await readFile(join(dir, 'linear-auth.json'));
  assert(bytes.includes(secret));
  await authStore.remove();
  await assert.rejects(() => readFile(join(dir, 'linear-auth.json')), { code: 'ENOENT' });
});

test('LinearClient throws auth and rate-limit GraphQL errors on HTTP 200 even beside non-null data', async t => {
  const fake = new FakeLinearServer();
  const endpoint = await fake.start();
  t.after(() => fake.close());
  const client = new LinearClient({ endpoint });

  // HTTP 200 with AUTHENTICATION_ERROR beside non-null empty issues list must throw key_rejected.
  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      data: { issues: { nodes: [], pageInfo: { hasNextPage: false, endCursor: null } } },
      errors: [{ message: 'Authentication required', extensions: { code: 'AUTHENTICATION_ERROR' } }],
    }));
    return true;
  });
  await assert.rejects(() => client.listIssues(fake.validKey, { teamId: 'team-pla', filter: 'active' }), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'key_rejected');
    return true;
  });

  // HTTP 200 with RATELIMITED beside non-null data must throw rate_limited with Retry-After seconds.
  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json', 'retry-after': '22' });
    res.end(JSON.stringify({
      data: { issues: { nodes: [], pageInfo: { hasNextPage: false, endCursor: null } } },
      errors: [{ message: 'Rate limited', extensions: { code: 'RATELIMITED' } }],
    }));
    return true;
  });
  await assert.rejects(() => client.listIssues(fake.validKey, { teamId: 'team-pla', filter: 'active' }), (error: unknown) => {
    assert(error instanceof LinearError);
    assert.equal(error.kind, 'rate_limited');
    assert.equal(error.retryAfterSeconds, 22);
    return true;
  });
});

test('LinearClient rejects cursors with bidi/C1 controls or >512 bytes and bounds numeric fields', async t => {
  const fake = new FakeLinearServer();
  const endpoint = await fake.start();
  t.after(() => fake.close());
  const client = new LinearClient({ endpoint });

  // 1. Cursor with bidi override and C1 control must be dropped, and priority out of 0..4 must fall back to 0.
  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      data: {
        issues: {
          nodes: [{
            id: 'i1', identifier: 'PLA-1', title: 'Ok', url: 'https://linear.app/acme/issue/PLA-1',
            priority: 1e308, updatedAt: '2026-11-18T12:00:00.000Z',
            team: { id: 'team-pla' }, state: { id: 's1', name: 'Todo', type: 'unstarted' },
            assignee: null, labels: { nodes: [] },
          }],
          pageInfo: { hasNextPage: true, endCursor: 'cur\u202e\u0085x' },
        },
      },
    }));
    return true;
  });
  const res1 = await client.listIssues(fake.validKey, { teamId: 'team-pla', filter: 'active' });
  assert.equal(res1.data.next_cursor, null, 'cursor containing bidi/C1 controls must be rejected');
  assert.equal(res1.data.has_more, false);
  assert.equal(res1.data.issues[0].priority, 0, 'out-of-range priority must be normalized to 0');

  // 2. Multibyte cursor under 512 UTF-16 units but over 512 UTF-8 bytes must be rejected; huge state position must be bounded.
  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      data: {
        teams: {
          nodes: [{
            id: 't1', key: 'PLA', name: 'Plait',
            states: { nodes: [{ id: 's1', name: 'Todo', type: 'unstarted', position: 1e308 }] },
          }],
          pageInfo: { hasNextPage: true, endCursor: 'あ'.repeat(300) },
        },
      },
    }));
    return true;
  });
  const teams = await client.listTeams(fake.validKey);
  assert(Math.abs(teams.data[0].states[0].position) <= 1_000_000, 'huge state position must be clamped');

  fake.enqueueFault((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      data: {
        issues: {
          nodes: [],
          pageInfo: { hasNextPage: true, endCursor: 'あ'.repeat(300) },
        },
      },
    }));
    return true;
  });
  const res2 = await client.listIssues(fake.validKey, { teamId: 'team-pla', filter: 'active' });
  assert.equal(res2.data.next_cursor, null, 'cursor over 512 UTF-8 bytes must be rejected');
});

test('LinearAuthStore.write masks filesystem errors without disclosing the credential path', async t => {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-linear-unwritable-'));
  t.after(async () => {
    await chmod(dir, 0o700).catch(() => {});
    await rm(dir, { recursive: true, force: true });
  });
  await chmod(dir, 0o500);
  const authStore = new LinearAuthStore(dir);
  await assert.rejects(() => authStore.write({
    api_key: 'lin_api_SECRET_NEVER_LEAK_PATH',
    workspace_id: 'org-1',
    workspace_name: 'Acme',
    workspace_url_key: 'acme',
    viewer_id: 'viewer-1',
    viewer_name: 'Alex',
    saved_at: 1000,
  }), (error: Error & { code?: string }) => {
    assert.equal(error.code, 'internal', `expected HostError internal, got ${error.code}`);
    assert(!error.message.includes(dir), `error must not disclose store path: ${error.message}`);
    assert(!error.message.includes('SECRET_NEVER_LEAK_PATH'), 'error must not disclose key');
    return true;
  });
});
