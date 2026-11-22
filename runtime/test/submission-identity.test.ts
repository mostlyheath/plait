import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { validate } from '../src/protocol.ts';
import { queueRequest } from '../src/checkout-queue.ts';
import { rootIdentity, verifyExpectedIdentity } from '../src/safety.ts';

test('submission identities are validated and replacement folders are refused', async () => {
  const root = await mkdtemp(join(tmpdir(), 'plait-submit-identity-'));
  try {
    const identity = await rootIdentity(root);
    const submit = { op: 'submit', thread_id: 't', request_id: 'r', text: 'hello', identity };
    assert.doesNotThrow(() => validate(submit));
    assert.doesNotThrow(() => queueRequest({ ...submit, op: 'submit_checkout' }));
    assert.throws(() => validate({ ...submit, identity: { dev: 'bad', ino: '1' } }), { code: 'invalid_request' });
    await verifyExpectedIdentity(root, identity);
    await assert.rejects(verifyExpectedIdentity(root, { ...identity, ino: identity.ino === '1' ? '2' : '1' }),
      { code: 'invalid_request', message: 'This folder changed since it was opened.' });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
