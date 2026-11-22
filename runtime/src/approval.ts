import { escapeDisplay } from './text.ts';
import { randomUUID } from 'node:crypto';
import type { AuthorizationPolicy, EffectCall } from './authorization.ts';
import { fail } from './protocol.ts';
import type { Approval, Snapshot } from './protocol.ts';
import { TRUSTED_COMMAND_SUMMARY, TRUSTED_RISK } from './command/policy.ts';

type SettlementFailure = { error: unknown };
type PendingApproval = {
  id: string;
  signal: AbortSignal;
  settle(allow: boolean): SettlementFailure | undefined;
};

/**
 * The run's user chose automatic approval at submission. Authorization still
 * requires an active run, and file protections and command containment still
 * apply after this policy, in tools.ts and its helpers.
 */
export const automaticApproval: AuthorizationPolicy = {
  async authorize(_call: EffectCall, signal: AbortSignal) {
    if (signal.aborted) fail('permission_denied', 'Tool permission denied or revoked.');
  },
};

export class InteractiveApproval implements AuthorizationPolicy {
  private pending?: PendingApproval;
  private readonly snapshot: Snapshot;
  private readonly save: () => void;

  constructor(snapshot: Snapshot, save: () => void) {
    this.snapshot = snapshot;
    this.save = save;
  }

  async authorize(call: EffectCall, signal: AbortSignal): Promise<void> {
    if (!await this.ask(describeApproval(call), signal)) fail('permission_denied', 'Tool permission denied or revoked.');
  }

  /**
   * Shows one approval and waits for the person's answer. False when denied or
   * when `signal` ends the wait. Another harness's own requests come through here.
   */
  async ask(approval: Approval, signal: AbortSignal): Promise<boolean> {
    approval = { ...approval, details: escapeDisplay(approval.details), summary: escapeDisplay(approval.summary) };
    if (signal.aborted) fail('permission_denied', 'Tool permission denied or revoked.');
    if (this.pending) fail('busy', 'Another tool is awaiting approval.');
    const allowed = await new Promise<boolean>((resolve, reject) => {
      const clear = () => {
        signal.removeEventListener('abort', cancel);
        if (this.pending !== pending) return;
        this.pending = undefined;
        if (this.snapshot.pending_approval?.id === approval.id) {
          this.snapshot.pending_approval = null;
          if (this.snapshot.status === 'awaiting_approval') this.snapshot.status = 'running';
        }
      };
      const settle = (allow: boolean): SettlementFailure | undefined => {
        if (this.pending !== pending) return;
        clear();
        try {
          this.save();
          resolve(allow);
        } catch (error) {
          reject(error);
          return { error };
        }
      };
      // Cancellation reports save failures through the waiter, not an event callback.
      const cancel = () => { settle(false); };
      const pending: PendingApproval = { id: approval.id, signal, settle };
      this.pending = pending;
      this.snapshot.pending_approval = approval;
      this.snapshot.status = 'awaiting_approval';
      signal.addEventListener('abort', cancel, { once: true });
      try {
        this.save();
        if (signal.aborted) cancel();
      } catch (error) {
        clear();
        reject(error);
      }
    });
    return allowed && !signal.aborted;
  }

  answer(id: string, allow: boolean): void {
    const pending = this.pending;
    if (!pending || pending.id !== id || pending.signal.aborted || this.snapshot.pending_approval?.id !== id) {
      fail('not_found', 'Approval is stale, stopped, or already answered.');
    }
    const failure = pending.settle(allow);
    if (failure) throw failure.error;
  }
}

export function describeApproval(call: EffectCall): Approval {
  if (call.mcp) return {
    id: randomUUID(), tool_id: call.toolId, kind: 'command',
    summary: `Use ${call.mcp.tool} from ${call.mcp.server}`,
    details: JSON.stringify(call.arguments), dirty_paths: [],
    risk: call.commandPolicy === 'trusted'
      ? 'Only this exact MCP call is approved. The server has full user access and network access.'
      : 'Only this exact MCP call is approved. The server can use the network. Local servers can read the checkout and command toolchain paths; writes are limited to private scratch and the thread build cache. Server hints never waive approval.',
  };
  if (call.commandPolicy === 'trusted') return {
    id: randomUUID(), tool_id: call.toolId, kind: call.name === 'command' ? 'command' : 'edit',
    summary: call.name === 'command' ? TRUSTED_COMMAND_SUMMARY : `Change ${call.arguments.path}`,
    details: JSON.stringify(call.arguments), dirty_paths: [...call.dirtyPaths], risk: TRUSTED_RISK,
  };
  return {
    id: randomUUID(), tool_id: call.toolId, kind: call.name === 'command' ? 'command' : 'edit',
    summary: call.name === 'command' ? 'Run sandboxed command' : `Change ${call.arguments.path}`,
    details: JSON.stringify(call.arguments), dirty_paths: [...call.dirtyPaths],
    risk: 'Only this exact call is approved. Dirty and ignored paths are protected. Commands write only private scratch and a private per-thread build cache; no external network or checkout/Git writes. Tests may use scratch-local Git repositories and Unix sockets. Read tools and commands can read ignored checkout files and send their contents to the provider.',
  };
}
