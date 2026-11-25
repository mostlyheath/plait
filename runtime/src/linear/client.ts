import { escapeDisplay } from '../text.ts';
import {
  FIELD_LIMITS,
  LINEAR_COMMENTS_PAGE_SIZE,
  LINEAR_GRAPHQL_URL,
  LINEAR_MAX_ATTACHMENTS,
  LINEAR_MAX_LABELS,
  LINEAR_MAX_RESPONSE_BYTES,
  LINEAR_MAX_STATES,
  LINEAR_MAX_TEAMS,
  LINEAR_PAGE_SIZE,
  LINEAR_TIMEOUT_MS,
  LinearError,
  PARTIAL_DATA_WARNING,
  boundLinearText,
  safeLinearUrl,
  safeNumber,
  LINEAR_DESCRIPTION_MAX_BYTES,
  LINEAR_IMAGE_HOST,
} from './types.ts';
import type {
  LinearAttachment,
  LinearComment,
  LinearFilterPreset,
  LinearIdentity,
  LinearIssueDetail,
  LinearIssueSummary,
  LinearLabel,
  LinearQueryResult,
  LinearTeam,
  LinearWorkflowState,
} from './types.ts';

export type LinearClientOptions = {
  /** Test-only override; production code always uses LINEAR_GRAPHQL_URL. */
  endpoint?: string;
  /** Test-only: read image paths from this origin instead of their own. */
  imageOrigin?: string;
  timeoutMs?: number;
  maxResponseBytes?: number;
  now?: () => number;
};

type JsonObject = Record<string, unknown>;
type GraphQlError = { message?: unknown; extensions?: { code?: unknown; type?: unknown } };
type AccessMode = 'read' | 'write';

// Fixed GraphQL documents only: query strings are never interpolated from input.
const VIEWER_QUERY = `query PlaitViewer {
  viewer { id name }
  organization { id name urlKey }
}`;

const TEAMS_QUERY = `query PlaitTeams($after: String) {
  teams(first: 50, after: $after) {
    nodes {
      id
      key
      name
      states(first: 50) {
        nodes { id name type position }
        pageInfo { hasNextPage endCursor }
      }
    }
    pageInfo { hasNextPage endCursor }
  }
}`;

const ISSUES_QUERY = `query PlaitIssues($after: String, $filter: IssueFilter) {
  issues(first: 50, after: $after, filter: $filter, orderBy: updatedAt) {
    nodes {
      id
      identifier
      title
      url
      priority
      updatedAt
      team { id }
      state { id name type }
      assignee { name }
      labels(first: 8) { nodes { id name } }
    }
    pageInfo { hasNextPage endCursor }
  }
}`;

const ISSUE_DETAIL_QUERY = `query PlaitIssueDetail($id: String!, $commentsAfter: String) {
  issue(id: $id) {
    id
    identifier
    title
    url
    priority
    updatedAt
    description
    team { id }
    state { id name type }
    assignee { name }
    labels(first: 8) { nodes { id name } }
    attachments(first: 20) { nodes { id title url } }
    comments(first: 25, after: $commentsAfter) {
      nodes { id body createdAt updatedAt user { name } }
      pageInfo { hasNextPage endCursor }
    }
  }
}`;

const COMMENT_CREATE_MUTATION = `mutation PlaitCommentCreate($issueId: String!, $body: String!, $commentId: String) {
  commentCreate(input: { id: $commentId, issueId: $issueId, body: $body }) {
    success
    comment { id }
  }
}`;

const ISSUE_UPDATE_STATE_MUTATION = `mutation PlaitIssueUpdateState($id: String!, $stateId: String!) {
  issueUpdate(id: $id, input: { stateId: $stateId }) {
    success
    issue { id }
  }
}`;

// Field names from Linear's SDK schema (IssueCreateInput, IssueUpdateInput) at the
// revision LINEAR.md cites. `id` is the documented optional UUID v4 for a new issue.
const ISSUE_CREATE_MUTATION = `mutation PlaitIssueCreate($id: String!, $teamId: String!, $title: String!, $description: String, $stateId: String) {
  issueCreate(input: { id: $id, teamId: $teamId, title: $title, description: $description, stateId: $stateId }) {
    success
    issue { id identifier }
  }
}`;

const ISSUE_UPDATE_TEXT_MUTATION = `mutation PlaitIssueUpdateText($id: String!, $title: String, $description: String) {
  issueUpdate(id: $id, input: { title: $title, description: $description }) {
    success
    issue { id }
  }
}`;

const ISSUE_TEXT_QUERY = `query PlaitIssueText($id: String!) {
  issue(id: $id) { id title description updatedAt }
}`;

/** Image bytes read from Linear's file storage; the desktop decodes them. */
export const LINEAR_IMAGE_MAX_BYTES = 5 * 1024 * 1024;
const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308]);
// The characters the write validators refuse; text holding them cannot round-trip.
const EDIT_TITLE_CONTROL = /[\u0000-\u001f\u007f]/;
const EDIT_DESCRIPTION_CONTROL = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/;

const MAX_WAIT_SECONDS = 86_400;
const DEFAULT_RATE_LIMIT_WAIT_SECONDS = 60;
const MAX_TEAM_PAGES = 10;
const MAX_STATE_POSITION = 1_000_000;
const UNSAFE_CURSOR_PATTERN = /[\p{Cf}\p{Zl}\p{Zp}\u0000-\u001f\u007f-\u009f]/u;

function safePriority(value: unknown): number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 4 ? value : 0;
}

function safePosition(value: unknown): number {
  const num = safeNumber(value, 0);
  return Math.max(-MAX_STATE_POSITION, Math.min(MAX_STATE_POSITION, num));
}

const asObject = (value: unknown): JsonObject | null =>
  typeof value === 'object' && value !== null && !Array.isArray(value) ? value as JsonObject : null;

const asArray = (value: unknown): unknown[] => Array.isArray(value) ? value : [];

/**
 * Computes seconds to wait from Linear's rate-limit headers:
 * `Retry-After` (seconds or HTTP date) and `X-RateLimit-*-Reset` (UTC epoch ms).
 */
export function parseRetryAfterSeconds(headers: Headers, now: number): number {
  const candidates: number[] = [];
  const retryAfter = headers.get('retry-after')?.trim();
  if (retryAfter) {
    if (/^\d+(\.\d+)?$/.test(retryAfter)) {
      candidates.push(Math.ceil(Number(retryAfter)));
    } else {
      const parsedDate = Date.parse(retryAfter);
      if (Number.isFinite(parsedDate)) candidates.push(Math.ceil((parsedDate - now) / 1000));
    }
  }
  for (const name of ['x-ratelimit-complexity-reset', 'x-ratelimit-requests-reset']) {
    const raw = headers.get(name)?.trim();
    if (!raw || !/^\d+$/.test(raw)) continue;
    const numeric = Number(raw);
    if (!Number.isFinite(numeric)) continue;
    // Linear sends UTC epoch milliseconds; guard against seconds if a header ever uses them.
    const resetMs = numeric > 1_000_000_000_000 ? numeric : numeric * 1000;
    candidates.push(Math.ceil((resetMs - now) / 1000));
  }
  const positive = candidates.filter(seconds => Number.isFinite(seconds) && seconds > 0);
  if (!positive.length) return DEFAULT_RATE_LIMIT_WAIT_SECONDS;
  return Math.min(MAX_WAIT_SECONDS, Math.max(...positive));
}

function classifyGraphQlErrors(errors: GraphQlError[], mode: AccessMode, headers: Headers, now: number): LinearError | null {
  for (const entry of errors) {
    const code = typeof entry.extensions?.code === 'string' ? entry.extensions.code.toUpperCase() : '';
    const type = typeof entry.extensions?.type === 'string' ? entry.extensions.type.toLowerCase() : '';
    if (code === 'RATELIMITED' || type === 'ratelimited') {
      const seconds = parseRetryAfterSeconds(headers, now);
      return new LinearError('rate_limited', `Linear rate limit reached. Retry in ${seconds} seconds.`, seconds);
    }
    if (code === 'AUTHENTICATION_ERROR' || code === 'UNAUTHENTICATED' || type === 'authentication error') {
      return new LinearError('key_rejected', 'Linear rejected the API key. Check or replace it in Settings.');
    }
    if (code === 'FORBIDDEN' || type === 'forbidden') {
      return mode === 'write'
        ? new LinearError('read_only_key', 'This Linear API key does not allow writes. Use a key with write access.')
        : new LinearError('key_rejected', 'Linear rejected the API key. Check or replace it in Settings.');
    }
  }
  for (const entry of errors) {
    const code = typeof entry.extensions?.code === 'string' ? entry.extensions.code.toUpperCase() : '';
    if (code === 'BAD_USER_INPUT' || code === 'GRAPHQL_VALIDATION_FAILED' || code === 'INPUT_ERROR') {
      return new LinearError('invalid_request', 'Linear rejected the request.');
    }
  }
  return null;
}

/** Builds a Linear `IssueFilter` variable object from validated inputs. */
export function buildIssueFilter(teamId: string, preset: LinearFilterPreset, search?: string): JsonObject {
  const filter: JsonObject = { team: { id: { eq: teamId } } };
  if (preset === 'active') {
    filter.state = { type: { in: ['started', 'unstarted', 'triage'] } };
  } else if (preset === 'assigned') {
    filter.state = { type: { in: ['started', 'unstarted', 'triage'] } };
    filter.assignee = { isMe: { eq: true } };
  } else if (preset === 'backlog') {
    filter.state = { type: { eq: 'backlog' } };
  } else if (preset === 'completed') {
    filter.state = { type: { in: ['completed', 'canceled'] } };
  }
  const query = search?.trim();
  if (query) {
    const identifierMatch = /^(?:[A-Za-z][A-Za-z0-9]{0,15}-|#?)(\d{1,9})$/.exec(query);
    if (identifierMatch) {
      filter.or = [
        { number: { eq: Number(identifierMatch[1]) } },
        { title: { containsIgnoreCase: query } },
      ];
    } else {
      filter.title = { containsIgnoreCase: query };
    }
  }
  return filter;
}

type ResponseSanitizer = {
  text(value: unknown, limit: number): string;
  nullableText(value: unknown, limit: number): string | null;
  url(value: unknown): string;
  containsSecret(value: string): boolean;
};

/**
 * Scrubs an echoed API key (raw, URL-encoded, or display-escaped) before any string
 * from Linear is clipped, cached in SQLite, or returned to the desktop.
 */
function makeSanitizer(apiKey: string): ResponseSanitizer {
  const variants = apiKey.length >= 8
    ? [...new Set([apiKey, encodeURIComponent(apiKey), escapeDisplay(apiKey)])]
        .filter(secret => secret.length >= 8)
        .sort((a, b) => b.length - a.length)
    : [];
  const scrub = (value: string): string => {
    let out = value;
    for (const secret of variants) {
      if (out.includes(secret)) out = out.split(secret).join('[redacted]');
    }
    return out;
  };
  const containsSecret = (value: string): boolean => variants.some(secret => value.includes(secret));
  return {
    text(value, limit) {
      if (typeof value !== 'string') return '';
      return scrub(boundLinearText(scrub(value), limit));
    },
    nullableText(value, limit) {
      if (typeof value !== 'string') return null;
      return scrub(boundLinearText(scrub(value), limit));
    },
    url(value) {
      if (typeof value !== 'string' || containsSecret(value)) return '';
      const safe = safeLinearUrl(value);
      return !safe || containsSecret(safe) ? '' : safe;
    },
    containsSecret,
  };
}

function parseState(raw: unknown, sanitize: ResponseSanitizer): LinearWorkflowState | null {
  const obj = asObject(raw);
  if (!obj) return null;
  const id = sanitize.text(obj.id, FIELD_LIMITS.id);
  const name = sanitize.text(obj.name, FIELD_LIMITS.name);
  if (!id || !name) return null;
  return {
    id,
    name,
    type: sanitize.text(obj.type, FIELD_LIMITS.type),
    position: safePosition(obj.position),
  };
}

function parseLabel(raw: unknown, sanitize: ResponseSanitizer): LinearLabel | null {
  const obj = asObject(raw);
  if (!obj) return null;
  const id = sanitize.text(obj.id, FIELD_LIMITS.id);
  const name = sanitize.text(obj.name, FIELD_LIMITS.name);
  if (!id || !name) return null;
  return { id, name };
}

function parseIssueSummary(raw: unknown, sanitize: ResponseSanitizer): LinearIssueSummary | null {
  const obj = asObject(raw);
  if (!obj) return null;
  const id = sanitize.text(obj.id, FIELD_LIMITS.id);
  const identifier = sanitize.text(obj.identifier, FIELD_LIMITS.identifier);
  if (!id || !identifier) return null;
  const stateObj = asObject(obj.state);
  const teamObj = asObject(obj.team);
  const assigneeObj = asObject(obj.assignee);
  const labelsObj = asObject(obj.labels);
  const labels = asArray(labelsObj?.nodes)
    .slice(0, LINEAR_MAX_LABELS)
    .map(item => parseLabel(item, sanitize))
    .filter((label): label is LinearLabel => label !== null);
  const assigneeName = assigneeObj ? sanitize.text(assigneeObj.name, FIELD_LIMITS.name) : '';
  return {
    id,
    identifier,
    title: sanitize.text(obj.title, FIELD_LIMITS.title),
    url: sanitize.url(obj.url),
    priority: safePriority(obj.priority),
    updated_at: sanitize.text(obj.updatedAt, FIELD_LIMITS.timestamp),
    team_id: sanitize.text(teamObj?.id, FIELD_LIMITS.id),
    state: {
      id: sanitize.text(stateObj?.id, FIELD_LIMITS.id),
      name: sanitize.text(stateObj?.name, FIELD_LIMITS.name),
      type: sanitize.text(stateObj?.type, FIELD_LIMITS.type),
    },
    assignee: assigneeName ? { name: assigneeName } : null,
    labels,
  };
}

function parseAttachment(raw: unknown, sanitize: ResponseSanitizer): LinearAttachment | null {
  const obj = asObject(raw);
  if (!obj) return null;
  const id = sanitize.text(obj.id, FIELD_LIMITS.id);
  const url = sanitize.url(obj.url);
  if (!id || !url) return null;
  return { id, title: sanitize.text(obj.title, FIELD_LIMITS.title), url };
}

function parseComment(raw: unknown, sanitize: ResponseSanitizer): LinearComment | null {
  const obj = asObject(raw);
  if (!obj) return null;
  const id = sanitize.text(obj.id, FIELD_LIMITS.id);
  if (!id) return null;
  const userObj = asObject(obj.user);
  const author = userObj ? sanitize.text(userObj.name, FIELD_LIMITS.name) : '';
  return {
    id,
    body: sanitize.text(obj.body, FIELD_LIMITS.commentBody),
    created_at: sanitize.text(obj.createdAt, FIELD_LIMITS.timestamp),
    updated_at: sanitize.text(obj.updatedAt, FIELD_LIMITS.timestamp),
    author: author || null,
  };
}

function parsePageInfo(raw: unknown, sanitize: ResponseSanitizer): { hasMore: boolean; nextCursor: string | null } {
  const info = asObject(raw);
  const hasMore = info?.hasNextPage === true;
  const rawCursor = info?.endCursor;
  const cursor = typeof rawCursor === 'string' && rawCursor.length > 0
    && Buffer.byteLength(rawCursor, 'utf8') <= FIELD_LIMITS.cursor
    && !UNSAFE_CURSOR_PATTERN.test(rawCursor) && !sanitize.containsSecret(rawCursor)
    ? rawCursor
    : null;
  return { hasMore: hasMore && cursor !== null, nextCursor: hasMore ? cursor : null };
}

/**
 * Hardened Linear GraphQL client.
 * Uses Node's fetch with `redirect: 'error'`, a 10 s deadline, a 2 MiB body cap,
 * `Authorization: <key>` (no Bearer for API keys), fixed query documents, and
 * display escaping + length bounds on every string returned from Linear.
 */
export class LinearClient {
  private readonly endpoint: string;
  private readonly timeoutMs: number;
  private readonly maxResponseBytes: number;
  private readonly now: () => number;
  private readonly imageOrigin: string | undefined;

  constructor(options: LinearClientOptions = {}) {
    this.endpoint = options.endpoint ?? LINEAR_GRAPHQL_URL;
    this.imageOrigin = options.imageOrigin;
    this.timeoutMs = options.timeoutMs ?? LINEAR_TIMEOUT_MS;
    this.maxResponseBytes = options.maxResponseBytes ?? LINEAR_MAX_RESPONSE_BYTES;
    this.now = options.now ?? Date.now;
  }

  /** Verifies a key with one small query: who am I, which workspace. */
  async viewer(apiKey: string, signal?: AbortSignal): Promise<LinearQueryResult<LinearIdentity>> {
    const sanitize = makeSanitizer(apiKey);
    const response = await this.graphql(apiKey, VIEWER_QUERY, {}, 'read', signal);
    const viewer = asObject(response.data?.viewer);
    const organization = asObject(response.data?.organization);
    const viewerId = sanitize.text(viewer?.id, FIELD_LIMITS.id);
    const workspaceId = sanitize.text(organization?.id, FIELD_LIMITS.id);
    if (!viewerId || !workspaceId) {
      throw this.fallbackError(response.errors, 'read', response.headers)
        ?? new LinearError('offline', 'Linear returned an unexpected identity response.');
    }
    return {
      data: {
        workspace_id: workspaceId,
        workspace_name: sanitize.text(organization?.name, FIELD_LIMITS.name),
        workspace_url_key: sanitize.text(organization?.urlKey, FIELD_LIMITS.name),
        viewer_id: viewerId,
        viewer_name: sanitize.text(viewer?.name, FIELD_LIMITS.name),
      },
      partial: response.errors.length > 0,
      warning: response.errors.length > 0 ? PARTIAL_DATA_WARNING : null,
    };
  }

  /** Lists teams with their workflow states, bounded to LINEAR_MAX_TEAMS. */
  async listTeams(apiKey: string, signal?: AbortSignal): Promise<LinearQueryResult<LinearTeam[]> & { complete: boolean }> {
    const sanitize = makeSanitizer(apiKey);
    const teams: LinearTeam[] = [];
    const seenTeamIds = new Set<string>();
    const seenCursors = new Set<string>();
    let after: string | null = null;
    let partial = false;
    let complete = false;
    let allRowsValid = true;
    for (let page = 0; page < MAX_TEAM_PAGES && teams.length < LINEAR_MAX_TEAMS; page++) {
      const response = await this.graphql(apiKey, TEAMS_QUERY, { after }, 'read', signal);
      const container = asObject(response.data?.teams);
      if (!container || !Array.isArray(container.nodes)) {
        if (teams.length > 0) { partial = true; break; }
        throw this.fallbackError(response.errors, 'read', response.headers)
          ?? new LinearError('offline', 'Linear returned an unexpected teams response.');
      }
      if (response.errors.length > 0) partial = true;
      const countBefore = teams.length;
      for (const rawTeam of container.nodes) {
        if (teams.length >= LINEAR_MAX_TEAMS) break;
        const obj = asObject(rawTeam);
        if (!obj) continue;
        const id = sanitize.text(obj.id, FIELD_LIMITS.id);
        const key = sanitize.text(obj.key, FIELD_LIMITS.key);
        const name = sanitize.text(obj.name, FIELD_LIMITS.name);
        if (!id || !key || seenTeamIds.has(id)) continue;
        seenTeamIds.add(id);
        const statesObj = asObject(obj.states);
        const states = asArray(statesObj?.nodes)
          .slice(0, LINEAR_MAX_STATES)
          .map(item => parseState(item, sanitize))
          .filter((state): state is LinearWorkflowState => state !== null)
          .sort((a, b) => a.position - b.position || a.name.localeCompare(b.name));
        const statesComplete = Array.isArray(statesObj?.nodes) && asObject(statesObj?.pageInfo)?.hasNextPage === false
          && states.length === asArray(statesObj?.nodes).length && !response.errors.length;
        teams.push({ id, key, name, states, states_complete: statesComplete });
      }
      const info = parsePageInfo(container.pageInfo, sanitize);
      allRowsValid = allRowsValid && teams.length - countBefore === container.nodes.length;
      complete = !partial && allRowsValid && asObject(container.pageInfo)?.hasNextPage === false;
      if (!info.hasMore || !info.nextCursor || seenCursors.has(info.nextCursor)) break;
      seenCursors.add(info.nextCursor);
      after = info.nextCursor;
    }
    return { data: teams, complete, partial, warning: partial ? PARTIAL_DATA_WARNING : null };
  }

  /** Lists one page of up to 50 issue summaries. Never requests descriptions or comments. */
  async listIssues(
    apiKey: string,
    params: { teamId: string; filter: LinearFilterPreset; search?: string; cursor?: string },
    signal?: AbortSignal,
  ): Promise<LinearQueryResult<{ issues: LinearIssueSummary[]; has_more: boolean; next_cursor: string | null }>> {
    const sanitize = makeSanitizer(apiKey);
    const filter = buildIssueFilter(params.teamId, params.filter, params.search);
    const response = await this.graphql(apiKey, ISSUES_QUERY, { after: params.cursor ?? null, filter }, 'read', signal);
    const container = asObject(response.data?.issues);
    if (!container || !Array.isArray(container.nodes)) {
      throw this.fallbackError(response.errors, 'read', response.headers)
        ?? new LinearError('offline', 'Linear returned an unexpected issue list response.');
    }
    const issues = container.nodes
      .slice(0, LINEAR_PAGE_SIZE)
      .map(item => parseIssueSummary(item, sanitize))
      .filter((issue): issue is LinearIssueSummary => issue !== null);
    const info = parsePageInfo(container.pageInfo, sanitize);
    const partial = response.errors.length > 0;
    return {
      data: { issues, has_more: info.hasMore, next_cursor: info.nextCursor },
      partial,
      warning: partial ? PARTIAL_DATA_WARNING : null,
    };
  }

  /** Fetches one issue's detail, attachments metadata (never fetched), and one page of comments. */
  async getIssue(
    apiKey: string,
    params: { issueId: string; commentsCursor?: string },
    signal?: AbortSignal,
  ): Promise<LinearQueryResult<LinearIssueDetail | null>> {
    const sanitize = makeSanitizer(apiKey);
    const response = await this.graphql(
      apiKey,
      ISSUE_DETAIL_QUERY,
      { id: params.issueId, commentsAfter: params.commentsCursor ?? null },
      'read',
      signal,
    );
    if (!response.data || !Object.hasOwn(response.data, 'issue')) {
      throw this.fallbackError(response.errors, 'read', response.headers)
        ?? new LinearError('offline', 'Linear returned an unexpected issue response.');
    }
    if (response.data.issue === null) {
      const classified = classifyGraphQlErrors(response.errors, 'read', response.headers, this.now());
      if (classified) throw classified;
      return { data: null, partial: response.errors.length > 0, warning: response.errors.length > 0 ? PARTIAL_DATA_WARNING : null };
    }
    const summary = parseIssueSummary(response.data.issue, sanitize);
    if (!summary) {
      throw this.fallbackError(response.errors, 'read', response.headers)
        ?? new LinearError('offline', 'Linear returned an unreadable issue.');
    }
    const raw = response.data.issue as JsonObject;
    const attachmentsObj = asObject(raw.attachments);
    const commentsObj = asObject(raw.comments);
    const attachments = asArray(attachmentsObj?.nodes)
      .slice(0, LINEAR_MAX_ATTACHMENTS)
      .map(item => parseAttachment(item, sanitize))
      .filter((item): item is LinearAttachment => item !== null);
    const comments = asArray(commentsObj?.nodes)
      .slice(0, LINEAR_COMMENTS_PAGE_SIZE)
      .map(item => parseComment(item, sanitize))
      .filter((item): item is LinearComment => item !== null);
    const commentsPage = parsePageInfo(commentsObj?.pageInfo, sanitize);
    const partial = response.errors.length > 0;
    return {
      data: {
        ...summary,
        description: sanitize.nullableText(raw.description, FIELD_LIMITS.description),
        attachments,
        comments,
        comments_has_more: commentsPage.hasMore,
        comments_next_cursor: commentsPage.nextCursor,
      },
      partial,
      warning: partial ? PARTIAL_DATA_WARNING : null,
    };
  }

  /** Posts one comment on an issue. Only called on an explicit person action. */
  async createComment(apiKey: string, issueId: string, body: string, commentId?: string, signal?: AbortSignal): Promise<{ comment_id: string }> {
    const sanitize = makeSanitizer(apiKey);
    const response = await this.graphql(apiKey, COMMENT_CREATE_MUTATION, { issueId, body, commentId: commentId ?? null }, 'write', signal);
    const payload = asObject(response.data?.commentCreate);
    const comment = asObject(payload?.comment);
    const returnedId = sanitize.text(comment?.id, FIELD_LIMITS.id) || commentId;
    if (payload?.success === true && returnedId) return { comment_id: returnedId };
    if (payload?.success === false) {
      throw rejected(this.fallbackError(response.errors, 'write', response.headers)
        ?? new LinearError('invalid_request', 'Linear did not accept the comment.'));
    }
    throw this.fallbackError(response.errors, 'write', response.headers)
      ?? new LinearError('offline', 'Linear returned an unreadable write result.');
  }

  /** Moves an issue to another workflow state. Only called on an explicit person action. */
  async updateIssueState(apiKey: string, issueId: string, stateId: string, signal?: AbortSignal): Promise<{ issue_id: string }> {
    const sanitize = makeSanitizer(apiKey);
    const response = await this.graphql(apiKey, ISSUE_UPDATE_STATE_MUTATION, { id: issueId, stateId }, 'write', signal);
    const payload = asObject(response.data?.issueUpdate);
    const issue = asObject(payload?.issue);
    const updatedId = sanitize.text(issue?.id, FIELD_LIMITS.id);
    if (payload?.success === true) return { issue_id: updatedId || issueId };
    if (payload?.success === false) {
      throw rejected(this.fallbackError(response.errors, 'write', response.headers)
        ?? new LinearError('invalid_request', 'Linear did not accept the state change.'));
    }
    throw this.fallbackError(response.errors, 'write', response.headers)
      ?? new LinearError('offline', 'Linear returned an unreadable write result.');
  }

  /** Creates an issue with a caller-chosen id. Only called on an explicit person action. */
  async createIssue(apiKey: string, input: {
    id: string; teamId: string; title: string; description: string | null; stateId: string | null;
  }, signal?: AbortSignal): Promise<{ issue_id: string; identifier: string | null }> {
    const sanitize = makeSanitizer(apiKey);
    const response = await this.graphql(apiKey, ISSUE_CREATE_MUTATION, { ...input }, 'write', signal);
    const payload = asObject(response.data?.issueCreate);
    const issue = asObject(payload?.issue);
    if (payload?.success === true) {
      return {
        issue_id: sanitize.text(issue?.id, FIELD_LIMITS.id) || input.id,
        identifier: sanitize.text(issue?.identifier, FIELD_LIMITS.identifier) || null,
      };
    }
    if (payload?.success === false) {
      throw rejected(this.fallbackError(response.errors, 'write', response.headers)
        ?? new LinearError('invalid_request', 'Linear did not create the issue.'));
    }
    throw this.fallbackError(response.errors, 'write', response.headers)
      ?? new LinearError('offline', 'Linear returned an unreadable write result.');
  }

  /**
   * An issue's title and description exactly as Linear stores them, for an
   * edit that must not lose text. Display reads bound and escape these; this
   * one does not, so it returns them only when they fit what Plait can write
   * back and carry no secret. Otherwise `editable` is false and both are null.
   */
  async getIssueText(apiKey: string, issueId: string, signal?: AbortSignal): Promise<{
    issue_id: string; updated_at: string; editable: boolean; title: string | null; description: string | null;
  } | null> {
    const sanitize = makeSanitizer(apiKey);
    const response = await this.graphql(apiKey, ISSUE_TEXT_QUERY, { id: issueId }, 'read', signal);
    const issue = asObject(response.data?.issue);
    if (!issue) return null;
    const id = sanitize.text(issue.id, FIELD_LIMITS.id);
    const updatedAt = sanitize.text(issue.updatedAt, FIELD_LIMITS.timestamp);
    if (!id || !updatedAt) throw new LinearError('offline', 'Linear returned an unreadable issue.');
    const title = typeof issue.title === 'string' ? issue.title : null;
    const description = issue.description === null || issue.description === undefined
      ? ''
      : typeof issue.description === 'string' ? issue.description : null;
    const editable = title !== null && description !== null
      && title.trim().length > 0 && title.length <= FIELD_LIMITS.title
      && !EDIT_TITLE_CONTROL.test(title)
      && Buffer.byteLength(description, 'utf8') <= LINEAR_DESCRIPTION_MAX_BYTES
      && !EDIT_DESCRIPTION_CONTROL.test(description)
      && !sanitize.containsSecret(title) && !sanitize.containsSecret(description);
    return { issue_id: id, updated_at: updatedAt, editable,
      title: editable ? title : null, description: editable ? description : null };
  }

  /** Changes an issue's title or description. Only called on an explicit person action. */
  async updateIssueText(apiKey: string, issueId: string, text: { title?: string; description?: string },
    signal?: AbortSignal): Promise<{ issue_id: string }> {
    const sanitize = makeSanitizer(apiKey);
    const variables: JsonObject = { id: issueId };
    if (text.title !== undefined) variables.title = text.title;
    if (text.description !== undefined) variables.description = text.description;
    const response = await this.graphql(apiKey, ISSUE_UPDATE_TEXT_MUTATION, variables, 'write', signal);
    const payload = asObject(response.data?.issueUpdate);
    const updatedId = sanitize.text(asObject(payload?.issue)?.id, FIELD_LIMITS.id);
    if (payload?.success === true) return { issue_id: updatedId || issueId };
    if (payload?.success === false) {
      throw rejected(this.fallbackError(response.errors, 'write', response.headers)
        ?? new LinearError('invalid_request', 'Linear did not accept the edit.'));
    }
    throw this.fallbackError(response.errors, 'write', response.headers)
      ?? new LinearError('offline', 'Linear returned an unreadable write result.');
  }

  /**
   * Reads one file from Linear's storage with the key, as Linear documents.
   * A redirect is followed once, without the key, so it never leaves Linear.
   */
  async readImage(apiKey: string, url: URL, signal?: AbortSignal): Promise<Buffer> {
    const timeout = AbortSignal.timeout(this.timeoutMs);
    const combined = signal ? AbortSignal.any([timeout, signal]) : timeout;
    const get = async (target: URL, authorized: boolean) => {
      try {
        return await fetch(target, {
          headers: authorized ? { authorization: apiKey } : {},
          redirect: 'manual',
          signal: combined,
        });
      } catch (error) {
        if (timeout.aborted || (error as Error | undefined)?.name === 'TimeoutError') {
          throw new LinearError('offline', 'Linear image request timed out.');
        }
        throw new LinearError('offline', 'Linear could not be reached.');
      }
    };
    const first = this.imageOrigin ? new URL(url.pathname + url.search, this.imageOrigin) : url;
    let response = await get(first, true);
    if (REDIRECT_STATUSES.has(response.status)) {
      const location = response.headers.get('location');
      await response.body?.cancel().catch(() => {});
      const next = location && URL.canParse(location, first) ? new URL(location, first) : null;
      // The same allowlist as the first request, checked before connecting:
      // Linear's storage host over https on the default port. Tests use their own origin.
      const local = this.imageOrigin !== undefined && next?.origin === new URL(this.imageOrigin).origin;
      const allowed = next !== null && !next.username && !next.password && (local
        || (next.protocol === 'https:' && next.hostname === LINEAR_IMAGE_HOST && next.port === ''));
      if (!allowed) {
        throw new LinearError('offline', 'Linear redirected the image away from its file storage.');
      }
      response = await get(next, false);
      if (REDIRECT_STATUSES.has(response.status)) {
        await response.body?.cancel().catch(() => {});
        throw new LinearError('offline', 'Linear redirected the image more than once.');
      }
    }
    if (response.status === 429) {
      await response.body?.cancel().catch(() => {});
      const seconds = parseRetryAfterSeconds(response.headers, this.now());
      throw new LinearError('rate_limited', `Linear rate limit reached. Retry in ${seconds} seconds.`, seconds);
    }
    // One file the key cannot read says nothing about the key, so this never
    // marks the sign-in rejected.
    if (response.status === 401 || response.status === 403 || response.status === 404) {
      await response.body?.cancel().catch(() => {});
      throw new LinearError('not_found', 'Linear has no image this key can read at this address.');
    }
    if (response.status !== 200) {
      await response.body?.cancel().catch(() => {});
      throw new LinearError('offline', 'Linear could not be reached.');
    }
    return this.readBoundedBytes(response, timeout, LINEAR_IMAGE_MAX_BYTES);
  }

  private fallbackError(errors: GraphQlError[], mode: AccessMode, headers: Headers): LinearError | null {
    if (!errors.length) return null;
    return classifyGraphQlErrors(errors, mode, headers, this.now())
      ?? new LinearError('invalid_request', 'Linear rejected the request.');
  }

  private async graphql(
    apiKey: string,
    query: string,
    variables: JsonObject,
    mode: AccessMode,
    callerSignal?: AbortSignal,
  ): Promise<{ data: JsonObject | null; errors: GraphQlError[]; headers: Headers }> {
    const timeout = AbortSignal.timeout(this.timeoutMs);
    const signal = callerSignal ? AbortSignal.any([timeout, callerSignal]) : timeout;
    let response: Response;
    try {
      // Redirects are refused so the API key can never be forwarded to another host.
      // Personal/restricted Linear API keys use `Authorization: <key>` with no `Bearer` prefix.
      response = await fetch(this.endpoint, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'accept': 'application/json',
          'authorization': apiKey,
        },
        body: JSON.stringify({ query, variables }),
        redirect: 'error',
        signal,
      });
    } catch (error) {
      if (timeout.aborted || (error as Error | undefined)?.name === 'TimeoutError') {
        throw new LinearError('offline', 'Linear request timed out.');
      }
      throw new LinearError('offline', 'Linear could not be reached.');
    }

    if (response.status === 429) {
      await response.body?.cancel().catch(() => {});
      const seconds = parseRetryAfterSeconds(response.headers, this.now());
      throw rejected(new LinearError('rate_limited', `Linear rate limit reached. Retry in ${seconds} seconds.`, seconds));
    }
    if (response.status === 401) {
      await response.body?.cancel().catch(() => {});
      throw rejected(new LinearError('key_rejected', 'Linear rejected the API key. Check or replace it in Settings.'));
    }
    if (response.status === 403) {
      await response.body?.cancel().catch(() => {});
      throw rejected(mode === 'write'
        ? new LinearError('read_only_key', 'This Linear API key does not allow writes. Use a key with write access.')
        : new LinearError('key_rejected', 'Linear rejected the API key. Check or replace it in Settings.'));
    }
    if (response.status >= 500 || (response.status !== 200 && response.status !== 400)) {
      await response.body?.cancel().catch(() => {});
      throw new LinearError('offline', 'Linear could not be reached.');
    }

    const bodyText = await this.readBoundedBody(response, timeout);
    let parsed: unknown;
    try {
      parsed = JSON.parse(bodyText);
    } catch {
      throw new LinearError('offline', 'Linear returned an unreadable response.');
    }
    const envelope = asObject(parsed);
    if (!envelope) throw new LinearError('offline', 'Linear returned an unreadable response.');
    const errors = asArray(envelope.errors).filter((item): item is GraphQlError => asObject(item) !== null);
    const data = asObject(envelope.data);

    // A mutation payload may prove acceptance despite errors in its selected fields.
    if (errors.length > 0) {
      const classified = classifyGraphQlErrors(errors, mode, response.headers, this.now());
      if (mode === 'read' && classified && classified.kind !== 'invalid_request') throw classified;
      if (!data) {
        if (classified) throw rejected(classified);
        throw new LinearError('invalid_request', 'Linear rejected the request.');
      }
    }
    if (response.status === 400) {
      throw classifyGraphQlErrors(errors, mode, response.headers, this.now())
        ?? new LinearError('invalid_request', 'Linear rejected the request.');
    }
    return { data, errors, headers: response.headers };
  }

  private async readBoundedBody(response: Response, timeout: AbortSignal): Promise<string> {
    return (await this.readBoundedBytes(response, timeout)).toString('utf8');
  }

  private async readBoundedBytes(response: Response, timeout: AbortSignal, limit = this.maxResponseBytes): Promise<Buffer> {
    const declared = response.headers.get('content-length');
    if (declared && /^\d+$/.test(declared.trim()) && Number(declared.trim()) > limit) {
      await response.body?.cancel().catch(() => {});
      throw new LinearError('offline', 'Linear response exceeded the size limit.');
    }
    if (!response.body) return Buffer.alloc(0);
    const chunks: Uint8Array[] = [];
    let total = 0;
    try {
      for await (const chunk of response.body) {
        total += chunk.byteLength;
        if (total > limit) {
          await response.body.cancel().catch(() => {});
          throw new LinearError('offline', 'Linear response exceeded the size limit.');
        }
        chunks.push(chunk);
      }
    } catch (error) {
      if (error instanceof LinearError) throw error;
      if (timeout.aborted || (error as Error | undefined)?.name === 'TimeoutError') {
        throw new LinearError('offline', 'Linear request timed out.');
      }
      throw new LinearError('offline', 'Linear could not be reached.');
    }
    return Buffer.concat(chunks);
  }
}

function rejected(error: LinearError): LinearError {
  error.definitelyRejected = true;
  return error;
}
