import { constants } from 'node:fs';
import { access, mkdir, mkdtemp, rename, rm, stat, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { body, matchesIntegrity } from './download.ts';
import { readTgz } from './tar.ts';

type Harness = 'codex' | 'claude';
/** The whole download, body included: each package is about 170 MB. */
const DOWNLOAD_TIMEOUT_MS = 15 * 60_000;
export type Build = { url: string; integrity: string; prefix: string; executable: string; version: string };
const codex = (platform: string, hash: string, triple: string): Build => ({
  url: `https://registry.npmjs.org/@openai/codex/-/codex-0.169.0-${platform}.tgz`, integrity: `sha512-${hash}`,
  prefix: `package/vendor/${triple}/`, executable: 'bin/codex', version: '0.169.0',
});
const claude = (platform: string, hash: string): Build => ({
  url: `https://registry.npmjs.org/@anthropic-ai/claude-code-${platform}/-/claude-code-${platform}-2.1.326.tgz`, integrity: `sha512-${hash}`,
  prefix: 'package/', executable: 'claude', version: '2.1.326',
});
const builds: Record<Harness, Record<string, Build>> = {
  codex: {
    'linux-x64': codex('linux-x64', 'o05NyAOCDygSPRo1yinzxvkRSdmsZyBD9E7mbpmjRanSfM8lypqWtQgEo4XfNkwhuCR2aFu8mlGOsQGFXVbVww==', 'x86_64-unknown-linux-musl'),
    'linux-arm64': codex('linux-arm64', 'NZ/DaQUpcQmxmqee7Z9uC1rzw3GhSbooawcaTe1LN5Yk+WlinUKGdTXJgClRT/9ogQEEM9TWxYzvxPfj5G/xFg==', 'aarch64-unknown-linux-musl'),
    'darwin-arm64': codex('darwin-arm64', '5WsWFBbVP78D+6FvNo1MMVxLPZdmcoQlqVJFrF4qx/TmvneLnMQKtLyXkUo54XOTDfoYKpNouTJ/F7gTVA8FnQ==', 'aarch64-apple-darwin'),
  },
  claude: {
    'linux-x64': claude('linux-x64', 'RvNuaeMGfPGUyUGttVvhHgVUZ3B2L+Pgf336ImZLsPP2UQHYblewi47CPFLObqz+K6D++PLP1eAsDBLYko3w+g=='),
    'linux-arm64': claude('linux-arm64', 'lZCCWXarB/1IeHkotI937eEdN4BCbVdWDdmejtlkbUO9M29rHjITtXW9BWCDFlxvo0GXsFogN0SFIJWgMelKLw=='),
    'linux-x64-musl': claude('linux-x64-musl', 'Gnd/9lghe4q2CX9mzPgz8db9GngP6KHx/q1V6wepDM2CG1pQSwgrvzrs8M5Z8KmbSrun1Fgd1d6InedeZeMZ/w=='),
    'linux-arm64-musl': claude('linux-arm64-musl', 'IB793mtG7aA1m4WxwQabigUD4sHDEcXu9lRltZITLQCPzBdGFlWINxdSCnk9TVWAv0Bb5eyirxyPI9WMER0wiA=='),
    'darwin-arm64': claude('darwin-arm64', 'u6Gk/xJdrSVbnJ4LsObftgY8/oAWv0YNkWmIirPtLrf80ocIve1VeJeoQTLJ5IkgQOSEIHMTt5V5McY/25cdPg=='),
  },
};
type Platform = { platform?: string; arch?: string; musl?: boolean };
function selected(harness: Harness, options: Platform = {}): Build | undefined {
  const platform = options.platform ?? process.platform;
  const arch = options.arch ?? process.arch;
  const musl = options.musl ?? (platform === 'linux' && !(process.report.getReport() as { header: { glibcVersionRuntime?: string } }).header.glibcVersionRuntime);
  return builds[harness][`${platform}-${arch}${harness === 'claude' && platform === 'linux' && musl ? '-musl' : ''}`];
}
export function installable(harness: Harness, options: Platform = {}): string | null { return selected(harness, options)?.version ?? null; }
export async function managedPath(harness: Harness, directory: string, options: Platform = {}): Promise<string | undefined> {
  const build = selected(harness, options);
  if (!build) return undefined;
  const path = join(directory, harness, build.version, build.executable);
  return await executable(path) ? path : undefined;
}
async function executable(path: string): Promise<boolean> {
  try { if (!(await stat(path)).isFile()) return false; await access(path, constants.X_OK); return true; } catch { return false; }
}
type InstallOptions = Platform & { directory: string; fetch?: typeof fetch };
export async function installTool(harness: Harness, options: InstallOptions): Promise<void> {
  const build = selected(harness, options);
  if (!build) throw new Error(`No ${harness} build for ${options.platform ?? process.platform}-${options.arch ?? process.arch}.`);
  await installBuild(harness, build, options);
}

/** Downloads, checks, and unpacks one build; exported so tests can pin a fixture. */
export async function installBuild(harness: Harness, build: Build, options: InstallOptions): Promise<void> {
  const present = join(options.directory, harness, build.version, build.executable);
  if (await executable(present)) return;
  const response = await (options.fetch ?? fetch)(build.url, { signal: AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS), redirect: 'follow' });
  if (!response.ok) throw new Error(`The registry answered ${response.status}.`);
  const archive = await body(response, 400 * 1024 * 1024);
  if (!matchesIntegrity(archive, build.integrity)) throw new Error('The download does not match the pinned hash.');
  const files = await readTgz(archive, { maxArchive: 800 * 1024 * 1024, maxEntry: 400 * 1024 * 1024, maxEntries: 10000 });
  const kept = files.filter(file => file.path.startsWith(build.prefix) && (harness === 'codex' || file.path === 'package/claude'));
  if (!kept.some(file => file.path === build.prefix + build.executable && (file.mode & 0o111))) throw new Error('The package has no executable.');
  const target = join(options.directory, harness, build.version);
  await mkdir(dirname(target), { recursive: true, mode: 0o700 });
  const staging = await mkdtemp(join(dirname(target), `.${build.version}-`));
  try {
    for (const file of kept) {
      const path = join(staging, file.path.slice(build.prefix.length));
      await mkdir(dirname(path), { recursive: true, mode: 0o700 });
      await writeFile(path, file.data, { mode: file.mode & 0o777, flag: 'wx' });
    }
    await rename(staging, target);
  } catch (error) {
    // Another install that finished first is as good as this one.
    if (!await executable(present)) throw error;
  } finally { await rm(staging, { recursive: true, force: true }); }
}
