import { readFile } from 'node:fs/promises';
import { stripTypeScriptTypes } from 'node:module';

// Verbatim runtime/src fixtures from 33911fc8f1b1ceaa83c04f70c20f58e7965f25a5.
// schema-five.ts is runtime/src/automations/schema.ts from 45b8b8e.
// Keep the rollback target fixed when main advances or CI uses a shallow checkout.
async function previousSource(path: string): Promise<string> {
  const source = await readFile(new URL('../fixtures/rollback/' + path + '.txt', import.meta.url), 'utf8');
  const base = new URL('../../src/' + path, import.meta.url);
  const schema = path === 'automations/service.ts' ? await previousSource('automations/schema.ts') : undefined;
  const code = stripTypeScriptTypes(source).replace(/from '([.][^']+)'/g,
    (_, relative: string) => `from '${relative === './schema.ts' && schema ? schema : new URL(relative, base).href}'`);
  return 'data:text/javascript;base64,' + Buffer.from(code).toString('base64');
}

export async function previousRuntime(path: string) { return import(await previousSource(path)); }
