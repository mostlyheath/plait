import { hostAccount, loginEnvironment } from './command/login-environment.ts';

/** Reads the account's login-shell environment; undefined when it could not be read. */
export type EnvironmentReader = () => Promise<Record<string, string> | undefined>;

// Pi 1.5.2 reads these for ambient cloud credentials and endpoints (Bedrock, Azure OpenAI, Vertex).
const CLOUD = [
  'AWS_PROFILE', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY', 'AWS_SESSION_TOKEN', 'AWS_REGION', 'AWS_DEFAULT_REGION',
  'AWS_BEARER_TOKEN_BEDROCK', 'AWS_CONTAINER_CREDENTIALS_RELATIVE_URI', 'AWS_CONTAINER_CREDENTIALS_FULL_URI',
  'AWS_WEB_IDENTITY_TOKEN_FILE', 'AZURE_OPENAI_BASE_URL', 'AZURE_OPENAI_RESOURCE_NAME', 'AZURE_OPENAI_API_VERSION',
  'AZURE_OPENAI_DEPLOYMENT_NAME_MAP', 'GOOGLE_APPLICATION_CREDENTIALS', 'GOOGLE_CLOUD_PROJECT', 'GCLOUD_PROJECT',
  'GOOGLE_CLOUD_LOCATION',
];
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const NAME_PREFIX = /^[A-Za-z_][A-Za-z0-9_]*/;
// Host plumbing never comes from a shell, whatever a template names.
const HOST_PRIVATE = /^PLAIT_/;
const READ_TIMEOUT_MS = 12_000;

/**
 * The variables a Pi key or header template reads, in Pi's own grammar: `$NAME` and
 * `${NAME}`; `$$` and `$!` are escapes, and a leading `!` is a command, which reads none.
 */
export function templateNames(value: string): string[] {
  if (value.startsWith('!')) return [];
  const names = new Set<string>();
  for (let index = value.indexOf('$'); index >= 0; index = value.indexOf('$', index)) {
    const next = value[index + 1];
    if (next === '$' || next === '!') { index += 2; continue; }
    if (next === '{') {
      const end = value.indexOf('}', index + 2);
      if (end < 0) { index += 1; continue; }
      const name = value.slice(index + 2, end);
      if (NAME.test(name)) names.add(name);
      index = end + 1;
      continue;
    }
    const match = value.slice(index + 1).match(NAME_PREFIX);
    if (match) names.add(match[0]);
    index += 1 + (match?.[0].length ?? 0);
  }
  return [...names];
}

const strings = (value: unknown): string[] => {
  if (typeof value === 'string') return [value];
  if (value && typeof value === 'object' && !Array.isArray(value)) return Object.values(value).filter(item => typeof item === 'string');
  return [];
};

/** Variables that models.json keys and headers name. A file Pi would reject names none. */
export function referencedNames(modelsJson: string | undefined): string[] {
  let file: unknown;
  try { file = modelsJson === undefined ? undefined : JSON.parse(modelsJson); } catch { return []; }
  const providers = (file as { providers?: unknown } | undefined)?.providers;
  if (!providers || typeof providers !== 'object') return [];
  const values: string[] = [];
  for (const provider of Object.values(providers) as Record<string, unknown>[]) {
    if (!provider || typeof provider !== 'object') continue;
    values.push(...strings(provider.apiKey), ...strings(provider.headers));
    if (Array.isArray(provider.models)) for (const model of provider.models) values.push(...strings((model as Record<string, unknown> | null)?.headers));
  }
  return [...new Set(values.flatMap(templateNames))];
}

/** Every variable provider authentication may read from the login environment. */
export async function providerNames(login: Record<string, string>, builtin: Iterable<string>, modelsJson: string | undefined): Promise<Set<string>> {
  // Pi's compatibility entry point is the one that exports its key-name table; load it only when needed.
  const { findEnvKeys } = await import('@earendil-works/pi-ai/compat');
  const names = new Set([...CLOUD, ...referencedNames(modelsJson)]);
  for (const id of builtin) for (const name of findEnvKeys(id, login) ?? []) names.add(name);
  for (const name of names) if (HOST_PRIVATE.test(name)) names.delete(name);
  return names;
}

/**
 * Pi resolves provider keys from `process.env` and offers no resolver through ModelRuntime,
 * so the provider variables found in the login shell are copied there. Only those names
 * are copied, a value the host process started with is never replaced, and a copied value
 * follows the login shell on the next read.
 */
export class ProviderEnvironment {
  private readonly imported = new Map<string, string>();
  private readonly target: NodeJS.ProcessEnv;

  constructor(target: NodeJS.ProcessEnv = process.env) { this.target = target; }

  /** Returns whether any variable changed. */
  apply(login: Record<string, string>, names: Set<string>): boolean {
    let changed = false;
    for (const [name, value] of this.imported) {
      if (names.has(name) && login[name]) continue;
      if (this.target[name] === value) delete this.target[name];
      this.imported.delete(name);
      changed = true;
    }
    for (const name of names) {
      const value = login[name];
      if (!value || HOST_PRIVATE.test(name)) continue;
      const own = this.target[name] !== undefined && !this.imported.has(name);
      if (own || this.imported.get(name) === value) continue;
      this.target[name] = value;
      this.imported.set(name, value);
      changed = true;
    }
    return changed;
  }
}

/** The account's login shell, read the way trusted commands read it. A fallback is not a login environment. */
export const readLoginEnvironment: EnvironmentReader = async () => {
  try {
    const login = await loginEnvironment(hostAccount(), AbortSignal.timeout(READ_TIMEOUT_MS));
    return login.source === 'login-shell' ? login.env : undefined;
  } catch { return undefined; }
};
