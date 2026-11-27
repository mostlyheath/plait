import { closeSync, constants, fstatSync, lstatSync, openSync, readSync, realpathSync, statSync, watch, type FSWatcher } from 'node:fs';
import { join, resolve, sep } from 'node:path';

const LIMIT = 64;
const DEBOUNCE_MS = 250;
/** A build writing ignored files would otherwise make clients reread status constantly. */
const MIN_INTERVAL_MS = 1000;
const GITFILE_LIMIT = 8192;
type Watched = {
  watcher: FSWatcher;
  gitWatcher?: FSWatcher;
  gitDir?: string;
  counter: number;
  timer?: ReturnType<typeof setTimeout>;
  last: number;
  refsChanged: boolean;
};

function gitDirAffectsRefs(parts: string[]): boolean {
  return parts.length === 1 && ['HEAD', 'packed-refs', 'config'].includes(parts[0])
    || parts[0] === 'refs' && parts.length > 1;
}

function gitDirAffectsStatus(parts: string[]): boolean {
  return parts.length === 1 && parts[0] === 'index' || gitDirAffectsRefs(parts);
}

function affectsRefs(filename: string | null): boolean {
  if (filename === null) return true;
  const parts = filename.split(sep);
  return parts[0] === '.git' && gitDirAffectsRefs(parts.slice(1));
}

function affectsStatus(filename: string | null): boolean {
  if (filename === null) return true;
  const parts = filename.split(sep);
  if (parts[0] !== '.git') return true;
  return gitDirAffectsStatus(parts.slice(1));
}

function readGitPointer(path: string): string | null {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    if (!fstatSync(fd).isFile()) return null;
    const buffer = Buffer.alloc(GITFILE_LIMIT + 1);
    let used = 0;
    while (used < buffer.length) {
      const count = readSync(fd, buffer, used, buffer.length - used, null);
      if (count === 0) return buffer.toString('utf8', 0, used).replace(/[\r\n]+$/, '');
      used += count;
    }
    return null;
  } finally {
    closeSync(fd);
  }
}

/** Resolves a linked worktree's Git directory from its `.git` file, or null for a normal checkout. */
function linkedGitDir(folder: string): string | null {
  const dotGit = join(folder, '.git');
  try {
    if (!lstatSync(dotGit, { throwIfNoEntry: false })?.isFile()) return null;
    const line = readGitPointer(dotGit);
    if (!line?.startsWith('gitdir:')) return null;
    const target = line.slice('gitdir:'.length).trimStart();
    if (!target || /[\0\r\n]/.test(target)) return null;
    const realFolder = realpathSync(folder);
    const gitDir = realpathSync(resolve(realFolder, target));
    if (gitDir === realFolder || !statSync(gitDir).isDirectory()) return null;
    if (!lstatSync(join(gitDir, 'HEAD')).isFile()) return null;
    const backlink = readGitPointer(join(gitDir, 'gitdir'));
    if (!backlink || /[\0\r\n]/.test(backlink)) return null;
    if (realpathSync(resolve(gitDir, backlink)) !== join(realFolder, '.git')) return null;
    return gitDir;
  } catch {
    return null;
  }
}

/** Watches the newest listed checkouts; failed watches are absent from status. */
export class CheckoutWatcher {
  private watched = new Map<string, Watched>();
  private folders: string[] = [];
  private changed: (folder?: string, refsChanged?: boolean) => void;
  constructor(changed: (folder?: string, refsChanged?: boolean) => void) { this.changed = changed; }

  sync(folders: string[]) {
    const listed = new Set(folders);
    const previous = new Set(this.folders);
    this.folders = [...this.folders.filter(folder => listed.has(folder)), ...[...listed].filter(folder => !previous.has(folder))];
    const desired = this.folders.slice(-LIMIT);
    const keep = new Set(desired);
    for (const folder of this.watched.keys()) if (!keep.has(folder)) this.remove(folder);
    for (const folder of desired) {
      const existing = this.watched.get(folder);
      if (existing) {
        this.syncGitDir(folder, existing);
        continue;
      }
      try {
        const watcher = watch(folder, { recursive: true }, (_event, filename) => {
          const current = this.watched.get(folder);
          if (!current || current.watcher !== watcher) return;
          const name = filename?.toString() ?? null;
          if (name === '.git') this.syncGitDir(folder, current);
          if (affectsStatus(name)) this.schedule(folder, affectsRefs(name));
        });
        const entry: Watched = { watcher, counter: 0, last: 0, refsChanged: false };
        this.watched.set(folder, entry);
        watcher.on('error', () => {
          if (this.watched.get(folder) !== entry) return;
          this.remove(folder);
          this.changed();
        });
        this.syncGitDir(folder, entry);
      } catch { /* An unavailable checkout can still be refreshed manually. */ }
    }
  }

  status(): Record<string, number> {
    return Object.fromEntries([...this.watched].map(([folder, entry]) => [folder, entry.counter]));
  }

  private syncGitDir(folder: string, entry: Watched) {
    const gitDir = linkedGitDir(folder) ?? undefined;
    if (entry.gitDir === gitDir) return;
    this.closeGitWatcher(entry);
    if (!gitDir) return;
    try {
      const gitWatcher = watch(gitDir, { recursive: false }, (_event, filename) => {
        if (this.watched.get(folder) !== entry || entry.gitWatcher !== gitWatcher) return;
        const name = filename?.toString() ?? null;
        if (name === null || name === 'index' || name === 'HEAD') this.schedule(folder, name !== 'index');
      });
      entry.gitWatcher = gitWatcher;
      entry.gitDir = gitDir;
      gitWatcher.on('error', () => {
        if (this.watched.get(folder) !== entry || entry.gitWatcher !== gitWatcher) return;
        this.closeGitWatcher(entry);
      });
    } catch { /* Staging changes can still be refreshed manually if the Git dir cannot be watched. */ }
  }

  private closeGitWatcher(entry: Watched) {
    const watcher = entry.gitWatcher;
    entry.gitWatcher = undefined;
    entry.gitDir = undefined;
    try { watcher?.close(); } catch { /* Failed metadata watches must not interrupt checkout operations. */ }
  }

  private schedule(folder: string, refsChanged: boolean) {
    const entry = this.watched.get(folder);
    if (!entry) return;
    clearTimeout(entry.timer);
    entry.refsChanged ||= refsChanged;
    // Quiet for a moment, and at most once a second; the last change is always announced.
    const wait = Math.max(DEBOUNCE_MS, entry.last + MIN_INTERVAL_MS - Date.now());
    entry.timer = setTimeout(() => { entry.counter++; entry.last = Date.now(); entry.timer = undefined; const refs = entry.refsChanged; entry.refsChanged = false; this.changed(folder, refs); }, wait);
  }

  private remove(folder: string) {
    const entry = this.watched.get(folder);
    if (!entry) return;
    clearTimeout(entry.timer);
    entry.watcher.close();
    this.closeGitWatcher(entry);
    this.watched.delete(folder);
  }

  close() { for (const folder of this.watched.keys()) this.remove(folder); }
}
