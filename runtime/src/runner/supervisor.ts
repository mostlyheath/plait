import { randomBytes, randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { releaseAfterHandoff, type CheckoutLease } from '../safety.ts';
import { RunnerRegistry, type RunnerEntry } from './registry.ts';
import { RunnerClient } from './socket.ts';
import { spawnRunner } from './spawn.ts';
import type { Command, Event } from './protocol.ts';

type Options = { store: string; onEvent: (event: Event) => void | Promise<void>; onDisconnect?: (entry: RunnerEntry | undefined, error?: unknown) => void; entry?: string };
export class RunnerSupervisor {
  private readonly registry: RunnerRegistry;
  private readonly options: Options;
  private client?: RunnerClient;
  private runId?: string;
  private lastSeq = 0;
  private queue = Promise.resolve();
  private heartbeat?: NodeJS.Timeout;
  private deadline?: NodeJS.Timeout;
  private nonce?: string;
  private detached = false;
  constructor(options: Options) { this.options = options; this.registry = new RunnerRegistry(options.store); }
  async launch(args: { threadId: string; requestId: string; lease: CheckoutLease; payload: Record<string, unknown> }): Promise<{ runId: string; pid: number }> {
    // Short ids keep the runner socket path inside macOS's 104-byte Unix socket limit.
    const runId = randomBytes(6).toString('hex'); this.runId = runId;
    let handedOff = false;
    try {
      await this.registry.writeManifest({ run_id: runId, thread_id: args.threadId, request_id: args.requestId, checkout: args.lease.root, harness: String(args.payload.kind ?? 'unknown'), build_id: 'runner-skeleton', lease: { root: args.lease.root, identity: args.lease.identity } });
      const pid = await spawnRunner(this.options.entry ?? fileURLToPath(new URL('./main.ts', import.meta.url)), args.lease, { PLAIT_RUNNER_STORE: this.options.store, PLAIT_RUNNER_ID: runId });
      const deadline = Date.now() + 5000;
      for (;;) {
        try { await this.attach(runId, 0, Math.max(1, deadline - Date.now())); break; }
        catch (error) { if (Date.now() >= deadline) throw error; await new Promise(resolve => setTimeout(resolve, 25)); }
      }
      await releaseAfterHandoff(args.lease); handedOff = true;
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
    try {
      this.client = await RunnerClient.connect(this.registry.socketPath(runId), lastSeq, message => {
        if (message.type === 'attached') resolveAttached(); else this.receive(message);
      }, () => { rejectAttached(new Error('Runner disconnected')); void this.disconnected(); });
      await attached;
      this.heartbeat = setInterval(() => this.ping(), 15000);
    } catch (error) { this.client?.close(); this.client = undefined; throw error; }
    finally { clearTimeout(timer); }
  }
  private receive(event: Event): void {
    if (event.type === 'pong' && event.nonce === this.nonce) { clearTimeout(this.deadline); this.nonce = undefined; }
    this.queue = this.queue.then(async () => {
      if (event.seq <= this.lastSeq) return;
      await this.options.onEvent(event); this.lastSeq = event.seq;
      this.client?.send({ type: 'ack', seq: event.seq });
    }).catch(error => { this.client?.close(); this.options.onDisconnect?.(undefined, error); });
  }
  private ping(): void {
    this.nonce = randomUUID(); this.send({ type: 'ping', nonce: this.nonce });
    this.deadline = setTimeout(() => this.client?.close(), 10000);
  }
  private async disconnected(): Promise<void> {
    if (this.detached) return;
    this.client = undefined; clearInterval(this.heartbeat); clearTimeout(this.deadline);
    try {
      let entry = (await this.registry.scan()).find(item => item.manifest.run_id === this.runId);
      // EOF can arrive just before the kernel releases the runner lock.
      if (entry?.status === 'live') {
        await new Promise(resolve => setTimeout(resolve, 100));
        entry = (await this.registry.scan()).find(item => item.manifest.run_id === this.runId);
      }
      if (!this.detached) this.options.onDisconnect?.(entry);
    }
    catch (error) { this.options.onDisconnect?.(undefined, error); }
  }
  send(command: Exclude<Command, { type: 'attach' }>): void {
    if (!this.client) throw new Error('Runner is not attached');
    this.client.send(command);
  }
  stop(): void { this.send({ type: 'stop' }); }
  approve(approvalId: string, allow: boolean): void { this.send({ type: 'approve', approval_id: approvalId, allow }); }
  steer(requestId: string, text: string, images: unknown[]): void { this.send({ type: 'steer', request_id: requestId, text, images }); }
  detach(): void { this.detached = true; clearInterval(this.heartbeat); clearTimeout(this.deadline); this.client?.close(); this.client = undefined; }
}
