import assert from 'node:assert/strict';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { validate, view } from '../src/protocol.ts';
import type { Snapshot } from '../src/protocol.ts';
import { acquireCheckout } from '../src/safety.ts';
import { codex, claude } from './support/record-runtime.ts';
import { request, setup as realHost, until } from './support/host.ts';

test('version 5 validates its operations and strictly rejects malformed fields', () => {
  for (const input of [
    { op: 'status' }, { op: 'harnesses' }, { op: 'turn_records', thread_id: 't' },
    { op: 'turn_records', thread_id: 't', before: '1:500:0:0', limit: 20 },
    { op: 'undo_turn', thread_id: 't', record_id: 'r', request_id: 'u' },
    { op: 'undo_turn', thread_id: 't', record_id: 'r', request_id: 'u', leave_changed: true },
  ]) assert.doesNotThrow(() => validate({ protocol_version: 5, ...input }));
  for (const input of [
    { op: 'turn_records', thread_id: 't', before: '1:0:0' },
    { op: 'turn_records', thread_id: 't', before: '-1:0:0:0' },
    { op: 'turn_records', thread_id: 't', limit: 0 }, { op: 'turn_records', thread_id: 't', limit: 51 },
    { op: 'turn_records', thread_id: 't', limit: 1.2 },
    { op: 'undo_turn', thread_id: 't', record_id: '../r', request_id: 'u' },
    { op: 'undo_turn', thread_id: 't', record_id: 'r', request_id: 'u', leave_changed: 'true' },
    { op: 'undo_turn', thread_id: 't', record_id: 'r' },
  ]) assert.throws(() => validate({ protocol_version: 5, ...input }));
  for (const protocol_version of [1, 2, 3, 4, 7]) {
    assert.throws(() => validate({ protocol_version, op: 'turn_records', thread_id: 't' }));
    assert.throws(() => validate({ protocol_version, op: 'undo_turn', thread_id: 't', record_id: 'r', request_id: 'u' }));
  }
});

test('records_revision is absent from every older snapshot shape', () => {
  const snapshot: Snapshot = { id: 't', checkout: '/repo', model: { provider: 'p', model_id: 'm' }, status: 'idle', messages: [], tools: [], live_text: '', pending_approval: null, error: null, truncated: false };
  for (const version of [1, 2, 3, 4] as const) {
    const old = view(snapshot, version);
    assert.deepEqual(view({ ...snapshot, records_revision: 99 }, version), old);
    assert.equal('records_revision' in old, false);
  }
  assert.equal((view(snapshot, 5) as any).records_revision, 0);
  assert.equal((view({ ...snapshot, records_revision: 7 }, 5) as any).records_revision, 7);
});

for (const [name, setup] of [['Codex', codex], ['Claude Code', claude]] as const) {
  test(`version 5 ${name}: list, busy, undo, deduplication and undo of undo`, { timeout: 30000 }, async t => {
    const h = await setup(t);
    const thread = await h.create();
    const call = (input: Record<string, unknown>) => h.call({ thread_id: thread.id, ...input }, 5);
    const revision = (await call({ op: 'watch' })).result.thread.records_revision;
    await writeFile(join(h.root, 'unrelated'), 'person\n');
    await h.submit(thread.id, 'r1');
    if (name === 'Codex') await h.asked(thread.id);
    assert.equal((await call({ op: 'undo_turn', record_id: 'not-yet', request_id: 'u-busy' })).error?.code, 'busy');
    await h.finish(thread.id);
    const page = (await call({ op: 'turn_records' })).result;
    assert.equal(page.records.length, 1);
    const record = page.records[0];
    assert.equal(record.state, 'ready'); assert.equal(record.kind, 'turn');
    assert.deepEqual(record.request_ids, ['r1']); assert.equal(record.after_restart, false);
    assert.equal(record.paths.length, 1);
    assert.ok((await call({ op: 'watch' })).result.thread.records_revision > revision);
    const path = join(h.root, record.paths[0].path), bytes = await readFile(path);
    const undo = { op: 'undo_turn', record_id: record.id, request_id: 'u' };
    const result = await call(undo); assert.equal(result.ok, true, JSON.stringify(result));
    assert.equal(result.result.state, 'undone');
    await assert.rejects(readFile(path), { code: 'ENOENT' });
    assert.equal(await readFile(join(h.root, 'unrelated'), 'utf8'), 'person\n');
    // Repeating the same ID must not remove a new person-created file.
    await writeFile(path, 'later person');
    assert.deepEqual(await call(undo), result);
    assert.equal(await readFile(path, 'utf8'), 'later person');
    assert.equal((await call({ ...undo, leave_changed: true })).error?.code, 'conflict');
    await rm(path);
    const redo = await call({ op: 'undo_turn', record_id: result.result.record_id, request_id: 'redo' });
    assert.equal(redo.result?.state, 'undone', JSON.stringify(redo));
    assert.deepEqual(await readFile(path), bytes);
    const latest = (await call({ op: 'turn_records' })).result.records;
    assert.equal(latest[0].kind, 'undo'); assert.equal(latest[0].undoes, result.result.record_id);
  });
}

test('Pi replay through the socket: version 5 and durable undo dedup after restart', { timeout: 30000 }, async t => {
  const f = await realHost(t, 'edit'); const thread = await f.create();
  const call = (input: Record<string, unknown>) => request(f.store, { protocol_version: 5, thread_id: thread.id, ...input }) as Promise<any>;
  const bytes = await readFile(join(f.repo, 'notes.txt'));
  await f.submit(thread.id);
  const approval = await until(() => f.watch(thread.id), s => !!s.pending_approval);
  assert.equal((await call({ op: 'undo_turn', record_id: 'not-yet', request_id: 'busy' })).error.code, 'busy');
  await f.approve(thread.id, approval.pending_approval!.id);
  await until(() => f.watch(thread.id), s => s.status === 'idle');
  const record = (await call({ op: 'turn_records' })).result.records[0];
  assert.deepEqual(record.paths.map((p: any) => p.path), ['notes.txt']);
  const undo = { op: 'undo_turn', record_id: record.id, request_id: 'undo' };
  const reply = await call(undo); assert.equal(reply.result?.state, 'undone', JSON.stringify(reply));
  assert.deepEqual(await readFile(join(f.repo, 'notes.txt')), bytes);
  f.host.kill('SIGTERM'); await until(async () => f.host.exitCode !== null || f.host.signalCode !== null, done => done);
  await f.start(); await writeFile(join(f.repo, 'notes.txt'), 'person after restart');
  assert.deepEqual(await call(undo), reply);
  const records = (await call({ op: 'turn_records' })).result.records;
  assert.equal(records.find((r: any) => r.id === record.id).state, 'undone');
  assert.equal(records.find((r: any) => r.id === record.id).after_restart, false);
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'person after restart');
});

test('a person, later run or other thread change blocks; leave_changed restores only safe paths', async t => {
  const h = await codex(t); const thread = await h.create(), other = await h.create();
  const call = (input: Record<string, unknown>) => h.call({ thread_id: thread.id, ...input }, 5);
  await h.submit(thread.id, 'r1'); await h.finish(thread.id);
  const record = (await call({ op: 'turn_records' })).result.records[0];
  await writeFile(join(h.root, 'codex-r1.txt'), 'person changed');
  const undo = { op: 'undo_turn', record_id: record.id, request_id: 'blocked' };
  assert.equal((await call(undo)).result.state, 'blocked');
  assert.equal(await readFile(join(h.root, 'codex-r1.txt'), 'utf8'), 'person changed');
  // Same blocked request ID is not cached: after matching the after image, it works.
  const db = h.runtime.store.db;
  const path = db.prepare('SELECT after_oid FROM record_paths WHERE record=?').get(record.id)!;
  const { exec } = await import('./support/safety.ts');
  const blob = await exec('/usr/bin/git', ['--git-dir', join(h.store, 'records/sha1'), 'cat-file', 'blob', String(path.after_oid)]);
  await writeFile(join(h.root, 'codex-r1.txt'), blob.stdout);
  assert.equal((await call(undo)).result.state, 'undone');
  // The other thread writes this same path from its own run named r1.
  await h.submit(other.id, 'r1'); await h.finish(other.id);
  await writeFile(join(h.root, 'codex-r1.txt'), 'later thread changed again');
  const blocked = await call({ op: 'undo_turn', record_id: record.id, request_id: 'other', leave_changed: true });
  assert.equal(blocked.result.state, 'undone');
  assert.deepEqual(blocked.result.changed_since, ['codex-r1.txt']);
  assert.equal(await readFile(join(h.root, 'codex-r1.txt'), 'utf8'), 'later thread changed again');
});

test('checkout lease refuses an undo held by another host', async t => {
  const h = await codex(t); const thread = await h.create(); await h.submit(thread.id, 'r1'); await h.finish(thread.id);
  const target = h.pairs()[0].after!.id;
  const lease = await acquireCheckout(h.root);
  try { assert.equal((await h.call({ op: 'undo_turn', thread_id: thread.id, record_id: target, request_id: 'u' }, 5)).error?.code, 'busy'); }
  finally { await lease.close(); }
});

test('record pages cover paths, exclusions and steering IDs inside the frame bound', async t => {
  const h = await codex(t); const thread = await h.create(); await h.submit(thread.id, 'r1'); await h.finish(thread.id);
  const id = h.pairs()[0].after!.id, db = h.runtime.store.db;
  const insert = db.prepare('INSERT INTO record_paths VALUES (?,?,NULL,NULL,?,?,1,0)');
  db.exec('BEGIN');
  for (let i = 0; i < 1600; i++) insert.run(id, `p${String(i).padStart(5, '0')}-${'界'.repeat(1000)}`, '100644', 'a'.repeat(40));
  const coverage = Array.from({ length: 1200 }, (_, i) => ({ path: `ignored-${i}/`, reason: 'ignored_directory' }));
  db.prepare('UPDATE records SET not_covered=?,not_covered_total=? WHERE id=?').run(JSON.stringify(coverage), coverage.length, id);
  db.exec('COMMIT');
  let before: string | undefined; const paths: string[] = [], uncovered: unknown[] = [], ids: string[] = [];
  let pages = 0;
  do {
    const reply = await h.call({ op: 'turn_records', thread_id: thread.id, ...(before ? { before } : {}) }, 5);
    assert.equal(reply.ok, true, JSON.stringify(reply)); assert.ok(Buffer.byteLength(JSON.stringify(reply)) < 1048576);
    const page = reply.result.records[0]; assert.ok(page.paths.length <= 500);
    assert.equal(page.paths_offset, paths.length); assert.equal(page.not_covered_offset, uncovered.length); assert.equal(page.request_ids_offset, ids.length);
    paths.push(...page.paths.map((p: any) => p.path)); uncovered.push(...page.not_covered); ids.push(...page.request_ids);
    before = reply.result.before ?? undefined; assert.ok(++pages < 100);
  } while (before);
  assert.equal(paths.length, 1601); assert.equal(new Set(paths).size, paths.length); assert.deepEqual(uncovered, coverage); assert.deepEqual(ids, ['r1']);
});

test('large checkout exhausts undo budget before writes and releases its lease', { timeout: 60000 }, async t => {
  const h = await codex(t); const thread = await h.create();
  await mkdir(join(h.root, 'many'));
  const filename = (i: number) => `${i}-${'x'.repeat(200)}`;
  for (let i = 0; i < 3500; i++) await writeFile(join(h.root, 'many', filename(i)), `before ${i}\n`);
  const before = await h.runtime.records.before(thread.id, 'large', h.root);
  for (let i = 0; i < 3500; i++) await writeFile(join(h.root, 'many', filename(i)), `after ${i}\n`);
  await h.runtime.records.after(before);
  const target = h.pairs().find(p => p.run === 'large')!.after!.id;
  const stored = h.runtime.store.db.prepare('SELECT * FROM record_paths WHERE record=?').all(target);
  assert.ok(Buffer.byteLength(JSON.stringify(stored)) > 1048576, 'the private path manifest exceeds the helper frame');
  const start = Date.now();
  const reply = await h.call({ op: 'undo_turn', thread_id: thread.id, record_id: target, request_id: 'large-undo' }, 5);
  assert.equal(reply.error?.code, 'undo_failed', JSON.stringify(reply));
  const elapsed = Date.now() - start;
  assert.ok(elapsed >= 9000 && elapsed < 12000, `undo must exhaust its budget inside the sidecar limit: ${elapsed} ms`);
  t.diagnostic(`3,500-file undo stopped after ${elapsed} ms; no working files changed.`);
  for (let i = 0; i < 3500; i++) assert.equal(await readFile(join(h.root, 'many', filename(i)), 'utf8'), `after ${i}\n`);
  const lease = await acquireCheckout(h.root); await lease.close();
});

for (const [name, setup] of [['Codex', codex], ['Claude Code', claude]] as const) {
  test(`${name} steering IDs remain attached to one record while follow-ups get another`, async t => {
    const h = name === 'Claude Code' ? await claude(t, 'turn', 'ask') : await codex(t); const thread = await h.create();
    await h.submit(thread.id, 'initial'); await h.asked(thread.id);
    await h.submit(thread.id, 'steering', { when_busy: 'steer' });
    await h.submit(thread.id, 'follow', { when_busy: 'follow_up' });
    while (h.pairs().length < 2 || h.pairs()[1].after?.state !== 'ready') {
      await h.until(thread.id, s => !!s.pending_approval || (h.pairs().length === 2 && h.pairs()[1].after?.state === 'ready'));
      if (h.runtime.snapshot(thread.id).pending_approval) await h.approve(thread.id);
    }
    const page = await h.call({ op: 'turn_records', thread_id: thread.id }, 5);
    assert.deepEqual(page.result.records.map((r: any) => r.request_ids), [['follow'], ['initial', 'steering']]);
  });
}

test('after-restart warning and version 5 command paths are exposed only to new clients', async t => {
  const h = await codex(t); const thread = await h.create();
  const before = await h.runtime.records.before(thread.id, 'crashed', h.root);
  await writeFile(join(h.root, 'command.txt'), 'written while down');
  await h.runtime.records.after(before, undefined, true);
  const call = (input: Record<string, unknown>, version = 5) => h.call({ thread_id: thread.id, ...input }, version);
  const record = (await call({ op: 'turn_records' })).result.records[0];
  assert.equal(record.after_restart, true);
  assert.deepEqual((await call({ op: 'changed_files' }, 4)).result.files, []);
  assert.deepEqual((await call({ op: 'changed_files' })).result.files, ['command.txt']);
  assert.equal((await call({ op: 'undo_turn', record_id: record.id, request_id: 'u' })).result.after_restart, true);
});

test('undo refuses a failed after record and a record from a different thread', async t => {
  const h = await codex(t); const thread = await h.create(), other = await h.create();
  await h.submit(thread.id, 'r1'); await h.finish(thread.id);
  const target = h.pairs()[0].after!.id;
  assert.equal((await h.call({ op: 'undo_turn', thread_id: other.id, record_id: target, request_id: 'u' }, 5)).error?.code, 'not_found');
  h.runtime.store.db.prepare("UPDATE records SET state='failed' WHERE id=?").run(target);
  assert.equal((await h.call({ op: 'undo_turn', thread_id: thread.id, record_id: target, request_id: 'u' }, 5)).error?.code, 'not_found');
  assert.ok(await readFile(join(h.root, 'codex-r1.txt')));
});

test('interrupted undo recovers only completed writes, warns after restart, and never replays', { timeout: 30000 }, async t => {
  const h = await codex(t); const thread = await h.create(); await h.submit(thread.id, 'r1'); await h.finish(thread.id);
  const record = h.pairs()[0].after!.id;
  const { exec } = await import('./support/safety.ts');
  const { chmod } = await import('node:fs/promises');
  const wrapper = join(h.store, 'git-undo-failure'), marker = join(h.store, 'records', 'undo-capture-marker');
  const realGit = process.platform === 'darwin' ? (await exec('/usr/bin/xcrun', ['--find', 'git'])).stdout.trim() : '/usr/bin/git';
  // U0 commits, restoration happens, then U1's commit fails. All file writes are real.
  await writeFile(wrapper, `#!/bin/sh\ncase " $* " in\n*' commit-tree '*) if [ -e '${marker}' ]; then exit 79; fi; /usr/bin/touch '${marker}' ;;\nesac\nexec '${realGit}' "$@"\n`);
  await chmod(wrapper, 0o700);
  const previous = process.env.PLAIT_GIT; process.env.PLAIT_GIT = wrapper;
  const undo = { op: 'undo_turn', thread_id: thread.id, record_id: record, request_id: 'interrupted' };
  try { assert.equal((await h.call(undo, 5)).error?.code, 'undo_incomplete'); }
  finally { if (previous === undefined) delete process.env.PLAIT_GIT; else process.env.PLAIT_GIT = previous; }
  await assert.rejects(readFile(join(h.root, 'codex-r1.txt')), { code: 'ENOENT' });
  const undoId = String(h.runtime.store.db.prepare("SELECT id FROM records WHERE kind='undo'").get()!.id);
  await h.close();
  await writeFile(join(h.root, 'unrelated-down'), 'person while down');
  const { Runtime } = await import('../src/runtime.ts');
  const { models } = await import('./support/record-runtime.ts');
  const recovered = new Runtime(h.store, models(), new Set(['faux'])); await recovered.recover(); t.after(() => recovered.close());
  await until(async () => recovered.store.db.prepare('SELECT * FROM records WHERE id=?').get(undoId), row => row?.state === 'ready');
  const page = await recovered.handle({ op: 'turn_records', protocol_version: 5, thread_id: thread.id }) as any;
  assert.equal(page.records[0].after_restart, true);
  assert.deepEqual(page.records[0].paths.map((p: any) => p.path), ['codex-r1.txt']);
  await writeFile(join(h.root, 'codex-r1.txt'), 'person after undo');
  const reply = await recovered.reply({ ...undo, protocol_version: 5 }) as any;
  assert.equal(reply.error.code, 'undo_incomplete');
  assert.equal(await readFile(join(h.root, 'codex-r1.txt'), 'utf8'), 'person after undo');
  assert.equal(await readFile(join(h.root, 'unrelated-down'), 'utf8'), 'person while down');
});

test('a later record in another thread blocks an earlier run without disturbing either file', async t => {
  const h = await codex(t); const first = await h.create(), later = await h.create();
  await writeFile(join(h.root, 'shared'), 'initial'); await writeFile(join(h.root, 'safe'), 'initial');
  const before = await h.runtime.records.before(first.id, 'first-run', h.root);
  await writeFile(join(h.root, 'shared'), 'first'); await writeFile(join(h.root, 'safe'), 'first');
  await h.runtime.records.after(before);
  const next = await h.runtime.records.before(later.id, 'later-run', h.root);
  await writeFile(join(h.root, 'shared'), 'second thread'); await h.runtime.records.after(next);
  const target = h.pairs().find(p => p.run === 'first-run')!.after!.id;
  const call = (extra: Record<string, unknown>) => h.call({ op: 'undo_turn', thread_id: first.id, record_id: target, request_id: 'u', ...extra }, 5);
  assert.equal((await call({})).result.state, 'blocked');
  assert.equal(await readFile(join(h.root, 'safe'), 'utf8'), 'first');
  assert.deepEqual((await call({ leave_changed: true })).result.changed_since, ['shared']);
  assert.equal(await readFile(join(h.root, 'shared'), 'utf8'), 'second thread');
  assert.equal(await readFile(join(h.root, 'safe'), 'utf8'), 'initial');
});

test('a zero-effect failed undo keeps the original action and survives restart without replay', async t => {
  const { chmod } = await import('node:fs/promises');
  const h = await codex(t); const thread = await h.create();
  await h.submit(thread.id, 'r1'); await h.finish(thread.id);
  const original = h.pairs()[0].after!.id;
  const undo = { op: 'undo_turn', thread_id: thread.id, record_id: original, request_id: 'read-only' };
  const path = join(h.root, 'codex-r1.txt'), bytes = await readFile(path);
  await chmod(h.root, 0o555);
  try { assert.equal((await h.call(undo, 5)).error?.code, 'undo_no_effect'); }
  finally { await chmod(h.root, 0o755); }
  const page = (await h.call({ op: 'turn_records', thread_id: thread.id }, 5)).result.records;
  assert.equal(page[0].state, 'no_effect'); assert.equal(page[0].undoes, original);
  assert.equal(page.find((r: any) => r.id === original).state, 'ready');
  assert.deepEqual(await readFile(path), bytes);
  await h.close();
  const { Runtime } = await import('../src/runtime.ts');
  const { models } = await import('./support/record-runtime.ts');
  const recovered = new Runtime(h.store, models(), new Set(['faux'])); await recovered.recover(); t.after(() => recovered.close());
  await recovered.records.settled(h.root);
  assert.equal((await recovered.reply({ ...undo, protocol_version: 5 }) as any).error.code, 'undo_no_effect');
  assert.deepEqual(await readFile(path), bytes);
  const records = recovered.records.page(thread.id).records;
  assert.equal(records[0].state, 'no_effect');
  assert.equal(records.find(r => r.id === original)!.state, 'ready');
  await recovered.records.prune();
  const retry = await recovered.reply({ ...undo, request_id: 'retry', protocol_version: 5 }) as any;
  assert.equal(retry.result?.state, 'undone', JSON.stringify(retry));
  await assert.rejects(readFile(path), { code: 'ENOENT' });
});
