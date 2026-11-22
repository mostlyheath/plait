import { randomUUID } from 'node:crypto';
import type { AuthEvent, AuthInteraction, AuthPrompt } from '@earendil-works/pi-ai';
import { fail } from './protocol.ts';
import type { Failure } from './protocol.ts';

export type SignInState = 'starting' | 'waiting' | 'finishing' | 'signed_in' | 'failed' | 'cancelled' | 'expired';
export type SignInInput = {
  kind: 'code' | 'text' | 'secret' | 'choice'; message: string; placeholder: string | null;
  required: boolean; choices: { id: string; label: string }[];
};
export type SignInView = {
  id: string; provider: string; state: SignInState; url: string | null; user_code: string | null;
  input: SignInInput | null; message: string | null; expires_in_seconds: number; error: Failure | null;
};
export type SignInLimits = {
  /** A flow that the user abandons is cancelled after this long. */
  ttlMs: number;
  /** Finished records stay readable this long, so a poll can still see the result. */
  retainMs: number;
  /** Flows in progress at once. */
  pending: number;
  /** Records kept, finished ones included. */
  records: number;
  /** How long a reply waits for a flow to show something or finish. */
  settleMs: number;
};
export const SIGN_IN_LIMITS: SignInLimits = { ttlMs: 600_000, retainMs: 300_000, pending: 4, records: 16, settleMs: 3000 };

/** Runs the provider's flow. It resolves once the credential is stored. */
export type Login = (interaction: AuthInteraction) => Promise<unknown>;

type Pending = { view: SignInInput; resolve(value: string): void; reject(error: Error): void };
type Entry = {
  id: string; provider: string; state: SignInState; url: string | null; userCode: string | null;
  message: string | null; error: Failure | null; method: string | undefined; pending: Pending | undefined;
  stored: boolean; deadline: number; finished: number; controller: AbortController;
  timer: NodeJS.Timeout | undefined; done: Promise<void>; waiters: Set<() => void>;
};

const cancelled = () => new Error('Login cancelled');
const terminal = (state: SignInState) => state !== 'starting' && state !== 'waiting' && state !== 'finishing';
const moving = (state: SignInState) => state === 'starting' || state === 'finishing';
const clip = (text: string | undefined, limit = 512) => text ? text.slice(0, limit) : null;

/** Sign-in pages open in the user's browser, so only web addresses are passed on. */
function webUrl(value: string): string | null {
  if (!URL.canParse(value)) return null;
  const url = new URL(value);
  return (url.protocol === 'https:' || url.protocol === 'http:') && value.length <= 4096 ? url.href : null;
}

/** True when the provider will send the browser back to this machine, so a pasted code is only a fallback. */
function returnsToLoopback(address: string | null): boolean {
  if (!address) return false;
  const parameters = new URL(address).searchParams;
  const target = parameters.get('redirect_uri') ?? parameters.get('callback_url');
  if (!target || !URL.canParse(target)) return false;
  return ['localhost', '127.0.0.1', '[::1]'].includes(new URL(target).hostname);
}

/**
 * Pi's messages can quote provider response bodies and token requests. Only the fixed
 * sentences below leave the host; the original text is used to choose one and then dropped.
 */
export class SignInFailure extends Error {}

function describe(error: unknown): Failure {
  if (error instanceof SignInFailure) return { code: 'permission_denied', message: error.message };
  const text = error instanceof Error ? error.message : '';
  if (/state mismatch/i.test(text)) return { code: 'invalid_request', message: 'The pasted code belongs to a different sign-in. Start again.' };
  if (/missing authorization code/i.test(text)) return { code: 'invalid_request', message: 'No authorization code was found in the pasted text. Start again.' };
  if (/port \d+ is in use|EADDRINUSE/i.test(text)) return { code: 'busy', message: 'The sign-in port is in use by another app or an unfinished sign-in. Close it and start again.' };
  if (/timed out|expired/i.test(text)) return { code: 'permission_denied', message: 'The sign-in timed out. Start again.' };
  if (/denied/i.test(text)) return { code: 'permission_denied', message: 'The provider denied the sign-in.' };
  return { code: 'permission_denied', message: 'Sign-in did not complete. Start again. Provider details are not shown.' };
}

/** Pending provider sign-ins. Bounded in number and time; nothing here is saved to disk. */
export class SignIns {
  private readonly entries = new Map<string, Entry>();
  private readonly limits: SignInLimits;

  constructor(limits: Partial<SignInLimits> = {}) {
    this.limits = { ...SIGN_IN_LIMITS, ...limits };
  }

  /** `stored` runs after the credential is saved and before the sign-in reports success. */
  async start(provider: string, method: string | undefined, login: Login, stored: () => Promise<void>, ttlMs = this.limits.ttlMs): Promise<SignInView> {
    this.sweep();
    // A second attempt replaces the first: the desktop may have lost the earlier ID.
    await this.cancelProvider(provider);
    if ([...this.entries.values()].filter(entry => !terminal(entry.state)).length >= this.limits.pending) {
      fail('busy', 'Too many sign-ins are in progress. Finish or cancel one first.');
    }
    this.makeRoom();
    const entry: Entry = {
      id: randomUUID(), provider, state: 'starting', url: null, userCode: null, message: null, error: null, method,
      pending: undefined, stored: false, deadline: Date.now() + ttlMs, finished: 0,
      controller: new AbortController(), timer: undefined, done: Promise.resolve(), waiters: new Set(),
    };
    this.schedule(entry);
    this.entries.set(entry.id, entry);
    entry.done = this.run(entry, login, stored);
    await this.settled(entry);
    return this.view(entry);
  }

  status(id: string): SignInView {
    this.sweep();
    return this.view(this.entry(id));
  }

  async answer(id: string, input: string): Promise<SignInView> {
    const entry = this.entry(id);
    const pending = entry.pending;
    if (!pending) fail('conflict', 'This sign-in is not waiting for input.');
    if (pending.view.kind === 'choice' && !pending.view.choices.some(choice => choice.id === input)) {
      fail('invalid_request', 'input must be one of the offered choices.');
    }
    // Only a free-text question, such as an optional enterprise domain, accepts an empty answer.
    if (pending.view.kind !== 'text' && !input.trim()) fail('invalid_request', 'input must not be empty.');
    entry.pending = undefined; entry.state = 'finishing';
    pending.resolve(input);
    await this.settled(entry);
    return this.view(entry);
  }

  cancel(id: string): SignInView {
    const entry = this.entry(id);
    this.stop(entry, 'cancelled');
    return this.view(entry);
  }

  /** Stops this provider's unfinished flow and waits for it to release its callback port. */
  async cancelProvider(provider: string): Promise<void> {
    const stopped = [...this.entries.values()].filter(entry => entry.provider === provider && this.stop(entry, 'cancelled'));
    await this.released(stopped);
  }

  async close(): Promise<void> {
    const stopped = [...this.entries.values()].filter(entry => this.stop(entry, 'cancelled'));
    await this.released(stopped);
    // A flow that already stored its credential still has to finish its reload.
    await this.released([...this.entries.values()].filter(entry => !terminal(entry.state)));
  }

  private entry(id: string): Entry {
    const entry = this.entries.get(id);
    if (!entry) fail('not_found', 'Sign-in not found. It may have finished long ago; start again.');
    return entry;
  }

  private view(entry: Entry): SignInView {
    return {
      id: entry.id, provider: entry.provider, state: entry.state, url: entry.url, user_code: entry.userCode,
      input: entry.pending?.view ?? null, message: entry.message,
      expires_in_seconds: terminal(entry.state) ? 0 : Math.max(0, Math.ceil((entry.deadline - Date.now()) / 1000)),
      error: entry.error,
    };
  }

  private async run(entry: Entry, login: Login, stored: () => Promise<void>): Promise<void> {
    try {
      await login({
        signal: entry.controller.signal,
        prompt: prompt => this.ask(entry, prompt),
        notify: event => this.note(entry, event),
      });
    } catch (error) {
      // Pi reports a saved credential whose in-place refresh failed; the reload below replaces that refresh.
      if ((error as Error | undefined)?.name !== 'CredentialSynchronizationError') {
        if (!terminal(entry.state)) this.finish(entry, 'failed', describe(error));
        return;
      }
    }
    // The credential is on disk. A late cancel or timeout must not report otherwise.
    entry.stored = true; entry.pending = undefined; entry.state = 'finishing'; entry.error = null;
    clearTimeout(entry.timer);
    this.wake(entry);
    try {
      await stored();
      this.finish(entry, 'signed_in');
    } catch {
      this.finish(entry, 'failed', { code: 'internal', message: 'The sign-in was saved, but the provider list could not be reloaded. Reload providers to finish.' });
    }
  }

  private ask(entry: Entry, prompt: AuthPrompt): Promise<string> {
    if (prompt.type === 'select' && entry.method !== undefined) {
      // The requested method answers only the flow's first choice.
      const method = entry.method; entry.method = undefined;
      if (prompt.options.some(option => option.id === method)) return Promise.resolve(method);
    }
    if (terminal(entry.state) || prompt.signal?.aborted) return Promise.reject(cancelled());
    return new Promise((resolve, reject) => {
      entry.pending?.reject(cancelled());
      const pending: Pending = { view: this.input(entry, prompt), resolve, reject };
      entry.pending = pending; entry.state = 'waiting';
      // The flow withdraws a prompt when the browser redirect arrives first.
      prompt.signal?.addEventListener('abort', () => {
        if (entry.pending === pending) { entry.pending = undefined; entry.state = 'finishing'; this.wake(entry); }
        reject(cancelled());
      }, { once: true });
      this.wake(entry);
    });
  }

  private input(entry: Entry, prompt: AuthPrompt): SignInInput {
    const kind = prompt.type === 'select' ? 'choice' : prompt.type === 'manual_code' ? 'code' : prompt.type;
    return {
      kind, message: clip(prompt.message) ?? '',
      placeholder: prompt.type === 'select' ? null : clip(prompt.placeholder, 256),
      required: kind !== 'code' || !returnsToLoopback(entry.url),
      choices: prompt.type === 'select' ? prompt.options.slice(0, 16).map(option => ({ id: option.id, label: clip(option.label, 128) ?? option.id })) : [],
    };
  }

  private note(entry: Entry, event: AuthEvent): void {
    if (terminal(entry.state) || entry.stored) return;
    if (event.type === 'auth_url' || event.type === 'device_code') {
      const url = webUrl(event.type === 'auth_url' ? event.url : event.verificationUri);
      if (!url) {
        this.finish(entry, 'failed', { code: 'internal', message: 'The provider returned a sign-in address that cannot be opened.' });
        entry.controller.abort();
        return;
      }
      entry.url = url; entry.state = 'waiting';
      if (event.type === 'auth_url') entry.message = clip(event.instructions);
      else {
        entry.userCode = clip(event.userCode, 64);
        // A device code that expires sooner than the host limit ends the sign-in then.
        if (event.expiresInSeconds && event.expiresInSeconds > 0) this.shorten(entry, event.expiresInSeconds * 1000);
      }
    } else {
      entry.message = clip(event.message);
      if (event.type === 'progress' && (entry.url || entry.userCode) && !entry.pending) entry.state = 'finishing';
    }
    this.wake(entry);
  }

  private schedule(entry: Entry): void {
    clearTimeout(entry.timer);
    entry.timer = setTimeout(() => this.stop(entry, 'expired'), Math.max(0, entry.deadline - Date.now()));
    entry.timer.unref();
  }

  private shorten(entry: Entry, ms: number): void {
    if (Date.now() + ms >= entry.deadline) return;
    entry.deadline = Date.now() + ms;
    this.schedule(entry);
  }

  /** Returns whether the flow was still running and has now been told to stop. */
  private stop(entry: Entry, state: 'cancelled' | 'expired'): boolean {
    if (terminal(entry.state) || entry.stored) return false;
    this.finish(entry, state);
    entry.controller.abort();
    return true;
  }

  private finish(entry: Entry, state: SignInState, error: Failure | null = null): void {
    clearTimeout(entry.timer);
    entry.pending?.reject(cancelled());
    // One-time flow values are not kept once they are useless.
    entry.pending = undefined; entry.url = null; entry.userCode = null; entry.message = null;
    entry.state = state; entry.error = error; entry.finished = Date.now();
    this.wake(entry);
  }

  private wake(entry: Entry): void {
    for (const waiter of [...entry.waiters]) waiter();
  }

  /** Resolves when the flow needs the user, has finished, or the short reply budget is spent. */
  private settled(entry: Entry): Promise<void> {
    if (!moving(entry.state)) return Promise.resolve();
    return new Promise(resolve => {
      const done = () => {
        if (moving(entry.state)) return;
        clearTimeout(timer); entry.waiters.delete(done); resolve();
      };
      const timer = setTimeout(() => { entry.waiters.delete(done); resolve(); }, this.limits.settleMs);
      entry.waiters.add(done);
    });
  }

  /** Bounded wait for stopped flows to unwind, including closing their loopback listeners. */
  private async released(entries: Entry[]): Promise<void> {
    if (!entries.length) return;
    let timer: NodeJS.Timeout | undefined;
    const limit = new Promise<void>(resolve => { timer = setTimeout(resolve, 2000); });
    await Promise.race([Promise.allSettled(entries.map(entry => entry.done)), limit]);
    clearTimeout(timer);
    await new Promise(resolve => setImmediate(resolve));
  }

  private sweep(): void {
    const now = Date.now();
    for (const [id, entry] of this.entries) {
      if (terminal(entry.state) && now - entry.finished > this.limits.retainMs) this.entries.delete(id);
    }
  }

  /** Makes space for one more record. Map order is insertion order, so the oldest finished records go first. */
  private makeRoom(): void {
    for (const [id, entry] of this.entries) {
      if (this.entries.size < this.limits.records) break;
      if (terminal(entry.state)) this.entries.delete(id);
    }
  }
}
