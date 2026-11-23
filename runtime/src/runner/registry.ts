import { constants } from 'node:fs';
import { lstat, mkdir, open, readdir, rename, unlink } from 'node:fs/promises';
import type { FileHandle } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';

export type Manifest = { run_id: string; thread_id: string; request_id: string; checkout: string; harness: string; build_id: string; cleanup_token?: string; lease?: { root: string; identity: { dev: string; ino: string } } };
export type RunnerEntry = { manifest: Manifest; status: 'live' | 'finished' | 'dead'; receipt?: unknown };
function id(value: string): string {
  if (!/^[A-Za-z0-9_-]{1,32}$/.test(value)) throw new Error('invalid runner id');
  return value;
}
function manifest(value: unknown): Manifest {
  if (!value || typeof value !== 'object') throw new Error('invalid runner manifest');
  const v = value as Record<string, unknown>;
  for (const key of ['run_id', 'thread_id', 'request_id', 'checkout', 'harness', 'build_id']) if (typeof v[key] !== 'string' || !v[key]) throw new Error('invalid runner manifest');
  id(v.run_id as string);
  return value as Manifest;
}
export function helper(fd: number, operation: string): Promise<boolean> {
  if (!process.env.PLAIT_HOST_EXE) throw new Error('PLAIT_HOST_EXE is required');
  return new Promise((resolve, reject) => {
    const child = spawn(process.env.PLAIT_HOST_EXE!, [operation], { stdio: ['ignore', 'ignore', 'pipe', fd] });
    let error = ''; child.stderr!.on('data', chunk => { error += String(chunk); });
    child.once('error', reject);
    child.once('close', code => { if (code === 0) resolve(true); else if (operation === 'runner-lock-fd' && error.includes('runner lock busy')) resolve(false); else reject(new Error(error || `${operation} failed`)); });
  });
}
export class RunnerRegistry {
  readonly directory: string;
  /** Sockets live in a separate short folder: macOS limits Unix socket paths to 104 bytes. */
  readonly sockets: string;
  constructor(store: string) { this.directory = join(store, 'runners'); this.sockets = join(store, 'r'); }
  async prepare(): Promise<void> {
    for (const directory of [this.directory, this.sockets]) {
      await mkdir(directory, { mode: 0o700 }).catch((error: NodeJS.ErrnoException) => { if (error.code !== 'EEXIST') throw error; });
      const stat = await lstat(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== process.getuid?.() || (stat.mode & 0o077) !== 0) throw new Error('invalid runners directory');
    }
  }
  path(runId: string, suffix: string): string { return join(this.directory, `${id(runId)}${suffix}`); }
  socketPath(runId: string): string { return join(this.sockets, id(runId)); }
  private async lockFile(runId: string): Promise<FileHandle> {
    const file = await open(this.path(runId, '.lock'), constants.O_RDWR | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
    try { const stat = await file.stat(); if (!stat.isFile() || stat.nlink !== 1 || stat.uid !== process.getuid?.() || (stat.mode & 0o077) !== 0) throw new Error('invalid runner lock'); return file; } catch (error) { await file.close(); throw error; }
  }
  async acquire(runId: string): Promise<FileHandle> {
    await this.prepare(); const file = await this.lockFile(runId);
    try { if (!await helper(file.fd, 'runner-lock-fd')) throw new Error('runner lock busy'); return file; } catch (error) { await file.close(); throw error; }
  }
  private async write(runId: string, suffix: string, value: unknown): Promise<void> {
    await this.prepare(); const temp = this.path(runId, `.${randomUUID()}.tmp`);
    const file = await open(temp, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    try { await file.writeFile(JSON.stringify(value)); await file.sync(); await file.close(); await rename(temp, this.path(runId, suffix)); } catch (error) { await file.close().catch(() => {}); await unlink(temp).catch(() => {}); throw error; }
  }
  async writeManifest(value: Manifest): Promise<void> { manifest(value); await this.write(value.run_id, '.json', value); }
  async writeReceipt(runId: string, value: unknown): Promise<void> { await this.write(runId, '.done.json', value); }
  /** Remove runner files, unlinking the manifest first so scan() never sees a half-removed entry. */
  async remove(runId: string): Promise<void> {
    const gone = (error: NodeJS.ErrnoException) => { if (error.code !== 'ENOENT') throw error; };
    await unlink(this.path(runId, '.json')).catch(gone);
    await unlink(this.path(runId, '.done.json')).catch(gone);
    await unlink(this.socketPath(runId)).catch(gone);
    await unlink(this.path(runId, '.lock')).catch(gone);
    await unlink(this.path(runId, '.log')).catch(gone);
  }
  async scan(): Promise<RunnerEntry[]> {
    await this.prepare(); const entries: RunnerEntry[] = [];
    for (const name of await readdir(this.directory)) {
      if (!name.endsWith('.json') || name.endsWith('.done.json')) continue;
      const runId = id(name.slice(0, -5));
      const file = await open(this.path(runId, '.json'), constants.O_RDONLY | constants.O_NOFOLLOW);
      let value: Manifest; try { value = manifest(JSON.parse(await file.readFile('utf8'))); } finally { await file.close(); }
      if (value.run_id !== runId) throw new Error('runner manifest id mismatch');
      const lock = await this.lockFile(runId);
      let unlocked: boolean; try { unlocked = await helper(lock.fd, 'runner-lock-fd'); } finally { await lock.close(); }
      if (!unlocked) { entries.push({ manifest: value, status: 'live' }); continue; }
      try { const receiptFile = await open(this.path(runId, '.done.json'), constants.O_RDONLY | constants.O_NOFOLLOW); let receipt: unknown; try { receipt = JSON.parse(await receiptFile.readFile('utf8')); } finally { await receiptFile.close(); } entries.push({ manifest: value, status: 'finished', receipt }); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; entries.push({ manifest: value, status: 'dead' }); }
    }
    return entries;
  }
}
