import { readFile } from 'node:fs/promises';
import { randomBytes, randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { releaseAfterHandoff, type CheckoutLease } from '../safety.ts';
import { RunnerRegistry, type RunnerEntry } from './registry.ts';
import { RunnerClient } from './socket.ts';
import { spawnRunner } from './spawn.ts';
import type { Command, Event } from './protocol.ts';

type Options = {
  store: string;
  onEvent: (event: Event) => void | Promise<void>;
  onDisconnect?: (entry: RunnerEntry | undefined) => void;
  onGap?: (floor: number, lastSeq: number) => void;
  entry?: string;
  heartbeatMs?: number;
  heartbeatTimeoutMs?: number;
  retryBaseMs?: number;
  retryMaxMs?: number;
  failedStopTimeoutMs?: number;
};
export class RunnerSupervisor {
  private readonly registry: RunnerRegistry;
  private readonly options: Options;
  private client?: RunnerClient;
  private runId?: string;
  private lastSeq = 0;
  private queue = Promise.resolve();
  private heartbeat?: NodeJS.Timeout;
  private deadline?: NodeJS.Timeout;
  private retryTimer?: NodeJS.Timeout;
  private retryInFlight = false;
  private backoffMs: number;
  private nonce?: string;
  private detached = false;
  private gapped = false;
  constructor(options: Options) {
    this.options = options;
    this.registry = new RunnerRegistry(options.store);
    this.backoffMs = options.retryBaseMs ?? 100;
  }
  async launch(args: { runId?: string; threadId: string; requestId: string; lease: CheckoutLease; payload: Record<string, unknown>; cleanupToken?: string; beforeStart?: () => void | Promise<void> }): Promise<{ runId: string; pid: number }> {
    // Short ids keep the runner socket path inside macOS's 104-byte Unix socket limit.
    const runId = args.runId ?? randomBytes(6).toString('hex'); this.runId = runId;
    let handedOff = false;
    try {
      await this.registry.writeManifest({ run_id: runId, thread_id: args.threadId, request_id: args.requestId, checkout: args.lease.root, harness: String(args.payload.kind ?? 'unknown'), build_id: 'runner-skeleton', ...(args.cleanupToken ? { cleanup_token: args.cleanupToken } : {}), lease: { root: args.lease.root, identity: args.lease.identity } });
      const pid = await spawnRunner(this.options.entry ?? fileURLToPath(new URL('./main.ts', import.meta.url)), args.lease, { PLAIT_RUNNER_STORE: this.options.store, PLAIT_RUNNER_ID: runId }, this.registry.path(runId, '.log'));
      const deadline = Date.now() + 5000;
      for (;;) {
        try { await this.attach(runId, 0, Math.max(1, deadline - Date.now())); break; }
        catch (error) { if (Date.now() >= deadline) throw await this.startFailure(runId, error); await new Promise(resolve => setTimeout(resolve, 25)); }
      }
      await releaseAfterHandoff(args.lease); handedOff = true;
      try {
        await args.beforeStart?.();
      } catch (error) {
        // Acceptance failed before `start` was sent: stop the unstarted runner and wait for lock release.
        try { this.stop(); } catch {}
        this.detach();
        const stopDeadline = Date.now() + (this.options.failedStopTimeoutMs ?? 5000);
        let lockFree = false;
        while (Date.now() < stopDeadline) {
          try {
            const entry = (await this.registry.scan()).find(item => item.manifest.run_id === runId);
            if (!entry || entry.status !== 'live') { lockFree = true; break; }
          } catch (scanError) {
            console.error(`Runner ${runId} scan during failed launch cleanup failed:`, scanError);
          }
          await new Promise(resolve => setTimeout(resolve, 25));
        }
        // Only remove registry files when scan affirmatively confirms the lock is free; otherwise leave them for recover().
        if (lockFree) await this.registry.remove(runId).catch(() => {});
        throw error;
      }
      // A host crash between durable acceptance and `start` matches in-process runs: no effects ran,
      // the unstarted runner exits on its orphan timeout, and recovery marks the thread interrupted.
      this.send({ type: 'start', payload: args.payload });
      return { runId, pid };
    } catch (error) { this.detach(); if (!handedOff) await args.lease.close(); throw error; }
  }
  async attach(runId: string, lastSeq: number, timeoutMs = 5000): Promise<void> {
    if (this.client) throw new Error('Supervisor already attached');
    this.runId = runId; this.lastSeq = lastSeq; this.detached = false;
    let resolveAttached!: () => void; let rejectAttached!: (error: Error) => void;
    const attached = new Promise<void>((resolve, reject) => { resolveAttached = resolve; rejectAttached = reject; });
    const timer = setTimeout(() => rejectAttached(new Error('Runner attach timed out')), timeoutMs);
    let client: RunnerClient | undefined;
    try {
      client = await RunnerClient.connect(this.registry.socketPath(runId), lastSeq, message => {
        if (message.type === 'attached') {
          const floor = typeof message.floor === 'number' ? message.floor : 1;
          if (this.lastSeq + 1 < floor) {
            // Runner pruned events this host never saved: refuse to apply later events over a stale snapshot.
            this.gapped = true;
            this.options.onGap?.(floor, this.lastSeq);
            try { client?.send({ type: 'stop' }); } catch {}
          }
          resolveAttached();
        } else {
          this.receive(message);
        }
      }, () => {
        rejectAttached(new Error('Runner disconnected'));
        if (this.client === client) this.onSocketClosed();
      });
      this.client = client;
      await attached;
      this.backoffMs = this.options.retryBaseMs ?? 100;
      if (this.client === client && !this.detached) {
        this.heartbeat = setInterval(() => this.ping(), this.options.heartbeatMs ?? 15000);
      }
    } catch (error) {
      if (this.client === client) this.client = undefined;
      client?.close();
      throw error;
    } finally { clearTimeout(timer); }
  }
  /** Schedule idempotent backoff reconnect retries while the runner's lock state is live or unknown. */
  startReconnect(runId: string, lastSeq: number): void {
    this.runId = runId; this.lastSeq = lastSeq; this.detached = false;
    this.backoffMs = this.options.retryBaseMs ?? 100;
    this.scheduleReconnect(this.backoffMs);
  }
  private onSocketClosed(): void {
    this.client = undefined;
    clearInterval(this.heartbeat); this.heartbeat = undefined;
    clearTimeout(this.deadline); this.deadline = undefined;
    this.nonce = undefined;
    // EOF can arrive just before the kernel releases the runner lock; check shortly and retry idempotently.
    this.scheduleReconnect(25);
  }
  /** Idempotent: at most one retry timer is ever scheduled at a time. */
  private scheduleReconnect(delayMs?: number): void {
    if (this.detached || this.client || this.retryTimer) return;
    const wait = delayMs ?? this.backoffMs;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = undefined;
      void this.reconnectTick().catch(error => {
        console.error(`Runner ${this.runId ?? 'unknown'} reconnect failed:`, error);
        this.bumpBackoff();
        this.scheduleReconnect();
      });
    }, wait);
  }
  private bumpBackoff(): void {
    const maxBackoff = this.options.retryMaxMs ?? 5000;
    this.backoffMs = Math.min(maxBackoff, this.backoffMs * 2);
  }
  private async reconnectTick(): Promise<void> {
    if (this.detached || this.client || this.retryInFlight) return;
    this.retryInFlight = true;
    try {
      let entry: RunnerEntry | undefined;
      try {
        entry = (await this.registry.scan()).find(item => item.manifest.run_id === this.runId);
      } catch (error) {
        // A scan error is "unknown", never "dead": keep reservation and retry with backoff.
        console.error(`Runner ${this.runId ?? 'unknown'} registry scan failed:`, error);
        this.bumpBackoff();
        this.scheduleReconnect();
        return;
      }
      if (this.detached) return;
      if (!entry || entry.status !== 'live') {
        this.options.onDisconnect?.(entry);
        return;
      }
      try {
        await this.attach(this.runId!, this.lastSeq, 5000);
      } catch {
        this.bumpBackoff();
        this.scheduleReconnect();
      }
    } finally {
      this.retryInFlight = false;
    }
  }
  private receive(event: Event): void {
    if (event.type === 'pong' && event.nonce === this.nonce) { clearTimeout(this.deadline); this.deadline = undefined; this.nonce = undefined; }
    if (this.gapped) return;
    this.queue = this.queue.then(async () => {
      if (this.gapped || event.seq <= this.lastSeq) return;
      await this.options.onEvent(event); this.lastSeq = event.seq;
      this.client?.send({ type: 'ack', seq: event.seq });
    }).catch(error => {
      console.error(`Runner ${this.runId ?? 'unknown'} event handling failed:`, error);
      this.client?.close();
    });
  }
  private ping(): void {
    if (!this.client || this.nonce) return;
    this.nonce = randomUUID();
    try { this.send({ type: 'ping', nonce: this.nonce }); }
    catch { this.nonce = undefined; return; }
    this.deadline = setTimeout(() => this.client?.close(), this.options.heartbeatTimeoutMs ?? 10000);
  }
  send(command: Exclude<Command, { type: 'attach' }>): void {
    if (!this.client) throw new Error('Runner is not attached');
    this.client.send(command);
  }
  stop(): void { this.send({ type: 'stop' }); }
  approve(approvalId: string, allow: boolean): void { this.send({ type: 'approve', approval_id: approvalId, allow }); }
  steer(requestId: string, text: string, images: unknown[]): void { this.send({ type: 'steer', request_id: requestId, text, images }); }
  /** A runner that never listened explains itself through the last lines of its log. */
  private async startFailure(runId: string, error: unknown): Promise<Error> {
    const log = await readFile(this.registry.path(runId, '.log'), 'utf8').catch(() => '');
    const tail = log.trim().split('\n').slice(-5).join('\n').slice(-2000);
    return new Error(`Runner did not start: ${error instanceof Error ? error.message : String(error)}${tail ? `\n${tail}` : ''}`);
  }
  detach(): void {
    this.detached = true;
    clearInterval(this.heartbeat); this.heartbeat = undefined;
    clearTimeout(this.deadline); this.deadline = undefined;
    clearTimeout(this.retryTimer); this.retryTimer = undefined;
    this.nonce = undefined;
    const client = this.client; this.client = undefined;
    client?.close();
  }
}
