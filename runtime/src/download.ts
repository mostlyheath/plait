import { createHash } from 'node:crypto';
/** Reads a response body, refusing one larger than `limit`. */
export async function body(response: Response, limit: number): Promise<Buffer> {
  const declared = Number(response.headers.get('content-length') ?? '0');
  if (declared > limit) throw new Error('The download is larger than expected.');
  if (!response.body) throw new Error('The download was empty.');
  const chunks: Buffer[] = []; let length = 0;
  for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) {
    length += chunk.byteLength;
    if (length > limit) throw new Error('The download is larger than expected.');
    chunks.push(Buffer.from(chunk));
  }
  return Buffer.concat(chunks, length);
}

/** Whether `data` has the `sha512-…` integrity npm records. */
export function matchesIntegrity(data: Buffer, integrity: string): boolean {
  const expected = /^sha512-([A-Za-z0-9+/]{86}==)$/.exec(integrity)?.[1];
  if (!expected) throw new Error('The pinned integrity is not a sha512 value.');
  return createHash('sha512').update(data).digest('base64') === expected;
}

