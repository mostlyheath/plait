import type { Snapshot, InputRecord } from './protocol.ts';
import type { Store } from './store.ts';
import type { Records } from './records.ts';
import type { RunCapacity } from './run-capacity.ts';
import type { LeaseHandle } from './safety.ts';
import { nameKey } from './mcp/tools.ts';
import { update } from './transcript.ts';

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
