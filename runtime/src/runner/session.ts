import { fail, queuedView } from '../protocol.ts';
import type { Approval, Snapshot } from '../protocol.ts';
import { nameKey, type Observation } from '../mcp/tools.ts';
import type { Records } from '../records.ts';
import type { Postimage } from '../safety.ts';
import type { Store } from '../store.ts';
import { TurnSnapshotApplier } from '../turn-executor.ts';
import { parseRunnerMessage, type Event, type RunnerState } from './protocol.ts';
import type { RunnerEntry } from './registry.ts';
import { RunnerSupervisor } from './supervisor.ts';

const APPROVAL_SETTLE_TIMEOUT_MS = 5000;
export const LOST_PROGRESS_MESSAGE = "Plait lost part of this run's progress and stopped it. Inspect the checkout before a new submission.";

export type RunnerReceipt = {
  result?: Record<string, unknown>;
  events?: unknown[];
};

export type RunnerEventHooks = {
  records?: Records;
  onMcpObserved?: (checkout: string, rootDev: string, rootIno: string, name: string, fingerprint: string, observation: Observation) => void;
};

export type RunnerCallbacks = RunnerEventHooks & {
  store: Store;
  directory: string;
  notify(id: string, summary: boolean): void;
  phase(id: string, phase: RunnerState): void;
  onClosed(session: RunnerSession, entry: RunnerEntry | undefined, finishedResult: Record<string, unknown> | undefined): void | Promise<void>;
};

/** Apply one runner event to a thread snapshot and persist it atomically with `runs.last_seq`. */
export function applyRunnerEvent(
  store: Store,
  runId: string,
  snapshot: Snapshot,
  applier: TurnSnapshotApplier,
  event: Event,
  onPhase?: (state: RunnerState) => void,
  hooks?: RunnerEventHooks,
): boolean {
  switch (event.type) {
    case 'phase': {
      const state = event.state as RunnerState;
      onPhase?.(state);
      if (state === 'running' && snapshot.status !== 'stopped' && snapshot.status !== 'interrupted') {
        snapshot.status = 'running';
      }
      return store.saveRunEvent(runId, event.seq, snapshot, { state });
    }
    case 'live':
      applier.livePayload(event.payload as Record<string, unknown>);
      for (const tool of snapshot.tools) {
        if (tool.name.startsWith('mcp__')) tool.name = store.setting(nameKey(snapshot.id, tool.name)) ?? tool.name;
      }
      return store.saveRunEvent(runId, event.seq, snapshot);
    case 'commit': {
      const head = typeof event.head === 'string' && /^\d+$/.test(event.head) ? Number(event.head) : 0;
      const placedEntries = Array.isArray((event.item as { placed_entries?: unknown } | undefined)?.placed_entries)
        ? (event.item as { placed_entries: unknown[] }).placed_entries
        : [];
      return store.saveRunEvent(runId, event.seq, snapshot, {
        head,
        extra: placedEntries.length ? () => {
          for (const raw of placedEntries) {
            if (!raw || typeof raw !== 'object') continue;
            const entry = raw as { request_id?: unknown; entry?: unknown };
            if (typeof entry.request_id === 'string' && typeof entry.entry === 'number' && Number.isSafeInteger(entry.entry)) {
              store.place(snapshot.id, entry.request_id, entry.entry);
            }
          }
          snapshot.queued = store.queue(snapshot.id).map(queuedView);
        } : undefined,
      });
    }
    case 'approval_needed':
      applier.approvalNeeded(event.approval as Approval);
      onPhase?.('awaiting_approval');
      return store.saveRunEvent(runId, event.seq, snapshot, { state: 'awaiting_approval' });
    case 'approval_settled':
      applier.approvalSettled(String(event.id));
      if (snapshot.status === 'running') onPhase?.('running');
      return store.saveRunEvent(runId, event.seq, snapshot, { state: snapshot.status === 'running' ? 'running' : undefined });
    case 'postimage':
      return store.saveRunEvent(runId, event.seq, snapshot, {
        extra: () => store.record(snapshot.id, event.postimage as Postimage),
      });
    case 'forget':
      return store.saveRunEvent(runId, event.seq, snapshot, {
        extra: () => store.forget(snapshot.id, String(event.path)),
      });
    case 'record_before':
    case 'record_after':
      return store.saveRunEvent(runId, event.seq, snapshot, {
        extra: () => hooks?.records?.indexRunnerRecord(
          event.type === 'record_before' ? 'before' : 'after',
          event.record as Record<string, unknown>,
        ),
      });
    case 'tool_label': {
      const name = String(event.name);
      const label = String(event.label);
      return store.saveRunEvent(runId, event.seq, snapshot, {
        extra: () => {
          const key = nameKey(snapshot.id, name);
          if (store.setting(key) === null) store.setSetting(key, label);
          for (const tool of snapshot.tools) if (tool.name === name) tool.name = label;
        },
      });
    }
    case 'mcp_observed':
      hooks?.onMcpObserved?.(
        String(event.checkout),
        String(event.root_dev),
        String(event.root_ino),
        String(event.name),
        String(event.fingerprint),
        event.observation as Observation,
      );
      store.advanceRunSeq(runId, event.seq);
      return false;
    case 'pong':
      store.advanceRunSeq(runId, event.seq);
      return false;
    case 'finished':
      // Save terminal snapshot/result and last_seq atomically before acking the runner.
      applier.finish(event.result && typeof event.result === 'object' ? event.result as Record<string, unknown> : undefined);
      return store.saveRunEvent(runId, event.seq, snapshot, { state: 'finished', finished: true });
  }
}

const GAP_EFFECT_EVENTS = new Set<Event['type']>(['postimage', 'forget', 'record_before', 'record_after', 'approval_settled', 'tool_label']);

/** Apply unacknowledged receipt events after `lastSeq`, then the terminal status, idempotently. */
export function applyRunnerReceipt(
  store: Store,
  runId: string,
  snapshot: Snapshot,
  receipt: unknown,
  lastSeq: number,
  gapped = false,
  hooks?: RunnerEventHooks,
): boolean {
  const applier = new TurnSnapshotApplier(snapshot);
  const parsed = receipt && typeof receipt === 'object' ? receipt as RunnerReceipt : {};
  const isGapped = gapped || store.run(runId)?.state === 'gap';
  let seq = lastSeq;
  let summaryChanged = false;
  if (Array.isArray(parsed.events)) {
    const valid: Event[] = [];
    for (const raw of parsed.events) {
      try {
        const message = parseRunnerMessage(raw);
        if (message.type !== 'attached') valid.push(message);
      } catch { /* Ignore malformed receipt entries. */ }
    }
    valid.sort((a, b) => a.seq - b.seq);
    for (const event of valid) {
      if (event.seq <= seq || event.type === 'finished') continue;
      if (isGapped) {
        // After a gap, still apply effect-carrying boundary events idempotently without overwriting the interrupted snapshot.
        if (!GAP_EFFECT_EVENTS.has(event.type)) continue;
        summaryChanged = applyRunnerEvent(store, runId, snapshot, applier, event, undefined, hooks) || summaryChanged;
      } else {
        summaryChanged = applyRunnerEvent(store, runId, snapshot, applier, event, undefined, hooks) || summaryChanged;
      }
      seq = event.seq;
    }
  }
  if (isGapped) {
    applier.interrupt(LOST_PROGRESS_MESSAGE);
    return store.saveRunEvent(runId, seq, snapshot, { state: 'gap', finished: true }) || summaryChanged;
  }
  applier.finish(parsed.result && typeof parsed.result === 'object' ? parsed.result : undefined);
  return store.saveRunEvent(runId, seq, snapshot, { state: 'finished', finished: true }) || summaryChanged;
}

/** Host-side session supervising a detached runner process. */
export class RunnerSession {
  readonly runId: string;
  readonly threadId: string;
  readonly requestId: string;
  readonly snapshot: Snapshot;
  readonly supervisor: RunnerSupervisor;
  private readonly applier: TurnSnapshotApplier;
  private readonly callbacks: RunnerCallbacks;
  private readonly steered = new Set<string>();
  private finishedResult?: Record<string, unknown>;
  private approvalWaiter?: { id: string; resolve: () => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> };
  private closed = false;
  gapped = false;

  constructor(runId: string, threadId: string, requestId: string, snapshot: Snapshot, callbacks: RunnerCallbacks) {
    this.runId = runId;
    this.threadId = threadId;
    this.requestId = requestId;
    this.snapshot = snapshot;
    this.callbacks = callbacks;
    this.applier = new TurnSnapshotApplier(snapshot);
    this.supervisor = new RunnerSupervisor({
      store: callbacks.directory,
      onEvent: event => this.onEvent(event),
      onReattach: () => this.resendSteers(),
      onDisconnect: entry => this.onDisconnect(entry),
      onGap: () => this.onGap(),
    });
  }

  private onGap(): void {
    if (this.gapped) return;
    this.gapped = true;
    this.applier.interrupt(LOST_PROGRESS_MESSAGE);
    // Persist `runs.state = 'gap'` in the same transaction as the interrupted snapshot so restarts preserve it.
    const lastSeq = this.callbacks.store.run(this.runId)?.last_seq ?? 0;
    const summaryChanged = this.callbacks.store.saveRunEvent(this.runId, lastSeq, this.snapshot, { state: 'gap' });
    this.callbacks.notify(this.threadId, summaryChanged);
    this.settleApprovalWaiter();
  }

  private onEvent(event: Event): void {
    if (event.type === 'finished') {
      this.finishedResult = event.result && typeof event.result === 'object' ? event.result as Record<string, unknown> : {};
    }
    const summaryChanged = applyRunnerEvent(
      this.callbacks.store,
      this.runId,
      this.snapshot,
      this.applier,
      event,
      state => this.callbacks.phase(this.threadId, state),
      this.callbacks,
    );
    if (event.type !== 'pong' && event.type !== 'mcp_observed') this.callbacks.notify(this.threadId, summaryChanged);
    if (event.type === 'finished' || (event.type === 'approval_settled' && this.approvalWaiter?.id === String(event.id))) {
      this.settleApprovalWaiter();
    }
  }

  private onDisconnect(entry: RunnerEntry | undefined): void {
    if (this.closed) return;
    this.closed = true;
    this.settleApprovalWaiter();
    void Promise.resolve(this.callbacks.onClosed(this, entry, this.finishedResult)).catch(error => {
      console.error(`Runner ${this.runId} close reconciliation failed:`, error);
    });
  }

  private settleApprovalWaiter(error?: Error): void {
    const waiter = this.approvalWaiter;
    if (!waiter) return;
    this.approvalWaiter = undefined;
    clearTimeout(waiter.timer);
    if (error) waiter.reject(error); else waiter.resolve();
  }

  async approve(approvalId: string, allow: boolean): Promise<void> {
    if (this.snapshot.pending_approval?.id !== approvalId) {
      fail('not_found', 'Approval is stale, stopped, or already answered.');
    }
    const settled = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.approvalWaiter?.id === approvalId) {
          this.approvalWaiter = undefined;
          reject(new Error('Approval settlement timed out.'));
        }
      }, APPROVAL_SETTLE_TIMEOUT_MS);
      this.approvalWaiter = { id: approvalId, resolve, reject, timer };
    });
    try {
      this.supervisor.approve(approvalId, allow);
    } catch (error) {
      this.settleApprovalWaiter(error instanceof Error ? error : new Error(String(error)));
    }
    await settled;
  }

  stop(): void {
    try { this.supervisor.stop(); } catch { /* Runner may already be disconnecting. */ }
  }

  private resendSteers(): void {
    this.steered.clear();
    for (const input of this.callbacks.store.queue(this.threadId)) {
      if (input.queued?.mode !== 'steer') continue;
      this.steer(input.request_id, input.text, input.images ?? []);
    }
  }

  steer(requestId: string, text: string, images: unknown[]): void {
    if (this.steered.has(requestId)) return;
    this.steered.add(requestId);
    try { this.supervisor.steer(requestId, text, images); }
    catch { this.steered.delete(requestId); }
  }

  detach(): void {
    this.closed = true;
    this.settleApprovalWaiter();
    this.supervisor.detach();
  }
}
