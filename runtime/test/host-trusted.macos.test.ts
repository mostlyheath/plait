import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readdir, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { after, test } from 'node:test';
import { TRUSTED_COMMAND_DESCRIPTION, TRUSTED_COMMAND_SUMMARY, TRUSTED_INSTRUCTIONS, TRUSTED_RISK } from '../src/command/policy.ts';
import type { Approval, Snapshot } from '../src/protocol.ts';
import { delay, exec, request, setup, until } from './support/host.ts';

// Trusted commands receive the account's login environment. Every host in this
// file starts with a disposable HOME, so nothing reads or writes the real one,
// and the developer's SSH agent stays out of the commands' environment.
const home = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-trusted-home-')));
process.env.HOME = home;
delete process.env.SSH_AUTH_SOCK;
after(async () => { await rm(home, { recursive: true, force: true }); });

// The wording the sandboxed mode used before trusted mode existed. It must not change.
const SANDBOXED_INSTRUCTIONS = 'You are Plait, a local coding assistant. File tools are the only checkout writers; edits and commands may require the user\'s approval. Commands read the checkout and write only private scratch (HOME/TMPDIR) and this thread\'s private build cache (CARGO_HOME/CARGO_TARGET_DIR), with no external network and a 15 minute limit. Tests may use scratch-local Git repositories and Unix sockets. Use offline locked builds. Unchanged files this thread previously wrote can be edited again. Never modify other dirty or ignored paths, bypass approval, or start daemons. Read project instructions below.\n';
const SANDBOXED_DESCRIPTION = 'Run a command (may require approval): checkout read-only, writes only to private TMPDIR and this thread\'s CARGO_HOME/CARGO_TARGET_DIR cache, no external network or checkout/Git writes, 15 minute limit. Tests may use scratch-local Git repositories and Unix sockets. Do not start background services. Use offline locked builds.';
const SANDBOXED_RISK = 'Only this exact call is approved. Dirty and ignored paths are protected. Commands write only private scratch and a private per-thread build cache; no external network or checkout/Git writes. Tests may use scratch-local Git repositories and Unix sockets. Read tools and commands can read ignored checkout files and send their contents to the provider.';

type Fixture = Awaited<ReturnType<typeof setup>>;
type PolicyReply = { ok: boolean; result: { checkout: string; policy: string }; error?: { code: string } };
const policy = (f: Fixture, value?: string, checkout = f.repo) =>
  request(f.store, value ? { op: 'set_command_policy', checkout, policy: value } : { op: 'command_policy', checkout }) as unknown as Promise<PolicyReply>;
const submitAuto = (f: Fixture, id: string, request_id = 'submit-1') =>
  request(f.store, { op: 'submit', thread_id: id, request_id, text: 'Do the scripted task.', protocol_version: 2, approval: 'auto' });
const git = (root: string, ...args: string[]) => exec('/usr/bin/git', ['-C', root, ...args],
  { env: { PATH: '/usr/bin:/bin', HOME: '/nonexistent', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' } });
const alive = (pid: number) => { try { process.kill(pid, 0); return true; } catch { return false; } };
const tool = (state: Snapshot, id: string) => state.tools.find(item => item.id === id);
const settled = (f: Fixture, id: string) => until(async () => existsSync(join(f.store, `${id}.settled`)), value => value);

/** Allows every prompt until the run ends, returning the prompts in order. */
async function allowAll(f: Fixture, id: string) {
  const approvals: Approval[] = [];
  for (;;) {
    const state = await until(() => f.watch(id), value => !!value.pending_approval || !['running', 'awaiting_approval'].includes(value.status));
    if (!state.pending_approval) return { state, approvals };
    approvals.push(state.pending_approval);
    assert.equal((await f.approve(id, state.pending_approval.id)).ok, true);
  }
}

async function pids(f: Fixture, id: string) {
  const running = await until(() => f.watch(id), value => value.tools.some(item => /READY \d+ \d+/.test(item.output)));
  const match = running.tools.map(item => item.output).join('').match(/READY (\d+) (\d+)/)!;
  return [Number(match[1]), Number(match[2])];
}

test('trusted is the default; a sandboxed project gets the same prompts but no checkout or Git writes', { timeout: 60000 }, async t => {
  const f = await setup(t, 'trusted-work', false, false);
  assert.deepEqual((await policy(f)).result, { checkout: f.repo, policy: 'trusted' });
  await policy(f, 'sandboxed');
  await writeFile(join(f.repo, 'dirty.txt'), 'dirty\n');
  assert.deepEqual((await policy(f)).result, { checkout: f.repo, policy: 'sandboxed' });
  const thread = await f.create();
  assert.equal((await f.submit(thread.id)).ok, true);
  const { state, approvals } = await allowAll(f, thread.id);
  assert.equal(state.status, 'idle', JSON.stringify(state));
  assert.deepEqual(approvals.map(approval => approval.summary), ['Run sandboxed command', 'Change notes.txt', 'Run sandboxed command', 'Change notes.txt']);
  assert(approvals.every(approval => approval.risk === SANDBOXED_RISK));
  assert.deepEqual(state.tools.map(item => [item.id, item.status]), [
    ['commit-1', 'error'], ['own-edit', 'done'], ['append-1', 'error'], ['later-edit', 'done'], ['unowned-edit', 'error'], ['dirty-edit', 'error'], ['dirty-write', 'error'],
  ]);
  assert.equal(existsSync(join(f.repo, 'generated.txt')), false);
  assert.equal(existsSync(join(f.repo, 'scratch.txt')), false);
  assert.equal((await git(f.repo, 'log', '--format=%s')).stdout, 'fixture\n');
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'again\n');
  assert.equal(await readFile(join(f.repo, 'dirty.txt'), 'utf8'), 'dirty\n');
});

test('trusted commands write and commit; the thread keeps its own files, and file tools refuse dirty and unowned ones', { timeout: 60000 }, async t => {
  const f = await setup(t, 'trusted-work');
  await writeFile(join(f.repo, 'dirty.txt'), 'dirty\n');
  assert.deepEqual((await policy(f, 'trusted')).result, { checkout: f.repo, policy: 'trusted' });
  const thread = await f.create();
  assert.equal((await f.submit(thread.id)).ok, true);
  const { state, approvals } = await allowAll(f, thread.id);
  assert.equal(state.status, 'idle', JSON.stringify(state));
  // The edits of the command's own new file and of the dirty file are refused before any prompt.
  assert.deepEqual(approvals.map(approval => [approval.kind, approval.summary]), [
    ['command', TRUSTED_COMMAND_SUMMARY], ['edit', 'Change notes.txt'], ['command', TRUSTED_COMMAND_SUMMARY], ['edit', 'Change notes.txt'],
  ]);
  assert.equal(TRUSTED_COMMAND_SUMMARY, 'Run command with full user access and network');
  assert(approvals.every(approval => approval.risk === TRUSTED_RISK && approval.dirty_paths.includes('dirty.txt')));
  assert.match(TRUSTED_RISK, /outside the sandbox, with network access/);
  assert.deepEqual(state.tools.map(item => [item.id, item.status]), [
    ['commit-1', 'done'], ['own-edit', 'done'], ['append-1', 'done'], ['later-edit', 'done'], ['unowned-edit', 'error'], ['dirty-edit', 'error'], ['dirty-write', 'error'],
  ]);
  assert.match(tool(state, 'commit-1')!.output, /COMMITTED/);
  assert.equal(await readFile(join(f.repo, 'generated.txt'), 'utf8'), 'made by command\n');
  assert.equal((await git(f.repo, 'log', '--format=%an <%ae> %s')).stdout, 'Fixture <fixture@invalid> trusted commit\nTest <test@invalid> fixture\n');
  // The thread edited its own file again after its command changed it.
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'again\nappended\n');
  // A file the command created was never the thread's, and the dirty file is untouched.
  assert.equal(await readFile(join(f.repo, 'scratch.txt'), 'utf8'), 'by command\n');
  assert.equal(await readFile(join(f.repo, 'dirty.txt'), 'utf8'), 'dirty\n');

  // The refreshed record is durable: the next run still owns that one file, and only it.
  await settled(f, thread.id);
  f.host.kill('SIGKILL'); await delay(500);
  const database = new DatabaseSync(join(f.store, 'host.sqlite'), { readOnly: true });
  try { assert.deepEqual(database.prepare('SELECT path FROM postimages WHERE thread_id=?').all(thread.id).map(row => row.path), ['notes.txt']); }
  finally { database.close(); }
  await f.start('trusted-followup');
  assert.equal((await policy(f)).result.policy, 'trusted');
  assert.equal((await submitAuto(f, thread.id, 'submit-2')).ok, true);
  const later = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(tool(later, 'followup-1')!.status, 'done', JSON.stringify(later));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'followup\nappended\n');
});

test('an install or in-place build of thousands of ignored files leaves file tools and the next run working', { timeout: 60000 }, async t => {
  const f = await setup(t, 'trusted-install');
  await writeFile(join(f.repo, '.gitignore'), 'node_modules/\n/target\n');
  await git(f.repo, 'add', '.gitignore');
  await git(f.repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@invalid', 'commit', '-qm', 'ignore build output');
  await policy(f, 'trusted');
  const thread = await f.create();
  assert.equal((await submitAuto(f, thread.id)).ok, true);
  const first = await until(() => f.watch(thread.id), value => value.status === 'idle' || value.status === 'error');
  assert.deepEqual(first.tools.map(item => [item.id, item.status]), [['install-1', 'done'], ['after-install-edit', 'done'], ['after-install-write', 'done']], JSON.stringify(first));
  assert.equal((await readdir(join(f.repo, 'node_modules', 'package', 'lib'))).length, 3000);
  assert.equal((await readdir(join(f.repo, 'target', 'debug', 'deps'))).length, 3000);

  // The directories now exist before the next run; each is one protected entry.
  await settled(f, thread.id);
  f.host.kill('SIGKILL'); await delay(500); await f.start('followup');
  assert.equal((await f.submit(thread.id, 'submit-2')).ok, true);
  const pending = await until(() => f.watch(thread.id), value => !!value.pending_approval || value.status === 'error');
  assert.deepEqual(pending.pending_approval?.dirty_paths.sort(), ['node_modules', 'target'], JSON.stringify(pending));
  await f.approve(thread.id, pending.pending_approval!.id);
  const second = await until(() => f.watch(thread.id), value => value.status === 'idle' || value.status === 'error');
  assert.equal(tool(second, 'followup-1')!.status, 'done', JSON.stringify(second));
  assert.equal(await readFile(join(f.repo, 'notes.txt'), 'utf8'), 'followup\n');
});

test('Stop kills a trusted command tree, and an approval after Stop does nothing', { timeout: 60000 }, async t => {
  const f = await setup(t, 'trusted-stop');
  await policy(f, 'trusted');
  const thread = await f.create();
  await f.submit(thread.id);
  const pending = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  assert.equal(pending.pending_approval!.summary, TRUSTED_COMMAND_SUMMARY);
  assert.equal((await request(f.store, { op: 'stop', thread_id: thread.id })).ok, true);
  assert.equal((await f.approve(thread.id, pending.pending_approval!.id)).error?.code, 'not_found');
  await settled(f, thread.id);
  assert.equal((await f.watch(thread.id)).status, 'stopped');
  assert.equal(existsSync(join(f.repo, 'started.txt')), false);

  // A new host fixture supplies a fresh scripted command; the saved policy survives restart.
  f.host.kill('SIGKILL'); await delay(500); await f.start('trusted-stop');
  assert.equal((await policy(f)).result.policy, 'trusted');
  assert.equal((await submitAuto(f, thread.id, 'submit-2')).ok, true);
  const tree = await pids(f, thread.id);
  assert.equal(await readFile(join(f.repo, 'started.txt'), 'utf8'), 'started');
  const stoppedAt = Date.now();
  assert.equal((await request(f.store, { op: 'stop', thread_id: thread.id })).ok, true);
  assert(Date.now() - stoppedAt < 5000);
  for (const pid of tree) await until(async () => !alive(pid), value => value);
  assert.equal((await f.watch(thread.id)).status, 'stopped');
});

test('host death kills a trusted command tree and never replays it', { timeout: 60000 }, async t => {
  const f = await setup(t, 'trusted-stop');
  await policy(f, 'trusted');
  const thread = await f.create();
  assert.equal((await submitAuto(f, thread.id)).ok, true);
  const tree = await pids(f, thread.id);
  f.host.kill('SIGKILL');
  for (const pid of tree) await until(async () => !alive(pid), value => value);
  await rm(join(f.repo, 'started.txt'));
  await delay(500); await f.start('answer');
  const reopened = await f.watch(thread.id);
  assert.equal(reopened.status, 'interrupted'); assert.equal(reopened.pending_approval, null);
  assert.equal(reopened.tools.filter(item => item.name === 'command').length, 1);
  assert.equal(existsSync(join(f.repo, 'started.txt')), false);
});

test('a run keeps the policy it was admitted with; a change applies to the next run', { timeout: 60000 }, async t => {
  const f = await setup(t, 'trusted-twice');
  await policy(f, 'trusted');
  const thread = await f.create();
  await f.submit(thread.id);
  const first = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  assert.equal(first.pending_approval!.summary, TRUSTED_COMMAND_SUMMARY);
  assert.equal((await policy(f, 'sandboxed')).result.policy, 'sandboxed');
  await f.approve(thread.id, first.pending_approval!.id);
  const ran = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(await readFile(join(f.repo, 'first.txt'), 'utf8'), 'one');
  // The command saw the host's HOME, which this file pointed at a disposable directory.
  assert.equal(tool(ran, 'first-1')!.output.split('\n[Command output:')[0], `FIRST ${home}`);
  assert.match(tool(ran, 'first-1')!.output, /read_output/);

  await f.submit(thread.id, 'submit-2');
  const second = await until(() => f.watch(thread.id), value => !!value.pending_approval);
  assert.equal(second.pending_approval!.summary, 'Run sandboxed command');
  assert.equal(second.pending_approval!.risk, SANDBOXED_RISK);
  // Trust granted while a sandboxed run is waiting does not reach that run.
  assert.equal((await policy(f, 'trusted')).result.policy, 'trusted');
  await f.approve(thread.id, second.pending_approval!.id);
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle');
  assert.equal(tool(done, 'second-1')!.status, 'error');
  assert.equal(existsSync(join(f.repo, 'second.txt')), false);
});

test('the model is told the mode in force, and the sandboxed wording is unchanged', { timeout: 60000 }, async t => {
  const f = await setup(t, 'record-context');
  const other = join(f.base, 'other'); await exec('/usr/bin/git', ['init', '-q', other]);
  await policy(f, 'trusted');
  await policy(f, 'sandboxed', other);
  const trusted = await f.create();
  const sandboxed = (await request(f.store, { op: 'create_thread', checkout: other, provider: 'faux', model_id: 'faux-1', request_id: 'create-other' })).result.thread;
  for (const thread of [trusted, sandboxed]) {
    assert.equal((await f.submit(thread.id)).ok, true);
    await until(() => f.watch(thread.id), value => value.status === 'idle');
  }
  const [toldTrusted, toldSandboxed] = (await readFile(join(f.store, 'model-contexts.jsonl'), 'utf8')).trim().split('\n');
  const within = (text: string) => JSON.stringify(text).slice(1, -1);
  assert(toldTrusted.includes(within(TRUSTED_INSTRUCTIONS)));
  assert(toldTrusted.includes(within(TRUSTED_COMMAND_DESCRIPTION)));
  assert(toldTrusted.includes(within('Keep the original dirty paths.')));
  assert(!toldTrusted.includes(within(SANDBOXED_INSTRUCTIONS)) && !toldTrusted.includes(within(SANDBOXED_DESCRIPTION)));
  assert(toldSandboxed.includes(within(SANDBOXED_INSTRUCTIONS)));
  assert(toldSandboxed.includes(within(SANDBOXED_DESCRIPTION)));
  assert(!toldSandboxed.includes('trusted'));
  assert.match(TRUSTED_INSTRUCTIONS, /write the checkout, use Git \(including commits\), use the network/);
});
