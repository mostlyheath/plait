import { mkdtemp, rm } from 'node:fs/promises';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { after } from 'node:test';

// Keep ownership through file teardown. A fixture can fail during setup, before
// its normal hook is installed, or a child can recreate files as it shuts down.
const directories: string[] = [];
after(async () => {
  const errors = [];
  for (const directory of directories.reverse()) {
    try { await rm(directory, { recursive: true, force: true, maxRetries: 10 }); }
    catch (error) { errors.push(error); }
  }
  if (errors.length) throw new AggregateError(errors, 'Could not remove test directories');
});

export async function fixtureDirectory(prefix: string) {
  const directory = await mkdtemp(prefix);
  directories.push(directory);
  return directory;
}

export function fixtureDirectorySync(prefix: string) {
  const directory = mkdtempSync(prefix);
  directories.push(directory);
  return directory;
}

// macOS tests exercise the /tmp alias and need a short Unix socket path.
const temporaryRoot = tmpdir();
const macAlias = temporaryRoot.replace(/^\/private\/tmp\//, '/tmp/');
export const socketTempRoot = process.platform === 'darwin'
  ? (macAlias.startsWith('/tmp/') ? macAlias : '/tmp')
  : temporaryRoot;
