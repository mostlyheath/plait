import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { TestContext } from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { createServer } from 'node:http';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { createRegistry } from '@earendil-works/pi-durable';
import type { ToolExecutionApi } from '@earendil-works/pi-durable';
import { loadMcp } from '../src/mcp/config.ts';
import { McpRun, eligible } from '../src/mcp/tools.ts';
import { MCP_START_TIMEOUT_MS, MCP_CALL_TIMEOUT_MS } from '../src/mcp/transport.ts';
import { RunAuthorization } from '../src/authorization.ts';
import { automaticApproval, InteractiveApproval } from '../src/approval.ts';
import { CommandOutputs } from '../src/tool-output.ts';
import { rootIdentity } from '../src/safety.ts';
import type { ToolState } from '../src/tools.ts';
import type { Snapshot } from '../src/protocol.ts';
import { setup, exec } from './support/safety.ts';
import { mcpPolicy, setMcpEnabled } from '../src/mcp/policy.ts';

const secret = 'SEEDED_MCP_SECRET_72919';
async function rig(t: TestContext, policy: 'trusted' | 'sandboxed' = 'trusted', mode: 'ask' | 'auto' = 'auto', behavior = 'normal', extraEnv: Record<string, string> = {}) {
  const { root, base } = await setup(t); const agent = join(base, 'agent'); await mkdir(agent);
  const store = join(base, 'store'); await mkdir(store, { mode: 0o700 });
  const marker = join(root, 'events'); await writeFile(join(root, 'notes.txt'), 'notes');
  const outside = join(base, 'outside'); await writeFile(outside, 'private');
  const script = join(root, 'server.mjs'); await writeFile(script, await readFile(resolve('test/support/mcp-fixture.mjs')));
  const config = { command: process.execPath, args: [script, behavior, ...(policy === 'trusted' ? [marker] : [])],
    env: { FIXTURE_SECRET: secret, FIXTURE_EXPANDED: policy === 'trusted' ? '${MCP_TEST_TOKEN}' : '${HOME}/fixture-variable-secret', FIXTURE_COMMAND: '!printf fixture-command-secret', FIXTURE_OUTSIDE: outside, ...extraEnv }, timeout: behavior === 'hang-call' ? 0.2 : 2 };
  const previous = process.env.MCP_TEST_TOKEN; process.env.MCP_TEST_TOKEN = 'fixture-variable-secret';
  t.after(() => { if (previous === undefined) delete process.env.MCP_TEST_TOKEN; else process.env.MCP_TEST_TOKEN = previous; });
  await writeFile(join(agent, 'mcp.json'), JSON.stringify({ mcpServers: { docs: config } }));
  const found = await loadMcp(root, policy === 'trusted', agent, true);
  const snapshot = { id: 'fixture-thread', checkout: root, status: 'running', pending_approval: null } as Snapshot;
  const approvals: unknown[] = []; const interactive = new InteractiveApproval(snapshot, () => { if (snapshot.pending_approval) approvals.push(structuredClone(snapshot.pending_approval)); });
  const authorization = new RunAuthorization(mode === 'auto' ? automaticApproval : interactive); authorization.activate();
  const db = new DatabaseSync(':memory:'); const outputs = new CommandOutputs(db);
  const state: ToolState = { snapshot, authorization, mcpApproval: interactive, commandPolicy: policy, baseline: [], protected: [], owned: new Map(),
    rootIdentity: await rootIdentity(root), buildCache: { store, thread: '12345678-1234-1234-1234-123456789abc' }, outputs, record() {},
    loginEnvironment: Promise.resolve({ env: { PATH: '/usr/bin:/bin', HOME: base, MCP_TEST_TOKEN: 'fixture-variable-secret' }, source: 'login-shell' }),
  };
  const settings = new Map<string, string>(); const prefs = { setting: (key: string) => settings.get(key) ?? null, setSetting: (key: string, value: string | null) => { if (value) settings.set(key, value); } };
  setMcpEnabled(prefs, root, state.rootIdentity, found.entries.get('docs')!, true);
  const entries = mcpPolicy(prefs, root, state.rootIdentity, found.servers, found.entries).flatMap(row => eligible(found.entries.get(row.name)!, row) ? [found.entries.get(row.name)!] : []);
  const observations: unknown[] = []; const names = new Map<string, string>(); let remembered = 0;
  const run = new McpRun(state, entries, (entry, observed) => observations.push({ name: entry.name, ...observed }), (name, label) => { remembered++; names.set(name, label); });
  const registry = createRegistry(); run.install(registry);
  let serial = 0;
  const api = () => ({ callId: `call-${serial++}`, agent: async () => ({ deferred: registry.snapshot().tools().map(item => item.tool).filter(tool => tool.deferred) }) }) as unknown as ToolExecutionApi;
  const search = () => registry.snapshot().tools()[0].tool.execute({ query: 'fixture', limit: 20 }, api(), context);
  const call = (tool = 'echo', args = { text: 'hello' }) => {
    const offered = registry.snapshot().tools().find(item => item.tool.description.startsWith(`mcp:docs/${tool}:`));
    assert.ok(offered, `registered ${tool}: ${JSON.stringify(observations)}`);
    return offered.tool.execute(args, api(), context);
  };
  t.after(async () => { authorization.revoke(); await run.close(); db.close(); });
  return { root, base, marker, snapshot, interactive, approvals, observations, names, registry, run, state, found, search, call, db, remembers: () => remembered };
}
async function waitFor(predicate: () => boolean) { for (let i = 0; i < 500; i++) { if (predicate()) return; await new Promise(resolve => setTimeout(resolve, 10)); } throw new Error('fixture wait timed out'); }

for (const policy of ['trusted', 'sandboxed'] as const) for (const mode of ['ask', 'auto'] as const) for (const allow of [true, false]) {
  test(`MCP ${policy} ${mode} ${allow ? 'allow' : 'deny'} preserves exact approval policy`, async t => {
    const f = await rig(t, policy, mode); await f.search(); const pending = f.call();
    if (policy === 'sandboxed' || mode === 'ask') {
      const settled = pending.then(value => ({ value }), error => ({ error }));
      await waitFor(() => !!f.snapshot.pending_approval);
      assert.equal(f.snapshot.pending_approval!.kind, 'command'); assert.equal(f.snapshot.pending_approval!.summary, 'Use echo from docs');
      assert.doesNotMatch(JSON.stringify(f.approvals), /SEEDED_MCP_SECRET|fixture-variable-secret|fixture-command-secret/);
      f.interactive.answer(f.snapshot.pending_approval!.id, allow);
      const result = await settled; assert.ok(allow ? 'value' in result : 'error' in result);
    } else { await pending; assert.equal(f.approvals.length, 0); }
  });
}

test('deferred search, collisions, unsafe replay, secret-free output and overflow storage', async t => {
  const f = await rig(t); await assert.rejects(readFile(f.marker), { code: 'ENOENT' }); await f.search();
  assert.equal(f.names.size, 0, 'Search writes no stored labels');
  const all = f.registry.snapshot().tools().map(item => item.tool);
  assert.ok(all.slice(1).every(tool => tool.deferred && tool.replay === 'unsafe'));
  assert.equal(new Set(all.map(tool => tool.name)).size, all.length);
  assert.ok(all.slice(1).every(tool => tool.name.startsWith('mcp__') && tool.name !== 'command'));
  const echo = await f.call(); const large = await f.call('large'); assert.ok(JSON.stringify(large).length < 24576);
  assert.equal(f.names.size, 2); await f.call(); assert.equal(f.remembers(), 2, 'A repeated call does not write another label');
  const rows = f.db.prepare('SELECT data FROM command_output_chunks').all();
  const serialized = JSON.stringify([all, echo, large, f.observations, f.approvals, rows.map(row => Buffer.from(row.data as Uint8Array).toString())]);
  for (const value of [secret, 'fixture-variable-secret', 'fixture-command-secret', 'SERVER_INSTRUCTIONS_MUST_NOT_REACH_MODEL']) assert.ok(!serialized.includes(value), value);
  assert.ok(rows.some(row => (row.data as Uint8Array).length > 12000));
  assert.match(JSON.stringify(await f.call('sample')), /-32601/);
  assert.equal((await readFile(f.marker, 'utf8')).match(/start /g)?.length, 1);
});

test('startup crash, ten-second startup timeout, and stalled call are bounded and never retried', { timeout: 30000 }, async t => {
  assert.equal(MCP_START_TIMEOUT_MS, 10000); assert.equal(MCP_CALL_TIMEOUT_MS, 60000);
  const crash = await rig(t, 'trusted', 'auto', 'crash-start'); await crash.search();
  assert.match(JSON.stringify(crash.observations), /fixture startup crash/); assert.doesNotMatch(JSON.stringify(crash.observations), new RegExp(secret));
  const malformed = await rig(t, 'trusted', 'auto', 'bad-schema'); await malformed.search();
  assert.equal(malformed.registry.snapshot().tools().length, 1); assert.match(JSON.stringify(malformed.observations), /Tool 1 was refused/);
  const silent = await rig(t, 'trusted', 'auto', 'silent'); const start = Date.now(); await silent.search();
  assert.ok(Date.now() - start < 13000); assert.match(JSON.stringify(silent.observations), /timed out/);
  const hang = await rig(t, 'trusted', 'auto', 'hang-call'); await hang.search(); assert.equal((await hang.call()).isError, true);
  assert.equal((await readFile(hang.marker, 'utf8')).match(/call echo/g)?.length, 1);
});

test('two servers with identical remote tool names remain separate from each other and builtins', async t => {
  const f = await rig(t); const entry = f.found.entries.get('docs')!; const labels = new Map<string, string>();
  const run = new McpRun(f.state, [entry, { ...entry, name: 'other' }], () => {}, (name, label) => labels.set(name, label));
  t.after(() => run.close()); const registry = createRegistry(); run.install(registry);
  const api = { callId: 'two-servers', agent: async () => ({ deferred: [] }) } as unknown as ToolExecutionApi;
  await registry.snapshot().tools()[0].tool.execute({ query: 'fixture' }, api, context);
  const offered = registry.snapshot().tools().slice(1).map(item => item.tool);
  assert.equal(labels.size, 0); assert.equal(offered.length, 16);
  assert.ok(offered.every(tool => tool.name.startsWith('mcp__') && tool.name.length <= 64));
  for (const server of ['docs', 'other']) await offered.find(tool => tool.description.startsWith(`mcp:${server}/echo:`))!.execute({}, api, context);
  assert.deepEqual([...labels.values()].sort(), ['mcp:docs/echo', 'mcp:other/echo']);
});

test('ordinary login environment, arguments and short configured values are not scrubbed', async t => {
  const f = await rig(t, 'trusted', 'auto', 'environment', { SHORT: '1' });
  const env = { PATH: '/usr/bin:/bin', SHLVL: '1', USER: 'fixture-user', HOME: '/Users/fixture-person', LANG: 'en_US.UTF-8', TERM: 'xterm-256color' };
  f.state.loginEnvironment = Promise.resolve({ env: { ...env, MCP_TEST_TOKEN: 'fixture-variable-secret' }, source: 'login-shell' });
  const config = f.found.entries.get('docs')!.config;
  assert.ok('command' in config); config.args!.push('-y', '.', 'run', 'ordinary-long-argument');
  await f.search(); const result = await f.call();
  const text = JSON.parse(result.output![0].type === 'text' ? result.output![0].text.split('\n[Command output:')[0] : '').content[0].text;
  assert.equal(text, `Found 12 results; ${JSON.stringify(env)}; -y . run ordinary-long-argument; 1`);
  assert.doesNotMatch(text, /\[redacted\]/);
});

test('unsafe names and credentials in tool names, titles or schema keys refuse only those tools', async t => {
  const f = await rig(t, 'trusted', 'auto', 'bad-metadata'); const search = await f.search();
  const tools = f.registry.snapshot().tools().slice(1).map(item => item.tool);
  assert.equal(tools.length, 9);
  assert.ok(tools.some(tool => tool.description.startsWith('mcp:docs/namespace:tool.v1/search:')));
  const serialized = JSON.stringify([tools, search, f.observations]);
  for (const unsafe of [secret, '\u001b', '\u202e', '\u200b', '\\u001b']) assert.ok(!serialized.includes(unsafe));
  assert.match(serialized, /was refused/);
  await f.call(); assert.deepEqual([...f.names.values()], ['mcp:docs/echo']);
  await f.run.close(); assert.match(JSON.stringify(f.observations.at(-1)), /"state":"stopped".*was refused/);
});

test('Stop kills the MCP server and its child process group', async t => {
  const f = await rig(t, 'trusted', 'auto', 'hang-call'); await f.search();
  const pids = [...(await readFile(f.marker, 'utf8')).matchAll(/(?:start|child) (\d+)/g)].map(match => Number(match[1]));
  assert.equal(pids.length, 2); f.state.authorization.revoke(); await f.run.close();
  await waitFor(() => pids.every(pid => { try { process.kill(pid, 0); return false; } catch { return true; } }));
});

test('sandboxed server reads checkout, denies checkout/Git/outside writes, and writes scratch/cache', async t => {
  const server = createServer((_request, response) => response.end('loopback-ok'));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve)); t.after(() => { server.closeAllConnections(); server.close(); });
  const f = await rig(t, 'sandboxed', 'auto', 'normal', { FIXTURE_URL: `http://127.0.0.1:${(server.address() as { port: number }).port}` });
  await f.search(); const pending = f.call('inspect');
  await waitFor(() => !!f.snapshot.pending_approval); f.interactive.answer(f.snapshot.pending_approval!.id, true);
  const text = JSON.stringify(await pending);
  for (const field of ['checkoutWrite', 'gitWrite', 'outsideRead', 'outsideWrite']) assert.match(text, new RegExp(field + '.*?denied'));
  for (const field of ['scratchWrite', 'cacheWrite']) assert.match(text, new RegExp(field + '.*?allowed'));
  assert.match(text, /notes/); assert.match(text, /loopback-ok/);
});

test('loopback HTTP resolves header values only at search and never invokes OAuth', async t => {
  const f = await rig(t); let requests = 0; const headers: string[] = [];
  const server = createServer(async (request, response) => {
    requests++; headers.push(request.headers.authorization ?? '');
    let body = ''; for await (const chunk of request) body += chunk;
    if (!body) { response.writeHead(204).end(); return; }
    const message = JSON.parse(body);
    if (!message.id) { response.writeHead(202).end(); return; }
    const result = message.method === 'initialize' ? { protocolVersion: message.params.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: 'http fixture', version: '1' } }
      : message.method === 'tools/list' ? { tools: [{ name: 'echo', description: 'fixture HTTP echo', inputSchema: { type: 'object' } }] }
      : { content: [{ type: 'text', text: `documentation search all-documents ${request.headers.authorization}` }] };
    response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }));
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve)); t.after(() => { server.closeAllConnections(); server.close(); });
  const entry = { ...f.found.entries.get('docs')!, config: { url: `http://127.0.0.1:${(server.address() as { port: number }).port}/documentation/search?view=all-documents`, headers: { Authorization: 'Bearer ${MCP_TEST_TOKEN}' } } };
  const run = new McpRun(f.state, [entry], () => {}, () => {}); t.after(() => run.close());
  const registry = createRegistry(); run.install(registry); assert.equal(requests, 0);
  const api = { callId: 'http', agent: async () => ({ deferred: registry.snapshot().tools().map(item => item.tool).filter(tool => tool.deferred) }) } as unknown as ToolExecutionApi;
  await registry.snapshot().tools()[0].tool.execute({ query: 'fixture' }, api, context);
  const result = await registry.snapshot().tools()[1].tool.execute({}, api, context);
  assert.ok(headers.every(value => value === 'Bearer fixture-variable-secret')); assert.doesNotMatch(JSON.stringify(result), /fixture-variable-secret/);
  assert.match(JSON.stringify(result), /documentation search all-documents/);
});

test('HTTP authentication challenge stays listed for Pi sign-in with no retry or leaked body', async t => {
  const f = await rig(t); let requests = 0;
  const server = createServer((_request, response) => { requests++; response.writeHead(401, { 'WWW-Authenticate': 'Bearer' }).end(secret); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve)); t.after(() => { server.closeAllConnections(); server.close(); });
  const entry = { ...f.found.entries.get('docs')!, config: { url: `http://127.0.0.1:${(server.address() as { port: number }).port}` } };
  const seen: unknown[] = []; const run = new McpRun(f.state, [entry], (_entry, value) => seen.push(value), () => {}); t.after(() => run.close());
  const registry = createRegistry(); run.install(registry);
  const api = { callId: 'auth', agent: async () => ({ deferred: [] }) } as unknown as ToolExecutionApi;
  await registry.snapshot().tools()[0].tool.execute({ query: 'fixture' }, api, context);
  assert.equal(requests, 1); assert.match(JSON.stringify(seen), /"needs_sign_in":true/); assert.doesNotMatch(JSON.stringify(seen), new RegExp(secret));
  await registry.snapshot().tools()[0].tool.execute({ query: 'again' }, api, context); assert.equal(requests, 1);
  const explicit = { ...entry, config: { ...entry.config, oauth: {} } };
  assert.equal(eligible(explicit, { ...f.found.servers[0], needs_sign_in: true }), false);
});

test('Stop during a pending approval prevents a later answer from reaching the server', async t => {
  const f = await rig(t, 'trusted', 'ask'); await f.search();
  const pending = f.call().then(() => false, () => true); await waitFor(() => !!f.snapshot.pending_approval);
  const id = f.snapshot.pending_approval!.id; f.state.authorization.revoke();
  assert.equal(await pending, true); assert.throws(() => f.interactive.answer(id, true));
  await f.run.close(); assert.doesNotMatch(await readFile(f.marker, 'utf8'), /call echo/);
});

test('a crashed connected server reports the stderr tail without retrying its call', async t => {
  const f = await rig(t); await f.search(); assert.equal((await f.call('crash')).isError, true);
  assert.match(JSON.stringify(f.observations), /fixture crashed/); assert.doesNotMatch(JSON.stringify(f.observations), new RegExp(secret));
  await f.search(); assert.equal((await readFile(f.marker, 'utf8')).match(/start /g)?.length, 1);
});

test('approval binds MCP arguments and rejects a replaced checkout before dispatch', async t => {
  const f = await rig(t, 'trusted', 'ask'); await f.search();
  const args = { text: 'approved-text' }; const first = f.call('echo', args);
  await waitFor(() => !!f.snapshot.pending_approval); args.text = 'changed-after-approval';
  assert.equal(JSON.parse(f.snapshot.pending_approval!.details).text, 'approved-text');
  f.interactive.answer(f.snapshot.pending_approval!.id, true);
  const result = JSON.stringify(await first); assert.match(result, /approved-text/); assert.doesNotMatch(result, /changed-after-approval/);
  const second = f.call().then(() => false, () => true); await waitFor(() => !!f.snapshot.pending_approval);
  const moved = join(f.base, 'original-repo'); await rename(f.root, moved); await mkdir(f.root);
  f.interactive.answer(f.snapshot.pending_approval!.id, true); assert.equal(await second, true);
  assert.equal((await readFile(join(moved, 'events'), 'utf8')).match(/call echo/g)?.length, 1);
  f.state.authorization.revoke(); await f.run.close();
});

for (const count of [2, 5]) test(`all ${count} servers resolve credentials before the first server lists tools`, async t => {
  const f = await rig(t); const base = f.found.entries.get('docs')!;
  const entries = Array.from({ length: count }, (_, index) => ({ ...base, name: `srv${index}`, config: {
    command: process.execPath, args: [join(f.root, 'server.mjs'), 'cross-secret'],
    env: index === count - 1 ? { KEY: '!sleep 0.1; printf LATER_SERVER_CREDENTIAL_72919' } : {} as Record<string, string>,
  } }));
  const seen: unknown[] = []; const run = new McpRun(f.state, entries, (_entry, observation) => seen.push(observation), () => {});
  t.after(() => run.close()); const registry = createRegistry(); run.install(registry);
  const api = { callId: 'barrier', agent: async () => ({ deferred: [] }) } as unknown as ToolExecutionApi;
  const result = await registry.snapshot().tools()[0].tool.execute({ query: 'Fixture' }, api, context);
  const offered = registry.snapshot().tools().slice(1).map(item => item.tool);
  assert.equal(offered.length, 8 * count); assert.match(JSON.stringify(seen), /was refused/);
  assert.doesNotMatch(JSON.stringify([offered, result, seen]), /LATER_SERVER_CREDENTIAL_72919/);
});

for (const policy of ['trusted', 'sandboxed'] as const) for (const key of ['PLAIT_HOST_EXE', 'PLAIT_LOCK_FD', 'PLAIT_NOTIFY_FD', 'SIDECAR_PRIVATE_KEY']) {
  test(`${policy} expansion cannot read the sidecar-only ${key}`, async t => {
    const f = await rig(t, policy); const prior = process.env[key];
    if (key !== 'PLAIT_HOST_EXE') process.env[key] = 'SIDECAR_CREDENTIAL_827291';
    t.after(() => { if (prior === undefined) delete process.env[key]; else process.env[key] = prior; });
    f.found.entries.get('docs')!.config = { command: process.execPath, args: [join(f.root, 'server.mjs')], env: { LEAK: '${' + key + '}' } };
    await f.search(); assert.equal(f.registry.snapshot().tools().length, 1);
    assert.match(JSON.stringify(f.observations), /could not be prepared/);
    await assert.rejects(readFile(f.marker), { code: 'ENOENT' });
  });
}

for (const mode of ['ask', 'auto'] as const) test(`tracked credentials in ${mode} call arguments are refused before approval or dispatch`, async t => {
  const f = await rig(t, 'trusted', mode); await f.search();
  for (const args of [{ text: secret }, { nested: { text: 'Bearer ' + secret } }, { [secret]: 'key' }]) {
    await assert.rejects(f.call('echo', args as { text: string }), /arguments contain a configured credential; the call was refused/);
  }
  assert.equal(f.approvals.length, 0); assert.equal(f.snapshot.pending_approval, null);
  assert.doesNotMatch(await readFile(f.marker, 'utf8'), /call echo/);
});

test('deep arguments, deep schemas, oversized listing and endless pages fail within bounds', async t => {
  const f = await rig(t); await f.search();
  const input = JSON.parse('{"text":' + '['.repeat(4000) + '0' + ']'.repeat(4000) + '}');
  assert.ok(JSON.stringify(input).length < 16384);
  await assert.rejects(f.call('echo', input), /64-level depth/);
  assert.equal(f.approvals.length, 0); assert.doesNotMatch(await readFile(f.marker, 'utf8'), /call echo/);
  for (const behavior of ['deep-schema', 'oversized-list', 'pages']) {
    const bad = await rig(t, 'trusted', 'auto', behavior); await bad.search();
    assert.equal(bad.registry.snapshot().tools().length, 1);
    assert.match(JSON.stringify(bad.observations), /could not start or list tools/);
    if (behavior === 'pages') assert.equal((await readFile(bad.marker, 'utf8')).match(/page /g)?.length, 32);
  }
});

test('a normal call succeeds through a burst of 1,000 progress notifications', async t => {
  const f = await rig(t, 'trusted', 'auto', 'notification-burst'); await f.search();
  const result = await f.call('echo', { text: 'indexed files' });
  assert.equal(result.isError, false); assert.match(JSON.stringify(result), /indexed files/);
  assert.match(await readFile(f.marker, 'utf8'), /call echo\nnotifications 1000\n/);
  assert.doesNotMatch(JSON.stringify(f.observations), /failed|exceeded its output budget/);
});

for (const behavior of ['flood', 'flood-bytes', 'flood-sustained']) test(`${behavior} is disconnected while another thread searches and calls a healthy server`, async t => {
  const abusive = await rig(t, 'trusted', 'auto', behavior); const healthy = await rig(t);
  // The output budget, not the clock, is under test: the observation below must name the budget.
  // This bound only catches a hang; a runner that just built a release took 10 s here.
  const start = Date.now(); await Promise.all([abusive.search(), healthy.search()]);
  assert.ok(Date.now() - start < 30_000);
  // On a slow machine a flood of large lines can fill the pipe and end the fixture before the
  // budget is spent. Either way the server has failed and the other thread is untouched.
  assert.match(JSON.stringify(abusive.observations),
    behavior === 'flood-bytes' ? /exceeded its output budget|could not start or list tools/ : /exceeded its output budget/);
  assert.equal(abusive.registry.snapshot().tools().length, 1);
  assert.equal((await healthy.call()).isError, false);
});

test('prototype tool names cannot bypass user-hidden exposure or unreviewed project limits', async t => {
  const f = await rig(t, 'trusted', 'auto', 'prototype-names');
  const entry = f.found.entries.get('docs')!;
  entry.config.exposure = 'hidden'; entry.config.toolExposure = { echo: 'direct' };
  await f.search(); assert.equal(f.registry.snapshot().tools().length, 2);
  const registry = createRegistry(); const run = new McpRun(f.state, [{ ...entry, limit: entry.config,
    config: { ...entry.config, exposure: 'direct', toolExposure: {} } }], () => {}, () => {});
  t.after(() => run.close()); run.install(registry);
  await registry.snapshot().tools()[0].tool.execute({ query: 'fixture' }, { callId: 'override', agent: async () => ({ deferred: [] }) } as unknown as ToolExecutionApi, context);
  assert.equal(registry.snapshot().tools().length, 2);
});

test('the MCP guardian starts clean and loader injection reaches only an explicitly trusted server', async t => {
  const f = await rig(t); const library = join(f.root, 'inject.dylib'); const source = join(f.root, 'inject.c'); const injected = join(f.base, 'injected');
  await writeFile(source, '#include <stdio.h>\n#include <unistd.h>\n__attribute__((constructor)) static void probe(void) { FILE *f = fopen(' + JSON.stringify(injected) + ', "a"); if(f) { fprintf(f, "%d\\n", getpid()); fclose(f); } }\n');
  await exec('/usr/bin/clang', ['-dynamiclib', source, '-o', library]);
  const entry = f.found.entries.get('docs')!; assert.ok('command' in entry.config);
  entry.config.env!.DYLD_INSERT_LIBRARIES = library;
  await f.search(); await f.call();
  const serverPid = (await readFile(f.marker, 'utf8')).match(/start (\d+)/)![1];
  assert.deepEqual((await readFile(injected, 'utf8')).trim().split('\n'), [serverPid], 'The unsandboxed guardian must never load the library');
  const sandboxed = await rig(t, 'sandboxed', 'auto', 'normal', { DYLD_INSERT_LIBRARIES: library });
  assert.equal(sandboxed.registry.snapshot().tools().length, 0);
  assert.match(sandboxed.found.servers[0].problem!, /loader environment/);
  assert.deepEqual((await readFile(injected, 'utf8')).trim().split('\n'), [serverPid]);
});

test('localhost lookup needs only the exact mDNSResponder socket exception', async t => {
  const { prepareCommand } = await import('../src/command/macos.ts');
  const { runGuardian } = await import('../src/command/supervisor.ts');
  const f = await rig(t, 'sandboxed'); const scratch = join(f.base, 'dns-scratch'); await mkdir(scratch);
  const prepared = await prepareCommand(f.root, scratch, f.state.authorization.signal);
  const script = join(f.root, 'dns.mjs'); await writeFile(script, "import { lookup } from 'node:dns/promises'; console.log(await lookup('localhost').then(() => 'resolved').catch(e => e.code));");
  const base = { root: f.root, text: `'${process.execPath}' '${script}'`, env: prepared.env, isolateEnvironment: true };
  const ip = '\n(allow network-outbound (remote ip))\n(allow network-inbound network-bind (local ip))';
  const without = await runGuardian({ ...base, policy: prepared.policy + ip }, f.state.authorization.signal, () => {}, 2000);
  const withDns = await runGuardian({ ...base, policy: prepared.policy + ip + '\n(allow network-outbound (literal "/private/var/run/mDNSResponder"))' }, f.state.authorization.signal, () => {}, 2000);
  assert.match(without, /ENOTFOUND|EAI_AGAIN/); assert.equal(withDns.trim(), 'resolved');
});

for (const policy of ['trusted', 'sandboxed'] as const) test(`${policy} HTTP headers cannot expand sidecar-private keys or make a request after failed expansion`, async t => {
  const f = await rig(t, policy); const prior = process.env.SIDECAR_PRIVATE_KEY; process.env.SIDECAR_PRIVATE_KEY = 'SIDECAR_CREDENTIAL_827291';
  t.after(() => { if (prior === undefined) delete process.env.SIDECAR_PRIVATE_KEY; else process.env.SIDECAR_PRIVATE_KEY = prior; });
  let requests = 0; const server = createServer((_request, response) => { requests++; response.end(); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve)); t.after(() => { server.closeAllConnections(); server.close(); });
  const entry = { ...f.found.entries.get('docs')!, transport: 'http' as const, config: {
    url: `http://127.0.0.1:${(server.address() as { port: number }).port}`, headers: { Authorization: 'Bearer ${SIDECAR_PRIVATE_KEY}' },
  } };
  const seen: unknown[] = []; const run = new McpRun(f.state, [entry], (_entry, value) => seen.push(value), () => {}); t.after(() => run.close());
  const registry = createRegistry(); run.install(registry);
  await registry.snapshot().tools()[0].tool.execute({ query: 'fixture' }, { callId: 'private-header', agent: async () => ({ deferred: [] }) } as unknown as ToolExecutionApi, context);
  assert.equal(requests, 0); assert.equal(registry.snapshot().tools().length, 1);
  assert.match(JSON.stringify(seen), /could not be prepared/); assert.doesNotMatch(JSON.stringify(seen), /SIDECAR_CREDENTIAL/);
});

test('the contained exec stage applies even an injected test environment only after Seatbelt and descriptor closure', async t => {
  const { prepareCommand } = await import('../src/command/macos.ts'); const { guardianPipe } = await import('../src/mcp/pipe.ts');
  const f = await rig(t, 'sandboxed'); const scratch = join(f.base, 'exec-scratch'); await mkdir(scratch);
  const outside = join(f.base, 'escaped-loader'); const inside = join(scratch, 'loader-ran');
  const library = join(f.root, 'probe.dylib'); const source = join(f.root, 'probe.c'); const program = join(f.root, 'probe');
  await writeFile(source, '#include <stdio.h>\n__attribute__((constructor)) static void probe(void) { FILE *f = fopen(' + JSON.stringify(outside) + ', "w"); if(f) { fputs("escaped", f); fclose(f); } f = fopen(' + JSON.stringify(inside) + ', "w"); if(f) { fputs("contained", f); fclose(f); } }\n');
  await exec('/usr/bin/clang', ['-dynamiclib', source, '-o', library]);
  await writeFile(source, '#include <stdio.h>\n#include <fcntl.h>\nint main(void) { int mask = 0; for(int fd=3;fd<16;fd++) if(fcntl(fd,F_GETFD)>=0) mask |= 1<<fd; printf("private descriptors: %d\\n",mask); }\n');
  await exec('/usr/bin/clang', [source, '-o', program]);
  const prepared = await prepareCommand(f.root, scratch, f.state.authorization.signal); let stdout = '';
  // Deliberately bypass the entry validator to exercise the native boundary itself.
  const pipe = guardianPipe(f.root, prepared.policy, program, [], { ...prepared.env, DYLD_INSERT_LIBRARIES: library }, f.state,
    { onStdout: bytes => { stdout += Buffer.from(bytes).toString(); }, onStderr: () => {} }, f.state.authorization.signal);
  await pipe.exited; assert.equal(stdout.trim(), 'private descriptors: 0');
  assert.equal(await readFile(inside, 'utf8'), 'contained'); await assert.rejects(readFile(outside), { code: 'ENOENT' });
});
