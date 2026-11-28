import { fixtureDirectory } from './support/temp.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  CLAUDE_MIN_VERSION, CLAUDE_SDK_VERSION, CLAUDE_TESTED_VERSION, catalogModel, claudeAuth, claudeCatalog, claudeLevels, compareVersions,
  findClaude, judgeVersion, parseVersionOutput,
} from '../src/claude/install.ts';
import { claudeThinking } from '../src/claude/session.ts';
import { loadSdk } from '../src/claude/sdk.ts';
import { DEV_SDK, FAKE_CLAUDE, FAKE_CLAUDE_SCRIPT, fakeEnv } from './support/fake-claude-env.ts';

test('the pinned SDK is the one installed, and names the Claude Code it was built against', () => {
  const sdk = JSON.parse(readFileSync(new URL('../node_modules/@anthropic-ai/claude-agent-sdk/package.json', import.meta.url), 'utf8'));
  const runtime = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
  assert.equal(sdk.version, CLAUDE_SDK_VERSION);
  // A development dependency only: releases leave it out, and the host downloads it on setup.
  assert.equal(runtime.devDependencies['@anthropic-ai/claude-agent-sdk'], CLAUDE_SDK_VERSION);
  assert.equal(runtime.dependencies['@anthropic-ai/claude-agent-sdk'], undefined);
  assert.equal(sdk.claudeCodeVersion, CLAUDE_TESTED_VERSION);
  assert.equal(compareVersions(CLAUDE_MIN_VERSION, CLAUDE_TESTED_VERSION), -1);
});

test('compareVersions orders releases and prereleases', () => {
  assert.equal(compareVersions('2.1.326', '2.1.326'), 0);
  assert.equal(compareVersions('2.1.289', '2.1.290'), -1);
  assert.equal(compareVersions('2.1.1000', '2.1.999'), 1);
  assert.equal(compareVersions('2.1.290-beta.1', '2.1.290'), -1);
  assert.throws(() => compareVersions('2.1', '2.1.0'), /Not a version/);
});

test('parseVersionOutput reads `claude --version` output only', () => {
  assert.equal(parseVersionOutput('2.1.326 (Claude Code)\n'), '2.1.326');
  assert.equal(parseVersionOutput('warning: update available\n2.1.400 (Claude Code)\n'), '2.1.400');
  assert.equal(parseVersionOutput('2.1.326\n'), undefined);
  assert.equal(parseVersionOutput('Claude Code 2.1.326\n'), undefined);
  assert.equal(parseVersionOutput(''), undefined);
});

test('judgeVersion refuses older, runs the tested line without a note, flags a later line', () => {
  const old = judgeVersion('/bin/claude', '2.1.289');
  assert.equal(old.state, 'unsupported_version');
  assert.match(old.message, /too old\. Run `claude update`, or install 2\.1\.290 or newer\./);
  assert.equal(judgeVersion('/bin/claude', '2.1.290-rc.1').state, 'unsupported_version');
  assert.deepEqual(judgeVersion('/bin/claude', '2.1.290'), { state: 'ready', path: '/bin/claude', version: '2.1.290', message: 'Claude Code 2.1.290 is ready.' });
  assert.equal(judgeVersion('/bin/claude', '2.1.512').untested, undefined);
  const later = judgeVersion('/bin/claude', '2.2.0');
  assert.equal(later.state, 'ready');
  assert.equal(later.untested, true);
  assert.match(later.message, /newer than Plait has tested \(2\.1\.326\)/);
});

test('findClaude checks an explicit path', async () => {
  assert.equal((await findClaude({ path: '/nonexistent/plait/claude', env: fakeEnv() })).state, 'not_installed');
  assert.equal((await findClaude({ path: 'relative/claude', env: fakeEnv() })).state, 'not_installed');
  assert.deepEqual(await findClaude({ path: FAKE_CLAUDE, env: fakeEnv() }),
    { state: 'ready', path: FAKE_CLAUDE, version: '2.1.326', managed: false, message: 'Claude Code 2.1.326 is ready.' });
  assert.equal((await findClaude({ path: FAKE_CLAUDE, env: fakeEnv({ FAKE_CLAUDE_VERSION: '2.1.100' }) })).state, 'unsupported_version');
  const garbled = await findClaude({ path: FAKE_CLAUDE, env: fakeEnv({ FAKE_CLAUDE_SCENARIO: 'no-version' }) });
  assert.equal(garbled.state, 'error');
  assert.match(garbled.message, /did not report its version/);
});

test('findClaude searches the given PATH', async () => {
  const empty = await fixtureDirectory(join(tmpdir(), 'plait-claude-empty-'));
  const bin = await fixtureDirectory(join(tmpdir(), 'plait-claude-bin-'));
  try {
    const missing = await findClaude({ env: { ...fakeEnv(), PATH: `${empty}:relative/dir` } });
    assert.equal(missing.state, 'not_installed');
    assert.match(missing.message, /Claude Code was not found\. Install Claude Code 2\.1\.290 or newer\./);
    await writeFile(join(empty, 'claude'), 'not a program', { mode: 0o600 });
    assert.equal((await findClaude({ env: { ...fakeEnv(), PATH: empty } })).state, 'not_installed');
    const shim = join(bin, 'claude');
    await writeFile(shim, `#!/bin/sh\nexec '${process.execPath}' '${FAKE_CLAUDE_SCRIPT}' "$@"\n`, { mode: 0o700 });
    const found = await findClaude({ env: { ...fakeEnv({ FAKE_CLAUDE_VERSION: '2.1.330' }), PATH: `${empty}:${bin}` } });
    assert.equal(found.state, 'ready');
    assert.equal(found.path, shim);
    assert.equal(found.version, '2.1.330');
  } finally {
    await rm(empty, { recursive: true, force: true });
    await rm(bin, { recursive: true, force: true });
  }
});

test('thinking levels follow what each model reports', () => {
  assert.deepEqual(claudeLevels({ supportsEffort: true, supportedEffortLevels: ['low', 'medium', 'high', 'xhigh', 'max'], supportsAdaptiveThinking: true }),
    ['low', 'medium', 'high', 'xhigh', 'max']);
  assert.deepEqual(claudeLevels({ supportsEffort: true, supportedEffortLevels: ['max', 'low'], supportsAdaptiveThinking: true }), ['low', 'max']);
  // Adaptive thinking cannot be turned off, so no level is offered without effort.
  assert.deepEqual(claudeLevels({ supportsAdaptiveThinking: true }), []);
  assert.deepEqual(claudeLevels({}), ['off', 'low', 'medium', 'high']);

  const opus = catalogModel({ value: 'opus', resolvedModel: 'claude-x', displayName: 'Opus', description: 'd', supportsEffort: true, supportedEffortLevels: ['low', 'high'], supportsAdaptiveThinking: true });
  assert.deepEqual(claudeThinking('high', opus), { thinking: { type: 'adaptive', display: 'summarized' }, effort: 'high' });
  // A level the model lacks, or none, keeps Claude Code's default effort but still asks for readable thinking.
  assert.deepEqual(claudeThinking('medium', opus), { thinking: { type: 'adaptive', display: 'summarized' } });
  assert.deepEqual(claudeThinking(undefined, undefined), { thinking: { type: 'adaptive', display: 'summarized' } });
  assert.deepEqual(claudeThinking('off', opus), { thinking: { type: 'adaptive', display: 'summarized' } });
  const budget = catalogModel({ value: 'old', displayName: 'Old', description: '' });
  assert.deepEqual(claudeThinking('off', budget), { thinking: { type: 'disabled' } });
  assert.deepEqual(claudeThinking('medium', budget), { thinking: { type: 'enabled', budgetTokens: 16384, display: 'summarized' } });
  assert.deepEqual(claudeThinking('max', budget), {});
});

test('claudeAuth keeps sign-in state and provider, nothing that names the account', async () => {
  assert.deepEqual(await claudeAuth(FAKE_CLAUDE, fakeEnv()), { signedIn: true, provider: 'firstParty' });
  assert.deepEqual(await claudeAuth(FAKE_CLAUDE, fakeEnv({ FAKE_CLAUDE_SCENARIO: 'signed-out' })), { signedIn: false, provider: 'firstParty' });
  assert.equal(await claudeAuth('/nonexistent/claude', fakeEnv()), undefined);
});

test('claudeCatalog lists models from initialize without sending a message', async t => {
  const dir = await fixtureDirectory(join(tmpdir(), 'plait-claude-catalog-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const log = join(dir, 'log');
  const { query } = await loadSdk(DEV_SDK);
  const catalog = await claudeCatalog({ query, path: FAKE_CLAUDE, env: fakeEnv({ FAKE_CLAUDE_LOG: log }), cwd: dir });
  assert.equal(catalog.state, 'ready');
  assert.equal(catalog.version, '2.1.326');
  assert.equal(catalog.provider, 'firstParty');
  assert.deepEqual(catalog.models.map(model => [model.id, model.isDefault, model.levels.join(',')]), [
    ['default', true, 'low,medium,high,xhigh,max'], ['fake-small', false, 'low,medium,high,max'], ['fake-budget', false, 'off,low,medium,high'],
  ]);
  assert.ok(catalog.ms >= 0);
  const frames = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line));
  const stdin = frames.filter(frame => frame.stdin).map(frame => JSON.parse(frame.stdin));
  assert.deepEqual(stdin.map(frame => frame.type), ['control_request']);
  assert.equal(stdin[0].request.subtype, 'initialize');
  const argv: string[] = frames.find(frame => frame.argv?.includes('stream-json')).argv;
  assert.ok(argv.includes('--strict-mcp-config'));
  assert.ok(argv.includes('--no-session-persistence'));
  assert.ok(argv.some(arg => arg.includes('disableAllHooks')));

  const signedOut = await claudeCatalog({ query, path: FAKE_CLAUDE, env: fakeEnv({ FAKE_CLAUDE_SCENARIO: 'signed-out' }), cwd: dir });
  assert.equal(signedOut.state, 'signed_out');
  assert.deepEqual(signedOut.models, []);
  const broken = await claudeCatalog({ query, path: '/nonexistent/claude', env: fakeEnv(), cwd: dir });
  assert.equal(broken.state, 'unavailable');
  assert.doesNotMatch(broken.note ?? '', /nonexistent/);
});
