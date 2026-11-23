import { spawn } from 'node:child_process';
import { handoffDescriptors, type CheckoutLease } from '../safety.ts';

export function spawnRunner(entry: string, lease: CheckoutLease, environment: NodeJS.ProcessEnv = {}): Promise<number> {
  const env = { ...process.env, ...environment };
  delete env.PLAIT_LOCK_FD;
  delete env.PLAIT_NOTIFY_FD;
  const stdio: ('ignore' | number)[] = ['ignore', 'ignore', 'ignore', ...handoffDescriptors(lease)];
  // Overwrite any non-CLOEXEC host lock/notify descriptor above fd 5 so the runner never holds host.lock.
  for (const key of ['PLAIT_LOCK_FD', 'PLAIT_NOTIFY_FD'] as const) {
    const fd = Number(process.env[key]);
    if (Number.isInteger(fd) && fd >= stdio.length && fd < 64) {
      while (stdio.length <= fd) stdio.push('ignore');
    }
  }
  const child = spawn(process.execPath, [entry], {
    env, detached: true, stdio,
  });
  return new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('spawn', () => {
      child.unref();
      resolve(child.pid!);
    });
  });
}
