import { cleanupSessionResources } from '@earendil-works/pi-ai';
import { Runtime } from './runtime.ts';
import { Providers } from './providers.ts';
import { piProviders } from './pi-providers.ts';
import { serve } from './stdio.ts';
import { hostClient } from './host-client.ts';

const background = process.env.PLAIT_GRAPH === '1' || process.env.PLAIT_AUTOMATIONS === '1';
const socket = background && process.env.PLAIT_BACKGROUND_SOCKET;
// Only an opted-in runtime consumes the socket path; otherwise the environment is left as main left it.
if (background) delete process.env.PLAIT_BACKGROUND_SOCKET;
const source = piProviders();
const registry = await source.load();
const runtime = new Runtime(process.argv[2], registry.models, registry.authenticated);
runtime.providers = new Providers(runtime, source, registry);
runtime.providers.published = () => runtime.providersPublished();
// Startup does not wait for the login shell; providers it authenticates appear after a reload.
void runtime.providers.adoptEnvironment();
// Closing the runtime also cancels pending sign-ins and waits for credential writes.
try {
  await runtime.recover();
  await serve(runtime, socket && process.env.PLAIT_NOTIFY_FD ? () => runtime.startBackground(hostClient(socket)) : undefined);
}
finally { await runtime.close(); cleanupSessionResources(); }
