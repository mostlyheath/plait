import { readFile } from 'node:fs/promises';
import { CleanupProof, cleanupHelper } from '../cleanup.ts';
import { adoptDescriptors, type CheckoutLease } from '../safety.ts';
import { RunnerRegistry, type Manifest } from './registry.ts';
import { RunnerServer } from './socket.ts';
import { receiptEvents, type Command, type Event, type RunnerState } from './protocol.ts';
import { createPiTurn } from './pi-runner.ts';
import { importTestSupport } from './test-support.ts';

export type RunnerTurnContext = {
  store: string;
  manifest: Manifest;
  lease: CheckoutLease;
};

export type RunnerTurn = {
  start(payload: Record<string, unknown>, emit: (event: Omit<Event, 'seq'>) => void, signal: AbortSignal, context: RunnerTurnContext): Promise<unknown>;
  command(command: Command): void;
};

const turnKinds: Record<string, () => Promise<RunnerTurn>> = {
  pi: async () => createPiTurn(),
  test: async () => {
    if (process.env.PLAIT_RUNNER_TEST_TURNS !== '1') throw new Error('Unknown runner turn kind');
    const mod = await importTestSupport<typeof import('../../test/support/runner-test-turn.ts')>('runner-test-turn.ts');
    return mod.createTurn();
  },
};

async function createTurnForKind(kind: unknown): Promise<RunnerTurn> {
  if (typeof kind !== 'string' || !Object.hasOwn(turnKinds, kind)) {
    throw new Error('Unknown runner turn kind');
  }
  return turnKinds[kind]();
}

async function main(): Promise<void> {
  const store = process.env.PLAIT_RUNNER_STORE; const id = process.env.PLAIT_RUNNER_ID;
  if (!store || !id) throw new Error('Runner store and id are required');
  const registry = new RunnerRegistry(store);
  const manifest = JSON.parse(await readFile(registry.path(id, '.json'), 'utf8')) as Manifest;
  if (manifest.run_id !== id || !manifest.lease || manifest.checkout !== manifest.lease.root) throw new Error('Invalid runner lease expectation');
  const lock = await registry.acquire(id);
  let lease: CheckoutLease | undefined;
  let server: RunnerServer | undefined;
  try {
    lease = await adoptDescriptors([3, 4, 5], manifest.lease);
    lease.cleanup = new CleanupProof();
    let state: RunnerState = 'preparing'; let seq = 0; let started = false; let attached = false;
    let pendingApproval: unknown = null;
    let head: string | null = null;
    let beforeRecordId: string | null = null;
    let liveSnapshot: Record<string, unknown> = {};
    let turn: RunnerTurn | undefined; const abort = new AbortController();
    let finishAck: (() => void) | undefined; let finalSeq = 0;
    let done!: () => void; const completed = new Promise<void>(resolve => { done = resolve; });
    const emit = (event: Omit<Event, 'seq'>) => {
      const next = { ...event, seq: ++seq } as Event;
      if (next.type === 'phase') state = next.state as RunnerState;
      else if (next.type === 'approval_needed') { state = 'awaiting_approval'; pendingApproval = next.approval; }
      else if (next.type === 'approval_settled') { pendingApproval = null; if (state === 'awaiting_approval') state = 'running'; }
      else if (next.type === 'live' && next.payload && typeof next.payload === 'object') { liveSnapshot = next.payload as Record<string, unknown>; }
      else if (next.type === 'commit' && typeof next.head === 'string') { head = next.head; }
      else if (next.type === 'record_before' && next.record && typeof (next.record as { id?: unknown }).id === 'string') { beforeRecordId = String((next.record as { id: string }).id); }
      server!.publish(next);
    };
    const finish = async (result: unknown) => {
      state = 'finishing';
      let finalResult: Record<string, unknown> = result && typeof result === 'object' ? { ...(result as Record<string, unknown>) } : { value: result };
      await lease?.cleanup?.wait();
      const reason = lease?.cleanup?.reason();
      if (manifest.cleanup_token && lease) {
        try {
          if (!reason) {
            await cleanupHelper('finish', manifest.checkout, manifest.cleanup_token, undefined, lease);
          } else {
            await cleanupHelper('mark', manifest.checkout, manifest.cleanup_token, reason, lease);
            finalResult = {
              ...finalResult,
              status: 'error',
              error: { code: 'cleanup_uncertain', message: `Stop unconfirmed. ${reason} New runs and Git changes are blocked until cleanup is confirmed.` },
            };
          }
        } catch {
          finalResult = {
            ...finalResult,
            status: 'error',
            error: { code: 'cleanup_uncertain', message: 'Stop unconfirmed. The host could not save cleanup confirmation. New runs and Git changes remain blocked.' },
          };
        }
      }
      const receipt = { result: finalResult, events: receiptEvents(server!.unacknowledged()) };
      await registry.writeReceipt(id, receipt);
      emit({ type: 'finished', result: finalResult }); finalSeq = seq;
      await new Promise<void>(resolve => { finishAck = resolve; setTimeout(resolve, 2000).unref(); });
      done();
    };
    server = new RunnerServer(registry.socketPath(id), () => ({ type: 'attached', state, seq, snapshot: liveSnapshot, pending_approval: pendingApproval, head, before_record_id: beforeRecordId }), command => {
      if (command.type === 'ack') { if (finalSeq && command.seq >= finalSeq) finishAck?.(); return; }
      if (command.type === 'ping') { emit({ type: 'pong', nonce: command.nonce }); return; }
      if (command.type === 'stop') {
        abort.abort();
        // If the host aborts launch before `start` (e.g. acceptance failed), clean up and exit immediately.
        if (!started) {
          void (async () => {
            if (manifest.cleanup_token && lease) {
              await cleanupHelper('finish', manifest.checkout, manifest.cleanup_token, undefined, lease).catch(() => {});
            }
            done();
          })().catch(() => done());
          return;
        }
      }
      if (command.type === 'start') {
        if (started || !attached) return;
        started = true;
        void (async () => {
          try {
            const payload = command.payload as Record<string, unknown>;
            turn = await createTurnForKind(payload.kind);
            await finish(await turn.start(payload, emit, abort.signal, { store, manifest, lease: lease! }));
          } catch (error) { await finish({ status: abort.signal.aborted ? 'stopped' : 'error', error: String(error) }); }
        })().catch(() => done());
      } else turn?.command(command);
    }, () => { attached = true; });
    await server.listen();
    const orphanMs = process.env.PLAIT_RUNNER_TEST_TURNS === '1' && process.env.PLAIT_RUNNER_TEST_ORPHAN_MS ? Number(process.env.PLAIT_RUNNER_TEST_ORPHAN_MS) : 15000;
    const orphan = setTimeout(() => { if (!started) done(); }, orphanMs);
    await completed; clearTimeout(orphan);
  } finally {
    await server?.close(); await lease?.close(); await lock.close();
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
