import { join } from 'node:path';
import type { AuthContext } from '@earendil-works/pi-ai';
import { ModelRuntime, SettingsManager, getAgentDir } from '@earendil-works/pi-coding-agent';
import { builtinProviders } from '@earendil-works/pi-ai/providers/all';
import type { ProviderSource, Registry } from './providers.ts';
import { providerAuthContext, providerOrigin, readLoginEnvironment } from './provider-environment.ts';
import type { EnvironmentReader, ProviderBinding } from './provider-environment.ts';

/**
 * Pi 1.5.2's ModelRuntime builds its Models without an auth context, so Pi would resolve keys
 * from `process.env`, which every child process inherits. Its Models keeps the context as a
 * plain field read on each resolution; set it before the first availability check.
 */
function useAuthContext(runtime: ModelRuntime, context: AuthContext): void {
  const models = (runtime as unknown as { models?: { authContext?: AuthContext } }).models;
  if (!models || !Object.hasOwn(models, 'authContext')) throw new Error('Pi no longer exposes the provider auth context.');
  models.authContext = context;
}

/**
 * The registry for one detached run. The runner does not read the login shell itself: it
 * resolves keys with exactly the values the host's registry used for the run's provider, so
 * the host and the run agree on the credential by construction.
 */
export async function loadRunModels(source: ProviderSource, binding: ProviderBinding, signal: AbortSignal): Promise<Registry> {
  const loaded = await source.load(signal, binding.values);
  const provider = loaded.models.getProvider(binding.provider);
  if (!provider || providerOrigin(provider.baseUrl) !== binding.origin) {
    throw new Error('This provider\'s server changed after the run was accepted. Send the message again.');
  }
  return loaded;
}

/**
 * Pi's own files: auth.json for credentials and models.json for custom endpoints, both in
 * Pi's agent directory. Each load reads them again and never refreshes catalogs over the network.
 */
export function piProviders(agentDir = getAgentDir(), read: EnvironmentReader = readLoginEnvironment): ProviderSource {
  const authPath = join(agentDir, 'auth.json');
  const modelsPath = join(agentDir, 'models.json');
  return {
    builtin: new Set(builtinProviders().map(provider => provider.id)),
    authPath, modelsPath,
    readEnvironment: read,
    // Pi's ChatGPT sign-in sends the installation's device ID, which Pi keeps in its settings.
    loginOptions: { getDeviceId: () => SettingsManager.create(process.cwd(), agentDir, { projectTrusted: false }).getOrCreateDeviceId() },
    async load(signal?: AbortSignal, login?: Record<string, string>): Promise<Registry> {
      // Create without the initial check, so availability is first computed with this context.
      const models = await ModelRuntime.create({ authPath, modelsPath, allowModelNetwork: false, signal, refreshOnCreate: false });
      useAuthContext(models, providerAuthContext(login));
      const refreshed = await models.refresh({ allowNetwork: false, signal });
      // An interrupted load has an incomplete authenticated set; keep the previous registry instead.
      if (signal?.aborted) throw new Error('Provider load timed out');
      // Pi records a models.json parse error or a failed provider instead of rejecting, and loads
      // what it can, so the caller decides whether such a registry may replace a working one.
      const problem = models.getError() !== undefined || refreshed.errors.size > 0;
      return {
        models, problem,
        authenticated: new Set(models.getAvailableSnapshot().map(model => model.provider)),
        stored: async () => new Map((await models.listCredentials()).map(entry => [entry.providerId, entry.type])),
        settle: () => models.settleCredentialWrites(),
      };
    },
  };
}
