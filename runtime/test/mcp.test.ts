import { fixtureDirectory } from './support/temp.ts';
import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { once } from 'node:events';
import net from 'node:net';
import tls from 'node:tls';
import http from 'node:http';
import https from 'node:https';
import { syncBuiltinESMExports } from 'node:module';
import { mkdir, open, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { test } from 'node:test';
import { DefaultPackageManager, SettingsManager } from '@earendil-works/pi-coding-agent';
import { definitionDigest, listMcp, loadMcp } from '../src/mcp/config.ts';
import { executionEntry, mcpPolicy, setMcpEnabled } from '../src/mcp/policy.ts';
import { eligible, toolExposure } from '../src/mcp/tools.ts';
import { commandPolicy, setCommandPolicy } from '../src/command/policy.ts';
import { report } from '../src/skills.ts';
import { validate } from '../src/protocol.ts';
import { Store } from '../src/store.ts';

async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const base = await realpath(await fixtureDirectory(join(tmpdir(), 'plait-mcp-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  const agent = join(base, 'agent'); const repo = join(base, 'repo');
  await mkdir(agent); await mkdir(join(repo, '.pi'), { recursive: true });
  const write = async (path: string, value: unknown) => writeFile(path, JSON.stringify(value));
  const user = (servers: unknown) => write(join(agent, 'mcp.json'), { mcpServers: servers });
  const project = (servers: unknown) => write(join(repo, '.pi', 'mcp.json'), { mcpServers: servers });
  return { base, agent, repo, user, project, write, list: (trusted = true, enabled = true) => listMcp(repo, trusted, agent, enabled) };
}

async function piConfig() {
  const entry = fileURLToPath(import.meta.resolve('@earendil-works/pi-coding-agent'));
  return import(pathToFileURL(join(dirname(entry), 'extensions/mcp/config.js')).href) as Promise<{
    loadMcpConfig(options: { agentDir: string; cwd: string; projectTrusted: boolean }): {
      servers: { name: string; source: string; override?: string; config: { enabled?: boolean } }[]; errors: string[];
    };
  }>;
}

test('MCP merge matches pinned Pi: replacement, restricted overrides, validation and namespaces', async t => {
  const f = await fixture(t); const pi = await piConfig();
  await f.user({
    base: { command: '/bin/node', args: ['user-argument'], env: { KEY: 'secret' } },
    replaced: { url: 'https://old.example/mcp', headers: { Authorization: 'secret' } },
    inherited: { url: 'https://provider.example/mcp', auth: { provider: 'test' } },
    'one-two': { command: 'npx' }, stays: { command: 'uvx' },
    invalid: { type: 'sse', url: 'https://sse.example' },
  });
  await f.project({
    base: { enabled: false, exposure: 'hidden', toolExposure: { find: 'codemode-deferred' } },
    replaced: { command: 'replacement', enabled: true }, inherited: { enabled: false },
    stays: { env: { KEY: 'project-secret' } }, missing: { enabled: true }, one_two: { command: 'collision' },
    projectAuth: { url: 'https://provider.example', auth: { provider: 'test' } },
    good: { url: 'https://docs.example/mcp', oauth: { clientRegistration: 'cimd' } },
    badExposure: { command: 'x', exposure: 'invalid' },
    badToolExposure: { command: 'x', toolExposure: { secret: 'invalid' } },
    badCallback: { url: 'https://example.com', oauth: { clientRegistration: 'cimd', callbackUrl: 'http://[::1]/callback' } },
    badAuth: { url: 'http://remote.example', auth: { provider: 'test' } },
    invalid: { type: 'sse', url: 'https://still-invalid.example' },
  });
  for (const trusted of [false, true]) {
    const ours = await f.list(trusted);
    const theirs = pi.loadMcpConfig({ agentDir: f.agent, cwd: f.repo, projectTrusted: trusted });
    assert.deepEqual(ours.servers.filter(server => !server.problem).map(server => ({
      name: server.name, source: server.source, override: server.override_source ?? undefined,
    })), theirs.servers.map(server => ({
      name: server.name, source: server.source, override: server.override,
    })));
    assert.equal(ours.problems.length, theirs.errors.length);
  }
  const { servers } = await f.list();
  const trusted = await loadMcp(f.repo, true, f.agent, true);
  const sandboxed = await loadMcp(f.repo, false, f.agent, true);
  assert.deepEqual(trusted.entries.get('replaced')?.config, { command: 'replacement', enabled: true });
  assert.deepEqual(sandboxed.entries.get('replaced')?.config, { url: 'https://old.example/mcp', headers: { Authorization: 'secret' } });
  assert.deepEqual(trusted.entries.get('base')?.config.toolExposure, { find: 'codemode-deferred' });
  assert.equal(sandboxed.entries.get('base')?.config.toolExposure, undefined);
  assert.equal(servers.find(server => server.name === 'replaced')?.transport, 'stdio');
  assert.equal(servers.find(server => server.name === 'base')?.program, '/bin/node');
  assert.equal(servers.find(server => server.name === 'stays')?.program, 'uvx');
  assert.ok(servers.find(server => server.name === 'invalid')?.problem);
  assert.equal(servers.filter(server => server.name === 'invalid').length, 1);
});

test('MCP JSON errors are safe, BOM is accepted, and reads and replies are bounded', async t => {
  const f = await fixture(t);
  await writeFile(join(f.agent, 'mcp.json'), '\uFEFF{"mcpServers":{"ok":{"command":"node"}}}');
  assert.equal((await f.list()).servers[0].name, 'ok');
  await writeFile(join(f.repo, '.pi', 'mcp.json'), '{"mcpServers":{"secret":"SECRET_BAD_JSON');
  assert.ok(!(JSON.stringify(await f.list())).includes('SECRET_BAD_JSON'));
  assert.equal((await f.list(false)).problems.length, 0);
  await writeFile(join(f.agent, 'mcp.json'), ' '.repeat(1024 * 1024 + 1));
  assert.match((await f.list(false)).problems[0].message, /1 MiB/);
  await f.user(Object.fromEntries(Array.from({ length: 700 }, (_, i) => [`server${i}`, { command: 'node' }])));
  const listed = await f.list(false);
  const reply = report(f.repo, false, { skills: [], templates: [], problems: [], mcpServers: listed.servers });
  assert.equal(reply.mcp_servers.length, 500);
  assert.ok(reply.problems.some(problem => problem.message.includes('500 of 700 MCP servers')));
  assert.ok(Buffer.byteLength(JSON.stringify(reply)) < 1024 * 1024);
});

test('listing resolves no secrets and starts no process, transport, or network request', async t => {
  const f = await fixture(t); const secret = 'SECRET_MCP_VALUE';
  await f.user({
    local: { command: '/opt/tools/node', args: ['--token', `${secret}_ARG`], env: { TOKEN: `${secret}_ENV`, COMMAND: `!touch ${f.base}/spawned`, VAR: '${MCP_SECRET_TEST}' } },
    remote: { url: `https://user:${secret}_PASSWORD@docs.example/private-mcp-endpoint/${secret}_PATH?token=${secret}_QUERY`, headers: { Authorization: `!echo ${secret}_HEADER`, Other: '${MCP_SECRET_TEST}' } },
    oauth: { url: 'https://oauth.example/mcp', oauth: { clientSecret: `!echo ${secret}_CLIENT` } },
  });
  await f.write(join(f.agent, 'mcp-auth.json'), {
    'mcp__oauth|https://oauth.example/mcp': { serverUrl: 'https://oauth.example/mcp', tokens: { access_token: `${secret}_TOKEN`, refresh_token: `${secret}_REFRESH` } },
  });
  const old = process.env.MCP_SECRET_TEST; process.env.MCP_SECRET_TEST = `${secret}_EXPANDED`;
  t.after(async () => { if (old === undefined) delete process.env.MCP_SECRET_TEST; else process.env.MCP_SECRET_TEST = old; });
  const calls: string[] = [];
  const deny = (name: string) => () => { calls.push(name); throw new Error(`Unexpected ${name}`); };
  for (const method of ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork'] as const) t.mock.method(childProcess, method, deny(method));
  t.mock.method(net.Server.prototype, 'listen', deny('listen'));
  t.mock.method(net.Socket.prototype, 'connect', deny('socket')); t.mock.method(tls, 'connect', deny('tls'));
  t.mock.method(http, 'request', deny('http')); t.mock.method(https, 'request', deny('https'));
  t.mock.method(globalThis, 'fetch', deny('fetch')); syncBuiltinESMExports();
  try {
    const found = await f.list();
    const serialized = JSON.stringify(report(f.repo, true, { skills: [], templates: [], problems: found.problems, mcpServers: found.servers }));
    for (const value of [secret, '!touch', '!echo', 'MCP_SECRET_TEST', '--token', '/private-mcp-endpoint/']) assert.ok(!serialized.includes(value), value);
    assert.equal(found.servers[0].program, '/opt/tools/node'); assert.equal(found.servers[0].argument_count, 2); assert.equal(found.servers[1].host, 'docs.example');
    assert.equal(found.servers[2].signed_in, true); assert.deepEqual(calls, []);
    await assert.rejects(readFile(join(f.base, 'spawned')), { code: 'ENOENT' });
  } finally { t.mock.restoreAll(); syncBuiltinESMExports(); }
});

test('OAuth metadata respects exact URLs, namespaces and legacy keys, without claiming public servers need sign-in', async t => {
  const f = await fixture(t);
  await f.user({
    public: { url: 'https://public.example' }, explicit: { url: 'https://explicit.example', oauth: {} },
    header: { url: 'https://header.example', oauth: {}, headers: { aUtHoRiZaTiOn: 'secret' } },
    provider: { url: 'https://provider.example', auth: { provider: 'p' } },
    'saved-name': { url: 'https://saved.example' }, legacy: { url: 'https://legacy.example' },
    wrong: { url: 'https://wrong.example' }, registered: { url: 'https://registered.example' },
  });
  const tokens = { access_token: 'secret' };
  const auth = {
    'mcp__saved_name|https://saved.example/': { serverUrl: 'https://saved.example/', tokens },
    'https://legacy.example/': { serverUrl: 'https://legacy.example/', tokens },
    'mcp__wrong|https://wrong.example/': { serverUrl: 'https://another.example/', tokens },
    'mcp__registered|https://registered.example/': { serverUrl: 'https://registered.example/', clientInformation: { client_id: 'secret' } },
  };
  await f.write(join(f.agent, 'mcp-auth.json'), auth);
  assert.deepEqual((await f.list()).servers.map(row => [row.name, row.needs_sign_in, row.signed_in]), [
    ['public', null, false], ['explicit', true, false], ['header', false, false], ['provider', false, false],
    ['saved-name', true, true], ['legacy', true, true], ['wrong', null, false], ['registered', true, false],
  ]);
  assert.deepEqual(JSON.parse(await readFile(join(f.agent, 'mcp-auth.json'), 'utf8')), auth, 'No migration or token writes');
});

test('Pi builtin MCP disable and project overrides follow its package resolver', async t => {
  const f = await fixture(t); await f.user({ local: { command: 'node' } });
  const home = process.env.HOME; process.env.HOME = f.base;
  t.after(async () => { if (home === undefined) delete process.env.HOME; else process.env.HOME = home; });
  await f.write(join(f.agent, 'settings.json'), { extensions: ['-builtin:mcp'] });
  await f.write(join(f.repo, '.pi', 'settings.json'), { extensions: ['+builtin:mcp'] });
  for (const trusted of [false, true]) {
    const settingsManager = SettingsManager.create(f.repo, f.agent, { projectTrusted: trusted });
    const resolved = await new DefaultPackageManager({ cwd: f.repo, agentDir: f.agent, settingsManager, builtinExtensions: ['mcp'] }).resolve(async () => 'skip');
    const enabled = resolved.extensions.find(entry => entry.path === 'builtin:mcp')!.enabled;
    assert.equal(enabled, trusted);
    const server = (await f.list(trusted, enabled)).servers[0];
    assert.equal(server.can_enable, trusted); assert.equal(server.enabled, trusted);
  }
});

test('project MCP switches round trip in the host store under both policies and cannot bypass Pi disable', async t => {
  const f = await fixture(t); await f.user({ on: { command: 'node' }, off: { command: 'node', enabled: false } });
  const directory = join(f.base, 'store'); await mkdir(directory);
  let store = new Store(directory); t.after(async () => store.close());
  const identity = { dev: '1', ino: '2' };
  const definitions = await loadMcp(f.repo, true, f.agent, true);
  const read = async (root = f.repo, who = identity, pi = true) => {
    const found = await loadMcp(f.repo, commandPolicy(store, root, who) === 'trusted', f.agent, pi);
    return mcpPolicy(store, root, who, found.servers, found.entries);
  };
  assert.deepEqual((await read()).map(row => row.enabled), [true, false]);
  setCommandPolicy(store, f.repo, identity, 'sandboxed');
  assert.deepEqual((await read()).map(row => row.enabled), [false, false]);
  setMcpEnabled(store, f.repo, identity, definitions.entries.get('on')!, true); setMcpEnabled(store, f.repo, identity, definitions.entries.get('off')!, true);
  assert.deepEqual((await read()).map(row => row.enabled), [true, true]);
  store.close(); store = new Store(directory);
  assert.deepEqual((await read()).map(row => row.enabled), [true, true]);
  setCommandPolicy(store, f.repo, identity, 'trusted'); setMcpEnabled(store, f.repo, identity, definitions.entries.get('on')!, false);
  assert.deepEqual((await read()).map(row => row.enabled), [false, true]);
  assert.deepEqual((await read('/other')).map(row => row.enabled), [true, false]);
  assert.deepEqual((await read(f.repo, { dev: '1', ino: '3' })).map(row => row.enabled), [true, false]);
  assert.deepEqual((await read(f.repo, identity, false)).map(row => row.enabled), [false, false]);
  for (const corrupt of ['{', 'null', '{}', 'true']) {
    store.setSetting(`mcp_server:${JSON.stringify([f.repo, 'on'])}`, corrupt);
    assert.equal((await read())[0].enabled, false);
  }
});

test('set_mcp_server is a version 3 additive operation with strict fields', () => {
  const request = { op: 'set_mcp_server', checkout: '/repo', name: 'docs', enabled: true, protocol_version: 3 };
  assert.deepEqual(validate(request), request);
  assert.deepEqual(validate({ ...request, definition: 'a'.repeat(64) }), { ...request, definition: 'a'.repeat(64) });
  for (const bad of [{ ...request, protocol_version: 1 }, { ...request, enabled: 'true' }, { ...request, name: '' },
    { ...request, env: 'secret' }, { ...request, enabled: undefined }, { ...request, definition: 'x' }, { ...request, definition: 1 }]) assert.throws(() => validate(bad), { code: 'invalid_request' });
});

const identity = { dev: '1', ino: '2' };
function switches() {
  const values = new Map<string, string>();
  return { values, setting: (key: string) => values.get(key) ?? null,
    setSetting: (key: string, value: string | null) => { if (value === null) values.delete(key); else values.set(key, value); } };
}

test('project definitions and replacements need review, with digest and directory identity binding', async t => {
  const f = await fixture(t); const settings = switches();
  await f.user({ user: { command: 'node' }, replaced: { command: 'user-program' } });
  const config = { command: 'project-program', args: ['SECRET_ARG'], cwd: '.', env: { TOKEN: '${SECRET_VAR}', KEY: '!echo SECRET_COMMAND' } };
  await f.project({ local: config, replaced: config });
  const load = () => loadMcp(f.repo, true, f.agent, true);
  let found = await load();
  const read = (who = identity) => mcpPolicy(settings, f.repo, who, found.servers, found.entries);
  assert.deepEqual(read().map(row => [row.name, row.enabled, row.consent]), [
    ['user', true, undefined], ['replaced', false, 'required'], ['local', false, 'required'],
  ]);
  assert.match(read()[1].disabled_reason!, /Defined by this project/);
  const entry = found.entries.get('local')!;
  setMcpEnabled(settings, f.repo, identity, entry, true);
  assert.equal(read()[2].enabled, true); assert.equal(read()[2].consent, undefined);
  assert.equal(JSON.parse([...settings.values.values()][0]).definition, definitionDigest(entry));
  assert.doesNotMatch(JSON.stringify([...settings.values]), /SECRET|project-program|TOKEN/);
  assert.equal(read({ dev: '1', ino: '3' })[2].enabled, false);
  await f.project({ local: { ...config, args: ['changed'] }, replaced: config }); found = await load();
  assert.equal(read()[2].enabled, false); assert.equal(read()[2].consent, 'changed');
  assert.match(read()[2].disabled_reason!, /definition changed/);
  setMcpEnabled(settings, f.repo, identity, found.entries.get('local')!, true);
  assert.equal(read()[2].enabled, true);
  settings.setSetting(`mcp_server:${JSON.stringify([f.repo, 'local'])}`, JSON.stringify({ root: identity, enabled: true }));
  assert.equal(read()[2].enabled, false); assert.equal(read()[2].consent, 'required');
  const sandboxed = await loadMcp(f.repo, false, f.agent, true);
  assert.deepEqual(sandboxed.servers.map(row => [row.name, row.enabled]), [['user', false], ['replaced', false]]);
});

test('every unresolved launch field and exposure change invalidates a saved project choice', async t => {
  const f = await fixture(t); const settings = switches();
  const stdio = { command: 'node', args: ['a'], cwd: '.', env: { KEY: '${ONE}' } };
  const http = { url: 'http://127.0.0.1:1234/mcp', headers: { Key: '!echo ONE' } };
  for (const [original, changes] of [
    [stdio, [{ ...stdio, command: 'other' }, { ...stdio, args: ['b'] }, { ...stdio, cwd: 'subdir' },
      { ...stdio, env: { OTHER: '${ONE}' } }, { ...stdio, env: { KEY: '${TWO}' } }, { ...stdio, exposure: 'hidden' }, http]],
    [http, [{ ...http, url: 'http://127.0.0.1:1235/mcp' }, { ...http, headers: { Other: '!echo ONE' } }, { ...http, headers: { Key: '!echo TWO' } }]],
  ] as const) {
    for (const changed of changes) {
      await f.project({ server: original }); const before = await loadMcp(f.repo, true, f.agent, true);
      setMcpEnabled(settings, f.repo, identity, before.entries.get('server')!, true);
      await f.project({ server: changed }); const after = await loadMcp(f.repo, true, f.agent, true);
      const row = mcpPolicy(settings, f.repo, identity, after.servers, after.entries)[0];
      assert.equal(row.enabled, false); assert.equal(row.consent, 'changed');
    }
  }
});

test('unreviewed project overrides only narrow user enablement and exact or wildcard tool exposure', async t => {
  const f = await fixture(t); const settings = switches();
  await f.user({
    disabled: { command: 'node', enabled: false },
    hidden: { command: 'node', exposure: 'hidden' },
    filtered: { command: 'node', exposure: 'direct', toolExposure: { private: 'hidden', 'secret*': 'hidden', public: 'direct' } },
  });
  await f.project({
    disabled: { enabled: true }, hidden: { exposure: 'direct', toolExposure: { '*': 'direct' } },
    filtered: { exposure: 'deferred', toolExposure: { private: 'direct', 'secret*': 'codemode', public: 'hidden' } },
  });
  const found = await loadMcp(f.repo, true, f.agent, true);
  const read = () => mcpPolicy(settings, f.repo, identity, found.servers, found.entries);
  const [disabled, hidden, filtered] = read();
  assert.equal(disabled.enabled, false); assert.equal(disabled.consent, 'required');
  assert.equal(eligible(executionEntry(found.entries.get('hidden')!, hidden), hidden), false);
  assert.equal(filtered.consent, 'restricted'); assert.equal(filtered.enabled, true);
  const entry = executionEntry(found.entries.get('filtered')!, filtered);
  for (const name of ['private', 'secretThing', 'public']) assert.equal(toolExposure(entry, name), 'hidden');
  assert.equal(toolExposure(entry, 'other'), 'deferred');
  for (const row of read()) setMcpEnabled(settings, f.repo, identity, found.entries.get(row.name)!, true);
  assert.ok(read().every(row => row.enabled && row.consent === undefined));
  assert.notEqual(toolExposure(executionEntry(found.entries.get('filtered')!, read()[2]), 'private'), 'hidden');
  assert.equal(eligible(executionEntry(found.entries.get('hidden')!, read()[1]), read()[1]), true);
  await f.user({ same: { command: 'node', enabled: true } });
  await f.project({ same: { enabled: true } });
  const before = await loadMcp(f.repo, true, f.agent, true);
  setMcpEnabled(settings, f.repo, identity, before.entries.get('same')!, true);
  await f.project({ same: {} });
  const after = await loadMcp(f.repo, true, f.agent, true);
  assert.equal(mcpPolicy(settings, f.repo, identity, after.servers, after.entries)[0].consent, 'changed', 'Even an equivalent raw project entry must be reviewed again');
});

test('server names with escapes, direction controls, or zero-width characters never reach listing labels', async t => {
  const f = await fixture(t);
  await f.user(Object.fromEntries(['bad\u001bname', 'bad\u202ename', 'bad\u200bname', 'x'.repeat(129)].map(name => [name, { command: 'never-start' }])));
  const found = await f.list(); assert.equal(found.servers.length, 0); assert.equal(found.problems.length, 4);
  const serialized = JSON.stringify(found);
  for (const unsafe of ['\u001b', '\u202e', '\u200b', '\\u001b']) assert.ok(!serialized.includes(unsafe));
});

test('project configuration symlinks and executable control characters are refused without echoing values', async t => {
  const f = await fixture(t); await f.user({ docs: { command: '/bin/true' } });
  await symlink(join(f.agent, 'mcp.json'), join(f.repo, '.pi/mcp.json'));
  const linked = await f.list(); assert.equal(linked.servers[0].source, join(f.agent, 'mcp.json'));
  assert.match(JSON.stringify(linked.problems), /could not be read/);
  for (const control of ['\n', '\u001b', '\u0085', '\u202e', '\u200b', '\u2028', '\ufeff']) {
    await f.user({ bad: { command: '/bin/sh' + control + 'Connects to: forged.example' } });
    const { servers, problems } = await f.list(false);
    assert.equal(servers[0].can_enable, false); assert.equal(servers[0].program, null);
    assert.match(JSON.stringify(problems), /control or format/); assert.doesNotMatch(JSON.stringify([servers, problems]), /forged.example/);
  }
});

test('sandboxed loader variables are listed as problems and never eligible', async t => {
  const f = await fixture(t);
  for (const key of ['DYLD_INSERT_LIBRARIES', 'DYLD_LIBRARY_PATH', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'LIBPATH', 'NODE_OPTIONS', 'PYTHONPATH', 'BASH_ENV']) {
    await f.user({ docs: { command: '/bin/true', env: { [key]: 'private-loader-value' } } });
    const found = await loadMcp(f.repo, false, f.agent, true); const row = found.servers[0];
    assert.equal(row.enabled, false); assert.equal(row.can_enable, false);
    assert.match(row.problem!, /loader environment/); assert.doesNotMatch(JSON.stringify(found.servers), /private-loader-value/);
    assert.equal(eligible(found.entries.get('docs')!, row), false);
    assert.equal((await f.list(true)).servers[0].problem, null);
  }
});


test('macOS project configuration rejects a linked parent directory too', { skip: process.platform !== 'darwin' }, async t => {
  const f = await fixture(t); await f.user({ docs: { command: '/bin/true' } });
  await rm(join(f.repo, '.pi'), { recursive: true }); await symlink(f.agent, join(f.repo, '.pi'));
  const found = await loadMcp(f.repo, true, f.agent, true);
  assert.equal(found.entries.get('docs')!.project, undefined);
  assert.equal(found.servers[0].source, join(f.agent, 'mcp.json'));
  assert.match(JSON.stringify(found.problems), /could not be read/);
});

test('a sustained flood fixture exits when the client disconnects', { timeout: 15_000 }, async t => {
  const path = fileURLToPath(new URL('./support/mcp-fixture.mjs', import.meta.url));
  const directory = await fixtureDirectory(join(tmpdir(), 'plait-mcp-flood-'));
  const input = join(directory, 'initialize');
  const output = join(directory, 'messages');
  await writeFile(input, JSON.stringify({ method: 'initialize', id: 1 }) + '\n');
  const request = await open(input);
  t.after(() => request.close());
  const messages = await open(output, 'w');
  t.after(() => messages.close());
  // EOF immediately after initialize exercises a disconnect while the interval is live.
  const child = childProcess.spawn(process.execPath, [path, 'flood-sustained'], { stdio: [request.fd, messages.fd, 'ignore'] });
  const closed = once(child, 'close');
  t.after(async () => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); await closed; });
  // The timeout catches a surviving interval before the fixture's 30-second orphan watchdog.
  assert.deepEqual(await closed, [0, null]);
  assert.match(await readFile(output, 'utf8'), /notifications\/progress/);
});
