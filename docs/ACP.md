# Antigravity through ACP

**Agreed implementation contract for M7 step 3; not implemented yet.**
Antigravity is the first ACP harness in the [roadmap](ROADMAP.md). The scope
includes the prerequisite routing and transcript-store refactor below.
[HOST.md](HOST.md) continues to describe the implemented host protocol.

## Agreed decisions

1. Run the person's installed `agy acp`, with its own sign-in, settings, tools,
   skills, and MCP servers. Do not install or update Antigravity, read its
   credentials, inject instructions, or impersonate another editor.
2. Keep Antigravity's normal permission rules. Show each permission request it
   sends as a one-call Plait approval. Do not enable bypass or sandbox flags,
   or advertise Plait's file protection for this harness.
3. Use the stable ACP v1 API from the official TypeScript SDK. Separate ACP
   transport and translation from the Antigravity install and capability profile.
4. First ship durable conversations, streaming, approvals, Stop, model/thinking
   choices, and capability-checked images. Keep follow-ups; enable steering only
   after its acceptance and cancellation behavior are proved. Do not invent
   token totals from ACP context-size reports.
5. First extract explicit external-harness dispatch and the shared transcript
   store. Keep Pi's execution path and existing on-disk formats unchanged.
   This is a small prerequisite refactor, not a general agent plugin framework.

### Decisions for the prerequisite refactor

Recorded before the refactor starts; the step plan is on
[issue #31](https://github.com/heath0xFF/plait/issues/31).

6. **A saved harness this host does not know.** Read operations (history,
   watch, rename, archive) keep working on the thread's stored transcript.
   Every operation that would run, steer, configure, or queue work for it is
   refused before admission. Nothing falls back to Pi or another harness.
7. **A harness that cannot steer.** The host refuses a steering request with
   `invalid_request` before queueing or accepting it, including from older
   clients. The desktop reads `capabilities.steer` and does not offer
   Option-Enter steering for such a thread. The refusal lands with the
   refactor; the desktop change lands with the Antigravity adapter.
8. **A harness with a single permission mode.** The Permissions chip shows the
   mode as a fixed label with its explanation, not a menu, and
   `configure_thread` accepts only that mode.
9. **Run instructions.** The run header Plait builds today is passed only to
   adapters that ask for it (Codex and Claude Code keep their current
   behaviour). Antigravity receives none, as decision 1 requires.
10. **Host downgrade.** Downgrading to a host older than the refactor is not
    supported once a thread uses a new harness; the release notes say so. No
    store-level guard is added.
11. **Test seams.** The runtime keeps its `codex` and `claude` properties,
    backed by the new dispatch, so existing tests keep injecting fakes the
    same way during the refactor.

## Evidence and version pins

Antigravity's guide describes ACP **1**, over newline-delimited JSON-RPC on
stdin/stdout. It documents approvals, session configuration, steering, Stop,
and session load/resume. These are upstream claims, not Plait test results.

- **CLI candidate: 1.5.13.** Version and `acp --help` were checked. A handshake,
  session lifecycle, and permission exchange have not been tested. Do not yet
  report this as a supported minimum or tested integration.
- **Selected SDK candidate: `@agentclientprotocol/sdk` 2.8.0**, Apache-2.0, at revision
  `97d748e705a03be67ce4cb4c78ff0011fcca7592`. Its manifest, stable bindings,
  generated v1 types, and stream code were inspected. Use the stable entry
  point, not `/unstable` or experimental ACP v2.
- Pin the required Zod peer to **4.6.7**, already in the runtime lockfile, if
  the compatibility check passes. Do not install the optional MCP-server peer.
  Retain the SDK license in the packaged runtime notices.

Contract approval does not install dependencies. Before adoption, check the published
package against these sources and test with Plait's pinned Node and TypeScript.
Publish `supported.min` and `supported.tested` only after CLI acceptance. A newer
CLI must still pass protocol and capability checks; version alone is not proof.

## Ownership and prerequisite refactor

The Rust host owns process supervision and checkout exclusion. The TypeScript
runtime owns ACP, translated history, and run lifecycle. The desktop consumes
Plait's thread, tool, approval, and harness models, not ACP packets or credentials.

| Planned area | Responsibility |
| --- | --- |
| `runtime/src/external/` | Explicit adapter selection and the transcript store already shared by Codex and Claude |
| `runtime/src/acp/` | SDK connection, session lifecycle, permission translation, incremental transcript projection |
| `runtime/src/antigravity/` | Executable discovery, readiness, version checks, and verified model/thinking mappings |

Several runtime decisions currently mean “Codex, otherwise Claude”; execution
also has a Pi fallback. Replace these with exhaustive selection before adding a
harness. Unknown saved IDs must fail before admission or startup, never select
another agent. Extract the shared `CodexEntries` store without changing Codex
or Claude file names or table formats. No arbitrary ACP-server registry is added.

## Discovery and authentication

- Find `agy` in the host's login-shell environment, or use an absolute path
  chosen through `set_harness_path`. Execute it directly, not through shell
  text assembled from agent output.
- Reuse the harness readiness states. `initialize` alone does not establish
  `ready`: session access and a usable model choice must work too.
- Give discovery 15 seconds total, including environment loading. Cache results
  for 60 seconds. Explicit refresh or a path change invalidates the cache.
  Late results cannot replace those for a newer path or host connection.
- Model options are session data, not `initialize` fields. Discover them in a
  disposable host-owned directory and short-lived session, with no prompt.
  Close the session and process. Do not probe in a person's checkout.
  Antigravity may load global customizations or contact its service; discovery
  is not guaranteed to be side-effect-free or offline.
- Sign-in stays external initially: explain `agy` or `agy acp --login` in the
  person's terminal. Do not launch browser login automatically, copy credentials,
  or use Pi provider credentials. `set_up_harness` stays Claude-only.

## Negotiation and capability mapping

Identify the client as **Plait**, with its real version. Negotiate ACP 1 and
reject an unsupported answer. Register handlers before session requests so
updates that precede their response are not lost.

Do not advertise client filesystem, terminal, terminal-authentication, or
elicitation-form capabilities. Antigravity runs its own tools. Send no Plait
MCP servers, additional roots, or system prompt. Refuse unadvertised client
operations; never route them to Pi tools or a shell. If the CLI needs these
capabilities to work, stop and revisit the contract.

| ACP data or operation | Plait behavior |
| --- | --- |
| `session/new` | Create for the canonical checkout; save its ID before prompting |
| `session/resume` | Prefer when advertised; reopen only the saved session |
| `session/load` | Fallback when advertised; replay is not new history or tool effects |
| `session/prompt` | One accepted input starts one run; retain Plait request-ID deduplication |
| Message/thought chunks | Incremental text/reasoning with stable local IDs; no full transcript resend per chunk |
| `tool_call`, `tool_call_update` | Merge by tool-call ID; updates are partial, not replacements |
| `session/request_permission` | One live request becomes one Plait approval |
| `session/cancel` | Stop the prompt, settle permissions, then prove cleanup |
| Session configuration | Use offered option/value IDs; apply choices before prompting |
| Titles and plans | Bounded presentation data, not execution; preserve person-renamed titles |
| Usage | Context occupancy/cost are not input/output/cache totals; leave `usage` unavailable initially |
| Unknown optional metadata | Ignore within bounds; do not infer capabilities |
| Unknown required methods or invalid known payloads | Refuse; stop the run if its state cannot be trusted |

Offer only models returned for Plait. Google currently limits third-party models
to named editors; do not promise them or spoof an editor. Recheck the selected
model in the real checkout session before sending. If absent, return
`model_unavailable`, never substitute the default.

The guide names Low, Medium, High, Extra, and Max thinking levels. Map verified
option IDs to `low`, `medium`, `high`, `xhigh`, and `max`. Do not derive IDs from
labels or offer `off` and `minimal` without evidence. Unknown configuration
categories remain unavailable rather than guessed.

Images require negotiated image capability and must obey any model-specific
restriction. Keep PNG/JPEG, the existing 5 MiB combined input limit, and protocol-6
image storage. Reject unsupported images before sending; never silently drop
them. Do not fetch URLs or read paths just because a resource block names them.

## Permissions

Launch `agy acp` without `--dangerously-skip-permissions` or `--sandbox`. Offer
one launch policy, `agent_settings`, labelled **Antigravity settings**. It means
Plait supplies no override, not that every action asks. Antigravity rules can
allow or deny actions without a Plait request.

Use this protection statement:

> Antigravity's settings control its tools. Plait asks only when Antigravity
> requests permission. Plait's command sandbox and protection for files you
> had already changed do not apply.

ACP session/planning modes are not permission guarantees. Do not put them in
the Permissions control. Keep `reported` null unless the agent supplies verified
evidence of its effective permission policy; launch arguments do not prove it.

For each permission request:

1. Bind it to the connection generation, run, session, and tool call. Reject
   wrong-session requests. Show bounded, escaped action details and relevant
   dirty paths as a warning, not as enforced protection.
2. Approve only an offered `allow_once`, with its exact `optionId`. Deny with
   `reject_once`, or return `cancelled` if none exists. Never choose
   `allow_always` or `reject_always` on the person's behalf.
3. Refuse an ambiguous action, unsafe display payload, or request without a
   one-time allow choice. Do not turn malformed input into a generic Yes.
4. Serialize visible approvals and bound waiting requests. Stop, process loss,
   or host restart invalidates unanswered requests. A late answer cannot
   approve a replacement request or another run.

Closing the desktop leaves the host and pending approval alive. This adds no
persistent permission rule, native question form, or auto-approval service.

## Run lifetime, history, and recovery

Use one ACP process per active Antigravity thread/run, none for an idle thread.
Start through the existing admission, capacity, checkout lease, and before-record
path. Discovery is not a run and cannot send a prompt.

Store translated entries and the Antigravity session ID in
`<thread-id>.antigravity.sqlite`, using the extracted external transcript format.
Record the executable version with the run; keep the selected harness fixed.
History/search read local entries without starting Antigravity. Its session IDs
and files are not Plait thread IDs or stores.

Require a tested continuation path: advertised `session/resume`, otherwise
`session/load`. During load replay, do not append history again, mark tools as
new effects, or grant permissions. Missing or invalid saved sessions fail with
an explanation. Never create a replacement and resend without the person's action.

Follow-ups use Plait's durable queue. Initially set `capabilities.steer=false`
and refuse steering before acceptance, including requests from older clients.
Antigravity documents steering, but the inspected ACP schema has no general
steering capability flag. Enable it only with a verified profile that proves
input acceptance, prompt-response ownership, Stop behavior, and membership in
the same checkout record. Never silently turn “steer” into a new turn or resend
an uncertain input.

`end_turn` finishes normally. Cancellation becomes stopped/interrupted; refusal
and token/request limits are explicit non-success outcomes. Transport loss is
an error, not completion. Preserve partial text and mark unfinished tools
interrupted. Never retry automatically after an uncertain prompt write.

Stop sends `session/cancel` and cancels unanswered permissions. Allow ten seconds
for graceful termination, then use existing process-group termination and
cleanup-proof handling. Parent exit is not cleanup proof. Keep the checkout
cleanup marker when proof is missing; block later writers and Git changes.
Take the after record through the existing verified cleanup path. Undo still
cannot restore outside-checkout or external-service effects.

Host restart marks active work interrupted and never replays it. Reopening the
desktop reads that state; a later explicit submission can continue the session.
Stale updates from an old process cannot change the next run.

## Wire and storage compatibility

Add `antigravity` as a harness ID and provider through the extensible protocol-4
contract. No new socket protocol version or general ACP messages are proposed.
Protocols 1–3 keep their Pi-only view; versions 4–6 keep their shapes. Images
still require version 6.

Reuse `harnesses`, `set_harness_path`, `set_harness_mode`, `create_thread`, and
`configure_thread`. The only initial mode is `agent_settings`; no live policy
switch is offered. Store `harness_path:antigravity` as an existing-style setting.
The adapter supplies models, capabilities, readiness, and default mode.

Older desktops decode unknown IDs as “Another harness” and cannot choose them
for creation. Test existing-thread operations, saved drafts, and reconnect too:
none may substitute Pi or Claude. A new desktop must not send an Antigravity
choice to a host that does not list it. Old stores load without conversion.
Do not claim that pre-ACP hosts can safely operate new Antigravity stores: test
rollback and document or gate unsupported downgrade before release.

## Bounds and verification gates

Use SDK `ndJsonStream` with an explicit 8 MiB incoming frame limit and a separate
outgoing encoded-size check. Bound pending calls, waiting approvals, decoded
queues, and retained tool data; a frame limit alone does not bound a stream.
Reuse transcript/output limits and coalesced publication. Do not log raw packets,
environment, credentials, or provider error bodies. Return fixed errors with
safe recovery advice.

Required evidence before completion:

- **Refactor:** unchanged Pi/Codex/Claude admission, permissions, history, Stop,
  and recovery; unknown-ID refusal before any execution.
- **Scripted peer:** handshake failures, partial/oversized frames, wrong session
  IDs, partial/duplicate tool updates, unsupported callbacks, and stream bounds.
- **Lifecycle:** save-before-prompt, lost replies without resend, resume/load
  without duplicate history, missing sessions, queues, crash, Stop during
  startup/approval, and late callbacks.
- **Permissions:** one-time allow/deny, always-only options, concurrent requests,
  unsafe payloads, dirty-path warnings, and stale answers.
- **Compatibility:** protocols 1–6, old stores, old/new clients, unknown harnesses,
  downgrade handling, and image limits.
- **Installed CLI, no prompt:** readiness, authentication-required state, genuine
  model/config IDs, capabilities, and session create/resume/load/close. Use a
  disposable directory; import no private transcripts.
- **Opt-in model run:** with consent, verify streaming, an allowed and denied
  action, Stop with a running child, continuation after process exit, images,
  and host-restart interruption in a disposable repository.
- **Native UI and performance:** inspect Settings, composer, tools, and approvals;
  measure event overhead and idle-process cleanup against roadmap targets.

Fake-peer tests do not establish live Antigravity compatibility. No model call,
ACP session, permission exchange, or native Antigravity view has been tested here.

## Sources

- [Antigravity CLI ACP](https://antigravity.google/docs/cli/acp/).
- [Antigravity editor integration](https://antigravity.google/docs/ide/extensions/acp/):
  protocol, models, permissions, authentication, and sessions.
- [SDK manifest at the inspected revision](https://github.com/agentclientprotocol/typescript-sdk/blob/97d748e705a03be67ce4cb4c78ff0011fcca7592/package.json).
- [Stable bindings](https://github.com/agentclientprotocol/typescript-sdk/blob/97d748e705a03be67ce4cb4c78ff0011fcca7592/src/v1/stable/bindings.ts),
  [v1 types](https://github.com/agentclientprotocol/typescript-sdk/blob/97d748e705a03be67ce4cb4c78ff0011fcca7592/src/v1/schema/types.gen.ts),
  and [stream framing](https://github.com/agentclientprotocol/typescript-sdk/blob/97d748e705a03be67ce4cb4c78ff0011fcca7592/src/core/stream.ts).
