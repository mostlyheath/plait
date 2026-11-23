import { watch, type FSWatcher } from 'node:fs';
import { sep } from 'node:path';

const LIMIT = 64;
const DEBOUNCE_MS = 250;
/** A build writing ignored files would otherwise make clients reread status constantly. */
const MIN_INTERVAL_MS = 1000;
type Watched = { watcher: FSWatcher; counter: number; timer?: ReturnType<typeof setTimeout>; last: number };

function affectsStatus(filename: string | null): boolean {
  if (filename === null) return true;
  const parts = filename.split(sep);
  if (parts[0] !== '.git') return true;
  return parts.length === 2 && (parts[1] === 'index' || parts[1] === 'HEAD')
    || parts[1] === 'refs' && parts.length > 2;
}

/** Watches the newest listed checkouts; failed watches are absent from status. */
export class CheckoutWatcher {
  private watched = new Map<string, Watched>();
  private folders: string[] = [];
  private changed: (folder?: string) => void;
  constructor(changed: (folder?: string) => void) { this.changed = changed; }

  sync(folders: string[]) {
    const listed = new Set(folders);
    const previous = new Set(this.folders);
    this.folders = [...this.folders.filter(folder => listed.has(folder)), ...[...listed].filter(folder => !previous.has(folder))];
    const desired = this.folders.slice(-LIMIT);
    const keep = new Set(desired);
    for (const folder of this.watched.keys()) if (!keep.has(folder)) this.remove(folder);
    for (const folder of desired) {
      if (this.watched.has(folder)) continue;
      try {
        const watcher = watch(folder, { recursive: true }, (_event, filename) => {
          if (affectsStatus(filename?.toString() ?? null)) this.schedule(folder);
        });
        const entry: Watched = { watcher, counter: 0, last: 0 };
        this.watched.set(folder, entry);
        watcher.on('error', () => {
          if (this.watched.get(folder) !== entry) return;
          this.remove(folder);
          this.changed();
        });
      } catch { /* An unavailable checkout can still be refreshed manually. */ }
    }
  }

  status(): Record<string, number> {
    return Object.fromEntries([...this.watched].map(([folder, entry]) => [folder, entry.counter]));
  }

  private schedule(folder: string) {
    const entry = this.watched.get(folder);
    if (!entry) return;
    clearTimeout(entry.timer);
    // Quiet for a moment, and at most once a second; the last change is always announced.
    const wait = Math.max(DEBOUNCE_MS, entry.last + MIN_INTERVAL_MS - Date.now());
    entry.timer = setTimeout(() => { entry.counter++; entry.last = Date.now(); entry.timer = undefined; this.changed(folder); }, wait);
  }

  private remove(folder: string) {
    const entry = this.watched.get(folder);
    if (!entry) return;
    clearTimeout(entry.timer);
    entry.watcher.close();
    this.watched.delete(folder);
  }

  close() { for (const folder of this.watched.keys()) this.remove(folder); }
}
