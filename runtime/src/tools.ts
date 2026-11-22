import { defineTool, createRegistry, defineExtension } from '@earendil-works/pi-durable';
import type { ToolExecutionApi } from '@earendil-works/pi-durable';
import type { Context } from '@earendil-works/chord/context';
import { fileAction, safePath, dirtyPaths, ignoredPaths, pathKey, sameImage } from './safety.ts';
import { command } from './command.ts';
import { trustedCommand } from './command/trusted.ts';
import { TRUSTED_COMMAND_DESCRIPTION } from './command/policy.ts';
import type { CommandPolicy } from './command/policy.ts';
import type { LoginEnvironment } from './command/login-environment.ts';
import type { FileIdentity, LeaseHandle, Postimage } from './safety.ts';
import type { BuildCache } from './build-cache.ts';
import { fail, HostError } from './protocol.ts';
import { rangeArgs, rangeText, outputTail } from './tool-output.ts';
import type { CommandOutputs, TextRange } from './tool-output.ts';
import type { Snapshot } from './protocol.ts';
import type { AuthorizationPolicy, RunAuthorization } from './authorization.ts';
import type { McpRun } from './mcp/tools.ts';
import { readableFolders, skillFile } from './skills.ts';
import type { SkillEntry } from './skills.ts';

export type ToolState = {
  snapshot: Snapshot; vcs?: 'git' | 'folder'; baseline: string[]; protected: FileIdentity[];
  authorization: RunAuthorization;
  lease?: LeaseHandle;
  rootIdentity: FileIdentity;
  buildCache: BuildCache;
  outputs: CommandOutputs;
  owned: Map<string, Postimage>;
  record(image: Postimage): void;
  // Read once at admission. Absent means sandboxed, the default.
  commandPolicy?: CommandPolicy;
  /** Skills discovered at run start; their folders are readable but never written. */
  skills?: SkillEntry[];
  forget?(path: string): void;
  loginEnvironment?: Promise<LoginEnvironment>;
  mcp?: McpRun;
  mcpApproval?: AuthorizationPolicy;
};
export function registry(state: ToolState) {
  const result = createRegistry();
  const schemas = {
    read: { path: { type: 'string' }, offset: { type: 'integer', minimum: 0 }, limit: { type: 'integer', minimum: 4, maximum: 12000 } },
    read_output: { id: { type: 'string' }, offset: { type: 'integer', minimum: 0 }, limit: { type: 'integer', minimum: 4, maximum: 12000 } },
    edit: { path: { type: 'string' }, old_text: { type: 'string' }, new_text: { type: 'string' } },
    write: { path: { type: 'string' }, content: { type: 'string' } },
    command: { command: { type: 'string' } },
  };
  const tools = (Object.keys(schemas) as (keyof typeof schemas)[]).map(name => defineTool({
    name,
    description: name === 'read' ? 'Read a UTF-8 regular file inside the checkout (a relative path) or inside a listed skill\'s directory (an absolute path). Optional offset is a zero-based byte offset, limit is bytes (default 8192, maximum 12000). Follow the returned continuation to read the rest.'
      : name === 'read_output' ? 'Read stored command output from this thread without rerunning the command. Use the id from its result, or the original command tool-call ID after an interruption (most recent match). Optional offset is a zero-based byte offset and limit is bytes (default 8192, maximum 12000).'
      : name === 'edit' ? 'Replace one unique exact old_text with new_text. May require approval; dirty paths are protected.'
      : name === 'write' ? 'Create a new file only; never overwrite. May require approval.'
      : state.commandPolicy === 'trusted' ? TRUSTED_COMMAND_DESCRIPTION
      : 'Run a command (may require approval): checkout read-only, writes only to private TMPDIR and this thread\'s CARGO_HOME/CARGO_TARGET_DIR cache, no external network or checkout/Git writes, 15 minute limit. Tests may use scratch-local Git repositories and Unix sockets. Do not start background services. Use offline locked builds.',
    parameters: { type: 'object', properties: schemas[name], required: Object.keys(schemas[name]).filter(key => key !== 'offset' && key !== 'limit'), additionalProperties: false },
    replay: 'unsafe' as const,
    executionMode: 'sequential' as const,
    outputLimits: { maxBytes: 24576, maxLines: 20000 },
    async execute(input: unknown, api: ToolExecutionApi, context: Context) {
      const args = Object.freeze({ ...input as Record<string, string> });
      const signal = AbortSignal.any([state.authorization.signal, ...(context.abortSignal ? [context.abortSignal] : [])]);
      state.authorization.check(signal);
      if (Buffer.byteLength(JSON.stringify(args)) > 16384) fail('permission_denied', 'Tool arguments exceed the bound.');
      let text: string;
      if (name === 'read') {
        const skill = skillFile(state.skills ?? [], args.path);
        // A skill's folder is its own root; the checkout lease and identity do not apply to it.
        const page = skill
          ? await fileAction<TextRange>(skill.root, 'read_range', skill.path, rangeArgs(args), undefined, skill.identity)
          : await fileAction<TextRange>(state.snapshot.checkout, 'read_range', args.path, rangeArgs(args), state.lease?.fd, state.rootIdentity, state.lease?.lifecycleFd);
        text = rangeText(page, offset => `read(${JSON.stringify({ path: args.path, offset })})`);
      } else if (name === 'read_output') {
        const { offset, limit } = rangeArgs(args);
        text = state.outputs.read(state.snapshot.id, args.id, offset, limit);
      }
      else {
        const key = args.path ? pathKey(args.path) : '';
        let expectedImage: Postimage | undefined;
        if (name !== 'command') {
          // File protections apply regardless of how the run obtains permission.
          safePath(args.path, state.baseline);
          const currentDirty = await dirtyPaths(state.snapshot.checkout, state.vcs);
          safePath(args.path, await ignoredPaths(state.snapshot.checkout, [args.path], state.vcs));
          safePath(args.path, currentDirty.filter(path => !state.owned.has(pathKey(path))));
          if (name === 'edit') {
            const current = await fileAction(state.snapshot.checkout, 'inspect', args.path, undefined, state.lease?.fd, state.rootIdentity, state.lease?.lifecycleFd);
            expectedImage = current.postimage;
            if (state.owned.has(key) && !sameImage(state.owned.get(key)!, current.postimage)) fail('permission_denied', 'File changed outside this thread.');
          }
        }
        await state.authorization.authorize(Object.freeze({
          toolId: api.callId, name, arguments: args, dirtyPaths: Object.freeze([...state.baseline]),
          ...(state.commandPolicy === 'trusted' ? { commandPolicy: state.commandPolicy } : {}),
        }), signal);
        // Stop can arrive between authorization and this continuation.
        state.authorization.check(signal);
        // Trusted mode is a separate path; it never loosens the sandboxed one.
        if (name === 'command') {
          const log = state.outputs.begin(state.snapshot.id, api.callId);
          const output = (chunk: string) => api.output(chunk);
          let failure: unknown;
          try {
            text = state.commandPolicy === 'trusted'
              ? await trustedCommand(state, args.command, signal, output, { capture: log.append })
              : await command(state.snapshot.checkout, args.command, state.baseline,
                signal, output, state.lease?.fd, state.rootIdentity, state.buildCache, log.append,
                readableFolders(state.skills ?? []), state.lease?.lifecycleFd, state.lease?.cleanup);
          } catch (error) { failure = error; text = ''; }
          const reference = log.finish();
          if (failure instanceof HostError) throw new HostError(failure.code, outputTail(failure.message) + reference);
          if (failure) throw new HostError('runtime_error', 'Command failed. Inspect its stored output.' + reference);
          text = outputTail(text) + reference;
        }
        else {
          const result = await fileAction(state.snapshot.checkout, name, args.path,
            name === 'edit' ? { expected: args.old_text, content: args.new_text, expected_image: expectedImage, protected: state.protected } : { content: args.content, protected: state.protected }, state.lease?.fd, state.rootIdentity, state.lease?.lifecycleFd);
          state.record(result.postimage);
          state.owned.set(pathKey(result.postimage.path), result.postimage);
          text = result.text;
        }
      }
      return { output: [{ type: 'text' as const, text }] };
    },
  }));
  result.install(defineExtension({ name: 'plait-local', tools }));
  state.mcp?.install(result);
  return result;
}
