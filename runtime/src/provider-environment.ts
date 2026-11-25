import { defaultProviderAuthContext } from '@earendil-works/pi-ai';
import type { AuthContext } from '@earendil-works/pi-ai';
import { hostAccount, loginEnvironment } from './command/login-environment.ts';

/** Reads the account's login-shell environment; undefined when it could not be read. */
export type EnvironmentReader = () => Promise<Record<string, string> | undefined>;

// Host plumbing never comes from a shell, whatever a template names.
const HOST_PRIVATE = /^PLAIT_/;
// Pi reads these through its auth context, but the cloud SDKs that make the requests read them
// from the process environment themselves. Answering from the shell would list a provider as
// usable whose requests cannot authenticate, so they come only from the host's own environment.
const AMBIENT = new Set([
  'GOOGLE_APPLICATION_CREDENTIALS', 'GOOGLE_CLOUD_PROJECT', 'GCLOUD_PROJECT', 'GOOGLE_CLOUD_LOCATION',
  'AZURE_OPENAI_BASE_URL', 'AZURE_OPENAI_RESOURCE_NAME', 'AZURE_OPENAI_API_VERSION', 'AZURE_OPENAI_DEPLOYMENT_NAME_MAP',
]);
const READ_TIMEOUT_MS = 12_000;
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const NAME_PREFIX = /^[A-Za-z_][A-Za-z0-9_]*/;

/**
 * The auth context Pi resolves provider keys with: the host process's own variables first,
 * then the login shell's. Only Pi's key and header resolution reads it, so a shell value never
 * enters `process.env` or reaches a child process.
 */
export function providerAuthContext(login: Record<string, string> | undefined): AuthContext {
  const base = defaultProviderAuthContext();
  return {
    async env(name) {
      // A variable the host sets, even to an empty value, is the host's answer; Pi treats blank as unset.
      if (process.env[name] !== undefined) return base.env(name);
      if (!login || HOST_PRIVATE.test(name) || name.startsWith('AWS_') || AMBIENT.has(name)) return undefined;
      const value = Object.hasOwn(login, name) ? login[name] : undefined;
      return typeof value === 'string' && value.trim() ? value : undefined;
    },
    fileExists: path => base.fileExists(path),
  };
}

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

/** Whether Pi would run a command or read a variable to resolve this key or header value. */
export function resolvesExternally(value: unknown): boolean {
  return typeof value === 'string' && (value.startsWith('!') || templateNames(value).length > 0);
}

/** The account's login shell, read the way trusted commands read it. A fallback is not a login environment. */
export const readLoginEnvironment: EnvironmentReader = async () => {
  try {
    const login = await loginEnvironment(hostAccount(), AbortSignal.timeout(READ_TIMEOUT_MS));
    return login.source === 'login-shell' ? login.env : undefined;
  } catch { return undefined; }
};
