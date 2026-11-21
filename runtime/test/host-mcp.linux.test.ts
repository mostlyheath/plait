import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createServer as unixServer } from 'node:net';
import { DatabaseSync } from 'node:sqlite';
import type { McpServer } from '../src/mcp/config.ts';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { setup, request, until, delay } from './support/host.ts';

async function config(f: Awaited<ReturnType<typeof setup>>, behavior = 'normal', enabled = true) {
  const agent = join(f.store, 'test-pi'); await mkdir(agent, { recursive: true });
  const marker = join(f.repo, 'mcp-events'); const server = join(f.repo, 'server.mjs');
  await writeFile(server, await readFile(resolve('test/support/mcp-fixture.mjs')));
  await writeFile(join(agent, 'mcp.json'), JSON.stringify({ mcpServers: { docs: {
    command: process.execPath, args: [server, behavior, marker], enabled, exposure: 'codemode',
    env: { FIXTURE_SECRET: 'HOST_MCP_SEEDED_SECRET', FIXTURE_EXPANDED: '!printf HOST_MCP_COMMAND_SECRET' },
  } } }));
  return marker;
}

test('real host searches lazily, records each call, prompts, and stores no seeded MCP secret', { timeout: 30000 }, async t => {
  const f = await setup(t, 'mcp-normal', false, false); const marker = await config(f); const thread = await f.create();
  await assert.rejects(readFile(marker), { code: 'ENOENT' }); await f.submit(thread.id);
  const pending = await until(() => f.watch(thread.id), value => !!value.pending_approval || value.status === 'idle' || value.status === 'error');
  assert.equal(pending.pending_approval?.summary, 'Use echo from docs', JSON.stringify(pending));
  assert.equal(pending.tools.find(tool => tool.id === 'call-mcp')?.name, 'mcp:docs/echo');
  await f.approve(thread.id, pending.pending_approval!.id);
  const done = await until(() => f.watch(thread.id), value => value.status === 'idle' || value.status === 'error');
  assert.equal(done.status, 'idle', JSON.stringify(done)); assert.equal(done.tools.find(tool => tool.id === 'call-mcp')?.status, 'done');
  const history = await request(f.store, { op: 'history', thread_id: thread.id, protocol_version: 3 });
  assert.match(JSON.stringify(history), /mcp:docs\/echo/);
  const resources = await request(f.store, { op: 'agent_resources', checkout: f.repo, protocol_version: 3 });
  assert.match(JSON.stringify(resources), /"state":"stopped"/); assert.match(JSON.stringify(resources), /"tool_count":8/);
  for (const data of [JSON.stringify([pending, done, history, resources]), await readFile(join(f.store, 'mcp-context.jsonl'), 'utf8'),
    (await readFile(join(f.store, 'host.sqlite'))).toString(), (await readFile(join(f.store, thread.id + '.sqlite'))).toString()]) {
    assert.doesNotMatch(data, /HOST_MCP_SEEDED_SECRET|HOST_MCP_COMMAND_SECRET|SERVER_INSTRUCTIONS_MUST_NOT_REACH_MODEL/);
  }
  const events = await readFile(marker, 'utf8'); assert.equal(events.match(/call echo/g)?.length, 1);
  const spawns = await readFile(join(f.store, 'mcp-spawns.jsonl'), 'utf8');
  assert.equal(spawns.match(/mcp-server/g)?.length, 1);
  const db = new DatabaseSync(join(f.store, 'host.sqlite'), { readOnly: true });
  try { assert.equal(db.prepare("SELECT count(*) AS n FROM settings WHERE key LIKE 'mcp_tool:%'").get()!.n, 1); }
  finally { db.close(); }
});

