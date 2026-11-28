import { randomBytes } from 'node:crypto';
import { constants, type Dirent } from 'node:fs';
import { chmod, lstat, mkdir, open, readdir, realpath, rename, rm, statfs } from 'node:fs/promises';
import { join } from 'node:path';
import { fail } from './protocol.ts';

export const MAX_BUILD_CACHE_BYTES = 20 * 1_000_000_000;
export const FREE_SPACE_FRACTION = 0.1;

const THREAD_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const REMOVING_PREFIX = '.removing-';
const REMOVING_NAME = /^\.removing-[0-9a-f-]+$/;
const LAST_USED_FILE = '.last-used';
const STAT_CONCURRENCY = 32;

export type BuildCache = {
  store: string;
  thread: string;
  onCommandStart?: () => void;
  onCommandEnd?: () => void;
};

export type BuildCacheStatus = {
  schema: 1;
  total_bytes: number;
  idle_bytes: number;
  thread_count: number;
  active_count: number;
  cap_bytes: number;
  measuring: boolean;
};

export type ClearBuildCachesReply = BuildCacheStatus & {
  cleared_threads: number;
  cleared_bytes: number;
  skipped_active: number;
};

export type BuildCacheHooks = {
  isActive: (threadId: string) => boolean;
  isArchived?: (threadId: string) => boolean;
};

export type BuildCacheOptions = {
  maxBytes?: number;
  freeSpaceFraction?: number;
  freeSpaceBytes?: () => Promise<number>;
  now?: () => number;
  measureDirectory?: (path: string) => Promise<number>;
};

type CacheEntry = {
  thread: string;
  path: string;
  bytes: number;
  lastUsedMs: number;
};

async function privateDirectory(path: string, create: boolean) {
  if (create) {
    try { await mkdir(path, { mode: 0o700 }); }
    catch (error) { if ((error as { code?: string }).code !== 'EEXIST') throw error; }
  }
  const info = await lstat(path);
  if (!info.isDirectory() || info.uid !== process.getuid!() || (info.mode & 0o077) !== 0 || await realpath(path) !== path) {
    fail('permission_denied', 'Build cache must be canonical, private, owned, and free of symlink traversal.');
  }
}

async function touchLastUsed(root: string, now: number) {
  const marker = join(root, LAST_USED_FILE);
  const handle = await open(marker, constants.O_CREAT | constants.O_WRONLY | constants.O_NOFOLLOW, 0o600);
  try {
    const info = await handle.stat();
    if (!info.isFile() || info.uid !== process.getuid!() || (info.mode & 0o077) !== 0) {
      fail('permission_denied', 'Build cache timestamp must be a private regular file.');
    }
    const at = new Date(now);
    await handle.utimes(at, at);
  } finally {
    await handle.close();
  }
}

export async function prepareBuildCache(cache: BuildCache, now = Date.now()) {
  if (!THREAD_ID.test(cache.thread)) {
    fail('permission_denied', 'Invalid build cache thread identity.');
  }
  await privateDirectory(cache.store, false);
  const parent = join(cache.store, 'build-cache');
  await privateDirectory(parent, true);
  const root = join(parent, cache.thread);
  await privateDirectory(root, true);
  const cargo = join(root, 'cargo'); const target = join(root, 'target');
  // The sandbox cannot rename these roots; host setup never writes below target.
  await privateDirectory(cargo, true); await privateDirectory(target, true);
  await touchLastUsed(root, now);
  return { cargo, target };
}

export function computeBuildCacheCap(
  freeBytes: number,
  maxBytes = MAX_BUILD_CACHE_BYTES,
  fraction = FREE_SPACE_FRACTION,
): number {
  const safeMax = Number.isFinite(maxBytes) && maxBytes > 0 ? Math.floor(maxBytes) : 0;
  const safeFree = Number.isFinite(freeBytes) && freeBytes > 0 ? freeBytes : 0;
  return Math.min(safeMax, Math.floor(safeFree * fraction));
}

export async function storeFreeBytes(store: string): Promise<number> {
  const stats = await statfs(store);
  return Math.max(0, Number(stats.bavail) * Number(stats.bsize));
}

async function lastUsedMs(root: string): Promise<number> {
  const marker = await lstat(join(root, LAST_USED_FILE)).catch(() => undefined);
  if (marker?.isFile() && !marker.isSymbolicLink()) return marker.mtimeMs;
  let latest = 0;
  for (const candidate of [root, join(root, 'target'), join(root, 'cargo')]) {
    const info = await lstat(candidate).catch(() => undefined);
    if (info && !info.isSymbolicLink()) latest = Math.max(latest, info.mtimeMs);
  }
  return latest;
}

/** Measures logical bytes inside one cache without following symlinks (such as cargo/registry). */
async function directoryBytes(root: string, shouldStop?: () => boolean): Promise<number> {
  let total = 0;
  const seen = new Set<string>();
  const stack = [root];
  while (stack.length) {
    if (shouldStop?.()) return total;
    const dir = stack.pop()!;
    let entries: Dirent[];
    try { entries = await readdir(dir, { withFileTypes: true }); }
    catch { continue; }
    const files: string[] = [];
    for (const entry of entries) {
      if (entry.isSymbolicLink()) continue;
      const full = join(dir, entry.name);
      if (entry.isDirectory()) stack.push(full);
      else files.push(full);
    }
    for (let offset = 0; offset < files.length; offset += STAT_CONCURRENCY) {
      if (shouldStop?.()) return total;
      const infos = await Promise.all(files.slice(offset, offset + STAT_CONCURRENCY).map(path =>
        lstat(path).then(info => ({ path, info }), () => undefined)));
      for (const item of infos) {
        if (!item || item.info.isSymbolicLink()) continue;
        if (item.info.isDirectory()) { stack.push(item.path); continue; }
        if (!item.info.isFile()) continue;
        if (item.info.nlink > 1) {
          const key = `${item.info.dev}:${item.info.ino}`;
          if (seen.has(key)) continue;
          seen.add(key);
        }
        total += item.info.size;
      }
    }
  }
  return total;
}

/** Restores owner access on directories a command left read-only, never traversing symlinks. */
async function makeWritable(path: string, shouldStop?: () => boolean): Promise<void> {
  if (shouldStop?.()) return;
  const info = await lstat(path).catch(() => undefined);
  if (!info || info.isSymbolicLink() || !info.isDirectory() || shouldStop?.()) return;
  await chmod(path, 0o700).catch(() => {});
  for (const name of await readdir(path).catch(() => [] as string[])) {
    if (shouldStop?.()) return;
    await makeWritable(join(path, name), shouldStop);
  }
}

async function removeTree(path: string, shouldStop?: () => boolean): Promise<void> {
  if (shouldStop?.()) return;
  try { await rm(path, { recursive: true, force: true, maxRetries: 3 }); return; }
  catch { /* retry after restoring owner permissions */ }
  if (shouldStop?.()) return;
  await makeWritable(path, shouldStop);
  if (shouldStop?.()) return;
  await rm(path, { recursive: true, force: true, maxRetries: 3 });
}

function validateOperation(input: unknown): 'build_cache_status' | 'clear_build_caches' {
  if (!input || typeof input !== 'object' || Array.isArray(input)) {
    fail('invalid_request', 'Invalid build cache request.');
  }
  const row = input as Record<string, unknown>;
  if ((row.op !== 'build_cache_status' && row.op !== 'clear_build_caches') || Object.keys(row).some(key => key !== 'op')) {
    fail('invalid_request', 'Unknown operation or field.');
  }
  return row.op;
}

export class BuildCacheManager {
  private readonly store: string;
  private readonly hooks: BuildCacheHooks;
  private readonly maxBytes: number;
  private readonly freeSpaceFraction: number;
  private readonly freeSpaceBytes: () => Promise<number>;
  private readonly now: () => number;
  private readonly measureDirectory: (path: string) => Promise<number>;
  private readonly activeCommands = new Map<string, number>();
  private readonly usedThreads = new Set<string>();
  private readonly sizes = new Map<string, number>();
  private readonly dirtyThreads = new Set<string>();
  private readonly threadVersions = new Map<string, number>();
  private nextVersion = 0;
  private queue: Promise<void> = Promise.resolve();
  private sizingDepth = 0;
  private capPending = false;
  private measurePending = false;
  private startupPending = false;
  private closing = false;

  constructor(store: string, hooks: BuildCacheHooks, options: BuildCacheOptions = {}) {
    this.store = store;
    this.hooks = hooks;
    this.maxBytes = options.maxBytes ?? MAX_BUILD_CACHE_BYTES;
    this.freeSpaceFraction = options.freeSpaceFraction ?? FREE_SPACE_FRACTION;
    this.freeSpaceBytes = options.freeSpaceBytes ?? (() => storeFreeBytes(this.store));
    this.now = options.now ?? Date.now;
    this.measureDirectory = options.measureDirectory ?? (path => directoryBytes(path, () => this.closing));
  }

  handles(input: unknown): boolean {
    if (!input || typeof input !== 'object' || Array.isArray(input)) return false;
    const op = (input as { op?: unknown }).op;
    return op === 'build_cache_status' || op === 'clear_build_caches';
  }

  async handle(input: unknown): Promise<BuildCacheStatus | ClearBuildCachesReply> {
    const op = validateOperation(input);
    return op === 'build_cache_status' ? this.status() : this.clearIdle();
  }

  isActive(threadId: string): boolean {
    return (this.activeCommands.get(threadId) ?? 0) > 0 || this.hooks.isActive(threadId);
  }

  forThread(threadId: string): BuildCache {
    return {
      store: this.store,
      thread: threadId,
      onCommandStart: () => this.commandStarted(threadId),
      onCommandEnd: () => this.commandEnded(threadId),
    };
  }

  private versionOf(threadId: string): number {
    return this.threadVersions.get(threadId) ?? 0;
  }

  private bumpVersion(threadId: string): void {
    this.threadVersions.set(threadId, ++this.nextVersion);
  }

  commandStarted(threadId: string): void {
    this.activeCommands.set(threadId, (this.activeCommands.get(threadId) ?? 0) + 1);
    this.usedThreads.add(threadId);
    this.dirtyThreads.add(threadId);
    this.bumpVersion(threadId);
  }

  commandEnded(threadId: string): void {
    const remaining = (this.activeCommands.get(threadId) ?? 0) - 1;
    if (remaining > 0) this.activeCommands.set(threadId, remaining);
    else this.activeCommands.delete(threadId);
    this.dirtyThreads.add(threadId);
    this.bumpVersion(threadId);
    this.scheduleEnforceCap();
  }

  consumeUsed(threadId: string): boolean {
    return this.usedThreads.delete(threadId);
  }

  async capBytes(): Promise<number> {
    try {
      const free = await this.freeSpaceBytes();
      return computeBuildCacheCap(free, this.maxBytes, this.freeSpaceFraction);
    } catch {
      return this.maxBytes;
    }
  }

  private async existingParent(): Promise<string | null> {
    const parent = join(this.store, 'build-cache');
    try {
      await privateDirectory(this.store, false);
      await privateDirectory(parent, false);
      return parent;
    } catch (error) {
      if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') return null;
      throw error;
    }
  }

  /** Lists top-level thread cache folders without descending into any cache tree. */
  private async listThreads(): Promise<{ parent: string | null; threads: string[] }> {
    const parent = await this.existingParent();
    if (!parent) return { parent: null, threads: [] };
    const names = await readdir(parent).catch(() => [] as string[]);
    const uid = process.getuid!();
    const threads: string[] = [];
    for (const name of names) {
      if (!THREAD_ID.test(name)) continue;
      const info = await lstat(join(parent, name)).catch(() => undefined);
      if (!info || info.isSymbolicLink() || !info.isDirectory() || info.uid !== uid) continue;
      threads.push(name);
    }
    return { parent, threads };
  }

  private pruneSizes(threads: string[]): void {
    const present = new Set(threads);
    for (const known of this.sizes.keys()) {
      if (!present.has(known)) this.sizes.delete(known);
    }
    for (const dirty of this.dirtyThreads) {
      if (!present.has(dirty)) this.dirtyThreads.delete(dirty);
    }
  }

  private hasUnmeasured(threads: string[]): boolean {
    return threads.some(thread => !this.sizes.has(thread));
  }

  private isMeasuring(threads: string[]): boolean {
    return this.sizingDepth > 0
      || this.measurePending
      || this.capPending
      || this.startupPending
      || this.hasUnmeasured(threads);
  }

  private async withSizing<T>(fn: () => Promise<T>): Promise<T> {
    this.sizingDepth++;
    try { return await fn(); }
    finally { this.sizingDepth--; }
  }

  private async measureThreads(forceAll: boolean): Promise<{ parent: string | null; threads: string[] }> {
    if (this.closing) return { parent: null, threads: [] };
    return this.withSizing(async () => {
      const { parent, threads } = await this.listThreads();
      if (this.closing) return { parent, threads };
      this.pruneSizes(threads);
      if (!parent) return { parent: null, threads: [] };
      for (const thread of threads) {
        if (this.closing) break;
        if (!forceAll && this.sizes.has(thread) && !this.dirtyThreads.has(thread)) continue;
        const version = this.versionOf(thread);
        this.dirtyThreads.delete(thread);
        const bytes = await this.measureDirectory(join(parent, thread)).catch(() => undefined);
        if (this.closing) break;
        if (bytes !== undefined && this.versionOf(thread) === version) {
          this.sizes.set(thread, bytes);
        }
      }
      return { parent, threads };
    });
  }

  private async removeTree(path: string): Promise<void> {
    if (this.closing) return;
    await removeTree(path, () => this.closing);
  }

  /** Atomically moves an idle thread cache aside so deletion never races a fresh command. */
  private async parkForRemoval(parent: string, threadId: string): Promise<string | null> {
    if (this.closing || !THREAD_ID.test(threadId) || this.isActive(threadId)) return null;
    const source = join(parent, threadId);
    const info = await lstat(source).catch(() => undefined);
    if (!info || info.isSymbolicLink() || !info.isDirectory() || info.uid !== process.getuid!()) return null;
    // Re-check synchronously right before renaming in case a run started or shutdown began during lstat.
    if (this.closing || this.isActive(threadId)) return null;
    const parked = join(parent, `${REMOVING_PREFIX}${threadId}-${randomBytes(6).toString('hex')}`);
    try {
      await rename(source, parked);
      this.bumpVersion(threadId);
      this.sizes.delete(threadId);
      this.dirtyThreads.delete(threadId);
      return parked;
    } catch (error) {
      if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') return null;
      throw error;
    }
  }

  private async sweepRemoving(): Promise<void> {
    if (this.closing) return;
    const parent = await this.existingParent().catch(() => null);
    if (!parent || this.closing) return;
    const uid = process.getuid!();
    const names = await readdir(parent).catch(() => [] as string[]);
    for (const name of names) {
      if (this.closing) return;
      if (!REMOVING_NAME.test(name)) continue;
      const path = join(parent, name);
      const info = await lstat(path).catch(() => undefined);
      if (this.closing) return;
      if (!info || info.isSymbolicLink() || !info.isDirectory() || info.uid !== uid) continue;
      await this.removeTree(path).catch(() => {});
    }
  }

  async status(): Promise<BuildCacheStatus> {
    const [{ threads }, cap] = await Promise.all([this.listThreads(), this.capBytes()]);
    this.pruneSizes(threads);
    let totalBytes = 0;
    let idleBytes = 0;
    let activeCount = 0;
    let hasUnmeasured = false;
    for (const thread of threads) {
      const measured = this.sizes.get(thread);
      if (measured === undefined) hasUnmeasured = true;
      const bytes = measured ?? 0;
      totalBytes += bytes;
      if (this.isActive(thread)) activeCount++;
      else idleBytes += bytes;
    }
    if (hasUnmeasured && !this.closing) this.scheduleMeasure();
    return {
      schema: 1,
      total_bytes: totalBytes,
      idle_bytes: idleBytes,
      thread_count: threads.length,
      active_count: activeCount,
      cap_bytes: cap,
      measuring: this.isMeasuring(threads),
    };
  }

  async clearIdle(): Promise<ClearBuildCachesReply> {
    const [{ parent, threads }, cap] = await Promise.all([this.listThreads(), this.capBytes()]);
    this.pruneSizes(threads);
    let clearedThreads = 0;
    let clearedBytes = 0;
    let skippedActive = 0;
    let remainingBytes = 0;
    let remainingIdleBytes = 0;
    let remainingThreads = 0;
    let activeCount = 0;
    let parkedAny = false;
    const remainingList: string[] = [];
    for (const thread of threads) {
      const bytes = this.sizes.get(thread) ?? 0;
      if (!parent || this.isActive(thread)) {
        skippedActive++;
        activeCount++;
        remainingBytes += bytes;
        remainingThreads++;
        remainingList.push(thread);
        continue;
      }
      const parked = await this.parkForRemoval(parent, thread).catch(() => null);
      if (parked) {
        clearedThreads++;
        clearedBytes += bytes;
        parkedAny = true;
      } else if (this.isActive(thread)) {
        skippedActive++;
        activeCount++;
        remainingBytes += bytes;
        remainingThreads++;
        remainingList.push(thread);
      } else if (this.closing) {
        remainingBytes += bytes;
        remainingIdleBytes += bytes;
        remainingThreads++;
        remainingList.push(thread);
      }
    }
    if (parkedAny) this.enqueue(() => this.sweepRemoving());
    if (this.hasUnmeasured(remainingList) && !this.closing) this.scheduleMeasure();
    return {
      schema: 1,
      cleared_threads: clearedThreads,
      cleared_bytes: clearedBytes,
      skipped_active: skippedActive,
      total_bytes: remainingBytes,
      idle_bytes: remainingIdleBytes,
      thread_count: remainingThreads,
      active_count: activeCount,
      cap_bytes: cap,
      measuring: this.isMeasuring(remainingList),
    };
  }

  async removeThread(threadId: string): Promise<boolean> {
    if (this.closing || !THREAD_ID.test(threadId) || this.isActive(threadId)) return false;
    const parent = await this.existingParent().catch(() => null);
    if (!parent || this.closing) return false;
    const parked = await this.parkForRemoval(parent, threadId).catch(() => null);
    if (!parked) return false;
    await this.removeTree(parked).catch(() => {});
    return true;
  }

  async enforceCap(forceMeasure = false): Promise<{ evicted: string[]; remainingBytes: number; capBytes: number }> {
    if (this.closing) return { evicted: [], remainingBytes: 0, capBytes: this.maxBytes };
    return this.withSizing(async () => {
      const [{ parent, threads }, cap] = await Promise.all([this.measureThreads(forceMeasure), this.capBytes()]);
      if (this.closing || !parent) {
        const remaining = threads.reduce((sum, thread) => sum + (this.sizes.get(thread) ?? 0), 0);
        return { evicted: [], remainingBytes: remaining, capBytes: cap };
      }
      const entries: CacheEntry[] = [];
      for (const thread of threads) {
        if (this.closing) break;
        const path = join(parent, thread);
        const bytes = this.sizes.get(thread) ?? 0;
        const used = await lastUsedMs(path);
        entries.push({ thread, path, bytes, lastUsedMs: used });
      }
      let remaining = entries.reduce((sum, entry) => sum + entry.bytes, 0);
      const evicted: string[] = [];
      if (this.closing || remaining <= cap) return { evicted, remainingBytes: remaining, capBytes: cap };
      const ordered = [...entries].sort((a, b) => a.lastUsedMs - b.lastUsedMs || a.thread.localeCompare(b.thread));
      for (const entry of ordered) {
        if (this.closing || remaining <= cap) break;
        if (this.isActive(entry.thread)) continue;
        const parked = await this.parkForRemoval(parent, entry.thread).catch(() => null);
        if (!parked) continue;
        remaining -= entry.bytes;
        evicted.push(entry.thread);
        await this.removeTree(parked).catch(() => {});
      }
      return { evicted, remainingBytes: remaining, capBytes: cap };
    });
  }

  async startupCleanup(): Promise<void> {
    if (this.closing) return;
    await this.withSizing(async () => {
      await this.sweepRemoving();
      if (this.closing) return;
      const parent = await this.existingParent().catch(() => null);
      if (parent && this.hooks.isArchived) {
        const names = await readdir(parent).catch(() => [] as string[]);
        for (const name of names) {
          if (this.closing) return;
          if (!THREAD_ID.test(name)) continue;
          if (this.hooks.isArchived(name) && !this.isActive(name)) {
            await this.removeThread(name);
          }
        }
      }
      if (!this.closing) await this.enforceCap(true);
    });
  }

  private enqueue(work: () => Promise<void>): void {
    if (this.closing) return;
    this.queue = this.queue.then(() => new Promise<void>(resolve => {
      if (this.closing) { resolve(); return; }
      setImmediate(() => {
        if (this.closing) { resolve(); return; }
        void work().catch(() => {}).finally(resolve);
      });
    }));
  }

  scheduleRemove(threadId: string): void {
    if (!THREAD_ID.test(threadId) || this.closing) return;
    this.enqueue(async () => { await this.removeThread(threadId); });
  }

  scheduleMeasure(): void {
    if (this.closing || this.measurePending) return;
    this.measurePending = true;
    this.enqueue(async () => {
      this.measurePending = false;
      await this.measureThreads(false);
    });
  }

  scheduleEnforceCap(threadId?: string): void {
    if (threadId && THREAD_ID.test(threadId)) {
      this.dirtyThreads.add(threadId);
      this.bumpVersion(threadId);
    }
    if (this.closing || this.capPending) return;
    this.capPending = true;
    this.enqueue(async () => {
      this.capPending = false;
      await this.enforceCap();
    });
  }

  scheduleStartupCleanup(): void {
    if (this.closing || this.startupPending) return;
    this.startupPending = true;
    this.enqueue(async () => {
      this.startupPending = false;
      await this.startupCleanup();
    });
  }

  /** Test and shutdown hook: waits for queued background cache work to finish. */
  async settled(): Promise<void> {
    await this.queue;
  }

  async close(): Promise<void> {
    this.closing = true;
    this.capPending = false;
    this.measurePending = false;
    this.startupPending = false;
    await this.queue;
  }

  /** Exposed for deterministic tests that advance last-used timestamps without waiting. */
  async touch(threadId: string, now = this.now()): Promise<void> {
    await prepareBuildCache({ store: this.store, thread: threadId }, now);
  }
}
