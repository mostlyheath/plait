// Agent harnesses a thread can run in, as the version 4 `harnesses` operation
// lists them, and the one Codex app-server that Codex threads share.
//
// Pi ships with the host. Codex is the person's own install: Plait finds it,
// checks its version, and asks it for its models and whether it is signed in.
// Plait reads none of Codex's files or credentials. Claude Code is in
// ./claude/harness.ts.
import { hostAccount, loginEnvironment } from './command/login-environment.ts';
import { codexAccount, codexModels, findCodex } from './codex/install.ts';
import type { CodexInstall, CodexModel } from './codex/install.ts';
import { declineUnsupported } from './codex/approval.ts';
import { CODEX_SCHEMA_VERSION, threadOf } from './codex/protocol.ts';
import { AppServer } from './codex/server.ts';
import { fail as failWith, THINKING_LEVELS } from './protocol.ts';
import type { HarnessId, HarnessMode, ThinkingLevel } from './protocol.ts';
import type { ApprovalsReviewer, AskForApproval, SandboxMode, SandboxPolicy } from './codex/protocol.ts';
import type { Store } from './store.ts';

/** `setup_required`: the harness needs a one-time download the person starts (Claude Code's Agent SDK). */
export type HarnessState = 'ready' | 'not_installed' | 'unsupported_version' | 'signed_out' | 'unavailable' | 'setup_required';
export type WireModel = {
  provider: string; model_id: string; name: string; authenticated: boolean;
  reasoning: boolean; thinking_levels: ThinkingLevel[]; context_window: number | null; input: string[];
};
export type HarnessEntry = {
  id: HarnessId; name: string; state: HarnessState; note: string | null; version: string | null;
  supported: { min: string; tested: string } | null; path: string | null;
  capabilities: { steer: boolean; approvals: boolean; thinking: boolean; usage: boolean };
  protection: string; models: WireModel[] | null;
  /** The harness's own permission modes; empty for Pi, whose threads use Plait's approval mode. */
  modes: HarnessMode[]; default_mode: string | null;
};
/** Codex threads name this provider, so a model choice says which harness runs it. */
export const CODEX_PROVIDER = 'codex';
const PATH_SETTING = 'harness_path:codex';
const MODE_SETTING = 'harness_mode:codex';

/** Codex's own permission presets, the same choices its `/permissions` menu offers. */
export const CODEX_MODES: HarnessMode[] = [
  { id: 'read_only', label: 'Read only', detail: 'Codex asks before any change; its sandbox lets commands only read.' },
  { id: 'ask', label: 'Ask', detail: "Codex asks when it needs more than its sandbox: commands can write this checkout, with no network." },
  { id: 'approve_for_me', label: 'Approve for me', detail: "Codex's reviewer approves or blocks each request itself, inside the same sandbox." },
  { id: 'full_access', label: 'Full access', detail: 'Codex never asks and has no sandbox: it can change any file and use the network.' },
];
/** Like `codex --yolo`. */
export const CODEX_DEFAULT_MODE = 'full_access';
export type CodexPolicy = {
  approvalPolicy: AskForApproval; sandbox: SandboxMode; sandboxPolicy: SandboxPolicy; approvalsReviewer: ApprovalsReviewer;
};
const WORKSPACE: SandboxPolicy = { type: 'workspaceWrite', writableRoots: [], networkAccess: false, excludeTmpdirEnvVar: false, excludeSlashTmp: false };
/** Every field is set, so the person's Codex config cannot change what the chosen mode means. */
export function codexPolicy(mode: string): CodexPolicy {
  switch (mode) {
    case 'read_only': return { approvalPolicy: 'on-request', sandbox: 'read-only', sandboxPolicy: { type: 'readOnly', networkAccess: false }, approvalsReviewer: 'user' };
    case 'ask': return { approvalPolicy: 'on-request', sandbox: 'workspace-write', sandboxPolicy: WORKSPACE, approvalsReviewer: 'user' };
    case 'approve_for_me': return { approvalPolicy: 'on-request', sandbox: 'workspace-write', sandboxPolicy: WORKSPACE, approvalsReviewer: 'auto_review' };
    default: return { approvalPolicy: 'never', sandbox: 'danger-full-access', sandboxPolicy: { type: 'dangerFullAccess' }, approvalsReviewer: 'user' };
  }
}
/** The mode Codex reports using, from a thread start or resume reply; `custom` when it matches none. */
export function codexReported(reply: { approvalPolicy?: unknown; sandbox?: { type?: unknown } | unknown; approvalsReviewer?: unknown }): string {
  const sandbox = (reply.sandbox as { type?: unknown } | undefined)?.type;
  const reviewer = reply.approvalsReviewer ?? 'user';
  if (reply.approvalPolicy === 'never' && sandbox === 'dangerFullAccess') return 'full_access';
  if (reply.approvalPolicy !== 'on-request') return 'custom';
  if (sandbox === 'readOnly') return 'read_only';
  if (sandbox === 'workspaceWrite') return reviewer === 'auto_review' ? 'approve_for_me' : reviewer === 'user' ? 'ask' : 'custom';
  return 'custom';
}
/** How long a Codex check is reused. A Settings refresh or a new path checks again at once. */
const CHECK_TTL_MS = 60_000;
/** An app-server with no thread left closes after this, so back-to-back runs share one. */
const IDLE_CLOSE_MS = 30_000;

export const PI_ENTRY: HarnessEntry = {
  id: 'pi', name: 'Pi', state: 'ready', note: null, version: null, supported: null, path: null,
  capabilities: { steer: true, approvals: true, thinking: true, usage: true },
  protection: "Plait's command sandbox and your approvals. Files you had already changed are protected.",
  models: null, modes: [], default_mode: null,
};
const CODEX_PROTECTION = "The thread's Codex permissions and Codex's own sandbox. Plait's command policy and its guard on files you had already changed do not apply.";

/** Plait's thinking levels that a Codex model offers, in Plait's order. Codex calls off `none`. */
export function codexLevels(efforts: string[]): ThinkingLevel[] {
  return THINKING_LEVELS.filter(level => efforts.includes(level === 'off' ? 'none' : level));
}
/** The Codex effort for a Plait level; `undefined` lets Codex use the model's default. */
export function codexEffort(level: ThinkingLevel | undefined, efforts: string[]): string | undefined {
  if (level === undefined) return undefined;
  const effort = level === 'off' ? 'none' : level;
  return efforts.includes(effort) ? effort : undefined;
}
export function wireModel(model: CodexModel): WireModel {
  const levels = codexLevels(model.efforts);
  return {
    provider: CODEX_PROVIDER, model_id: model.id, name: model.name, authenticated: true,
    reasoning: levels.some(level => level !== 'off'), thinking_levels: levels, context_window: null, input: ['text'],
  };
}

type Check = { install: CodexInstall; env: NodeJS.ProcessEnv; models: CodexModel[]; entry: HarnessEntry; at: number };
/** One Codex thread's handlers on the shared server. */
export type Route = { notify(method: string, params: any): void; request(method: string, params: any): Promise<unknown> };
export type Lease = {
  server: AppServer; install: CodexInstall; models: CodexModel[];
  /** Sends this Codex thread's notifications and requests to `route` until the returned function is called. */
  route(thread: string, route: Route): () => void;
  release(): void;
};
export type CodexDependencies = {
  find: typeof findCodex;
  environment(): Promise<NodeJS.ProcessEnv>;
  start(path: string, env: NodeJS.ProcessEnv): Promise<AppServer>;
  now(): number;
  idleMs: number;
};
const defaults: CodexDependencies = {
  find: findCodex,
  // The host runs as a per-user service with a minimal environment; Codex expects the login one.
  environment: async () => (await loginEnvironment(hostAccount(), AbortSignal.timeout(15_000))).env,
  start: (path, env) => AppServer.start(path, { env, requestTimeoutMs: 60_000 }),
  now: Date.now,
  idleMs: IDLE_CLOSE_MS,
};

type Probe = { install: CodexInstall; env: NodeJS.ProcessEnv; at: number };
type Shared = { server: Promise<{ server: AppServer; check: Check }>; ready?: AppServer; users: number; idle?: NodeJS.Timeout };

/** Finds and checks Codex, and keeps one app-server while any Codex thread runs. */
export class CodexHarness {
  readonly supportsSteer: boolean = true;
  private check?: Promise<Check>;
  /** The latest finished check, for decisions that must not wait. */
  private latest?: Check;
  private readonly routes = new Map<string, Route>();
  private shared?: Shared;
  private readonly deps: CodexDependencies;
  private readonly store: Store;
  constructor(store: Store, deps: Partial<CodexDependencies> = {}) {
    this.store = store;
    this.deps = { ...defaults, ...deps };
  }
  path() { return this.store.setting(PATH_SETTING) ?? undefined; }
  /** The mode new Codex threads start in. */
  defaultMode() {
    const saved = this.store.setting(MODE_SETTING);
    return saved && CODEX_MODES.some(mode => mode.id === saved) ? saved : CODEX_DEFAULT_MODE;
  }
  setDefaultMode(mode: string) {
    if (!CODEX_MODES.some(candidate => candidate.id === mode)) failWith('invalid_request', 'Codex has no such mode.');
    this.store.setSetting(MODE_SETTING, mode);
    // The default is part of the listed entry; the rest of a recent check stays valid.
    if (this.latest) this.remember({ ...this.latest, entry: { ...this.latest.entry, default_mode: mode } });
  }
  setPath(path: string | undefined) {
    this.store.setSetting(PATH_SETTING, path ?? null);
    this.check = undefined; this.latest = undefined;
  }
  /** The list entry, from a recent check unless `fresh`. */
  async entry(fresh = false): Promise<HarnessEntry> { return (await this.checked(fresh)).entry; }
  /** A model this Codex offers, or a refusal the client can show. */
  async model(modelId: string): Promise<CodexModel> {
    const check = await this.checked(false);
    if (check.entry.state !== 'ready') fail(check.entry.note ?? 'Codex is not ready.');
    const model = check.models.find(candidate => candidate.id === modelId)
      ?? (await this.checked(true)).models.find(candidate => candidate.id === modelId);
    return model ?? fail('This Codex model is not offered by your Codex. Choose another.');
  }
  /**
   * Refuses at once when a recent check says the model cannot run. Without a
   * recent check it says nothing: the run checks over the server it starts,
   * so a submission never starts Codex twice.
   */
  refuse(modelId: string) {
    const check = this.latest;
    if (!check || this.deps.now() - check.at >= CHECK_TTL_MS) return;
    if (check.entry.state !== 'ready') fail(check.entry.note ?? 'Codex is not ready.');
    if (!check.models.some(candidate => candidate.id === modelId)) fail('This Codex model is not offered by your Codex. Choose another.');
  }
  /** Only the app-server catalog can establish image support. */
  imageInput(modelId: string): 'yes' | 'no' | 'unknown' {
    const input = this.latest?.models.find(model => model.id === modelId)?.input;
    return input ? input.includes('image') ? 'yes' : 'no' : 'unknown';
  }
  /** The shared app-server for one run; `release` when the run ends. */
  async acquire(): Promise<Lease> {
    if (!this.shared) {
      const shared: Shared = { server: this.launch(), users: 0 };
      this.shared = shared;
      // A failed start or an exit leaves no server behind for the next run to reuse.
      shared.server.then(({ server }) => {
        shared.ready = server;
        return server.exited.then(() => { if (this.shared === shared) this.shared = undefined; });
      }, () => { if (this.shared === shared) this.shared = undefined; });
    }
    const shared = this.shared;
    shared.users++;
    if (shared.idle) { clearTimeout(shared.idle); shared.idle = undefined; }
    let released = false;
    const release = () => {
      if (released) return; released = true;
      if (--shared.users > 0 || this.shared !== shared) return;
      shared.idle = setTimeout(() => void this.closeShared(shared), this.deps.idleMs);
      shared.idle.unref?.();
    };
    try {
      const { server, check } = await shared.server;
      const route = (thread: string, handlers: Route) => {
        this.routes.set(thread, handlers);
        return () => { if (this.routes.get(thread) === handlers) this.routes.delete(thread); };
      };
      return { server, install: check.install, models: check.models, route, release };
    } catch (error) { release(); throw error; }
  }
  async close() { if (this.shared) await this.closeShared(this.shared); }
  /** Starts the shared server. Without a recent check, the check is made over this server. */
  private async launch(): Promise<{ server: AppServer; check: Check }> {
    const recent = this.latest && this.deps.now() - this.latest.at < CHECK_TTL_MS ? this.latest : undefined;
    const probe = recent ?? await this.probe();
    if (probe.install.state !== 'ready' || !probe.install.path) {
      const check = this.remember(this.unready(probe));
      fail(check.entry.note ?? 'Codex is not ready.');
    }
    const server = await this.deps.start(probe.install.path, probe.env);
    this.dispatch(server);
    const check = recent ?? this.remember(await this.describe(probe, server));
    if (check.entry.state !== 'ready') { await server.close(); fail(check.entry.note ?? 'Codex is not ready.'); }
    return { server, check };
  }
  /** Each message goes to the run of the Codex thread it names; nothing else is answered. */
  private dispatch(server: AppServer) {
    server.onNotification((method, params) => {
      const thread = threadOf(params);
      if (thread !== undefined) this.routes.get(thread)?.notify(method, params);
    });
    server.onRequest(async (method, params) => {
      const thread = threadOf(params);
      const route = thread === undefined ? undefined : this.routes.get(thread);
      if (route) return route.request(method, params);
      const refusal = declineUnsupported(method);
      if (refusal !== undefined) return refusal;
      throw new Error('Plait does not answer this request.');
    });
  }
  private async closeShared(shared: Shared) {
    if (this.shared === shared) this.shared = undefined;
    if (shared.idle) clearTimeout(shared.idle);
    await shared.server.then(({ server }) => server.close(), () => {});
  }
  private remember(check: Check) {
    this.latest = check; this.check = Promise.resolve(check);
    return check;
  }
  private checked(fresh: boolean): Promise<Check> {
    const now = this.deps.now();
    if (!fresh && this.check) {
      // A failed or stale check is repeated; a recent one is reused.
      return this.check.then(check => now - check.at < CHECK_TTL_MS ? check : this.recheck());
    }
    return this.recheck();
  }
  private recheck(): Promise<Check> {
    const check = this.run().then(result => { if (this.check === check) this.latest = result; return result; });
    this.check = check;
    check.catch(() => { if (this.check === check) this.check = undefined; });
    return check;
  }
  /** Finds the install and reads its version; starts nothing. */
  private async probe(): Promise<Probe> {
    const at = this.deps.now();
    const configured = this.path();
    const env = await this.deps.environment();
    return { at, env, install: await this.deps.find(configured === undefined ? { env } : { path: configured, env }) };
  }
  private base(install: CodexInstall) {
    return {
      id: 'codex' as const, name: 'Codex', version: install.version ?? null,
      supported: { min: CODEX_SCHEMA_VERSION, tested: CODEX_SCHEMA_VERSION }, path: install.path ?? null,
      capabilities: { steer: true, approvals: true, thinking: true, usage: true }, protection: CODEX_PROTECTION,
      modes: CODEX_MODES, default_mode: this.defaultMode(),
    };
  }
  private unready({ install, env, at }: Probe): Check {
    const state: HarnessState = install.state === 'error' ? 'unavailable' : install.state === 'ready' ? 'unavailable' : install.state;
    return { install, env, models: [], at, entry: { ...this.base(install), state, note: install.message, models: [] } };
  }
  /** Asks a running server whether Codex is signed in and which models it offers. */
  private async describe({ install, env, at }: Probe, server: AppServer): Promise<Check> {
    const base = this.base(install);
    try {
      if (await codexAccount(server) === 'signed_out') {
        return { install, env, models: [], at, entry: { ...base, state: 'signed_out', note: 'Sign in with `codex login` in a terminal, then Refresh.', models: [] } };
      }
      const models = await codexModels(server);
      return { install, env, models, at, entry: { ...base, state: 'ready', note: install.untested ? install.message : null, models: models.map(wireModel) } };
    } catch (error) {
      return { install, env, models: [], at, entry: { ...base, state: 'unavailable', note: `Codex did not answer: ${(error as Error).message}`, models: [] } };
    }
  }
  private async run(): Promise<Check> {
    const probe = await this.probe();
    if (probe.install.state !== 'ready') return this.unready(probe);
    // A server already running is asked; otherwise one is started just to ask, and closed.
    const running = this.shared?.ready;
    if (running) return this.describe(probe, running);
    const server = await this.deps.start(probe.install.path!, probe.env).catch(error => error as Error);
    if (server instanceof Error) {
      return { ...probe, models: [], entry: { ...this.base(probe.install), state: 'unavailable', note: `Codex could not start: ${server.message}`, models: [] } };
    }
    try { return await this.describe(probe, server); } finally { await server.close(); }
  }
}

/** Codex not being ready, or lacking a model, refuses like a missing Pi model. */
function fail(message: string): never { return failWith('model_unavailable', message); }
