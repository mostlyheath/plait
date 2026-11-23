import { access } from 'node:fs/promises';
import type { RunnerTurn } from '../../src/runner/main.ts';
import type { Command } from '../../src/runner/protocol.ts';

export function createTurn(): RunnerTurn {
  let approve: ((allow: boolean) => void) | undefined;
  return {
    async start(payload, emit, signal) {
      emit({ type: 'phase', state: 'running' });
      emit({ type: 'live', payload: { text: 'working' } });
      emit({ type: 'commit', item: {}, head: null, placed: [], changed_paths: [], usage: null });
      emit({ type: 'approval_needed', approval: { id: 'approval', tool_id: 'tool', kind: 'edit', summary: 'Edit', details: '', dirty_paths: [], risk: '' } });
      const allowed = await new Promise<boolean>(resolve => {
        approve = resolve; signal.addEventListener('abort', () => resolve(false), { once: true });
        if (signal.aborted) resolve(false);
      });
      if (signal.aborted) return { status: 'stopped' };
      emit({ type: 'approval_settled', id: 'approval' });
      while (typeof payload.gate === 'string' && !signal.aborted) {
        try { await access(payload.gate); break; } catch { await new Promise(resolve => setTimeout(resolve, 25)); }
      }
      return { status: signal.aborted ? 'stopped' : 'finished', allowed };
    },
    command(command: Command) { if (command.type === 'approve' && command.approval_id === 'approval') approve?.(command.allow); },
  };
}
