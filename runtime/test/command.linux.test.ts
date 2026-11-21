import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { test } from 'node:test';
import { command } from '../src/command.ts';
import { commandPolicy } from '../src/command/policy.ts';
import { rootIdentity } from '../src/safety.ts';

test('Linux denies sandboxed commands in runtime and direct guardian entry points', async t => {
  const root = await mkdtemp(join(tmpdir(), 'plait-linux-denial-'));
  t.after(async () => { await rm(root, { recursive: true, force: true }); });
  const identity = await rootIdentity(root);
  const saved = JSON.stringify({ policy: 'sandboxed', root: identity });
  assert.equal(commandPolicy({ setting: () => saved, setSetting: () => {} }, root, identity), 'sandboxed');
  const script = 'printf unsafe > marker';
  await assert.rejects(command(root, script, [], new AbortController().signal, () => {}), /command sandbox is not available on Linux yet/);
  await assert.rejects(promisify(execFile)(resolve('../target/debug/plait-host'), ['command', root, '', script]), /containment is not enabled/);
  await assert.rejects(readFile(join(root, 'marker')), { code: 'ENOENT' });
});
