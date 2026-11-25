import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { Models } from '@earendil-works/pi-ai';
import { providerAuthContext, resolvesExternally, templateNames } from '../src/provider-environment.ts';
import { Providers } from '../src/providers.ts';
import type { ProviderSource, Registry } from '../src/providers.ts';
import type { HostError } from '../src/protocol.ts';

test('template names follow Pi\'s key grammar', () => {
  assert.deepEqual(templateNames('$NOUS_KEY'), ['NOUS_KEY']);
  assert.deepEqual(templateNames('${NOUS_KEY}'), ['NOUS_KEY']);
  assert.deepEqual(templateNames('Bearer ${A}-$B_2.x'), ['A', 'B_2']);
  // Escapes, commands, and malformed references name nothing.
  assert.deepEqual(templateNames('sk-$$A-$!B'), []);
  assert.deepEqual(templateNames('!pass show key $A'), []);
  assert.deepEqual(templateNames('${not valid} ${A $9 $'), []);
  assert.deepEqual(templateNames('literal'), []);
});

test('a value resolves externally when it reads a variable or runs a command', () => {
  assert.equal(resolvesExternally('$KEY'), true);
  assert.equal(resolvesExternally('Bearer ${KEY}'), true);
  assert.equal(resolvesExternally('!pass show key'), true);
  assert.equal(resolvesExternally('sk-$$literal'), false);
  assert.equal(resolvesExternally('none'), false);
  assert.equal(resolvesExternally({ key: '$KEY' }), false);
});

test('the auth context prefers the host\'s own variables and never answers host or ambient names', async t => {
  t.after(() => { delete process.env.CONTEXT_OWN; });
  process.env.CONTEXT_OWN = 'host';
  const context = providerAuthContext({
    CONTEXT_OWN: 'shell', CONTEXT_SHELL: 'shell', CONTEXT_BLANK: '  ', PLAIT_HOST_EXE: '/x',
    AWS_PROFILE: 'p', GOOGLE_CLOUD_PROJECT: 'g', AZURE_OPENAI_BASE_URL: 'https://a', CLOUDFLARE_ACCOUNT_ID: 'c',
  });
  assert.equal(await context.env('CONTEXT_OWN'), 'host');
  assert.equal(await context.env('CONTEXT_SHELL'), 'shell');
  assert.equal(await context.env('CLOUDFLARE_ACCOUNT_ID'), 'c');
  for (const name of ['CONTEXT_BLANK', 'PLAIT_HOST_EXE', 'AWS_PROFILE', 'GOOGLE_CLOUD_PROJECT', 'AZURE_OPENAI_BASE_URL', 'CONTEXT_MISSING', 'toString']) {
    assert.equal(await context.env(name), undefined, name);
  }
  assert.equal(await providerAuthContext(undefined).env('CONTEXT_SHELL'), undefined);
});

test('a host variable set to an empty or blank value is not replaced by the shell\'s', async t => {
  process.env.CONTEXT_EMPTY = '';
  process.env.CONTEXT_BLANK_HOST = '   ';
  t.after(() => { delete process.env.CONTEXT_EMPTY; delete process.env.CONTEXT_BLANK_HOST; });
  const context = providerAuthContext({ CONTEXT_EMPTY: 'shell', CONTEXT_BLANK_HOST: 'shell' });
  assert.equal(await context.env('CONTEXT_EMPTY'), undefined);
  assert.equal(await context.env('CONTEXT_BLANK_HOST'), undefined);
});

test('a login environment is published only with the registry that loaded it', async () => {
  const loads: (Record<string, string> | undefined)[] = [];
  let reading: Record<string, string> = { KEY: 'one' };
  const registry = (login?: Record<string, string>): Registry => ({
    models: { getProviders: () => [] } as unknown as Models,
    authenticated: new Set(login?.KEY ? ['lab'] : []),
    stored: async () => new Map(), settle: async () => {},
  });
  const source: ProviderSource = {
    builtin: new Set(),
    readEnvironment: async () => ({ ...reading }),
    async load(_, login) {
      loads.push(login);
      if (login?.FAIL) throw new Error('timed out');
      return registry(login);
    },
  };
  const holder = { models: {} as Models, authenticated: new Set<string>() };
  const providers = new Providers(holder, source, registry());
  let published = 0;
  providers.published = () => { published += 1; };
  await providers.adoptEnvironment();
  assert.deepEqual([...holder.authenticated], ['lab']);
  // Clients learn of the new registry: the runtime wakes workspace watchers with its revision.
  assert.deepEqual([providers.revision, published], [1, 1]);

  // The key is removed but loading fails: the previous registry and its environment stay together.
  reading = { FAIL: '1' };
  await assert.rejects(providers.handle({ op: 'reload_providers' }), (error: HostError) => error.code === 'internal');
  assert.deepEqual([...holder.authenticated], ['lab']);
  // A reload after a credential change uses the environment of the registry in use.
  await providers.reload();
  assert.deepEqual(loads.at(-1), { KEY: 'one' });
  // An unchanged shell does not reload.
  reading = { KEY: 'one' };
  const count = loads.length;
  await providers.adoptEnvironment();
  assert.equal(loads.length, count);
  // The failed load published nothing; the two successful ones did.
  assert.deepEqual([providers.revision, published], [2, 2]);
  await providers.close();
});
