import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import type { Snapshot } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';

for (const status of ['running', 'awaiting_approval'] as const) {
  test(`startup interrupts ${status} only during recovery`, async () => {
    const directory = await mkdtemp(join(tmpdir(), 'plait-recovery-'));
    const first = new Runtime(directory, createModels(), new Set());
    const snapshot: Snapshot = { id: 'thread', checkout: '/checkout', model: { provider: 'faux', model_id: 'model' },
      status, messages: [], tools: [], live_text: 'partial', pending_approval: null, error: null, truncated: false };
    first.store.save(snapshot);
    await first.close();
    const runtime = new Runtime(directory, createModels(), new Set());
    try {
      assert.equal(runtime.store.get('thread').status, status);
      await runtime.recover();
      const reply = await runtime.handle({ op: 'watch', thread_id: 'thread' }) as { thread: Snapshot };
      assert.equal(reply.thread.status, 'interrupted');
      assert.equal(reply.thread.live_text, '');
      assert.equal(reply.thread.error?.code, 'interrupted');
    } finally { await runtime.close(); await rm(directory, { recursive: true, force: true }); }
  });
}
