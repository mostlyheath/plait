import { spawn } from 'node:child_process';
import { closeSync, constants, openSync } from 'node:fs';
import { handoffDescriptors, type CheckoutLease } from '../safety.ts';

/**
 * `log` receives the runner's stderr, so a runner that dies at startup leaves its reason behind.
 * `onExit` lets a launch stop waiting for a runner that has already died.
 */
export function spawnRunner(entry: string, lease: CheckoutLease, environment: NodeJS.ProcessEnv = {}, log?: string, onExit?: () => void): Promise<number> {
  const env = { ...process.env, ...environment };
  delete env.PLAIT_LOCK_FD;
  delete env.PLAIT_NOTIFY_FD;
  const logFd = log === undefined ? undefined : openSync(log, constants.O_WRONLY | constants.O_CREAT | constants.O_APPEND | constants.O_NOFOLLOW, 0o600);
  const stdio: ('ignore' | number)[] = ['ignore', 'ignore', logFd ?? 'ignore', ...handoffDescriptors(lease)];
  // Overwrite any non-CLOEXEC host lock/notify descriptor above fd 5 so the runner never holds host.lock.
  for (const key of ['PLAIT_LOCK_FD', 'PLAIT_NOTIFY_FD'] as const) {
    const fd = Number(process.env[key]);
    if (Number.isInteger(fd) && fd >= stdio.length && fd < 64) {
      while (stdio.length <= fd) stdio.push('ignore');
    }
  }
  let child;
  try { child = spawn(process.execPath, [entry], { env, detached: true, stdio }); }
  finally { if (logFd !== undefined) closeSync(logFd); }
  return new Promise((resolve, reject) => {
    child.once('error', reject);
    if (onExit) child.once('exit', onExit);
    child.once('spawn', () => {
      child.unref();
      resolve(child.pid!);
    });
  });
}
