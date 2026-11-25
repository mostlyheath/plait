import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import type { Conversation, ConversationRetryPolicy, ConversationWatch, Harness, Submission, SubmissionDraft } from '@earendil-works/pi-durable';
import type { Models } from '@earendil-works/pi-ai';
import { RunAuthorization } from '../authorization.ts';
import { NEVER } from '../cleanup.ts';
import type { CommandPolicy } from '../command/policy.ts';
import type { ImageData, ImageRef } from '../images.ts';
import { fingerprint, nameKey } from '../mcp/tools.ts';
import { loadRunModels, piProviders } from '../pi-providers.ts';
import { failure, type InputRecord, type Snapshot, type ThinkingLevel } from '../protocol.ts';
import { capture, RECORD_FAILED, type Made } from '../records.ts';
import { resourceFailure } from '../run-capacity.ts';
import { gitCheckout, runHeader, type FileIdentity, type Postimage } from '../safety.ts';
import { discover, expandInput } from '../skills.ts';
import type { TemplateEntry } from '../templates.ts';
import { CommandOutputs, threadOutputsPath } from '../tool-output.ts';
import { registry, type ToolState } from '../tools.ts';
import { TurnExecutor, TurnSnapshotApplier, toolStateFor, type PiTurnInput, type TurnEvent, type TurnSink } from '../turn-executor.ts';
import type { RunnerTurn, RunnerTurnContext } from './main.ts';
import type { Command, Event } from './protocol.ts';
import { importTestSupport } from './test-support.ts';

export type PiTurnStartPayload = {
  kind: 'pi';
  thread_id: string;
  request_id: string;
  text: string;
  checkout: string;
  vcs: 'git' | 'folder';
  model: { provider: string; modelId: string };
  thinking_level: ThinkingLevel | null;
  max_turns: number;
  retry: ConversationRetryPolicy;
  approval_mode: 'auto' | 'manual';
  command_policy?: CommandPolicy;
  root_identity: FileIdentity;
  agent_dir?: string;
};

type PlacedEntry = { request_id: string; entry: number };

type TestProviderOverride = {
  models: Models;
  retry?: ConversationRetryPolicy;
  maxTurns?: number;
  resources?: typeof discover;
  registry?: typeof registry;
};

const steerKey = (requestId: string) => `steer:${requestId}`;

/** Reads initial thread context from host.sqlite read-only; detached runners never write host.sqlite. */
function readHostThreadState(store: string, threadId: string): {
  inputs: InputRecord[];
  priorOwned: Postimage[];
  mcpSettings: Record<string, string>;
  toolLabels: Map<string, string>;
} {
  const hostDbPath = join(store, 'host.sqlite');
  if (!existsSync(hostDbPath)) {
    return { inputs: [], priorOwned: [], mcpSettings: {}, toolLabels: new Map() };
  }
  const db = new DatabaseSync(hostDbPath, { readOnly: true, timeout: 5000 });
  try {
    const requestImages = (requestId: string): ImageRef[] => {
      try {
        const row = db.prepare('SELECT ids FROM message_images WHERE thread=? AND request=?').get(threadId, requestId) as { ids?: string } | undefined;
        if (!row?.ids) return [];
        const ids = JSON.parse(String(row.ids)) as string[];
        return ids.flatMap(id => {
          const img = db.prepare('SELECT metadata FROM images WHERE thread=? AND id=?').get(threadId, id) as { metadata?: string } | undefined;
          return img?.metadata ? [JSON.parse(String(img.metadata)) as ImageRef] : [];
        });
      } catch {
        return [];
      }
    };
    const inputs: InputRecord[] = db.prepare(
      "SELECT id,input,entry,anchor,state,queue FROM requests WHERE scope=? AND input IS NOT NULL AND (state IS NULL OR state='queued') ORDER BY rowid DESC LIMIT 100",
    ).all(threadId).reverse().map(row => {
      const requestId = String(row.id);
      const images = requestImages(requestId);
      return {
        request_id: requestId,
        text: JSON.parse(String(row.input)),
        entry: row.entry === null ? null : Number(row.entry),
        anchor: Number(row.anchor ?? 0),
        ...(row.state === 'queued' ? { queued: JSON.parse(String(row.queue)) } : {}),
        ...(images.length ? { images } : {}),
      };
    });
    const priorOwned: Postimage[] = db.prepare('SELECT image FROM postimages WHERE thread_id=? LIMIT 500')
      .all(threadId)
      .map(row => JSON.parse(String(row.image)))
      .filter((image): image is Postimage => image !== null && typeof image === 'object');
    const mcpSettings = Object.fromEntries(
      db.prepare("SELECT key, value FROM settings WHERE key LIKE 'mcp_server:%'")
        .all()
        .map(row => [String(row.key), String(row.value)]),
    );
    const labelPrefix = `mcp_name:${threadId}:`;
    const toolLabels = new Map<string, string>();
    for (const row of db.prepare('SELECT key, value FROM settings WHERE key LIKE ?').all(`${labelPrefix}%`)) {
      const key = String(row.key);
      toolLabels.set(key.slice(labelPrefix.length), String(row.value));
    }
    return { inputs, priorOwned, mcpSettings, toolLabels };
  } finally {
    db.close();
  }
}

function readRequestImages(store: string, threadId: string, requestId: string): ImageData[] {
  const hostDbPath = join(store, 'host.sqlite');
  if (!existsSync(hostDbPath)) return [];
  const db = new DatabaseSync(hostDbPath, { readOnly: true, timeout: 5000 });
  try {
    const row = db.prepare('SELECT ids FROM message_images WHERE thread=? AND request=?').get(threadId, requestId) as { ids?: string } | undefined;
    if (!row?.ids) return [];
    const ids = JSON.parse(String(row.ids)) as string[];
    const result: ImageData[] = [];
    for (const id of ids) {
      const img = db.prepare('SELECT bytes, metadata FROM images WHERE thread=? AND id=?').get(threadId, id) as { bytes?: Uint8Array; metadata?: string } | undefined;
      if (!img?.bytes || !img.metadata) continue;
      result.push({ ref: JSON.parse(String(img.metadata)) as ImageRef, data: Buffer.from(img.bytes).toString('base64') });
    }
    return result;
  } catch {
    return [];
  } finally {
    db.close();
  }
}

/** Coalesces Pi TurnExecutor updates onto the runner event stream like InProcessSink. */
export class RunnerTurnSink implements TurnSink {
  readonly snapshot: Snapshot;
  private readonly applier: TurnSnapshotApplier;
  private readonly emitEvent: (event: Omit<Event, 'seq'>) => void;
  private readonly toolLabels: Map<string, string>;
  private timer?: ReturnType<typeof setTimeout>;
  private pendingHead?: number;
  private pendingLive = false;
  private pendingPlaced: PlacedEntry[] = [];
  private boundary = '';
  private savedBoundary = '';
  private finished = false;
  head = 0;

  constructor(snapshot: Snapshot, emitEvent: (event: Omit<Event, 'seq'>) => void, toolLabels: Map<string, string>) {
    this.snapshot = snapshot;
    this.applier = new TurnSnapshotApplier(snapshot);
    this.emitEvent = emitEvent;
    this.toolLabels = toolLabels;
  }

  queuePlaced(requestId: string, entry: number): void {
    this.pendingPlaced.push({ request_id: requestId, entry });
  }

  flush(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
    const hasLive = this.pendingLive;
    const head = this.pendingHead;
    const placed = this.pendingPlaced;
    this.pendingLive = false;
    this.pendingHead = undefined;
    this.pendingPlaced = [];
    if (hasLive) {
      this.emitEvent({
        type: 'live',
        payload: {
          text: this.snapshot.live_text,
          reasoning: this.snapshot.live_reasoning ?? '',
          messages: this.snapshot.messages,
          tools: this.snapshot.tools,
          usage: this.snapshot.usage ?? null,
          retry: this.snapshot.retry ?? null,
          truncated: this.snapshot.truncated,
          error: this.snapshot.error ?? null,
        },
      });
    }
    if (head !== undefined || placed.length > 0) {
      const nextHead = head !== undefined ? Math.max(this.head, head) : this.head;
      this.head = nextHead;
      this.emitEvent({
        type: 'commit',
        item: placed.length ? { placed_entries: placed } : {},
        head: nextHead > 0 ? String(nextHead) : null,
        placed: placed.map(entry => entry.request_id),
        changed_paths: [],
        usage: this.snapshot.usage ?? null,
      });
    }
    this.savedBoundary = this.boundary;
  }

  private scheduleCommit(head: number): void {
    if (this.finished) return;
    this.pendingHead = Math.max(this.pendingHead ?? 0, head);
    if (this.boundary !== this.savedBoundary || this.pendingPlaced.length > 0) {
      this.flush();
      return;
    }
    if (!this.timer) this.timer = setTimeout(() => this.flush(), 50);
  }

  emit(event: Extract<TurnEvent, { type: 'record_before' }>): Promise<string>;
  emit(event: Extract<TurnEvent, { type: 'record_after' }>): Promise<void>;
  emit(event: Exclude<TurnEvent, { type: 'record_before' | 'record_after' }>): void;
  emit(event: TurnEvent): void | Promise<string> | Promise<void> {
    switch (event.type) {
      case 'phase':
        this.flush();
        this.emitEvent({ type: 'phase', state: event.phase });
        break;
      case 'live':
        if (this.finished) break;
        this.applier.live(event.projection, event.inputs, event.contextWindow);
        for (const tool of this.snapshot.tools) {
          if (tool.name.startsWith('mcp__')) tool.name = this.toolLabels.get(tool.name) ?? tool.name;
        }
        this.boundary = `${event.projection.entries.length}:${event.projection.entries.at(-1)?.id ?? ''}:${this.snapshot.tools.map(tool => `${tool.id}=${tool.status}`).join(',')}`;
        this.pendingLive = true;
        break;
      case 'commit':
        this.scheduleCommit(event.head);
        break;
      case 'approval_needed':
        this.flush();
        this.applier.approvalNeeded(event.approval);
        this.emitEvent({ type: 'approval_needed', approval: event.approval });
        break;
      case 'approval_settled':
        this.flush();
        this.applier.approvalSettled(event.id);
        this.emitEvent({ type: 'approval_settled', id: event.id });
        break;
      case 'postimage':
        this.flush();
        this.emitEvent({ type: 'postimage', postimage: event.image });
        break;
      case 'forget':
        this.flush();
        this.emitEvent({ type: 'forget', path: event.path });
        break;
      case 'finished':
        this.flush();
        this.finished = true;
        break;
      case 'record_before':
        return Promise.reject<string>(new Error('record_before is handled directly by pi-runner'));
      case 'record_after':
        return Promise.reject<void>(new Error('record_after is handled directly by pi-runner'));
    }
  }
}

async function loadPiModels(store: string, agentDir: string | undefined, provider: string, signal: AbortSignal): Promise<TestProviderOverride> {
  // Test-only provider injection requires both PLAIT_TEST_SCENARIO and PLAIT_RUNNER_TEST_TURNS=1.
  if (process.env.PLAIT_RUNNER_TEST_TURNS === '1' && process.env.PLAIT_TEST_SCENARIO) {
    const mod = await importTestSupport<typeof import('../../test/support/runner-test-provider.ts')>('runner-test-provider.ts');
    return mod.loadTestProvider(store, process.env.PLAIT_TEST_SCENARIO);
  }
  const loaded = await loadRunModels(piProviders(agentDir), provider, signal);
  return { models: loaded.models };
}

export function createPiTurn(): RunnerTurn {
  const executor = new TurnExecutor();
  let authorization: RunAuthorization | undefined;
  let root: Conversation | undefined;
  let triggerSteer: (() => void) | undefined;
  let addSteerInput: ((requestId: string, text: string, images: ImageRef[]) => void) | undefined;

  return {
    async start(rawPayload: Record<string, unknown>, emit: (event: Omit<Event, 'seq'>) => void, signal: AbortSignal, runnerContext: RunnerTurnContext): Promise<unknown> {
      const payload = rawPayload as unknown as PiTurnStartPayload;
      const { store, lease } = runnerContext;
      const threadId = payload.thread_id;
      const requestId = payload.request_id;
      const hostState = readHostThreadState(store, threadId);
      const inputs = hostState.inputs;
      const toolLabels = hostState.toolLabels;

      const shadowSnapshot: Snapshot = {
        id: threadId,
        checkout: payload.checkout,
        model: { provider: payload.model.provider, model_id: payload.model.modelId },
        status: 'running',
        messages: [],
        live_text: '',
        tools: [],
        pending_approval: null,
        error: null,
        truncated: false,
        approval_mode: payload.approval_mode === 'auto' ? 'auto' : 'ask',
        thinking_level: payload.thinking_level ?? undefined,
      };

      const sink = new RunnerTurnSink(shadowSnapshot, emit, toolLabels);
      executor.connect(sink);

      const outputsDb = new DatabaseSync(threadOutputsPath(store, threadId), { timeout: 5000 });
      outputsDb.exec('PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;');
      const outputs = new CommandOutputs(outputsDb, { fallbackDbPath: join(store, 'host.sqlite') });

      authorization = new RunAuthorization(payload.approval_mode === 'auto' ? { async authorize() {} } : executor);
      const onAbort = () => {
        authorization?.revoke();
        void root?.abort(context).catch(() => {});
      };
      signal.addEventListener('abort', onAbort, { once: true });
      if (signal.aborted) onAbort();

      let harness: Harness | undefined;
      let watch: ConversationWatch | undefined;
      let started = false;
      let templates: TemplateEntry[] = [];
      let state: ToolState | undefined;
      const steered = new Map<string, Submission>();
      const steerAttempted = new Set<string>();
      let beforeRecordId: string | undefined;
      let beforeCommit: string | undefined;
      let runError: { code: string; message: string } | undefined;

      const piInput = async (reqId: string, text: string) => {
        const expanded = await expandInput(text, { skills: state?.skills, templates });
        const images = readRequestImages(store, threadId, reqId);
        return images.length
          ? [...(expanded ? [{ type: 'text' as const, text: expanded }] : []), ...images.map(image => ({ type: 'image' as const, mimeType: image.ref.media_type, data: image.data }))]
          : expanded;
      };

      const syncInputs = async (all = false) => {
        if (!harness || !root) return;
        for (const input of inputs) {
          const isSteered = steered.has(input.request_id);
          if (input.entry !== null || (!all && !isSteered && input.request_id !== requestId)) continue;
          if (input.queued && (input.queued.mode !== 'steer' || !(all || isSteered))) continue;
          const key = input.queued ? steerKey(input.request_id) : input.request_id;
          const record = await harness.commit(tx => tx.submissionByRequest(root!.id, key), context);
          if (record?.type === 'input' && record.entry !== undefined) {
            input.entry = Number(record.entry);
            delete input.queued;
            sink.queuePlaced(input.request_id, input.entry);
          }
        }
      };

      const steer = async () => {
        if (!started || !root || authorization!.signal.aborted) return;
        for (const input of inputs) {
          if (input.entry !== null || input.queued?.mode !== 'steer' || steered.has(input.request_id) || steerAttempted.has(input.request_id)) continue;
          // Mark synchronously before awaiting submission so resent steer commands are idempotent.
          steerAttempted.add(input.request_id);
          try {
            const submission = await root.submit(
              { type: 'input', requestId: steerKey(input.request_id), content: await piInput(input.request_id, input.text), whenBusy: 'steer', whenIdle: 'reject' },
              context,
            );
            steered.set(input.request_id, submission);
          } catch {
            steerAttempted.delete(input.request_id);
            /* The run is ending; input stays queued for the next run. */
          }
        }
      };

      addSteerInput = (reqId, text, images) => {
        if (inputs.some(item => item.request_id === reqId)) return;
        inputs.push({
          request_id: reqId,
          text,
          entry: null,
          anchor: sink.head,
          queued: { mode: 'steer', approval: 'ask' },
          ...(images.length ? { images } : {}),
        });
      };
      triggerSteer = () => { void steer(); };

      try {
        if (payload.vcs === 'git') await gitCheckout(payload.checkout);

        // Capture before-record before any model or tool runs.
        if (payload.vcs === 'git') {
          beforeRecordId = randomUUID();
          try {
            const made: Made = await capture(store, payload.checkout, beforeRecordId, undefined, false, lease);
            beforeCommit = made.commit;
            emit({
              type: 'record_before',
              record: { id: beforeRecordId, thread_id: threadId, request_id: requestId, checkout: payload.checkout, state: 'ready', made, created_at: Date.now() },
            });
          } catch {
            emit({
              type: 'record_before',
              record: { id: beforeRecordId, thread_id: threadId, request_id: requestId, checkout: payload.checkout, state: 'failed', created_at: Date.now() },
            });
            beforeRecordId = undefined;
            runError = { code: 'record_failed', message: RECORD_FAILED };
            return { status: 'error', error: runError };
          }
        }

        if (authorization.signal.aborted) return { status: 'stopped' };

        const provider = await loadPiModels(store, payload.agent_dir, payload.model.provider, authorization.signal);
        if (authorization.signal.aborted) return { status: 'stopped' };

        const header = await runHeader(payload.checkout, payload.vcs);
        const turnInput: PiTurnInput = {
          sessionFile: join(store, `${threadId}.sqlite`),
          checkout: payload.checkout,
          model: payload.model,
          maxTurns: provider.maxTurns ?? payload.max_turns,
          vcs: payload.vcs,
          priorOwned: hostState.priorOwned,
          baseline: [],
          protected: [],
          rootIdentity: payload.root_identity,
          commandPolicy: payload.command_policy,
          approvalMode: payload.approval_mode,
          directory: store,
          threadId,
          trusted: payload.command_policy === 'trusted',
          header,
          mcpSettings: hostState.mcpSettings,
          agentDir: payload.agent_dir,
        };

        state = toolStateFor(turnInput, {
          lease,
          buildCache: { store, thread: threadId },
          outputs,
        }, {
          sink,
          approval: executor,
          authorization,
        });

        await executor.prepare(turnInput, state);
        const resources = await executor.resources(turnInput, state, {
          observe: (entry, observation) => {
            emit({
              type: 'mcp_observed',
              checkout: payload.checkout,
              root_dev: payload.root_identity.dev,
              root_ino: payload.root_identity.ino,
              name: entry.name,
              fingerprint: fingerprint(entry),
              observation,
            });
          },
          remember: (name, label) => {
            if (!toolLabels.has(name)) {
              toolLabels.set(name, label);
              for (const tool of shadowSnapshot.tools) if (tool.name === name) tool.name = label;
              emit({ type: 'tool_label', name, label });
            }
          },
        }, provider.resources ?? discover);
        templates = resources.templates;

        const opened = await executor.open(turnInput, {
          models: provider.models,
          state,
          retry: provider.retry ?? payload.retry,
          registry: provider.registry ?? registry,
        });
        harness = opened.harness;
        root = opened.root;

        await syncInputs(true);
        watch = await executor.watch(
          root,
          sink,
          async () => { await syncInputs(); return inputs; },
          () => provider.models.getModel(payload.model.provider, payload.model.modelId)?.contextWindow ?? null,
        );

        const draft = async (): Promise<SubmissionDraft> => {
          const earlier = await harness!.commit(tx => tx.submissionByRequest(root!.id, steerKey(requestId)), context);
          if (earlier?.type !== 'input' || earlier.entry === undefined) {
            return { type: 'input', requestId, content: await piInput(requestId, payload.text) };
          }
          const entryNumber = Number(earlier.entry);
          for (const input of inputs) {
            if (input.request_id === requestId) {
              input.entry = entryNumber;
              delete input.queued;
            }
          }
          sink.queuePlaced(requestId, entryNumber);
          return { type: 'continue' };
        };

        await executor.run({
          root,
          instructions: resources.instructions,
          model: payload.model,
          thinkingLevel: payload.thinking_level,
          maxTurns: turnInput.maxTurns,
          signal: authorization.signal,
        }, sink, {
          activate: () => authorization!.activate(),
          draft,
          started: () => { started = true; },
          steer,
        });
      } catch (error) {
        if (!signal.aborted) {
          runError = resourceFailure(error) ?? failure(error);
        }
      } finally {
        signal.removeEventListener('abort', onAbort);
        authorization.revoke();
        await state?.mcp?.close().catch(() => lease.cleanup?.track(NEVER, 'The MCP connection did not confirm cleanup.'));
        await syncInputs(true).catch(() => {});
        sink.flush();
        try {
          await watch?.stop();
          if (harness && root && steered.size) {
            await syncInputs().catch(() => {});
            for (const submission of steered.values()) await submission.abort(context).catch(() => {});
          }
          await harness?.close(context);
          root = undefined;
        } catch {
          lease.cleanup?.track(NEVER, 'The harness did not confirm cleanup.');
        }

        // Wait for any tracked command/helper cleanup before taking record_after.
        await lease.cleanup?.wait();
        if (!lease.cleanup?.reason() && beforeRecordId && beforeCommit) {
          const afterRecordId = randomUUID();
          try {
            const made = await capture(store, payload.checkout, afterRecordId, beforeCommit, false, lease);
            emit({
              type: 'record_after',
              record: { id: afterRecordId, before: beforeRecordId, thread_id: threadId, request_id: requestId, checkout: payload.checkout, state: 'ready', made, created_at: Date.now() },
            });
          } catch {
            emit({
              type: 'record_after',
              record: { id: afterRecordId, before: beforeRecordId, thread_id: threadId, request_id: requestId, checkout: payload.checkout, state: 'failed', created_at: Date.now() },
            });
          }
          await lease.cleanup?.wait();
        }
        sink.emit({ type: 'finished' });
        try { outputsDb.close(); } catch { /* Best-effort close on exit. */ }
      }

      if (signal.aborted) return { status: 'stopped' };
      const finalError = runError ?? shadowSnapshot.error ?? undefined;
      if (finalError) return { status: 'error', error: finalError };
      return { status: 'idle' };
    },
    command(command: Command): void {
      if (command.type === 'approve') {
        try {
          executor.command({ type: 'approve', id: command.approval_id, allow: command.allow });
        } catch { /* Approval may already be settled or aborted. */ }
      } else if (command.type === 'stop') {
        authorization?.revoke();
        void root?.abort(context).catch(() => {});
      } else if (command.type === 'steer') {
        addSteerInput?.(command.request_id, command.text, command.images as ImageRef[]);
        triggerSteer?.();
      }
    },
  };
}
