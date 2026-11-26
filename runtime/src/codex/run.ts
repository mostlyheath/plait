import { type CleanupProof } from '../cleanup.ts';
import type { ImageData } from '../images.ts';
// One Plait run of a Codex thread: a single Codex turn on the shared app-server.
//
// Codex owns the work: its sandbox, its tools, and its approvals policy. Plait
// starts or resumes the Codex thread in the thread's Codex mode, records the
// mode Codex reports, shows each approval Codex asks for as a Plait approval,
// and stores each finished item so the thread reads like a Pi one.
import { randomUUID } from 'node:crypto';
import { isAbsolute, relative, sep } from 'node:path';
import type { ConversationView } from '@earendil-works/pi-durable';
import { approvalFor, approvalReply, declineUnsupported } from './approval.ts';
import type { ExternalEntries } from '../external/entries.ts';
import { NOTIFICATIONS } from './protocol.ts';
import { CodexRpcError } from './server.ts';
import type {
  FileChangeItem, ItemNotification, ThreadResumeParams, ThreadStartParams,
  ThreadStartResponse, TurnInterruptParams, TurnStartParams, TurnStartResponse, TurnSteerParams, UserInput,
} from './protocol.ts';
import { CodexTranscript, isTerminalTurn } from './transcript.ts';
import { codexPolicy, codexReported } from '../harness.ts';
import type { Lease } from '../harness.ts';
import type { InteractiveApproval } from '../approval.ts';
import { fail, HostError } from '../protocol.ts';
import type { InputRecord, Snapshot } from '../protocol.ts';
import { update } from '../transcript.ts';

/** The newest entries the live view is built from, as for Pi. */
const VIEW_ENTRIES = 100;
/** Streaming text reaches watchers at most this often; finished items go at once. */
const PUBLISH_MS = 16;
/** How long Stop waits for Codex to end the turn before the run ends without it. */
const INTERRUPT_WAIT_MS = 10_000;

export type CodexRunContext = {
  snapshot: Snapshot;
  cleanup?: CleanupProof;
  instructions?: string;
  /** The thread's Codex mode for this turn. */
  mode: string;
  lease: Lease;
  entries: ExternalEntries;
  approvals: InteractiveApproval;
  /** Ends the run: Stop, a host shutdown, or the run finishing. */
  signal: AbortSignal;
  /** Checkout paths that had changes when the run started, shown on edit approvals. */
  dirty: ReadonlySet<string>;
  inputs(): InputRecord[];
  /** Records that a Plait request became this transcript entry. */
  place(requestId: string, entry: number): void;
  /** Saves the snapshot (and tells watchers); `head` is the newest stored entry. */
  persist(head: number): void;
  /** Tells watchers about live text without writing it to disk. */
  publish(): void;
  /** Records a checkout file Codex wrote, for changed files. */
  changed(path: string): void;
};

const content = (value: string, images: ImageData[]): UserInput[] => [...(value ? [{ type: 'text' as const, text: value, text_elements: [] as [] }] : []), ...images.map(image => ({ type: 'image' as const, url: `data:${image.ref.media_type};base64,${image.data}` }))];

export class CodexRun {
  private readonly context: CodexRunContext;
  private readonly transcript: CodexTranscript;
  private thread?: string;
  private turn?: string;
  private interruptTimer?: NodeJS.Timeout;
  private confirm?: () => void;
  private readonly confirmed = new Promise<void>(resolve => { this.confirm = resolve; });
  private startedRequest = false;
  private closed = false;
  /** Turns that ended before `turn/start` replied, so a fast turn is not missed. */
  private readonly ended = new Map<string, NonNullable<ReturnType<typeof isTerminalTurn>>>();
  private finish?: (end: NonNullable<ReturnType<typeof isTerminalTurn>>) => void;
  private timer?: NodeJS.Timeout;
  private head = 0;
  private unroute?: () => void;
  /** The run's own request until its user entry arrives. */
  private unplaced?: string;

  constructor(context: CodexRunContext) {
    this.context = context;
    const { entries, next } = context.entries.tail(VIEW_ENTRIES);
    this.transcript = new CodexTranscript(entries, next);
    this.head = next - 1;
  }

  /** Runs one turn for `text`; resolves when Codex ends it, and throws when it fails. */
  async run(requestId: string, input: string, model: string, effort: string | undefined, images: ImageData[] = []) {
    try { return await this.turnFor(requestId, input, model, effort, images); }
    catch (error) {
      // Codex's own errors and output can quote the provider or the machine, so the client gets a fixed message.
      if (error instanceof HostError) throw error;
      fail('runtime_error', 'Codex stopped or did not answer as expected. Inspect the checkout before a new submission.');
    }
  }

  private async turnFor(requestId: string, input: string, model: string, effort: string | undefined, images: ImageData[] = []) {
    const { snapshot, lease, signal } = this.context;
    const cwd = snapshot.checkout;
    // The mode's settings are sent in full each turn, so a change between turns applies to the next one.
    const { approvalPolicy, sandbox, sandboxPolicy, approvalsReviewer } = codexPolicy(this.context.mode);
    const saved = this.context.entries.thread();
    let reply: ThreadStartResponse;
    if (saved) {
      const params: ThreadResumeParams = { threadId: saved, cwd, approvalPolicy, sandbox, approvalsReviewer, model, excludeTurns: true, developerInstructions: this.context.instructions };
      reply = await lease.server.request<ThreadStartResponse>('thread/resume', params).catch(() => {
        fail('runtime_error', 'Codex could not continue this thread. Its session may have been removed; start a new thread.');
      });
      this.thread = saved;
    } else {
      const params: ThreadStartParams = { model, cwd, approvalPolicy, sandbox, approvalsReviewer, developerInstructions: this.context.instructions };
      reply = await lease.server.request<ThreadStartResponse>('thread/start', params);
      this.thread = reply.thread.id;
      this.context.entries.setThread(this.thread);
    }
    // What Codex says it will use, which its own requirements can override; the thread shows this, not what was asked.
    if (snapshot.harness) snapshot.harness.reported = codexReported(reply);
    this.unroute = lease.route(this.thread, {
      notify: (method, params) => this.notify(method, params),
      request: (method, params) => this.request(method, params),
    });
    if (signal.aborted) return;
    this.unplaced = requestId;
    const ended = new Promise<NonNullable<ReturnType<typeof isTerminalTurn>>>(resolve => { this.finish = resolve; });
    const params: TurnStartParams = {
      threadId: this.thread, input: content(input, images), clientUserMessageId: requestId, cwd, approvalPolicy, sandboxPolicy, approvalsReviewer, model,
      effort: effort ?? null, summary: 'auto',
    };
    this.startedRequest = true;
    this.context.cleanup?.track(Promise.race([this.confirmed, lease.server.cleanup]), 'Codex did not confirm its turn ended and its app-server has not exited.');
    const { turn } = await lease.server.request<TurnStartResponse>('turn/start', params).catch(error => {
      // A refusal acknowledges that no turn started; a lost reply cannot.
      if (error instanceof CodexRpcError) this.confirm?.();
      throw error;
    });
    this.turn = turn.id;
    // The reply can come before Codex announces the turn; the transcript needs to know it either way.
    this.notify(NOTIFICATIONS.turnStarted, { threadId: this.thread, turn });
    const early = this.ended.get(turn.id);
    if (early) { this.confirm?.(); this.finish!(early); }
    const stop = () => void this.interrupt();
    signal.addEventListener('abort', stop, { once: true });
    if (signal.aborted) stop();
    try {
      const end = await Promise.race([
        ended,
        lease.server.exited.then(() => fail('runtime_error', 'Codex stopped unexpectedly. Inspect the checkout before a new submission.')),
      ]);
      if (end.status === 'interrupted' && !signal.aborted) {
        return { code: 'interrupted', message: 'Codex interrupted the turn.' };
      }
      // A failed turn's message is Codex's own and may quote the provider, so it is not passed on.
      if (end.status === 'failed') fail('runtime_error', 'Codex ended the turn with an error. Its own logs have the details.');
    } finally {
      signal.removeEventListener('abort', stop);
      this.flush();
    }
  }

  /** Joins a message to the turn that is running now. False when there is no turn to join. */
  async steer(requestId: string, input: string, images: ImageData[] = []): Promise<boolean> {
    if (!this.thread || !this.turn || this.context.signal.aborted) return false;
    const params: TurnSteerParams = { threadId: this.thread, clientUserMessageId: requestId, input: content(input, images), expectedTurnId: this.turn };
    return this.context.lease.server.request('turn/steer', params).then(() => true, () => false);
  }

  /** Asks Codex to end the turn, and stops waiting for it after a bound. */
  private async interrupt() {
    if (!this.thread || !this.turn) return;
    const turn = this.turn;
    const params: TurnInterruptParams = { threadId: this.thread, turnId: turn };
    this.interruptTimer ??= setTimeout(() => this.finish?.({ turnId: turn, status: 'interrupted' }), INTERRUPT_WAIT_MS);
    this.interruptTimer.unref?.();
    await this.context.lease.server.request('turn/interrupt', params).catch(() => {});
  }

  close() {
    this.closed = true;
    clearTimeout(this.timer);
    clearTimeout(this.interruptTimer);
    if (!this.startedRequest) this.unroute?.();
    else void Promise.race([this.confirmed, this.context.lease.server.cleanup]).then(() => this.unroute?.());
  }

  private notify(method: string, params: any) {
    const end = isTerminalTurn(method, params);
    if (end && end.turnId === this.turn) {
      this.confirm?.(); clearTimeout(this.interruptTimer);
    }
    if (this.closed) return;
    const applied = this.transcript.apply(method, params);
    if (applied.append.length) {
      this.context.entries.append(applied.append);
      this.head = Math.max(this.head, ...applied.append.map(entry => entry.id));
    }
    for (const { clientId, entry } of applied.placed) {
      if (clientId === this.unplaced) this.unplaced = undefined;
      this.context.place(clientId, entry);
    }
    // Codex is expected to echo the request ID; if it does not, the turn's first user entry is the run's own.
    const user = this.unplaced === undefined ? undefined
      : applied.append.find(entry => entry.model[0]?.role === 'user' && !applied.placed.some(placed => placed.entry === entry.id));
    if (user && this.unplaced) { this.context.place(this.unplaced, user.id); this.unplaced = undefined; }
    if (method === NOTIFICATIONS.itemCompleted) this.recordChanges((params as ItemNotification).item);
    if (end) {
      if (end.turnId === this.turn) this.finish?.(end);
      else this.ended.set(end.turnId, end);
    }
    if (applied.append.length || applied.placed.length || end) this.flush();
    else if (applied.changed) this.schedule();
  }

  /** Answers one Codex request: an approval Plait shows, or a refusal. */
  private async request(method: string, params: any): Promise<unknown> {
    const approval = approvalFor(method, params, this.transcript.edit(params?.itemId));
    if (!approval) {
      const refusal = declineUnsupported(method);
      if (refusal === undefined) throw new Error('Plait does not answer this request.');
      return refusal;
    }
    this.flush();
    const dirty = approval.paths.map(path => this.inCheckout(path)).filter((path): path is string => !!path && this.context.dirty.has(path));
    const allowed = await this.context.approvals.ask({
      id: randomUUID(), tool_id: typeof params?.itemId === 'string' ? params.itemId : '', kind: approval.kind,
      summary: approval.summary, details: approval.details, dirty_paths: dirty, risk: approval.risk,
    }, this.context.signal).catch(() => false);
    return approvalReply(method, allowed);
  }

  private recordChanges(item: ItemNotification['item'] | undefined) {
    if (item?.type !== 'fileChange' || (item as FileChangeItem).status !== 'completed') return;
    for (const change of (item as FileChangeItem).changes) {
      const path = this.inCheckout(change.path);
      if (path) this.context.changed(path);
      const moved = change.kind.type === 'update' ? change.kind.move_path : null;
      const target = moved ? this.inCheckout(moved) : undefined;
      if (target) this.context.changed(target);
    }
  }

  /** A checkout-relative path, or undefined for one outside the checkout. */
  private inCheckout(path: string): string | undefined {
    const inside = relative(this.context.snapshot.checkout, isAbsolute(path) ? path : `${this.context.snapshot.checkout}${sep}${path}`);
    return inside && !inside.startsWith(`..${sep}`) && inside !== '..' && !isAbsolute(inside) ? inside : undefined;
  }

  private schedule() {
    if (this.timer) return;
    this.timer = setTimeout(() => { this.timer = undefined; this.render(); this.context.publish(); }, PUBLISH_MS);
  }

  /** Brings the snapshot up to date and saves it. */
  private flush() {
    clearTimeout(this.timer); this.timer = undefined;
    this.render();
    this.context.persist(this.head);
  }

  private render() {
    const { snapshot } = this.context;
    update(snapshot, this.transcript.view() as unknown as ConversationView, this.context.inputs(), null);
    snapshot.usage = this.transcript.usage(null) ?? snapshot.usage ?? null;
  }
}
