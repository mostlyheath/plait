import { validIdentity } from './protocol.ts';
// Cleanup is a proof obligation, separate from a tool or turn's success.
import { spawn, type ChildProcess } from 'node:child_process';
import { isAbsolute } from 'node:path';
import { fail, HostError } from './protocol.ts';
import type { LeaseHandle } from './safety.ts';

export type CleanupState = { token: string; reason: string; process?: { group: number; command: string; can_stop: boolean } };
export type CleanupReply = { checkout: string; cleanup: CleanupState | null };
export const RESTART_REASON = "The host ended before it confirmed this run's cleanup.";
export const NEVER = new Promise<void>(() => {});

export class CleanupProof {
  private pending = new Map<Promise<void>, string>();
  track(proof: Promise<void>, reason: string) {
    // A failed check is not exit evidence. Keep it pending for a person.
    const checked = proof.catch(() => NEVER);
    this.pending.set(checked, reason);
    void checked.then(() => this.pending.delete(checked));
  }
  reason() { return this.pending.values().next().value as string | undefined; }
  proved() { return Promise.all([...this.pending.keys()]).then(() => {}); }
  async wait(ms = 2500) {
    let timer: NodeJS.Timeout | undefined;
    await Promise.race([this.proved(), new Promise<void>(resolve => { timer = setTimeout(resolve, ms); })]);
    clearTimeout(timer);
  }
}

/** A process we started in its own group. Exit alone does not prove cleanup. */
export function groupExited(child: ChildProcess): Promise<void> {
  return new Promise(resolve => {
    child.once('error', () => { if (child.pid === undefined) resolve(); });
    child.once('exit', () => {
      if (child.pid === undefined) return resolve();
      try { process.kill(-child.pid, 'SIGKILL'); } catch { /* The check below decides. */ }
      const check = () => {
        try { process.kill(-child.pid!, 0); }
        catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return resolve(); }
        setTimeout(check, 100).unref();
      };
      check();
    });
  });
}

/** Guardian fd 8 is private: only verified group quiescence writes this receipt. */
export function guardianProof(child: ChildProcess): Promise<void> {
  return new Promise(resolve => {
    const receipt = (child.stdio as (import('node:stream').Readable | null | undefined)[])[8];
    let buffer = ''; let group: number | undefined; let polling = false;
    let exited = child.exitCode !== null || child.signalCode !== null;
    const poll = () => {
      if (!exited || group === undefined || polling) return;
      polling = true;
      const check = () => {
        try { process.kill(-group!, 0); }
        catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return resolve(); }
        setTimeout(check, 100).unref();
      };
      check();
    };
    receipt?.on('data', (chunk: Buffer) => {
      buffer += chunk.toString('utf8');
      if (buffer.length > 128) { receipt.destroy(); return; }
      for (;;) {
        const end = buffer.indexOf('\n');
        if (end < 0) break;
        const line = buffer.slice(0, end); buffer = buffer.slice(end + 1);
        if (line === 'clean') resolve();
        else if (/^group:[1-9][0-9]{0,9}$/.test(line)) { group = Number(line.slice(6)); poll(); }
      }
    });
    child.once('exit', () => { exited = true; poll(); });
    child.once('error', () => { if (child.pid === undefined) resolve(); });
  });
}

export function cleanupHelper(action: 'read' | 'begin' | 'mark' | 'finish' | 'resolve' | 'stop', root: string,
  token?: string, reason?: string, lease?: LeaseHandle): Promise<CleanupReply> {
  const executable = process.env.PLAIT_HOST_EXE;
  if (!executable) fail('internal', 'Host cleanup helper is not configured.');
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ['cleanup', action, root], {
      stdio: ['pipe', 'pipe', 'pipe', 'ignore', lease?.fd ?? 'ignore', 'ignore', 'ignore', lease?.lifecycleFd ?? 'ignore'],
    });
    let out = ''; let diagnostic = '';
    child.stdin!.on('error', () => {});
    child.stdin!.end(JSON.stringify({ token, reason }));
    child.stdout!.setEncoding('utf8').on('data', data => { out = (out + data).slice(-16384); });
    child.stderr!.setEncoding('utf8').on('data', data => { diagnostic = (diagnostic + data).slice(-2048); });
    child.once('error', reject);
    child.once('close', code => {
      // Failing to read a record, or to save one before a run starts, leaves
      // nothing unconfirmed: only a saved record holds the checkout.
      const failure = action === 'read' || action === 'begin' ? 'permission_denied' : 'cleanup_uncertain';
      if (code !== 0) return reject(new HostError(failure, diagnostic.trim().replace(/^plait-host:\s*/, '') || 'Cleanup state could not be verified.'));
      try { resolve(JSON.parse(out)); } catch { reject(new HostError('internal', 'Invalid cleanup helper reply.')); }
    });
  });
}


/** Late proof can outlive the runtime store, but may clear only its own marker. */
export async function resolveProvedCleanup(proof: CleanupProof, root: string, token: string): Promise<boolean> {
  await proof.proved();
  for (;;) {
    try { return (await cleanupHelper('resolve', root, token)).cleanup === null; }
    catch {
      if ((await cleanupHelper('read', root)).cleanup?.token !== token) return false;
      // The guardian may still hold the lease briefly after sending its receipt.
      await new Promise<void>(resolve => setTimeout(resolve, 100).unref());
    }
  }
}

export function handlesCleanup(input: unknown) {
  return !!input && typeof input === 'object' && ['checkout_cleanup', 'resolve_checkout_cleanup', 'stop_checkout_cleanup'].includes(String((input as { op?: unknown }).op));
}
export function validateCleanup(input: unknown): { op: string; checkout: string; token?: string; identity?: unknown } {
  if (!input || typeof input !== 'object' || Array.isArray(input)) fail('invalid_request', 'Invalid cleanup request.');
  const request = input as Record<string, unknown>;
  const resolve = request.op === 'resolve_checkout_cleanup';
  const stop = request.op === 'stop_checkout_cleanup';
  const confirmation = stop ? 'confirm_stop' : 'confirm_stopped';
  const fields = resolve || stop ? ['op', 'checkout', 'token', confirmation] : ['op', 'checkout', 'identity'];
  if (!handlesCleanup(request) || Object.keys(request).some(key => !fields.includes(key))
      || typeof request.checkout !== 'string' || !isAbsolute(request.checkout) || request.checkout.length > 4096
      || /[\u0000-\u001f\u007f]/u.test(request.checkout)
      || (resolve || stop) && (request[confirmation] !== true || typeof request.token !== 'string' || !/^[A-Za-z0-9-]{1,128}$/.test(request.token))) {
    fail('invalid_request', 'Invalid cleanup request. Confirm the processes stopped and use the current cleanup token.');
  }
  if (request.identity !== undefined && !validIdentity(request.identity)) fail('invalid_request', 'Invalid checkout identity.');
  return request as { op: string; checkout: string; token?: string; identity?: unknown };
}
