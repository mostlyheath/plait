import assert from 'node:assert/strict';
import { test } from 'node:test';
import { LinearClient } from '../src/linear/client.ts';

test('post-save verification uses only the sent fields and compares exact text', async t => {
  const issue = { id: 'issue-1', identifier: 'PLA-1', title: '漢'.repeat(240), description: null };
  t.mock.method(globalThis, 'fetch', async () => Response.json({ data: { issue } }));
  const client = new LinearClient();
  const read = (verify: { title?: string; description?: string }) =>
    client.getIssue('test-key', { issueId: issue.id, verify });

  const matching = await read({ title: issue.title });
  assert.notEqual(matching.data?.title, issue.title, 'the display title is clipped');
  assert.equal(matching.verification, 'matched');
  assert.equal((await read({ description: '' })).verification, 'matched');
  assert.equal((await read({ description: 'mine' })).verification, 'different');
  assert.equal((await read({ title: issue.title.slice(0, -1) })).verification, 'different');
});
