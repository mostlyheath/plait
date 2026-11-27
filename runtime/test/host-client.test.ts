import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { mkdtemp, chmod, open, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer, type Socket } from 'node:net';
import { Readable } from 'node:stream';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import './support/safety.ts';
import { hostClient, readReply } from '../src/host-client.ts';

async function location(t: TestContext) {
  const dir = await mkdtemp(join(tmpdir(), 'host-client-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return join(dir, 'host.sock');
}
async function listener(t: TestContext, accept: (socket: Socket) => void) {
  const path = await location(t);
  const server = createServer(accept);
  await new Promise<void>((resolve, reject) => server.once('error', reject).listen(path, resolve));
  t.after(() => server.close());
  await chmod(path, 0o600);
  return path;
}
test('reads a fragmented first reply and rejects an oversized or incomplete frame', async () => {
  const frames = (...chunks: Buffer[]) => Readable.from(chunks);
  assert.equal(await readReply(frames(Buffer.from('{"ok":true,'), Buffer.from('"result":"ok"}\nextra'))), 'ok');
  await assert.rejects(readReply(frames(Buffer.alloc(1048576, 32), Buffer.from('\n'))), /exceeds/);
  await assert.rejects(readReply(frames(Buffer.from('{}'))), /closed before reply/);
});
test('refuses a non-socket endpoint and oversized requests before sending', async t => {
  const path = await location(t);
  await writeFile(path, '');
  const send = hostClient(path);
  await assert.rejects(send({}), /private and owned/);
  await assert.rejects(send({ text: 'é'.repeat(32768) }), /request exceeds/);
});
test('refuses shared socket permissions and peer verification failure', async t => {
  const path = await listener(t, socket => socket.on('data', () => assert.fail('sent before verifying ownership')));
  const send = hostClient(path, 1000, async () => { throw new Error('peer UID mismatch'); });
  await chmod(path, 0o660);
  await assert.rejects(send({}), /private and owned/);
  await chmod(path, 0o600);
  await assert.rejects(send({}), /UID mismatch/);
});
test('rejects oversized reply', async t => {
  const path = await listener(t, socket => {
    socket.on('error', () => {});
    socket.once('data', () => socket.end(Buffer.alloc(1048577, 120)));
  });
  await assert.rejects(hostClient(path, 1000, async () => {})({}), /reply exceeds/);
});
test('times out waiting for a reply', async t => {
  const path = await listener(t, socket => socket.resume());
  await assert.rejects(hostClient(path, 50, async () => {})({}), /timed out/);
});
test('reads just the initial watch snapshot', async t => {
  const path = await listener(t, socket => socket.once('data', () => socket.write('{"ok":true,"result":1}\n{"event":"snapshot"}\n')));
  assert.equal(await hostClient(path)({ op: 'watch' }), 1);
});

test('foreground status stays available without a background readiness signal', { timeout: 20000 }, async t => {
  const store = dirname(await location(t));
  const notifications = join(store, 'notifications');
  execFileSync('mkfifo', [notifications], { stdio: 'inherit' });
  const channel = await open(notifications, 'r+');
  t.after(() => channel.close());
  const requests = join(store, 'requests');
  const replies = join(store, 'replies');
  await writeFile(requests, '{"op":"status"}\n'.repeat(2));
  const input = await open(requests, 'r');
  const output = await open(replies, 'w');
  try {
    execFileSync(process.execPath, [fileURLToPath(new URL('../src/main.ts', import.meta.url)), store], {
      stdio: [input.fd, output.fd, 'inherit', channel.fd], timeout: 15000,
      env: { ...process.env, PLAIT_NOTIFY_FD: '3', PLAIT_BACKGROUND_SOCKET: join(store, 'unused.sock'),
        PLAIT_GRAPH: '1', PLAIT_AUTOMATIONS: '0' }
    });
    const lines = (await readFile(replies, 'utf8')).trimEnd().split('\n');
    assert.deepEqual(lines.map(line => JSON.parse(line).ok), [true, true]);
    assert.deepEqual(lines.map(line => JSON.parse(line).result.background_socket_v1), [true, undefined]);
  } finally { await Promise.all([input.close(), output.close()]); }
});
