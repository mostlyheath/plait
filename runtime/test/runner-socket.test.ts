import { strict as assert } from 'node:assert';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createConnection } from 'node:net';
import { once } from 'node:events';
import './support/safety.ts';
import { COMMAND_LIMIT } from '../src/runner/protocol.ts';
import { RunnerClient, RunnerServer } from '../src/runner/socket.ts';
import type { RunnerMessage, Command } from '../src/runner/protocol.ts';

test('attach replays later events and delivers commands', async () => {
  const root = await mkdtemp(join(tmpdir(), 'plait-socket-'));
  const messages: RunnerMessage[] = []; const commands: Command[] = [];
  const server = new RunnerServer(join(root, 'run.sock'), () => ({ type: 'attached', state: 'running', seq: 2, snapshot: {}, pending_approval: null, head: null, before_record_id: null }), command => commands.push(command));
  try {
    await server.listen(); server.publish({ type: 'phase', seq: 1, state: 'preparing' }); server.publish({ type: 'phase', seq: 2, state: 'running' });
    const client = await RunnerClient.connect(join(root, 'run.sock'), 1, message => messages.push(message));
    try { client.send({ type: 'stop' }); await new Promise(resolve => setTimeout(resolve, 100)); assert.deepEqual(messages.map(m => m.type), ['attached', 'phase']); assert.equal(messages[1].seq, 2); assert.equal(commands[0]?.type, 'stop'); }
    finally { client.close(); }
  } finally { await server.close(); await rm(root, { recursive: true, force: true }); }
});

test('server closes a connection that sends an oversized frame', async () => {
  const root = await mkdtemp(join(tmpdir(), 'plait-socket-'));
  const server = new RunnerServer(join(root, 'run.sock'), () => ({ type: 'attached', state: 'running', seq: 0, snapshot: {}, pending_approval: null, head: null, before_record_id: null }), () => assert.fail('oversized command delivered'));
  try {
    await server.listen();
    const socket = createConnection(join(root, 'run.sock'));
    socket.on('error', () => {});
    await once(socket, 'connect');
    const closed = once(socket, 'close');
    socket.write(Buffer.alloc(COMMAND_LIMIT + 1, 0x20));
    await closed;
  } finally { await server.close(); await rm(root, { recursive: true, force: true }); }
});

test('second attach replaces first client, ignores stale acks, and replays remaining events', async () => {
  const root = await mkdtemp(join(tmpdir(), 'plait-socket-'));
  const sock = join(root, 'run.sock');
  const commands: Command[] = [];
  const server = new RunnerServer(sock, () => ({ type: 'attached', state: 'running', seq: 2, snapshot: {}, pending_approval: null, head: null, before_record_id: null }), command => commands.push(command));
  try {
    await server.listen();
    server.publish({ type: 'phase', seq: 1, state: 'preparing' });
    server.publish({ type: 'phase', seq: 2, state: 'running' });

    let firstClosed = false;
    const firstMessages: RunnerMessage[] = [];
    const first = await RunnerClient.connect(sock, 0, message => firstMessages.push(message), () => { firstClosed = true; });
    await new Promise(resolve => setTimeout(resolve, 60));
    assert.deepEqual(firstMessages.map(m => m.type), ['attached', 'phase', 'phase']);

    // Second client attaches with last_seq=0: first client is closed and second gets full replay.
    const secondMessages: RunnerMessage[] = [];
    const second = await RunnerClient.connect(sock, 0, message => secondMessages.push(message));
    await new Promise(resolve => setTimeout(resolve, 80));
    assert.equal(firstClosed, true);
    assert.deepEqual(secondMessages.map(m => m.type), ['attached', 'phase', 'phase']);

    // Second client acks seq 1; buffer retains seq 2.
    second.send({ type: 'ack', seq: 1 });
    await new Promise(resolve => setTimeout(resolve, 60));
    assert.deepEqual(server.unacknowledged().map(e => e.seq), [2]);

    // Third client attaches with older last_seq=0 (already pruned seq 1): attach succeeds and replays remaining seq 2.
    const thirdMessages: RunnerMessage[] = [];
    const third = await RunnerClient.connect(sock, 0, message => thirdMessages.push(message));
    await new Promise(resolve => setTimeout(resolve, 80));
    assert.deepEqual(thirdMessages.map(m => m.type), ['attached', 'phase']);
    assert.equal(thirdMessages[1].seq, 2);

    first.close(); second.close(); third.close();
  } finally { await server.close(); await rm(root, { recursive: true, force: true }); }
});
