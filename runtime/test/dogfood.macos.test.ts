import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { cp, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { connect } from 'node:net';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { binary, delay, exec, request, until } from './support/host.ts';
import { replayKey, replayModel, replayServer } from './support/openai-replay.ts';
import { installedClient } from './support/installed-ui.ts';
import type { Snapshot } from '../src/protocol.ts';

// This is real Plait source and real Cargo execution, but a scripted model, not a live account.
test('production runtime edits Plait source, runs Cargo, reconnects and resumes without replay', { timeout: 240000 }, async t => {
  const base = await realpath(await mkdtemp('/tmp/plait-dogfood-'));
  const repo = join(base, 'repo'); const store = join(base, 'store'); const agent = join(base, 'pi');
  const source = resolve(import.meta.dirname, '../..');
  const path = 'crates/plait-client/src/session.rs';
  const before = await readFile(join(source, path), 'utf8');
  const oldConstant = 'const FILE_LIMIT: usize = 4 * 1024 * 1024;';
  const newConstant = oldConstant + '\nconst TITLE_LIMIT: usize = 72;';
  const marker = '    #[test]\n    fn titles_are_plain_whitespace_normalized_and_unicode_safe()';
  const regression = '    #[test]\n    fn dogfood_normalizes_before_applying_the_title_limit() {\n'
    + '        let input = format!(" \\t{} \\n", "x".repeat(TITLE_LIMIT + 1));\n'
    + '        assert_eq!(title(&input), "x".repeat(TITLE_LIMIT));\n    }\n\n' + marker;
  assert(before.includes('.take(72)') && before.includes(oldConstant) && before.includes(marker), 'Update the dogfood fixture when the title implementation changes.');
  const edit = (id: string, old_text: string, new_text: string) => ({ tool: { id, name: 'edit', input: { path, old_text, new_text } } });
  // Cargo builds the crate and runs the tests of the module the turn edits. The
  // whole core suite belongs to the workspace check, not inside this deadline.
  const command = (id: string) => ({ tool: { id, name: 'command', input: {
    command: `sleep ${id === 'test_refactor' && process.env.PLAIT_VERIFY_INSTALLED_UI === '1' ? 8 : 1}; cargo test --offline --locked -p plait-client --lib session::`,
  } } });
  const replay = await replayServer([
    edit('extract_limit', '.take(72)', '.take(TITLE_LIMIT)'), edit('name_limit', oldConstant, newConstant),
    edit('add_regression', marker, regression), command('test_refactor'), { text: '## Refactor verified\n\nScripted provider; real approved edits and Cargo tests.\n\n```rust\nconst TITLE_LIMIT: usize = 72;\n```\n\nThe regression test passed. This is not a live-model session.' },
    edit('restore_limit', '.take(TITLE_LIMIT)', '.take(72)'), edit('remove_constant', newConstant, oldConstant),
    edit('remove_regression', regression, marker), command('test_original'), { text: 'Scripted fixture: the follow-up passed with the original source restored.' },
  ]);
  const hosts: ChildProcess[] = [];
  let client: Awaited<ReturnType<typeof installedClient>>;
  t.after(async () => {
    try { await client?.close(); }
    finally {
      for (const host of hosts) if (host.exitCode === null && host.signalCode === null) host.kill('SIGKILL');
      await delay(500); await replay.close(); await rm(base, { recursive: true, force: true });
    }
  });
  for (const directory of [repo, store, agent]) await mkdir(directory, { recursive: true, mode: 0o700 });
  // No user's Git metadata, ignored files, credentials, or build output enter the disposable checkout.
  await cp(join(source, 'crates'), join(repo, 'crates'), { recursive: true });
  // plait-client embeds the bundled theme schemes at compile time.
  await cp(join(source, 'assets', 'themes'), join(repo, 'assets', 'themes'), { recursive: true });
  for (const file of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'AGENTS.md']) {
    await cp(join(source, file), join(repo, file));
  }
  const environment = { PATH: process.env.PATH, HOME: process.env.HOME, PI_CODING_AGENT_DIR: agent, PI_OFFLINE: '1', PLAIT_LEASE_DIR: process.env.PLAIT_LEASE_DIR };
  assert(environment.HOME, 'Installed Rust toolchain discovery needs the test account home.');
  const git = (args: string[]) => exec('/usr/bin/git', ['-C', repo, ...args], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' },
  });
  await git(['init', '-q']); await git(['add', '.']);
  await git(['-c', 'user.name=Plait Fixture', '-c', 'user.email=test@invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'fixture']);
  await writeFile(join(repo, 'existing-dirty.txt'), 'Do not change this pre-existing work.\n');
  const index = await readFile(join(repo, '.git/index'));
  await writeFile(join(agent, 'models.json'), JSON.stringify({ providers: { 'plait-replay': {
    baseUrl: replay.baseUrl, api: 'openai-completions', apiKey: replayKey,
    models: [{ id: replayModel, name: 'Local replay fixture', reasoning: false, contextWindow: 32768, maxTokens: 1024 }],
  } } }), { mode: 0o600 });
  const start = async () => {
    const host = spawn(binary, ['run', '--store', store], { stdio: ['ignore', 'ignore', 'pipe'], env: environment });
    hosts.push(host);
    let errors = ''; host.stderr!.on('data', chunk => { errors = (errors + chunk.toString()).slice(-8192); });
    try {
      await until(async () => {
        if (host.exitCode !== null || host.signalCode !== null) throw new Error(`Fixture host exited: ${errors}`);
        return request(store, { op: 'status' });
      }, reply => reply.ok);
    } catch (error) { throw new Error(`Fixture host did not become ready: ${errors}`, { cause: error }); }
    return host;
  };
  let host = await start();
  // This suite proves the sandboxed path end to end; trusted is the default.
  assert.equal((await request(store, { op: 'set_command_policy', checkout: repo, policy: 'sandboxed' })).ok, true);
  const created = await request(store, { op: 'create_thread', checkout: repo, provider: 'plait-replay', model_id: replayModel, request_id: 'dogfood-create' });
  assert.equal(created.ok, true, JSON.stringify(created));
  const id = created.result.thread.id;
  client = await installedClient(base, store, repo, created.result.thread);
  const watch = async () => (await request(store, { op: 'watch', thread_id: id })).result.thread;
  const submit = (request_id: string) => request(store, { op: 'submit', thread_id: id, request_id, text: 'Perform the next scripted fixture turn.' });
  for (const turn of [1, 2]) {
    const requestId = `dogfood-${turn}`;
    assert.equal((await submit(requestId)).ok, true);
    const answered = new Set<string>();
    let disconnected = false;
    let closedNativeWhileActive = !client || turn !== 1;
    const deadline = Date.now() + 180000;
    let done: Snapshot | undefined;
    while (Date.now() < deadline) {
      const snapshot = await watch();
      if (snapshot.pending_approval && !answered.has(snapshot.pending_approval.id)) {
        answered.add(snapshot.pending_approval.id);
        assert.equal((await request(store, { op: 'approve', thread_id: id, approval_id: snapshot.pending_approval.id, allow: true })).ok, true);
        if (snapshot.pending_approval.kind === 'command') {
          const observer = connect(join(store, 'host.sock'));
          observer.on('error', () => {}); observer.on('data', () => {});
          await new Promise<void>((resolve, reject) => { observer.once('error', reject); observer.once('connect', resolve); });
          observer.write(JSON.stringify({ op: 'watch', thread_id: id }) + '\n');
          await delay(100); observer.destroy(); disconnected = true;
          assert.equal((await watch()).status, 'running');
          if (client && turn === 1) {
            await client.open(true); await client.capture('active'); await client.close();
            assert.equal((await watch()).status, 'running', 'Closing the native client must not cancel the command.');
            closedNativeWhileActive = true;
          }
        }
      }
      if (['idle', 'error', 'stopped'].includes(snapshot.status)) { done = snapshot; break; }
      await delay(40);
    }
    assert(done, 'The coding turn did not finish within its deadline.');
    assert.equal(done.status, 'idle', JSON.stringify(done));
    assert.equal(answered.size, 4);
    assert(disconnected);
    assert(closedNativeWhileActive, 'The installed client must open and close during accepted work.');
    assert(done.tools.every(tool => tool.status === 'done'), JSON.stringify(done.tools));
    const commands = done.tools.filter(tool => tool.name === 'command');
    assert.equal(commands.length, turn);
    assert.match(commands.at(-1)!.output, /test result: ok/);
    // The first turn's run includes the regression test it added; the second, with the source restored, does not.
    assert.equal(/dogfood_normalizes_before_applying_the_title_limit \.\.\. ok/.test(commands.at(-1)!.output), turn === 1);
    assert.match(commands.at(-1)!.output, /titles_are_plain_whitespace_normalized_and_unicode_safe \.\.\. ok/);
    assert.equal(replay.requests.length, turn * 5);
    assert.equal((await submit(requestId)).ok, true);
    assert.equal(replay.requests.length, turn * 5, 'Retry must not call the model again.');
    assert.deepEqual(await readFile(join(repo, '.git/index')), index);
    assert.equal(await readFile(join(repo, 'existing-dirty.txt'), 'utf8'), 'Do not change this pre-existing work.\n');
    if (turn === 1) {
      assert.match((await git(['diff', '--', path])).stdout, /TITLE_LIMIT/);
      if (client) {
        await client.open(false); await client.capture('coding'); await client.close();
        assert.equal(replay.requests.length, 5, 'Reopening a saved draft must not submit it.');
      }
      const messages = done.messages.map(message => message.id);
      const tools = done.tools.map(tool => tool.id);
      host.kill('SIGTERM'); await until(async () => host.signalCode !== null, value => value); await delay(500);
      host = await start();
      const restored = await watch();
      assert.equal(restored.status, 'idle');
      assert.deepEqual(restored.messages.map(message => message.id), messages);
      assert.deepEqual(restored.tools.map(tool => tool.id), tools);
      await delay(100); assert.equal(replay.requests.length, 5);
    }
  }
  assert.equal(await readFile(join(repo, path), 'utf8'), before);
  assert.equal(await readFile(join(source, path), 'utf8'), before, 'The source checkout must not be edited by the acceptance fixture.');
  for (const captured of replay.requests) {
    assert.equal(captured.key, `Bearer ${replayKey}`); assert.equal(captured.body.model, replayModel);
  }
});
