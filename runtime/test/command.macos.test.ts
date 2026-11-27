import { fixtureDirectory, socketTempRoot } from './support/temp.ts';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, writeFile, readFile, realpath, rm, symlink, chmod, stat, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { createServer as unixServer } from 'node:net';
import { randomUUID } from 'node:crypto';
import { command, COMMAND_TIMEOUT_MS } from '../src/command.ts';
import { fileAction } from '../src/safety.ts';
import { prepareBuildCache } from '../src/build-cache.ts';
import { exec, setup } from './support/safety.ts';

test('sandboxed command streams preserve split UTF-8 on both output pipes', { timeout: 15000 }, async t => {
  const { root } = await setup(t);
  const script = 'process.stdout.write(Buffer.from([0xe2]));process.stderr.write(Buffer.from([0xf0,0x9f]));setTimeout(()=>{process.stdout.write(Buffer.from([0x82,0xac]));process.stderr.write(Buffer.from([0x9a,0x80]));},30);';
  const chunks: string[] = [];
  const result = await command(root, `'${process.execPath}' -e '${script}'`, [], new AbortController().signal, chunk => chunks.push(chunk));
  assert(result.includes('€')); assert(result.includes('🚀'));
  assert(!result.includes('�'));
  assert.equal(chunks.join(''), result);
});

test('sandbox reuses compiled artifacts across commands in a private thread cache', { timeout: 30000 }, async t => {
  const { root, base } = await setup(t);
  const store = join(base, 'store'); await mkdir(store, { mode: 0o700 });
  const cache = { store, thread: randomUUID() };
  await mkdir(join(root, 'src'));
  await writeFile(join(root, 'Cargo.toml'), '[package]\nname="plait-sandbox-proof"\nversion="0.1.0"\nedition="2024"\n[dependencies]\nitoa="=1.0.19"\n');
  await writeFile(join(root, 'rust-toolchain.toml'), '[toolchain]\nchannel="1.98.1"\n');
  await writeFile(join(root, 'src/lib.rs'), '#[test] fn works() { assert_eq!(2 + 2, 4); }\n');
  await exec('cargo', ['generate-lockfile', '--offline'], { cwd: root });
  const run = (text: string) => command(root, text, [], new AbortController().signal, () => {}, undefined, undefined, cache);
  assert.match(await run('cargo build --offline --locked && cargo test --offline --locked'), /1 passed/);
  const { target } = await prepareBuildCache(cache);
  const deps = join(target, 'debug', 'deps');
  const artifact = join(deps, (await readdir(deps)).find(name => name.startsWith('libitoa-') && name.endsWith('.rlib'))!);
  const before = await stat(artifact, { bigint: true });
  const warm = await run('cargo test --offline --locked --verbose');
  assert.match(warm, /Fresh itoa/); assert.doesNotMatch(warm, /Compiling/);
  assert.equal((await stat(artifact, { bigint: true })).mtimeNs, before.mtimeNs);
});

test('thread caches deny sibling/store access, symlink escapes and root replacement', { timeout: 30000 }, async t => {
  const { root, base } = await setup(t);
  const store = join(base, 'store'); await mkdir(store, { mode: 0o700 });
  const cache = { store, thread: randomUUID() };
  const first = await prepareBuildCache(cache);
  const sibling = await prepareBuildCache({ store, thread: randomUUID() });
  const secret = join(store, 'auth.json'); await writeFile(secret, 'private-fixture');
  const other = join(sibling.target, 'keep'); await writeFile(other, 'keep');
  const run = (text: string) => command(root, text, [], new AbortController().signal, () => {}, undefined, undefined, cache);
  await run('printf cached > "$CARGO_TARGET_DIR/own"');
  assert.equal(await readFile(join(first.target, 'own'), 'utf8'), 'cached');
  for (const path of [secret, other]) {
    await assert.rejects(run(`cat '${path}'`));
    await assert.rejects(run(`printf bad > '${path}'`));
  }
  await run(`ln -s '${sibling.target}' "$CARGO_TARGET_DIR/escape"`);
  await assert.rejects(run('printf bad > "$CARGO_TARGET_DIR/escape/keep"'));
  await assert.rejects(run('mv "$CARGO_TARGET_DIR" "$TMPDIR/stolen"'));
  await assert.rejects(run('mv "$CARGO_HOME" "$TMPDIR/stolen"'));
  await assert.rejects(run('chmod 777 "$CARGO_HOME"'));
  assert.equal(await readFile(other, 'utf8'), 'keep');
  assert.equal(await readFile(secret, 'utf8'), 'private-fixture');
});

test('Stop kills a command promptly under the fifteen-minute cap and leaves cache usable', { timeout: 15000 }, async t => {
  const { root, base } = await setup(t);
  const store = join(base, 'store'); await mkdir(store, { mode: 0o700 });
  const cache = { store, thread: randomUUID() };
  assert.equal(COMMAND_TIMEOUT_MS, 900000);
  const controller = new AbortController();
  let pid = 0;
  await assert.rejects(command(root, 'echo READY $$; sleep 120', [], controller.signal, chunk => {
    const match = chunk.match(/READY (\d+)/);
    if (match) { pid = Number(match[1]); controller.abort(); }
  }, undefined, undefined, cache));
  assert(pid > 0); assert.throws(() => process.kill(pid, 0));
  assert.match(await command(root, 'printf usable', [], new AbortController().signal, () => {}, undefined, undefined, cache), /usable/);
});

test('sandbox compiles and links Metal with installed tools and no discovery services', { timeout: 30000 }, async t => {
  const { root } = await setup(t);
  const result = await command(root, `printf '#include <metal_stdlib>\\nusing namespace metal;\\nkernel void noop() {}\\n' > "$TMPDIR/input.metal" && xcrun -sdk "$SDKROOT" metal -c "$TMPDIR/input.metal" -o "$TMPDIR/out.air" && xcrun -sdk "$SDKROOT" metallib "$TMPDIR/out.air" -o "$TMPDIR/out.metallib" && test -s "$TMPDIR/out.metallib" && printf METAL_OK`, [], new AbortController().signal, () => {});
  assert.match(result, /METAL_OK/);
});

test('sandbox denies outside/symlink writes, Git writes, network and baseline dirty commands', { timeout: 15000 }, async t => {
  const { root, base } = await setup(t);
  const outside = join(base, 'protected.txt'); await writeFile(outside, 'keep');
  await symlink(base, join(root, 'escape'));
  await writeFile(join(root, 'source.txt'), 'source');
  const run = (text: string, baseline: string[] = []) => command(root, text, baseline, new AbortController().signal, () => {});
  await assert.rejects(run(`printf bad > '${outside}'`));
  await assert.rejects(run('printf bad > escape/protected.txt'));
  await assert.rejects(run('printf bad > .git/config'));
  await assert.rejects(run('printf bad > .GIT/config'));
  await assert.rejects(run('printf bad > .Git/config'));
  await assert.rejects(run('ln source.txt "$TMPDIR/linked" && printf bad > "$TMPDIR/linked"'));
  assert.equal(await readFile(join(root, 'source.txt'), 'utf8'), 'source');
  await assert.rejects(run('printf bad > anything', ['notes.txt']));
  assert.match(await run('printf safe', ['notes.txt']), /safe/);
  assert.equal(await readFile(outside, 'utf8'), 'keep');
  let hits = 0;
  const server = createServer((_request, response) => { hits++; response.end('unsafe'); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const port = (server.address() as { port: number }).port;
    await assert.rejects(run(`/usr/bin/curl --max-time 2 http://127.0.0.1:${port}`));
    assert.equal(hits, 0);
  } finally { await new Promise<void>(resolve => server.close(() => resolve())); }
});

test('scratch Git and child signals work without checkout writes or external signals', { timeout: 15000 }, async t => {
  const { root } = await setup(t);
  const sentinel = spawn('/bin/sleep', ['30'], { stdio: 'ignore' });
  t.after(() => { sentinel.kill('SIGKILL'); });
  const run = (text: string) => command(root, text, [], new AbortController().signal, () => {});
  assert.match(await run('git init -q "$TMPDIR/fixture" && git -C "$TMPDIR/fixture" status --porcelain && printf SCRATCH_GIT_OK'), /SCRATCH_GIT_OK/);
  await assert.rejects(run(`ln -s '${root}/.git' "$TMPDIR/git-alias"; printf bad > "$TMPDIR/git-alias/config"`));
  await assert.rejects(run(`kill -TERM ${sentinel.pid}`));
  process.kill(sentinel.pid!, 0);
  assert.match(await run('sleep 5 & pid=$!; kill -TERM "$pid"; wait "$pid"; test $? -gt 128 && printf CHILD_STOPPED'), /CHILD_STOPPED/);
});

test('Unix IPC stays in scratch and cannot connect to host or sibling cache sockets', { timeout: 15000 }, async t => {
  const { root } = await setup(t);
  const store = await realpath(await fixtureDirectory(join(socketTempRoot, 'p-ipc-')));
  t.after(async () => { await rm(store, { recursive: true, force: true }); });
  const cache = { store, thread: randomUUID() };
  const sibling = await prepareBuildCache({ store, thread: randomUUID() });
  const run = (text: string) => command(root, text, [], new AbortController().signal, () => {}, undefined, undefined, cache);
  const script = 'const net=require("net");const d=process.env.TMPDIR+"/plait-client-"+process.pid;require("fs").mkdirSync(d);const p=d+"/host.sock";const s=net.createServer(c=>{c.end("LOCAL_IPC_OK");s.close()});s.listen(p,()=>{net.connect(p).on("data",d=>process.stdout.write(d))});';
  assert.match(await run(`'${process.execPath}' -e '${script}'`), /LOCAL_IPC_OK/);
  for (const path of [join(store, 'host.sock'), join(sibling.target, 's')]) {
    let hits = 0;
    const server = unixServer(socket => { hits++; socket.destroy(); });
    await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(path, resolve); });
    try {
      await assert.rejects(run(`/usr/bin/nc -z -w 1 -U '${path}'`));
      assert.equal(hits, 0);
    } finally { await new Promise<void>(resolve => server.close(() => resolve())); }
  }
});

test('edit opens originals read-only and reports fixed mismatch diagnostics', async t => {
  const { root } = await setup(t);
  const path = join(root, 'readonly.txt');
  await writeFile(path, 'before\n');
  await exec('/usr/bin/xattr', ['-w', 'com.plait.test', 'read-only-original', path]);
  await chmod(path, 0o444);
  const before = await stat(path, { bigint: true });
  const image = (await fileAction(root, 'read', 'readonly.txt')).postimage;
  await assert.rejects(fileAction(root, 'edit', 'readonly.txt', { expected: 'not present', content: 'bad', expected_image: image }), /old_text must match exactly once/);
  assert.equal((await stat(path, { bigint: true })).ctimeNs, before.ctimeNs);
  await fileAction(root, 'edit', 'readonly.txt', { expected: 'before', content: 'after', expected_image: image });
  assert.equal(await readFile(path, 'utf8'), 'after\n');
  assert.equal((await stat(path)).mode & 0o777, 0o444);
});

test('a sandboxed command in a linked worktree reads, but cannot write, the main repository', { timeout: 15000 }, async t => {
  const { root, base } = await setup(t);
  const git = (cwd: string, ...args: string[]) => exec('/usr/bin/git', ['-C', cwd, '-c', 'user.name=Plait', '-c', 'user.email=plait@example.invalid', ...args]);
  await writeFile(join(root, 'README.md'), 'hello\n');
  await git(root, 'add', 'README.md'); await git(root, 'commit', '-q', '-m', 'First');
  const tree = join(base, 'repo.worktrees', 'side');
  await git(root, 'worktree', 'add', '-q', '-b', 'plait/side', tree, 'HEAD');
  const run = (text: string) => command(tree, text, [], new AbortController().signal, () => {});
  await writeFile(join(tree, 'README.md'), 'changed\n');
  assert.match(await run('git status --porcelain && git log -1 --format=%s'), /M README\.md[\s\S]*First/);
  await assert.rejects(run('git branch escaped'));
  await assert.rejects(git(root, 'rev-parse', '--verify', '-q', 'refs/heads/escaped'));
});

test('skill folders passed to the sandbox are readable and never writable', { timeout: 30000 }, async t => {
  const { root, base } = await setup(t);
  const skill = join(base, 'skills', 'notes');
  await mkdir(skill, { recursive: true });
  await writeFile(join(skill, 'SKILL.md'), 'skill body');
  const run = (text: string, readable: string[] = []) =>
    command(root, text, [], new AbortController().signal, () => {}, undefined, undefined, undefined, undefined, readable);
  await assert.rejects(run(`cat '${join(skill, 'SKILL.md')}'`), 'not readable unless passed');
  assert.match(await run(`cat '${join(skill, 'SKILL.md')}'`, [skill]), /skill body/);
  await assert.rejects(run(`echo x > '${join(skill, 'SKILL.md')}'`, [skill]));
  assert.equal(await readFile(join(skill, 'SKILL.md'), 'utf8'), 'skill body');
});
