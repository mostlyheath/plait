// Each worker records its current test for the outer package smoke test's deadline.
import { rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { beforeEach, afterEach } from 'node:test';

const marker = join(process.env.PLAIT_ACTIVE_TESTS, String(process.pid));
beforeEach(t => writeFileSync(marker, `${process.argv[1]} — ${t.fullName}`));
afterEach(() => rmSync(marker, { force: true }));
