# Linear integration options

## Status

- **Shipped in the host (`runtime/src/linear/`).** Stage L1 implements the
  host contract documented in [HOST.md](HOST.md#linear): API-key storage in
  `linear-auth.json` (mode `0600`), a hardened GraphQL client locked to
  `https://api.linear.app/graphql`, additive host operations for auth, teams,
  project binding, bounded paged issue reads and SQLite caching with offline
  fallback, local thread-to-issue links, and two durably recorded person-initiated
  writes (post comment and change workflow state). Nothing is visible in the
  desktop app yet.
- **Rust client for the read and link operations (`plait-core::host::linear`).**
  Typed requests and validated replies for sign-in status, the API key, teams,
  project binding, issue lists and details, exact resolution, and thread links,
  with the key redacted from debug output and typed rejections (offline, rate
  limited with its wait, conflict, not found, rejected key, signed out, older
  host). The two writes and `linear_get_write` are not modelled yet. No desktop
  view calls the client yet.
- **Planned next.** A persistent project-level issue view adds search, ticket
  details, references in existing threads, and starting a thread from an issue.
  Settings holds the API key; a source-issue label links back to Linear.
  The view does not depend on issue-triggered runs. Those remain separate,
  with permissions and execution rules not yet agreed.
- **Unverified against Linear.** Nothing in this stage has been tried against
  Linear's live GraphQL API or MCP server. Every automated test runs against a
  local loopback fake server. An opt-in manual probe (`scripts/probe-linear.mjs`)
  is provided to test the GraphQL client against a real Linear key without
  running the full desktop.

## Write retry contract

The host now records intent before sending either write and exposes
`linear_info` and `linear_get_write` for capability discovery and local outcome
lookup. Outcomes distinguish not sent, rejected, accepted, and unknown. An
unknown write is never replayed; the person must inspect Linear. Accepted
receipts survive full-result eviction and credential changes in permanent,
bounded retry history. New writes stop at 10,000 identities instead of silently
forgetting old requests. Existing saved receipts migrate on store open; already
forgotten effects from older hosts cannot be reconstructed.

Linear's [public SDK schema](https://github.com/linear/linear/blob/2ca00918e3ae699a0aa1a6b07330b433397cd201/packages/sdk/src/schema.graphql)
documents an optional `CommentCreateInput.id` in UUID v4 format, with a
server-generated default. The host supplies a deterministic
UUID-shaped ID derived from workspace and request ID. This is additional
protection, not a reason to replay an unknown write. That schema's
`issueUpdate(id,input)` and `IssueUpdateInput` expose no conditional state or
revision guard. A state move with an uncertain result is never repeated.

Tests simulate dropped replies, crash windows by reopening stores, concurrent
requests, receipt eviction, and credential changes against the loopback fake
server. Nothing here has been tried against Linear itself, including duplicate
comment-ID refusal or mutation response/error semantics.

## Exact identity and local link history

`linear_resolve_issue` is the exact identifier/URL entry point for a bound
project. It checks the returned identifier, team, and workspace and caches
aliases for offline resolution; the existing list search remains broad.
The [GraphQL guide](https://linear.app/developers/graphql) demonstrates
`issue(id: "BLA-123")`; the published SDK schema explicitly accepts either a
UUID or an issue identifier. This documents support, while fake
server tests do not establish live behavior, including moved/renamed issues.

Links now carry a workspace ID and historical status. Old links keep null
workspace identity, sign-out keeps local history, and `linear_get_linked_issue`
refuses to combine a historical link with another workspace's current data.
Rate-limit replies include typed wait seconds. Teams and states expose nullable
completeness evidence, but remain capped at 50 each without a continuation API;
comments still have no declared order or total. These are additive host
contracts: there is still no desktop UI or automatic Linear activity.

## Using Linear's MCP server from an agent

An agent can still use Linear's official MCP server through Plait's existing
MCP support, though MCP is not the path Plait's own native Linear features
take (native features use the host GraphQL client above).

Linear provides an official Streamable HTTP MCP server at
`https://mcp.linear.app/mcp`. Its documentation supports browser OAuth and an
`Authorization: Bearer <token>` header with an API key or OAuth token. A key
with only the Read permission provides a read-only starting point. The `/sse`
endpoint is a deprecated fallback, not the choice for a new connection.

Plait's Pi runtime already accepts configured HTTP headers and makes each tool
call under the thread's approval mode in trusted projects. Sandboxed projects
require explicit server enablement and approval for every call. This makes a
restricted API key a candidate for using Linear without waiting for MCP browser
sign-in. Compatibility still needs a live check against the official server.
Do not mistake a saved Pi sign-in shown in Settings for working token reuse:
Plait's MCP OAuth connection and refresh support remain planned.

Keep credentials in the user's private configuration, never in a repository,
issue, prompt, or example committed to Git. Start read-only, verify the connected
workspace and team, then read one known issue. Add write access only after the
person chooses which operations are needed. Use Ask mode for the first write
checks; changing Linear issues has effects outside the checkout and cannot be
reversed by Plait's Undo.

This first step gives an agent issue context and, when permitted, issue tools.
It does not add a native issue list, a durable thread-to-issue link, automatic
status changes, or an unattended agent.

## Agreed native scope

The first desktop step is a **persistent project-level issue view**:

1. Associate a Plait project with a chosen Linear workspace, team, and optionally
   a Linear project. Reuse the host's existing binding where it applies.
2. Browse and search tickets, or resolve a pasted issue ID or URL. Show the
   identifier, title, state, source, and selected ticket's details.
3. Insert a reference and chosen context into an existing thread's draft, or
   fill a new thread draft. The person reviews it before sending; nothing runs
   automatically and an existing draft is not silently replaced.
4. Keep the source issue's stable identity and URL with a thread started from
   it. Offer Open in Linear. Text references to other tickets do not change
   that source link or introduce a multi-issue link schema.
5. Keep posting comments and changing issue state out of the first slice.
   Those can follow with explicit confirmation and clear failure handling.

Issue text is external task data, not permission to run tools or change the
project's policy. Linked files and images need separate bounded reads; do not
send credentials to arbitrary URLs in an issue.

### Ownership

- The host owns credentials, Linear requests, and durable issue links. The
  desktop receives only the data needed to show and select issues.
- Plain models hold issue identity and request state without GPUI. A focused
  integration module handles Linear's API; thread execution does not depend on
  its network availability.
- The desktop owns the issue view, draft confirmation, and navigation. It must
  discard stale replies after a project or query changes.
- Use explicit refresh and bounded, paged queries initially. Do not add a
  public webhook listener to the local host or poll each issue separately.

Linear's GraphQL API supports personal API keys and OAuth. For a distributed
native integration, investigate OAuth with PKCE rather than embedding a client
secret. The current docs support PKCE and token refresh. Confirm callback
registration and credential storage before choosing the flow. Handle GraphQL
errors even with HTTP 200, partial results, rate limits, revoked access, and
workspace changes.

This needs agreement on host operations, storage, and authentication before
implementation. No new SDK is selected; if one is used, pin its version and
check its source before using its APIs.

## Decisions before implementation

Agreed:

- **Order of native work.**
  1. Host foundation in `runtime/` (built; unverified against Linear).
  2. With thread tabs built, add the project-level issue view, API-key
     Settings, draft references, and source-issue labels described above.
  3. Issue-triggered runs follow only after their rules are agreed. They are
     not a prerequisite for the view.
- **Native path vs MCP.** Plait's native Linear features use the host's
  GraphQL client directly. Agents can still use Linear's official MCP server
  when configured, but MCP is not the path Plait's own features take.
- **Sign-in first step.** Start with a personal or restricted API key pasted in
  Settings and stored by the host in its own private file (mode `0600`), never
  in `host.sqlite` or any reply. Browser OAuth with PKCE comes later.
- **Explicit writes.** Plait may perform two writes against Linear, each only
  when the desktop asks on a person's action: posting a comment on an issue,
  and moving an issue to another workflow state in its team. Nothing writes
  automatically, and no agent, tool, or MCP server can reach these operations.
- **Thread links.** One local source-issue link per thread, stored in the host
  without writing to Linear.

Not yet agreed / still open:

- **Issue-triggered runs.** Separate from the issue view. Their rules are not
  agreed: which issues are eligible, what a run may do with
  nobody there, and where it stops. They also require rewording Plait's rule
  against agent orchestration, which has not been done.
- Browser OAuth client registration, PKCE callback handling, and token refresh
  for distributed builds.
- Which default workspace, team, and optional Linear project selectors to
  present in Settings and the issue view.
- Whether a thread will ever need more than one linked issue, and whether
  thread summaries should carry a linked-issue field in a future protocol
  version when the desktop stage lands.
- Live verification against Linear's GraphQL and MCP endpoints.

## Sources

Official documentation consulted for this proposal:

- [Linear MCP](https://linear.app/docs/mcp): endpoint, tools, header
  authentication, read-only keys, and separate workspace authentication.
- [GraphQL API](https://linear.app/developers/graphql): queries, issue IDs,
  authentication, partial errors, and update guidance. Note that a personal API
  key uses `Authorization: <key>` for GraphQL, unlike the MCP Bearer header.
- [OAuth authentication](https://linear.app/developers/oauth-2-0-authentication):
  PKCE, scopes, refresh, and revocation.

These sources establish supported interfaces, not end-to-end Plait verification.
