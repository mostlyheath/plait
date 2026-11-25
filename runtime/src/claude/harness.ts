// Claude Code as a harness: the person's own `claude`, driven through the
// Agent SDK the host downloads when the person sets up Claude Code threads.
// Plait reads none of Claude Code's files or credentials, and accepts
// whichever sign-in the person's `claude` uses.
import { delimiter, join } from 'node:path';
import { hostAccount, loginEnvironment } from '../command/login-environment.ts';
import { fail as failWith } from '../protocol.ts';
import type { HarnessMode } from '../protocol.ts';
import type { HarnessEntry, HarnessState, WireModel } from '../harness.ts';
import { catalogFrom, claudeAuth, claudeCatalog, CLAUDE_MIN_VERSION, CLAUDE_SDK_VERSION, CLAUDE_TESTED_VERSION, findClaude } from './install.ts';
import type { ClaudeCatalog, ClaudeInstall, ClaudeModel } from './install.ts';
import { loadSdk, sdkPresent, setUpSdk } from './sdk.ts';
import type { ClaudeSdk } from './sdk.ts';
import type { ClaudeModeId } from './session.ts';
import type { SDKControlInitializeResponse } from '@anthropic-ai/claude-agent-sdk';

/** Claude Code threads name this provider, so a model choice says which harness runs it. */
export const CLAUDE_PROVIDER = 'claude';
const PATH_SETTING = 'harness_path:claude';
const MODE_SETTING = 'harness_mode:claude';
/** How long a check is reused. A Settings refresh, a new path, or a setup checks again at once. */
const CHECK_TTL_MS = 60_000;

/** Claude Code's own permission modes, as its Shift+Tab cycle and `--permission-mode` name them. */
export const CLAUDE_MODES: HarnessMode[] = [
  { id: 'ask', label: 'Ask', detail: 'Claude Code asks before each edit and command your Claude Code settings do not already allow.' },
  { id: 'accept_edits', label: 'Accept edits', detail: 'Claude Code edits files in this checkout without asking, and asks before other commands.' },
  { id: 'plan', label: 'Plan', detail: 'Claude Code reads and plans without changing anything, and asks before it starts on the plan.' },
  { id: 'auto', label: 'Auto', detail: "Claude Code's own safety check approves or blocks each request, and asks only when it cannot decide." },
  { id: 'bypass', label: 'Bypass', detail: 'Claude Code never asks and has no sandbox: it can change any file and run any command.' },
];
export const CLAUDE_DEFAULT_MODE = 'auto';
const SDK_MODES: Record<string, ClaudeModeId> = { ask: 'default', accept_edits: 'acceptEdits', plan: 'plan', auto: 'auto', bypass: 'bypassPermissions' };
/** The mode Claude Code takes for one of Plait's mode IDs. */
export function claudeMode(mode: string): ClaudeModeId { return SDK_MODES[mode] ?? 'auto'; }
/** The Plait mode for one Claude Code reports; `custom` for one Plait does not offer, such as `dontAsk` from the person's settings. */
export function claudeReported(mode: string | null | undefined): string | null {
  if (mode === null || mode === undefined) return null;
  return Object.entries(SDK_MODES).find(([, sdk]) => sdk === mode)?.[0] ?? 'custom';
}
const PROTECTION = "The thread's Claude Code mode and your Claude Code settings. Plait's command sandbox and its guard on files you had already changed do not apply.";
const SETUP_NOTE = `Set up Claude Code threads to download Anthropic's Agent SDK ${CLAUDE_SDK_VERSION} from the npm registry to this Mac. Plait then runs your installed \`claude\`.`;

/**
 * The host runs as a per-user service with a minimal environment; Claude Code
 * expects the login one. When the login shell cannot be read, the fixed
 * fallback PATH gains the folder Claude Code's own installer uses.
 */
async function claudeEnvironment(): Promise<NodeJS.ProcessEnv> {
  const account = hostAccount();
  const login = await loginEnvironment(account, AbortSignal.timeout(15_000));
  if (login.source !== 'fallback') return login.env;
  return { ...login.env, PATH: [login.env.PATH, join(account.home, '.local/bin')].filter(Boolean).join(delimiter) };
}

export function claudeWireModel(model: ClaudeModel): WireModel {
  return {
    provider: CLAUDE_PROVIDER, model_id: model.id, name: model.name, authenticated: true,
    reasoning: model.adaptive || model.levels.some(level => level !== 'off'), thinking_levels: model.levels, context_window: null, input: ['text'],
  };
}

/** What a run needs to start Claude Code. */
export type ClaudeLaunch = { install: ClaudeInstall & { path: string }; env: NodeJS.ProcessEnv; sdk: ClaudeSdk; models?: ClaudeModel[] };
export type ClaudeDependencies = {
  /** Where the set-up SDK lives; tests point this at the dev copy. */
  sdk: string;
  find: typeof findClaude;
  environment(): Promise<NodeJS.ProcessEnv>;
  catalog: typeof claudeCatalog;
  auth: typeof claudeAuth;
  fetch: typeof fetch;
  /** Tests serve their own archive; the host always downloads the pinned tarball and checks the pinned hash. */
  tarball?: { url: string; integrity: string };
  /** Where the model check runs Claude Code: a folder with no project settings of its own. */
  cwd: string;
  now(): number;
};
type Check = { install: ClaudeInstall; env?: NodeJS.ProcessEnv; models: ClaudeModel[]; entry: HarnessEntry; at: number };

/** Finds and checks Claude Code. Each run starts its own Claude Code process, so nothing is shared. */
export class ClaudeHarness {
  readonly supportsSteer: boolean = true;
  private check?: Promise<Check>;
  private latest?: Check;
  private setup?: Promise<void>;
  /** Why the last setup failed, shown until the next one. */
  private setupError?: string;
  private readonly deps: ClaudeDependencies;
  private readonly store: { setting(key: string): string | null; setSetting(key: string, value: string | null): void };
  constructor(store: ClaudeHarness['store'], deps: Partial<ClaudeDependencies> & { sdk: string; cwd: string }) {
    this.store = store;
    this.deps = {
      find: findClaude, catalog: claudeCatalog, auth: claudeAuth, fetch: (...args) => fetch(...args), now: Date.now,
      environment: claudeEnvironment,
      ...deps,
    };
  }
  path() { return this.store.setting(PATH_SETTING) ?? undefined; }
  defaultMode() {
    const saved = this.store.setting(MODE_SETTING);
    return saved && CLAUDE_MODES.some(mode => mode.id === saved) ? saved : CLAUDE_DEFAULT_MODE;
  }
  setDefaultMode(mode: string) {
    if (!CLAUDE_MODES.some(candidate => candidate.id === mode)) failWith('invalid_request', 'Claude Code has no such mode.');
    this.store.setSetting(MODE_SETTING, mode);
    if (this.latest) this.remember({ ...this.latest, entry: { ...this.latest.entry, default_mode: mode } });
  }
  setPath(path: string | undefined) {
    this.store.setSetting(PATH_SETTING, path ?? null);
    this.forget();
  }
  /** Downloads the pinned SDK, once; the reply's entry then says whether Claude Code is ready. Never throws. */
  async setUp(): Promise<void> {
    this.setup ??= (async () => {
      try {
        await setUpSdk({ directory: this.deps.sdk, fetch: this.deps.fetch, ...this.deps.tarball });
        this.setupError = undefined;
      } catch (error) {
        // The message is Plait's own (a status code, a hash mismatch, an unsafe archive), never a response body.
        this.setupError = `Setup did not finish: ${(error as Error).message.slice(0, 300)} Nothing was installed; try again.`;
      } finally { this.setup = undefined; this.forget(); }
    })();
    return this.setup;
  }
  async entry(fresh = false): Promise<HarnessEntry> { return (await this.checked(fresh)).entry; }
  async model(modelId: string): Promise<ClaudeModel> {
    const check = await this.checked(false);
    if (check.entry.state !== 'ready') fail(check.entry.note ?? 'Claude Code is not ready.');
    const model = check.models.find(candidate => candidate.id === modelId)
      ?? (await this.checked(true)).models.find(candidate => candidate.id === modelId);
    return model ?? fail('This model is not offered by your Claude Code. Choose another.');
  }
  /** Refuses at once when a recent check says the model cannot run; otherwise the run checks as it starts. */
  refuse(modelId: string) {
    const check = this.latest;
    if (!check || this.deps.now() - check.at >= CHECK_TTL_MS) return;
    if (check.entry.state !== 'ready') fail(check.entry.note ?? 'Claude Code is not ready.');
    if (!check.models.some(candidate => candidate.id === modelId)) fail('This model is not offered by your Claude Code. Choose another.');
  }
  /**
   * What a run needs. A recent check is reused; otherwise Claude Code is
   * found and its sign-in read, and the run's own `initialize` answer stands
   * in for the model list, so a run never starts Claude Code twice.
   */
  async launch(): Promise<ClaudeLaunch> {
    const recent = this.latest && this.deps.now() - this.latest.at < CHECK_TTL_MS && this.latest.entry.state === 'ready' ? this.latest : undefined;
    if (recent?.env && recent.install.path) {
      return { install: recent.install as ClaudeLaunch['install'], env: recent.env, sdk: await this.sdk(), models: recent.models };
    }
    if (!await sdkPresent(this.deps.sdk)) fail(SETUP_NOTE);
    const sdk = await this.sdk();
    const at = this.deps.now();
    const { install, env } = await this.probe();
    if (install.state !== 'ready' || !install.path) fail(this.remember(this.unready(install, env, at)).entry.note ?? 'Claude Code is not ready.');
    const auth = await this.deps.auth(install.path, env);
    if (auth && !auth.signedIn) fail(this.remember(this.described(install, env, at, catalogFrom(auth, new Error(), 0))).entry.note!);
    // An older list still says how each model thinks; the run's own `initialize` checks the model.
    return { install: install as ClaudeLaunch['install'], env, sdk, models: this.latest?.models };
  }
  /** Records a run's `initialize` answer as a fresh check, so the list and later runs need not ask again. */
  observe(launch: ClaudeLaunch, init: SDKControlInitializeResponse) {
    this.remember(this.described(launch.install, launch.env, this.deps.now(), catalogFrom({ signedIn: true, provider: null }, init, 0)));
  }
  private async sdk(): Promise<ClaudeSdk> {
    return loadSdk(this.deps.sdk).catch(() => fail('The Agent SDK on this Mac is damaged. Set up Claude Code threads again.'));
  }
  forget() { this.check = undefined; this.latest = undefined; }
  private remember(check: Check) {
    this.latest = check; this.check = Promise.resolve(check);
    return check;
  }
  private checked(fresh: boolean): Promise<Check> {
    const now = this.deps.now();
    if (!fresh && this.check) return this.check.then(check => now - check.at < CHECK_TTL_MS ? check : this.recheck());
    return this.recheck();
  }
  private recheck(): Promise<Check> {
    const check = this.run().then(result => { if (this.check === check) this.latest = result; return result; });
    this.check = check;
    check.catch(() => { if (this.check === check) this.check = undefined; });
    return check;
  }
  async probe() {
    const configured = this.path();
    const env = await this.deps.environment();
    return { env, install: await this.deps.find(configured === undefined ? { env, directory: join(this.deps.cwd, 'tools') } : { path: configured, env }) };
  }
  private base(install: ClaudeInstall | undefined) {
    return {
      id: 'claude' as const, name: 'Claude Code', version: install?.version ?? null,
      supported: { min: CLAUDE_MIN_VERSION, tested: CLAUDE_TESTED_VERSION }, path: install?.path ?? null,
      capabilities: { steer: true, approvals: true, thinking: true, usage: true }, protection: PROTECTION,
      modes: CLAUDE_MODES, default_mode: this.defaultMode(),
    };
  }
  private unready(install: ClaudeInstall, env: NodeJS.ProcessEnv | undefined, at: number): Check {
    const state: HarnessState = install.state === 'error' || install.state === 'ready' ? 'unavailable' : install.state;
    return { install, env, models: [], at, entry: { ...this.base(install), state, note: install.message, models: [] } };
  }
  private described(install: ClaudeInstall, env: NodeJS.ProcessEnv, at: number, catalog: ClaudeCatalog): Check {
    const base = this.base(install);
    if (catalog.state !== 'ready') return { install, env, models: [], at, entry: { ...base, state: catalog.state, note: catalog.note, models: [] } };
    const note = install.untested ? install.message : null;
    return { install, env, models: catalog.models, at, entry: { ...base, state: 'ready', note, models: catalog.models.map(claudeWireModel) } };
  }
  /** The SDK is looked for first, which needs no process; then Claude Code is found and asked. */
  private async run(): Promise<Check> {
    const at = this.deps.now();
    if (!await sdkPresent(this.deps.sdk)) {
      return { install: { state: 'not_installed', message: '' }, models: [], at, entry: { ...this.base(undefined), state: 'setup_required', note: this.setupError ?? SETUP_NOTE, models: [] } };
    }
    const sdk = await loadSdk(this.deps.sdk).catch(() => undefined);
    if (!sdk) {
      return { install: { state: 'not_installed', message: '' }, models: [], at, entry: { ...this.base(undefined), state: 'setup_required', note: 'The Agent SDK on this Mac is damaged. Set up Claude Code threads again.', models: [] } };
    }
    const { install, env } = await this.probe();
    if (install.state !== 'ready' || !install.path) return this.unready(install, env, at);
    const catalog = await this.deps.catalog({ query: sdk.query, path: install.path, env, cwd: this.deps.cwd });
    return this.described(install, env, at, catalog);
  }
}

function fail(message: string): never { return failWith('model_unavailable', message); }
