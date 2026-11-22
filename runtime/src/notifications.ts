import { Socket } from 'node:net';
import type { Writable } from 'node:stream';

export type ChangeSource = { subscribe?: (listener: (id: string) => void) => () => void };

export function forwardChanges(source: ChangeSource, output: Writable): () => void {
  const pending = new Set<string>();
  let scheduled: NodeJS.Immediate | undefined;
  let blocked = false;
  const schedule = () => {
    if (!blocked && !scheduled && pending.size) scheduled = setImmediate(flush);
  };
  const flush = () => {
    scheduled = undefined;
    const frame = [...pending].join('\n') + '\n';
    pending.clear();
    blocked = !output.write(frame);
  };
  const drained = () => { blocked = false; schedule(); };
  output.on('drain', drained);
  const unsubscribe = source.subscribe?.(id => {
    if (pending.has('*')) return;
    pending.add(id);
    // Overflow invalidates all watches instead of losing a change or growing a queue.
    // Reserved names: `#` some thread summary changed, `%` the workspace changed.
    if (pending.size > 64 || (id !== '#' && id !== '%' && !/^[A-Za-z0-9_-]{1,128}$/.test(id))) {
      pending.clear(); pending.add('*');
    }
    schedule();
  });
  return () => {
    unsubscribe?.();
    if (scheduled) clearImmediate(scheduled);
    output.off('drain', drained);
    pending.clear();
  };
}

export function notificationChannel(source: ChangeSource): () => void {
  const value = process.env.PLAIT_NOTIFY_FD;
  if (value === undefined) return () => {};
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value)) || Number(value) < 3) {
    throw new Error('Invalid notification descriptor');
  }
  const output = new Socket({ fd: Number(value), readable: false, writable: true });
  // A broken notification channel must not leave connected clients silently stale.
  output.on('error', error => process.stdin.destroy(error));
  const stop = forwardChanges(source, output);
  return () => { stop(); output.destroy(); };
}
