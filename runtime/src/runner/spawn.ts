import { spawn } from 'node:child_process';
import { handoffDescriptors, type CheckoutLease } from '../safety.ts';

export function spawnRunner(entry: string, lease: CheckoutLease, environment: NodeJS.ProcessEnv = {}): Promise<number> {
  const env = { ...process.env, ...environment };
  delete env.PLAIT_LOCK_FD;
  delete env.PLAIT_NOTIFY_FD;
  const child = spawn(process.execPath, [entry], {
    env, detached: true, stdio: ['ignore', 'ignore', 'ignore', ...handoffDescriptors(lease)],
  });
  return new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('spawn', () => {
      child.unref();
      resolve(child.pid!);
    });
  });
}
