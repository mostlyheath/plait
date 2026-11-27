import { fixtureDirectory } from './support/temp.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import { rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { codexAccount, codexModels, compareVersions, findCodex, judgeVersion, parseVersionOutput } from '../src/codex/install.ts';
import { CODEX_SCHEMA_VERSION } from '../src/codex/protocol.ts';
import { AppServer } from '../src/codex/server.ts';
import { FAKE_CODEX, FAKE_CODEX_SCRIPT, fakeEnv } from './support/fake-codex-env.ts';

test('compareVersions orders releases and prereleases', () => {
  assert.equal(compareVersions('0.169.0', '0.169.0'), 0);
  assert.equal(compareVersions('0.168.9', '0.169.0'), -1);
  assert.equal(compareVersions('0.170.0', '0.169.12'), 1);
  assert.equal(compareVersions('1.0.0', '0.999.999'), 1);
  assert.equal(compareVersions('0.169.10', '0.169.9'), 1);
  assert.equal(compareVersions('0.169.0-alpha.1', '0.169.0'), -1);
  assert.equal(compareVersions('0.169.0', '0.169.0-alpha.1'), 1);
  assert.throws(() => compareVersions('0.169', '0.169.0'), /Not a version/);
  assert.throws(() => compareVersions('0.169.0', 'v0.169.0'), /Not a version/);
});

test('parseVersionOutput reads codex-cli output only', () => {
  assert.equal(parseVersionOutput('codex-cli 0.169.0\n'), '0.169.0');
  assert.equal(parseVersionOutput('warning: something\ncodex-cli 0.170.2-alpha.3\n'), '0.170.2-alpha.3');
  assert.equal(parseVersionOutput('codex 0.169.0\n'), undefined);
  assert.equal(parseVersionOutput('codex-cli 0.169\n'), undefined);
  assert.equal(parseVersionOutput(''), undefined);
});

test('judgeVersion refuses older, tests 0.169.x, flags newer', () => {
  assert.equal(CODEX_SCHEMA_VERSION, '0.169.0');
  const old = judgeVersion('/bin/codex', '0.168.4');
  assert.equal(old.state, 'unsupported_version');
  assert.match(old.message, /Install Codex 0\.169 or newer\./);
  assert.equal(judgeVersion('/bin/codex', '0.169.0-alpha.1').state, 'unsupported_version');
  assert.deepEqual(judgeVersion('/bin/codex', '0.169.5'), { state: 'ready', path: '/bin/codex', version: '0.169.5', message: 'Codex 0.169.5 is ready.' });
  const newer = judgeVersion('/bin/codex', '0.171.0');
  assert.equal(newer.state, 'ready');
  assert.equal(newer.untested, true);
});

test('findCodex reports a missing explicit path', async () => {
  const result = await findCodex({ path: '/nonexistent/plait/codex', env: fakeEnv() });
  assert.equal(result.state, 'not_installed');
  assert.match(result.message, /Choose another path or install Codex\./);
  assert.equal((await findCodex({ path: 'relative/codex', env: fakeEnv() })).state, 'not_installed');
});

test('findCodex checks the version of an explicit path', async () => {
  assert.deepEqual(await findCodex({ path: FAKE_CODEX, env: fakeEnv() }),
    { state: 'ready', path: FAKE_CODEX, version: '0.169.0', managed: false, message: 'Codex 0.169.0 is ready.' });
  const old = await findCodex({ path: FAKE_CODEX, env: fakeEnv({ FAKE_CODEX_VERSION: '0.150.0' }) });
  assert.equal(old.state, 'unsupported_version');
  assert.equal(old.version, '0.150.0');
  const newer = await findCodex({ path: FAKE_CODEX, env: fakeEnv({ FAKE_CODEX_VERSION: '0.170.1' }) });
  assert.equal(newer.state, 'ready');
  assert.equal(newer.untested, true);
  const garbled = await findCodex({ path: FAKE_CODEX, env: fakeEnv({ FAKE_CODEX_SCENARIO: 'no-version' }) });
  assert.equal(garbled.state, 'error');
  assert.match(garbled.message, /did not report its version/);
});

test('findCodex searches the given PATH', async () => {
  const empty = await fixtureDirectory(join(tmpdir(), 'plait-codex-empty-'));
  const bin = await fixtureDirectory(join(tmpdir(), 'plait-codex-bin-'));
  try {
    const missing = await findCodex({ env: { ...fakeEnv(), PATH: `${empty}:relative/dir` } });
    assert.equal(missing.state, 'not_installed');
    assert.match(missing.message, /Codex was not found\. Install Codex 0\.169 or newer\./);
    // A file named codex that is not executable is skipped.
    await writeFile(join(empty, 'codex'), 'not a program', { mode: 0o600 });
    assert.equal((await findCodex({ env: { ...fakeEnv(), PATH: empty } })).state, 'not_installed');
    const shim = join(bin, 'codex');
    await writeFile(shim, `#!/bin/sh\nexec '${process.execPath}' '${FAKE_CODEX_SCRIPT}' "$@"\n`, { mode: 0o700 });
    const found = await findCodex({ env: { ...fakeEnv({ FAKE_CODEX_VERSION: '0.169.2' }), PATH: `${empty}:${bin}` } });
    assert.equal(found.state, 'ready');
    assert.equal(found.path, shim);
    assert.equal(found.version, '0.169.2');
  } finally {
    await rm(empty, { recursive: true, force: true });
    await rm(bin, { recursive: true, force: true });
  }
});

test('codexAccount reads sign-in state from account/read', async () => {
  const signedIn = await AppServer.start(FAKE_CODEX, { env: fakeEnv() });
  const signedOut = await AppServer.start(FAKE_CODEX, { env: fakeEnv({ FAKE_CODEX_SCENARIO: 'signed-out' }) });
  try {
    assert.equal(await codexAccount(signedIn), 'signed_in');
    assert.equal(await codexAccount(signedOut), 'signed_out');
  } finally {
    await signedIn.close();
    await signedOut.close();
  }
});

test('codexModels follows pages and drops hidden models', async () => {
  const server = await AppServer.start(FAKE_CODEX, { env: fakeEnv() });
  try {
    assert.deepEqual(await codexModels(server), [
      { id: 'gpt-fake', name: 'GPT-FAKE', efforts: ['low', 'high'], defaultEffort: 'low', isDefault: true },
      { id: 'gpt-fake-mini', name: 'GPT-FAKE-MINI', efforts: ['low', 'high'], defaultEffort: 'low', isDefault: false },
    ]);
  } finally {
    await server.close();
  }
});
