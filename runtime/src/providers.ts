import { chmod, stat } from 'node:fs/promises';
import type { AuthPrompt, LoginOptions, Models, Provider } from '@earendil-works/pi-ai';
import { HostError, PROTOCOL_VERSIONS, fail } from './protocol.ts';
import { SignIns } from './sign-in.ts';
import type { SignInLimits } from './sign-in.ts';
import { addEndpoint, checkEndpoint, readEndpoints, removeEndpoint, renderEndpoints, updateEndpoint, writeEndpoints } from './endpoints.ts';
import type { EndpointFile } from './endpoints.ts';

/** One complete, immutable view of Pi's providers. A reload builds a new one and swaps it in. */
export type Registry = {
  models: Models;
  /** Providers with usable authentication, as the model catalog reports them. */
  authenticated: Set<string>;
  /** Which providers have a credential in Pi's store, and of what type. Never the credential. */
  stored(): Promise<Map<string, 'api_key' | 'oauth'>>;
  /** Resolves when credential writes already started, such as a token refresh, are on disk. */
  settle(): Promise<void>;
};
export type ProviderSource = {
  /** Reads Pi's files again. A reload passes a deadline; host startup does not. */
  load(signal?: AbortSignal): Promise<Registry>;
  /** Provider IDs that ship with Pi. Any other provider is a custom endpoint from models.json. */
  builtin: ReadonlySet<string>;
  /** Pi's models.json. Absent when this host cannot edit custom endpoints. */
  modelsPath?: string;
  /** Pi's credential file, kept owner-only after every write. */
  authPath?: string;
  loginOptions?: LoginOptions;
};
/** What a reload replaces. `Runtime` has this shape. */
export type RegistryHolder = { models: Models; authenticated: Set<string> };

export type ProviderView = {
  id: string; name: string; kind: 'builtin' | 'custom'; authenticated: boolean;
  credential: 'none' | 'sign_in' | 'api_key' | 'external';
  model_count: number;
  sign_in: { name: string; subscription: boolean } | null;
  api_key: { name: string } | null;
  endpoint: { base_url: string | null; api: string | null; model_ids: string[] } | null;
};

const operations: Record<string, { required: string[]; optional: string[] }> = {
  reload_providers: { required: [], optional: [] },
  list_providers: { required: [], optional: [] },
  start_sign_in: { required: ['provider'], optional: ['method'] },
  sign_in_status: { required: ['sign_in_id'], optional: [] },
  answer_sign_in: { required: ['sign_in_id', 'input'], optional: [] },
  cancel_sign_in: { required: ['sign_in_id'], optional: [] },
  set_provider_key: { required: ['provider', 'key'], optional: [] },
  sign_out_provider: { required: ['provider'], optional: [] },
  add_endpoint: { required: ['id', 'name', 'base_url', 'model_ids'], optional: ['key'] },
  update_endpoint: { required: ['id'], optional: ['name', 'base_url', 'model_ids', 'key'] },
  remove_endpoint: { required: ['id'], optional: [] },
  check_endpoint: { required: [], optional: ['id', 'base_url', 'key'] },
};
type Request = Record<string, unknown> & { op: string };

/** Every Pi call made for a request ends well inside the host's 20 second sidecar deadline. */
const PI_DEADLINE_MS = 5000;
const SECRET_LIMIT = 8192;
const CONTROL = /[\u0000-\u001f\u007f]/;

// Messages name the field and never repeat its value: a field can hold a key or a pasted code.
// Values are trimmed first because pasted text often ends with a newline.
function text(request: Request, key: string, limit: number, empty = false): string {
  const value = typeof request[key] === 'string' ? (request[key] as string).trim() : undefined;
  if (value === undefined || (!empty && !value)) fail('invalid_request', `${key} must be a ${empty ? '' : 'nonempty '}string.`);
  if (Buffer.byteLength(value) > limit || CONTROL.test(value)) fail('invalid_request', `${key} is too long or contains control characters.`);
  return value;
}
const optional = (request: Request, key: string, limit: number) => request[key] === undefined ? undefined : text(request, key, limit);
const identifier = (request: Request, key: string) => {
  const value = text(request, key, 128);
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(value)) fail('invalid_request', `Invalid ${key}.`);
  return value;
};

function endpointUrl(value: string): string {
  const url = URL.canParse(value) ? new URL(value) : undefined;
  if (!url || (url.protocol !== 'http:' && url.protocol !== 'https:')) fail('invalid_request', 'base_url must be an http or https URL.');
  // A key belongs in the credential store, not in a URL that is listed and saved in models.json.
  if (url.username || url.password || /[?#]/.test(value)) fail('invalid_request', 'base_url must not contain a user name, password, query, or fragment.');
  return value;
}

function modelIds(request: Request): string[] {
  const value = request.model_ids;
  const valid = Array.isArray(value) && value.length >= 1 && value.length <= 200
    && value.every(id => typeof id === 'string' && id.trim() === id && id.length >= 1 && id.length <= 256 && !CONTROL.test(id))
    && new Set(value).size === value.length;
  if (!valid) fail('invalid_request', 'model_ids must be 1 to 200 distinct model IDs of at most 256 characters.');
  return value as string[];
}

/** The base URL as shown to a client: no user name, password, query, or fragment. */
function displayUrl(value: string | undefined): string | null {
  if (!value || !URL.canParse(value)) return null;
  const url = new URL(value);
  return url.origin + url.pathname;
}

/**
 * Pi resolves a stored key as a template: a leading `!` runs a shell command and `$NAME` reads
 * the environment. A key from the socket is always a literal, so both are escaped.
 */
export function literalKey(key: string): string {
  const escaped = key.replace(/\$/g, () => '$$');
  return escaped.startsWith('!') ? `$${escaped}` : escaped;
}

class Unsupported extends Error {}

function validate(input: unknown): Request {
  const request = input as Request;
  const { required, optional: extra } = operations[request.op];
  const allowed = [...required, ...extra, 'protocol_version'];
  if (Object.keys(request).some(key => key !== 'op' && !allowed.includes(key)) || required.some(key => request[key] === undefined)) {
    fail('invalid_request', 'Unknown operation or field.');
  }
  if (request.protocol_version !== undefined && !(PROTOCOL_VERSIONS as readonly unknown[]).includes(request.protocol_version)) {
    fail('invalid_request', 'Unsupported protocol_version.');
  }
  if (Buffer.byteLength(JSON.stringify(request)) > 65000) fail('invalid_request', 'Request is too large.');
  return request;
}

/** Provider sign-in for the host: reload, list, subscription sign-in, API keys, and custom endpoints. */
export class Providers {
  private registry: Registry;
  private readonly holder: RegistryHolder;
  private readonly source: ProviderSource;
  readonly signIns: SignIns;
  private reloading: Promise<void> = Promise.resolve();
  // Runs that started before a reload keep their registry; its credential writes still settle at shutdown.
  private readonly retired: Registry[] = [];

  constructor(holder: RegistryHolder, source: ProviderSource, registry: Registry, limits: Partial<SignInLimits> = {}) {
    this.holder = holder; this.source = source; this.registry = registry;
    this.signIns = new SignIns(limits);
  }

  handles(input: unknown): boolean {
    return typeof input === 'object' && input !== null && !Array.isArray(input)
      && typeof (input as Request).op === 'string' && Object.hasOwn(operations, (input as Request).op);
  }

  async handle(input: unknown): Promise<unknown> {
    const request = validate(input);
    switch (request.op) {
      case 'reload_providers': await this.reload(); return { providers: await this.list() };
      case 'list_providers': return { providers: await this.list() };
      case 'start_sign_in': return { sign_in: await this.startSignIn(text(request, 'provider', 128), optional(request, 'method', 64)) };
      case 'sign_in_status': return { sign_in: this.signIns.status(identifier(request, 'sign_in_id')) };
      case 'answer_sign_in': return { sign_in: await this.signIns.answer(identifier(request, 'sign_in_id'), text(request, 'input', SECRET_LIMIT, true)) };
      case 'cancel_sign_in': return { sign_in: this.signIns.cancel(identifier(request, 'sign_in_id')) };
      case 'set_provider_key': return { provider: await this.setKey(text(request, 'provider', 128), text(request, 'key', SECRET_LIMIT)) };
      case 'sign_out_provider': return { provider: await this.signOut(text(request, 'provider', 128)) };
      case 'add_endpoint': return { provider: await this.addEndpoint(request) };
      case 'update_endpoint': return { provider: await this.updateEndpoint(request) };
      case 'remove_endpoint': return { removed: await this.removeEndpoint(text(request, 'id', 128)) };
      default: return this.checkEndpoint(request);
    }
  }

  /**
   * Builds a complete registry, then replaces the models and the authenticated set in one
   * synchronous step. Reloads run one after another so the last one requested is the one kept.
   */
  reload(): Promise<void> {
    const next = this.reloading.then(() => this.swap());
    this.reloading = next.catch(() => {});
    return next;
  }

  private async swap(): Promise<void> {
    const registry = await this.source.load(AbortSignal.timeout(PI_DEADLINE_MS)).catch(() => fail('internal', 'Providers could not be reloaded. The previous list is still in use.'));
    this.retired.push(this.registry);
    if (this.retired.length > 8) this.retired.shift();
    this.registry = registry;
    this.holder.models = registry.models; this.holder.authenticated = registry.authenticated;
  }

  /** The provider list, for `list_providers` and for any reply that later wants to include it. */
  async list(): Promise<ProviderView[]> {
    const stored = await this.registry.stored();
    return this.registry.models.getProviders().map(provider => this.view(provider, stored));
  }

  async close(): Promise<void> {
    await this.signIns.close();
    await this.reloading;
    await Promise.allSettled([this.registry, ...this.retired].map(registry => registry.settle()));
  }

  private view(provider: Provider, stored: Map<string, 'api_key' | 'oauth'>): ProviderView {
    const authenticated = this.registry.authenticated.has(provider.id);
    const kept = stored.get(provider.id);
    const models = provider.getModels();
    const custom = !this.source.builtin.has(provider.id);
    return {
      id: provider.id, name: provider.name.slice(0, 256), kind: custom ? 'custom' : 'builtin', authenticated,
      // `external` is an environment variable, a key in models.json, or ambient cloud credentials.
      credential: kept === 'oauth' ? 'sign_in' : kept === 'api_key' ? 'api_key' : authenticated ? 'external' : 'none',
      model_count: models.length,
      sign_in: provider.auth.oauth ? { name: provider.auth.oauth.name, subscription: provider.auth.oauth.isSubscription === true } : null,
      api_key: provider.auth.apiKey?.login ? { name: provider.auth.apiKey.name } : null,
      endpoint: custom ? { base_url: displayUrl(provider.baseUrl), api: models[0]?.api ?? null, model_ids: models.map(model => model.id) } : null,
    };
  }

  private provider(id: string): Provider {
    const provider = this.registry.models.getProvider(id);
    if (!provider) fail('not_found', 'Unknown provider.');
    return provider;
  }

  private async one(id: string): Promise<ProviderView> {
    return this.view(this.provider(id), await this.registry.stored());
  }

  /** After a credential write: keep Pi's credential file owner-only, then show the new state. */
  private async credentialChanged(): Promise<void> {
    const path = this.source.authPath;
    if (path) {
      const mode = await stat(path).then(info => info.mode, () => undefined);
      if (mode !== undefined && (mode & 0o077) !== 0) await chmod(path, 0o600);
    }
    await this.reload();
  }

  private async startSignIn(id: string, method: string | undefined) {
    const provider = this.provider(id);
    if (!provider.auth.oauth) fail('invalid_request', 'This provider has no sign-in. Set an API key instead.');
    const models = this.registry.models;
    return this.signIns.start(id, method,
      interaction => models.login(id, 'oauth', interaction, this.source.loginOptions),
      () => this.credentialChanged());
  }

  private async setKey(id: string, key: string): Promise<ProviderView> {
    if (!this.provider(id).auth.apiKey?.login) fail('invalid_request', 'This provider does not take an API key.');
    // Pi queues credential changes per provider; an unfinished sign-in would hold the queue.
    await this.signIns.cancelProvider(id);
    const stored = literalKey(key);
    const answer = (prompt: AuthPrompt) => prompt.type === 'secret' ? Promise.resolve(stored) : Promise.reject(new Unsupported());
    try {
      await this.registry.models.login(id, 'api_key', { signal: AbortSignal.timeout(PI_DEADLINE_MS), prompt: answer, notify() {} });
    } catch (error) {
      if (error instanceof Unsupported) fail('invalid_request', 'This provider needs more than one API key value. Configure it in Pi.');
      // Pi reports a saved key whose in-place refresh failed; the reload below replaces that refresh.
      if ((error as Error | undefined)?.name !== 'CredentialSynchronizationError') fail('internal', 'The API key could not be stored.');
    }
    await this.credentialChanged();
    return this.one(id);
  }

  private async signOut(id: string): Promise<ProviderView> {
    this.provider(id);
    await this.forget(id);
    await this.credentialChanged();
    return this.one(id);
  }

  private async forget(id: string): Promise<void> {
    await this.signIns.cancelProvider(id);
    try { await this.registry.models.logout(id, { signal: AbortSignal.timeout(PI_DEADLINE_MS) }); } catch (error) {
      if ((error as Error | undefined)?.name !== 'CredentialSynchronizationError') fail('internal', 'The stored credential could not be removed.');
    }
  }

  private modelsPath(): string {
    if (!this.source.modelsPath) fail('invalid_request', 'This host cannot edit custom endpoints.');
    return this.source.modelsPath;
  }

  /**
   * Writes models.json, reloads, and checks that Pi now reports what was asked for. Pi drops
   * every custom provider when the file fails its schema, so a rejected write is undone.
   */
  private async commit(file: EndpointFile, accepted: () => boolean): Promise<void> {
    const next = renderEndpoints(file);
    await writeEndpoints(file, next);
    try {
      await this.reload();
      if (!accepted()) fail('conflict', 'Pi did not accept the endpoint, so models.json was put back. Check the file for other errors.');
    } catch (error) {
      await writeEndpoints(file, file.text, next).then(() => this.reload()).catch(() => {});
      throw error instanceof HostError ? error : new HostError('internal', 'The endpoint could not be loaded, so models.json was put back.');
    }
  }

  private lists(id: string, models: string[] | undefined): boolean {
    const provider = this.registry.models.getProvider(id);
    const listed = new Set(provider?.getModels().map(model => model.id));
    return provider !== undefined && (models ?? []).every(model => listed.has(model));
  }

  private async addEndpoint(request: Request): Promise<ProviderView> {
    const id = text(request, 'id', 64);
    if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(id)) fail('invalid_request', 'id must be 1 to 64 lowercase letters, digits, dots, hyphens, or underscores.');
    const change = { name: text(request, 'name', 128), baseUrl: endpointUrl(text(request, 'base_url', 2048)), modelIds: modelIds(request) };
    const key = request.key === undefined ? undefined : text(request, 'key', SECRET_LIMIT);
    if (this.source.builtin.has(id) || this.registry.models.getProvider(id)) fail('conflict', 'A provider with that ID already exists.');
    const file = await readEndpoints(this.modelsPath());
    addEndpoint(file, id, change);
    await this.commit(file, () => this.lists(id, change.modelIds));
    return key === undefined ? this.one(id) : this.setKey(id, key);
  }

  private async updateEndpoint(request: Request): Promise<ProviderView> {
    const id = this.custom(text(request, 'id', 128));
    const name = optional(request, 'name', 128);
    const baseUrl = request.base_url === undefined ? undefined : endpointUrl(text(request, 'base_url', 2048));
    const models = request.model_ids === undefined ? undefined : modelIds(request);
    const key = request.key === undefined ? undefined : text(request, 'key', SECRET_LIMIT);
    if (name !== undefined || baseUrl !== undefined || models !== undefined) {
      const file = await readEndpoints(this.modelsPath());
      updateEndpoint(file, id, { name, baseUrl, modelIds: models });
      await this.commit(file, () => this.lists(id, models));
    }
    return key === undefined ? this.one(id) : this.setKey(id, key);
  }

  private async removeEndpoint(value: string): Promise<string> {
    const id = this.custom(value);
    const file = await readEndpoints(this.modelsPath());
    removeEndpoint(file, id);
    // The key goes first: a listed endpoint without its key is visible, an orphaned key is not.
    await this.forget(id);
    await this.commit(file, () => !this.registry.models.getProvider(id));
    return id;
  }

  private custom(id: string): string {
    if (this.source.builtin.has(id)) fail('invalid_request', 'Built-in providers are not custom endpoints.');
    return id;
  }

  private async checkEndpoint(request: Request) {
    if ((request.id === undefined) === (request.base_url === undefined) || (request.key !== undefined && request.id !== undefined)) {
      fail('invalid_request', 'Send either id, or base_url with an optional key.');
    }
    if (request.base_url !== undefined) {
      const key = request.key === undefined ? undefined : text(request, 'key', SECRET_LIMIT);
      return checkEndpoint(endpointUrl(text(request, 'base_url', 2048)), key === undefined ? {} : { authorization: `Bearer ${key}` });
    }
    const provider = this.provider(this.custom(text(request, 'id', 128)));
    // The saved endpoint is checked with its saved key; neither is sent back.
    const auth = await this.registry.models.getAuth(provider.id, { signal: AbortSignal.timeout(PI_DEADLINE_MS) }).catch(() => undefined);
    const baseUrl = auth?.auth.baseUrl ?? provider.baseUrl;
    if (!baseUrl || !URL.canParse(baseUrl)) fail('conflict', 'This endpoint has no usable base URL.');
    const headers: Record<string, string> = {};
    if (auth?.auth.apiKey) headers.authorization = `Bearer ${auth.auth.apiKey}`;
    for (const [name, value] of Object.entries(auth?.auth.headers ?? {})) if (typeof value === 'string') headers[name.toLowerCase()] = value;
    return checkEndpoint(baseUrl, headers);
  }
}
