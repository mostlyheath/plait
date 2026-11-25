import type { Dirent } from 'node:fs';
import { readdir, realpath, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join } from 'node:path';
import { fail } from './protocol.ts';

const LIMIT = 1000;
const CONCURRENCY = 32;
const GIT_TIME_MS = 2000;

function request(input: unknown): { path?: string; hidden?: boolean } {
  if (!input || typeof input !== 'object' || Array.isArray(input)) fail('invalid_request', 'Invalid folder request.');
  const row = input as Record<string, unknown>;
  if (row.op !== 'browse_folder' || Object.keys(row).some(key => !['op', 'path', 'hidden'].includes(key))) fail('invalid_request', 'Unknown operation or field.');
  if (row.path !== undefined && (typeof row.path !== 'string' || !isAbsolute(row.path) || Buffer.byteLength(row.path) > 4096 || /[\u0000-\u001f\u007f]/u.test(row.path))) fail('invalid_request', 'Invalid path.');
  if (row.hidden !== undefined && typeof row.hidden !== 'boolean') fail('invalid_request', 'Invalid hidden.');
  return row as { path?: string; hidden?: boolean };
}

function folderError(error: unknown): never {
  const code = (error as NodeJS.ErrnoException)?.code;
  if (code === 'ENOENT') fail('not_found', 'That folder does not exist.');
  if (code === 'ENOTDIR') fail('invalid_request', 'That is not a folder.');
  fail('permission_denied', 'Plait cannot read that folder.');
}

async function hasGit(folder: string, deadline: number): Promise<boolean> {
  if (Date.now() >= deadline) return false;
  try {
    const entry = await stat(join(folder, '.git'));
    return entry.isDirectory() || entry.isFile();
  } catch { return false; }
}

export class Folders {
  private home: string;
  constructor(home = homedir()) { this.home = home; }
  handles(input: unknown) { return !!input && typeof input === 'object' && (input as { op?: unknown }).op === 'browse_folder'; }

  async handle(input: unknown) {
    const options = request(input);
    const deadline = Date.now() + GIT_TIME_MS;
    let path: string;
    let children: Dirent[];
    try {
      path = await realpath(options.path ?? this.home);
      if (!(await stat(path)).isDirectory()) fail('invalid_request', 'That is not a folder.');
      children = await readdir(path, { withFileTypes: true });
    } catch (error) {
      // Preserve our explicit file error while hiding operating system paths.
      if ((error as { code?: string }).code === 'invalid_request') throw error;
      return folderError(error);
    }
    const candidates = children.filter(child => options.hidden || !child.name.startsWith('.'))
      .sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()) || a.name.localeCompare(b.name));
    const entries: { name: string; git: boolean }[] = [];
    let truncated = false;
    const git = await hasGit(path, deadline);
    for (let offset = 0; offset < candidates.length && !truncated; offset += CONCURRENCY) {
      const batch = candidates.slice(offset, offset + CONCURRENCY);
      const rows = await Promise.all(batch.map(async child => {
        const childPath = join(path, child.name);
        let directory = child.isDirectory();
        if (child.isSymbolicLink()) {
          try { directory = (await stat(childPath)).isDirectory(); } catch { return null; }
        }
        if (!directory) return null;
        return { name: child.name, git: await hasGit(childPath, deadline) };
      }));
      for (const row of rows) if (row) {
        if (entries.length === LIMIT) { truncated = true; break; }
        entries.push(row);
      }
    }
    const parent = dirname(path);
    return { path, parent: parent === path ? null : parent, home: this.home, git, entries, truncated };
  }
}
