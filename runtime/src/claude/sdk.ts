import { body, matchesIntegrity } from '../download.ts';
export { matchesIntegrity } from '../download.ts';
// Anthropic's Agent SDK, which Claude Code threads run through. Plait's
// releases do not include it, since its license reserves all rights. When the
// person sets up Claude Code threads, the host downloads exactly the pinned
// version from the npm registry, checks it against the pinned hash before
// writing anything, and keeps only the module it loads and the package's
// license and readme, in its own data directory. Nothing else reaches the
// network, and nothing in the runtime imports the package statically.
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { query as Query } from '@anthropic-ai/claude-agent-sdk';
import { readTgz } from '../tar.ts';
import { CLAUDE_SDK_VERSION } from './install.ts';

export type QueryFunction = typeof Query;
export type ClaudeSdk = { query: QueryFunction };

/** The registry tarball and its `integrity`, as package-lock.json records them for the dev copy. */
export const CLAUDE_SDK_TARBALL = `https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-${CLAUDE_SDK_VERSION}.tgz`;
export const CLAUDE_SDK_INTEGRITY = 'sha512-BUTSky/4YWfL/MxC+4kV0NDbJFjbJPoQMFEk/s9G6hV9LUfYPMbsWz94bFQ7ZIo2ubj2xwJ2c2NVxkNlQhVw+Q==';
/** The SHA-256 of `sdk.mjs` in that tarball, checked again each time the host loads it. */
export const CLAUDE_SDK_MODULE_SHA256 = 'f926275ddfdc1ab55581232fbadd12e9837eef77f7642077104609c9f5d2d256';

/** `sdk.mjs` imports only Node's built-in modules and runs the person's own `claude`, so it is all that runs. */
const KEPT = ['sdk.mjs', 'package.json', 'LICENSE.md', 'README.md'];
const REQUIRED = ['sdk.mjs', 'LICENSE.md'];
/** The tarball is about 1.6 MB, and 6 MB unpacked; these bounds leave room without trusting the server. */
const MAX_DOWNLOAD = 16 * 1024 * 1024;
const LIMITS = { maxArchive: 64 * 1024 * 1024, maxEntry: 16 * 1024 * 1024, maxEntries: 512 };
// The host relays each request within 20 seconds and replaces a runtime that misses it, and the
// desktop waits as long; the download must end, or give up, well inside that.
const DOWNLOAD_TIMEOUT_MS = 15_000;

/** Where the host keeps this version, beside `host.sqlite`. */
export function sdkDirectory(dataDirectory: string): string {
  return join(dataDirectory, 'harnesses', 'claude-agent-sdk', CLAUDE_SDK_VERSION);
}

/** Whether a set-up copy is present. Its contents are checked when it loads. */
export async function sdkPresent(directory: string): Promise<boolean> {
  try { return (await stat(join(directory, 'sdk.mjs'))).isFile(); } catch { return false; }
}

export type SetUpOptions = {
  directory: string;
  fetch?: typeof fetch;
  /** Tests substitute their own archive and its hash; the host always uses the pinned ones. */
  url?: string;
  integrity?: string;
};
export type SetUpResult = { downloaded: boolean };

/**
 * The files to keep from a verified tarball, by name. npm packs everything
 * under `package/`; anything unsafe anywhere in the archive refuses all of it.
 */
export async function sdkFiles(archive: Buffer): Promise<Map<string, Buffer>> {
  const kept = new Map<string, Buffer>();
  for (const file of await readTgz(archive, LIMITS)) {
    const name = file.path.startsWith('package/') ? file.path.slice('package/'.length) : undefined;
    if (name && KEPT.includes(name)) kept.set(name, file.data);
  }
  for (const name of REQUIRED) if (!kept.has(name)) throw new Error(`The package has no ${name}.`);
  return kept;
}

/**
 * Downloads, checks, and unpacks the pinned SDK into `directory`, unless it is
 * already there. The files are written into a fresh folder beside it and moved
 * into place in one rename, so a failure leaves nothing a load would use.
 */
export async function setUpSdk(options: SetUpOptions): Promise<SetUpResult> {
  const { directory } = options;
  if (await sdkPresent(directory)) {
    if (await moduleMatches(directory)) return { downloaded: false };
    // A damaged copy is moved aside in one rename, then replaced.
    const damaged = `${directory}.damaged-${process.pid}-${Date.now()}`;
    await rename(directory, damaged);
    await rm(damaged, { recursive: true, force: true });
  }
  const response = await (options.fetch ?? fetch)(options.url ?? CLAUDE_SDK_TARBALL, { signal: AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS), redirect: 'follow' });
  if (!response.ok) throw new Error(`The registry answered ${response.status}.`);
  const archive = await body(response, MAX_DOWNLOAD);
  if (!matchesIntegrity(archive, options.integrity ?? CLAUDE_SDK_INTEGRITY)) throw new Error('The download does not match the pinned hash.');
  const files = await sdkFiles(archive);
  const parent = dirname(directory);
  await mkdir(parent, { recursive: true, mode: 0o700 });
  const staging = await mkdtemp(join(parent, `.${CLAUDE_SDK_VERSION}-`));
  try {
    for (const [name, data] of files) await writeFile(join(staging, name), data, { mode: 0o600, flag: 'wx' });
    await rename(staging, directory);
  } catch (error) {
    await rm(staging, { recursive: true, force: true });
    // Another setup that finished first is as good as this one.
    if (await sdkPresent(directory)) return { downloaded: true };
    throw error;
  }
  return { downloaded: true };
}

async function moduleMatches(directory: string) {
  try { return createHash('sha256').update(await readFile(join(directory, 'sdk.mjs'))).digest('hex') === CLAUDE_SDK_MODULE_SHA256; } catch { return false; }
}

const loaded = new Map<string, Promise<ClaudeSdk>>();

/**
 * Loads the SDK from a set-up copy (or, in tests, the dev copy), after
 * checking `sdk.mjs` against the pinned hash. Loaded once per directory.
 */
export function loadSdk(directory: string): Promise<ClaudeSdk> {
  let module = loaded.get(directory);
  if (!module) {
    module = (async () => {
      const file = join(directory, 'sdk.mjs');
      if (!await moduleMatches(directory)) throw new Error('The Agent SDK on this Mac does not match the pinned version.');
      const sdk = await import(pathToFileURL(file).href) as Partial<ClaudeSdk>;
      if (typeof sdk.query !== 'function') throw new Error('The Agent SDK on this Mac has no query function.');
      return { query: sdk.query };
    })();
    loaded.set(directory, module);
    // A failed load is tried again next time; the file may have been repaired.
    module.catch(() => { if (loaded.get(directory) === module) loaded.delete(directory); });
  }
  return module;
}
