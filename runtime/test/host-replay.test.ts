import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { binary, delay, exec, request, until } from './support/host.ts';
import { replayKey, replayModel, replayServer } from './support/replay.ts';

const provider = 'plait-replay';
const canary = 'PLAIT_LOCAL_REPLAY_CANARY_83b07c';

test('production Pi runtime routes loopback replay, retains instructions, and requires effect approval', { timeout: 30000 }, async t => {
  const base = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-replay-')));
  const repo = join(base, 'repo'); const store = join(base, 'store');
  const home = join(base, 'home'); const agent = join(home, '.pi', 'agent');
  let host: ChildProcess | undefined;
  let replay: Awaited<ReturnType<typeof replayServer>> | undefined;
  t.after(async () => {
    if (host?.exitCode === null && host.signalCode === null) host.kill('SIGKILL');
    await delay(400);
    await replay?.close();
    await rm(base, { recursive: true, force: true });
  });
  replay = await replayServer([
    { text: 'Loopback production-runtime fixture response.' },
    { tool: { id: 'fixture_edit', name: 'edit', input: { path: 'notes.txt', old_text: 'before\n', new_text: 'not permitted\n' } } },
    { text: 'Fixture edit was denied.' },
  ]);
  for (const directory of [repo, store, agent]) await mkdir(directory, { recursive: true, mode: 0o700 });
  await writeFile(join(repo, 'AGENTS.md'), `Preserve the local canary: ${canary}\n`);
  await writeFile(join(repo, 'notes.txt'), 'before\n');
  await exec('/usr/bin/git', ['init', '-q', repo]);
  await exec('/usr/bin/git', ['-C', repo, 'add', '.']);
  await exec('/usr/bin/git', ['-C', repo, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', 'commit', '-qm', 'fixture']);
  await writeFile(join(agent, 'models.json'), JSON.stringify({ providers: { [provider]: {
    baseUrl: replay.baseUrl, api: 'anthropic-messages', apiKey: replayKey,
    models: [{ id: replayModel, name: 'Local replay fixture', reasoning: false, contextWindow: 32768, maxTokens: 1024 }],
  } } }), { mode: 0o600 });

  // Use the normal sidecar entrypoint, with no inherited provider credentials or user configuration.
  host = spawn(binary, ['run', '--store', store], {
    stdio: ['ignore', 'ignore', 'pipe'],
    env: { PATH: process.env.PATH, HOME: home, PI_CODING_AGENT_DIR: agent, PI_OFFLINE: '1' },
  });
  let stderr = '';
  host.stderr!.on('data', chunk => { stderr = (stderr + chunk.toString()).slice(-8192); });
  host.once('error', error => { stderr = error.message; });
  await until(async () => {
    if (host!.exitCode !== null || host!.signalCode !== null) throw new Error(`Production host exited: ${stderr}`);
    return request(store, { op: 'status' });
  }, reply => reply.ok);
  const models = await request(store, { op: 'status' });
  assert.equal(models.ok, true);
  assert(models.result.models.some(value => {
    const model = value as { provider: string; model_id: string; authenticated: boolean };
    return model.provider === provider && model.model_id === replayModel && model.authenticated;
  }));
  const created = await request(store, { op: 'create_thread', checkout: repo, provider, model_id: replayModel, request_id: 'replay-create' });
  assert.equal(created.ok, true, JSON.stringify(created));
  const id = created.result.thread.id;
  const submit = (text: string, request_id: string) => request(store, { op: 'submit', thread_id: id, text, request_id });
  const watch = async () => (await request(store, { op: 'watch', thread_id: id })).result.thread;
  assert.equal((await submit('Reply using the local fixture.', 'replay-first')).ok, true);
  const first = await until(watch, snapshot => snapshot.status === 'idle' || snapshot.status === 'error');
  assert.equal(first.status, 'idle', JSON.stringify(first));
  assert(first.messages.some(message => message.role === 'assistant' && message.text === 'Loopback production-runtime fixture response.'));
  assert.equal(replay.requests.length, 1);

  assert.equal((await submit('Propose the fixture edit.', 'replay-second')).ok, true);
  const pending = await until(watch, snapshot => snapshot.pending_approval !== null || snapshot.status === 'error');
  assert.equal(pending.pending_approval?.kind, 'edit', JSON.stringify(pending));
  assert.equal(await readFile(join(repo, 'notes.txt'), 'utf8'), 'before\n');
  assert.equal((await request(store, { op: 'approve', thread_id: id, approval_id: pending.pending_approval!.id, allow: false })).ok, true);
  const done = await until(watch, snapshot => snapshot.status === 'idle' || snapshot.status === 'error');
  assert.equal(done.status, 'idle', JSON.stringify(done));
  assert.equal(await readFile(join(repo, 'notes.txt'), 'utf8'), 'before\n');
  assert(done.messages.some(message => message.text === 'Fixture edit was denied.'));
  assert.equal(replay.requests.length, 3);
  for (const captured of replay.requests) {
    assert.equal(captured.key, replayKey);
    assert.equal(captured.body.model, replayModel);
    assert.equal(captured.body.stream, true);
    assert(JSON.stringify(captured.body.system).includes(canary));
  }
  assert(JSON.stringify(replay.requests[0].body.messages).includes('Reply using the local fixture.'));
  assert(JSON.stringify(replay.requests[1].body.messages).includes('Loopback production-runtime fixture response.'));
  const tools = replay.requests[0].body.tools as { name: string }[];
  assert.deepEqual(tools.map(tool => tool.name).sort(), ['command', 'edit', 'read', 'read_output', 'write']);
  const messages = replay.requests[2].body.messages as { content: unknown }[];
  const blocks = messages.flatMap(message => Array.isArray(message.content) ? message.content : []) as {
    type: string; tool_use_id?: string; is_error?: boolean; content?: unknown;
  }[];
  const denial = blocks.find(block => block.type === 'tool_result' && block.tool_use_id === 'fixture_edit');
  assert(denial);
  assert.equal(denial.is_error, true);
  assert.match(JSON.stringify(denial.content), /Tool permission denied or revoked/);
});
