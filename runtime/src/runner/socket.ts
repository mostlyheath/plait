import { createServer, createConnection, type Server, type Socket } from 'node:net';
import { chmod, lstat, unlink } from 'node:fs/promises';
import { COMMAND_LIMIT, EVENT_LIMIT, encodeFrame, EventBuffer, FrameDecoder, parseCommand, parseRunnerMessage, type Command, type Attached, type Event, type RunnerMessage } from './protocol.ts';
import { helper } from './registry.ts';

// Node does not expose peer credentials; the host checks the connected descriptor.
export async function verifyPeer(socket: Socket, signal?: AbortSignal): Promise<void> {
  const fd = (socket as Socket & { _handle?: { fd?: number } })._handle?.fd;
  if (fd === undefined || !await helper(fd, 'runner-peer-fd', signal)) throw new Error('socket peer check failed');
}
export class RunnerServer {
  private server?: Server;
  private readonly sockets = new Set<Socket>();
  private current?: Socket;
  private readonly events = new EventBuffer();
  private readonly path: string;
  private readonly snapshot: () => Attached;
  private readonly command: (command: Command) => void;
  private readonly onAttach: () => void;
  constructor(path: string, snapshot: () => Attached, command: (command: Command) => void, onAttach: () => void = () => {}) { this.path = path; this.snapshot = snapshot; this.command = command; this.onAttach = onAttach; }
  async listen(): Promise<void> {
    socketPathFits(this.path);
    const server = createServer(socket => { socket.pause(); void this.accept(socket); }); this.server = server;
    await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(this.path, () => { server.off('error', reject); resolve(); }); });
    await chmod(this.path, 0o600);
  }
  private async accept(socket: Socket): Promise<void> {
    try {
      await verifyPeer(socket); this.sockets.add(socket);
      let attached = false; const decoder = new FrameDecoder(COMMAND_LIMIT, parseCommand);
      socket.on('data', (data: Buffer) => { try { for (const message of decoder.push(data)) {
        if (!attached) {
          if (message.type !== 'attach') throw new Error('attach required');
          const floor = this.events.floor();
          const replay = this.events.replay(message.last_seq);
          // Enforce a single attached supervisor: close any previous client and only honor acks/commands from the current one.
          const previous = this.current;
          this.current = socket;
          attached = true;
          if (previous && previous !== socket) previous.destroy();
          this.onAttach();
          socket.write(encodeFrame({ ...this.snapshot(), floor }, EVENT_LIMIT));
          for (const event of replay) socket.write(encodeFrame(event, EVENT_LIMIT));
        } else {
          if (message.type === 'attach') throw new Error('already attached');
          if (this.current !== socket) { socket.destroy(); return; }
          if (message.type === 'ack') this.events.ack(message.seq);
          this.command(message);
        }
      } } catch { socket.destroy(); } });
      socket.on('close', () => { this.sockets.delete(socket); if (this.current === socket) this.current = undefined; }); socket.on('error', () => {}); socket.resume();
    } catch { socket.destroy(); }
  }
  publish(event: Event): void {
    const frame = encodeFrame(event, EVENT_LIMIT); this.events.publish(event); this.current?.write(frame);
  }
  unacknowledged(): Event[] { return this.events.unacknowledged(); }
  async close(): Promise<void> { for (const socket of this.sockets) socket.destroy(); if (this.server) await new Promise<void>((resolve, reject) => this.server!.close(error => error ? reject(error) : resolve())); await unlink(this.path).catch((error: NodeJS.ErrnoException) => { if (error.code !== 'ENOENT') throw error; }); }
}
export class RunnerClient {
  private readonly socket: Socket;
  private constructor(socket: Socket) { this.socket = socket; }
  static async connect(path: string, lastSeq: number, receive: (message: RunnerMessage) => void, onClose: () => void = () => {}): Promise<RunnerClient> {
    const stat = await lstat(path); const parent = await lstat(path.substring(0, path.lastIndexOf('/')) || '/');
    if (!stat.isSocket() || stat.uid !== process.getuid?.() || (stat.mode & 0o077) !== 0 || !parent.isDirectory() || parent.uid !== process.getuid?.() || (parent.mode & 0o077) !== 0) throw new Error('invalid runner socket');
    const socket = createConnection(path); socket.pause();
    try { await new Promise<void>((resolve, reject) => { socket.once('connect', resolve); socket.once('error', reject); }); await verifyPeer(socket);
      const decoder = new FrameDecoder(EVENT_LIMIT, parseRunnerMessage); socket.on('data', (data: Buffer) => { try { for (const message of decoder.push(data)) receive(message); } catch { socket.destroy(); } }); socket.on('error', () => {}); socket.on('close', onClose);
      socket.write(encodeFrame({ type: 'attach', protocol: 1, last_seq: lastSeq }, COMMAND_LIMIT)); socket.resume(); return new RunnerClient(socket);
    } catch (error) { socket.destroy(); throw error; }
  }
  send(command: Exclude<Command, { type: 'attach' }>): void { this.socket.write(encodeFrame(parseCommand(command), COMMAND_LIMIT)); }
  close(): void { this.socket.destroy(); }
}

/** macOS refuses Unix socket paths over 103 bytes; fail with a reason instead of a silent timeout. */
export function socketPathFits(path: string): void {
  if (Buffer.byteLength(path) > 103) throw new Error(`Runner socket path is too long (${Buffer.byteLength(path)} bytes): ${path}`);
}
