import { writeFile, access } from 'node:fs/promises';
import { fstatSync } from 'node:fs';
import { adoptDescriptors } from '../../src/safety.ts';

const { root, identity, ready, stop } = JSON.parse(process.env.PLAIT_TEST_LEASE!);
const lease = await adoptDescriptors([3, 4, 5], { root, identity });
const identities = [3, 4, 5].map(fd => {
  const stat = fstatSync(fd, { bigint: true });
  return { dev: String(stat.dev), ino: String(stat.ino) };
});
await writeFile(ready, JSON.stringify({ identities, lock: process.env.PLAIT_LOCK_FD, notify: process.env.PLAIT_NOTIFY_FD }));
for (;;) {
  try { await access(stop); break; } catch { await new Promise(resolve => setTimeout(resolve, 20)); }
}
await lease.close();
