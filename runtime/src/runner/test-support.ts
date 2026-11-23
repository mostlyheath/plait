import { isAbsolute, join } from 'node:path';
import { pathToFileURL } from 'node:url';

/**
 * Loads a test-only runner module. Installed runtimes ship without runtime/test, so a test run
 * against one names the checkout's support folder in PLAIT_RUNNER_TEST_SUPPORT. Callers gate this
 * behind PLAIT_RUNNER_TEST_TURNS=1; production never reaches it.
 */
export function importTestSupport<T>(file: string): Promise<T> {
  const directory = process.env.PLAIT_RUNNER_TEST_SUPPORT;
  const url = directory && isAbsolute(directory)
    ? pathToFileURL(join(directory, file)).href
    : new URL(`../../test/support/${file}`, import.meta.url).href;
  return import(url) as Promise<T>;
}
