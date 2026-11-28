import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { mkdtempSync, readdirSync, realpathSync, rmSync, statSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const options = process.argv.slice(2);
if (options.some(option => option !== '--require-command-sandbox')) {
  console.error('Usage: node test/run.mjs [--require-command-sandbox]');
  process.exit(1);
}
const platform = { darwin: 'macos', linux: 'linux' }[process.platform];
if (!platform) {
  console.error('Runtime development tests support macOS and Linux only.');
  process.exit(1);
}
if (options.includes('--require-command-sandbox') && platform !== 'macos') {
  console.error('Full command-sandbox acceptance is blocked: Linux containment is not enabled. No tests ran.');
  process.exit(1);
}
const directory = fileURLToPath(new URL('.', import.meta.url));
const tests = readdirSync(directory).filter(name => name.endsWith('.test.ts')
  && (!/\.(macos|linux)\.test\.ts$/.test(name) || name.endsWith(`.${platform}.test.ts`)))
  .sort().map(name => fileURLToPath(new URL(name, import.meta.url)));
console.log(platform === 'macos'
  ? 'Runtime checks: common daemon tests and real macOS command-sandbox tests.'
  : 'Runtime checks: common daemon tests and Linux trusted execution tests; sandboxed execution is disabled.');
// An outer integration test must not make Node silently skip this independent suite.
const environment = { ...process.env };
delete environment.NODE_TEST_CONTEXT;
const progress = environment.PLAIT_ACTIVE_TESTS ? ['--import', './test/support/active-test.mjs'] : [];
// The parent can still clean up if a worker crashes before Node runs its hooks.
const suiteStarted = Date.now();
const scratch = mkdtempSync(join(platform === 'macos' ? '/tmp' : tmpdir(), 'plait-t-'));
try {
  // Checkout identities require canonical paths even when the system temp directory is a symlink.
  environment.TMPDIR = realpathSync(scratch);
  const result = spawnSync(process.execPath, ['--test', '--test-concurrency=2', ...progress, ...tests], {
    stdio: 'inherit', cwd: fileURLToPath(new URL('..', import.meta.url)), env: environment,
  });
  if (result.error) console.error(result.error.message);
  process.exitCode = result.status ?? 1;
  // Tests that kill a process mid-command leave its macOS command scratch in /private/tmp,
  // as a crashed host would; the host sweeps those on start. Every test process has
  // ended here, so remove the ones this run created.
  if (platform === 'macos') for (const name of readdirSync('/private/tmp')) {
    if (!name.startsWith('plait-command-')) continue;
    const path = join('/private/tmp', name);
    const info = statSync(path, { throwIfNoEntry: false });
    if (info?.isDirectory() && info.uid === process.getuid() && info.birthtimeMs >= suiteStarted)
      rmSync(path, { recursive: true, force: true, maxRetries: 10 });
  }
  const leftovers = readdirSync(scratch).filter(name => name.startsWith('plait-'));
  if (leftovers.length) {
    console.error('Runtime tests left temporary entries:', leftovers.join(', '));
    process.exitCode = 1;
  }
} finally {
  rmSync(scratch, { recursive: true, force: true, maxRetries: 10 });
}
