import { randomBytes } from 'node:crypto';
import { mkdir, open, readFile, realpath, rename, stat, unlink } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fail } from './protocol.ts';
import { resolvesExternally } from './provider-environment.ts';

// Pi's models.json: {"providers": {ID: {name, baseUrl, api, apiKey, models: [{id, ...}], ...}}}.
// Plait edits one provider entry and writes every other key back as it was read.
type Json = Record<string, unknown>;
export type EndpointFile = {
  path: string;
  /** Exact text that was read; undefined when the file does not exist yet. */
  text: string | undefined;
  data: Json & { providers: Json };
  indent: string | number;
  mode: number;
};
export type EndpointChange = { name?: string; baseUrl?: string; modelIds?: string[] };
export type EndpointCheck = {
  reachable: boolean;
  reason: 'timeout' | 'unreachable' | 'unauthorized' | 'not_found' | 'http_error' | 'invalid_response' | null;
  status: number | null; model_ids: string[]; truncated: boolean;
};

/** New endpoints speak the OpenAI chat-completions API. Existing entries keep their own `api`. */
const API = 'openai-completions';
/** Pi treats a provider with no key as not configured; its documented fix for keyless servers is a dummy key. */
const KEYLESS = 'none';
const CHECK_BODY_LIMIT = 1048576;
const CHECK_MODEL_LIMIT = 500;

const object = (value: unknown): value is Json => typeof value === 'object' && value !== null && !Array.isArray(value);
const missing = (error: unknown) => (error as { code?: string }).code === 'ENOENT';

async function contents(path: string): Promise<string | undefined> {
  try { return await readFile(path, 'utf8'); } catch (error) { if (missing(error)) return undefined; throw error; }
}

export async function readEndpoints(path: string): Promise<EndpointFile> {
  // Write beside the real file so a symlinked models.json stays a symlink.
  const target = await realpath(path).catch(error => { if (missing(error)) return path; throw error; });
  const text = await contents(target);
  if (text === undefined) return { path: target, text, data: { providers: {} }, indent: 2, mode: 0o600 };
  let data: unknown;
  // Pi also accepts comments here. Rewriting would delete them, so such a file stays hand-edited.
  try { data = JSON.parse(text.replace(/^﻿/, '')); } catch {
    fail('conflict', 'models.json is not plain JSON (it may contain comments). Plait will not rewrite it; edit the file by hand.');
  }
  if (!object(data) || (data.providers !== undefined && !object(data.providers))) {
    fail('conflict', 'models.json has an unexpected layout. Plait will not rewrite it; edit the file by hand.');
  }
  data.providers ??= {};
  const indent = /^([ \t]+)"/m.exec(text)?.[1] ?? 2;
  return { path: target, text, data: data as EndpointFile['data'], indent, mode: (await stat(target)).mode & 0o777 };
}

export function renderEndpoints(file: EndpointFile): string {
  return JSON.stringify(file.data, null, file.indent) + '\n';
}

/** Replaces the file in one rename, or removes it when `text` is undefined. `expected` is what must still be on disk. */
export async function writeEndpoints(file: EndpointFile, text: string | undefined, expected = file.text): Promise<void> {
  const changed = () => fail('conflict', 'models.json changed while it was being edited. Nothing was written; try again.');
  if (text === undefined) {
    if (await contents(file.path) !== expected) changed();
    await unlink(file.path).catch(error => { if (!missing(error)) throw error; });
    return;
  }
  await mkdir(dirname(file.path), { recursive: true, mode: 0o700 });
  const temporary = join(dirname(file.path), `.models.json.${randomBytes(6).toString('hex')}.tmp`);
  const handle = await open(temporary, 'wx', 0o600);
  try {
    await handle.writeFile(text);
    // The creation mode is cut by the umask; an existing file keeps its own mode.
    await handle.chmod(file.mode);
    await handle.sync();
    // Pi or an editor may have saved since the read. Their change wins.
    if (await contents(file.path) !== expected) changed();
    await rename(temporary, file.path);
  } catch (error) {
    await unlink(temporary).catch(() => {});
    throw error;
  } finally { await handle.close(); }
}

export function addEndpoint(file: EndpointFile, id: string, change: Required<EndpointChange>): void {
  if (Object.hasOwn(file.data.providers, id)) fail('conflict', 'An endpoint with that ID already exists.');
  file.data.providers[id] = {
    name: change.name, baseUrl: change.baseUrl, api: API, apiKey: KEYLESS,
    models: change.modelIds.map(model => ({ id: model })),
  };
}

export function updateEndpoint(file: EndpointFile, id: string, change: EndpointChange): void {
  const entry = existing(file, id);
  // Credentials Pi resolves for an entry were set up for one server. Moving the entry over the
  // socket would send them to another, so only an entry with nothing but literal values moves here.
  if (change.baseUrl !== undefined && origin(change.baseUrl) !== origin(entry.baseUrl) && !literalOnly(entry)) {
    fail('conflict', 'This endpoint\'s credentials come from your environment, a command, or a sign-in. Change its address in models.json.');
  }
  if (change.name !== undefined) entry.name = change.name;
  if (change.baseUrl !== undefined) entry.baseUrl = change.baseUrl;
  if (change.modelIds !== undefined) {
    const models = Array.isArray(entry.models) ? entry.models.filter(object) : [];
    // A model that stays keeps the fields written for it by hand, such as its context window.
    entry.models = change.modelIds.map(model => models.find(item => item.id === model) ?? { id: model });
  }
}

export function removeEndpoint(file: EndpointFile, id: string): void {
  existing(file, id);
  delete file.data.providers[id];
}

function origin(value: unknown): string | undefined {
  return typeof value === 'string' && URL.canParse(value) ? new URL(value).origin : undefined;
}

const values = (headers: unknown): unknown[] => object(headers) ? Object.values(headers) : [];

/**
 * Whether Pi resolves nothing for this entry beyond its literal text. Without a literal key, or
 * with `oauth` (such as `"radius"`), Pi falls back to a provider resolver that reads the
 * environment, so the entry is not literal even when no value is a template.
 */
function literalOnly(entry: Json): boolean {
  if (entry.oauth !== undefined || typeof entry.apiKey !== 'string' || resolvesExternally(entry.apiKey)) return false;
  const models = Array.isArray(entry.models) ? entry.models.filter(object) : [];
  const overrides = object(entry.modelOverrides) ? Object.values(entry.modelOverrides).filter(object) : [];
  return ![...values(entry.headers), ...[...models, ...overrides].flatMap(model => values(model.headers))].some(resolvesExternally);
}

function existing(file: EndpointFile, id: string): Json {
  const entry = Object.hasOwn(file.data.providers, id) ? file.data.providers[id] : undefined;
  if (!object(entry)) fail('not_found', 'Endpoint not found in models.json.');
  return entry;
}

function failed(reason: EndpointCheck['reason'], status: number | null = null): EndpointCheck {
  return { reachable: false, reason, status, model_ids: [], truncated: false };
}

async function boundedText(response: Response): Promise<string | undefined> {
  if (!response.body) return '';
  const chunks: Uint8Array[] = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.byteLength;
    if (size > CHECK_BODY_LIMIT) return undefined;
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString('utf8');
}

/**
 * Lists models from an OpenAI-compatible server: GET BASE_URL/models. The reply reports only a
 * reason, the HTTP status, and model IDs; the response body is never passed on.
 */
export async function checkEndpoint(baseUrl: string, headers: Record<string, string>, timeoutMs = 5000): Promise<EndpointCheck> {
  let response: Response;
  let body: string | undefined;
  try {
    // A redirect could carry the key to another server, so it is reported instead of followed.
    response = await fetch(baseUrl.replace(/\/+$/, '') + '/models', {
      headers: { accept: 'application/json', ...headers }, redirect: 'manual', signal: AbortSignal.timeout(timeoutMs),
    });
    if (response.status === 401 || response.status === 403) return failed('unauthorized', response.status);
    if (response.status === 404) return failed('not_found', response.status);
    if (!response.ok) return failed('http_error', response.status);
    body = await boundedText(response);
  } catch (error) {
    return failed((error as Error).name === 'TimeoutError' ? 'timeout' : 'unreachable');
  }
  let listed: unknown;
  try { listed = body === undefined ? undefined : (JSON.parse(body) as { data?: unknown }).data; } catch { listed = undefined; }
  if (!Array.isArray(listed)) return failed('invalid_response', response.status);
  const ids = [...new Set(listed.map(item => (item as { id?: unknown } | null)?.id)
    .filter((id): id is string => typeof id === 'string' && id.length > 0 && id.length <= 256 && !/[\u0000-\u001f\u007f]/.test(id)))];
  return {
    reachable: true, reason: null, status: response.status,
    model_ids: ids.slice(0, CHECK_MODEL_LIMIT), truncated: ids.length > CHECK_MODEL_LIMIT,
  };
}
