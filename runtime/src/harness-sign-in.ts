import { spawn } from 'node:child_process';
import type { Login } from './sign-in.ts';
import { SignInFailure } from './sign-in.ts';

const clean = (s: string) => s.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '');
/** Relay only the interactive messages; answers are written directly to the child. */
export function harnessLogin(harness: 'codex' | 'claude', path: string, env: NodeJS.ProcessEnv): Login {
  return async interaction => {
    await new Promise<void>((resolve, reject) => {
      const child = spawn(path, harness === 'codex' ? ['login', '--device-auth'] : ['auth', 'login'], {
        env, detached: true, stdio: [harness === 'codex' ? 'ignore' : 'pipe', 'pipe', 'pipe'],
      });
      let stdout = ''; let stderr = ''; let shown = false; let answer = '';
      const stop = () => { if (child.pid) { try { process.kill(-child.pid, 'SIGKILL'); } catch { /* already exited */ } } };
      const abort = () => stop();
      interaction.signal?.addEventListener('abort', abort, { once: true });
      if (interaction.signal?.aborted) stop();
      const inspect = () => {
        if (shown) return;
        const output = clean(harness === 'codex' ? stdout + '\n' + stderr : stdout);
        const url = /https:\/\/[^\s\x1b]+/.exec(output)?.[0];
        const code = /\b[A-Z0-9]{4}-[A-Z0-9]{4,6}\b/.exec(output)?.[0];
        if (!url || (harness === 'codex' && !code)) return;
        shown = true;
        if (harness === 'codex') interaction.notify({ type: 'device_code', verificationUri: url, userCode: code! });
        else {
          interaction.notify({ type: 'auth_url', url });
          void interaction.prompt({ type: 'secret', message: 'Paste the code from the sign-in page.' }).then(value => {
            answer = value; child.stdin?.end(value + '\n');
          }).catch(stop);
        }
      };
      child.stdout?.on('data', (chunk: Buffer) => { stdout = (stdout + chunk.toString()).slice(-65536); inspect(); });
      child.stderr?.on('data', (chunk: Buffer) => { stderr = (stderr + chunk.toString()).slice(-65536); inspect(); });
      child.once('error', reject);
      child.once('close', code => {
        interaction.signal?.removeEventListener('abort', abort);
        if (interaction.signal?.aborted) return reject(new Error('Sign-in cancelled.'));
        if (code === 0) return resolve();
        const line = clean(stderr).split(/\r?\n/).map(s => s.trim()).filter(Boolean).at(-1) ?? 'Sign-in did not complete.';
        reject(new SignInFailure((answer ? line.replaceAll(answer, '[redacted]') : line).slice(0, 300)));
        answer = '';
      });
    });
  };
}
