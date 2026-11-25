import { strict as assert } from 'node:assert';
import { mkdtemp, rm, stat, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import './support/safety.ts';
import { RunnerRegistry } from '../src/runner/registry.ts';

test('registry reports dead and finished runs and keeps private permissions', async () => {
  const root = await mkdtemp(join(tmpdir(), 'plait-runners-'));
  try {
    const registry = new RunnerRegistry(root);
    const manifest = { run_id: 'one', thread_id: 'thread', request_id: 'request', checkout: '/checkout', harness: 'pi', build_id: 'build' };
    await registry.writeManifest(manifest);
    assert.equal((await stat(registry.directory)).mode & 0o777, 0o700);
    assert.equal((await stat(registry.path('one', '.json'))).mode & 0o777, 0o600);
    assert.equal((await registry.scan())[0].status, 'dead');
    await registry.writeReceipt('one', { result: 'done' });
    assert.equal((await registry.scan())[0].status, 'finished');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('registry refuses a symlink directory', async () => {
  const root = await mkdtemp(join(tmpdir(), 'plait-runners-'));
  try { await symlink(root, join(root, 'runners')); await assert.rejects(new RunnerRegistry(root).prepare()); }
  finally { await rm(root, { recursive: true, force: true }); }
});

test('registry detects a lock held by another process', async () => {
  const root = await mkdtemp(join(tmpdir(), 'plait-runners-live-'));
  const registry = new RunnerRegistry(root);
  const manifest = { run_id: 'live', thread_id: 'thread', request_id: 'request', checkout: '/checkout', harness: 'pi', build_id: 'build' };
  let child: ReturnType<typeof spawn> | undefined;
  try {
    await registry.writeManifest(manifest);
    const script = `import fcntl, os, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT, 0o600)
fcntl.flock(fd, fcntl.LOCK_EX)
print('ready', flush=True)
sys.stdin.read()
os.close(fd)`;
    child = spawn('python3', ['-c', script, registry.path('live', '.lock')], { stdio: ['pipe', 'pipe', 'pipe'] });
    let stderr = ''; child.stderr!.on('data', data => { stderr += String(data); });
    const ready = await Promise.race([
      once(child.stdout!, 'data').then(([data]) => String(data)),
      once(child, 'exit').then(([code]) => { throw new Error(`lock holder exited: ${code}: ${stderr}`); }),
    ]);
    assert.equal(ready, 'ready\n');
    assert.equal((await registry.scan())[0].status, 'live');
    child.stdin!.end();
    await once(child, 'exit');
    assert.equal((await registry.scan())[0].status, 'dead');
  } finally {
    if (child && child.exitCode === null) { child.kill(); await once(child, 'exit'); }
    await rm(root, { recursive: true, force: true });
  }
});

test('runner sockets fit the macOS Unix socket limit for a long store path', async () => {
  const { socketPathFits } = await import('../src/runner/socket.ts');
  // As long as a CI temp store on macOS: /private/var/folders/xx/<28>/T/plait-sandbox-xxxxxx
  const store = '/private/var/folders/q2/p1t6rgxj5fj32g1dc7dmd49r0000gn/T/plait-sandbox-myr1mc';
  const path = new RunnerRegistry(store).socketPath('0123456789ab');
  assert.ok(Buffer.byteLength(path) <= 103, path);
  socketPathFits(path);
  assert.throws(() => socketPathFits('/x'.repeat(60)), /too long/);
});
