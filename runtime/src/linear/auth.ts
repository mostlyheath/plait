import { randomBytes } from 'node:crypto';
import { constants } from 'node:fs';
import { chmod, lstat, mkdir, open, rename, unlink } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fail } from '../protocol.ts';
import { FIELD_LIMITS, LINEAR_KEY_MAX_BYTES, boundLinearText } from './types.ts';
import type { LinearAuthStatus, LinearIdentity } from './types.ts';

export type StoredLinearAuth = LinearIdentity & {
  api_key: string;
  saved_at: number;
  /** Non-secret per-save token so even same-millisecond key replacements have a distinct cache owner. */
  credential_id?: string;
};

const FILE_NAME = 'linear-auth.json';
// Linear API keys are printable ASCII with no whitespace or header-breaking control characters.
const KEY_PATTERN = /^[\x21-\x7e]+$/;
const missing = (error: unknown) => (error as { code?: string }).code === 'ENOENT';

/** Validates a pasted Linear API key without ever repeating its value in an error message. */
export function validateLinearKey(raw: unknown): string {
  const value = typeof raw === 'string' ? raw.trim() : undefined;
  if (!value) fail('invalid_request', 'key must be a nonempty string.');
  if (Buffer.byteLength(value) > LINEAR_KEY_MAX_BYTES || !KEY_PATTERN.test(value)) {
    fail('invalid_request', 'key must be printable ASCII without whitespace or control characters, at most 4096 bytes.');
  }
  return value;
}

/** Formats non-secret auth status for replies. Never returns the API key or file path. */
export function formatAuthStatus(auth: StoredLinearAuth | null, keyRejected = false): LinearAuthStatus {
  if (!auth) {
    return {
      signed_in: false, status: 'signed_out',
      workspace_id: null, workspace_name: null, workspace_url_key: null,
      viewer_id: null, viewer_name: null,
    };
  }
  return {
    signed_in: !keyRejected,
    status: keyRejected ? 'key_rejected' : 'signed_in',
    workspace_id: auth.workspace_id,
    workspace_name: auth.workspace_name,
    workspace_url_key: auth.workspace_url_key,
    viewer_id: auth.viewer_id,
    viewer_name: auth.viewer_name,
  };
}

/**
 * Owns `$STORE/linear-auth.json` with mode 0600, separate from `host.sqlite`.
 * Never logs or exposes the stored key or file path.
 */
export class LinearAuthStore {
  private readonly path: string;

  constructor(storeDirectory: string) {
    this.path = join(storeDirectory, FILE_NAME);
  }

  async read(): Promise<StoredLinearAuth | null> {
    let handle;
    try {
      // Open with O_NOFOLLOW so symlinks are refused atomically without a lstat-to-read race.
      handle = await open(this.path, constants.O_RDONLY | constants.O_NOFOLLOW);
    } catch (error) {
      if (missing(error)) return null;
      const code = (error as { code?: string }).code;
      if (code === 'ELOOP' || code === 'EISDIR') {
        fail('internal', 'Linear credential storage has an unexpected file type.');
      }
      fail('internal', 'Linear credential could not be read.');
    }
    let parsed: unknown;
    try {
      const info = await handle.stat();
      if (!info.isFile()) fail('internal', 'Linear credential storage has an unexpected file type.');
      if (info.size > 65_536) fail('internal', 'Linear credential storage has an unexpected format.');
      // Tighten permissions if another process loosened them, matching Pi's auth.json handling.
      if ((info.mode & 0o077) !== 0) {
        await handle.chmod(0o600).catch(() => fail('internal', 'Linear credential permissions could not be secured.'));
      }
      try {
        parsed = JSON.parse(await handle.readFile('utf8'));
      } catch {
        fail('internal', 'Linear credential storage could not be read.');
      }
    } finally {
      await handle.close();
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      fail('internal', 'Linear credential storage has an unexpected format.');
    }
    const record = parsed as Record<string, unknown>;
    if (typeof record.api_key !== 'string' || !KEY_PATTERN.test(record.api_key) || Buffer.byteLength(record.api_key) > LINEAR_KEY_MAX_BYTES) {
      fail('internal', 'Linear credential storage is invalid. Sign in again.');
    }
    const credentialId = typeof record.credential_id === 'string' && /^[a-f0-9]{16,64}$/.test(record.credential_id)
      ? record.credential_id
      : undefined;
    return {
      api_key: record.api_key,
      workspace_id: boundLinearText(record.workspace_id, FIELD_LIMITS.id),
      workspace_name: boundLinearText(record.workspace_name, FIELD_LIMITS.name),
      workspace_url_key: boundLinearText(record.workspace_url_key, FIELD_LIMITS.name),
      viewer_id: boundLinearText(record.viewer_id, FIELD_LIMITS.id),
      viewer_name: boundLinearText(record.viewer_name, FIELD_LIMITS.name),
      saved_at: typeof record.saved_at === 'number' && Number.isFinite(record.saved_at) ? record.saved_at : 0,
      ...(credentialId ? { credential_id: credentialId } : {}),
    };
  }

  async write(auth: StoredLinearAuth): Promise<void> {
    const existing = await lstat(this.path).catch(error => missing(error) ? undefined : fail('internal', 'Linear credential storage could not be inspected.'));
    if (existing && !existing.isFile()) fail('internal', 'Linear credential storage has an unexpected file type.');
    const temporary = join(dirname(this.path), `.${FILE_NAME}.${randomBytes(6).toString('hex')}.tmp`);
    let handle;
    try {
      await mkdir(dirname(this.path), { recursive: true, mode: 0o700 });
      handle = await open(temporary, 'wx', 0o600);
      await handle.writeFile(JSON.stringify(auth) + '\n', 'utf8');
      await handle.chmod(0o600);
      await handle.sync();
      await rename(temporary, this.path);
      await chmod(this.path, 0o600);
    } catch {
      await unlink(temporary).catch(() => {});
      fail('internal', 'Linear credential could not be stored.');
    } finally {
      await handle?.close();
    }
  }

  async remove(): Promise<void> {
    await unlink(this.path).catch(error => {
      if (!missing(error)) fail('internal', 'Linear credential could not be removed.');
    });
  }
}
