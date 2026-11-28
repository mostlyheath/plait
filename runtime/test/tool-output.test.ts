import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { join } from 'node:path';
import { writeFile, symlink, link, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { CommandOutputs, OUTPUT_COUNT, OUTPUT_LIMIT, rangeArgs } from '../src/tool-output.ts';
import type { TextRange } from '../src/tool-output.ts';
import { fileAction, rootIdentity } from '../src/safety.ts';
import { setup } from './support/safety.ts';

test('ranged reads reach every byte of a large Unicode file without relaxing path checks', async t => {
  const { root, base } = await setup(t);
  const content = 'line 🚀 é\n'.repeat(30000) + 'LAST LINE';
  await writeFile(join(root, 'large.txt'), content);
  const identity = await rootIdentity(root);
  let offset = 0; let actual = '';
  for (;;) {
    const page = await fileAction<TextRange>(root, 'read_range', 'large.txt', { offset, limit: 12000 }, undefined, identity);
    actual += page.text;
    assert.equal(page.total_bytes, Buffer.byteLength(content));
    assert.equal(page.offset, offset);
    assert(Buffer.byteLength(page.text) <= 12000);
    if (page.next_offset === null) break;
    assert(page.next_offset > offset); offset = page.next_offset;
  }
  assert.equal(actual, content);
  const before = await fileAction(root, 'inspect', 'large.txt');
  await fileAction(root, 'edit', 'large.txt', { expected: 'LAST LINE', content: 'CHANGED', expected_image: before.postimage });
  await assert.rejects(fileAction(root, 'edit', 'large.txt', { expected: 'CHANGED', content: 'STALE', expected_image: before.postimage }), /identity or metadata changed/);
  await assert.rejects(fileAction(root, 'read_range', 'large.txt', { offset: 6, limit: 100 }));
  await symlink(join(root, 'large.txt'), join(root, 'alias'));
  await assert.rejects(fileAction(root, 'read_range', 'alias', { offset: 0 }));
  await link(join(root, 'large.txt'), join(base, 'hard'));
  await assert.rejects(fileAction(root, 'read_range', 'large.txt', { offset: 0 }));
  for (const path of ['../outside', '.git/config']) await assert.rejects(fileAction(root, 'read_range', path, { offset: 0 }));
});

test('ranged helper errors are specific, fixed, and preserve valid boundaries', async t => {
  const root = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-read-range-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, 'unicode.txt'), 'abc🚀z');
  const read = (args: unknown) => fileAction<TextRange>(root, 'read_range', 'unicode.txt', args);
  for (const value of [null, -1, 1.5, '4', true, [], {}]) {
    await assert.rejects(read({ offset: value }), { message: 'Read denied: offset must be a non-negative integer byte offset.' });
    await assert.rejects(read({ limit: value }), { message: 'Read denied: limit must be an integer from 4 to 12000 bytes.' });
  }
  for (const limit of [0, 3, 12001]) {
    await assert.rejects(read({ limit }), { message: 'Read denied: limit must be an integer from 4 to 12000 bytes.' });
  }
  assert.equal((await read({})).text, 'abc🚀z');
  assert.deepEqual(await read({ offset: 0, limit: 4 }), { text: 'abc', offset: 0, next_offset: 3, total_bytes: 8 });
  assert.deepEqual(await read({ offset: 3, limit: 4 }), { text: '🚀', offset: 3, next_offset: 7, total_bytes: 8 });
  assert.deepEqual(await read({ offset: 8 }), { text: '', offset: 8, next_offset: null, total_bytes: 8 });
  await assert.rejects(read({ offset: 9 }), { message: 'Read denied: offset is beyond the end of the file.' });
  for (const offset of [4, 5, 6]) {
    await assert.rejects(read({ offset }), { message: 'Read denied: offset is inside a UTF-8 character. Use the returned next offset.' });
  }
  for (const bytes of [[0xff], [0x80, 97], [97, 0xf0, 0x9f]]) {
    await writeFile(join(root, 'unicode.txt'), Buffer.from(bytes));
    await assert.rejects(read({}), { message: 'Read denied: file contents are not valid UTF-8.' });
  }
  await writeFile(join(root, 'unicode.txt'), '');
  assert.deepEqual(await read({ offset: 0 }), { text: '', offset: 0, next_offset: null, total_bytes: 0 });
  await assert.rejects(fileAction(root, 'read_range', 'private-missing-path', {}), {
    message: 'File access denied, changed, non-unique edit, or unsafe path.',
  });
});

test('command logs survive reopen, preserve Unicode across chunks, and belong to one thread', async t => {
  const { base } = await setup(t);
  let db = new DatabaseSync(join(base, 'logs.sqlite'));
  t.after(() => { db.close(); });
  let outputs = new CommandOutputs(db);
  const log = outputs.begin('thread-a', 'build-1');
  log.append('FIRST\n' + '🚀'.repeat(10000)); log.append('\nLAST');
  assert.match(outputs.read('thread-a', log.id, 0, 12000), /Capture did not finish/);
  assert.match(outputs.read('thread-a', 'build-1', 0, 12000), /^FIRST/);
  assert.match(log.finish(), /read_output/);
  db.close(); db = new DatabaseSync(join(base, 'logs.sqlite')); outputs = new CommandOutputs(db);
  assert.match(outputs.read('thread-a', log.id, 0, 12000), /^FIRST/);
  assert.equal(outputs.read('thread-a', log.id, 40006, 12000).split('\n\n[Bytes')[0], '\nLAST');
  assert.throws(() => outputs.read('thread-a', log.id, 7, 100), /UTF-8/);
  assert.throws(() => outputs.read('thread-b', log.id, 0, 100), /unavailable/);
  assert.throws(() => outputs.read('thread-a', '../host.sqlite', 0, 100), /unavailable/);
  assert.throws(() => outputs.read('thread-a', log.id, 100000, 100), /beyond/);
  const firstPage = outputs.read('thread-a', log.id, 0, 12000);
  assert.match(firstPage, /"offset":11998/);
});

test('command log limits are explicit and retention removes the oldest logs only', () => {
  const db = new DatabaseSync(':memory:');
  try {
    const outputs = new CommandOutputs(db);
    const keep = outputs.begin('other'); keep.append('other project'); keep.finish();
    const log = outputs.begin('thread');
    log.append('x'.repeat(OUTPUT_LIMIT - 1) + '🚀'); log.append('must not be appended');
    assert.match(log.finish(), /exceeded 16 MiB/);
    assert.match(outputs.read('thread', log.id, OUTPUT_LIMIT - 5, 100), /^xxxx\n/);
    assert.equal(db.prepare('SELECT bytes FROM command_outputs WHERE id=?').get(log.id)?.bytes, OUTPUT_LIMIT - 1);
    for (let i = 0; i < OUTPUT_COUNT; i++) outputs.begin('thread').finish();
    assert.throws(() => outputs.read('thread', log.id, 0, 100), /expired/);
    assert.equal(outputs.read('other', keep.id, 0, 100), 'other project');
    assert.equal(db.prepare('SELECT count(*) AS n FROM command_output_chunks WHERE id=?').get(log.id)?.n, 0);
  } finally { db.close(); }
});

test('range arguments reject malformed and unbounded reads', () => {
  for (const args of [{ offset: -1 }, { offset: 0.5 }, { offset: '1' }, { limit: 0 }, { limit: 12001 }, { offset: Infinity }]) {
    assert.throws(() => rangeArgs(args));
  }
});

test('range arguments default only omitted fields and preserve valid byte ranges', () => {
  for (const args of [
    { offset: null }, { limit: null }, { offset: null, limit: null },
    { offset: null, limit: 4 }, { offset: 0, limit: null },
    { offset: undefined }, { limit: undefined },
  ]) {
    assert.throws(() => rangeArgs(args), {
      code: 'invalid_request',
      message: 'offset must be a non-negative byte offset; limit must be 4–12000 bytes.',
    });
  }
  assert.deepEqual(rangeArgs({}), { offset: 0, limit: 8192 });
  assert.deepEqual(rangeArgs({ offset: 0 }), { offset: 0, limit: 8192 });
  assert.deepEqual(rangeArgs({ offset: 27 }), { offset: 27, limit: 8192 });
  assert.deepEqual(rangeArgs({ limit: 4 }), { offset: 0, limit: 4 });
  assert.deepEqual(rangeArgs({ limit: 12000 }), { offset: 0, limit: 12000 });
  assert.deepEqual(rangeArgs({ offset: 0, limit: 4 }), { offset: 0, limit: 4 });
  assert.deepEqual(rangeArgs({ offset: Number.MAX_SAFE_INTEGER, limit: 12000 }), {
    offset: Number.MAX_SAFE_INTEGER, limit: 12000,
  });
});
