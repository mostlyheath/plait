import assert from 'node:assert/strict';
import { mkdir, rename, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { commandPolicy, parseCommandPolicy, setCommandPolicy } from '../src/command/policy.ts';
import { validate } from '../src/protocol.ts';
import { delay, exec, request, setup } from './support/host.ts';

// No command runs here: these tests cover only where the policy is stored and who it applies to.
type Fixture = Awaited<ReturnType<typeof setup>>;
type PolicyReply = { ok: boolean; result: { checkout: string; policy: string }; error?: { code: string; message: string } };
const send = (store: string, value: unknown) => request(store, value) as unknown as Promise<PolicyReply>;
const read = (f: Fixture, checkout = f.repo) => send(f.store, { op: 'command_policy', checkout });
const set = (f: Fixture, policy: string, checkout = f.repo) => send(f.store, { op: 'set_command_policy', checkout, policy });

test('a missing record is trusted; a sandboxed record holds for its directory; an unreadable one sandboxes', () => {
  const values = new Map<string, string>();
  const settings = {
    setting: (key: string) => values.get(key) ?? null,
    setSetting: (key: string, value: string | null) => { if (value === null) values.delete(key); else values.set(key, value); },
  };
  const root = { dev: '1', ino: '2' };
  assert.equal(commandPolicy(settings, '/repo', root), 'trusted');
  setCommandPolicy(settings, '/repo', root, 'sandboxed');
  assert.equal(commandPolicy(settings, '/repo', root), 'sandboxed');
  assert.equal(commandPolicy(settings, '/repo/nested', root), 'trusted');
  // A different directory now at that path did not ask for the sandbox.
  assert.equal(commandPolicy(settings, '/repo', { dev: '1', ino: '3' }), 'trusted');
  // Records written before trusted became the default stay trusted.
  values.set('command_policy:/repo', JSON.stringify({ policy: 'trusted', root }));
  assert.equal(commandPolicy(settings, '/repo', root), 'trusted');
  for (const corrupt of ['{', 'not json']) {
    values.set('command_policy:/repo', corrupt);
    assert.equal(commandPolicy(settings, '/repo', root), 'sandboxed', corrupt);
  }
  setCommandPolicy(settings, '/repo', root, 'sandboxed');
  setCommandPolicy(settings, '/repo', root, 'trusted');
  assert.equal(values.size, 0);
  assert.equal(parseCommandPolicy('trusted'), 'trusted');
  for (const value of ['', 'Trusted', 'auto', true, undefined]) assert.throws(() => parseCommandPolicy(value), { code: 'invalid_request' });
});

test('policy operations accept only their own fields', () => {
  validate({ op: 'command_policy', checkout: '/repo' });
  validate({ op: 'set_command_policy', checkout: '/repo', policy: 'trusted', protocol_version: 2 });
  for (const input of [
    { op: 'command_policy' },
    { op: 'command_policy', checkout: '/repo', policy: 'trusted' },
    { op: 'set_command_policy', checkout: '/repo' },
    { op: 'set_command_policy', checkout: '/repo', policy: true },
    { op: 'set_command_policy', checkout: '/repo', policy: 'trusted', thread_id: 't' },
    { op: 'submit', thread_id: 't', request_id: 'r', text: 'hi', policy: 'trusted' },
    { op: 'create_thread', checkout: '/repo', provider: 'p', model_id: 'm', request_id: 'r', policy: 'trusted' },
  ]) assert.throws(() => validate(input), { code: 'invalid_request' }, JSON.stringify(input));
});

test('the policy is per canonical checkout and per store, durable, and trusted by default', { timeout: 60000, skip: process.platform === 'linux' }, async t => {
  const f = await setup(t, 'answer', false, false);
  const elsewhere = await setup(t, 'answer', false, false);
  const other = join(f.base, 'other'); await exec('/usr/bin/git', ['init', '-q', other]);
  const nested = join(f.repo, 'nested'); await mkdir(nested);
  assert.deepEqual(await read(f), { ok: true, result: { checkout: f.repo, policy: 'trusted' } });

  // Any path inside the working tree names its canonical checkout.
  assert.deepEqual(await set(f, 'sandboxed', nested), { ok: true, result: { checkout: f.repo, policy: 'sandboxed' } });
  assert.equal((await read(f)).result.policy, 'sandboxed');
  assert.deepEqual((await read(f, other)).result, { checkout: other, policy: 'trusted' });
  assert.equal((await send(elsewhere.store, { op: 'command_policy', checkout: f.repo })).result.policy, 'trusted');
  assert.equal((await read(elsewhere)).result.policy, 'trusted');

  for (const invalid of [
    { op: 'set_command_policy', checkout: f.repo, policy: 'open' },
    { op: 'set_command_policy', checkout: 'repo', policy: 'trusted' },
  ]) assert.equal((await send(f.store, invalid)).error?.code, 'invalid_request', JSON.stringify(invalid));
  assert.equal((await send(f.store, { op: 'command_policy', checkout: join(f.base, 'missing') })).error?.code, 'checkout_missing');
  assert.equal((await read(f)).result.policy, 'sandboxed');
  // A folder outside any repository is a plain project with its own policy.
  assert.deepEqual((await read(f, f.base)).result, { checkout: f.base, policy: 'trusted' });

  // Existing replies keep their shape; the policy travels only in its own operations.
  const status = await request(f.store, { op: 'status' });
  assert.deepEqual(Object.keys(status.result).sort(), ['default_model', 'models', 'protocol_version']);
  const thread = await f.create();
  assert(!JSON.stringify(thread).includes('policy'));

  f.host.kill('SIGKILL'); await delay(500); await f.start();
  assert.equal((await read(f)).result.policy, 'sandboxed');

  // The sandbox belongs to that directory; a moved checkout's path no longer names it.
  await rename(f.repo, join(f.base, 'moved'));
  await exec('/usr/bin/git', ['init', '-q', f.repo]);
  assert.deepEqual((await read(f)).result, { checkout: f.repo, policy: 'trusted' });
  assert.equal((await read(f, join(f.base, 'moved'))).result.policy, 'trusted');

  assert.equal((await set(f, 'sandboxed')).result.policy, 'sandboxed');
  assert.deepEqual((await set(f, 'trusted')).result, { checkout: f.repo, policy: 'trusted' });
  assert.equal((await read(f)).result.policy, 'trusted');
});

test('agent_resources reports under the checkout\'s current policy, only for version 3', { timeout: 60000 }, async t => {
  const f = await setup(t);
  const resources = () => request(f.store, { op: 'agent_resources', checkout: join(f.repo, '.'), protocol_version: 3 }) as Promise<{ ok: boolean; result: Record<string, unknown> }>;
  // The test runtime offers none of the machine's own setup; the shape and trust are what matter here.
  assert.deepEqual(await resources(), { ok: true, result: { checkout: f.repo, trusted: false, instructions: null, skills: [], templates: [], mcp_servers: [], problems: [] } });
  await set(f, 'trusted');
  assert.equal((await resources()).result.trusted, true);
  for (const invalid of [
    { op: 'agent_resources', checkout: f.repo },
    { op: 'agent_resources', protocol_version: 3 },
    { op: 'agent_resources', checkout: f.repo, thread_id: 't', protocol_version: 3 },
  ]) assert.equal((await send(f.store, invalid)).error?.code, 'invalid_request', JSON.stringify(invalid));
  const missing = { op: 'agent_resources', checkout: join(f.base, 'missing'), protocol_version: 3 };
  assert.equal((await send(f.store, missing)).error?.code, 'checkout_missing');
});


test('MCP switch operation returns the refreshed safe report and survives host restart', { timeout: 60000 }, async t => {
  const f = await setup(t);
  const agent = join(f.store, 'test-pi'); await mkdir(agent);
  const config = JSON.stringify({ mcpServers: { local: { command: 'never-spawn-this', args: ['SECRET_ARG'] } } });
  await writeFile(join(agent, 'mcp.json'), config);
  const read = () => request(f.store, { op: 'agent_resources', checkout: f.repo, protocol_version: 3 }) as unknown as Promise<{ result: { mcp_servers: { enabled: boolean }[] } }>;
  const change = (enabled: boolean, name = 'local') => request(f.store, { op: 'set_mcp_server', checkout: join(f.repo, '.'), name, enabled, protocol_version: 3 });
  assert.equal((await read()).result.mcp_servers[0].enabled, false);
  const enabled = await change(true);
  assert.equal(enabled.ok, true);
  assert.ok(!JSON.stringify(enabled).includes('SECRET_ARG'));
  assert.equal((await read()).result.mcp_servers[0].enabled, true);
  f.host.kill('SIGKILL'); await delay(500); await f.start();
  assert.equal((await read()).result.mcp_servers[0].enabled, true);
  assert.equal((await change(false)).ok, true);
  await set(f, 'trusted');
  assert.equal((await read()).result.mcp_servers[0].enabled, false);
  assert.equal((await change(true, 'missing')).error?.code, 'not_found');
  assert.equal(await readFile(join(agent, 'mcp.json'), 'utf8'), config, 'Pi configuration is never edited');
});

 test('Linux refuses a sandboxed policy', { skip: process.platform !== 'linux' }, async t => {
  const f = await setup(t, 'answer', false, false);
  const result = await set(f, 'sandboxed');
  assert.equal(result.error?.code, 'invalid_request');
  assert.match(result.error!.message, /command sandbox is not available on Linux yet/);
  assert.equal((await read(f)).result.policy, 'trusted');
});
