import { mkdtemp, realpath, rm } from 'node:fs/promises';
import { homedir } from 'node:os';
import { resolve } from 'node:path';
import { PipeTransport, StreamableHttpTransport } from '@earendil-works/pi-mcp';
import type { ToolState } from '../tools.ts';
import { unsafeLoaderEnvironment } from './config.ts';
import type { Entry } from './config.ts';
import { guardianPipe } from './pipe.ts';
import { commandPlatform, trustedPlatform } from '../command/platform.ts';
import { hostAccount, loginEnvironment } from '../command/login-environment.ts';
import { runGuardian } from '../command/supervisor.ts';
import { checkRoot } from '../safety.ts';
import { readableFolders } from '../skills.ts';
import { expand, Secrets } from './secrets.ts';

export const MCP_START_TIMEOUT_MS = 10_000;
export const MCP_CALL_TIMEOUT_MS = 60_000;
export const MCP_MESSAGE_BYTES = 16 * 1024 * 1024;
const home = (value: string) => value === '~' ? homedir() : value.startsWith('~/') ? resolve(homedir(), value.slice(2)) : value;

/** The only Unix socket exception outside command scratch is Apple's DNS resolver. */
export const MCP_NETWORK_POLICY = `
(allow network-outbound (remote ip))
(allow network-inbound network-bind (local ip))
(allow network-outbound (literal "/private/var/run/mDNSResponder"))`;

/** Secrets and subprocesses are first touched by search, never by discovery or settings. */
export async function transport(entry: Entry, state: ToolState, secrets: Secrets, signal: AbortSignal) {
  const root = state.snapshot.checkout;
  const config = entry.config;
  await checkRoot(root, state.rootIdentity);
  const trusted = state.commandPolicy === 'trusted';
  if (trusted) trustedPlatform();
  else commandPlatform();
  if (!trusted && unsafeLoaderEnvironment(config)) throw new Error('MCP loader environment is not allowed in a sandboxed project.');
  let scratch: string | undefined;
  let env: NodeJS.ProcessEnv;
  let policy = 'trusted';
  const cleanup = async () => { if (scratch) await rm(scratch, { recursive: true, force: true }); };
  try {
    // HTTP needs a process only when an explicit !command resolves a header.
    const values = 'command' in config ? config.env : config.headers;
    const needsProcess = 'command' in config || Object.values(values ?? {}).some(value => value.startsWith('!'));
    const needsEnvironment = needsProcess || Object.values(values ?? {}).some(value => /\$(?:[A-Za-z_]|\{)/.test(value));
    if (trusted && needsEnvironment) {
      env = (await (state.loginEnvironment ??= loginEnvironment(hostAccount(), signal))).env;
    } else if (!trusted && needsEnvironment) {
      const platform = commandPlatform();
      scratch = await realpath(await mkdtemp(platform.scratchPrefix));
      const prepared = await platform.prepare(root, scratch, signal, state.buildCache, readableFolders(state.skills ?? []));
      env = prepared.env;
      // Same filesystem permissions as commands; network is the explicit MCP exception.
      policy = prepared.policy + MCP_NETWORK_POLICY;
    } else env = {};
    const resolved: Record<string, string> = Object.create(null);
    for (const [key, value] of Object.entries(values ?? {})) {
      if (value.startsWith('!')) {
        // Neither the command nor its output enters a transcript or a command log.
        try {
          let bytes = 0;
          resolved[key] = (await runGuardian({ root, text: `{\n${value.slice(1)}\n} 2>/dev/null`, env, isolateEnvironment: true, checkoutFd: state.lease?.fd, lifecycleFd: state.lease?.lifecycleFd,
            ...(trusted ? { trusted: true as const } : { policy }) }, signal, () => {}, MCP_START_TIMEOUT_MS)).trim();
          bytes = Buffer.byteLength(resolved[key]);
          if (bytes >= 16000) throw new Error();
        } catch { throw new Error('An MCP credential command failed.'); }
        if (!resolved[key] || resolved[key] === 'Command completed.') throw new Error('An MCP credential command returned no value.');
      } else resolved[key] = expand(value, env);
      secrets.add(resolved[key]);
    }
    if ('url' in config) {
      const connection = new StreamableHttpTransport({ url: config.url, headers: resolved,
        maxMessageBytes: MCP_MESSAGE_BYTES, openGetStream: false, reconnect: { maxRetries: 0 },
        // No redirect may forward a configured header to an unconfigured endpoint.
        fetch: (input, init) => fetch(input, { ...init, redirect: 'error', signal: AbortSignal.any([signal, AbortSignal.timeout(Math.min((config.timeout ?? 60) * 1000, 900000)), ...(init?.signal ? [init.signal] : [])]) }),
      });
      return { connection, cleanup, stderr: () => '', failure: () => undefined };
    }
    let failure: string | undefined;
    const connection = new PipeTransport({ maxMessageBytes: MCP_MESSAGE_BYTES, maxStderrBytes: 8192, closeTimeoutMs: 0,
      open: async handlers => guardianPipe(resolve(root, home(config.cwd ?? '.')), policy, home(config.command), (config.args ?? []).map(home),
        trusted ? { ...env, ...resolved } : { ...resolved, ...env }, state, handlers, signal, message => { failure = message; }),
    });
    return { connection, cleanup, stderr: () => connection.stderr, failure: () => failure };
  } catch (error) { await cleanup(); throw error; }
}
