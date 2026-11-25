import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import type { Snapshot } from '../../src/protocol.ts';
import { delay, exec, until } from './host.ts';

type SavedThread = { title: string; draft: { text: string } };

// The installed app connects to the real fixture host. No sample-view code enters the app.
export async function installedClient(base: string, store: string, repo: string, thread: Snapshot) {
  if (process.env.PLAIT_VERIFY_INSTALLED_UI !== '1') return undefined;
  assert(process.platform === 'darwin' && process.env.GITHUB_ACTIONS === 'true');
  const artifacts = process.env.PLAIT_INSTALLED_UI_ARTIFACTS;
  assert(artifacts && isAbsolute(artifacts));
  const directory = join(base, 'desktop');
  const file = join(directory, 'desktop.json');
  // Thread work lives in one file per machine; desktop.json keeps the older
  // records, which the app copies in once (#174).
  const machines = join(directory, 'desktop-machines');
  const draft = 'Review the real test output before making the next change.';
  await mkdir(directory, { mode: 0o700 });
  await writeFile(file, JSON.stringify({
    schema_version: 1, host_store: store, checkout: repo, selected: thread.id,
    threads: { [thread.id]: { checkout: repo, model: thread.model, title: '',
      draft: { text: draft, revision: 2, submitted: null }, pending: null } },
  }), { mode: 0o600 });
  let child: ChildProcess | undefined;
  const legacy = async () => JSON.parse(await readFile(file, 'utf8'));
  // The machine file holding this thread, once the app has copied it in.
  const machineFile = async (): Promise<string | undefined> => {
    let names: string[];
    try { names = await readdir(machines); } catch { return undefined; }
    for (const name of names.filter(n => n.endsWith('.json'))) {
      const path = join(machines, name);
      const document = JSON.parse(await readFile(path, 'utf8'));
      if ((document.work?.threads ?? []).some((t: { id: string }) => t.id === thread.id)) return path;
    }
    return undefined;
  };
  // The thread as the app last saved it: its machine file once copied, else desktop.json.
  const saved = async (): Promise<SavedThread> => {
    const path = await machineFile();
    if (!path) return (await legacy()).threads[thread.id];
    const document = JSON.parse(await readFile(path, 'utf8'));
    return document.work.threads.find((t: { id: string }) => t.id === thread.id).thread;
  };
  // Clears the saved title wherever it is kept, so a fresh one proves this launch saw the host.
  const clearTitle = async () => {
    const state = await legacy();
    state.threads[thread.id].title = '';
    await writeFile(file, JSON.stringify(state), { mode: 0o600 });
    const path = await machineFile();
    if (!path) return;
    const document = JSON.parse(await readFile(path, 'utf8'));
    document.work.threads.find((t: { id: string }) => t.id === thread.id).thread.title = '';
    await writeFile(path, JSON.stringify(document), { mode: 0o600 });
  };
  return {
    open: async (first: boolean) => {
      assert(!child || child.exitCode !== null || child.signalCode !== null);
      assert.equal((await legacy()).selected, thread.id);
      assert.equal((await saved()).draft.text, draft);
      await clearTitle();
      child = spawn('/Applications/Plait.app/Contents/MacOS/plait', first ? [repo] : [], {
        stdio: ['ignore', 'ignore', 'pipe'],
        env: { PATH: process.env.PATH, HOME: process.env.HOME, PLAIT_HOST_STORE: store, PLAIT_DESKTOP_STATE_DIR: directory },
      });
      let errors = '';
      child.stderr!.on('data', chunk => { errors = (errors + chunk.toString()).slice(-8192); });
      child.on('error', error => { errors = error.message; });
      const observed = await until(async () => ({
        thread: await saved(),
        error: child!.exitCode !== null || child!.signalCode !== null ? `Installed client exited: ${errors}` : null,
      }), value => value.error !== null || value.thread.title !== '');
      assert.equal(observed.error, null, observed.error ?? 'Installed client must stay running.');
      await delay(500);
      assert.equal((await saved()).draft.text, draft);
    },
    capture: async (name: 'active' | 'coding') => {
      await exec('/usr/sbin/screencapture', ['-x', join(artifacts, `installed-client-${name}.png`)]);
    },
    close: async () => {
      if (!child || child.exitCode !== null || child.signalCode !== null) return;
      child.kill('SIGTERM');
      try { await until(async () => child!.signalCode !== null || child!.exitCode !== null, value => value, 3000); }
      finally { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); }
    },
  };
}
