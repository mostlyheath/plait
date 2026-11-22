import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { installable, installBuild, installTool, managedPath } from '../src/agent-tools.ts';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import { readFile, readdir, stat } from 'node:fs/promises';
import { findCodex } from '../src/codex/install.ts';
import { SignIns } from '../src/sign-in.ts';
import { harnessLogin } from '../src/harness-sign-in.ts';
import { validate } from '../src/protocol.ts';

async function script(path: string, body: string) { await mkdir(dirname(path), { recursive: true }); await writeFile(path, `#!/bin/sh\n${body}\n`, { mode: 0o755 }); }
test('unsupported builds and integrity mismatch leave no managed executable', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-tools-'));
  try {
    assert.equal(installable('codex', { platform: 'plan9', arch: 'x64' }), null);
    await assert.rejects(installTool('codex', { directory: dir, platform: 'plan9' }), /plan9/);
    await assert.rejects(installTool('codex', { directory: dir, platform: 'linux', arch: 'x64', fetch: async () => new Response('bad') }), /pinned hash/);
    assert.equal(await managedPath('codex', dir, { platform: 'linux', arch: 'x64' }), undefined);
    assert.throws(() => validate({ op: 'install_harness', harness: 'unknown' }));
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('configured and PATH Codex take precedence over managed', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-find-'));
  try {
    const managed = join(dir, 'tools/codex/0.169.0/bin/codex'); const onPath = join(dir, 'path/codex'); const configured = join(dir, 'chosen/codex');
    for (const path of [managed, onPath, configured]) await script(path, "echo 'codex-cli 0.169.0'");
    const env = { PATH: join(dir, 'path') }; const directory = join(dir, 'tools');
    assert.equal((await findCodex({ path: configured, env, directory })).path, configured);
    assert.equal((await findCodex({ env, directory })).path, onPath);
    assert.equal((await findCodex({ env: { PATH: join(dir, 'empty') }, directory })).path, managed);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('Claude sign-in relays a secret', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-login-')); const executable = join(dir, 'claude'); const flows = new SignIns({ settleMs: 1000 });
  try {
    await script(executable, "echo https://example.com/login; read code; [ \"$code\" = good ] || { echo 'Code rejected' >&2; exit 1; }");
    const view = await flows.start('harness:claude', undefined, harnessLogin('claude', executable, process.env), async () => {});
    assert.equal(view.url, 'https://example.com/login');
    assert.equal((await flows.answer(view.id, 'good')).state, 'signed_in');
  } finally { await flows.close(); await rm(dir, { recursive: true, force: true }); }
});
test('Codex device code and CLI failure are relayed', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-codex-login-')); const executable = join(dir, 'codex'); const flows = new SignIns({ settleMs: 1000 });
  try {
    await script(executable, "printf '\\033[32mhttps://example.com/device\\033[0m ABCD-1234\\n' >&2; sleep 1; exit 1");
    const view = await flows.start('harness:codex', undefined, harnessLogin('codex', executable, process.env), async () => {});
    assert.equal(view.user_code, 'ABCD-1234');
    await new Promise(resolve => setTimeout(resolve, 1200));
    assert.equal(flows.status(view.id).state, 'failed');
  } finally { await flows.close(); await rm(dir, { recursive: true, force: true }); }
});

test('a build unpacks only its own tree, keeps exec bits, and installs once', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-tools-ok-'));
  try {
    const source = join(dir, 'source');
    await script(join(source, 'package/vendor/triple/bin/codex'), "echo 'codex-cli 0.169.0'");
    await script(join(source, 'package/vendor/triple/codex-path/rg'), 'exit 0');
    await writeFile(join(source, 'package/vendor/triple/README'), 'readme', { mode: 0o644 });
    await writeFile(join(source, 'package/bin-codex.js'), 'left out');
    const archive = join(dir, 'codex.tgz');
    await promisify(execFile)('tar', ['-czf', archive, '-C', source, 'package']);
    const bytes = await readFile(archive);
    const build = { url: 'https://registry.invalid/codex.tgz', integrity: `sha512-${createHash('sha512').update(bytes).digest('base64')}`,
      prefix: 'package/vendor/triple/', executable: 'bin/codex', version: '0.169.0' };
    let downloads = 0;
    const fetch = async () => { downloads++; return new Response(bytes); };
    const tools = join(dir, 'tools');
    await installBuild('codex', build, { directory: tools, fetch });
    const root = join(tools, 'codex/0.169.0');
    assert.deepEqual((await readdir(root)).sort(), ['README', 'bin', 'codex-path']);
    assert.ok((await stat(join(root, 'bin/codex'))).mode & 0o100);
    assert.ok((await stat(join(root, 'codex-path/rg'))).mode & 0o100);
    assert.equal((await stat(join(root, 'README'))).mode & 0o111, 0);
    // A second install keeps the copy and downloads nothing.
    await installBuild('codex', build, { directory: tools, fetch });
    assert.equal(downloads, 1);
    assert.deepEqual((await readdir(join(tools, 'codex'))), ['0.169.0']);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('cancelling a sign-in stops the CLI', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'plait-login-cancel-')); const executable = join(dir, 'claude'); const flows = new SignIns({ settleMs: 1000 });
  const pid = join(dir, 'pid');
  try {
    await script(executable, `echo $$ > '${pid}'; echo https://example.com/login; read code; exit 0`);
    const view = await flows.start('harness:claude', undefined, harnessLogin('claude', executable, process.env), async () => {});
    assert.equal(view.url, 'https://example.com/login');
    const child = Number((await readFile(pid, 'utf8')).trim());
    assert.equal(flows.cancel(view.id).state, 'cancelled');
    const deadline = Date.now() + 3000;
    const alive = () => { try { process.kill(child, 0); return true; } catch { return false; } };
    while (alive() && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 50));
    assert.equal(alive(), false);
  } finally { await flows.close(); await rm(dir, { recursive: true, force: true }); }
});
