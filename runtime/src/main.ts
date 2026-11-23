import { cleanupSessionResources } from '@earendil-works/pi-ai';
import { Runtime } from './runtime.ts';
import { Providers } from './providers.ts';
import { piProviders } from './pi-providers.ts';
import { serve } from './stdio.ts';

const source = piProviders();
const registry = await source.load();
const runtime = new Runtime(process.argv[2], registry.models, registry.authenticated);
runtime.providers = new Providers(runtime, source, registry);
// Closing the runtime also cancels pending sign-ins and waits for credential writes.
try { await runtime.recover(); await serve(runtime); }
finally { await runtime.close(); cleanupSessionResources(); }
