import { fixtureDirectory } from './support/temp.ts';
import { setup } from './support/safety.ts';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { chmod, lstat, mkdir, readdir, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import {
  BuildCacheManager,
  type BuildCacheStatus,
  type ClearBuildCachesReply,
  MAX_BUILD_CACHE_BYTES,
  computeBuildCacheCap,
  prepareBuildCache,
} from '../src/build-cache.ts';
import type { RunnerSession } from '../src/runner/session.ts';
import { Runtime } from '../src/runtime.ts';

async function exists(path: string): Promise<boolean> {
  return lstat(path).then(() => true, () => false);
}

async function privateStore(prefix: string): Promise<string> {
  const dir = await realpath(await fixtureDirectory(join(tmpdir(), prefix)));
  await chmod(dir, 0o700);
  return dir;
}

async function seedCache(store: string, thread: string, bytes: number, usedAt: number): Promise<string> {
  const { target } = await prepareBuildCache({ store, thread }, usedAt);
  await writeFile(join(target, 'artifact.bin'), Buffer.alloc(bytes, 0x61));
  return join(store, 'build-cache', thread);
}

test('computeBuildCacheCap picks the smaller of the fixed cap and 10% of free space', () => {
  assert.equal(computeBuildCacheCap(500 * 1_000_000_000), MAX_BUILD_CACHE_BYTES);
  assert.equal(computeBuildCacheCap(100 * 1_000_000_000), 10 * 1_000_000_000);
  assert.equal(computeBuildCacheCap(5_000, 2_000, 0.1), 500);
  assert.equal(computeBuildCacheCap(50_000, 2_000, 0.1), 2_000);
  assert.equal(computeBuildCacheCap(0), 0);
});

test('eviction removes oldest-used idle caches first, updates order on touch, and respects the cap', async t => {
  const store = await privateStore('plait-bc-lru-');
  t.after(async () => { await rm(store, { recursive: true, force: true }); });

  const a = randomUUID();
  const b = randomUUID();
  const c = randomUUID();
  const pathA = await seedCache(store, a, 600, 1_000_000);
  const pathB = await seedCache(store, b, 600, 2_000_000);
  const pathC = await seedCache(store, c, 600, 3_000_000);

  // Free space is 13,000 -> 10% is 1,300, smaller than maxBytes (5,000). Total is 1,800.
  let freeBytes = 13_000;
  const manager = new BuildCacheManager(store, { isActive: () => false }, {
    maxBytes: 5_000,
    freeSpaceBytes: async () => freeBytes,
  });
  t.after(async () => { await manager.close(); });

  // Touching `a` makes it newest (4_000_000), so `b` (2_000_000) is now the oldest.
  await manager.touch(a, 4_000_000);
  const first = await manager.enforceCap();
  assert.deepEqual(first.evicted, [b]);
  assert.equal(first.capBytes, 1_300);
  assert.equal(first.remainingBytes, 1_200);
  assert.equal(await exists(pathB), false);
  assert.equal(await exists(pathA), true);
  assert.equal(await exists(pathC), true);

  // Now switch to maxBytes-limited cap: freeBytes is huge, maxBytes = 700 -> evicts `c` (older than `a`).
  freeBytes = 10_000_000;
  const tight = new BuildCacheManager(store, { isActive: () => false }, {
    maxBytes: 700,
    freeSpaceBytes: async () => freeBytes,
  });
  t.after(async () => { await tight.close(); });
  const second = await tight.enforceCap();
  assert.deepEqual(second.evicted, [c]);
  assert.equal(second.capBytes, 700);
  assert.equal(second.remainingBytes, 600);
  assert.equal(await exists(pathC), false);
  assert.equal(await exists(pathA), true);
});

test('active sessions and running commands are never evicted or cleared, and command end triggers cap check', async t => {
  const store = await privateStore('plait-bc-active-');
  t.after(async () => { await rm(store, { recursive: true, force: true }); });

  const activeSessionThread = randomUUID();
  const activeCommandThread = randomUUID();
  const idleThread = randomUUID();

  const sessionPath = await seedCache(store, activeSessionThread, 800, 1_000_000);
  const commandPath = await seedCache(store, activeCommandThread, 800, 2_000_000);
  const idlePath = await seedCache(store, idleThread, 800, 3_000_000);

  const activeSessions = new Set<string>([activeSessionThread]);
  const manager = new BuildCacheManager(store, { isActive: id => activeSessions.has(id) }, {
    maxBytes: 1_000,
    freeSpaceBytes: async () => 100_000,
  });
  t.after(async () => { await manager.close(); });

  const handle = manager.forThread(activeCommandThread);
  handle.onCommandStart!();

  // Even though activeSessionThread and activeCommandThread are older and exceed the 1,000-byte cap,
  // only idleThread may be evicted.
  const evicted = await manager.enforceCap();
  assert.deepEqual(evicted.evicted, [idleThread]);
  assert.equal(await exists(idlePath), false);
  assert.equal(await exists(sessionPath), true);
  assert.equal(await exists(commandPath), true);

  // clearIdle also protects both active threads.
  const statusBefore = await manager.handle({ op: 'build_cache_status' });
  assert.deepEqual(statusBefore, {
    schema: 1,
    total_bytes: 1_600,
    idle_bytes: 0,
    thread_count: 2,
    active_count: 2,
    cap_bytes: 1_000,
    measuring: false,
  });
  const clearedWhileActive = await manager.handle({ op: 'clear_build_caches' });
  assert.deepEqual(clearedWhileActive, {
    schema: 1,
    cleared_threads: 0,
    cleared_bytes: 0,
    skipped_active: 2,
    total_bytes: 1_600,
    idle_bytes: 0,
    thread_count: 2,
    active_count: 2,
    cap_bytes: 1_000,
    measuring: false,
  });

  // Ending the command schedules a background cap check that evicts activeCommandThread now that it is idle,
  // while activeSessionThread remains protected.
  handle.onCommandEnd!();
  await manager.settled();
  assert.equal(await exists(commandPath), false);
  assert.equal(await exists(sessionPath), true);

  // Releasing the session lets clear_build_caches clear the remaining thread off the request path.
  activeSessions.delete(activeSessionThread);
  const clearedAfter = await manager.handle({ op: 'clear_build_caches' });
  assert.deepEqual(clearedAfter, {
    schema: 1,
    cleared_threads: 1,
    cleared_bytes: 800,
    skipped_active: 0,
    total_bytes: 0,
    idle_bytes: 0,
    thread_count: 0,
    active_count: 0,
    cap_bytes: 1_000,
    measuring: false,
  });
  await manager.settled();
  assert.equal(await exists(sessionPath), false);
});

test('symlinks inside caches are never followed and read-only cache directories are removed', async t => {
  const base = await privateStore('plait-bc-safe-');
  const store = join(base, 'store');
  const outside = join(base, 'outside');
  await mkdir(store, { mode: 0o700 });
  await chmod(store, 0o700);
  await mkdir(outside, { mode: 0o700 });
  t.after(async () => { await rm(base, { recursive: true, force: true }); });

  await writeFile(join(outside, 'registry.bin'), Buffer.alloc(10_000, 0x7a));
  const thread = randomUUID();
  const { cargo, target } = await prepareBuildCache({ store, thread }, 1_000_000);
  await symlink(outside, join(cargo, 'registry'));
  const lockedDir = join(target, 'locked');
  await mkdir(lockedDir, { mode: 0o700 });
  await writeFile(join(lockedDir, 'obj.o'), Buffer.alloc(400, 0x62));
  await chmod(lockedDir, 0o500);

  const manager = new BuildCacheManager(store, { isActive: () => false }, {
    maxBytes: 100_000,
    freeSpaceBytes: async () => 10_000_000,
  });
  t.after(async () => { await manager.close(); });

  const initial = await manager.status();
  assert.equal(initial.measuring, true);
  assert.equal(initial.total_bytes, 0);
  await manager.settled();

  const status = await manager.status();
  assert.equal(status.measuring, false);
  assert.equal(status.total_bytes, 400, 'symlinked registry must not be counted');

  const cleared = await manager.clearIdle();
  assert.equal(cleared.cleared_threads, 1);
  assert.equal(cleared.cleared_bytes, 400);
  await manager.settled();
  assert.equal(await exists(join(store, 'build-cache', thread)), false);
  assert.equal((await readFile(join(outside, 'registry.bin'))).length, 10_000, 'symlink target must stay intact');
});

test('runtime removes caches on archive, defers while active, recreates on unarchive, and sweeps at startup', async t => {
  const { root } = await setup(t);
  const directory = await privateStore('plait-bc-runtime-');
  const leases = await privateStore('plait-bc-leases-');
  const previousLeaseDir = process.env.PLAIT_LEASE_DIR;
  process.env.PLAIT_LEASE_DIR = leases;
  t.after(async () => {
    if (previousLeaseDir === undefined) delete process.env.PLAIT_LEASE_DIR;
    else process.env.PLAIT_LEASE_DIR = previousLeaseDir;
    await rm(directory, { recursive: true, force: true });
    await rm(leases, { recursive: true, force: true });
  });

  const models = createModels();
  const faux = fauxProvider({ tokensPerSecond: 1e8 });
  models.setProvider(faux.provider);
  const runtime = new Runtime(directory, models, new Set(['faux']));
  t.after(async () => { await runtime.close(); });

  const create = async (requestId: string) =>
    (await runtime.handle({ op: 'create_thread', protocol_version: 6, request_id: requestId, checkout: root, provider: 'faux', model_id: 'faux-1' }) as { thread: { id: string } }).thread.id;

  // 1. Archiving an idle thread removes its build cache in the background; unarchiving recreates on demand.
  const idleId = await create('create-idle');
  const idleCachePath = await seedCache(directory, idleId, 500, 1_000_000);
  assert.equal(await exists(idleCachePath), true);
  await runtime.handle({ op: 'archive_thread', protocol_version: 3, thread_id: idleId, archived: true });
  await runtime.buildCaches.settled();
  assert.equal(await exists(idleCachePath), false);

  await runtime.handle({ op: 'archive_thread', protocol_version: 3, thread_id: idleId, archived: false });
  const recreated = await prepareBuildCache({ store: directory, thread: idleId });
  assert.equal(await exists(recreated.target), true);

  // 2. Archiving a thread with an active run keeps its cache until the run finishes, then removes it.
  let release!: () => void;
  let started = false;
  const gate = new Promise<void>(resolve => { release = resolve; });
  faux.setResponses([async () => { started = true; await gate; return fauxAssistantMessage('done'); }]);

  const activeId = await create('create-active');
  const activeCachePath = await seedCache(directory, activeId, 700, 2_000_000);
  const until = async (done: () => boolean) => {
    const deadline = Date.now() + 30_000;
    while (!done()) {
      assert.ok(Date.now() < deadline);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  try {
    await runtime.handle({ op: 'submit', protocol_version: 6, thread_id: activeId, request_id: 'run-active', text: 'hello', approval: 'auto' });
    await until(() => started);
    await runtime.handle({ op: 'archive_thread', protocol_version: 3, thread_id: activeId, archived: true });
    await runtime.buildCaches.settled();
    assert.equal(await exists(activeCachePath), true, 'active thread cache must not be removed during run');
    release();
    await until(() => !runtime.sessions.has(activeId));
    await runtime.buildCaches.settled();
    assert.equal(await exists(activeCachePath), false, 'archived thread cache must be removed once run finishes');
  } finally {
    release();
  }

  // 3. Additive host operations validate fields and report/clear idle caches.
  assert.equal((await runtime.reply({ op: 'build_cache_status', extra: 1 })).error?.code, 'invalid_request');
  assert.equal((await runtime.reply({ op: 'clear_build_caches', protocol_version: 7 })).error?.code, 'invalid_request');
  const status = await runtime.handle({ op: 'build_cache_status' }) as { schema: number; thread_count: number };
  assert.equal(status.schema, 1);
  assert.equal(status.thread_count, 1);

  // 4. Startup recovery removes leftover archived caches, sweeps .removing-* dirs, and enforces the cap.
  const leftoverArchived = await create('create-archived-startup');
  await runtime.handle({ op: 'archive_thread', protocol_version: 3, thread_id: leftoverArchived, archived: true });
  await runtime.buildCaches.settled();
  const leftoverPath = await seedCache(directory, leftoverArchived, 300, 1_500_000);
  const strayRemoving = join(directory, 'build-cache', `.removing-${randomUUID()}-deadbeef`);
  await mkdir(strayRemoving, { mode: 0o700 });
  await writeFile(join(strayRemoving, 'junk'), 'junk');

  await runtime.recover();
  await runtime.buildCaches.settled();
  assert.equal(await exists(leftoverPath), false);
  assert.equal(await exists(strayRemoving), false);
});

test('Runtime.close stops queued enforceCap before detaching runnerSessions and refuses deletes once closing', async t => {
  const directory = await privateStore('plait-bc-close-');
  const leases = await privateStore('plait-bc-close-leases-');
  const previousLeaseDir = process.env.PLAIT_LEASE_DIR;
  process.env.PLAIT_LEASE_DIR = leases;
  t.after(async () => {
    if (previousLeaseDir === undefined) delete process.env.PLAIT_LEASE_DIR;
    else process.env.PLAIT_LEASE_DIR = previousLeaseDir;
    await rm(directory, { recursive: true, force: true });
    await rm(leases, { recursive: true, force: true });
  });

  const models = createModels();
  const faux = fauxProvider({ tokensPerSecond: 1e8 });
  models.setProvider(faux.provider);

  // 1. An in-flight enforceCap finishes before runnerSessions are detached/cleared, so the runner cache survives.
  const runtime = new Runtime(directory, models, new Set(['faux']));
  const runnerThread = randomUUID();
  const cachePath = await seedCache(directory, runnerThread, 900, 1_000_000);
  let detached = false;
  let detachedDuringCap: boolean | null = null;
  let runnerPresentDuringCap: boolean | null = null;
  let capStarted = false;
  let releaseCap!: () => void;
  const capGate = new Promise<void>(resolve => { releaseCap = resolve; });
  runtime.runnerSessions.set(runnerThread, { detach() { detached = true; } } as unknown as RunnerSession);
  runtime.buildCaches.capBytes = async () => {
    capStarted = true;
    await capGate;
    detachedDuringCap = detached;
    runnerPresentDuringCap = runtime.runnerSessions.has(runnerThread);
    return 0;
  };

  try {
    runtime.buildCaches.scheduleEnforceCap();
    const deadline = Date.now() + 10_000;
    while (!capStarted) {
      assert.ok(Date.now() < deadline, 'enforceCap did not start');
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    const closing = runtime.close();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(detached, false, 'runnerSessions must not detach while buildCaches.close awaits queued work');
    assert.equal(runtime.runnerSessions.has(runnerThread), true);
    releaseCap();
    await closing;
    assert.equal(detachedDuringCap, false);
    assert.equal(runnerPresentDuringCap, true);
    assert.equal(detached, true);
    assert.equal(runtime.runnerSessions.size, 0);
    assert.equal(await exists(cachePath), true, 'runner thread cache must not be evicted during close');
  } finally {
    releaseCap();
  }

  // 2. A queued (not yet started) enforceCap during Runtime.close also preserves the runner thread cache.
  const runtime2 = new Runtime(directory, models, new Set(['faux']));
  const runnerThread2 = randomUUID();
  const cachePath2 = await seedCache(directory, runnerThread2, 900, 1_000_000);
  runtime2.runnerSessions.set(runnerThread2, { detach() {} } as unknown as RunnerSession);
  runtime2.buildCaches.capBytes = async () => 0;
  runtime2.buildCaches.scheduleEnforceCap();
  await runtime2.close();
  assert.equal(await exists(cachePath2), true, 'queued enforceCap during close must not remove runner cache');

  // 3. Once closing is set, park/delete helpers refuse to remove even idle caches or stray .removing-* trees.
  const idleThread = randomUUID();
  const idlePath = await seedCache(directory, idleThread, 500, 1_000_000);
  const strayRemoving = join(directory, 'build-cache', `.removing-${randomUUID()}-cafe`);
  await mkdir(strayRemoving, { mode: 0o700 });
  await writeFile(join(strayRemoving, 'keep'), 'keep');

  const manager = new BuildCacheManager(directory, { isActive: () => false }, {
    maxBytes: 0,
    freeSpaceBytes: async () => 0,
  });
  await manager.close();
  assert.equal(await manager.removeThread(idleThread), false);
  assert.deepEqual((await manager.enforceCap()).evicted, []);
  assert.equal((await manager.clearIdle()).cleared_threads, 0);
  await manager.startupCleanup();
  assert.equal(await exists(idlePath), true, 'closing manager must refuse to park/remove idle caches');
  assert.equal(await exists(strayRemoving), true, 'closing manager must refuse to delete parked trees');
});

test('build_cache_status and clear_build_caches return without walking cache trees', async t => {
  const store = await privateStore('plait-bc-nowalk-');
  t.after(async () => { await rm(store, { recursive: true, force: true }); });

  const a = randomUUID();
  const b = randomUUID();
  const pathA = await seedCache(store, a, 600, 1_000_000);
  const pathB = await seedCache(store, b, 400, 2_000_000);

  let walkCount = 0;
  let walkEntered = false;
  let releaseWalk!: () => void;
  let walkGate = new Promise<void>(resolve => { releaseWalk = resolve; });

  const manager = new BuildCacheManager(store, { isActive: () => false }, {
    maxBytes: 100_000,
    freeSpaceBytes: async () => 10_000_000,
    measureDirectory: async path => {
      walkCount++;
      walkEntered = true;
      await walkGate;
      return (await lstat(join(path, 'target', 'artifact.bin'))).size;
    },
  });
  t.after(async () => {
    releaseWalk();
    await manager.close();
  });

  try {
    // Unmeasured threads: status returns immediately with measuring=true and no walk on the request path.
    const initial = await manager.handle({ op: 'build_cache_status' }) as BuildCacheStatus;
    assert.equal(walkCount, 0);
    assert.equal(initial.measuring, true);
    assert.equal(initial.thread_count, 2);
    assert.equal(initial.total_bytes, 0);

    // Wait until background sizing is actively in progress and blocked on walkGate; status still answers at once.
    const deadline = Date.now() + 10_000;
    while (!walkEntered) {
      assert.ok(Date.now() < deadline, 'background sizing did not start');
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    const during = await manager.handle({ op: 'build_cache_status' }) as BuildCacheStatus;
    assert.equal(during.measuring, true);
    assert.equal(during.thread_count, 2);

    // Finish background sizing; stored totals are now populated.
    releaseWalk();
    await manager.settled();
    assert.equal(walkCount, 2);

    // Block walkGate again: neither status nor clear may walk.
    walkCount = 0;
    walkGate = new Promise<void>(resolve => { releaseWalk = resolve; });

    const ready = await manager.handle({ op: 'build_cache_status' }) as BuildCacheStatus;
    assert.equal(walkCount, 0);
    assert.equal(ready.measuring, false);
    assert.equal(ready.total_bytes, 1_000);
    assert.equal(ready.idle_bytes, 1_000);
    assert.equal(ready.thread_count, 2);
    assert.equal(ready.active_count, 0);

    const cleared = await manager.handle({ op: 'clear_build_caches' }) as ClearBuildCachesReply;
    assert.equal(walkCount, 0);
    assert.equal(cleared.measuring, false);
    assert.equal(cleared.cleared_threads, 2);
    assert.equal(cleared.cleared_bytes, 1_000);
    assert.equal(cleared.total_bytes, 0);
    assert.equal(cleared.idle_bytes, 0);
    assert.equal(cleared.thread_count, 0);
    // Idle directories were renamed aside synchronously before background deletion.
    assert.equal(await exists(pathA), false);
    assert.equal(await exists(pathB), false);

    await manager.settled();
    assert.equal(walkCount, 0);
    assert.deepEqual(await readdir(join(store, 'build-cache')), []);

    // Even when a cache has never been measured, clear renames it aside without walking.
    const c = randomUUID();
    const pathC = await seedCache(store, c, 500, 3_000_000);
    const clearedUnmeasured = await manager.handle({ op: 'clear_build_caches' }) as ClearBuildCachesReply;
    assert.equal(walkCount, 0);
    assert.equal(clearedUnmeasured.cleared_threads, 1);
    assert.equal(clearedUnmeasured.cleared_bytes, 0);
    assert.equal(clearedUnmeasured.measuring, false);
    assert.equal(await exists(pathC), false);
    await manager.settled();
    assert.equal(walkCount, 0);
  } finally {
    releaseWalk();
  }
});

test('thread cache used as an in-progress seed clone source is treated as active and not evicted', async t => {
  const { root } = await setup(t);
  const directory = await privateStore('plait-bc-seed-');
  const leases = await privateStore('plait-bc-seed-leases-');
  const previousLeaseDir = process.env.PLAIT_LEASE_DIR;
  process.env.PLAIT_LEASE_DIR = leases;
  t.after(async () => {
    if (previousLeaseDir === undefined) delete process.env.PLAIT_LEASE_DIR;
    else process.env.PLAIT_LEASE_DIR = previousLeaseDir;
    await rm(directory, { recursive: true, force: true });
    await rm(leases, { recursive: true, force: true });
  });

  const models = createModels();
  const faux = fauxProvider({ tokensPerSecond: 1e8 });
  models.setProvider(faux.provider);
  const runtime = new Runtime(directory, models, new Set(['faux']));
  t.after(async () => { await runtime.close(); });

  const seedThread = randomUUID();
  const otherThread = randomUUID();
  // seedThread is older, so LRU would evict it first unless it is protected as active.
  const seedPath = await seedCache(directory, seedThread, 800, 1_000_000);
  const otherPath = await seedCache(directory, otherThread, 800, 2_000_000);
  runtime.buildCaches.capBytes = async () => 500;

  let seedStarted = false;
  let observedSource: string | null = null;
  let releaseSeed!: () => void;
  const seedGate = new Promise<void>(resolve => { releaseSeed = resolve; });
  runtime.worktrees.builds.invokeHelper = async <T>(input: unknown): Promise<T> => {
    const req = input as { action?: string; source_cache?: string | null };
    if (req.action === 'seed') {
      seedStarted = true;
      observedSource = req.source_cache ?? null;
      await seedGate;
      return {
        outcome: { warm: true, reason: '', directories: [], counts: { files: 0, links: 0, skipped: 0, bytes: 0, elapsed_ms: 0, notes: [] } },
        checked_at: Math.floor(Date.now() / 1000),
      } as T;
    }
    throw new Error(`unexpected helper action: ${String(req.action)}`);
  };

  try {
    runtime.worktrees.builds.scheduleSeed(root, seedThread, true);
    assert.equal(runtime.buildCaches.isActive(seedThread), true, 'scheduled seed source must immediately be active');

    const deadline = Date.now() + 10_000;
    while (!seedStarted) {
      assert.ok(Date.now() < deadline, 'seed helper did not start');
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    assert.equal(observedSource, seedThread);
    assert.equal(runtime.buildCaches.isActive(seedThread), true);

    const during = await runtime.buildCaches.enforceCap();
    assert.deepEqual(during.evicted, [otherThread]);
    assert.equal(await exists(seedPath), true, 'seed source cache must not be evicted mid-clone');
    assert.equal(await exists(otherPath), false);

    const clearedDuring = await runtime.handle({ op: 'clear_build_caches' }) as ClearBuildCachesReply;
    assert.equal(clearedDuring.cleared_threads, 0);
    assert.equal(clearedDuring.skipped_active, 1);
    assert.equal(await exists(seedPath), true);

    releaseSeed();
    await runtime.worktrees.builds.settled();
    assert.equal(runtime.buildCaches.isActive(seedThread), false);

    const after = await runtime.buildCaches.enforceCap();
    assert.deepEqual(after.evicted, [seedThread]);
    assert.equal(await exists(seedPath), false);
  } finally {
    releaseSeed();
  }
});
