import { chmod, lstat, readdir, rm, stat } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';

/** A command's scratch folder is private to one command; once it ends, nothing else uses it. */
async function makeWritable(path: string): Promise<void> {
  const info = await lstat(path).catch(() => undefined);
  if (!info || !info.isDirectory()) return;
  // A command may leave folders it can no longer list or delete; restore owner access first.
  await chmod(path, 0o700).catch(() => {});
  for (const name of await readdir(path).catch(() => [] as string[])) await makeWritable(join(path, name));
}

/** Removes a command's scratch folder. Cleanup never replaces the command's own result. */
export async function removeScratch(path: string): Promise<void> {
  try { await rm(path, { recursive: true, force: true, maxRetries: 3 }); return; }
  catch { /* retry below with owner access restored */ }
  try {
    await makeWritable(path);
    await rm(path, { recursive: true, force: true, maxRetries: 3 });
  } catch (error) {
    console.error('Command scratch cleanup failed:', path, (error as Error).message);
  }
}

/**
 * Removes this user's command scratch folders left behind when a process was
 * killed mid-command. Only folders older than `olderThanMs` are touched: every
 * command has a deadline shorter than that, so none can still be in use.
 */
export async function sweepStaleScratch(prefix: string, olderThanMs: number, now = Date.now()): Promise<number> {
  const parent = dirname(prefix); const stem = basename(prefix);
  const uid = process.getuid?.();
  let removed = 0;
  for (const name of await readdir(parent).catch(() => [] as string[])) {
    if (!name.startsWith(stem)) continue;
    const path = join(parent, name);
    const info = await stat(path).catch(() => undefined);
    if (!info || !info.isDirectory() || (uid !== undefined && info.uid !== uid)) continue;
    const born = info.birthtimeMs > 0 ? info.birthtimeMs : info.ctimeMs;
    if (now - born < olderThanMs) continue;
    await removeScratch(path);
    removed++;
  }
  return removed;
}
