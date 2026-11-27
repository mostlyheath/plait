import type { CleanupProof } from '../cleanup.ts';
import type { ImageData } from '../images.ts';
// One Plait run of a Claude Code thread: one Claude Code process, which takes
// the run's message and any steering, and ends when no message is left.
//
// Claude Code owns the work: its tools, its permission mode, and the person's
// settings. Plait starts or resumes the thread's Claude session in the
// thread's mode, records the mode Claude Code reports, shows each permission
// request as a Plait approval, and stores each finished item so the thread
// reads like a Pi one.
import { randomUUID } from 'node:crypto';
import { isAbsolute, relative, sep } from 'node:path';
import type { SDKControlInitializeResponse, SDKMessage } from '@anthropic-ai/claude-agent-sdk';
import type { ConversationView } from '@earendil-works/pi-durable';
import type { InteractiveApproval } from '../approval.ts';
import type { ExternalEntries } from '../external/entries.ts';
import { fail, HostError } from '../protocol.ts';
import type { InputRecord, Snapshot, ThinkingLevel } from '../protocol.ts';
import { update } from '../transcript.ts';
import { canUseToolFrom } from './approval.ts';
import type { Ask } from './approval.ts';
import { claudeMode, claudeReported } from './harness.ts';
import type { ClaudeLaunch } from './harness.ts';
import { catalogModel } from './install.ts';
import { ClaudeSession } from './session.ts';
import { ClaudeTranscript, isTurnEnd } from './transcript.ts';
import type { Applied } from '../external/entries.ts';

const VIEW_ENTRIES = 100;
const PUBLISH_MS = 16;
/** How long Stop waits for Claude Code to end the turn before the run ends without it. */
const INTERRUPT_WAIT_MS = 10_000;
const STOPPED = 'Claude Code stopped unexpectedly. Inspect the checkout before a new submission.';
const RESUME_FAILED = 'Claude Code could not continue this thread. Its session may have been removed; start a new thread.';
const NOT_STARTED = 'Claude Code did not start. Run `claude` in a terminal to see why.';
const TURN_FAILED = 'Claude Code ended the turn with an error. Its own logs have the details.';

export type ClaudeRunContext = {
  snapshot: Snapshot;
  cleanup?: CleanupProof;
  instructions?: string;
  /** The thread's Plait mode ID when the run starts; a change during the run goes through `setMode`. */
  mode: string;
  launch: ClaudeLaunch;
  /** A model ID from Claude Code's list; `default` is Claude Code's own choice. */
  model: string;
  level: ThinkingLevel | undefined;
  entries: ExternalEntries;
  approvals: InteractiveApproval;
  signal: AbortSignal;
  /** Checkout paths that had changes when the run started, shown on edit approvals. */
  dirty: ReadonlySet<string> | undefined;
  inputs(): InputRecord[];
  place(requestId: string, entry: number): void;
  persist(head: number): void;
  publish(): void;
  changed(path: string): void;
  /** Claude Code's `initialize` answer, which says what it offers now. */
  initialized(init: SDKControlInitializeResponse): void;
  /** The run's message is sent, so steering messages can join. */
  opened(): void;
};

export class ClaudeRun {
  private readonly context: ClaudeRunContext;
  private readonly transcript: ClaudeTranscript;
  private session?: ClaudeSession;
  private head: number;
  private timer?: NodeJS.Timeout;
  private interruptTimer?: NodeJS.Timeout;
  /** Set once the run's own message is sent, until the run ends; steering needs it. */
  private open = false;
  /** Set once Claude Code has answered `initialize`, until the run ends; a mode change needs it. */
  private ready = false;
  /** Whether Claude Code started a turn, which tells a failed resume from a failed turn. */
  private started = false;
  /** Edit and write calls by tool ID, with the checkout path they name. */
  private readonly writes = new Map<string, string>();
  /** Approvals are shown one at a time; Claude Code may ask for parallel calls together. */
  private asking: Promise<unknown> = Promise.resolve();

  constructor(context: ClaudeRunContext) {
    this.context = context;
    const { entries, next } = context.entries.tail(VIEW_ENTRIES);
    this.transcript = new ClaudeTranscript(entries, next);
    this.head = next - 1;
  }

  /** Runs until Claude Code has answered every message; throws a fixed message when it fails. */
  async run(requestId: string, input: string, images: ImageData[] = []) {
    try { return await this.turns(requestId, input, images); }
    catch (error) {
      // Claude Code's own errors can quote the provider, the account, or the machine.
      if (error instanceof HostError) throw error;
      fail('runtime_error', STOPPED);
    } finally {
      clearTimeout(this.interruptTimer);
      this.open = false; this.ready = false;
      this.session?.close();
      this.flush();
    }
  }

  /** Sends a message into the running session. False when there is none to join. */
  steer(requestId: string, input: string, images: ImageData[] = []): boolean {
    if (!this.open || !this.session || this.context.signal.aborted) return false;
    try {
      this.transcript.sent(this.session.send(input, requestId, images), requestId, input);
      return true;
    } catch { return false; }
  }

  /** Switches the running session's mode. `reported` follows when Claude Code confirms it. */
  async setMode(mode: string) {
    const session = this.session;
    if (!session || !this.ready) return;
    // A refused switch leaves the mode Claude Code last reported.
    await session.setPermissionMode(claudeMode(mode)).catch(() => this.report(session.permissionMode()));
  }

  close() { clearTimeout(this.interruptTimer); clearTimeout(this.timer); this.session?.close(); }

  private async turns(requestId: string, input: string, images: ImageData[]) {
    const { snapshot, launch, signal, entries } = this.context;
    const resume = entries.thread();
    const known = launch.models?.find(model => model.id === this.context.model);
    const ask: Ask = (approval, request, aborted) => this.ask(approval, request.toolUseID, aborted);
    const session = new ClaudeSession({
      cleanup: this.context.cleanup, query: launch.sdk.query, path: launch.install.path, env: launch.env, cwd: snapshot.checkout,
      model: this.context.model, modelInfo: known, level: this.context.level, mode: claudeMode(this.context.mode),
      resume, instructions: this.context.instructions, canUseTool: canUseToolFrom(ask),
    });
    this.session = session;
    const stop = () => void this.interrupt();
    signal.addEventListener('abort', stop, { once: true });
    if (signal.aborted) stop();
    try {
      // Messages are read while initialize is answered; the SDK sees control replies only then.
      const consumed = this.consume(session.messages()[Symbol.asyncIterator](), resume !== undefined);
      const exited = consumed.then(() => undefined, () => undefined);
      const init = await Promise.race([session.initialized.catch(() => undefined), exited]);
      if (signal.aborted) { session.close(); await exited; return; }
      if (init === undefined) {
        session.close(); await exited;
        fail('runtime_error', resume ? RESUME_FAILED : NOT_STARTED);
      }
      this.context.initialized(init);
      const offered = Array.isArray(init.models) ? init.models.map(catalogModel) : [];
      if (this.context.model !== 'default' && offered.length && !offered.some(model => model.id === this.context.model)) {
        session.close(); await exited;
        fail('model_unavailable', 'This model is no longer offered by your Claude Code. Choose another.');
      }
      this.ready = true;
      this.report(session.permissionMode());
      // A mode chosen while Claude Code was starting applies before the message is sent.
      const wanted = snapshot.harness?.mode ?? this.context.mode;
      if (wanted !== this.context.mode) await this.setMode(wanted);
      if (!resume) entries.setThread(session.sessionId);
      if (signal.aborted) { session.close(); await exited; return; }
      this.transcript.sent(session.send(input, requestId, images), requestId, input);
      this.open = true;
      this.context.opened();
      return await consumed;
    } finally {
      signal.removeEventListener('abort', stop);
    }
  }

  /** Reads Claude Code's messages until every sent message is answered, Stop ends the turn, or Claude Code exits. */
  private async consume(messages: AsyncIterator<SDKMessage>, resumed: boolean) {
    const { signal } = this.context;
    for (;;) {
      let next: IteratorResult<SDKMessage>;
      try { next = await messages.next(); }
      catch { next = { done: true, value: undefined }; }
      if (next.done) {
        this.apply(this.transcript.finish());
        if (signal.aborted || !this.open && !resumed) return;
        fail('runtime_error', resumed && !this.started ? RESUME_FAILED : STOPPED);
      }
      const message = next.value;
      if (message.type === 'system' && message.subtype === 'init') this.started = true;
      this.apply(this.transcript.apply(message));
      const reported = message.type === 'system' && (message.subtype === 'init' || message.subtype === 'status') ? (message as { permissionMode?: unknown }).permissionMode : undefined;
      if (typeof reported === 'string') this.report(reported);
      const end = isTurnEnd(message);
      if (!end) continue;
      if (signal.aborted) return;
      if (end.status === 'interrupted') {
        return { code: 'interrupted', message: 'Claude Code interrupted the turn.' };
      }
      if (end.status === 'failed') fail('runtime_error', resumed && !this.started ? RESUME_FAILED : TURN_FAILED);
      // A message sent after the turn's last step runs as a turn of its own.
      if (this.open && end.queued === 0 && this.transcript.unplaced().length === 0) return;
    }
  }

  private apply(applied: Applied) {
    if (applied.append.length) {
      this.context.entries.append(applied.append);
      this.head = Math.max(this.head, ...applied.append.map(entry => entry.id));
      this.recordChanges(applied);
    }
    for (const { clientId, entry } of applied.placed) this.context.place(clientId, entry);
    if (applied.append.length || applied.placed.length) this.flush();
    else if (applied.changed) this.schedule();
  }

  /** Files Claude Code's edit and write tools changed in the checkout, once their result says they succeeded. */
  private recordChanges(applied: Applied) {
    for (const entry of applied.append) for (const message of entry.model) {
      if (message.role === 'assistant' && Array.isArray(message.content)) {
        for (const part of message.content) {
          if (part.type !== 'toolCall' || (part.name !== 'edit' && part.name !== 'write') || !part.id) continue;
          const path = this.inCheckout(String((part.arguments as { path?: unknown })?.path ?? ''));
          if (path) this.writes.set(part.id, path);
        }
      }
      if (message.role === 'toolResult' && message.toolCallId && !message.isError) {
        const path = this.writes.get(message.toolCallId);
        if (path) { this.context.changed(path); this.writes.delete(message.toolCallId); }
      }
    }
  }

  private async ask(approval: Parameters<Ask>[0], toolId: string, aborted: AbortSignal): Promise<boolean> {
    const signal = AbortSignal.any([this.context.signal, aborted]);
    const turn = this.asking.then(async () => {
      if (signal.aborted) return false;
      this.flush();
      const dirty = approval.paths.map(path => this.inCheckout(path)).filter((path): path is string => !!path && this.context.dirty?.has(path) === true);
      return this.context.approvals.ask({
        id: randomUUID(), tool_id: toolId, kind: approval.kind, summary: approval.summary, details: approval.details, dirty_paths: dirty, risk: approval.kind === 'edit' && !this.context.dirty
          ? `Plait could not check for pre-existing changes. ${approval.risk}` : approval.risk,
      }, signal);
    }).catch(() => false);
    this.asking = turn;
    return turn;
  }

  /** Asks Claude Code to stop the turn and drop what is queued; ends the session if it does not answer in time. */
  private async interrupt() {
    const session = this.session;
    if (!session) return;
    this.open = false;
    this.interruptTimer ??= setTimeout(() => session.close(), INTERRUPT_WAIT_MS);
    this.interruptTimer.unref?.();
    try {
      const receipt = await session.interrupt();
      // Those requests never ran; Stop has already taken them off the queue.
      this.transcript.drop(receipt.cancelled);
    } catch { session.close(); }
  }

  private report(mode: string | null) {
    const { snapshot } = this.context;
    if (!snapshot.harness || mode === null) return;
    snapshot.harness = { ...snapshot.harness, reported: claudeReported(mode) };
    this.flush();
  }

  private inCheckout(path: string): string | undefined {
    if (!path) return undefined;
    const inside = relative(this.context.snapshot.checkout, isAbsolute(path) ? path : `${this.context.snapshot.checkout}${sep}${path}`);
    return inside && !inside.startsWith(`..${sep}`) && inside !== '..' && !isAbsolute(inside) ? inside : undefined;
  }

  private schedule() {
    if (this.timer) return;
    this.timer = setTimeout(() => { this.timer = undefined; this.render(); this.context.publish(); }, PUBLISH_MS);
  }

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
