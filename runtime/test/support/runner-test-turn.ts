import { access, writeFile } from 'node:fs/promises';
import type { RunnerTurn } from '../../src/runner/main.ts';
import type { Command } from '../../src/runner/protocol.ts';

export function createTurn(): RunnerTurn {
  let approve: ((allow: boolean) => void) | undefined;
  let steerFile: string | undefined;
  const acceptedSteers = new Set<string>();
  let report: Parameters<RunnerTurn['start']>[1] | undefined;
  return {
    async start(payload, emit, signal) {
      report = emit;
      if (typeof payload.pid_file === 'string') await writeFile(payload.pid_file, String(process.pid));
      if (typeof payload.steer_file === 'string') steerFile = payload.steer_file;
      emit({ type: 'phase', state: 'running' });
      emit({ type: 'live', payload: { text: 'working' } });
      emit({ type: 'commit', item: {}, head: null, placed: [], changed_paths: [], usage: null });
      let allowed = true;
      if (payload.skip_approval !== true) {
        emit({ type: 'approval_needed', approval: { id: 'approval', tool_id: 'tool', kind: 'edit', summary: 'Edit', details: '', dirty_paths: [], risk: '' } });
        allowed = await new Promise<boolean>(resolve => {
          approve = resolve; signal.addEventListener('abort', () => resolve(false), { once: true });
          if (signal.aborted) resolve(false);
        });
        if (signal.aborted) return { status: 'stopped' };
        emit({ type: 'approval_settled', id: 'approval' });
      }
      while (typeof payload.gate === 'string' && !signal.aborted) {
        try { await access(payload.gate); break; } catch { await new Promise(resolve => setTimeout(resolve, 25)); }
      }
      if (signal.aborted) return { status: 'stopped' };
      if (typeof payload.after_gate_live === 'string') emit({ type: 'live', payload: { text: payload.after_gate_live } });
      if (payload.after_gate_postimage && typeof payload.after_gate_postimage === 'object') {
        emit({ type: 'postimage', postimage: payload.after_gate_postimage });
      }
      return { status: 'finished', allowed };
    },
    command(command: Command) {
      if (command.type === 'approve' && command.approval_id === 'approval') approve?.(command.allow);
      if (command.type === 'steer' && steerFile) {
        if (acceptedSteers.has(command.request_id)) return;
        acceptedSteers.add(command.request_id);
        void writeFile(steerFile, JSON.stringify(command)).catch(() => {});
        // Like the Pi runner, report the input as placed so the host stops queuing it.
        const entry = 1000 + acceptedSteers.size;
        report?.({ type: 'commit', item: { placed_entries: [{ request_id: command.request_id, entry }] }, head: null, placed: [command.request_id], changed_paths: [], usage: null });
      }
    },
  };
}
