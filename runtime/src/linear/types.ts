import { HostError } from '../protocol.ts';
import { escapeDisplay } from '../text.ts';

/** Fixed Linear GraphQL endpoint. Production code never overrides this from settings or env. */
export const LINEAR_GRAPHQL_URL = 'https://api.linear.app/graphql';
export const LINEAR_TIMEOUT_MS = 10_000;
export const LINEAR_MAX_RESPONSE_BYTES = 2 * 1024 * 1024;

export const LINEAR_PAGE_SIZE = 50;
export const LINEAR_MAX_TEAMS = 50;
export const LINEAR_MAX_STATES = 50;
export const LINEAR_MAX_LABELS = 8;
export const LINEAR_MAX_ATTACHMENTS = 20;
export const LINEAR_COMMENTS_PAGE_SIZE = 25;

export const LINEAR_CACHE_TEAMS_TTL_MS = 15 * 60 * 1000;
export const LINEAR_CACHE_ISSUES_TTL_MS = 3 * 60 * 1000;
export const LINEAR_CACHE_DETAIL_TTL_MS = 2 * 60 * 1000;
export const LINEAR_CACHE_MAX_ISSUES_PER_TEAM = 500;
export const LINEAR_CACHE_MAX_DETAILS = 200;
export const LINEAR_CACHE_MAX_PAGES = 100;
export const LINEAR_MAX_RECENT_WRITES = 500;

/** An issue description written in Plait; Linear stores more, the request line holds 64 KB. */
export const LINEAR_DESCRIPTION_MAX_BYTES = 32 * 1024;
/** Linear's file storage, documented at linear.app/developers/file-storage-authentication. */
export const LINEAR_IMAGE_HOST = 'uploads.linear.app';
export const LINEAR_COMMENT_BODY_MAX_BYTES = 16 * 1024;
export const LINEAR_KEY_MAX_BYTES = 4096;

export const LINEAR_FILTER_PRESETS = ['active', 'assigned', 'backlog', 'completed', 'all'] as const;
export type LinearFilterPreset = typeof LINEAR_FILTER_PRESETS[number];

export const PARTIAL_DATA_WARNING = 'Some Linear fields could not be loaded.';

export const FIELD_LIMITS = {
  id: 128,
  identifier: 32,
  key: 32,
  name: 256,
  title: 512,
  url: 2048,
  type: 64,
  timestamp: 64,
  cursor: 512,
  description: 65_536,
  commentBody: 16_384,
} as const;

export type LinearErrorKind =
  | 'not_signed_in'
  | 'key_rejected'
  | 'read_only_key'
  | 'rate_limited'
  | 'offline'
  | 'not_found'
  | 'invalid_request';

const ERROR_CODES: Record<LinearErrorKind, string> = {
  not_signed_in: 'linear_not_signed_in',
  key_rejected: 'linear_key_rejected',
  read_only_key: 'linear_read_only_key',
  rate_limited: 'linear_rate_limited',
  offline: 'linear_offline',
  not_found: 'not_found',
  invalid_request: 'invalid_request',
};

/** Fixed, safe host error for Linear operations. Never includes raw response text, headers, or keys. */
export class LinearError extends HostError {
  readonly kind: LinearErrorKind;
  definitelyRejected = false;
  readonly retryAfterSeconds?: number;

  constructor(kind: LinearErrorKind, message: string, retryAfterSeconds?: number) {
    super(ERROR_CODES[kind], message);
    this.kind = kind;
    this.retryAfterSeconds = retryAfterSeconds;
  }
}

export type LinearIdentity = {
  workspace_id: string;
  workspace_name: string;
  workspace_url_key: string;
  viewer_id: string;
  viewer_name: string;
};

export type LinearAuthStatus = {
  signed_in: boolean;
  status: 'signed_in' | 'signed_out' | 'key_rejected';
  workspace_id: string | null;
  workspace_name: string | null;
  workspace_url_key: string | null;
  viewer_id: string | null;
  viewer_name: string | null;
};

export type LinearWorkflowState = {
  id: string;
  name: string;
  type: string;
  position: number;
};

export type LinearTeam = {
  id: string;
  key: string;
  name: string;
  states: LinearWorkflowState[];
  states_complete?: boolean | null;
};

export type LinearLabel = {
  id: string;
  name: string;
};

export type LinearIssueSummary = {
  id: string;
  identifier: string;
  title: string;
  url: string;
  priority: number;
  updated_at: string;
  team_id: string;
  state: { id: string; name: string; type: string };
  assignee: { name: string } | null;
  labels: LinearLabel[];
};

export type LinearAttachment = {
  id: string;
  title: string;
  url: string;
};

export type LinearComment = {
  id: string;
  body: string;
  created_at: string;
  updated_at: string;
  author: string | null;
};

export type LinearIssueDetail = LinearIssueSummary & {
  description: string | null;
  attachments: LinearAttachment[];
  comments: LinearComment[];
  comments_has_more: boolean;
  comments_next_cursor: string | null;
};

export type LinearProjectBinding = {
  workspace_id: string | null;
  team_id: string;
  team_key: string | null;
  team_name: string | null;
  updated_at: number;
};

export type LinearThreadLink = {
  thread_id: string;
  workspace_id: string | null;
  issue_id: string;
  identifier: string;
  url: string;
  title: string;
  linked_at: number;
};

export type LinearQueryResult<T> = {
  data: T;
  partial: boolean;
  warning: string | null;
};

/** Clips a string to at most `maxBytes` UTF-8 bytes without splitting a Unicode code point. */
function clipUtf8Bytes(text: string, maxBytes: number): string {
  if (Buffer.byteLength(text, 'utf8') <= maxBytes) return text;
  let bytes = 0;
  let end = 0;
  while (end < text.length) {
    const code = text.codePointAt(end)!;
    const charBytes = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
    const units = code > 0xffff ? 2 : 1;
    if (bytes + charBytes > maxBytes) break;
    bytes += charBytes;
    end += units;
  }
  return text.slice(0, end);
}

/** Escapes display controls and bounds any string returned by Linear in UTF-8 bytes before it leaves the host. */
export function boundLinearText(value: unknown, limit: number): string {
  if (typeof value !== 'string') return '';
  return clipUtf8Bytes(escapeDisplay(value), limit);
}

export function boundNullableText(value: unknown, limit: number): string | null {
  if (typeof value !== 'string') return null;
  return clipUtf8Bytes(escapeDisplay(value), limit);
}

/** Keeps only bounded http/https URLs without embedded credentials. */
export function safeLinearUrl(value: unknown): string {
  if (typeof value !== 'string' || Buffer.byteLength(value, 'utf8') > FIELD_LIMITS.url) return '';
  if (/[\u0000-\u001f\u007f]/.test(value) || !URL.canParse(value)) return '';
  const url = new URL(value);
  if ((url.protocol !== 'https:' && url.protocol !== 'http:') || url.username || url.password) return '';
  return clipUtf8Bytes(escapeDisplay(url.href), FIELD_LIMITS.url);
}

export function safeNumber(value: unknown, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

export type IssueEditFields = { title?: string; description?: string };
export type EditVerification = 'matched' | 'different' | 'unverified';
