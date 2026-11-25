import { fileAction, ignoredPaths, pathKey, safePath, sameImage } from './safety.ts';
import type { FileIdentity, Postimage } from './safety.ts';

export async function restoreOwned(root: string, baseline: string[], records: Postimage[], identity: FileIdentity, lease?: number, lifecycleFd?: number, vcs?: 'git' | 'folder') {
  const owned = new Map<string, Postimage>();
  const ignored = await ignoredPaths(root, records.map(record => record.path), vcs);
  for (const record of records) {
    try {
      safePath(record.path, ignored);
      // An ignored parent, submodule, or other dirty ancestor is never exempt.
      safePath(record.path, baseline.filter(path => pathKey(path) !== pathKey(record.path)));
      const current = await fileAction(root, 'inspect', record.path, undefined, lease, identity, lifecycleFd);
      if (sameImage(record, current.postimage)) owned.set(pathKey(record.path), record);
    } catch {
      // Missing, changed, or unsafe files retain ordinary dirty-path protection.
    }
  }
  return owned;
}
