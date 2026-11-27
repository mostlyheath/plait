import { lstat } from 'node:fs/promises';
import { dirname } from 'node:path';
import { Socket } from 'node:net';
import { once } from 'node:events';
import { verifyPeer } from './runner/socket.ts';
import { HostError } from './protocol.ts';

export class HostTransportError extends Error {}

export async function readReply(stream: AsyncIterable<Buffer>): Promise<unknown> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const data of stream) {
    const newline = data.indexOf(10);
    const part = newline < 0 ? data : data.subarray(0, newline + 1);
    size += part.length;
    if (size > 1048576) throw new Error('Host reply exceeds frame bound');
    chunks.push(part);
    if (newline >= 0) {
      const reply = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      if (reply?.ok === true && 'result' in reply) return reply.result;
      if (reply?.ok === false && typeof reply.error?.code === 'string' && typeof reply.error.message === 'string')
        throw new HostError(reply.error.code, reply.error.message);
      throw new Error('Invalid host reply');
    }
  }
  throw new Error('Host closed before reply');
}

export function hostClient(path: string, timeout = 60_000, verify = verifyPeer) {
  return async (request: Record<string, unknown>): Promise<unknown> => {
    const frame = Buffer.from(JSON.stringify(request) + '\n');
    if (frame.length > 65536) throw new Error('Host request exceeds frame bound');
    const socket = new Socket();
    const controller = new AbortController();
    async function exchange() {
      const [parent, leaf] = await Promise.all([lstat(dirname(path)), lstat(path)]);
      const uid = process.geteuid?.();
      if (!parent.isDirectory() || !leaf.isSocket() || parent.uid !== uid || leaf.uid !== uid || ((leaf.mode | parent.mode) & 0o077))
        throw new Error('Host socket and directory must be private and owned by this user');
      if (socket.destroyed) throw new Error('Host request expired');
      socket.connect(path);
      await once(socket, 'connect', { signal: controller.signal });
      await verify(socket, controller.signal);
      controller.signal.throwIfAborted();
      socket.write(frame);
      return readReply(socket);
    }
    let timer: NodeJS.Timeout | undefined;
    // Cover ownership checks, connection, peer verification and the entire first frame.
    socket.on('error', () => {});
    try {
      return await Promise.race([exchange(), new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('Host request timed out')), timeout);
      })]);
    } catch (error) {
      if (error instanceof HostError) throw error;
      throw new HostTransportError(String(error), { cause: error });
    } finally {
      clearTimeout(timer);
      controller.abort();
      socket.destroy();
    }
  };
}
