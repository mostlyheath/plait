import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { HostError } from '../src/protocol.ts';
import { GitReads, PAGE, validateGitRead } from '../src/git-read.ts';
import { binary } from './support/host.ts';

const checkout = Buffer.from('/repo').toString('base64');
const code = (fn: () => unknown, expected: string) => assert.throws(fn, (error: unknown) => error instanceof HostError && error.code === expected);

test('Git reads accept only known reads and their fields', () => {
  const valid = { op: 'git_read', read: 'diff', checkout, identity: { dev: '1', ino: '18446744073709551615' }, change: {}, mode: 'Staged' };
  assert.equal(validateGitRead(valid).read, 'diff');
  assert.equal(validateGitRead({ op: 'git_read', read: 'status', checkout, identity: null }).read, 'status');
  for (const input of [null, [], { ...valid, read: 'write' }, { ...valid, protocol_version: 6 }, { ...valid, path: 'x' },
    { ...valid, checkout: '' }, { ...valid, checkout: '/repo' }, { ...valid, identity: { dev: 1, ino: '2' } },
    { ...valid, identity: { dev: '1', ino: '2', extra: '3' } }, { ...valid, change: 'x'.repeat(65_001) },
    { op: 'git_read_page', token: 'nope', offset: 0 }, { op: 'git_read_page', token: crypto.randomUUID(), offset: -1 },
    { op: 'git_read_page', token: crypto.randomUUID(), offset: 0, extra: 1 }]) code(() => validateGitRead(input), 'invalid_request');
});

test('a large result is served in pages that reassemble exactly, then forgotten', async () => {
  const whole = Buffer.alloc(PAGE * 2 + 123, 7); whole[0] = 1; whole[whole.length - 1] = 9;
  const seen: unknown[] = [];
  const reads = new GitReads(async input => { seen.push(input); return whole; });
  const first: any = await reads.handle({ op: 'git_read', read: 'status', checkout, identity: null });
  assert.deepEqual(seen, [{ read: 'status', checkout, identity: null }], 'the helper never sees the op');
  assert.equal(first.total, whole.length);
  const parts = [Buffer.from(first.data, 'base64')];
  let page: any = first;
  while (page.token) {
    page = await reads.handle({ op: 'git_read_page', token: page.token, offset: parts.reduce((n, p) => n + p.length, 0) });
    parts.push(Buffer.from(page.data, 'base64'));
    assert.ok(page.data.length < 1024 * 1024, 'a page fits one reply');
  }
  assert.deepEqual(Buffer.concat(parts), whole);
  await assert.rejects(reads.handle({ op: 'git_read_page', token: first.token, offset: 0 }), (e: HostError) => e.code === 'not_found');
});

test('small results need no token; old and excess tokens expire', async () => {
  let now = 0;
  const reads = new GitReads(async input => Buffer.alloc((input as { read: string }).read === 'files' ? PAGE + 1 : 10), () => now);
  assert.deepEqual(await reads.handle({ op: 'git_read', read: 'status', checkout, identity: null }), { total: 10, data: Buffer.alloc(10).toString('base64'), token: null });
  const tokens: string[] = [];
  for (let i = 0; i < 5; i++) tokens.push(((await reads.handle({ op: 'git_read', read: 'files', checkout, identity: null })) as any).token);
  await assert.rejects(reads.handle({ op: 'git_read_page', token: tokens[0], offset: PAGE }), (e: HostError) => e.code === 'not_found');
  await assert.rejects(reads.handle({ op: 'git_read_page', token: tokens[4], offset: PAGE + 1 }), (e: HostError) => e.code === 'invalid_request');
  now += 121_000;
  await assert.rejects(reads.handle({ op: 'git_read_page', token: tokens[4], offset: PAGE }), (e: HostError) => e.code === 'not_found');
});

test('a failed helper reveals nothing about why', async () => {
  const reads = new GitReads(async () => { throw new Error('/secret/store path'); });
  await assert.rejects(reads.handle({ op: 'git_read', read: 'status', checkout, identity: null }),
    (e: HostError) => e.code === 'git_read_failed' && !e.message.includes('secret'));
});

test('the real helper reads a checkout', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'plait-git-read-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  const git = (...args: string[]) => execFileSync('git', ['-c', 'user.name=Plait', '-c', 'user.email=plait@example.invalid', ...args], { cwd: root });
  git('init', '-q', '-b', 'main'); await writeFile(join(root, 'a.txt'), 'one\n'); git('add', '.'); git('commit', '-q', '-m', 'First');
  await writeFile(join(root, 'a.txt'), 'two\n');
  const previous = process.env.PLAIT_HOST_EXE; process.env.PLAIT_HOST_EXE = binary;
  t.after(() => { if (previous === undefined) delete process.env.PLAIT_HOST_EXE; else process.env.PLAIT_HOST_EXE = previous; });
  const reads = new GitReads();
  const reply: any = await reads.handle({ op: 'git_read', read: 'status', checkout: Buffer.from(root).toString('base64'), identity: null });
  const outcome = JSON.parse(Buffer.from(reply.data, 'base64').toString('utf8'));
  assert.equal(outcome.ok.result.branch, 'main');
  assert.equal(Buffer.from(outcome.ok.result.changes[0].path, 'base64').toString(), 'a.txt');
  const refused: any = await reads.handle({ op: 'git_read', read: 'status', checkout: Buffer.from(join(root, '.git')).toString('base64'), identity: null });
  assert.ok(JSON.parse(Buffer.from(refused.data, 'base64').toString('utf8')).error.message, 'a folder inside Git metadata is refused');
});
