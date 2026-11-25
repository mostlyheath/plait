import { readFile, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export async function installedFixture(runtime, directory) {
  await symlink(join(runtime, 'node_modules'), join(directory, 'node_modules'));
  const source = await readFile(new URL('../runtime/test/fixture.ts', import.meta.url), 'utf8');
  const fixture = source.replaceAll("'../src/", "'" + pathToFileURL(join(runtime, 'src')).href + '/')
    .replace('sleep 30 & wait', 'sleep 300 & wait');
  const path = join(directory, 'fixture.ts');
  await writeFile(path, fixture);
  return path;
}
