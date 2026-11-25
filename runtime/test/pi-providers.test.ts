import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { chmod, lstat, mkdir, mkdtemp, readFile, realpath, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { TestContext } from 'node:test';
import { failure } from '../src/protocol.ts';
import type { ProviderView } from '../src/providers.ts';
import { checkEndpoint, readEndpoints, renderEndpoints, writeEndpoints } from '../src/endpoints.ts';

// These tests run Pi's real registry and credential store against a disposable agent
// directory. The home and agent directory are redirected before Pi is loaded, and any
// request that is not to this machine fails the test instead of leaving it.
const sandbox = await realpath(await mkdtemp(join(tmpdir(), 'plait-pi-providers-')));
process.env.HOME = sandbox;
process.env.PI_CODING_AGENT_DIR = join(sandbox, 'unused-default-agent');
process.env.PI_OFFLINE = '1';
for (const name of Object.keys(process.env)) if (/_API_KEY$|_TOKEN$/.test(name)) delete process.env[name];
const outbound: string[] = [];
const realFetch = globalThis.fetch;
globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
  const url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url);
  if (url.hostname !== '127.0.0.1') { outbound.push(url.origin); throw new Error('Tests must not reach the network'); }
  return realFetch(input, init);
}) as typeof fetch;
const { Providers } = await import('../src/providers.ts');
const { piProviders } = await import('../src/pi-providers.ts');
test.after(async () => {
  // Pi's default agent directory was redirected and must not have been used at all.
  assert(!existsSync(process.env.PI_CODING_AGENT_DIR!));
  await rm(sandbox, { recursive: true, force: true });
});

const KEY = 'sk-or-KEY_ONE_DO_NOT_LEAK';
const NEXT_KEY = 'sk-or-KEY_TWO_DO_NOT_LEAK';
const FILE_KEY = 'sk-FILE_KEY_DO_NOT_LEAK';
type Provider = { provider: ProviderView };
type List = { providers: ProviderView[] };

async function host(t: TestContext, seed?: (agent: string) => Promise<void>) {
  const agent = await mkdtemp(join(sandbox, 'agent-'));
  await seed?.(agent);
  const source = piProviders(agent);
  const registry = await source.load();
  const holder = { models: registry.models, authenticated: registry.authenticated };
  const providers = new Providers(holder, source, registry);
  t.after(() => providers.close());
  const transcript: string[] = [];
  const call = async <T = Record<string, any>>(input: unknown) => {
    let reply;
    try { reply = { ok: true, result: await providers.handle(input) }; } catch (error) { reply = { ok: false, error: failure(error) }; }
    transcript.push(JSON.stringify(reply));
    return reply as { ok: boolean; result: T; error?: { code: string; message: string } };
  };
  const provider = async (id: string) => (await call<List>({ op: 'list_providers' })).result.providers.find(item => item.id === id);
  const noSecrets = () => {
    const joined = transcript.join('\n');
    for (const secret of [KEY, NEXT_KEY, FILE_KEY]) assert(!joined.includes(secret), `a reply contains ${secret}`);
    assert.deepEqual(outbound, []);
  };
  return { agent, auth: join(agent, 'auth.json'), models: join(agent, 'models.json'), holder, call, provider, noSecrets };
}
const mode = async (path: string) => (await stat(path)).mode & 0o777;
const json = async (path: string) => JSON.parse(await readFile(path, 'utf8'));

async function server(t: TestContext, handler: Parameters<typeof createServer>[1]) {
  const seen: (string | undefined)[] = [];
  const listener = createServer((request, response) => { seen.push(request.headers.authorization); handler!(request, response); });
  await new Promise<void>(resolve => listener.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => { listener.closeAllConnections(); listener.close(resolve); }));
  return { url: `http://127.0.0.1:${(listener.address() as AddressInfo).port}/v1`, seen };
}

test('thread naming uses a configured keyless local endpoint with no tools or conversation context', async t => {
  const { nameThread } = await import('../src/thread-naming.ts');
  const f = await host(t);
  const requests: Record<string, any>[] = [];
  const local = await server(t, async (request, response) => {
    assert.equal(request.url, '/v1/chat/completions');
    let body = ''; for await (const chunk of request) body += chunk;
    requests.push(JSON.parse(body));
    response.setHeader('content-type', 'text/event-stream');
    const chunk = (delta: object, finish_reason: string | null) => ({ id: 'name', object: 'chat.completion.chunk', created: 1, model: 'local-small', choices: [{ index: 0, delta, finish_reason }] });
    response.write(`data: ${JSON.stringify(chunk({ role: 'assistant', content: 'Improve thread naming' }, null))}\n\n`);
    response.end(`data: ${JSON.stringify(chunk({}, 'stop'))}\n\ndata: [DONE]\n\n`);
  });
  const added = await f.call({ op: 'add_endpoint', id: 'local-naming', name: 'Local naming', base_url: local.url, model_ids: ['local-small'] });
  assert.equal(added.ok, true, JSON.stringify(added));
  const result = await nameThread(f.holder.models, { provider: 'local-naming', model_id: 'local-small' }, 'Please improve how threads get their names.', AbortSignal.timeout(5000));
  assert.equal(result, 'Improve thread naming');
  assert.equal(requests.length, 1);
  assert.equal(requests[0].model, 'local-small');
  assert.equal(requests[0].tools, undefined);
  assert.deepEqual(requests[0].messages.map((message: { role: string }) => message.role), ['system', 'user']);
  assert.equal(requests[0].messages[1].content, 'Please improve how threads get their names.');
  assert.equal(requests[0].max_tokens ?? requests[0].max_completion_tokens, 2048);
  f.noSecrets();
});

test('Pi providers are listed with their sign-in and key methods', async t => {
  const f = await host(t);
  const list = (await f.call<List>({ op: 'list_providers' })).result.providers;
  assert(list.length >= 40);
  const byId = Object.fromEntries(list.map(provider => [provider.id, provider]));
  assert.deepEqual(byId.anthropic, {
    id: 'anthropic', name: 'Anthropic', kind: 'builtin', authenticated: false, credential: 'none', model_count: byId.anthropic.model_count,
    sign_in: { name: 'Anthropic (Claude Pro/Max)', subscription: true }, api_key: { name: 'Anthropic API key' }, endpoint: null,
  });
  assert.deepEqual([byId['openai-codex'].sign_in, byId['openai-codex'].api_key], [{ name: 'OpenAI (ChatGPT Plus/Pro)', subscription: true }, null]);
  assert.deepEqual([byId.openrouter.sign_in, byId.openrouter.api_key], [{ name: 'OpenRouter OAuth', subscription: false }, { name: 'OpenRouter API key' }]);
  assert.deepEqual([byId.groq.sign_in, byId.groq.api_key], [null, { name: 'Groq API key' }]);
  assert(list.every(provider => provider.kind === 'builtin' && provider.endpoint === null));
  assert.equal((await f.call({ op: 'start_sign_in', provider: 'groq' })).error?.code, 'invalid_request');
  // Pi creates its credential file empty and owner-only when it first loads.
  assert.deepEqual([await json(f.auth), await mode(f.auth)], [{}, 0o600]);
  f.noSecrets();
});

test('an API key goes into Pi\'s private credential file and comes back out of no reply', async t => {
  const f = await host(t);
  const set = await f.call<Provider>({ op: 'set_provider_key', provider: 'openrouter', key: KEY });
  assert.deepEqual([set.ok, set.result.provider.authenticated, set.result.provider.credential], [true, true, 'api_key']);
  assert(f.holder.authenticated.has('openrouter'));
  assert.equal(await mode(f.auth), 0o600);
  assert.equal(await mode(f.agent) & 0o077, 0);
  assert.deepEqual(await json(f.auth), { openrouter: { type: 'api_key', key: KEY } });
  assert.equal((await f.holder.models.getAuth('openrouter'))?.auth.apiKey, KEY);

  // A credential file left readable by others is made owner-only on the next write.
  await chmod(f.auth, 0o644);
  await f.call({ op: 'set_provider_key', provider: 'openrouter', key: NEXT_KEY });
  assert.equal(await mode(f.auth), 0o600);
  assert.deepEqual(await json(f.auth), { openrouter: { type: 'api_key', key: NEXT_KEY } });

  const out = await f.call<Provider>({ op: 'sign_out_provider', provider: 'openrouter' });
  assert.deepEqual([out.result.provider.authenticated, out.result.provider.credential], [false, 'none']);
  assert.deepEqual(await json(f.auth), {});
  assert(!f.holder.authenticated.has('openrouter'));

  // Bedrock's key setup asks several questions; nothing is stored for it here.
  const bedrock = await f.call({ op: 'set_provider_key', provider: 'amazon-bedrock', key: KEY });
  assert.deepEqual([bedrock.ok, bedrock.error?.code], [false, 'invalid_request']);
  assert.deepEqual(await json(f.auth), {});
  f.noSecrets();
});

test('a key that looks like a Pi command or variable is stored and used as plain text', async t => {
  const f = await host(t);
  const marker = join(f.agent, 'command-ran');
  const command = `!touch ${marker}`;
  await f.call({ op: 'set_provider_key', provider: 'groq', key: command });
  assert.equal((await f.holder.models.getAuth('groq'))?.auth.apiKey, command);
  await f.call({ op: 'set_provider_key', provider: 'mistral', key: 'sk-$HOME-${PATH}' });
  assert.equal((await f.holder.models.getAuth('mistral'))?.auth.apiKey, 'sk-$HOME-${PATH}');
  await f.call({ op: 'list_providers' });
  assert(!existsSync(marker));
});

const HAND_WRITTEN = {
  $schema: 'https://pi.dev/schemas/models.schema.json',
  'x-top': { note: 'an unknown top-level key' },
  providers: {
    anthropic: { baseUrl: 'https://proxy.example.invalid/v1' },
    lab: {
      name: 'Lab', baseUrl: 'http://127.0.0.1:9/v1', api: 'openai-completions', apiKey: FILE_KEY,
      headers: { 'x-team': 'kept' }, compat: { supportsDeveloperRole: false }, 'x-note': 'a field Pi does not define',
      models: [
        { id: 'big', name: 'Big model', contextWindow: 262144, maxTokens: 32768, reasoning: true },
        { id: 'small' },
      ],
    },
  },
};
const seeded = (content: unknown = HAND_WRITTEN, indent: number | string = '\t') => async (agent: string) => {
  const path = join(agent, 'models.json');
  await writeFile(path, JSON.stringify(content, null, indent) + '\n', { mode: 0o644 });
  // Mode-preservation tests need this exact starting mode regardless of umask.
  await chmod(path, 0o644);
};

test('adding an endpoint writes Pi\'s format, keeps the rest of models.json, and reloads', async t => {
  const f = await host(t, seeded());
  const lab = (await f.provider('lab'))!;
  assert.deepEqual(lab, {
    id: 'lab', name: 'Lab', kind: 'custom', authenticated: true, credential: 'external', model_count: 2,
    sign_in: null, api_key: { name: 'API key' }, endpoint: { base_url: 'http://127.0.0.1:9/v1', api: 'openai-completions', model_ids: ['big', 'small'] },
  });

  const added = await f.call<Provider>({ op: 'add_endpoint', id: 'home', name: 'Home server', base_url: 'http://127.0.0.1:8080/v1', model_ids: ['qwen', 'llama'], key: KEY });
  assert.equal(added.ok, true, JSON.stringify(added));
  assert.deepEqual(added.result.provider, {
    id: 'home', name: 'Home server', kind: 'custom', authenticated: true, credential: 'api_key', model_count: 2,
    sign_in: null, api_key: { name: 'API key' }, endpoint: { base_url: 'http://127.0.0.1:8080/v1', api: 'openai-completions', model_ids: ['qwen', 'llama'] },
  });
  assert(f.holder.models.getModel('home', 'qwen'));
  assert(f.holder.authenticated.has('home'));
  // The key is in the credential file; models.json holds only Pi's keyless placeholder.
  assert.deepEqual((await json(f.auth)).home, { type: 'api_key', key: KEY });
  assert.deepEqual(await json(f.models), { ...HAND_WRITTEN, providers: { ...HAND_WRITTEN.providers, home: {
    name: 'Home server', baseUrl: 'http://127.0.0.1:8080/v1', api: 'openai-completions', apiKey: 'none', models: [{ id: 'qwen' }, { id: 'llama' }],
  } } });
  const text = await readFile(f.models, 'utf8');
  assert(text.includes('\n\t"providers"') && text.endsWith('}\n'));
  assert.equal(await mode(f.models), 0o644);

  assert.equal((await f.call({ op: 'add_endpoint', id: 'home', name: 'Again', base_url: 'http://127.0.0.1:1/v1', model_ids: ['a'] })).error?.code, 'conflict');
  assert.equal((await f.call({ op: 'add_endpoint', id: 'anthropic', name: 'Mine', base_url: 'http://127.0.0.1:1/v1', model_ids: ['a'] })).error?.code, 'conflict');
  f.noSecrets();
});

test('editing an endpoint preserves fields Plait does not understand', async t => {
  const f = await host(t, seeded());
  const updated = await f.call<Provider>({ op: 'update_endpoint', id: 'lab', name: 'Lab (renamed)', model_ids: ['small', 'big', 'new'] });
  assert.equal(updated.ok, true, JSON.stringify(updated));
  assert.deepEqual([updated.result.provider.name, updated.result.provider.endpoint?.model_ids], ['Lab (renamed)', ['small', 'big', 'new']]);
  assert.deepEqual(await json(f.models), { ...HAND_WRITTEN, providers: { ...HAND_WRITTEN.providers, lab: {
    ...HAND_WRITTEN.providers.lab, name: 'Lab (renamed)',
    models: [{ id: 'small' }, HAND_WRITTEN.providers.lab.models[0], { id: 'new' }],
  } } });
  assert.equal(f.holder.models.getModel('lab', 'big')?.contextWindow, 262144);

  // A new key goes to the credential store and outranks the hand-written one, which stays put.
  const keyed = await f.call<Provider>({ op: 'update_endpoint', id: 'lab', base_url: 'http://127.0.0.1:10/v1', key: KEY });
  assert.deepEqual([keyed.result.provider.credential, keyed.result.provider.endpoint?.base_url], ['api_key', 'http://127.0.0.1:10/v1']);
  assert.equal((await json(f.models)).providers.lab.apiKey, FILE_KEY);
  assert.equal((await f.holder.models.getAuth('lab'))?.auth.apiKey, KEY);

  assert.equal((await f.call({ op: 'update_endpoint', id: 'absent', name: 'x' })).error?.code, 'not_found');
  assert.equal((await f.call({ op: 'update_endpoint', id: 'openai', name: 'x' })).error?.code, 'invalid_request');
  assert.equal((await f.call({ op: 'remove_endpoint', id: 'anthropic' })).error?.code, 'invalid_request');

  const removed = await f.call({ op: 'remove_endpoint', id: 'lab' });
  assert.deepEqual(removed.result, { removed: 'lab' });
  assert.deepEqual(await json(f.models), { ...HAND_WRITTEN, providers: { anthropic: HAND_WRITTEN.providers.anthropic } });
  assert.deepEqual(await json(f.auth), {});
  assert.equal(await f.provider('lab'), undefined);
  assert.equal((await f.call({ op: 'remove_endpoint', id: 'lab' })).error?.code, 'not_found');
  f.noSecrets();
});

test('models.json is created privately, written through a symlink, and never half-written', async t => {
  const created = await host(t);
  await created.call({ op: 'add_endpoint', id: 'first', name: 'First', base_url: 'http://127.0.0.1:1/v1', model_ids: ['a'] });
  assert.equal(await mode(created.models), 0o600);
  assert.deepEqual((await created.provider('first'))?.credential, 'external');
  assert.deepEqual(await json(created.auth), {});

  const linked = await host(t, async agent => {
    await mkdir(join(agent, 'dotfiles'));
    await seeded()(join(agent, 'dotfiles'));
    await symlink(join(agent, 'dotfiles', 'models.json'), join(agent, 'models.json'));
  });
  await linked.call({ op: 'update_endpoint', id: 'lab', name: 'Linked' });
  assert((await lstat(linked.models)).isSymbolicLink());
  assert.equal((await json(join(linked.agent, 'dotfiles', 'models.json'))).providers.lab.name, 'Linked');
  assert.deepEqual((await import('node:fs')).readdirSync(join(linked.agent, 'dotfiles')), ['models.json']);

  // A save that raced with another editor is refused and leaves no temporary file.
  const file = await readEndpoints(linked.models);
  await writeFile(file.path, (await readFile(file.path, 'utf8')) + '\n');
  const racing = await writeEndpoints(file, renderEndpoints(file)).then(() => undefined, error => failure(error));
  assert.equal(racing?.code, 'conflict');
  assert.deepEqual((await import('node:fs')).readdirSync(join(linked.agent, 'dotfiles')), ['models.json']);
});

test('a models.json that Plait cannot rewrite safely is left exactly as it was', async t => {
  const commented = '{\n  // my servers\n  "providers": { "lab": { "baseUrl": "http://127.0.0.1:9/v1", "api": "openai-completions", "apiKey": "x", "models": [{ "id": "a" }] } }\n}\n';
  const withComments = await host(t, agent => writeFile(join(agent, 'models.json'), commented));
  assert.equal((await withComments.provider('lab'))?.kind, 'custom');
  for (const input of [
    { op: 'add_endpoint', id: 'home', name: 'Home', base_url: 'http://127.0.0.1:1/v1', model_ids: ['a'] },
    { op: 'update_endpoint', id: 'lab', name: 'Renamed' },
    { op: 'remove_endpoint', id: 'lab' },
  ]) assert.equal((await withComments.call(input)).error?.code, 'conflict');
  assert.equal(await readFile(withComments.models, 'utf8'), commented);

  // Pi ignores the whole file when one entry breaks its schema. The edit is undone rather than left unseen.
  const broken = JSON.stringify({ providers: { bad: { models: [{ id: '' }] } } }, null, 2) + '\n';
  const invalid = await host(t, agent => writeFile(join(agent, 'models.json'), broken));
  const refused = await invalid.call({ op: 'add_endpoint', id: 'home', name: 'Home', base_url: 'http://127.0.0.1:1/v1', model_ids: ['a'] });
  assert.deepEqual([refused.ok, refused.error?.code], [false, 'conflict']);
  assert.equal(await readFile(invalid.models, 'utf8'), broken);
  assert.equal(await invalid.provider('home'), undefined);
});

test('check connection lists models from a local server and reports failures without their bodies', async t => {
  const f = await host(t, seeded());
  const good = await server(t, (_request, response) => {
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify({ object: 'list', data: [{ id: 'qwen' }, { id: 'llama' }, { id: 'qwen' }, { name: 'no id' }, { id: 7 }] }));
  });
  assert.deepEqual((await f.call({ op: 'check_endpoint', base_url: good.url, key: KEY })).result,
    { reachable: true, reason: null, status: 200, model_ids: ['qwen', 'llama'], truncated: false });
  assert.deepEqual((await f.call({ op: 'check_endpoint', base_url: `${good.url}/` })).result.model_ids, ['qwen', 'llama']);
  assert.deepEqual(good.seen, [`Bearer ${KEY}`, undefined]);

  // A saved endpoint is checked with its saved key.
  await f.call({ op: 'update_endpoint', id: 'lab', base_url: good.url });
  assert.equal((await f.call({ op: 'check_endpoint', id: 'lab' })).result.reachable, true);
  assert.equal(good.seen[2], `Bearer ${FILE_KEY}`);
  assert.equal((await f.call({ op: 'check_endpoint', id: 'anthropic' })).error?.code, 'invalid_request');
  assert.equal((await f.call({ op: 'check_endpoint', id: 'absent' })).error?.code, 'not_found');

  const denied = await server(t, (_request, response) => { response.statusCode = 401; response.end(`{"error":"bad key ${KEY}"}`); });
  assert.deepEqual((await f.call({ op: 'check_endpoint', base_url: denied.url, key: KEY })).result,
    { reachable: false, reason: 'unauthorized', status: 401, model_ids: [], truncated: false });
  const page = await server(t, (_request, response) => { response.end('<html>not an API</html>'); });
  assert.equal((await f.call({ op: 'check_endpoint', base_url: page.url })).result.reason, 'invalid_response');
  const moved = await server(t, (_request, response) => { response.statusCode = 302; response.setHeader('location', good.url + '/models'); response.end(); });
  assert.deepEqual([(await f.call({ op: 'check_endpoint', base_url: moved.url, key: KEY })).result.reason, good.seen.length], ['http_error', 3]);
  const many = await server(t, (_request, response) => { response.end(JSON.stringify({ data: Array.from({ length: 600 }, (_, index) => ({ id: `m${index}` })) })); });
  const clipped = (await f.call({ op: 'check_endpoint', base_url: many.url })).result;
  assert.deepEqual([clipped.model_ids.length, clipped.truncated], [500, true]);
  const huge = await server(t, (_request, response) => { response.end(JSON.stringify({ data: [{ id: 'x'.repeat(2_000_000) }] })); });
  assert.equal((await f.call({ op: 'check_endpoint', base_url: huge.url })).result.reason, 'invalid_response');

  const silent = await server(t, () => {});
  assert.deepEqual(await checkEndpoint(silent.url, {}, 150), { reachable: false, reason: 'timeout', status: null, model_ids: [], truncated: false });
  // Port 9 (discard) is closed on a development machine; the reply names no address.
  assert.equal((await f.call({ op: 'check_endpoint', base_url: 'http://127.0.0.1:9/v1' })).result.reason, 'unreachable');
  f.noSecrets();
});
