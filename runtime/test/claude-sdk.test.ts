import { fixtureDirectory } from './support/temp.ts';
// Setting up the Agent SDK: the tar reader, the hash check, and the atomic
// install. No test downloads anything: fetch is a stand-in serving archives
// built here, and the module that loads is the dev copy.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';
import { readTar, readTgz, safePath } from '../src/tar.ts';
import {
  CLAUDE_SDK_INTEGRITY, CLAUDE_SDK_MODULE_SHA256, CLAUDE_SDK_TARBALL, loadSdk, matchesIntegrity, sdkDirectory, sdkPresent, setUpSdk,
} from '../src/claude/sdk.ts';
import { CLAUDE_SDK_VERSION } from '../src/claude/install.ts';
import { DEV_SDK } from './support/fake-claude-env.ts';

type Entry = { name: string; data?: Buffer | string; type?: string; prefix?: string; linkname?: string; size?: number };
const LIMITS = { maxArchive: 1 << 24, maxEntry: 1 << 22, maxEntries: 64 };

/** One ustar header and its padded data, with a correct checksum unless told otherwise. */
function header(entry: Entry, data: Buffer) {
  const block = Buffer.alloc(512);
  block.write(entry.name, 0, 100, 'utf8');
  block.write('0000644\0', 100); block.write('0000000\0', 108); block.write('0000000\0', 116);
  block.write(`${(entry.size ?? data.length).toString(8).padStart(11, '0')}\0`, 124);
  block.write('00000000000\0', 136);
  block.write(entry.type ?? '0', 156);
  if (entry.linkname) block.write(entry.linkname, 157, 100);
  block.write('ustar\0', 257); block.write('00', 263);
  if (entry.prefix) block.write(entry.prefix, 345, 155);
  block.fill(0x20, 148, 156);
  let sum = 0; for (const byte of block) sum += byte;
  block.write(`${sum.toString(8).padStart(6, '0')}\0 `, 148);
  return Buffer.concat([block, data, Buffer.alloc((512 - (data.length % 512)) % 512)]);
}
function tar(entries: Entry[]) {
  return Buffer.concat([...entries.map(entry => header(entry, Buffer.from(entry.data ?? ''))), Buffer.alloc(1024)]);
}
const integrity = (data: Buffer) => `sha512-${createHash('sha512').update(data).digest('base64')}`;
/** An archive shaped like the registry's: the real module, its license, and files the host leaves out. */
function sdkArchive(extra: Entry[] = []) {
  return gzipSync(tar([
    { name: 'package/package.json', data: '{"name":"@anthropic-ai/claude-agent-sdk","version":"0.3.326"}' },
    { name: 'package/LICENSE.md', data: 'License text' },
    { name: 'package/README.md', data: 'Readme text' },
    { name: 'package/sdk.mjs', data: readFileSync(join(DEV_SDK, 'sdk.mjs')) },
    { name: 'package/bridge.mjs', data: 'export {};' },
    { name: 'package/sdk.d.ts', data: 'export {};' },
    ...extra,
  ]));
}
/** A fetch that serves `body` and counts its calls. */
function serving(body: Buffer, status = 200) {
  const calls: string[] = [];
  const fetch = (async (url: string) => { calls.push(String(url)); return new Response(new Uint8Array(body), { status }); }) as unknown as typeof globalThis.fetch;
  return { fetch, calls };
}
async function scratch(t: test.TestContext) {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-claude-sdk-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return { dir, target: sdkDirectory(dir) };
}

test('the pinned tarball and hash are the ones package-lock.json records for the dev copy', () => {
  const lock = JSON.parse(readFileSync(new URL('../package-lock.json', import.meta.url), 'utf8'));
  const recorded = lock.packages['node_modules/@anthropic-ai/claude-agent-sdk'];
  assert.equal(recorded.version, CLAUDE_SDK_VERSION);
  assert.equal(recorded.integrity, CLAUDE_SDK_INTEGRITY);
  assert.equal(recorded.resolved, CLAUDE_SDK_TARBALL);
  // Releases install production dependencies only, so they never carry the SDK.
  assert.equal(recorded.dev, true);
  assert.equal(lock.packages[''].dependencies['@anthropic-ai/claude-agent-sdk'], undefined);
  assert.equal(createHash('sha256').update(readFileSync(join(DEV_SDK, 'sdk.mjs'))).digest('hex'), CLAUDE_SDK_MODULE_SHA256);
});

test('the tar reader returns regular files and refuses unsafe or broken archives', () => {
  const files = readTar(tar([{ name: 'package/', type: '5' }, { name: 'a.txt', prefix: 'package', data: 'hello' }]), LIMITS);
  assert.deepEqual(files.map(file => [file.path, file.data.toString()]), [['package/a.txt', 'hello']]);
  // pax headers can carry a long path.
  const long = `package/${'d/'.repeat(80)}long.txt`;
  const record = (key: string, value: string) => { const body = ` ${key}=${value}\n`; let length = body.length; while (`${length}${body}`.length !== length) length = `${length}${body}`.length; return `${length}${body}`; };
  assert.equal(readTar(tar([{ name: 'PaxHeader', type: 'x', data: record('path', long) }, { name: 'short', data: 'x' }]), LIMITS)[0].path, long);
  for (const [entry, pattern] of [
    [{ name: '/etc/passwd', data: 'x' }, /Unsafe path/],
    [{ name: 'package/../../escape', data: 'x' }, /Unsafe path/],
    [{ name: 'package/./x', data: 'x' }, /Unsafe path/],
    [{ name: 'package\\x', data: 'x' }, /Unsafe path/],
    [{ name: 'package/link', type: '2', linkname: '/etc/passwd' }, /link or special/],
    [{ name: 'package/hard', type: '1', linkname: 'package/sdk.mjs' }, /link or special/],
    [{ name: 'package/fifo', type: '6' }, /link or special/],
    [{ name: 'package/big', data: 'x', size: LIMITS.maxEntry + 1 }, /too large/],
  ] as [Entry, RegExp][]) assert.throws(() => readTar(tar([entry]), LIMITS), pattern, entry.name);
  const good = tar([{ name: 'a.txt', data: 'hello' }]);
  assert.throws(() => readTar(good.subarray(0, 700), LIMITS), /truncated/);
  assert.throws(() => readTar(good.subarray(0, 512), LIMITS), /truncated/);
  const corrupt = Buffer.from(good); corrupt[0] ^= 1;
  assert.throws(() => readTar(corrupt, LIMITS), /corrupt/);
  assert.throws(() => readTar(tar(Array.from({ length: 65 }, (_, index) => ({ name: `f${index}`, data: '' }))), LIMITS), /too many/);
  assert.equal(safePath('package/dir/'), 'package/dir');
});

test('a compression bomb and a corrupt gzip stream are refused', async () => {
  await assert.rejects(readTgz(gzipSync(Buffer.alloc(LIMITS.maxArchive + 1024)), LIMITS), /corrupt or too large/);
  const archive = gzipSync(tar([{ name: 'a.txt', data: 'hello' }]));
  await assert.rejects(readTgz(archive.subarray(0, archive.length - 8), LIMITS), /corrupt or too large/);
});

test('setup downloads once, checks the hash first, keeps only what loads, and loads it', async t => {
  const { target } = await scratch(t);
  const archive = sdkArchive();
  const served = serving(archive);
  assert.equal(await sdkPresent(target), false);
  assert.deepEqual(await setUpSdk({ directory: target, fetch: served.fetch, url: 'https://registry.invalid/sdk.tgz', integrity: integrity(archive) }), { downloaded: true });
  assert.deepEqual(served.calls, ['https://registry.invalid/sdk.tgz']);
  assert.deepEqual((await readdir(target)).sort(), ['LICENSE.md', 'README.md', 'package.json', 'sdk.mjs']);
  // Nothing else is left beside it: the staging folder was renamed into place.
  assert.deepEqual(await readdir(join(target, '..')), [CLAUDE_SDK_VERSION]);
  assert.equal(typeof (await loadSdk(target)).query, 'function');
  // Already present: no download at all.
  assert.deepEqual(await setUpSdk({ directory: target, fetch: served.fetch, integrity: integrity(archive) }), { downloaded: false });
  assert.equal(served.calls.length, 1);
});

test('a hash mismatch, a bad status, a truncated or unsafe archive install nothing', async t => {
  const { target } = await scratch(t);
  const archive = sdkArchive();
  const cases: [Buffer, string, number, RegExp][] = [
    [archive, CLAUDE_SDK_INTEGRITY, 200, /does not match the pinned hash/],
    [archive, integrity(archive), 404, /answered 404/],
    [archive.subarray(0, archive.length - 100), integrity(archive.subarray(0, archive.length - 100)), 200, /corrupt or too large/],
    [sdkArchive([{ name: 'package/../../../evil', data: 'x' }]), '', 200, /Unsafe path/],
    [sdkArchive([{ name: 'package/sdk-link', type: '2', linkname: '/etc/passwd' }]), '', 200, /link or special/],
    [gzipSync(tar([{ name: 'package/sdk.mjs', data: 'export {}' }])), '', 200, /no LICENSE\.md/],
  ];
  for (const [body, pinned, status, pattern] of cases) {
    const served = serving(body, status);
    await assert.rejects(setUpSdk({ directory: target, fetch: served.fetch, integrity: pinned || integrity(body) }), pattern);
    assert.equal(await sdkPresent(target), false, String(pattern));
  }
  assert.deepEqual(await readdir(join(target, '..')).catch(() => []), []);
  assert.equal(matchesIntegrity(archive, integrity(archive)), true);
  assert.throws(() => matchesIntegrity(archive, 'sha1-abc'), /not a sha512/);
});

test('a damaged copy does not load, and setup replaces it', async t => {
  const { target } = await scratch(t);
  const archive = sdkArchive();
  await setUpSdk({ directory: target, fetch: serving(archive).fetch, integrity: integrity(archive) });
  await writeFile(join(target, 'sdk.mjs'), 'export const query = () => { throw new Error("tampered"); };');
  await assert.rejects(loadSdk(target), /does not match the pinned version/);
  const served = serving(archive);
  assert.deepEqual(await setUpSdk({ directory: target, fetch: served.fetch, integrity: integrity(archive) }), { downloaded: true });
  assert.equal(served.calls.length, 1);
  assert.equal(createHash('sha256').update(await readFile(join(target, 'sdk.mjs'))).digest('hex'), CLAUDE_SDK_MODULE_SHA256);
  assert.equal(typeof (await loadSdk(target)).query, 'function');
});
