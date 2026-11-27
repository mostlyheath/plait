import { fixtureDirectorySync } from './support/temp.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { ExternalEntries, transcriptPath, type Entry } from '../src/external/entries.ts';

function withDirectory(run: (directory: string) => void) {
  const directory = fixtureDirectorySync(join(tmpdir(), 'plait-entries-'));
  try { run(directory); } finally { rmSync(directory, { recursive: true, force: true }); }
}

const entries: Entry[] = [1, 2, 3, 4].map(id => ({ id, kind: 'message', model: [{ role: 'user', content: `message ${id}` }] }));

test('thread and entries round-trip, with tail and bounded pages', () => withDirectory(directory => {
  const store = new ExternalEntries(transcriptPath(directory, 'thread', 'codex'));
  try {
    assert.equal(store.thread(), undefined);
    store.setThread('first');
    store.setThread('second');
    assert.equal(store.thread(), 'second');
    store.append(entries);
    assert.deepEqual(store.tail(2), { entries: entries.slice(2), next: 5 });
    assert.deepEqual(store.entries({ order: 'ascending', minEntryId: 2, maxEntryId: 3 }, 10), entries.slice(1, 3));
    assert.deepEqual(store.entries({ order: 'descending', minEntryId: 2, maxEntryId: 4 }, 2), [entries[3], entries[2]]);
  } finally { store.close(); }
}));

test('read-only store reads a writer’s data', () => withDirectory(directory => {
  const path = transcriptPath(directory, 'thread', 'claude');
  const writer = new ExternalEntries(path);
  writer.setThread('session');
  writer.append(entries);
  const reader = new ExternalEntries(path, { readOnly: true });
  try {
    assert.equal(reader.thread(), 'session');
    assert.deepEqual(reader.tail(10), { entries, next: 5 });
  } finally { reader.close(); writer.close(); }
}));

test('transcript paths retain harness file names', () => {
  assert.equal(transcriptPath('/store', 'abc', 'codex'), join('/store', 'abc.codex.sqlite'));
  assert.equal(transcriptPath('/store', 'abc', 'claude'), join('/store', 'abc.claude.sqlite'));
});

test('reads an existing old-format store unchanged', () => withDirectory(directory => {
  const path = join(directory, 'old.codex.sqlite');
  const db = new DatabaseSync(path);
  db.exec(`CREATE TABLE entries(id INTEGER PRIMARY KEY, model TEXT NOT NULL);
    CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
    INSERT INTO meta VALUES ('thread', 'old-session');`);
  db.prepare('INSERT INTO entries VALUES (?, ?)').run(entries[0].id, JSON.stringify(entries[0].model));
  db.close();
  const store = new ExternalEntries(path, { readOnly: true });
  try {
    assert.equal(store.thread(), 'old-session');
    assert.deepEqual(store.tail(10), { entries: entries.slice(0, 1), next: 2 });
  } finally { store.close(); }
}));
