import assert from 'node:assert/strict';
import { chmod, mkdir, readFile, writeFile, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import { test } from 'node:test';
import { validateWorktree, type Operation } from '../src/worktrees.ts';
import { acquireCheckout, acquireCommonMetadata } from '../src/safety.ts';
import { codex, claude } from './support/record-runtime.ts';
import { exec } from './support/safety.ts';

async function commit(root: string) {
  await writeFile(join(root, 'source.txt'), 'committed\n');
  await exec('/usr/bin/git', ['-C', root, 'add', '.']);
  await exec('/usr/bin/git', ['-C', root, '-c', 'user.name=Test', '-c', 'user.email=test@invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'base']);
}
async function done(runtime: { reply(input: unknown): Promise<any> }, id: string) {
  const deadline = Date.now() + 20_000;
  for (;;) {
    const reply = await runtime.reply({ op: 'worktree_status', request_id: id });
    assert.equal(reply.ok, true, JSON.stringify(reply));
    const operation = reply.result.operation as Operation;
    if (operation.state !== 'preparing' && operation.state !== 'removing') return operation;
    assert.ok(Date.now() < deadline, JSON.stringify(operation));
    await new Promise(resolve => setTimeout(resolve, 25));
  }
}
for (const harness of ['codex', 'claude'] as const) test(`worktree binds selected ${harness}, cold base, permissions and idempotent lost reply`, async t => {
  const h = await (harness === 'codex' ? codex(t) : claude(t));
  await chmod(h.store, 0o700); await commit(h.root);
  await mkdir(join(h.root, 'target')); await writeFile(join(h.root, 'target', 'artifact'), 'do not copy');
  await writeFile(join(h.root, 'source.txt'), 'dirty\n');
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root }); assert.equal(info.ok, true, JSON.stringify(info));
  const request = { op: 'create_worktree_thread', request_id: 'creation', parent: info.result.parent, harness, provider: harness, model_id: harness === 'codex' ? 'gpt-fake' : 'default', mode: harness === 'codex' ? 'ask' : 'auto', approval: 'ask', thinking: 'high' };
  const [first, duplicate]: any[] = await Promise.all([h.runtime.reply(request), h.runtime.reply(request)]);
  assert.equal(first.ok, true, JSON.stringify(first)); assert.equal(duplicate.ok, true, JSON.stringify(duplicate));
  assert.equal(first.result.operation.thread_id, duplicate.result.operation.thread_id);
  // Discard the first acknowledgement: status recovers the complete result.
  const ready = await done(h.runtime, 'creation'); assert.equal(ready.state, 'ready', JSON.stringify(ready));
  const tree = ready.worktree!;
  assert.equal(await readFile(join(tree.path, 'source.txt'), 'utf8'), 'committed\n');
  await assert.rejects(readFile(join(tree.path, 'target', 'artifact')), { code: 'ENOENT' });
  const snapshot = h.runtime.snapshot(ready.thread_id);
  assert.equal(snapshot.harness?.id, harness); assert.equal(snapshot.harness?.mode, request.mode); assert.equal(snapshot.model.model_id, request.model_id); assert.equal(snapshot.checkout, tree.path);
  assert.equal(tree.branch, `plait/${ready.thread_id.replaceAll('-', '').slice(0, 12)}`);
  assert.equal((await h.runtime.reply({ ...request, model_id: 'different' })).error?.code, 'invalid_request');
  assert.equal((await h.runtime.reply({ ...request, protocol_version: 6 })).error?.code, 'invalid_request');
  const replay: any = await h.runtime.reply(request); assert.equal(replay.result.operation.thread_id, ready.thread_id);
  assert.equal(h.runtime.store.db.prepare('SELECT COUNT(*) AS count FROM worktree_operations').get()?.count, 1);
});

test('worktree restart reconciles creation and window closure before acknowledgement', async t => {
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  const request = { op: 'create_worktree_thread', request_id: 'restart', parent: info.result.parent, harness: 'codex', provider: 'codex', model_id: 'gpt-fake', mode: 'ask', approval: 'ask' };
  await h.runtime.reply(request); const ready = await done(h.runtime, 'restart'); assert.equal(ready.state, 'ready', JSON.stringify(ready));
  // Crash after the helper finished but before the runtime persisted its reply.
  const creating = { ...ready, state: 'preparing', worktree: null };
  h.runtime.store.db.prepare('UPDATE worktree_operations SET result=? WHERE request_id=?').run(JSON.stringify(creating), 'restart');
  const journalPath = join(h.store, 'worktrees', `${ready.thread_id}.json`);
  const journal = JSON.parse(await readFile(journalPath, 'utf8')); journal.stage = 'creating';
  await writeFile(journalPath, JSON.stringify(journal));
  await h.close(); const reopened = await h.reopen();
  const recovered = await done(reopened.runtime, 'restart'); assert.equal(recovered.state, 'ready', JSON.stringify(recovered));
  assert.equal(recovered.thread_id, ready.thread_id); assert.equal(recovered.worktree!.path, ready.worktree!.path);
  const listed: any = await reopened.runtime.reply({ op: 'list_worktrees' }); assert.equal(listed.result.operations[0].thread_id, ready.thread_id);
  assert.equal(reopened.runtime.store.db.prepare('SELECT COUNT(*) AS count FROM threads').get()?.count, 1);
});

test('host rejects forged parent, missing filesystem and imported deletion authority', async t => {
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  const request = { op: 'create_worktree_thread', request_id: 'forged', parent: { ...info.result.parent, main: join(h.root, 'missing') }, harness: 'codex', provider: 'codex', model_id: 'gpt-fake', approval: 'ask' };
  assert.equal((await h.runtime.reply(request)).ok, true);
  assert.equal((await done(h.runtime, 'forged')).state, 'failed');
  assert.equal((await h.runtime.reply({ op: 'worktree_info', checkout: join(h.root, 'missing') })).ok, false);
  assert.equal((await h.runtime.reply({ op: 'remove_worktree', request_id: 'remove', creation_id: 'desktop-import', identity: { dev: '1', ino: '2' }, allow_unpushed: true })).error?.code, 'not_found');
  assert.equal(h.runtime.store.db.prepare('SELECT COUNT(*) AS count FROM threads').get()?.count, 0);
});

test('host removal refuses uncommitted, unpushed and stale busy summaries and keeps history', async t => {
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  await h.runtime.reply({ op: 'create_worktree_thread', request_id: 'create', parent: info.result.parent, harness: 'codex', provider: 'codex', model_id: 'gpt-fake', approval: 'ask' });
  const ready = await done(h.runtime, 'create'); assert.equal(ready.state, 'ready', JSON.stringify(ready));
  const tree = ready.worktree!;
  // The host records which project its worktree belongs to, for every client.
  const owned = async () => ((await h.runtime.reply({ op: 'workspace_view' })).result as any).worktrees;
  assert.deepEqual(await owned(), [{ folder: tree.path, main: h.root, branch: tree.branch, creation_id: 'create', identity: tree.identity }]);
  const remove = async (id: string, allow = false) => {
    const result = await h.runtime.reply({ op: 'remove_worktree', request_id: id, creation_id: 'create', identity: tree.identity, allow_unpushed: allow });
    assert.equal(result.ok, true, JSON.stringify(result)); return done(h.runtime, id);
  };
  assert.match((await remove('unpushed')).error!.message, /unpushed|publication/);
  await writeFile(join(tree.path, 'source.txt'), 'preserve');
  assert.match((await remove('dirty', true)).error!.message, /uncommitted/);
  await writeFile(join(tree.path, 'source.txt'), 'committed\n');
  const lease = await acquireCheckout(tree.path);
  try { assert.match((await remove('stale-summary', true)).error!.message, /in use|busy/); } finally { await lease.close(); }
  assert.equal((await owned()).length, 1, 'a refused removal kept the worktree');
  const removed = await remove('remove', true); assert.equal(removed.state, 'removed', JSON.stringify(removed));
  assert.deepEqual(await owned(), []);
  assert.equal((await remove('remove', true)).state, 'removed');
  assert.equal(h.runtime.snapshot(ready.thread_id).checkout, tree.path);
  await exec('/usr/bin/git', ['-C', h.root, 'show-ref', '--verify', `refs/heads/${tree.branch}`]);
});

test('refused removal leaves the creation receipt as it was', async t => {
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  await h.runtime.reply({ op: 'create_worktree_thread', request_id: 'create', parent: info.result.parent, harness: 'codex', provider: 'codex', model_id: 'gpt-fake', approval: 'ask' });
  const ready = await done(h.runtime, 'create'); assert.equal(ready.state, 'ready', JSON.stringify(ready));
  const tree = ready.worktree!;
  await writeFile(join(tree.path, 'source.txt'), 'preserve');
  const remove = async (id: string) => {
    const result: any = await h.runtime.reply({ op: 'remove_worktree', request_id: id, creation_id: 'create', identity: tree.identity, allow_unpushed: true });
    assert.equal(result.ok, true, JSON.stringify(result)); return done(h.runtime, id);
  };
  // The refusal is the removal's answer, not a new fact about the creation.
  assert.match((await remove('dirty')).error!.message, /uncommitted/);
  const creation = await done(h.runtime, 'create');
  assert.equal(creation.state, 'ready'); assert.equal(creation.error, null, JSON.stringify(creation));
  // A creation whose thread was never admitted stays failed after a refused removal.
  const failed = { ...ready, state: 'failed', error: { code: 'internal', message: 'admission failed' } };
  h.runtime.store.db.prepare('UPDATE worktree_operations SET result=? WHERE request_id=?').run(JSON.stringify(failed), 'create');
  h.runtime.store.db.prepare('UPDATE threads SET id=? WHERE id=?').run('00000000-0000-4000-8000-000000000000', ready.thread_id);
  assert.match((await remove('dirty-again')).error!.message, /uncommitted/);
  const kept = await done(h.runtime, 'create');
  assert.equal(kept.state, 'failed', JSON.stringify(kept)); assert.equal(kept.error?.message, 'admission failed');
});

test('worktree operation validation rejects unknown fields, malformed identities and lengths', () => {
  const identity = { dev: '1', ino: '2' };
  const request = { op: 'create_worktree_thread', request_id: 'create', parent: { root: '/repo', main: '/repo', common: '/repo/.git', identity, main_identity: identity, common_identity: identity, base: 'a'.repeat(40) }, harness: 'codex', provider: 'codex', model_id: 'model', approval: 'ask' };
  assert.deepEqual(validateWorktree(request), request);
  for (const bad of [{ ...request, protocol_version: 6 }, { ...request, unknown: true }, { ...request, parent: { ...request.parent, base: 'HEAD' } }, { ...request, request_id: 'x'.repeat(129) }, { ...request, parent: { ...request.parent, root: '/a/../b' } }, { ...request, parent: { ...request.parent, identity: { dev: '1', ino: '18446744073709551616' } } }, { ...request, harness: 'unknown' }, { ...request, model_id: 'm'.repeat(257) }]) assert.throws(() => validateWorktree(bad));
});

test('real host loss during checkout resumes the admitted UUID without repeating hooks', { timeout: 40_000 }, async t => {
  const { setup, request, childPids, until } = await import('./support/host.ts');
  const f = await setup(t);
  const calls = join(f.base, 'hook-calls');
  const hook = join(f.repo, '.git', 'hooks', 'post-checkout');
  await writeFile(hook, `#!/bin/sh\nprintf x >> '${calls}'\nsleep 1\n`); await chmod(hook, 0o755);
  const info = await request(f.store, { op: 'worktree_info', checkout: f.repo }) as any;
  const create = { op: 'create_worktree_thread', request_id: 'lost-client', parent: info.result.parent, harness: 'pi', provider: 'faux', model_id: 'faux-1', approval: 'ask' };
  const accepted = await request(f.store, create) as any; assert.equal(accepted.ok, true, JSON.stringify(accepted));
  await until(() => readFile(calls, 'utf8'), value => value === 'x');
  const [node] = await childPids(f.host.pid!); process.kill(node, 'SIGKILL');
  await until(async () => f.host.exitCode !== null || f.host.signalCode !== null, value => value);
  await f.start();
  const recovered = await until(async () => (await request(f.store, { op: 'worktree_status', request_id: 'lost-client' }) as any).result.operation, value => value.state !== 'preparing');
  assert.equal(recovered.state, 'ready', JSON.stringify(recovered));
  assert.equal(recovered.thread_id, accepted.result.operation.thread_id);
  assert.equal(await readFile(calls, 'utf8'), 'x');
  const retried = await request(f.store, create) as any;
  assert.equal(retried.result.operation.thread_id, recovered.thread_id);
});

test('worktree supervisor reclaims a background checkout hook before acknowledging', async t => {
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  const later = join(h.store, 'late-hook-effect');
  const hook = join(h.root, '.git', 'hooks', 'post-checkout');
  await writeFile(hook, `#!/bin/sh\n(sleep 1; printf unexpected > '${later}') >/dev/null 2>&1 &\nexit 0\n`); await chmod(hook, 0o755);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  await h.runtime.reply({ op: 'create_worktree_thread', request_id: 'hook', parent: info.result.parent, harness: 'codex', provider: 'codex', model_id: 'gpt-fake', approval: 'ask' });
  const ready = await done(h.runtime, 'hook'); assert.equal(ready.state, 'ready', JSON.stringify(ready));
  await new Promise(resolve => setTimeout(resolve, 1200));
  await assert.rejects(readFile(later), { code: 'ENOENT' });
});

test('provider keys from the login shell never reach a worktree hook', async t => {
  const { piProviders } = await import('../src/pi-providers.ts');
  const { Providers } = await import('../src/providers.ts');
  const sentinel = 'sk-SHELL_ONLY_SENTINEL_DO_NOT_LEAK';
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  // Pi's providers live in this process beside the runtime, as in the sidecar.
  const agent = join(h.store, 'pi-agent'); await mkdir(agent, { mode: 0o700 });
  await writeFile(join(agent, 'models.json'), JSON.stringify({ providers: {
    lab: { baseUrl: 'http://127.0.0.1:9/v1', api: 'openai-completions', apiKey: '$SHELL_ONLY_KEY', models: [{ id: 'a' }] },
  } }));
  const source = piProviders(agent, async () => ({ SHELL_ONLY_KEY: sentinel }));
  const registry = await source.load();
  const holder = { models: registry.models, authenticated: registry.authenticated };
  const providers = new Providers(holder, source, registry); t.after(() => providers.close());
  await providers.adoptEnvironment();
  assert.equal(holder.authenticated.has('lab'), true, 'the shell key is in use');

  const dump = join(h.store, 'hook-environment');
  const hook = join(h.root, '.git', 'hooks', 'post-checkout');
  await writeFile(hook, `#!/bin/sh\nenv > '${dump}'\n`); await chmod(hook, 0o755);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  await h.runtime.reply({ op: 'create_worktree_thread', request_id: 'keys', parent: info.result.parent, harness: 'codex', provider: 'codex', model_id: 'gpt-fake', approval: 'ask' });
  const ready = await done(h.runtime, 'keys'); assert.equal(ready.state, 'ready', JSON.stringify(ready));
  const seen = await readFile(dump, 'utf8');
  assert.match(seen, /^PATH=/m, 'the hook ran');
  assert(!seen.includes(sentinel) && !seen.includes('SHELL_ONLY_KEY'));
});

test('killed worktree worker leaves checkout and common leases with Git descendants', { timeout: 30_000 }, async t => {
  const { spawn } = await import('node:child_process');
  const { binary, childPids, until } = await import('./support/host.ts');
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  const marker = join(h.store, 'hook-started');
  const hook = join(h.root, '.git', 'hooks', 'post-checkout');
  await writeFile(hook, `#!/bin/sh\nprintf started > '${marker}'\nsleep 20\n`); await chmod(hook, 0o755);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  const id = '12345678-1234-4abc-8abc-123456789abc';
  const helper = spawn(binary, ['worktree'], { stdio: ['pipe', 'ignore', 'ignore'] });
  helper.stdin!.end(JSON.stringify({ action: 'create', store: h.store, thread_id: id, parent: info.result.parent }));
  let gitGroup: number | undefined;
  try {
    await until(() => readFile(marker, 'utf8'), value => value === 'started');
    const [worker] = await childPids(helper.pid!);
    const [supervisor] = await childPids(worker);
    [gitGroup] = await childPids(supervisor);
    assert.ok(gitGroup, 'nested Git worker has its own process group');
    // Hold both cleanup supervisors: an orphaned stopped group otherwise receives
    // SIGHUP/SIGCONT and exits before we can probe the inherited leases.
    process.kill(helper.pid!, 'SIGSTOP');
    await until(async () => (await exec('/bin/ps', ['-o', 'stat=', '-p', String(helper.pid)])).stdout, state => state.includes('T'));
    process.kill(-gitGroup, 'SIGSTOP');
    await until(async () => (await exec('/bin/ps', ['-o', 'stat=', '-p', String(gitGroup)])).stdout, state => state.includes('T'));
    process.kill(worker, 'SIGKILL');
    await until(async () => (await exec('/bin/ps', ['-o', 'stat=', '-p', String(worker)]).catch(() => ({ stdout: '' }))).stdout, state => !state || state.includes('Z'));
    assert.match((await exec('/bin/ps', ['-o', 'stat=', '-p', String(gitGroup)])).stdout, /T/);
    const journal = JSON.parse(await readFile(join(h.store, 'worktrees', `${id}.json`), 'utf8'));
    await assert.rejects(async () => {
      const lease = await acquireCheckout(journal.path); await lease.close();
    }, /in use|busy/);
    await assert.rejects(async () => {
      const lease = await acquireCommonMetadata(h.root, 0); await lease.close();
    }, /busy|metadata/);
    process.kill(helper.pid!, 'SIGCONT');
    await until(async () => helper.exitCode !== null || helper.signalCode !== null, Boolean);
    try { process.kill(-gitGroup, 'SIGCONT'); } catch { /* Cleanup already reclaimed it. */ }
    await until(async () => {
      const lease = await acquireCheckout(journal.path);
      await lease.close();
      const common = await acquireCommonMetadata(h.root, 0);
      await common.close();
      return true;
    }, Boolean);
  } finally {
    if (gitGroup) { try { process.kill(-gitGroup, 'SIGKILL'); } catch { /* Already reaped. */ } }
    if (helper.exitCode === null && helper.signalCode === null) {
      helper.kill('SIGCONT'); helper.kill('SIGKILL');
    }
  }
});

test('project inventory lists main, owned and external worktrees with live removal facts', async t => {
  const h = await codex(t); await chmod(h.store, 0o700); await commit(h.root);
  const info: any = await h.runtime.reply({ op: 'worktree_info', checkout: h.root });
  await h.runtime.reply({ op: 'create_worktree_thread', request_id: 'inventory', parent: info.result.parent, harness: 'codex', provider: 'codex', model_id: 'gpt-fake', mode: 'ask', approval: 'ask' });
  const ready = await done(h.runtime, 'inventory');
  const external = join(h.root, '..', 'external');
  await exec('/usr/bin/git', ['-C', h.root, 'worktree', 'add', '-b', 'external', external]);
  await writeFile(join(ready.worktree!.path, 'source.txt'), 'changed');
  const reply: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root });
  assert.equal(reply.ok, true, JSON.stringify(reply));
  const rows = reply.result.checkouts;
  assert.equal(rows.length, 3); assert.equal(rows[0].main, true);
  assert.equal(rows[0].preservation.can_keep_local, false);
  const owned = rows.find((r: any) => r.creation_id === 'inventory');
  assert.equal(owned.preservation.changed, 1); assert.equal(owned.preservation.exclusive, 0);
  assert.equal(owned.preservation.unpushed, null); assert.equal(owned.base, info.result.parent.base);
  assert.equal(owned.preservation.can_keep_local, false);
  const removed: any = await h.runtime.reply({ op: 'remove_worktree', request_id: 'inventory-remove', creation_id: 'inventory', identity: ready.worktree!.identity, allow_unpushed: false });
  assert.equal(removed.ok, true);
  const refusal = await done(h.runtime, 'inventory-remove');
  assert.equal(owned.preservation.refusal, refusal.error!.message);
  const one: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root, only: owned.path });
  assert.equal(one.ok, true, JSON.stringify(one));
  assert.deepEqual(one.result.checkouts.map((r: any) => [r.path, r.main, r.creation_id]), [[owned.path, false, 'inventory']]);
  assert.equal(one.result.checkouts[0].preservation.refusal, owned.preservation.refusal);
  const main: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root, only: rows[0].path });
  assert.deepEqual(main.result.checkouts.map((r: any) => [r.path, r.main]), [[rows[0].path, true]]);
  const stranger: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root, only: join(h.root, '..', 'stranger') });
  assert.equal(stranger.ok, false); assert.equal(stranger.error.code, 'invalid_request');
  const foreign = rows.find((r: any) => r.path === external);
  assert.equal(foreign.creation_id, null); assert.match(foreign.preservation.refusal, /did not create/);
  assert.equal(foreign.preservation.can_keep_local, false);
  await writeFile(join(ready.worktree!.path, 'source.txt'), 'committed\n');
  const clean: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root });
  const local = clean.result.checkouts.find((r: any) => r.creation_id === 'inventory');
  assert.equal(local.preservation.can_keep_local, true);
  assert.equal(local.preservation.unpushed, null);
  await h.runtime.reply({ op: 'remove_worktree', request_id: 'inventory-published', creation_id: 'inventory', identity: ready.worktree!.identity, allow_unpushed: false });
  assert.equal((await done(h.runtime, 'inventory-published')).error!.message, local.preservation.refusal);
  h.runtime.store.db.prepare("INSERT INTO requests(scope,id,hash,result,state) VALUES (?,'waiting','test','{}','queued')").run(ready.thread_id);
  const queued: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root });
  const busy = queued.result.checkouts.find((r: any) => r.creation_id === 'inventory');
  assert.equal(busy.preservation.can_keep_local, false);
  assert.match(busy.preservation.refusal, /queued/);
  for (const allow of [false, true]) {
    const request_id = `inventory-queued-${allow}`;
    await h.runtime.reply({ op: 'remove_worktree', request_id, creation_id: 'inventory', identity: ready.worktree!.identity, allow_unpushed: allow });
    assert.equal((await done(h.runtime, request_id)).error!.message, busy.preservation.refusal);
  }
  h.runtime.store.db.prepare("DELETE FROM requests WHERE scope=? AND id='waiting'").run(ready.thread_id);
  h.runtime.store.db.prepare('UPDATE worktree_operations SET result=? WHERE request_id=?').run(JSON.stringify({ ...ready, state: 'removing' }), 'inventory');
  const changing: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root });
  assert.equal(changing.result.checkouts.find((r: any) => r.creation_id === 'inventory').preservation.can_keep_local, false);
  h.runtime.store.db.prepare('UPDATE worktree_operations SET result=? WHERE request_id=?').run(JSON.stringify(ready), 'inventory');
  await h.runtime.reply({ op: 'remove_worktree', request_id: 'inventory-clean', creation_id: 'inventory', identity: ready.worktree!.identity, allow_unpushed: true });
  assert.equal((await done(h.runtime, 'inventory-clean')).state, 'removed');
  const after: any = await h.runtime.reply({ op: 'project_worktrees', checkout: h.root });
  assert.equal(after.ok, true, JSON.stringify(after));
  const retained = after.result.checkouts.find((r: any) => r.creation_id === 'inventory');
  assert.equal(retained.removed, true); assert.equal(retained.missing, true);
  assert.equal(retained.preservation.can_keep_local, false);
  assert.match(retained.preservation.refusal, /cache remain/);
  await unlink(join(external, 'source.txt'));
  const changed: any = await h.runtime.reply({ op: 'project_worktrees', checkout: external });
  assert.equal(changed.result.checkouts[0].path, h.root);
  assert.equal(changed.result.checkouts.find((r: any) => r.path === external).preservation.changed, 1);
});

test('project inventory validates its additive request strictly', () => {
  assert.deepEqual(validateWorktree({ op: 'project_worktrees', checkout: '/repo' }), { op: 'project_worktrees', checkout: '/repo' });
  assert.deepEqual(validateWorktree({ op: 'project_worktrees', checkout: '/repo', only: '/repo.worktrees/a' }), { op: 'project_worktrees', checkout: '/repo', only: '/repo.worktrees/a' });
  for (const request of [{ op: 'project_worktrees', checkout: '/repo', only: 'relative' }, { op: 'project_worktrees', checkout: '/repo', only: null }, { op: 'project_worktrees', checkout: '/repo', only: '/repo/../x' }, { op: 'project_worktrees', checkout: '/repo', protocol_version: 6 }, { op: 'project_worktrees', checkout: '/repo/../other' }, { op: 'project_worktrees', checkout: 4 }, { op: 'project_worktrees' }]) assert.throws(() => validateWorktree(request));
});
