import type { Snapshot, InputRecord } from './protocol.ts';
import type { Store } from './store.ts';
import type { Records } from './records.ts';
import type { RunCapacity } from './run-capacity.ts';
import type { LeaseHandle } from './safety.ts';
import { nameKey } from './mcp/tools.ts';
import { update } from './transcript.ts';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { Harness } from '@earendil-works/pi-durable';
import type { Conversation, ConversationRetryPolicy, SubmissionDraft } from '@earendil-works/pi-durable';
import { createModels } from '@earendil-works/pi-ai/models';
import type { Models } from '@earendil-works/pi-ai';
import { SqliteStorage } from '@earendil-works/pi-durable/storage/sqlite';
import { openNodeSqliteDatabase } from '@earendil-works/pi-durable/storage/sqlite/node';
import { registry, type ToolState } from './tools.ts';
import { dirtyPaths, identities, pathKey } from './safety.ts';
import { restoreOwned } from './provenance.ts';
import type { Postimage } from './safety.ts';
import type { ThinkingLevel } from './protocol.ts';

export type PiTurnInput = {
  sessionFile: string; checkout: string; model: { provider: string; modelId: string };
  maxTurns: number; vcs: 'git' | 'folder'; priorOwned: Postimage[];
};

export type TurnInput = {
  root: Conversation;
  instructions: string;
  model: { provider: string; modelId: string };
  thinkingLevel: ThinkingLevel | null;
  maxTurns: number;
  signal: AbortSignal;
};

export interface TurnCommands {
  activate(): void;
  draft(): Promise<SubmissionDraft>;
  started(): void;
  steer(): Promise<void>;
}

/** Runs the configured Pi conversation; the caller retains its session and cleanup. */
export class TurnExecutor {
  async prepare(input: PiTurnInput, state: ToolState): Promise<void> {
    const baseline = await dirtyPaths(input.checkout, input.vcs);
    state.owned = await restoreOwned(input.checkout, baseline, input.priorOwned, state.rootIdentity, state.lease?.fd, state.lease?.lifecycleFd, input.vcs);
    state.baseline = baseline.filter(path => !state.owned.has(pathKey(path)));
    state.protected = await identities(input.checkout, state.baseline);
  }

  async open(input: PiTurnInput, dependencies: { models: Models; state: ToolState; retry: ConversationRetryPolicy; registry?: typeof registry }): Promise<{ harness: Harness; root: Conversation }> {
    const database = await openNodeSqliteDatabase(input.sessionFile);
    let harness: Harness;
    try {
      await database.exec('PRAGMA synchronous=FULL');
      const models = dependencies.models;
      harness = await Harness.open(await SqliteStorage.open(database), {
        models: () => dependencies.state.authorization.active ? models : createModels(),
        registry: (dependencies.registry ?? registry)(dependencies.state),
        // Only model requests retry; a tool call is unsafe to replay.
        settings: () => ({ retry: dependencies.retry, stream: { timeoutMs: 60000 }, toolExecution: 'sequential', steeringMode: 'all' }),
      }, context);
    } catch (error) { await database.close(); throw error; }
    try {
      const root = await harness.root(context, { agent: { model: input.model, cwd: input.checkout, maxTurns: input.maxTurns } });
      // Fence recovery before enabling scheduling: no model or tool can run here.
      const inspection = await harness.inspect(context);
      for (const task of inspection.tasks) await harness.abortTask(task.record.id, context);
      await root.abort(context);
      return { harness, root };
    } catch (error) { await harness.close(context); throw error; }
  }

  async run(input: TurnInput, sink: TurnSink, commands: TurnCommands): Promise<void> {
    await input.root.configure({ instructions: input.instructions, model: input.model,
      thinkingLevel: input.thinkingLevel, maxTurns: input.maxTurns }, context);
    if (input.signal.aborted) return;
    commands.activate();
    const submission = await input.root.submit(await commands.draft(), context);
    commands.started(); sink.emit({ type: 'phase', phase: 'running' });
    await commands.steer();
    await submission.wait(context);
    await input.root.waitForIdle(context);
  }
}

type Projection = Parameters<typeof update>[1];

/** In-process events preserve the synchronous ordering of the current runtime. */
export type TurnEvent =
  | { type: 'phase'; phase: 'running' }
  | { type: 'live'; projection: Projection; inputs: InputRecord[]; contextWindow: number | null }
  | { type: 'commit'; head: number }
  | { type: 'approval_needed' }
  | { type: 'record_before'; capacity: RunCapacity; records: Records; dev: string; requestId: string; signal: AbortSignal; lease?: LeaseHandle }
  | { type: 'record_after'; records: Records; record: string; lease: LeaseHandle }
  | { type: 'finished' };

export interface TurnSink {
  emit(event: Extract<TurnEvent, { type: 'record_before' }>): Promise<string>;
  emit(event: Extract<TurnEvent, { type: 'record_after' }>): Promise<void>;
  emit(event: Exclude<TurnEvent, { type: 'record_before' | 'record_after' }>): void;
}

export class InProcessSink implements TurnSink {
  private snapshot: Snapshot;
  private store: Store;
  private save: (head?: number) => void;
  private phase: () => void;
  private finish: () => void;

  constructor(snapshot: Snapshot, store: Store, save: (head?: number) => void, phase: () => void, finish: () => void) {
    this.snapshot = snapshot; this.store = store; this.save = save; this.phase = phase; this.finish = finish;
  }

  emit(event: Extract<TurnEvent, { type: 'record_before' }>): Promise<string>;
  emit(event: Extract<TurnEvent, { type: 'record_after' }>): Promise<void>;
  emit(event: Exclude<TurnEvent, { type: 'record_before' | 'record_after' }>): void;
  emit(event: TurnEvent): void | Promise<string> | Promise<void> {
    switch (event.type) {
      case 'phase': this.phase(); break;
      case 'live':
        update(this.snapshot, event.projection, event.inputs, event.contextWindow);
        for (const tool of this.snapshot.tools) if (tool.name.startsWith('mcp__')) tool.name = this.store.setting(nameKey(this.snapshot.id, tool.name)) ?? tool.name;
        break;
      case 'commit': this.save(event.head); break;
      case 'approval_needed': this.save(); break;
      case 'record_before': return event.capacity.prepare(event.dev, this.snapshot.id, event.signal,
        () => event.records.before(this.snapshot.id, event.requestId, this.snapshot.checkout, event.lease));
      case 'record_after': return event.records.after(event.record, event.lease);
      case 'finished': this.finish(); break;
    }
  }
}
