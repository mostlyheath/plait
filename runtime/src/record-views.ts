// Version 5 views: complete stored lists are paged, never truncated silently.
import { fail } from './protocol.ts';
import type { Store } from './store.ts';

type Row = Record<string, unknown>;
const BYTES = 800_000;
const size = (value: unknown) => Buffer.byteLength(JSON.stringify(value));

/** Input entry positions are durable for all harnesses, including joined steering. */
function requests(store: Store, row: Row): string[] {
  if (row.kind === 'undo') return [String(row.run_request)];
  const first = store.db.prepare('SELECT entry FROM requests WHERE scope=? AND id=?').get(String(row.thread), String(row.run_request));
  if (first?.entry == null) return [String(row.run_request)];
  // The turn ends at the next turn's before record, or at the next input marked as starting its
  // own run: a run an older build started after a rollback may have no before record.
  const end = store.db.prepare(`SELECT MIN(entry) AS entry FROM (
      SELECT q.entry FROM records r JOIN requests q ON q.scope=r.thread AND q.id=r.run_request
      WHERE r.thread=? AND r.kind='before' AND q.entry>? AND NOT EXISTS (SELECT 1 FROM records a WHERE a.before=r.id AND a.kind='undo')
      UNION ALL SELECT n.entry FROM requests n WHERE n.scope=? AND n.state IS NULL AND n.consumed='started' AND n.entry>?
        AND NOT EXISTS (SELECT 1 FROM records b JOIN records u ON u.before=b.id AND u.kind='undo'
          WHERE b.thread=n.scope AND b.run_request=n.id AND b.kind='before'))`)
    .get(String(row.thread), first.entry as number, String(row.thread), first.entry as number);
  return store.db.prepare(`SELECT id FROM requests WHERE scope=? AND state IS NULL AND entry>=? AND (? IS NULL OR entry<?) ORDER BY entry,rowid`)
    .all(String(row.thread), first.entry as number, end?.entry ?? null, end?.entry ?? null).map(item => String(item.id));
}
function change(path: Row) {
  return {
    path: String(path.path),
    change: path.before_mode == null ? 'added' : path.after_mode == null ? 'deleted'
      : path.before_oid === path.after_oid && path.before_mode !== path.after_mode ? 'mode' : 'modified',
    added: path.added ?? null, removed: path.removed ?? null,
  };
}

export function recordsPage(store: Store, thread: string, cursor?: string, limit = 20, protocol = 5) {
  let [rowid, pathOffset, coverageOffset, requestOffset] = cursor?.split(':').map(Number) ?? [Number.MAX_SAFE_INTEGER, 0, 0, 0];
  const records: Record<string, unknown>[] = [];
  let bytes = 0;
  let before: string | null = null;
  for (let count = 0; count < limit; count++) {
    const row = store.db.prepare(`SELECT b.rowid AS position,b.id AS first_id,b.head AS before_head,b.head_ref AS before_ref,
      COALESCE(a.id,b.id) AS id,b.thread,b.run_request,COALESCE(a.kind,'after') AS kind,COALESCE(a.state,b.state) AS state,
      COALESCE(a.created_at,b.created_at) AS created_at,a.head,a.head_ref,a.undoes,a.after_restart,COALESCE(a.not_covered,b.not_covered) AS not_covered,r.outcome
      FROM records b LEFT JOIN records a ON a.before=b.id
      LEFT JOIN runs r ON r.thread_id=b.thread AND r.request_id=b.run_request AND r.finished_at IS NOT NULL
      WHERE b.thread=? AND b.kind='before' AND b.rowid<=?
      ORDER BY b.rowid DESC LIMIT 1`).get(thread, rowid);
    if (!row) break;
    const paths = store.db.prepare('SELECT * FROM record_paths WHERE record=? ORDER BY path').all(String(row.id));
    const coverage = JSON.parse(String(row.not_covered ?? '[]')) as unknown[];
    const ids = requests(store, row);
    if (pathOffset > paths.length || coverageOffset > coverage.length || requestOffset > ids.length) fail('invalid_request', 'Record cursor is beyond the end of a list.');
    const page = {
      id: String(row.id), request_ids: [] as string[], request_ids_offset: requestOffset, request_ids_total: ids.length,
      kind: row.kind === 'undo' ? 'undo' : 'turn', undoes: row.undoes ?? null,
      state: row.id === row.first_id && row.state === 'ready' ? 'recording' : String(row.state),
      head_moved: ['ready', 'undone'].includes(String(row.state)) && row.id !== row.first_id && (row.before_head !== row.head || row.before_ref !== row.head_ref),
      ...(protocol >= 7 && row.kind !== 'undo' && row.outcome != null ? { outcome: row.outcome } : {}),
      after_restart: row.after_restart === 1,
      paths: [] as ReturnType<typeof change>[], paths_offset: pathOffset, paths_total: paths.length,
      not_covered: [] as unknown[], not_covered_offset: coverageOffset, not_covered_total: coverage.length,
      created_at: Number(row.created_at),
    };
    // Leave room for metadata and the envelope; a single filesystem name fits.
    let available = BYTES - bytes - size(page) - 1024;
    const fill = <T>(source: T[], offset: number, target: T[]) => {
      while (offset < source.length && target.length < 500) {
        const cost = size(source[offset]) + 1;
        if (cost > available) break;
        available -= cost; target.push(source[offset++]);
      }
      return offset;
    };
    const nextRequest = fill(ids, requestOffset, page.request_ids);
    const nextPath = fill(paths.map(change), pathOffset, page.paths);
    const nextCoverage = fill(coverage, coverageOffset, page.not_covered);
    const partial = nextRequest < ids.length || nextPath < paths.length || nextCoverage < coverage.length;
    if (partial && nextRequest === requestOffset && nextPath === pathOffset && nextCoverage === coverageOffset && records.length) {
      before = `${row.position}:${pathOffset}:${coverageOffset}:${requestOffset}`; break;
    }
    records.push(page); bytes += size(page);
    if (partial) { before = `${row.position}:${nextPath}:${nextCoverage}:${nextRequest}`; break; }
    rowid = Number(row.position) - 1; pathOffset = 0; coverageOffset = 0; requestOffset = 0;
    const more = store.db.prepare("SELECT 1 FROM records WHERE thread=? AND kind='before' AND rowid<=? LIMIT 1").get(thread, rowid);
    before = more ? `${rowid}:0:0:0` : null;
    if (!more) break;
  }
  return { records, before };
}

/** The undo reply previews lists; turn_records pages the complete undo record. */
export function undoReply(state: 'undone' | 'blocked', changed: string[], uncovered: unknown[], record: string, restart: boolean) {
  let bytes = 0;
  const bounded = <T>(values: T[]) => {
    const result: T[] = [];
    for (const value of values) {
      const cost = size(value) + 1;
      if (result.length === 500 || bytes + cost > BYTES) break;
      bytes += cost; result.push(value);
    }
    return result;
  };
  return { state, changed_since: bounded(changed), changed_since_total: changed.length,
    not_covered: bounded(uncovered), not_covered_total: uncovered.length, record_id: record, after_restart: restart };
}
