import { createServer } from 'node:http';
import { gzipSync } from 'node:zlib';
import type { IncomingMessage, Server, ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';

export type FakeState = { id: string; name: string; type: string; position: number };
export type FakeTeam = { id: string; key: string; name: string; states: FakeState[] };
export type FakeComment = { id: string; body: string; createdAt: string; updatedAt: string; user: { name: string } | null };
export type FakeAttachment = { id: string; title: string; url: string };
export type FakeIssue = {
  id: string;
  identifier: string;
  number: number;
  title: string;
  url: string;
  priority: number;
  updatedAt: string;
  description: string | null;
  teamId: string;
  stateId: string;
  assigneeName: string | null;
  assignedToMe?: boolean;
  labels: { id: string; name: string }[];
  attachments: FakeAttachment[];
  comments: FakeComment[];
};

export type RecordedRequest = {
  method: string;
  path: string;
  authorization: string | undefined;
  query: string;
  variables: Record<string, unknown>;
};

export type FaultHandler = (
  request: IncomingMessage,
  response: ServerResponse,
  parsed: { query: string; variables: Record<string, unknown> },
) => boolean | Promise<boolean>;

/** A 1x1 PNG: the smallest image the host's describe() accepts. */
export const TINY_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);

export class FakeLinearServer {
  readonly server: Server;
  url = '';
  validKey = 'lin_api_TEST_GOOD_KEY_SECRET_123456';
  readOnlyKey = 'lin_api_TEST_READONLY_KEY_SECRET_789';
  readOnlyMode: 'http403' | 'graphql_forbidden' = 'graphql_forbidden';
  viewer = { id: 'viewer-1', name: 'Alex Tester' };
  organization = { id: 'org-1', name: 'Acme Workspace', urlKey: 'acme' };
  teams: FakeTeam[] = [];
  issues: FakeIssue[] = [];
  requests: RecordedRequest[] = [];
  redirectTargetHits = 0;
  attachmentHits = 0;
  dropNextMutationReply = false;
  /** Authorization sent with each image read, in order; a redirect's second read must carry none. */
  imageAuthorizations: (string | undefined)[] = [];
  /** Where /org-1/redirect-away sends the reader: another origin, which must get nothing. */
  awayOrigin = '';
  /** Bytes served at /org-1/<name>, as Linear's storage serves an organization's files. */
  images = new Map<string, Buffer>([['png', TINY_PNG]]);
  private faults: FaultHandler[] = [];
  private commentSeq = 1;

  constructor() {
    this.seedDefaults();
    this.server = createServer((req, res) => { void this.handle(req, res); });
  }

  seedDefaults() {
    this.teams = [
      {
        id: 'team-pla',
        key: 'PLA',
        name: 'Plait',
        states: [
          { id: 'state-triage', name: 'Triage', type: 'triage', position: 0 },
          { id: 'state-backlog', name: 'Backlog', type: 'backlog', position: 1 },
          { id: 'state-todo', name: 'Todo', type: 'unstarted', position: 2 },
          { id: 'state-progress', name: 'In Progress', type: 'started', position: 3 },
          { id: 'state-done', name: 'Done', type: 'completed', position: 4 },
        ],
      },
      {
        id: 'team-ops',
        key: 'OPS',
        name: 'Operations',
        states: [
          { id: 'state-ops-todo', name: 'Queued', type: 'unstarted', position: 0 },
          { id: 'state-ops-done', name: 'Finished', type: 'completed', position: 1 },
        ],
      },
    ];
    this.issues = [
      {
        id: 'issue-142',
        identifier: 'PLA-142',
        number: 142,
        title: 'Fix retry backoff on 429s',
        url: 'https://linear.app/acme/issue/PLA-142/fix-retry-backoff',
        priority: 2,
        updatedAt: '2026-11-18T12:00:00.000Z',
        description: 'Honor Retry-After and reset headers when rate limited.',
        teamId: 'team-pla',
        stateId: 'state-progress',
        assigneeName: 'Alex Tester',
        assignedToMe: true,
        labels: [{ id: 'lbl-1', name: 'host' }],
        attachments: [{ id: 'att-1', title: 'Trace', url: 'https://example.invalid/attachment-never-fetch' }],
        comments: [
          { id: 'cmt-1', body: 'First comment', createdAt: '2026-11-18T11:00:00.000Z', updatedAt: '2026-11-18T11:00:00.000Z', user: { name: 'Alex Tester' } },
        ],
      },
      {
        id: 'issue-143',
        identifier: 'PLA-143',
        number: 143,
        title: 'Backlog item',
        url: 'https://linear.app/acme/issue/PLA-143/backlog-item',
        priority: 3,
        updatedAt: '2026-11-17T12:00:00.000Z',
        description: 'Backlog description',
        teamId: 'team-pla',
        stateId: 'state-backlog',
        assigneeName: null,
        assignedToMe: false,
        labels: [],
        attachments: [],
        comments: [],
      },
    ];
  }

  async start(): Promise<string> {
    await new Promise<void>(resolve => this.server.listen(0, '127.0.0.1', resolve));
    const port = (this.server.address() as AddressInfo).port;
    this.url = `http://127.0.0.1:${port}/graphql`;
    return this.url;
  }

  async close(): Promise<void> {
    this.server.closeAllConnections();
    await new Promise<void>(resolve => this.server.close(() => resolve()));
  }

  /** Queues a one-shot fault handler for the next GraphQL request. */
  enqueueFault(handler: FaultHandler) {
    this.faults.push(handler);
  }

  private stateById(stateId: string): FakeState | undefined {
    for (const team of this.teams) {
      const found = team.states.find(state => state.id === stateId);
      if (found) return found;
    }
    return undefined;
  }

  private async handle(req: IncomingMessage, res: ServerResponse) {
    if (req.url?.startsWith('/redirect-target')) {
      this.redirectTargetHits++;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ data: { viewer: this.viewer, organization: this.organization } }));
      return;
    }
    if (req.url?.startsWith('/org-1/')) {
      this.serveImage(req, res);
      return;
    }
    if (req.url?.startsWith('/attachment')) {
      this.attachmentHits++;
      res.writeHead(200, { 'content-type': 'text/plain' });
      res.end('should never be fetched');
      return;
    }
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    let parsed: { query: string; variables: Record<string, unknown> } = { query: '', variables: {} };
    try {
      const raw = JSON.parse(Buffer.concat(chunks).toString('utf8')) as { query?: string; variables?: Record<string, unknown> };
      parsed = { query: raw.query ?? '', variables: raw.variables ?? {} };
    } catch {
      res.writeHead(400, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ errors: [{ message: 'Invalid JSON' }] }));
      return;
    }

    const authHeader = typeof req.headers.authorization === 'string' ? req.headers.authorization : undefined;
    this.requests.push({
      method: req.method ?? '',
      path: req.url ?? '',
      authorization: authHeader,
      query: parsed.query,
      variables: parsed.variables,
    });

    const fault = this.faults.shift();
    if (fault && await fault(req, res, parsed)) return;

    // Linear personal API keys do NOT accept Bearer prefix.
    if (!authHeader || (authHeader !== this.validKey && authHeader !== this.readOnlyKey)) {
      res.writeHead(401, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ errors: [{ message: `Invalid key ${authHeader ?? ''}`, extensions: { code: 'AUTHENTICATION_ERROR' } }] }));
      return;
    }

    const isMutation = parsed.query.trimStart().startsWith('mutation');
    if (isMutation && authHeader === this.readOnlyKey) {
      if (this.readOnlyMode === 'http403') {
        res.writeHead(403, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ errors: [{ message: 'Forbidden raw body SECRET_DIAGNOSTIC', extensions: { code: 'FORBIDDEN' } }] }));
      } else {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: null, errors: [{ message: 'Write forbidden RAW_BODY_SECRET', extensions: { code: 'FORBIDDEN' } }] }));
      }
      return;
    }

    if (parsed.query.includes('PlaitViewer')) {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ data: { viewer: this.viewer, organization: this.organization } }));
      return;
    }

    if (parsed.query.includes('PlaitTeams')) {
      const after = typeof parsed.variables.after === 'string' ? Number(parsed.variables.after) : 0;
      const slice = this.teams.slice(after, after + 50);
      const next = after + slice.length < this.teams.length ? String(after + slice.length) : null;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({
        data: {
          teams: {
            nodes: slice.map(team => ({ id: team.id, key: team.key, name: team.name, states: { nodes: team.states.slice(0, 50), pageInfo: { hasNextPage: team.states.length > 50, endCursor: team.states.length > 50 ? "50" : null } } })),
            pageInfo: { hasNextPage: next !== null, endCursor: next },
          },
        },
      }));
      return;
    }

    if (parsed.query.includes('PlaitIssues')) {
      const filter = (parsed.variables.filter ?? {}) as Record<string, any>;
      const teamId = filter.team?.id?.eq as string | undefined;
      const stateTypes = filter.state?.type?.in as string[] | undefined;
      const stateTypeEq = filter.state?.type?.eq as string | undefined;
      const isMe = filter.assignee?.isMe?.eq === true;
      const titleQuery = (filter.title?.containsIgnoreCase as string | undefined)?.toLowerCase();
      const orClauses = filter.or as Array<Record<string, any>> | undefined;
      const filtered = this.issues
        .filter(issue => {
          if (teamId && issue.teamId !== teamId) return false;
          const st = this.stateById(issue.stateId);
          if (stateTypes && (!st || !stateTypes.includes(st.type))) return false;
          if (stateTypeEq && st?.type !== stateTypeEq) return false;
          if (isMe && !issue.assignedToMe) return false;
          if (titleQuery && !issue.title.toLowerCase().includes(titleQuery)) return false;
          if (orClauses) {
            const matched = orClauses.some(clause => {
              if (clause.number?.eq !== undefined && issue.number === clause.number.eq) return true;
              if (clause.title?.containsIgnoreCase && issue.title.toLowerCase().includes(String(clause.title.containsIgnoreCase).toLowerCase())) return true;
              return false;
            });
            if (!matched) return false;
          }
          return true;
        })
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt) || b.number - a.number);
      const after = typeof parsed.variables.after === 'string' ? Number(parsed.variables.after) : 0;
      const page = filtered.slice(after, after + 50);
      const next = after + page.length < filtered.length ? String(after + page.length) : null;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({
        data: {
          issues: {
            nodes: page.map(issue => this.issueSummaryNode(issue)),
            pageInfo: { hasNextPage: next !== null, endCursor: next },
          },
        },
      }));
      return;
    }

    if (parsed.query.includes('PlaitIssueDetail')) {
      const issue = this.issues.find(item => item.id === parsed.variables.id || item.identifier === parsed.variables.id);
      if (!issue) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: { issue: null } }));
        return;
      }
      const after = typeof parsed.variables.commentsAfter === 'string' ? Number(parsed.variables.commentsAfter) : 0;
      const comments = issue.comments.slice(after, after + 25);
      const next = after + comments.length < issue.comments.length ? String(after + comments.length) : null;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({
        data: {
          issue: {
            ...this.issueSummaryNode(issue),
            description: issue.description,
            attachments: { nodes: issue.attachments },
            comments: { nodes: comments, pageInfo: { hasNextPage: next !== null, endCursor: next } },
          },
        },
      }));
      return;
    }

    if (parsed.query.includes('PlaitCommentCreate')) {
      const issue = this.issues.find(item => item.id === parsed.variables.issueId);
      if (!issue) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: { commentCreate: { success: false, comment: null } }, errors: [{ message: 'Entity not found', extensions: { code: 'INPUT_ERROR' } }] }));
        return;
      }
      const commentId = typeof parsed.variables.commentId === 'string' ? parsed.variables.commentId : `cmt-new-${this.commentSeq++}`;
      if (this.issues.some(item => item.comments.some(comment => comment.id === commentId))) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: { commentCreate: { success: false, comment: null } } }));
        return;
      }
      const comment: FakeComment = {
        id: commentId,
        body: String(parsed.variables.body),
        createdAt: '2026-11-18T13:00:00.000Z',
        updatedAt: '2026-11-18T13:00:00.000Z',
        user: { name: this.viewer.name },
      };
      issue.comments.push(comment);
      issue.updatedAt = comment.updatedAt;
      if (this.dropMutationReply(res)) return;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ data: { commentCreate: { success: true, comment: { id: comment.id } } } }));
      return;
    }

    if (parsed.query.includes('PlaitIssueText')) {
      const issue = this.issues.find(item => item.id === parsed.variables.id || item.identifier === parsed.variables.id);
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ data: { issue: issue
        ? { id: issue.id, title: issue.title, description: issue.description, updatedAt: issue.updatedAt }
        : null } }));
      return;
    }

    if (parsed.query.includes('PlaitIssueCreate')) {
      const team = this.teams.find(t => t.id === parsed.variables.teamId);
      const stateId = typeof parsed.variables.stateId === 'string' ? parsed.variables.stateId : team?.states[0]?.id;
      if (!team || !team.states.some(s => s.id === stateId)) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: { issueCreate: { success: false, issue: null } }, errors: [{ message: 'Invalid team', extensions: { code: 'INPUT_ERROR' } }] }));
        return;
      }
      const id = String(parsed.variables.id);
      if (!this.issues.some(item => item.id === id)) {
        const number = 200 + this.issues.length;
        this.issues.push({
          id, identifier: `${team.key}-${number}`, number, title: String(parsed.variables.title),
          url: `https://linear.app/acme/issue/${team.key}-${number}/new`, priority: 0,
          updatedAt: '2026-11-18T15:00:00.000Z',
          description: typeof parsed.variables.description === 'string' ? parsed.variables.description : null,
          teamId: team.id, stateId: stateId!, assigneeName: null, assignedToMe: false,
          labels: [], attachments: [], comments: [],
        });
      }
      const issue = this.issues.find(item => item.id === id)!;
      if (this.dropMutationReply(res)) return;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ data: { issueCreate: { success: true, issue: { id: issue.id, identifier: issue.identifier } } } }));
      return;
    }

    if (parsed.query.includes('PlaitIssueUpdateText')) {
      const issue = this.issues.find(item => item.id === parsed.variables.id);
      if (!issue) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: { issueUpdate: { success: false, issue: null } }, errors: [{ message: 'Entity not found', extensions: { code: 'INPUT_ERROR' } }] }));
        return;
      }
      if (typeof parsed.variables.title === 'string') issue.title = parsed.variables.title;
      if (typeof parsed.variables.description === 'string') issue.description = parsed.variables.description;
      issue.updatedAt = '2026-11-18T16:00:00.000Z';
      if (this.dropMutationReply(res)) return;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ data: { issueUpdate: { success: true, issue: { id: issue.id } } } }));
      return;
    }

    if (parsed.query.includes('PlaitIssueUpdateState')) {
      const issue = this.issues.find(item => item.id === parsed.variables.id);
      const stateId = String(parsed.variables.stateId);
      const team = issue ? this.teams.find(t => t.id === issue.teamId) : undefined;
      if (!issue || !team?.states.some(s => s.id === stateId)) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: { issueUpdate: { success: false, issue: null } }, errors: [{ message: 'Invalid state', extensions: { code: 'INPUT_ERROR' } }] }));
        return;
      }
      issue.stateId = stateId;
      issue.updatedAt = '2026-11-18T14:00:00.000Z';
      if (this.dropMutationReply(res)) return;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ data: { issueUpdate: { success: true, issue: { id: issue.id } } } }));
      return;
    }

    res.writeHead(400, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ errors: [{ message: 'Unknown query' }] }));
  }

  /** Linear's file storage: the key is required, and `redirect` sends the reader elsewhere once. */
  private serveImage(req: IncomingMessage, res: ServerResponse) {
    const auth = typeof req.headers.authorization === 'string' ? req.headers.authorization : undefined;
    this.imageAuthorizations.push(auth);
    const name = (req.url ?? '').slice('/org-1/'.length);
    if (name === 'redirect') {
      res.writeHead(302, { location: `${this.url.replace(/\/graphql$/, '')}/org-1/signed-png` });
      res.end();
      return;
    }
    if (name === 'redirect-away') {
      res.writeHead(302, { location: `${this.awayOrigin}/org-1/png` });
      res.end();
      return;
    }
    if (name === 'limited') {
      res.writeHead(429, { 'retry-after': '7' });
      res.end();
      return;
    }
    if (name === 'signed-png') {
      res.writeHead(200, { 'content-type': 'image/png' });
      res.end(TINY_PNG);
      return;
    }
    if (auth !== this.validKey) {
      res.writeHead(401);
      res.end();
      return;
    }
    if (name === 'streamed-huge') {
      // No content-length: the reader must stop on what arrives.
      res.writeHead(200, { 'content-type': 'image/png' });
      const chunk = Buffer.alloc(1024 * 1024);
      for (let i = 0; i < 6; i++) res.write(chunk);
      res.end();
      return;
    }
    if (name === 'gzip-bomb') {
      res.writeHead(200, { 'content-type': 'image/png', 'content-encoding': 'gzip' });
      res.end(gzipSync(Buffer.concat([TINY_PNG, Buffer.alloc(6 * 1024 * 1024)])));
      return;
    }
    const bytes = this.images.get(name);
    if (!bytes) {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, { 'content-type': 'application/octet-stream' });
    res.end(bytes);
  }

  private dropMutationReply(res: ServerResponse): boolean {
    if (!this.dropNextMutationReply) return false;
    this.dropNextMutationReply = false;
    res.destroy();
    return true;
  }

  private issueSummaryNode(issue: FakeIssue) {
    const state = this.stateById(issue.stateId) ?? { id: issue.stateId, name: 'Unknown', type: 'unstarted' };
    return {
      id: issue.id,
      identifier: issue.identifier,
      title: issue.title,
      url: issue.url,
      priority: issue.priority,
      updatedAt: issue.updatedAt,
      team: { id: issue.teamId },
      state: { id: state.id, name: state.name, type: state.type },
      assignee: issue.assigneeName ? { name: issue.assigneeName } : null,
      labels: { nodes: issue.labels },
    };
  }
}
