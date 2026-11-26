import type { historyPage, historyText } from '../history.ts';
import { clipTail } from '../text.ts';
import { fail, HostError } from '../protocol.ts';

import type { Send } from './requests.ts';
export function clip(text: string, limit: number) {
  const bytes = Buffer.from(text);
  if (bytes.length <= limit) return text;
  const marker = '[Earlier text clipped]\n';
  let at = bytes.length - limit + marker.length;
  while ((bytes[at] & 0xc0) === 0x80) at++;
  return marker + bytes.subarray(at).toString();
}

// Submit bounds the escaped JSON frame, so control characters can consume six bytes each.
export function fitAnswer(answer: string, allowance: number) {
  if (Buffer.byteLength(JSON.stringify(answer)) <= allowance) return answer;
  const marker = '[Earlier text clipped]\n';
  let low = 0, high = answer.length;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (Buffer.byteLength(JSON.stringify(marker + clipTail(answer, middle))) <= allowance) low = middle;
    else high = middle - 1;
  }
  return marker + clipTail(answer, low);
}

export async function excerpt(send: Send, thread_id: string, at?: string, mode: 'conversation' | 'answer' | 'locate' = 'conversation') {
  const answer = mode === 'answer';
  const limit = answer ? 8192 : 24576;
  let before: string | undefined;
  let found = !at;
  let result = '';
  let pages = 0;
  const readText = (ref: string, offset: number) => send({ op: 'history_text', protocol_version: 6, thread_id, ref, offset }) as ReturnType<typeof historyText>;
  do {
    if (++pages > 20) fail('unavailable', 'History search exceeded twenty pages.');
    const page = await send({ op: 'history', protocol_version: 6, thread_id, limit: 32, ...(before && { before }) }) as Awaited<ReturnType<typeof historyPage>>;
    for (const item of page.items.reverse()) {
      if (item.type !== 'message') continue;
      if (!found && item.id !== at) continue;
      found = true;
      if (mode === 'locate') return '';
      if (answer && item.role !== 'assistant') return '';
      let text = item.text;
      const more = item.more?.find(p => p.field === 'text');
      if (more) {
        let offset = Math.max(more.offset, more.length - limit);
        const omitted = offset > more.offset;
        if (omitted) text = '[Earlier text clipped]\n';
        let part;
        try { part = await readText(more.ref, offset); }
        catch (error) { if (!omitted) throw error; part = await readText(more.ref, ++offset); }
        for (; ;) {
          text += part.text;
          if (part.next_offset === null) break;
          part = await readText(more.ref, part.next_offset);
        }
      }
      text = text.replaceAll('\0', '');
      if (answer) return clip(text, limit);
      result = item.role + ': ' + text + (result ? '\n\n' + result : '');
      if (Buffer.byteLength(result) > limit) return clip(result, limit);
    }
    before = page.before ?? undefined;
  } while (before);
  if (!found) fail('invalid_request', 'Unknown message.');
  return result;
}

export class ReadTimeout extends HostError {
  constructor() { super('unavailable', 'Graph request exceeded four seconds.'); }
}

export class Deadline {
  private end = performance.now() + 4000;
  async read<T>(work: () => Promise<T>): Promise<T> {
    const remaining = this.end - performance.now();
    if (remaining <= 0) throw new ReadTimeout();
    let timer: NodeJS.Timeout | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new ReadTimeout()), remaining);
    });
    try {
      const result = await Promise.race([work(), timeout]);
      if (performance.now() >= this.end) throw new ReadTimeout();
      return result;
    } finally { clearTimeout(timer); }
  }
}
