// A small, strict reader for the gzipped tar archives the npm registry serves.
// It returns regular files only and refuses anything that could write outside
// the destination or exhaust memory: absolute paths, `..`, links, devices,
// oversized entries, too many entries, and truncated or corrupt archives.
import { promisify } from 'node:util';
import { gunzip } from 'node:zlib';

export type TarFile = { path: string; data: Buffer; mode: number };
export type TarLimits = {
  /** The largest the decompressed archive may be. */
  maxArchive: number;
  /** The largest one file may be. */
  maxEntry: number;
  maxEntries: number;
};

const BLOCK = 512;
const inflate = promisify(gunzip);

/** Decompresses and reads a `.tgz`, refusing what `readTar` refuses. */
export async function readTgz(archive: Buffer, limits: TarLimits): Promise<TarFile[]> {
  let tar: Buffer;
  // `maxOutputLength` stops a compression bomb before it is held in memory.
  try { tar = await inflate(archive, { maxOutputLength: limits.maxArchive }); } catch { throw new Error('The archive is corrupt or too large.'); }
  return readTar(tar, limits);
}

function field(header: Buffer, start: number, length: number) {
  const bytes = header.subarray(start, start + length);
  const end = bytes.indexOf(0);
  return bytes.subarray(0, end === -1 ? length : end).toString('utf8');
}

function octal(header: Buffer, start: number, length: number): number {
  // Base-256 sizes (high bit set) only appear for entries over 8 GiB, which are refused anyway.
  if (header[start] & 0x80) throw new Error('A tar entry is too large.');
  const text = field(header, start, length).trim();
  if (!/^[0-7]*$/.test(text)) throw new Error('A tar header is corrupt.');
  return text ? parseInt(text, 8) : 0;
}

function checksumMatches(header: Buffer) {
  let sum = 0;
  for (let index = 0; index < BLOCK; index++) sum += index >= 148 && index < 156 ? 0x20 : header[index];
  return octal(header, 148, 8) === sum;
}

/** A relative path with no empty, `.`, or `..` segment; anything else is refused. */
export function safePath(path: string): string {
  const trimmed = path.replace(/\/+$/, '');
  if (!trimmed || trimmed.startsWith('/') || trimmed.includes('\\') || trimmed.includes('\0')) throw new Error(`Unsafe path in archive: ${JSON.stringify(path.slice(0, 200))}`);
  for (const segment of trimmed.split('/')) {
    if (!segment || segment === '.' || segment === '..') throw new Error(`Unsafe path in archive: ${JSON.stringify(path.slice(0, 200))}`);
  }
  return trimmed;
}

/** `path` and `size` from a pax extended header; other keys do not change what is read. */
function pax(data: Buffer): { path?: string; size?: number } {
  const result: { path?: string; size?: number } = {};
  let offset = 0;
  while (offset < data.length) {
    const space = data.indexOf(0x20, offset);
    if (space === -1) throw new Error('A tar header is corrupt.');
    const length = Number(data.subarray(offset, space).toString('ascii'));
    if (!Number.isSafeInteger(length) || length <= space - offset || offset + length > data.length) throw new Error('A tar header is corrupt.');
    const record = data.subarray(space + 1, offset + length - 1).toString('utf8');
    const equals = record.indexOf('=');
    if (equals > 0) {
      const key = record.slice(0, equals); const value = record.slice(equals + 1);
      if (key === 'path') result.path = value;
      if (key === 'size') {
        if (!/^\d+$/.test(value)) throw new Error('A tar header is corrupt.');
        result.size = Number(value);
      }
    }
    offset += length;
  }
  return result;
}

/** Reads an uncompressed tar archive's regular files. Directories are skipped; any other entry type is refused. */
export function readTar(tar: Buffer, limits: TarLimits): TarFile[] {
  const files: TarFile[] = [];
  let offset = 0;
  let entries = 0;
  let extended: { path?: string; size?: number } = {};
  for (;;) {
    if (offset + BLOCK > tar.length) throw new Error('The archive is truncated.');
    const header = tar.subarray(offset, offset + BLOCK);
    // The archive ends with zero blocks.
    if (header.every(byte => byte === 0)) return files;
    if (!checksumMatches(header)) throw new Error('A tar header is corrupt.');
    if (++entries > limits.maxEntries) throw new Error('The archive has too many entries.');
    const type = String.fromCharCode(header[156] || 0x30);
    const size = extended.size ?? octal(header, 124, 12);
    if (size > limits.maxEntry) throw new Error('A tar entry is too large.');
    const start = offset + BLOCK;
    const end = start + size;
    if (end > tar.length) throw new Error('The archive is truncated.');
    offset = start + Math.ceil(size / BLOCK) * BLOCK;
    const ustar = field(header, 257, 6) === 'ustar';
    const prefix = ustar ? field(header, 345, 155) : '';
    const name = extended.path ?? (prefix ? `${prefix}/${field(header, 0, 100)}` : field(header, 0, 100));
    const data = tar.subarray(start, end);
    if (type === 'x') { extended = pax(data); continue; }
    if (type === 'g') continue;
    extended = {};
    const path = safePath(name);
    if (type === '5') continue;
    if (type !== '0') throw new Error(`The archive holds a link or special file: ${JSON.stringify(path.slice(0, 200))}`);
    files.push({ path, data, mode: octal(header, 100, 8) });
  }
}
