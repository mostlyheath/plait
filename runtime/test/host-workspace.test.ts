import assert from 'node:assert/strict';
import { connect } from 'node:net';
import { join } from 'node:path';
import { test } from 'node:test';
import { request, setup, until } from './support/host.ts';

type Frame = { ok?: boolean; result?: any; event?: string; seq?: number; workspace?: any };

test('a workspace subscriber gets a snapshot, then each change in order', { timeout: 30000 }, async t => {
  const f = await setup(t, 'answer', false, false);
  const socket = connect(join(f.store, 'host.sock'));
  t.after(async () => { socket.destroy(); });
  const frames: Frame[] = []; let pending = '';
  socket.once('connect', () => socket.write(JSON.stringify({ op: 'workspace_subscribe' }) + '\n'));
  socket.on('data', data => {
    pending += data.toString(); let end: number;
    while ((end = pending.indexOf('\n')) >= 0) { frames.push(JSON.parse(pending.slice(0, end))); pending = pending.slice(end + 1); }
  });
  await until(async () => frames.length, n => n >= 1);
  const snapshot = frames[0].result;
  assert.equal(frames[0].ok, true);
  assert.equal(snapshot.revision, 0);
  assert.match(snapshot.host.id, /^[0-9a-f-]{36}$/);

  assert.equal((await request(f.store, { op: 'workspace_open_project', path: f.repo })).ok, true);
  await until(async () => frames.length, n => n >= 2);
  assert.deepEqual([frames[1].event, frames[1].seq, frames[1].workspace.revision], ['workspace', 1, 1]);
  assert.deepEqual(frames[1].workspace.projects, [{ path: f.repo, hidden: false }]);

  await request(f.store, { op: 'workspace_hide_project', path: f.repo, hidden: true });
  await until(async () => frames.length, n => n >= 3);
  assert.deepEqual([frames[2].seq, frames[2].workspace.projects[0].hidden], [2, true]);
  // A thread change is not a workspace change.
  await f.create();
  await new Promise(resolve => setTimeout(resolve, 300));
  assert.equal(frames.length, 3);
});
