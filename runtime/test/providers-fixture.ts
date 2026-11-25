import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fauxProvider, fauxAssistantMessage, fauxText, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { Runtime } from '../src/runtime.ts';
import { Providers } from '../src/providers.ts';
import { serve } from '../src/stdio.ts';
import { scriptedProvider, scriptedSource } from './support/providers.ts';

// Sidecar for provider tests behind the real host binary. Providers and sign-in flows are
// scripted; marker files in the disposable store choose what the next reload reports.
const store = process.argv[2];
const faux = fauxProvider({ tokensPerSecond: 120, tokenSize: { min: 1, max: 1 } });
faux.setResponses([
  fauxAssistantMessage([fauxToolCall('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, { id: 'edit-1' })], { stopReason: 'toolUse' }),
  ...Array.from({ length: 4 }, () => fauxAssistantMessage([fauxText('Scripted reply after the provider reload.')])),
]);
const fake = scriptedSource(() => [
  ...(existsSync(join(store, 'drop-faux')) ? [] : [faux.provider]),
  ...(existsSync(join(store, 'late-provider')) ? [scriptedProvider('late', ['late-1'])] : []),
]);
// Like Pi's browser flows, the scripted flow holds a loopback listener until it ends.
fake.flows.subscription.listen = async interaction => {
  const listener = createServer((_request, response) => response.end('scripted callback'));
  await new Promise<void>(resolve => listener.listen(0, '127.0.0.1', resolve));
  writeFileSync(join(store, 'sign-in-listening'), String((listener.address() as AddressInfo).port));
  interaction.signal?.addEventListener('abort', () => {
    listener.close();
    writeFileSync(join(store, 'sign-in-released'), 'closed by cancellation');
  }, { once: true });
};
const registry = await fake.source.load();
const runtime = new Runtime(store, registry.models, registry.authenticated);
runtime.providers = new Providers(runtime, fake.source, registry);
try { await runtime.recover(); await serve(runtime); }
finally { await runtime.close(); writeFileSync(join(store, 'sidecar-closed'), String(process.pid)); }
