// Host policy is private state; repository files may suggest but never approve output.
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { fail, HostError } from './protocol.ts';
import { rootIdentity, type FileIdentity } from './safety.ts';
import type { Store } from './store.ts';
import type { Managed } from './worktrees.ts';

export type Counts = { files: number; links: number; skipped: number; bytes: number; elapsed_ms: number; notes: string[] };
export type Outcome = { warm: boolean; reason: string; directories: string[]; counts: Counts };
export type BuildState = { phase: 'waiting' | 'cloning' | 'warm' | 'cold'; files: number; outcome: Outcome | null };
export type BuildInfo = { schema: 1; worktree_build_output_v1: true; policy: { schema: 1; project: string; identity: FileIdentity; revision: number; paths: string[] }; suggestions: string[]; seed: { bytes: number; files: number; verified_at: number } | null };
const emptyCounts = (): Counts => ({ files: 0, links: 0, skipped: 0, bytes: 0, elapsed_ms: 0, notes: [] });
const cold = (reason: string): Outcome => ({ warm: false, reason, directories: [], counts: emptyCounts() });

export function buildHelper<T>(input: unknown, signal?: AbortSignal, progress?: (counts: Counts) => void): Promise<T> {
  if (signal?.aborted) return Promise.reject(new HostError('cancelled', 'Build output cloning was skipped.'));
  const executable = process.env.PLAIT_HOST_EXE;
  if (!executable) fail('internal', 'Build output helper is not configured.');
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ['build-output'], { env: { ...process.env, PLAIT_BUILD_LIFETIME_FD: '4' }, stdio: ['pipe', 'pipe', 'pipe', process.env.PLAIT_LOCK_FD ? Number(process.env.PLAIT_LOCK_FD) : 'ignore', 'pipe'] });
    let pending = ''; let value: T | undefined; let error: string | undefined;
    const stop = () => { child.kill('SIGKILL'); };
    const timeout = setTimeout(stop, 150_000);
    signal?.addEventListener('abort', stop, { once: true });
    child.stdin!.on('error', () => {}); child.stdin!.end(JSON.stringify(input)); child.stderr!.resume();
    child.stdout!.setEncoding('utf8').on('data', (data: string) => {
      pending += data;
      if (Buffer.byteLength(pending) > 65536) { error = 'Build output reply exceeds its limit.'; stop(); return; }
      for (;;) {
        const end = pending.indexOf('\n'); if (end < 0) break;
        const line = pending.slice(0, end); pending = pending.slice(end + 1);
        try {
          const reply = JSON.parse(line) as { progress?: Counts; value?: T; error?: string };
          if (reply.progress) progress?.(reply.progress);
          if (reply.error) error = reply.error;
          if (reply.value !== undefined) value = reply.value;
        } catch { error = 'Invalid build output reply.'; stop(); }
      }
    });
    const cleanup = () => { clearTimeout(timeout); signal?.removeEventListener('abort', stop); child.stdio[4]?.destroy(); };
    child.once('error', error => { cleanup(); reject(error); });
    child.once('close', code => {
      cleanup();
      if (signal?.aborted) return reject(new HostError('cancelled', 'Build output cloning was skipped.'));
      if (error || code !== 0 || value === undefined) return reject(new HostError('build_output', error ?? 'Build output preparation was interrupted.'));
      resolve(value);
    });
  });
}
type Job = { abort: AbortController; done: Promise<unknown>; sourceCache?: string | null };
type SeedReply = { outcome: Outcome; checked_at: number };
type CloneReply = { outcome: Outcome; seedless: boolean };
/** The helper's SEED_FRESH: a seed that matches the approval and was taken or
 * found unchanged within the hour is not refreshed after a run. Dependency
 * output, the part worth cloning, changes with a lockfile, not every run. */
export const SEED_FRESH_MS = 60 * 60 * 1000;
export class BuildOutputs {
  private seeds = new Map<string, Job>();
  private clones = new Map<string, Job>();
  private pendingSeedSources = new Map<string, number>();
  private closing = false;
  private seedErrors = new Map<string, string>();
  /** When the helper last judged each checkout's seed, so a run end inside
   * the hour starts no process at all. Cleared when approvals change. */
  private checked = new Map<string, number>();
  /** Helper actions started, in order; tests count processes with it. */
  readonly started: string[] = [];
  /** Invoker for the build-output helper; overridable in deterministic tests. */
  invokeHelper: typeof buildHelper = buildHelper;
  private store: Store; private directory: string; private changed: () => void; private busy: (root: string) => boolean;
  constructor(store: Store, directory: string, changed: () => void, busy: (root: string) => boolean) {
    this.store = store; this.directory = directory; this.changed = changed; this.busy = busy;
  }
  private helper<T>(input: { action: string } & Record<string, unknown>, signal?: AbortSignal, progress?: (counts: Counts) => void) {
    // Bounded: a long-lived runtime starts helpers for days; tests read only the recent ones.
    if (this.started.length >= 256) this.started.shift();
    this.started.push(input.action); return this.invokeHelper<T>(input, signal, progress);
  }
  /** The Refresh control asks for a refresh now, whatever the seed's age. */
  async info(checkout: string, refresh = false): Promise<BuildInfo> {
    const info = await this.helper<BuildInfo>({ action: 'info', store: this.directory, root: checkout });
    if (refresh) this.scheduleSeed(info.policy.project, undefined, true);
    return info;
  }
  async set(checkout: string, identity: FileIdentity, revision: number, paths: string[]) {
    const info = await this.helper<BuildInfo>({ action: 'set', store: this.directory, root: checkout, identity, revision, paths });
    this.checked.clear(); this.scheduleSeed(info.policy.project, undefined, true);
    return info;
  }
  state(thread: string): BuildState {
    const value = this.store.setting(`build_output:${thread}`);
    return value ? JSON.parse(value) as BuildState : { phase: 'waiting', files: 0, outcome: null };
  }
  private put(thread: string, state: BuildState) { this.store.setSetting(`build_output:${thread}`, JSON.stringify(state)); this.changed(); }
  async warm(tree: Managed, privateTarget: boolean): Promise<Outcome> {
    const prior = this.state(tree.thread_id);
    if (prior.outcome) return prior.outcome;
    // An interrupted installation is rolled back, never silently resumed file by file.
    if (prior.phase === 'cloning') {
      const outcome = await this.helper<Outcome>({ action: 'recover', store: this.directory, worktree: tree });
      this.put(tree.thread_id, { phase: outcome.warm ? 'warm' : 'cold', files: outcome.counts.files, outcome }); return outcome;
    }
    const abort = new AbortController();
    const started = Date.now();
    if (this.closing) abort.abort();
    if (this.store.setting(`build_skip:${tree.thread_id}`) === 'yes') abort.abort();
    this.put(tree.thread_id, { phase: 'cloning', files: 0, outcome: null });
    const done = (async () => {
      let outcome: Outcome;
      try {
        // Seeds come only from the main checkout, even for a worktree made from another.
        const main = tree.parent.main;
        const clone = (retry: boolean) => {
          if (abort.signal.aborted) throw new HostError('cancelled', 'Build output cloning was skipped.');
          return this.helper<CloneReply>({ action: 'clone', store: this.directory, worktree: tree, private_target: privateTarget, retry_seedless: retry }, abort.signal,
            counts => this.put(tree.thread_id, { phase: 'cloning', files: counts.files, outcome: null }));
        };
        // Any published seed that matches the approval is used at once, however
        // old. Only with none at all does creation wait for a capture.
        const retry = !this.busy(main);
        let reply = await clone(retry);
        if (reply.seedless && retry) {
          if (!this.busy(main)) {
            const cancelSeed = () => this.seeds.get(main)?.abort.abort();
            abort.signal.addEventListener('abort', cancelSeed, { once: true });
            try { await this.refreshSeed(main, true); } finally { abort.signal.removeEventListener('abort', cancelSeed); }
          }
          // This attempt records its outcome, warm or cold.
          reply = await clone(false);
        }
        outcome = reply.outcome;
        const seedError = this.seedErrors.get(main);
        if (!outcome.warm && outcome.reason.includes('no idle build seed') && seedError) outcome = cold(seedError);
      } catch (error) {
        // Recovery finishes before admitting a first message, including after Skip.
        const recovered = await this.helper<Outcome>({ action: 'recover', store: this.directory, worktree: tree });
        outcome = recovered.warm ? recovered : cold(error instanceof Error ? error.message : 'Build output cloning failed.');
      }
      outcome.counts.elapsed_ms = Date.now() - started;
      this.put(tree.thread_id, { phase: outcome.warm ? 'warm' : 'cold', files: outcome.counts.files, outcome }); return outcome;
    })();
    this.clones.set(tree.thread_id, { abort, done });
    try { return await done; } finally { this.clones.delete(tree.thread_id); }
  }
  skip(thread: string) {
    this.store.setSetting(`build_skip:${thread}`, 'yes'); this.clones.get(thread)?.abort.abort();
    return this.state(thread);
  }
  /** True while a worktree creation is cloning build output into this thread's cache. */
  isWarming(thread: string): boolean { return this.clones.has(thread); }
  /** True while a scheduled or in-progress seed clone is reading from this thread's cache. */
  isSeedingFrom(thread: string): boolean {
    if ((this.pendingSeedSources.get(thread) ?? 0) > 0) return true;
    for (const job of this.seeds.values()) {
      if (job.sourceCache === thread) return true;
    }
    return false;
  }
  /** After a run or an admitted creation. Inside the hour this starts nothing;
   * after it the helper walks the source and clones only what changed. */
  scheduleSeed(root: string, sourceCache?: string, force = false) {
    if (sourceCache !== undefined) this.store.setSetting(`build_source:${root}`, sourceCache);
    const checked = this.checked.get(root);
    if (!force && checked !== undefined && checked <= Date.now() && Date.now() - checked < SEED_FRESH_MS) return;
    const source = this.store.setting(`build_source:${root}`) || null;
    if (source) this.pendingSeedSources.set(source, (this.pendingSeedSources.get(source) ?? 0) + 1);
    let released = false;
    const release = () => {
      if (released || !source) return;
      released = true;
      const remaining = (this.pendingSeedSources.get(source) ?? 0) - 1;
      if (remaining > 0) this.pendingSeedSources.set(source, remaining);
      else this.pendingSeedSources.delete(source);
    };
    // Queue draining reserves its run synchronously first. A user run always wins.
    setImmediate(() => {
      if (this.closing || this.busy(root)) { release(); return; }
      void this.refreshSeed(root, force, release);
    });
  }
  private async refreshSeed(root: string, force = false, onRegistered?: () => void): Promise<void> {
    try {
      if (this.closing || this.busy(root)) return;
      const previous = this.seeds.get(root);
      if (previous) {
        await previous.done;
        if (force) {
          const pass = onRegistered;
          onRegistered = undefined;
          await this.refreshSeed(root, true, pass);
        }
        return;
      }
      const sourceCache = this.store.setting(`build_source:${root}`) || null;
      const abort = new AbortController();
      const done = (async () => {
        const identity = await rootIdentity(root);
        if (abort.signal.aborted || this.busy(root)) return;
        const reply = await this.helper<SeedReply>({ action: 'seed', store: this.directory, root, identity, generation: randomUUID(), source_cache: sourceCache, force }, abort.signal);
        this.checked.set(root, reply.checked_at * 1000);
        this.seedErrors.delete(root);
      })().catch(error => {
        // A failure is retried after the same bound, not after every run; a run
        // that interrupted the refresh leaves the next run end free to retry.
        if (!abort.signal.aborted) this.checked.set(root, Date.now());
        this.seedErrors.set(root, error instanceof Error ? error.message : 'The build seed could not be prepared.');
      });
      this.seeds.set(root, { abort, done, sourceCache });
      onRegistered?.();
      onRegistered = undefined;
      try { await done; } finally { if (this.seeds.get(root)?.done === done) this.seeds.delete(root); }
    } finally {
      onRegistered?.();
    }
  }
  /** Stops the low-priority helper before a local run attempts its writer lease. */
  async yieldToRun(root: string) { const job = this.seeds.get(root); if (job) { job.abort.abort(); await job.done; } }
  /** Waits for scheduled and running refreshes without interrupting them. */
  async settled() { await new Promise(resolve => setImmediate(resolve)); await Promise.allSettled([...this.seeds.values()].map(job => job.done)); }
  async close() {
    this.closing = true;
    this.pendingSeedSources.clear();
    for (const job of [...this.seeds.values(), ...this.clones.values()]) job.abort.abort();
    await Promise.allSettled([...this.seeds.values(), ...this.clones.values()].map(job => job.done));
  }
}
