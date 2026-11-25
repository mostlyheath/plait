# Host contract (M2.5)

This is the version 1 contract plus the opt-in additions described in
[Protocol version 2](#protocol-version-2) and
[Protocol version 3](#protocol-version-3), [Protocol version 4](#protocol-version-4),
[Protocol version 5](#protocol-version-5),
[Protocol version 6](#protocol-version-6), and [Protocol version 7](#protocol-version-7). Changes require compatibility review.
All values below are JSON. The desktop has no dependency on the host crate and
does not own its process.

## Launch and recovery

Build `cargo build -p plait-host`; install runtime packages with
`cd runtime && npm ci --ignore-scripts`. Node 22.19 or newer is required for
source development. The [Mac host package](GUIDE.md#install-and-updates) includes pinned Node and
runtime dependencies; it does not need a development checkout or shell Node.

- `plait-host run [--store ABSOLUTE_PATH] [--runtime ABSOLUTE_PATH]`: foreground
  daemon. `--runtime` is the path to `runtime/src/main.ts`.
- `plait-host run-user [same options]`: installed per-user service entry. Resolve
  the home from the OS account, reject root, and set the sidecar home and working
  directory explicitly. Do not inherit an installer's home or store override.
- `plait-host start [same options]`: spawn an independent daemon and return when
  its socket answers, or report a startup error. Closing the client has no effect.
- `plait-host status [--store ABSOLUTE_PATH]`: print the status JSON reply.
- `plait-host version`: print local binary version/revision and protocol metadata.
  This is a CLI operation, not a new socket operation.
- Default store: `$PLAIT_HOST_STORE`, if set, otherwise `$HOME/.local/share/plait`.
  Socket: `STORE/host.sock`. Store overrides must be absolute.
- The store directory is mode 0700 and socket mode 0600. A lifetime OS writer
  lock on `STORE/host.lock` rejects a second host, including after a crash.
- The Rust host owns and monitors the Node sidecar. The optional Tailscale
  listener accepts authorized peers; provider sign-in may open a loopback
  callback. See [Remote hosts](REMOTE.md).
  Sidecar failure closes the host. Development restart is explicit through
  `start`; the installed Mac LaunchAgent restarts failed hosts. Neither route
  replays interrupted work. The packaged host uses adjacent immutable Node and
  runtime paths, not a build-machine source path. It removes inherited
  `NODE_OPTIONS` and `NODE_PATH` before starting its sidecar.
- Startup marks work without a surviving detached runner as `interrupted`. It does not run the model
  or replay commands. A new user submission resumes the saved conversation.
  Thread IDs, submitted request IDs, and exact model choice survive restart.

## Detached runners

Detached Pi runners are available with `PLAIT_DETACHED_RUNNERS=1` in the host
runtime environment. The default is off pending verification. When enabled, an
accepted Pi run can continue through a host or sidecar restart, including an
approval wait. The restarted host reconnects to a live runner or applies its
completion receipt; it does not replay the run. Other harnesses use the existing
in-process behavior. Recovery checks for existing runners even if the flag is
subsequently turned off. Host updates wait for active runs and live runner locks.

`STORE/runners` holds runner manifests, locks, logs, and completion receipts;
`STORE/r` holds their sockets. A detached runner writes command output to the
per-thread `STORE/<thread_id>.outputs.sqlite`; older output remains readable from
`host.sqlite`. The host remains the only writer of `host.sqlite`.

If a runner dies without a completion receipt, the host marks the thread
`interrupted`. The checkout can require cleanup confirmation before further
writes; inspect its effects before confirming. If runner events needed for
recovery have been lost, the host marks the run interrupted and stops the
runner rather than applying later events over incomplete state. Interrupted
commands and effects are not replayed.

## Transport

Owner-only Unix stream socket, UTF-8 newline-delimited JSON, protocol version 1
by default; versions 2–7 are opt-in per request.
Clients and the daemon verify connected-peer user identity with `getpeereid` on
macOS or `SO_PEERCRED` on Linux. Clients also check socket and parent ownership
and private modes before connecting. Linux supports trusted commands and trusted MCP servers, but not sandboxed execution.
One request and one reply per connection, then EOF, except `watch` and the
version 3 `watch_threads`. No greeting.
Request limit: 64 KiB including newline. Reply/event limit: 1 MiB including
newline. Absolute input deadline: 5 seconds; sidecar exchange deadline: 20 seconds. Slow watchers are disconnected rather
than accumulating an unbounded queue. Reconnect starts with the latest snapshot;
clients replace state, not append it. Unknown operations and malformed requests
return an error and close the connection. Unknown object fields are rejected.

Success: `{"ok":true,"result":VALUE}`.
Failure: `{"ok":false,"error":{"code":"invalid_request","message":"Safe human-readable detail"}}`.
Reply codes: `invalid_request`, `not_found`, `busy`, `conflict`, `model_unavailable`,
`permission_denied`, `internal`. Snapshot errors can also use `runtime_error`,
`interrupted`, and, in version 3 snapshots, `turn_limit`. Never send credentials, auth paths, or raw
provider error bodies. Errors are not accepted submissions.

## Operations

```json
{"op":"status"}
{"op":"set_default_model","provider":"anthropic","model_id":"MODEL"}
{"op":"list_threads","checkout":"/absolute/checkout"}
{"op":"create_thread","checkout":"/absolute/checkout","provider":"anthropic","model_id":"MODEL","request_id":"CLIENT_UUID"}
{"op":"submit","thread_id":"HOST_UUID","request_id":"CLIENT_UUID","text":"User message"}
{"op":"stop","thread_id":"HOST_UUID"}
{"op":"approve","thread_id":"HOST_UUID","approval_id":"HOST_UUID","allow":true}
{"op":"watch","thread_id":"HOST_UUID"}
```

Version 3 adds operations for history, thread management, a live thread list,
model changes, and queued messages; see [Protocol version 3](#protocol-version-3).

`status` result:
```json
{"protocol_version":1,"default_model":null,"models":[{"provider":"anthropic","model_id":"MODEL","name":"Display name","authenticated":true}]}
```

`default_model` is null or `{"provider":"anthropic","model_id":"MODEL"}`.
Catalog and auth status come from Pi ModelRuntime. No reply carries a credential;
[Provider sign-in](#provider-sign-in) names the requests that carry one to the host.
`set_default_model` result: `{"default_model":{"provider":"anthropic","model_id":"MODEL"}}`.
`list_threads` result: `{"threads":[{"id":"HOST_UUID","checkout":"/canonical/checkout","model":{"provider":"anthropic","model_id":"MODEL"},"status":"idle"}]}` (latest 100 summaries).
`create_thread` result: `{"thread":SNAPSHOT}`. It never starts model execution.
`submit` result: `{"thread_id":"HOST_UUID","request_id":"CLIENT_UUID","accepted":true}`.
`stop` and `approve` result: `{"thread":SNAPSHOT}`. Stop revokes permission and
signals cancellation immediately; the checkout lease remains held until cleanup.
A new submission can return `busy` while cleanup finishes, including an already
approved atomic edit. `idle` is published only after the checkout lease is free. Repeating an approval
after it has been used returns `not_found`; reconnect to inspect the current state.

Request IDs are nonempty ASCII identifiers (letters, digits, hyphen, underscore),
maximum 128 bytes. They are permanent deduplication keys. Repeating an ID with
identical input returns the original acceptance; changed input returns `conflict`.
A client must retain an uncertain submission's ID, including across reconnect.
Never retry an uncertain submission with a fresh ID. At most one active
run is permitted for a canonical checkout, including across different host stores.
There is no default active-checkout count limit. The optional positive integer
`PLAIT_MAX_ACTIVE_CHECKOUTS` sets an explicit host limit. Empty submissions are rejected.
A submission to a thread with an active run returns `busy`, unless a version 3
client asks to [queue it](#messages-during-a-run).

`watch` first returns `{"ok":true,"result":{"thread":SNAPSHOT}}`, then zero or
more `{"event":"snapshot","thread":SNAPSHOT}` lines. The connection stays open
until disconnect, failure, or host shutdown. A disconnect does not stop work.

Delivery is event-driven. The runtime signals changed thread IDs, and one
reserved name when any thread summary changed, through a
private inherited socket (`PLAIT_NOTIFY_FD`); the host fetches state only after
an invalidation. This internal channel is separate from request/reply stdio and
is not a client API. The host and runtime must be updated together, as packaged.
No snapshot or credential travels in a notification. Repeated changes coalesce;
a full ID set becomes an all-watch invalidation instead of an unbounded queue.
Each client has one pending wakeup, registers before its initial snapshot read,
and releases its subscription on disconnect. EOF or malformed notification
frames stop the host rather than leave clients silently stale. The public watch
frames, request deadlines, durable writes, and recovery rules are unchanged.

The desktop client waits for snapshots without a periodic read timeout. Its
initial watch reply has a 15-second deadline; after that, a partial frame must
finish within 15 seconds of its first byte. An idle, complete connection can
wait indefinitely. Explicit cancellation closes only that subscription on
selection, reconnect, or window release. A dedicated reader and one-slot async
inbox keep blocked sockets and snapshot backlogs out of GPUI's worker pool and
UI loop. Cancellation never submits a Stop operation.

## Snapshot

All keys below are always present. Nullable fields use JSON null.

```json
{"id":"HOST_UUID","checkout":"/canonical/checkout","model":{"provider":"anthropic","model_id":"MODEL"},"status":"idle","messages":[{"id":"STABLE_ID","role":"user","text":"Hello"}],"live_text":"","tools":[{"id":"STABLE_ID","name":"read","status":"done","input":"JSON argument preview","output":"Text output preview"}],"pending_approval":null,"error":null,"truncated":false}
```

Statuses: `idle`, `running`, `awaiting_approval`, `stopped`, `interrupted`, `error`.
Message roles: `user`, `assistant`. Tool statuses: `running`, `done`, `error`,
`interrupted`. `error` is null or `{code,message}` with the same safe error shape.
`pending_approval` is null or
`{"id":"HOST_UUID","tool_id":"STABLE_ID","kind":"edit","summary":"Change src/main.rs","details":"Full exact JSON tool arguments","dirty_paths":["notes.txt"],"risk":"Pre-existing dirty paths are protected."}`.
`details` is never clipped: tool arguments over 16384 bytes are rejected.
Approval kind is `edit` or `command`. Approval is bound to one tool call; it is
not blanket permission. Stop and restart revoke pending approvals, except
that a detached Pi runner (off by default, `PLAIT_DETACHED_RUNNERS=1`) that
survives a host restart keeps its approval wait.
Messages: latest 100, 4096 characters each. Tools: latest 50, 4096 characters
per input/output. Live text: latest 16384 characters. These text budgets count
UTF-16 code units; clipping keeps whole Unicode code points and does not increase
the budget. Command stdout and stderr each decode UTF-8 across pipe chunks.
The original accepted input remains in the private journal. The complete serialized
snapshot is also limited to 512 KiB; older content is removed first. Approval
arguments are never clipped. `truncated` flags clipping.

## Protocol version 2

Version 2 is negotiated per request, so desktops and hosts can update separately.
Every operation accepts an optional `"protocol_version":1`, `2`, `3`, `4`, `5`, `6`, or `7`; omitting it
means version 1, and version 1 replies are byte-for-byte unchanged.

- **Negotiation.** A client sends `{"op":"status","protocol_version":2}`. A
  version 2 host replies with `"protocol_version":2`. A version 1 host rejects the
  unknown field with `invalid_request`; the client then repeats plain `status` and
  uses version 1. Clients send version 2 fields only to a host that negotiated them.
- **Snapshots.** Requests with `"protocol_version":2` receive snapshots with two
  extra keys. Each tool has `after`: the ID of the message it followed, or null
  for a tool before any message; tools saved before version 2 omit it and are shown
  in the old order. Clients show tools after that message. A tool
  whose message was clipped from the preview is shown first. The snapshot has
  `approval_mode`: `"ask"` or `"auto"`, the mode of the latest accepted run.
- **Submit.** `submit` accepts `"approval":"ask"` or `"approval":"auto"`, only with
  `"protocol_version":2`. Omitted means `ask`. The mode applies to that run only;
  the next submission chooses again. Request-ID deduplication still compares only
  the text, so a retry returns the original acceptance and mode.

Version 1 clients that watch a thread started in `auto` mode still see version 1
snapshots, without the mode. Downgrading a host after version 2 snapshots were
saved is not supported.

## Protocol version 3

Version 3 is negotiated per request, exactly as version 2 is. Every operation
accepts `"protocol_version":3`. Version 1 and 2 replies are byte-for-byte
unchanged, and the operations and fields below do not exist for a version 1 or
2 request: they are rejected with `invalid_request`, `Unknown operation or field.`

- **Negotiation.** A client sends `{"op":"status","protocol_version":3}`. A
  version 3 host replies with `"protocol_version":3`. A version 2 host rejects the
  number and a version 1 host rejects the field, both with `invalid_request`; the
  client then asks for version 2, and finally sends plain `status`. Clients send
  version 3 operations and fields only to a host that negotiated them.
- **Versioned replies.** Every operation that returns `{"thread":SNAPSHOT}`
  (`create_thread`, `stop`, `approve`, `watch`, and the new ones) returns a
  version 3 snapshot when the request carries `"protocol_version":3`.

```json
{"op":"status","protocol_version":3}
{"op":"list_threads","protocol_version":3,"checkout":"/absolute/checkout","limit":100,"cursor":"CURSOR","archived":true}
{"op":"watch_threads","protocol_version":3,"archived":true}
{"op":"history","protocol_version":3,"thread_id":"HOST_UUID","before":"CURSOR","limit":100}
{"op":"history_text","protocol_version":3,"thread_id":"HOST_UUID","ref":"REF","offset":65536}
{"op":"rename_thread","protocol_version":3,"thread_id":"HOST_UUID","title":"New title"}
{"op":"archive_thread","protocol_version":3,"thread_id":"HOST_UUID","archived":true}
{"op":"create_thread","protocol_version":3,"checkout":"/absolute/checkout","provider":"anthropic","model_id":"MODEL","request_id":"CLIENT_UUID","thinking":"medium"}
{"op":"configure_thread","protocol_version":3,"thread_id":"HOST_UUID","provider":"anthropic","model_id":"MODEL","thinking":"high"}
{"op":"submit","protocol_version":3,"thread_id":"HOST_UUID","request_id":"CLIENT_UUID","text":"User message","approval":"ask","when_busy":"follow_up"}
{"op":"cancel_queued","protocol_version":3,"thread_id":"HOST_UUID","request_id":"CLIENT_UUID"}
```

In `list_threads`, `checkout`, `limit`, `cursor`, and `archived` are optional. In
`watch_threads`, `archived` is optional. In `history`, `before` and `limit` are
optional. In `create_thread`, `thinking` is optional: the thread starts at that
level, or the nearest one its model supports (a model without reasoning starts
at `off`); it is part of the request's deduplicated input. In
`configure_thread`, give `provider` with `model_id`, `thinking`, or all three,
and a level the model lacks is rejected. In `submit`, `approval` and `when_busy` are optional. Every other
field shown is required. Timestamps are Unix milliseconds.

### Snapshot and summary

A version 3 snapshot has every version 2 key, with these changes and additions:

```json
{"id":"HOST_UUID","checkout":"/canonical/checkout","model":{"provider":"anthropic","model_id":"MODEL"},"status":"running","messages":[{"id":"STABLE_ID","role":"assistant","text":"Answer","reasoning":"Reasoning text"}],"live_text":"","tools":[{"id":"STABLE_ID","name":"read","status":"done","input":"JSON argument preview","output":"Text output preview","after":"STABLE_ID"}],"pending_approval":null,"error":null,"truncated":false,"approval_mode":"ask","title":"Fix the parser","created_at":1763300000000,"updated_at":1763300060000,"archived":false,"thinking_level":"off","queued":[{"id":"request-CLIENT_UUID","request_id":"CLIENT_UUID","text":"Queued message","mode":"follow_up","approval":"ask"}],"usage":{"input":1200,"output":300,"cache_read":8000,"cache_write":0,"reasoning":120,"total":9500,"context_used":9500,"context_window":200000},"live_reasoning":"","retry":null}
```

- **Messages.** Each message has `reasoning`: the model's readable reasoning for
  that message, or `""`. An assistant message that reasoned and then only called
  tools has `"text":""` and nonempty `reasoning`; a tool's `after` can name it.
  Version 1 and 2 snapshots omit such messages and order those tools after the
  last message with text, as before. Redacted reasoning is never sent.
- **`title`.** `""` until the first accepted message, then that message with
  whitespace runs collapsed to one space, control characters removed, and at
  most 72 characters (Unicode scalar values). With optional [thread naming](#thread-naming)
  enabled, a background model call may replace this fallback with a short task
  summary. A renamed thread keeps its name. Threads saved before version 3 get
  a title from their first message once; enabling naming does not rename them.
- **`created_at`, `updated_at`.** `updated_at` advances when the thread's status
  changes, which includes every accepted run. Streaming text, rename, and archive
  do not advance it. Threads saved before version 3 report the time the store
  was first opened by a version 3 host for both.
- **`thinking_level`.** One of `off`, `minimal`, `low`, `medium`, `high`, `xhigh`,
  `max`. The level `create_thread` asked for, or `off`, until changed with
  `configure_thread`.
- **`queued`.** Inputs waiting behind the active run, oldest first. `id` is the
  message ID the input takes when it joins the conversation. `mode` is
  `follow_up` or `steer`. See [Messages during a run](#messages-during-a-run).
- **`usage`.** `null` before the first model response, then the token counts of
  the latest response that did not fail. `reasoning` is `null` when the provider
  does not report it. `context_used` is that response's total, which is what the
  next request carries; `context_window` is the model's window from the catalog,
  or `null` for a model no longer listed. There are no cost figures.
- **`live_reasoning`.** Reasoning text of the response being generated, as
  `live_text` is for its answer. Empty when idle.
- **`retry`.** `null`, or `{"attempt":1,"at":1763300065000}` while the host waits
  to repeat a failed model request. `at` is when the next attempt starts. The
  provider's error text is not sent.
- **`error`.** A run that reaches its turn limit ends with code `turn_limit`.
  Version 1 and 2 snapshots report the same event as `runtime_error`.

Bounds: latest 100 messages with text, as before, plus the latest 50
reasoning-only messages; `reasoning` keeps its last 4096 characters per message;
`live_reasoning` its last 16384; each queued `text` its last 4096. These count
UTF-16 code units like the other budgets. Clipping reasoning does not set
`truncated`. A whole version 3 snapshot is at most 768 KiB: the content shared
with version 2 is bounded to 512 KiB exactly as before, then the oldest
reasoning is dropped first. The snapshot stays the live tail; use `history` for
complete text.

A version 3 thread summary, in `list_threads` and `watch_threads`:

```json
{"id":"HOST_UUID","checkout":"/canonical/checkout","model":{"provider":"anthropic","model_id":"MODEL"},"status":"idle","title":"Fix the parser","created_at":1763300000000,"updated_at":1763300060000,"archived":false,"pending_approval":false}
```

`pending_approval` is true while the thread waits for an approval answer.

### Catalog

`status` result, version 3:

```json
{"protocol_version":3,"default_model":null,"models":[{"provider":"anthropic","model_id":"MODEL","name":"Display name","authenticated":true,"reasoning":true,"thinking_levels":["off","minimal","low","medium","high"],"context_window":200000,"input":["text","image"]}]}
```

`reasoning` says whether the model can reason. `thinking_levels` lists the levels
`configure_thread` accepts for it; a model without reasoning lists only `off`.
`context_window` is in tokens. `input` lists `text` and, where supported,
`image`; this host does not accept image input yet. All four come from Pi's
catalog.

### Full history

`history` returns a thread's complete conversation, newest page first:

```json
{"items":[{"type":"message","id":"STABLE_ID","role":"user","text":"Full text","reasoning":""},{"type":"tool","id":"STABLE_ID","name":"read","status":"done","input":"Full JSON arguments","output":"Full output"}],"before":"CURSOR"}
```

- Items are in conversation order, oldest first within a page. A tool item sits
  where its call was made; there is no `after`. IDs are the snapshot's IDs, so a
  client can replace a clipped snapshot message with its history item.
- Without `before`, the page ends at the newest item. Pass the reply's `before`
  to get the next older page; `"before":null` means the page starts the
  conversation. Cursors are opaque and stay valid as the thread grows.
- `limit` is 1 to 500, default 100. A page ends on a stored-entry boundary, so it
  can hold a few more items than `limit`, and fewer when it reaches about 600 KiB.
- Text is complete and unclipped, including inputs that were accepted but never
  ran. A tool call with no result is `running` while the thread's run is active
  and `interrupted` otherwise. Live output of a running command is only in the
  snapshot.
- **Oversized fields.** A `text`, `reasoning`, `input`, or `output` longer than
  65536 UTF-16 code units is delivered in parts. The item carries the first part
  and a `more` array, present only then:
  `"more":[{"field":"text","ref":"REF","offset":65536,"length":200010}]`.
  `offset` is where the item's part ends and `length` is the full length. When a
  page would still exceed the frame, the host shortens the largest inline parts
  further; `offset` always says how much the item holds.
- `history_text` returns the following part of one field:
  `{"text":"Next part","offset":65536,"next_offset":131072,"length":200010}`.
  Each part holds at most 65536 code units and never splits a character. Repeat
  with `next_offset` until it is `null`. `ref` is opaque. An unknown `ref` returns
  `not_found`; an `offset` beyond the text or inside a character returns
  `invalid_request`.
- Every reply fits the 1 MiB frame.

Reading history never starts or resumes work. For a thread with no active run
the host opens the saved conversation read-only: it recovers no task, runs no
tool, and sends no model request, including for a thread whose last run was
interrupted.

### Thread list and management

`list_threads` result, version 3: `{"threads":[SUMMARY],"next_cursor":null}`.

- Without `checkout` it lists threads of every checkout. With it, the checkout is
  resolved as in version 1.
- Order is `updated_at`, newest first. `limit` is 1 to 200, default 100.
  `next_cursor` is `null` on the last page; otherwise pass it as `cursor`. A
  thread that becomes active during paging can appear on two pages; merge by `id`.
- Archived threads are omitted unless `"archived":true`, which lists both.
  Version 1 and 2 listings have no archive and still include them.

`rename_thread` and `archive_thread` return `{"thread":SNAPSHOT}` and are allowed
during a run. A title is normalized like a derived one, with a limit of 200
characters; a title with no visible text is `invalid_request`. `"archived":false`
restores a thread. Archiving changes only listing: the thread, its history, and
its files are kept, and it can still run.

### Thread naming

These separate operations leave all existing status and thread reply shapes
unchanged. An older host rejects them with `invalid_request`; the desktop shows
an update notice instead of saving a device-only preference.

```json
{"op":"get_thread_naming"}
{"op":"set_thread_naming","enabled":true,"model":{"provider":"local","model_id":"small-model"}}
```

Both return `{"enabled":true,"model":{"provider":"local","model_id":"small-model"}}`.
The initial value is `{"enabled":false,"model":null}`. Enabling requires an
exact, available model from the host's Pi provider catalog. A model can be
kept while disabled even if it is no longer available. Credentials and local
endpoints use the existing Providers setup, never Codex or Claude Code's
private credentials. The host stores the setting as `thread_naming_v1` in its
settings table; older hosts ignore it. No schema migration is required.

After the first new submission is accepted, including a checkout-queued
submission, the host can send its first 8,000 UTF-16 units (without splitting
a surrogate pair) to the selected naming model. This is a separate text-only
request with no tools, files, images, later messages, or conversation session.
It works for every coding harness. Naming requests run one at a time, with
at most 32 waiting, a 45-second deadline per call, a 2,048-token output budget,
and no retry or provider fallback. An empty text prompt makes no call.
Naming usage is not added to the coding conversation's usage field.

A successful title is one plain line of at most 72 Unicode scalar values.
An invalid response, error, deadline, full queue, or host shutdown leaves the
fallback title unchanged. Work is not replayed after a restart, on a duplicate
submission, or on a later message. Enabling naming does not scan old threads.
Settings changes abort current naming work and drop waiting names. A late
result must still match the initial title and settings; it never replaces a
manual rename or changes the run's state or activity time. A saved name uses
the existing snapshot and thread-summary notifications.

### Thread-list watch

`watch_threads` is a subscription shaped like `watch`. Its first frame is
`{"ok":true,"result":{"threads":[SUMMARY]}}` with the 200 most recently updated
threads, archived ones only with `"archived":true`. Each later frame is

```json
{"event":"threads","threads":[SUMMARY],"removed":["HOST_UUID"]}
```

- `threads` holds only summaries that are new or changed since the previous
  frame; `removed` holds IDs that left the list, by archive or by falling out of
  the newest 200. Replace by `id`, delete the removed, and order by `updated_at`.
- A summary changes with its status, title, model, archive flag, pending
  approval, or `updated_at`. Streaming text does not produce a frame.
- Delivery follows the `watch` rules: the subscription registers before the
  first read, repeated changes coalesce into one pending wakeup per client, a
  client that does not read is disconnected rather than queued for, and a
  disconnect stops no work. After a reconnect the first frame is the full list.

### Changed files

`changed_files` with `thread_id` returns the files that thread's `edit` and
`write` tools wrote, as the host records them for its file protections:

```json
{"files":["docs/notes.md","src/parser.rs"]}
```

Paths are relative to the thread's checkout, sorted, at most 500. A file stays
listed after the thread's run ends, after a commit, and after someone else
changes it; the list says what the thread wrote, not what differs now. Files
that only commands changed are not listed. Unknown threads return `not_found`.
The operation needs version 3; hosts from before it reject it as an unknown
operation, so a client treats that as no list.

### Model and thinking level

`configure_thread` changes a thread's model, its thinking level, or both, and
returns `{"thread":SNAPSHOT}`. The change applies from the next run.

- A thread with an active run, including one being stopped, returns `busy`.
- The model must be in the catalog with configured authentication, or the reply
  is `model_unavailable`. This is also checked for the current model when only
  the thinking level changes.
- `thinking` must be in the chosen model's `thinking_levels`, or the reply is
  `invalid_request`. When only the model changes, a level the new model lacks
  moves to its nearest supported level.
- The choice is saved with the thread and survives restart. The default model
  still affects only new threads.

### Messages during a run

Without `when_busy`, `submit` to a thread with an active run returns `busy`, as
before. With it, the input is accepted and queued:

- **`"when_busy":"follow_up"`** waits for the active run to end, then starts a
  new run.
- **`"when_busy":"steer"`** joins the active run at its next step: after the tool
  calls in progress finish, or when the model finishes its answer. It runs under
  that run's authorization and approval mode. A steering input the run did not
  take before it ended stays queued and then behaves as a follow-up.
- If the thread has no active run and nothing queued, `when_busy` has no effect
  and the submission starts a run at once.
- `approval` is the mode of the run this submission starts, now or later. A
  steering input that joins a run does not change that run's mode.

The reply is the usual `{"thread_id":"HOST_UUID","request_id":"CLIENT_UUID","accepted":true}`.
A queued input is saved with FULL durability before the reply. Request IDs
deduplicate as for any submission: the same ID and text returns the original
acceptance without queueing again, and different text returns `conflict`. A
thread holds at most 16 queued inputs; one more returns `busy`. `when_busy` does
not opt into waiting for another thread's run: that still returns `busy`. The
explicit [checkout queue](#durable-checkout-queue) uses `submit_checkout`.

A queued input is listed in the snapshot's `queued` and is not a message yet.
When it starts or is taken, it leaves `queued` and appears in `messages` and
`history` with the ID `request-CLIENT_UUID`, at the position where it ran.

- **Starting.** When a run ends with status `idle`, eligible follow-ups and
  checkout waiters share the durable acceptance order described below. The next
  input starts through the same admission as a submission: checkout
  lease, checkout identity check, and a new authorization object in that input's
  own `approval` mode. Nothing carries over from the run before it.
- **Held queue.** A run that ends in `error` or `interrupted`, and a host
  restart, leave the queue in place and start nothing. Queued inputs survive
  restart. The next submission with `when_busy` to that thread joins the queue
  and makes its follow-ups eligible in their original order; a plain submission
  can run first only when no checkout waiter or eligible follow-up precedes it.
  Its held follow-ups become eligible and follow when that run ends `idle`. A queued input that cannot start, for
  example because its model is no longer available, sets the thread to `error`
  with that reason and stays queued.
- **Stop** cancels every queued input of the thread, as well as the run. An
  input sent with `when_busy` after Stop starts when cleanup finishes.
- **`cancel_queued`** removes one queued input and returns `{"thread":SNAPSHOT}`.
  An input that already started, was taken by the run, or was cancelled returns
  `not_found`.
- A cancelled input never runs. Its request ID stays used: repeating it returns
  the original acceptance and queues nothing.

### Run limits

- **Turn limit.** A run may take 300 tool rounds. The round that reaches the
  limit completes its tools, then the run ends with status `error` and
  `{"code":"turn_limit","message":"..."}`. Nothing is lost: the next submission
  continues from those results with a new limit.
- **Retry.** A model request that fails with a transient provider error, such as
  overload, rate limiting, or a dropped connection, is repeated up to three
  times, after about 2, 4, and 8 seconds. The snapshot shows `retry` while
  waiting, and Stop cancels the wait. Other provider errors, and a fourth
  failure, end the run with `runtime_error` as before. Only the model request is
  repeated. A tool call that ran is recorded before the next request and is
  never run again, and no retry follows a host restart.

### Compatibility

- Version 1 and 2 clients keep working against a version 3 host and see none of
  the above, with one exception: a thread's model can now change between runs.
  Desktop builds from before version 3 treat a model change on the thread they
  are watching as a protocol error and must reselect the thread.
- A version 3 client must not send version 3 operations to an older host, and
  must treat every version 3 field as absent there.
- Storage changes are additive. A store written by an older host loads without
  an export step: on first start the host adds summary columns to `host.sqlite`
  and fills them from the saved snapshots. Repeating the migration changes
  nothing. Downgrading a host after version 3 state was saved is not supported.
- The host and its runtime sidecar are one update unit, as before. The internal
  notification channel gained one reserved name for thread-list changes.

## Protocol version 4

Version 4 adds agent harnesses: a thread may be run by Codex or Claude Code
instead of Pi. It
is negotiated as version 3 is, and every operation accepts
`"protocol_version":4`. Replies to version 1, 2, and 3 requests are unchanged,
and the operations and fields below are rejected there with `invalid_request`.
A client that steps down from 4 to 3 sees only Pi.

```json
{"op":"harnesses","protocol_version":4}
{"op":"set_harness_path","protocol_version":4,"harness":"codex","path":"/absolute/path/to/codex"}
{"op":"set_harness_mode","protocol_version":4,"harness":"codex","mode":"ask"}
{"op":"set_up_harness","protocol_version":4,"harness":"claude"}
{"op":"install_harness","protocol_version":4,"harness":"codex"}
{"op":"start_harness_sign_in","harness":"codex"}
{"op":"create_thread","protocol_version":4,"checkout":"/absolute/checkout","provider":"codex","model_id":"MODEL","request_id":"CLIENT_UUID","harness":"codex","thinking":"high","mode":"ask"}
{"op":"configure_thread","protocol_version":4,"thread_id":"THREAD_ID","mode":"read_only"}
```

- **`harnesses`** lists every harness, Pi first:

  ```json
  {"harnesses":[{"id":"codex","name":"Codex","state":"ready","note":null,"version":"0.169.0","supported":{"min":"0.169.0","tested":"0.169.0"},"path":"/absolute/path/to/codex","capabilities":{"steer":true,"approvals":true,"thinking":true,"usage":true},"protection":"One line","models":[{"provider":"codex","model_id":"MODEL","name":"Name","authenticated":true,"reasoning":true,"thinking_levels":["low","medium","high"],"context_window":null,"input":["text"]}],"modes":[{"id":"ask","label":"Ask","detail":"One sentence"}],"default_mode":"full_access"}]}
  ```

  `state` is `ready`, `not_installed`, `unsupported_version`, `signed_out`,
  `unavailable`, or `setup_required` (the harness needs a download the person
  starts with `set_up_harness`); `note` says how to fix anything but `ready`, or gives a caveat
  for a ready harness newer than tested. `supported.min` is the oldest version
  Plait runs and `supported.tested` the one its message types come from. Pi
  has `version`, `supported`, and `path` null and `models` null: its models are
  the `status` catalog. `protection` says in one line what protects the
  checkout in that harness's threads. `modes` are the harness's own permission
  modes and `default_mode` the one new threads start in; Pi has `modes` empty
  and `default_mode` null, because its threads use Plait's approval mode.
  Clients must accept harness IDs, states, modes, and fields they do not know.
- **`set_harness_path`** runs Codex or Claude Code from `path` instead of the
  login shell's `PATH`; omit `path` to return to `PATH`. It replies with the
  whole list, that harness checked again. The path is a host setting in
  `host.sqlite`; Pi is `invalid_request`.
- **`set_harness_mode`** sets a harness's `default_mode`, also a host setting,
  and replies with the whole list. A mode the harness does not list, or Pi,
  is `invalid_request`.
- **`set_up_harness`** sets up a harness that needs it; only `claude` does
  (see [Claude Code](#claude-code)), and any other harness is
  `invalid_request`. It replies with the whole list, Claude Code checked again: a
  finished setup shows whatever the normal checks find, and a failed one stays
  `setup_required` with a `note` saying what went wrong. Setting up again
  when the copy is already in place downloads nothing. The download gives up
  after 15 seconds, inside the host's 20-second limit for any request, and
  setup can simply be tried again.
- **`install_harness`** installs the Codex or Claude Code version Plait is
  tested with into `tools/` in the host store, and for Claude Code also runs
  setup. The host downloads the vendor's npm package for its own platform,
  checks its pinned sha512, and unpacks only the executable's tree; a failed
  install leaves nothing a lookup would use. A chosen path and the login
  shell's `PATH` are searched first, so a copy the person installed always
  wins. Older hosts reject the operation as `invalid_request`. It replies
  with the whole list. Entries carry `installable` (the version this host can
  install, or null when it has no build for its platform) and `managed`
  (the copy that runs is the one the host installed). Only `set_up_harness`
  and `install_harness` reach the network for a harness.
- **`start_harness_sign_in`** runs the harness's own sign-in on the host:
  `codex login --device-auth`, or `claude auth login`. It replies like
  `start_sign_in`, with provider `harness:codex` or `harness:claude`, and
  `sign_in_status`, `answer_sign_in`, and `cancel_sign_in` follow it. Codex shows
  a link and code; Claude Code shows a link and asks for the code the page
  gives. Credentials stay in the harness's own files; the host never reads
  them or keeps a pasted code. A flow lasts at most 15 minutes, and
  cancelling it stops the process.
- **`create_thread`** takes an optional `harness`, `pi` when absent. A Codex
  thread's model has provider `codex` and a model ID from Codex's list, and a
  Claude Code thread's has provider `claude` and a model ID from Claude Code's
  list; a model from another harness is rejected with `invalid_request`.
  `thinking` goes to the nearest level the model offers. `mode` is one of the
  harness's modes, its `default_mode` when absent; Pi threads refuse it. A
  thread stays in its harness: `configure_thread` accepts only models of the
  same harness. `configure_thread` also takes `mode`; given alone, it is
  accepted while the thread runs, and applies from the next Codex turn or at
  once in Claude Code.
- **Snapshots and summaries.** A version 4 snapshot adds
  `"harness":{"id":"codex","version":"0.169.0","mode":"ask","reported":"ask"}`:
  the harness, the version of its latest run (`null` before the first), the
  thread's mode, and the mode the harness reported using for its latest run
  (`null` before the first). `reported` differs from `mode` when the
  harness's own configuration overrides it, and is `custom` when it matches
  none of the modes. Pi threads, including every thread saved earlier, report
  `{"id":"pi","version":null,"mode":null,"reported":null}`. Version 4 thread
  summaries in `list_threads` and `watch_threads` add `"harness":"pi"`,
  `"harness":"codex"`, or `"harness":"claude"`.

### Codex

The host never installs, updates, or configures Codex, and reads none of its
files or credentials. It finds `codex` on the login shell's `PATH` (or the path
set above), reads `codex --version`, and refuses versions older than
`supported.min`. It asks Codex itself, through `codex app-server`, whether it
is signed in and which models it offers; that answer is reused for a minute,
and `set_harness_path` asks again at once. Codex keeps its own sign-in,
configuration, MCP servers, and instructions.

One `codex app-server` runs, in the login environment, while any Codex thread
runs; it closes 30 seconds after the last run ends, so an idle host holds no
Codex process. A run still holds the checkout as a Pi run does, so one checkout
has one active run across both harnesses.

Each run is one Codex turn. The first run starts a Codex thread and later runs
resume it; if Codex no longer has it, the run fails and asks for a new thread.
What protects the checkout is the thread's Codex mode. It replaces Plait's
approval mode and the project's command policy, which do not apply to Codex
threads; a submission's `approval` is ignored. Each turn sends the mode's
approval policy, sandbox, and reviewer in full, so Codex's own configuration
defaults do not change what a mode means, though its enforced requirements
can, and `reported` then shows what Codex used:

| Mode | Approval policy | Sandbox | Reviewer |
|---|---|---|---|
| `read_only` | `on-request` | `read-only` | `user` |
| `ask` | `on-request` | `workspace-write`, no network | `user` |
| `approve_for_me` | `on-request` | `workspace-write`, no network | `auto_review` |
| `full_access`, the default | `never` | `danger-full-access` | `user` |

- Each request Codex sends to the person (a command, an edit) is a Plait
  approval; denying it declines the request. With `approve_for_me`, Codex's
  own reviewer answers most requests. With `full_access`, Codex never asks.
- Plait's guard on files that had changes before the run does not apply to
  Codex. Edit approvals list such files in `dirty_paths` so the person can see
  them, but nothing stops Codex writing them.
- Requests Plait does not present (extra permissions, questions for the
  person, MCP forms) are declined.

Codex's events are translated into the same transcript entries Pi threads
keep, in `<thread>.codex.sqlite` beside the host store, written as each item
finishes. Streaming text and reasoning reach watchers at most every 16 ms and
are not written to disk. History, full text, and usage work as for Pi. Changed
files lists the checkout files Codex's completed edits wrote; files that
Codex's commands wrote are not listed. A message sent with `"when_busy":"steer"`
joins the running turn at once, so it cannot be cancelled after that. Stop asks
Codex to end the turn and waits up to 10 seconds. A run interrupted by a host
restart is never resumed or repeated. Codex's own error text and diagnostics
can quote the provider or the machine, so a failed run reports a fixed message
instead.

### Claude Code

Claude Code threads run the person's installed `claude` through Anthropic's
Agent SDK, pinned at 0.3.326. Claude Code loads the person's own settings,
`CLAUDE.md` files, skills, slash commands, hooks, and MCP servers, as `claude`
does in a terminal. The host never installs, updates, configures, or signs in
to Claude Code, and reads none of its files or credentials.

**Setup.** Plait's releases do not include the SDK, whose license reserves all
rights, so the Claude Code entry starts as `setup_required` and no Claude Code
process is started before setup. `set_up_harness` downloads
`claude-agent-sdk-0.3.326.tgz` from the npm registry and checks its SHA-512
against the `integrity` the runtime's `package-lock.json` records, before
anything is written. It unpacks only `sdk.mjs` (which imports only Node's
built-in modules), `package.json`, and Anthropic's `LICENSE.md` and
`README.md`, through a reader that refuses absolute paths, `..`, links,
special files, oversized entries, and truncated or corrupt archives. The files
are written into a new folder and renamed into place as
`harnesses/claude-agent-sdk/0.3.326/` beside `host.sqlite`, so a failure
leaves nothing behind. Each time the host loads `sdk.mjs`, its SHA-256 is
checked against the pinned value; a changed file is refused, and setting up
again replaces it. The SDK's per-platform Claude Code binaries are never
downloaded: Plait always runs the installed `claude`. Because Plait does not
ship the SDK, it is not listed in Plait's third-party notices; the person's
copy keeps Anthropic's `LICENSE.md`.

**Checks.** After setup the host finds `claude` on the login shell's `PATH`
(or the path set above), reads `claude --version`, and refuses versions older
than `supported.min`, 2.1.290: the first whose replayed messages wait until a
turn takes them, which places steering messages. Releases on the tested 2.1
line run without a note; a later line runs with one. Sign-in comes from
`claude auth status --json`, of which only whether it is signed in and the
provider kind are kept. Any sign-in the person's `claude` uses is accepted: a
Claude subscription, an API key, or a cloud provider. `signed_out` means only
that Claude Code says it is not signed in. Models and their thinking levels
come from the answer to the SDK's `initialize` request, from a Claude Code
started without any message, with hooks off and no MCP server loaded, and
closed at once; no model is asked anything. That check takes about 0.2 seconds
and is reused for a minute. A run does not repeat it: without a recent check,
the run's own `initialize` answer checks the model.

**Runs.** Each run starts one Claude Code process for the thread's checkout,
which takes the run's message and any steering messages and ends when none is
left. The first run starts a Claude session under an ID the host chooses, and
later runs resume it; if Claude Code no longer has it, the run fails and asks
for a new thread. What protects the checkout is the thread's Claude Code mode,
together with the person's Claude Code settings, which may allow or deny tools
on their own. It replaces Plait's approval mode and the project's command
policy, which do not apply; a submission's `approval` is ignored.

| Mode | Claude Code mode | What asks |
|---|---|---|
| `ask` | `default` | Each edit and command the person's settings do not already allow |
| `accept_edits` | `acceptEdits` | Commands; edits in the checkout run without asking |
| `plan` | `plan` | Leaving plan mode to start on a plan; nothing changes before |
| `auto`, the default | `auto` | Only what Claude Code's own safety check cannot decide |
| `bypass` | `bypassPermissions` | Nothing; no sandbox |

- Each permission request Claude Code sends is a Plait approval for that one
  call; denying it declines the call, and Claude Code tells the model. Plait
  never saves a permission rule to the person's settings. Questions Claude Code
  would ask the person (`AskUserQuestion`) are declined with a note asking the
  model to put the question in its reply.
- A mode change during a run is sent to Claude Code at once. `reported` is
  cleared until Claude Code confirms the mode it is in; a mode Plait does not
  offer, such as `dontAsk` from the person's settings, reports `custom`.
- Plait's guard on files that had changes before the run does not apply.
  Edit approvals list such files in `dirty_paths`, but nothing stops Claude
  Code writing them.

Claude Code's messages are translated into the same transcript entries Pi
threads keep, in `<thread>.claude.sqlite` beside the host store, written as
each message finishes; streaming text and reasoning reach watchers at most
every 16 ms and are not written to disk. History, full text, and usage work as
for Pi, and history reads the stored file without starting Claude Code.
Changed files lists checkout files that Claude Code's edit and write tools
changed; files that its commands wrote are not listed. A message sent with
`"when_busy":"steer"` goes to Claude Code at once, which takes it at its next
step or as the next turn, so it cannot be cancelled after that. Stop
interrupts the turn, drops any message Claude Code has not taken yet, and ends
the process if Claude Code does not answer within 10 seconds. A run
interrupted by a host restart is never resumed or repeated. Claude Code's own
error text can quote the provider, the account, or the machine, so a failed
run reports a fixed message instead.

What has been checked against a real Claude Code 2.1.326 without sending a
message: version and sign-in parsing, the model list, every flag a run starts
Claude Code with, mode changes in every mode, and interrupting with no turn
running. What happens inside a turn has been tested only against a scripted
stand-in so far; opt-in live tests for it are described in
[Verification](VERIFICATION.md#known-gaps).

## Internal authorization boundary

Each accepted run gets a new in-memory authorization object. Its policy is either
exact-call interactive approval (`ask`) or automatic approval (`auto`), chosen by
that run's submission. A thread ID, an earlier approval, or a recovered snapshot
does not grant permission to another run. Recovery keeps authorization inactive;
Stop and shutdown revoke it. Answers after cancellation or consumption return
`not_found`, as before.

Authorization and tool execution are separate. Argument limits, dirty-path and
ignored-path protection, checked file helpers, and the command sandbox still
apply in both modes. Automatic approval removes the prompt, not those checks.
Trusted is the default [command policy](#command-policy) and the sandbox is
opt-in per checkout; approval mode never changes which policy a run uses.

A version 3 [queued input](#messages-during-a-run) is the only deferred start. The
host saves its text and the approval mode its user submitted it with; that is a
recorded choice, not a grant. When the input starts, it passes the same
admission as a new submission and gets its own authorization object, so nothing
from the run before it, including automatic approval, carries over. A queue
never starts by itself after a failed run or a host restart. A steering input
joins a run that is already authorized and adds no permission to it. There is no
other scheduler and no persisted grant in this contract. Future automation
grants require separate protocol, persistence, and permission review.

## Provider sign-in

These operations change which providers the host can use while it runs. The
desktop Settings UI in source builds calls them. They are additions, not
a protocol version: each accepts the optional `protocol_version` and replies the
same for every version. A host without them rejects the operation name with
`invalid_request`.

```json
{"op":"reload_providers"}
{"op":"list_providers"}
{"op":"start_sign_in","provider":"anthropic","method":"browser"}
{"op":"sign_in_status","sign_in_id":"HOST_UUID"}
{"op":"answer_sign_in","sign_in_id":"HOST_UUID","input":"Pasted code or choice ID"}
{"op":"cancel_sign_in","sign_in_id":"HOST_UUID"}
{"op":"set_provider_key","provider":"openrouter","key":"SECRET"}
{"op":"sign_out_provider","provider":"openrouter"}
{"op":"add_endpoint","id":"home","name":"Home server","base_url":"http://127.0.0.1:8080/v1","model_ids":["MODEL"],"key":"SECRET"}
{"op":"update_endpoint","id":"home","name":"Home server","base_url":"http://127.0.0.1:8080/v1","model_ids":["MODEL"],"key":"SECRET"}
{"op":"remove_endpoint","id":"home"}
{"op":"check_endpoint","base_url":"http://127.0.0.1:8080/v1","key":"SECRET"}
{"op":"check_endpoint","id":"home"}
```

`method` and every `key` are optional. `update_endpoint` needs only `id`; a field
left out keeps its saved value. String values are trimmed and must not contain
control characters. Provider and endpoint IDs are at most 128 bytes; `key` and
`input` are at most 8192 bytes.

### What crosses the socket

Credentials stay in Pi's own files in Pi's agent directory (`~/.pi/agent`, or
`PI_CODING_AGENT_DIR`): `auth.json` for sign-ins and keys, `models.json` for
custom endpoints. The host store holds none of them.

- **Desktop to host, once each:** an API key in `key`, and the text a user pastes
  to finish a sign-in in `input`. Each travels in one request on the owner-only
  socket. The host passes it to Pi and keeps no other copy. The `key` of
  `check_endpoint` with `base_url` is used for that one check and not stored.
- **Host to desktop, never:** a key, token, or any other stored credential; a
  credential file path; a provider response body; the pasted `input`. No
  operation reads a stored credential back.
- **Host to desktop, for the user to act on:** the sign-in `url` and, in a device
  flow, the `user_code`. These are one-time values of a pending sign-in, not
  credentials, but the address can hold the flow's state. Open it; do not log or
  save it. The host drops both when the sign-in ends.
- Notifications still carry thread IDs only.

### Reload

`reload_providers` result: `{"providers":[PROVIDER]}`. The host reads `auth.json`,
`models.json`, and Pi's cached catalogs again, with no network request. It builds
a complete registry first, then replaces the model catalog and the authenticated
set of `status` in one step. If loading fails or exceeds 5 seconds, the reply is
`internal` and the previous registry stays in use.

A run in progress keeps the registry it started with, including a provider or
endpoint that the reload removed. Credentials are read from Pi's store for each
model request, so signing out of a provider fails a run that needs it at its
next model request.
Every operation below that changes a credential or an endpoint reloads before it
replies; the next `status` already shows the change.

### Provider list

`list_providers` result: `{"providers":[PROVIDER]}`, one entry per Pi provider.
All keys are always present.

```json
{"id":"anthropic","name":"Anthropic","kind":"builtin","authenticated":false,"credential":"none","model_count":18,"sign_in":{"name":"Anthropic (Claude Pro/Max)","subscription":true},"api_key":{"name":"Anthropic API key"},"endpoint":null}
{"id":"home","name":"Home server","kind":"custom","authenticated":true,"credential":"external","model_count":1,"sign_in":null,"api_key":{"name":"API key"},"endpoint":{"base_url":"http://127.0.0.1:8080/v1","api":"openai-completions","model_ids":["MODEL"]}}
```

- `kind`: `builtin` ships with Pi; `custom` is declared in `models.json`.
- `authenticated` matches the `authenticated` flag of that provider's models in `status`.
- `credential`: `sign_in` or `api_key` is stored in `auth.json` and can be removed
  with `sign_out_provider`. `external` means the provider works through an
  environment variable, a key in `models.json`, or ambient cloud credentials,
  which the host does not manage. `none` means not authenticated.
- `sign_in` is null or names the sign-in `start_sign_in` runs; `subscription` tells
  a plan such as Claude Pro or ChatGPT Plus from an ordinary account sign-in.
- `api_key` is null when `set_provider_key` cannot apply. A provider can offer both
  methods, one, or neither.
- `endpoint` is null for built-in providers. `base_url` is shown without any user
  name, password, query, or fragment, and is null if it cannot be parsed. `api`
  is the API of the first model.

### Subscription sign-in

`start_sign_in`, `sign_in_status`, `answer_sign_in`, and `cancel_sign_in` all
return `{"sign_in":SIGN_IN}`. All keys are always present.

```json
{"id":"HOST_UUID","provider":"anthropic","state":"waiting","url":"https://provider.example/authorize?...","user_code":null,"input":{"kind":"code","message":"Complete login in your browser, or paste the authorization code / redirect URL here:","placeholder":"http://localhost:53692/callback","required":false,"choices":[]},"message":"Complete login in your browser. If the browser is on another machine, paste the final redirect URL here.","expires_in_seconds":598,"error":null}
```

The host runs Pi's sign-in flow for the provider and stores the result where Pi
stores it. The desktop shows what the flow asks for and relays answers:

- `state`: `starting` (nothing to show yet), `waiting` (the user must act),
  `finishing` (the host is exchanging the result), then one of `signed_in`,
  `failed`, `cancelled`, or `expired`. Those four are final.
- `url`: open it in the user's browser. It is always `http` or `https`.
  `user_code`: show it; the user types it on that page (device flows).
- `input` is null or one question. `kind` is `code` (an authorization code or the
  final redirect URL), `choice` (answer with one `choices[].id`), `text`, or
  `secret`. `required` is false only for a `code` when the address returns the
  browser to this machine: the sign-in then normally completes by itself, and
  the field is a fallback if the redirect cannot arrive. Keep showing it.
- `message` is Pi's current instruction or progress text, or null.
- `error` is null unless `failed`; it is `{code,message}` with fixed host text.
- `signed_in` is reported after the reload, so `status` already agrees.

Pi's flows for Anthropic and OpenAI Codex first ask for a method. If `method`
names one of the offered choices, the host answers that first question; otherwise
it is passed on as a `choice`. A browser method listens on a loopback TCP port
(Pi's callback server on `127.0.0.1`, on a port Pi chooses) until the sign-in
ends. Pi's flow contacts the provider to exchange the result; the desktop does not.

Limits: four sign-ins in progress; one per provider, where a new start cancels the
earlier one; expiry after 10 minutes, or sooner when a device code expires
sooner. Final states stay readable for 5 minutes, at most 16 records. Nothing is
saved: a host restart forgets every sign-in, and host shutdown cancels those in
progress and closes their listeners. `start_sign_in` and `answer_sign_in` wait up
to 3 seconds for the flow to show something or finish, then reply with the state
so far. Read `sign_in_status` about once a second while `starting`, `waiting`, or
`finishing`. Other requests queue behind a waiting reply.

### API keys and sign-out

`set_provider_key` and `sign_out_provider` return `{"provider":PROVIDER}`.
`set_provider_key` stores or replaces the key through Pi's own key setup. Pi keeps
one credential per provider, so a key replaces a subscription sign-in and a
sign-in replaces a key. `auth.json` is created with mode 0600, and the host
restores 0600 after a write if group or other access was present. The key is
stored as literal text: Pi's `!command` and `$NAME` forms are escaped, so a
request cannot make the host run a command or read its environment. A provider
whose key setup asks for more than one secret value (for example Amazon Bedrock)
returns `invalid_request`; configure it in Pi.

`sign_out_provider` removes the stored credential and is not an error when none
exists. It cannot remove an `external` credential. Both operations first cancel a
pending sign-in for the same provider.

### Custom endpoints

`add_endpoint` and `update_endpoint` return `{"provider":PROVIDER}`;
`remove_endpoint` returns `{"removed":"home"}`. They edit one provider entry in
Pi's `models.json`:

```json
{"providers":{"home":{"name":"Home server","baseUrl":"http://127.0.0.1:8080/v1","api":"openai-completions","apiKey":"none","models":[{"id":"MODEL"}]}}}
```

- New entries use the `openai-completions` API. `apiKey` `"none"` is Pi's
  documented placeholder for servers without a key; it makes the endpoint usable
  and is not a secret. A real `key` goes to `auth.json` like any API key and takes
  precedence. `sign_out_provider` removes it again.
- `id`: 1 to 64 lowercase letters, digits, dots, hyphens, or underscores, starting
  with a letter or digit, and not a built-in provider ID. `name`: at most 128 bytes.
  `base_url`: an `http` or `https` URL of at most 2048 bytes with no user name,
  password, query, or fragment. `model_ids`: 1 to 200 distinct IDs of at most 256
  characters.
- Every other entry and every field the host does not set is written back as
  read, including a hand-written `apiKey`, `headers`, `compat`, and per-model
  fields of models that stay. Indentation and file mode are kept; a new file is
  0600. The write is a synced temporary file and one rename beside the real
  file, so a symlinked `models.json` stays a symlink.
- The host refuses with `conflict`, and writes nothing, when `models.json` is
  not plain JSON (Pi also accepts comments, which a rewrite would delete), has
  an unexpected layout, or changed on disk during the edit. After writing, the
  host reloads and checks that Pi reports the endpoint. Pi ignores the whole
  file when any entry breaks its schema; in that case the host restores the
  previous file and returns `conflict`.
- Built-in providers cannot be edited or removed here. `remove_endpoint` deletes
  the stored key first, then the entry.
- With `key`, `add_endpoint` and `update_endpoint` save the entry first. If
  storing the key then fails, the entry remains and the reply is an error.

`check_endpoint` sends one `GET BASE_URL/models` and returns:

```json
{"reachable":true,"reason":null,"status":200,"model_ids":["MODEL"],"truncated":false}
{"reachable":false,"reason":"unauthorized","status":401,"model_ids":[],"truncated":false}
```

With `base_url` it uses that address and the optional `key`. With `id` it checks
a saved custom endpoint with its saved key and headers. `reason` is `timeout`,
`unreachable`, `unauthorized` (401 or 403), `not_found` (404), `http_error`, or
`invalid_response`; `status` is null without an HTTP response. Limits: 5 seconds,
a 1 MiB response, the first 500 model IDs (`truncated` flags more). Redirects are
reported as `http_error`, not followed. The request goes only to the address
named, and only when this operation is sent. Other requests queue until it
replies.

### Errors

The reply codes are the existing ones. `invalid_request`: malformed input, an
unknown field, a provider that lacks the requested method, or a built-in provider
given as an endpoint. `not_found`: unknown provider, endpoint, or sign-in ID.
`busy`: four sign-ins already in progress. `conflict`: an existing endpoint or
provider ID, an `answer_sign_in` when no input is pending, or a `models.json` the
host will not rewrite. `internal`: Pi could not store, remove, or reload. Messages
name the field at fault and never repeat its value.

## Plain folders

A project may be a plain folder when neither it nor any ancestor has a
repository marker: a `.git` file, or a `.git` folder containing `HEAD`, as Git
itself requires (an empty stray `.git` folder is no marker). A folder inside a
Git repository resolves to the repository root; an inaccessible or broken
repository is refused. Workspace projects report `vcs` (`git` or `folder`),
and Git reads of a plain folder return an empty status marked `plain` and a
file list walked without Git. The thread retains its kind even
if the folder later becomes a repository. Threads, approvals, and file viewing
work in plain folders. Undo, Changes, History, and worktrees are unavailable.
Nothing in a plain folder is protected or can be undone.

## Command policy

Each canonical checkout has a command policy: `trusted`, the default, or
`sandboxed`. It decides how the `command` tool runs for runs in that checkout.
Trusted matches how Codex and Claude Code run out of the box: commands run as
the user, with Git writes and the network. The sandbox is opt-in per project
under the project's settings → Commands; returning to Trusted asks for
explicit confirmation of its consequences. Hosts through v0.1.0-preview.10
defaulted to `sandboxed`.

```json
{"op":"command_policy","checkout":"/absolute/checkout"}
{"op":"set_command_policy","checkout":"/absolute/checkout","policy":"trusted"}
```

All return `{"checkout":"/canonical/checkout","policy":"sandboxed"}`, with the
policy now in force for that checkout. `checkout` is an absolute path inside a
Git working tree or a plain folder and is resolved to its canonical root, as in `list_threads`.
`policy` is `sandboxed` or `trusted`; anything else is `invalid_request`. Both
operations accept the optional `protocol_version` and reply the same way for
every version. A host without them rejects the unknown operation with
`invalid_request`; the desktop reports that policy could not be loaded. No existing request
or reply changes: `create_thread`, `submit`, and snapshots carry no policy. A
snapshot does not yet report the policy its run used.

- **Storage.** `host.sqlite` keeps one `settings` row per sandboxed checkout,
  keyed by the canonical path and holding the checkout root's device and inode.
  There is no schema change. `trusted` removes the row. A missing row, or a
  directory that is no longer the one that was sandboxed, is `trusted`, and so
  is a row a host before the change wrote for a trusted checkout. A row that
  cannot be read is `sandboxed`, rather than a guess. Another host store never
  shares the setting.
- **Who can set it.** Only this operation, sent by an authenticated client as
  an explicit user action. No tool, model output, thread, run, or snapshot carries or
  changes the policy, and a thread cannot take it to another checkout.
- **When it applies.** A run reads the policy once at admission and keeps it
  until it ends. A change applies to the next run. Recovery starts no run.
- **Linux.** Trusted commands and trusted MCP servers run as the user. Sandboxed commands and MCP servers are refused because a Linux containment backend is not yet enabled; see [Linux sandbox](LINUX-SANDBOX.md).

| | `sandboxed` | `trusted` |
| --- | --- | --- |
| Process | Seatbelt profile, `/bin/sh -c` | `/bin/sh -c` as the user, no profile |
| Working directory | Checkout | Checkout |
| Checkout and Git writes | Denied | Allowed |
| Other files | Read system and toolchain paths, and the main repository's Git directory when the checkout is a linked worktree; write private scratch and the thread build cache | Anything the user account can read or write |
| Network | Denied, except Unix sockets in scratch | Allowed |
| HOME and TMPDIR | A new private directory per command | The user's real home and temporary directory |
| Environment | Fixed PATH and Apple toolchain | The user's login-shell environment |
| Build output | Per-thread cache; Cargo offline | Wherever the project's tools put it |
| Standard input | None | None |
| Deadline, Stop, output bounds | As in [Safety boundary](#safety-boundary) | The same |

**Trusted environment.** The host runs as a per-user service with a minimal
PATH, so it asks the account's login shell, as a new terminal would. Once per
run, before the first trusted command, it starts the account's shell (from the
OS account record) as a login shell in the host's HOME and reads the resulting
environment. The shell is also interactive for zsh, bash, and fish, whose
interactive startup files commonly extend PATH. Startup files are the user's
own and run with the user's access. That shell starts from a fixed base: `HOME`, `USER`, `LOGNAME`,
`SHELL`, the system PATH, and, when the host has them, `TMPDIR`, `LANG`,
`LC_ALL`, `LC_CTYPE`, `SSH_AUTH_SOCK`, and `__CF_USER_TEXT_ENCODING`. Nothing
else from the host process is inherited. `PLAIT_HOST_EXE`, `PLAIT_LOCK_FD`, and
`PLAIT_NOTIFY_FD` are removed, and the guardian marks every inherited descriptor
close-on-exec, so the store lock, the checkout lease, and the notification
socket never reach the command. The read has a 10-second limit and runs under
the guardian. If it fails, the command runs with the base variables and
`HOME/.cargo/bin`, Homebrew, `/usr/local/bin`, and the system PATH, and its
output begins with a notice. Shell functions and aliases are not available:
commands run under `/bin/sh`.

**Unchanged in trusted mode.**

- Authorization. `ask` still prompts for each exact call and `auto` still
  removes only the prompt. Authorization is checked again before the command
  starts, so an approval after Stop does nothing.
- Lifecycle. The Rust guardian holds the store and checkout writer leases and
  kills the command's process group on Stop, host or sidecar death, the
  15-minute deadline, and shell exit. Output keeps its bounds and Unicode rules.
- File tools. `read`, `edit`, and `write` keep every check in
  [Safety boundary](#safety-boundary). Paths that were dirty or ignored before
  the run stay refused for the whole run, even if a command commits them.
- Approval text. A command prompt reads `Run command with full user access and
  network`, and the risk text of every prompt in a trusted run says that
  commands run as the user, outside the sandbox, with network access.
- Model instructions. The run's instructions and the `command` tool description
  describe the mode in force. The sandboxed wording is unchanged.

**Around each trusted command**, whether it succeeds, fails, or is stopped:

- Before it starts, the host compares each file this thread owns with its
  postimage record: identity, mode, size, and times. A file that already
  differs was changed by something else. Its record is dropped, in memory and
  in the store, so the command cannot bring that change into the thread's
  ownership.
- After it ends, a file the thread still owned at the start and that changed
  during the command gets a fresh record of its new state, read through the
  checked file helper and committed to the store. The thread keeps ownership,
  so edit, format or generate, then edit again works. The host cannot tell the
  command's change from another program's change made while the command ran;
  both are refreshed.
- A file that is gone, ignored, or no longer one the file helper accepts loses
  its record instead.
- Nothing else becomes owned. A file the command creates, or changes without
  the thread having owned it, is an ordinary dirty path: file tools refuse it,
  in this run and later ones, until it is clean again, for example after a
  commit. Paths that were dirty before the run stay refused whatever the
  command does.
- A later edit still compares the whole record, including the content hash. If
  the host stops between the command and the refresh, the old record remains
  and never matches, as before.
- If a path that was dirty before the run changed during the command, the
  command's output begins with a notice naming up to 20 such files.
  Directories are not examined. This reports the change; it does not prevent
  or undo it.

**What trusted mode gives up.** A trusted command is a program running as you.

- It can change, delete, or commit any file your account can, including files
  that were dirty before the run, ignored files, Git history, other checkouts,
  and your home directory. Dirty-path protection covers only the file tools.
- It can use the network, your credentials, your SSH agent, and your keychain
  as any program you run can, and can send what it reads anywhere.
- It can reach the host itself: the socket, the store, and the installed host
  files. The host does not offer the model a way to change the policy, approve
  a call, or reach another thread, but it cannot stop a trusted command from
  doing those things as you. Trust the project and the model accordingly.
- It can signal other processes, and a process that deliberately leaves the
  command's process group survives Stop with the same access.
- The checkout writer lease still excludes other Plait runs, not the command's
  own writes to other places.
- With automatic approval, trusted commands run without a prompt.
- Build output and dependency installs land in the checkout or your home, not
  in a private cache. An ignored directory such as `node_modules` or `target`
  is one entry of the 500-entry dirty baseline however many files it holds,
  whether it existed before the run or a command created it. Untracked files,
  and files ignored one at a time by a file pattern such as `*.o`, each count:
  a command that leaves more than 500 of them makes file tools and later runs
  fail until they are removed, committed, or ignored by directory.

## Development tools

The runtime exposes `read`, `read_output`, `edit`, `write`, and `command`.
These are model tools, independent of the client socket protocol version.

- `read`: `{path, offset?, limit?}` reads a regular UTF-8 checkout file. Offsets
  count bytes from zero; omitted offset defaults to zero, and omitted limit
  defaults to 8192 (4–12000 bytes allowed). Supplied values must be integers;
  negative offsets, nulls, strings, and fractional or out-of-range values are
  rejected rather than defaulted. Zero and EOF offsets are valid; EOF returns
  empty text with no continuation. The result gives the next offset when more
  remains. Pages never split a UTF-8 character. An offset beyond EOF, an offset
  inside a character, and invalid UTF-8 encountered while reading have distinct
  fixed errors; raw helper diagnostics are never exposed. UTF-8 validation is
  page-local, not a scan of the entire file. Reads keep the descriptor, root,
  symlink, hard-link, and Git-metadata checks, without loading the whole file.
  An absolute path inside a listed skill's folder (see
  [Skills and user instructions](#skills-and-user-instructions)) is read the
  same way, with that folder's resolved path as the root.
- `edit`: `{path, old_text, new_text}` replaces exactly one occurrence in a file
  up to 8 MiB. Approval compares its complete fingerprint, including content
  hash, before atomic replacement. Tool arguments still have a 16384-byte bound.
- `write`: `{path, content}` creates a new file and never overwrites an existing
  one. The same argument bound applies.
- `command`: `{command}` returns a bounded tail preview and a stored-output ID,
  including after nonzero exit. Live output stays bounded. Captured stdout and
  stderr are combined in arrival order, decoded independently as UTF-8.
- `read_output`: `{id, offset?, limit?}` pages captured command output from the
  current thread without executing anything. Range units and bounds match
  `read`. After interruption, the original command tool-call ID also resolves
  its most recent retained log. IDs grant no access to another thread or
  arbitrary host files.

The host stores command output in additive SQLite tables, outside the checkout,
using the host store's durability settings. It retains the first 16 MiB of each
command and the newest 32 logs per thread (at most 512 MiB of payload per thread).
Crossing that limit is explicit; expired IDs return `not_found`. Interrupted
captures are marked incomplete. Storage errors are reported without retrying the
command. Deletions free database pages for reuse rather than shrinking the file.
Logs may contain secrets a command prints; the host does not redact their contents.

### Skills and user instructions

At the start of each run, the runtime reads the person's Pi setup with Pi's own
resolution (`DefaultPackageManager` and `loadSkills` from the pinned
`@earendil-works/pi-coding-agent`), so a skill is offered exactly when Pi would
load it, including Pi settings filters and installed packages. Discovery never
installs or updates a package. Project skills (the checkout's `.pi/skills`,
`.agents/skills`, and project settings) load only for a project set to
`trusted`. A discovery failure leaves the run without skills rather than
failing it.

- The run's instructions gain, in order: the user-level context file from the
  Pi agent directory (the first of `AGENTS.override.md`, `AGENTS.md`,
  `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`, bounded at 32768 characters like the
  project's `AGENTS.md`, which follows it), then each skill's name,
  description, and `SKILL.md` location in Pi's `<available_skills>` format.
  Full skill text costs context only when the model reads it.
- `read` accepts an absolute path inside the folder of a skill with its own
  `SKILL.md`, or exactly the file of a standalone Markdown skill; a standalone
  skill never grants the folder it sits in. A skill folder is skipped when it
  is, or contains, the home, Pi agent, or host store directory, or when it is
  in or holds a credential folder (`.ssh`, `.gnupg`, `.aws`, `.azure`, `.kube`,
  `.docker`, `.config/gh`, `Library/Keychains`). Every skill read, including
  the one below, goes through the no-symlink helper rooted at the folder found
  at discovery, with that folder's device and inode checked, so a file or
  folder swapped for a link later fails rather than being read.
- Sandboxed commands may read, never write, those skill folders (not
  standalone skills' folders), so a skill's scripts and references work.
  Trusted commands already run as the user.
- A message that starts with `/skill:name` is sent as Pi sends it: the skill's
  file (at most 65536 characters) in a `<skill name location>` block, then the
  rest of the message. Unknown names are sent unchanged. The model receives the
  expanded message, while snapshots and `history` give the text as typed, so a
  client shows and recalls the `/skill:` command whatever the skill's length.
- A message that starts with `/name` for a prompt template (Pi's agent
  `prompts` folder, Pi settings and packages, and, for a trusted project, the
  checkout's `.pi/prompts`) is sent as the template's body with its arguments
  filled in: `$1`, `$@`, `$ARGUMENTS`, `${N:-default}`, `${@:-default}`,
  `${@:N}`, and `${@:N:L}`, split with Pi's quoting rules. Pi does not export
  its expander, so Plait has its own, tested against Pi's. The first template
  of a name wins. A linked template is followed once, at discovery, and only to
  a Markdown file outside credential folders; the body (at most 65536
  characters) is read through the no-symlink helper rooted where the link led,
  with that folder's identity checked. Unlike a skill, the expanded text is
  what snapshots and `history` show, as Pi shows it. Runs read no template file
  until one is used.

`agent_resources` (version 3) with `checkout` reports what a run there would
be offered now, under the checkout's current command policy, with the same
discovery. It never returns file contents:

```json
{"checkout":"/repo","trusted":false,
 "instructions":{"path":"/home/.pi/agent/AGENTS.md","bytes":2150},
 "skills":[{"name":"release-notes","description":"…","origin":"user","path":"…/SKILL.md","model_invocation":true}],
 "templates":[{"name":"review","description":"Review a change","argument_hint":"<path>","origin":"user","path":"…/review.md"}],
 "mcp_servers":[],
 "problems":[{"path":"…/SKILL.md","message":"description is required"}]}
```

`origin` is `user` (the Pi agent folder or `~/.agents`), `project`, `package`,
or `path` (named in Pi settings). `model_invocation` is false for a skill only
`/skill:name` loads. Problems include Pi's own diagnostics, skipped folders,
refused links, name clashes, and an oversized context file, which fails a run
but is only reported here. Each list holds at most 500 entries and each
description at most 1024 characters, and the whole reply stays under the
client's 1 MiB frame; anything left out is said first in `problems`. Hosts
from before this operation reject it as `invalid_request`.

MCP server metadata is also listed, as described below. Extensions and named
CLIs are not included. See the [roadmap](ROADMAP.md).

Automatic model-context compaction keeps all original history entries. Its
internal summary entries are omitted from displayed messages. If a required
summary fails, version 3 reports `context_compaction`; older clients retain the
existing `runtime_error` shape. A new user submission can retry; host restart or
history reads never restart model work automatically.

### MCP server listing and project switches

Pi threads use configured stdio servers and Streamable HTTP servers that need
no sign-in or use keys supplied by the configuration. Each run reads the bounded user file and,
for a trusted project, the project file once during resource discovery. Changes
apply to the next run. Missing files cost a failed open; malformed, oversized,
and unreadable files are problems rather than run failures. Codex and Claude
Code continue to use their own MCP setup and make none of these reads.

With no eligible server switched on, there is no MCP process, credential
resolution, network request, or search tool. Otherwise `tool_search` is offered;
its first call connects the enabled servers and registers their tools as deferred.
All exposures except `hidden` use search, including Pi's `codemode` and `direct`;
per-tool exposure overrides and patterns are honored. Each tool is a separate
call, shown as `mcp:server/tool`. Server instructions never enter the model's
instructions. Names have a reserved prefix and stable hash, are checked for
collisions, and cannot replace built-in tools.
Remote tool names must contain 1–128 ASCII letters, digits, `_`, `-`, `.`, `:`,
or `/`. Control, bidirectional, and zero-width characters are refused without
echoing the supplied name. Invalid tools are omitted and listed as problems;
other tools from the server remain usable. Server names also pass this display
boundary before Pi's stricter configuration-name validation.

The host reads `mcp.json` in Pi's agent directory (normally `~/.pi/agent`), then
`<checkout>/.pi/mcp.json` only for trusted commands. Merge and validation follow
Pi **1.5.2**, using the pinned `@earendil-works/pi-mcp` validator, with an
additional Plait consent boundary for repository-controlled entries:

- A full project entry replaces the user entry of the same name. An entry
  without `command`, `url`, or `type` may override only `enabled`, `exposure`,
  and `toolExposure`; all other user fields stay in place.
- A project definition or replacement is **off until explicitly enabled in
  project settings**, even when commands are trusted. Until then nothing from
  it runs: no process, credential command, variable expansion, or network request.
- Without that explicit choice, a restricted project override can only narrow
  the user configuration. It cannot enable a disabled user server. Tool
  visibility is the intersection of the user and project rules, including
  exact names and wildcard patterns; user-hidden tools stay hidden. Visible
  tools can be deferred or hidden. An explicit enable permits the reviewed
  override, bound to the same definition digest as a full project entry.
- An invalid entry is reported and skipped; a rejected project replacement
  leaves the valid user entry intact. Names whose `-` and `_` spellings share
  a tool namespace conflict. Legacy SSE is refused.
- A project entry cannot select a provider credential through `auth`. A
  restricted override may retain `auth` from the user entry.
- Pi's `-builtin:mcp` setting disables every server. Pi's package resolver
  supplies this setting, including trusted project overrides. Plait never
  loads the extension itself.

`agent_resources` adds `mcp_servers`, with at most 500 entries under its existing
reply budget. Each row has this shape:

```json
{"name":"docs","transport":"http","source":"/home/.pi/agent/mcp.json",
 "override_source":null,"program":null,"host":"docs.example",
 "enabled":false,"can_enable":true,"needs_sign_in":null,"signed_in":false,
 "disabled_reason":"Off until enabled for this sandboxed project.","problem":null}
```

`transport` is `stdio`, `http`, or null for an invalid entry. `program` is
the configured executable, never its arguments; `argument_count` is a count
only. Hover text directs the person to read those hidden arguments in the
defining file before turning the server on. For an in-checkout definition or
override, **Open file** uses the existing Files view; files outside the checkout
are shown as paths only. `host` is only the URL host,
never credentials, path, query, or fragment. `source` names the defining file;
`override_source` names a restricted project override. `problem` explains an
invalid entry or an observed connection/tool problem; rejected entries and
unreadable files also appear in `problems`.
Files are read up to 1 MiB each. Parse errors contain no file excerpts, and
neither environment values, headers, OAuth secrets nor resolved `!command` or
`${VAR}` values enter the reply.

`needs_sign_in` is true when an OAuth configuration or matching saved OAuth
state identifies browser sign-in, false for stdio or header/provider auth,
and null when it cannot be known without connecting. Public HTTP servers are
not assumed to need sign-in. `signed_in` says only that Pi holds an access or
refresh token, not that it is valid. This read uses Pi's normalized server
namespace and URL key in `mcp-auth.json`, with its legacy URL-only fallback,
without migrating, refreshing, or writing credentials. A server with known
OAuth requirements and no saved tokens reads **Sign in with Pi** in the desktop.
Browser sign-in, reuse/refresh of Pi's OAuth tokens, and Pi provider-backed
`auth` are planned, not executed in this stage. Known OAuth servers are skipped.
An HTTP authentication challenge marks the server as needing sign-in without
opening a browser, retrying the request, or exposing the response body.

A desktop can save its project choice and receive a refreshed `agent_resources`
reply with:

```json
{"op":"set_mcp_server","protocol_version":3,"checkout":"/absolute/checkout","name":"docs","enabled":true,"definition":"<64-character digest returned by agent_resources>"}
```

The host stores the boolean by canonical checkout and server name, bound to
the checkout directory's device/inode and a SHA-256 digest of the unresolved
definition. The digest includes transport, command, arguments, cwd, environment
and header names/values, URL, exposure rules, and the user definition underlying
an override. It never stores resolved values or edits Pi's files. A changed
definition makes the saved choice off again, labelled **Changed**; a new project
definition says **Needs review**. Unreviewed overrides say **User limits** while
using the intersection; turning the switch off and on explicitly enables the
reviewed override. User-level servers remain on by default in trusted projects
and off by default in sandboxed ones.

The settings request echoes the displayed `definition` digest. The host rejects
an enable if that digest is stale, and requires it for every project definition
or override. Old name-only switches cannot approve repository-controlled entries;
older clients can still disable them and manage user-only entries. A saved
choice cannot enable an invalid entry or bypass Pi's MCP disable setting.
Missing servers return `not_found`; blocked or stale choices return
`permission_denied`. A new directory at the same path does not inherit a choice.
The optional `consent` metadata is `required`, `changed`, or `restricted`; it is
configuration policy, separate from observed connection `state`.

This is an additive version 3 operation, not a protocol version bump. Older
hosts omit `mcp_servers` and show no section; older clients ignore the new list.
After a run connects a server, its matching configuration may also report
`state` (`connected`, `stopped`, or `failed`) and `tool_count` from its real
listing. These observations live only in this host, are invalidated by a
configuration or checkout-identity change, and disappear after host restart. Settings never connects
a server to fill them in. Startup failures and disconnected servers appear under
Problems with a bounded, redacted stderr tail; other servers and the run continue.

**Approval and containment.** Trusted projects follow the thread's Ask or Auto
mode. Sandboxed projects always ask for each call, including in Auto mode. The
existing `command` approval kind reads **Use <tool> from <server>**, with only
that call's arguments. Read-only annotations and other server hints cannot skip
approval. Stop revokes a pending answer before any call is dispatched.
Approval gates calls, not the server's own behaviour. Once on, a local server
can read the project and use the network from the moment first search starts
it, including when sandboxed and before any call has been approved. The settings
hover states this boundary. Environment/header values and credential commands
are always hidden in settings; inspect the defining file before opting in.

A stdio server is started by `plait-host mcp-server`, not Pi's process transport.
The guardian retains the store and checkout leases and closes all private file
descriptors in the server. The guardian starts with a fixed system PATH only;
the server environment arrives
through a bounded private pipe and never affects the guardian's loader. In a
sandboxed project, `sandbox-exec` starts a clean host exec stage inside Seatbelt;
only that stage reads the environment and replaces itself with the server.
MCP credential commands use the same environment handoff.
Private descriptors are close-on-exec before the server starts. A separate
lifetime pipe kills the server's process group on Stop, run completion, failure,
or host/sidecar death. As with commands, a descendant which deliberately leaves
that group with `setsid()` or `setpgid()` can survive. A sandboxed survivor keeps
its Seatbelt restrictions; trusted survivors retain full account access. The server's working
directory follows Pi's configured `cwd`, relative to the checkout; `~` in the
program, arguments, and working directory expands as in Pi.

In a sandboxed project the filesystem profile is the command profile:

- Read checkout contents (including ignored files), private scratch, the
  thread's build cache, system paths (`/System`, `/usr`, `/bin`, `/sbin`,
  `/Library/Developer`, `/private/var/db`, installed Xcode, and `/opt/homebrew`),
  the resolved Node and host executables and Apple/Rust toolchains, Cargo source caches,
  discovered skill folders, and a linked worktree's shared Git directory.
  File metadata may be read anywhere; other file contents remain denied.
- Write only private scratch, that thread's Cargo/target cache, and `/dev/null`.
  Checkout and Git writes remain denied, as do outside files and sibling caches.
  Cache-root removal and permission changes are denied. HOME and TMPDIR stay
  private even when the configuration supplies those environment keys.
- MCP additionally permits IP networking, including loopback. Unix-domain sockets
  remain confined to private scratch, with one exact outbound exception for the
  system resolver: `/private/var/run/mDNSResponder`. There is no broad Unix-socket
  or Mach-service permission; the host socket and other sockets outside scratch
  remain denied. Commands retain their existing no-network profile.
- Configured loader variables (`DYLD_*`, `LD_*`, `LDR_*`, `LIBPATH`, `SHLIB_PATH`,
  `GCONV_PATH`, and Node/Python/Ruby/Perl or shell startup injection variables)
  are refused in sandboxed server environments and listed as problems.

Trusted stdio servers have the user's login-shell environment and full account
access. The run shares its existing lazy login-environment lookup with commands.
Thread-owned files are checked before and reconciled after trusted calls.
Configured `${VAR}`, `$VAR`, escaping (`$$`, `$!`), and `!command` values in env
and headers resolve only on first search, against the policy's server environment,
never against the sidecar's environment or host-private plumbing. All eligible
entries finish credential preparation before any server connects or offers tools.
A failed entry remains a problem while the other entries continue. Credential
commands run under a
guardian and the same project policy, with a 10-second deadline, and produce no
transcript or command log. Credential redaction tracks only configured env/header
values after expansion, credential-command results, URL userinfo, and the token
part of Authorization values. It does not treat inherited login environment,
arguments, URL paths, queries, or entire URLs as secrets. Values shorter than
eight characters are not scrubbed; this is a deliberate limit to avoid corrupting
ordinary output. Descriptions, results, stored output, and diagnostics are scrubbed.
A call containing a tracked credential in its arguments is refused before
approval or dispatch with a fixed message. Approval details and displayed tool
inputs/outputs escape Unicode format, directional, zero-width, line/paragraph
separator, and control characters as visible `\uXXXX` sequences; ordinary tabs
and newlines in output remain readable. This applies to command approvals too.
Configured executable paths containing control or format characters are invalid.
The project's configuration file is opened without following symlinks; on macOS
this includes every path component within the canonical checkout. Names and
schema keys are never rewritten: a tool
containing a known credential in its name, title, schema key, or non-description
schema constraint is refused. HTTP uses the configured endpoint
with redirects and authentication recovery disabled; it starts no server process.

**Bounds and recovery.** Each server has 10 seconds of active startup work,
shared by credential preparation and connection/tool listing. The wait at the
credential barrier is excluded. Preparation and connection each run in groups
of four; for N silent servers with immediate credentials the wait is about
`10 × ceil(N / 4)` seconds. Mixed slow credential preparation and connections
can take up to `20 × ceil(N / 4)` seconds, plus shutdown/filesystem/scheduling
overhead. There is no separate whole-search deadline. Calls default to 60 seconds;
a positive configured `timeout` overrides it, capped at 15 minutes.

Listing is bounded to 500 tools, 32 pages, and 1 MiB of metadata per server.
JSON is checked iteratively before serialization or copying, with a depth limit
of 64 levels; each tool stays within 64 KiB and call arguments within 16 KiB.
Each wire message is bounded to 16 MiB. Each stdio pipe has token buckets of
32 MiB replenished at 32 MiB per second, and 2,048 stdout lines replenished at
512 lines per second. The larger line burst permits progress and log notifications
during ordinary calls while bounding sustained parsing work. Stdout and
stderr both spend the byte budget, and all stdout lines spend the message budget.
Exceeding either closes the pipe and fails that server before parsing more output.
Text and structured results use the existing `CommandOutputs` store (16 MiB per output,
32 newest outputs per thread), with a 12 KB tail preview and `read_output`
continuations. Non-text content is omitted with an explicit notice; message
images are a separate milestone. Sampling and elicitation have no handlers and
receive method-not-found replies.

Both search and calls are registered as unsafe to replay. Failed connections and
calls are never retried. After a host restart the interrupted call is shown as
interrupted; a new submission is required for new work. No protocol version was
changed.

Display labels are stored only when a tool is first called in a thread, never
for every offered tool at discovery. Repeated calls reuse the stored label.

## Safety boundary

Command rules in this section describe the opt-in `sandboxed` policy. A
project left at `trusted`, the default, replaces them as described in
[Command policy](#command-policy). File-tool rules apply to both.

Load checkout `AGENTS.md` explicitly before execution, after the user-level
context file and before the skills list described in
[Skills and user instructions](#skills-and-user-instructions). A thread's checkout is
immutable. Its model and thinking level change only through the version 3
`configure_thread`, never during a run; changing the default affects only new threads.
Edits and commands need explicit approval unless the run was submitted with
automatic approval. Either way, protect pre-existing dirty paths,
including untracked and ignored paths. File descriptors are checked against
saved pre-run device/inode identities; case and Unicode aliases cannot bypass
these checks. New outside changes are checked again before each edit.

A private per-thread postimage record permits follow-up edits to this thread's
own unchanged files. The Rust helper returns the canonical descriptor path,
device/inode, SHA-256 of the exact bytes written, size, mode, mtime, and ctime
after the atomic replacement. It does not include atime. The host commits this
record with SQLite FULL durability before tool completion. A later run compares
the current descriptor fingerprint with that exact record. Only a full match
from the same thread can be removed from the protected dirty set. Ignored paths
and parents, external changes, another thread's files, root replacements, and
missing records remain protected. A crash after an effect but before its record
never grants ownership. No ownership is inferred from an unrelated later read. Never reset, stash, or overwrite these paths. Reject
symlinks, parent traversal, special files, Git metadata, and filesystem escapes.
Sandboxed command execution uses the macOS filesystem sandbox; Linux refuses
the sandboxed policy and supports trusted commands.
Commands READ the checkout and WRITE ONLY private scratch and the current
thread's private build cache. File edit/write tools are the sole checkout
writers. HOME and TMPDIR use a new mode-0700 directory under `/private/tmp` for each
command. The short path leaves room for native Unix socket names.
CARGO_HOME and CARGO_TARGET_DIR stay in `STORE/build-cache/THREAD_ID/{cargo,target}`
so later commands reuse compiled dependencies. Cache directories must be
canonical, owner-only directories with no symlink traversal; the sandbox cannot
rename or change the mode/owner of their roots. No other thread cache, host store,
credentials, or general home access is granted. The host resolves store aliases
such as `/tmp` once, after it rejects a symlink store leaf.

Installed Rust toolchains, shared Cargo registry/Git source caches, and Xcode
remain read-only. No user Cargo configuration or Pi authentication is copied.
Fixed native `xcrun --find` queries resolve installed Apple Metal tools before
execution; a scratch-local adapter handles only the pinned GPUI Metal calls.
Other forms run stock xcrun inside the sandbox. No license is changed and no
Mach service is granted. Git uses the installed Xcode binary, not its system
license-discovery stub. Cargo is offline. Builds must use an existing lockfile
(`cargo test --offline --locked`); use an approved file tool to create a missing
lockfile. Tests that require checkout writes fail. Dirty sources can be tested
safely, including this run's approved edits.

Tests may create Git repositories and Unix sockets only in disposable scratch.
Anonymous socket pairs also work. TCP, outside Unix sockets, real checkout/Git
writes, source-cache writes, and unlisted Mach services remain denied. Commands
can signal only processes in the same sandbox, not the host or outside processes.
Each command has a 15-minute deadline; Stop remains available during a build.
Caches have no automatic eviction or size quota. To discard one, stop the host
and remove only that thread's `STORE/build-cache/THREAD_ID` directory.

Commands must not daemonize or start background services. A Rust guardian kills
the normal shell process group on Stop, host/Node death, timeout, or shell exit,
then checks that the group no longer exists before issuing a private cleanup receipt.
An intentionally detached descendant can evade group cleanup, but still cannot
write the checkout or outside that command's scratch and thread cache. This is
not a general process container.
The guardian holds the store lock and both checkout leases (writer and
lifecycle) until it exits, and marks every inherited descriptor close-on-exec
before it starts a command, so no command holds them.

Checkout exclusion uses three OS `flock` layers, always taken in this order and
never converted in place:

1. **Checkout lifecycle lock**: `checkout-<dev>-<ino>.lock` in
   `~/.local/share/plait-leases`, keyed by the canonical root's device and
   inode. The directory must be the user's own, mode `0700`; each lock file
   must be the user's own, mode `0600`, a regular file with one link, opened
   without following symlinks. It does not depend on the host store or
   `TMPDIR`. Runs take it shared. Desktop Git writes, turn undo and redo, and
   worktree removal take it exclusive. Creating a worktree takes it shared on
   the checkout it starts from, so it can proceed beside a run there. Lock
   files are never removed: a holder may still use one. A missing lock file
   is created with `O_EXCL` after an open finds none, so callers that first
   use a new lease directory together all get their leases.
2. **Checkout writer lock**: `flock(LOCK_EX)` on the canonical root directory
   itself, as hosts did before the lifecycle lock, so an older host is still
   excluded. Runs, turn undo and redo, desktop Git writes and worktree removal
   take it. A future enforced reader would not.
3. **Shared Git metadata lock**: `common-<dev>-<ino>.lock` in the same
   directory, keyed by the repository's common Git directory. It is taken
   exclusive, waiting at most five seconds, around writes to refs, upstream
   configuration or the worktree list (commit, push, publish, fetch, pull,
   creating or switching a branch, adding or removing a worktree), so linked
   worktrees of one repository take turns even though their checkouts differ.
   Unrelated repositories never wait for each other.

The checkout and the common directory are found from the root's `.git` entry
(`gitdir:` and `commondir`, as Git reads them), asking Git only when the path
is not a checkout root. Symlinks, `/private` and other aliases, a different
letter case on a case-insensitive volume, and subdirectories all resolve to the
same root directory, so they share one lease.

A lease is held by the runtime and by the helpers and command guardians it
starts with the descriptors; a helper's own Git processes inherit them. In the
desktop, `git worktree add` and `git worktree remove` hold the lease too
(`CheckoutLease::lend_to`). Other desktop Git writes do not lend their lock to
hooks or credential helpers that can detach. Instead, every writing Git child
records its owner and process-group birth identity before executing Git. A
surviving child after a desktop crash therefore remains excluded by its saved
cleanup record. When every descriptor holder exits, the kernel releases its
OS locks; the cleanup records are checked separately before admission.

Harness processes do not hold lease descriptors: Codex's shared app-server,
Claude Code, and their descendants cannot retain those OS locks themselves.
The run's durable marker bridges that gap when their turn or process cleanup
cannot be confirmed, including after runtime death. A Pi command or external
harness that deliberately detaches a writer remains the explicit limit
in the cleanup contract below.

All shipped runs are writers: one active run per checkout, with no default
count limit across independent checkouts. No reader run ships; the reader class exists in
the lease layer for later stages. Editors and other programs outside Plait do
not take these locks.
File edits use a synced temporary file and atomic rename, with original mode and
identity/content checks before replacement. The original is opened read-only:
a write-capable open can change macOS provenance metadata before any write. Root replacement is rejected by its
saved device/inode identity. Concurrent hostile filesystem mutation by another
same-user process is outside this local trust boundary. Stop kills
command process groups and aborts the model. No interrupted effect is replayed. Acceptance is a durable journal commit before
execution: a crash in the admission/start gap leaves `interrupted`, even if the
input never reached Pi Durable. The same acceptance transaction saves the full
original input in a private journal and its bounded user message in the snapshot.
Request-derived message IDs remain stable when Pi later places the input. Failed
preflight and interrupted admissions remain visible in conversation order; they
are not silently inserted into the model context. Retrying the same ID confirms
acceptance but does not run it; inspect the snapshot and submit a new explicit
request.

Read tools and approved commands can read ignored files inside the chosen
checkout. Their contents can be sent to the provider. Keep secrets outside the
checkout, or do not give the agent tasks that require those files. This host does
not provide a secret-file filter. New files use private mode 0600.

Tests must use disposable repositories and a scripted provider before any live
model integration. Scripted data is test-only and never appears as real catalog
or transcript data in the production host.

## Verification

Use `bash scripts/check.sh` for the shared macOS/Linux development checks.
`bash scripts/check.sh --require-command-sandbox` requires full command-sandbox
coverage and currently fails on Linux. Runtime `npm test` labels its platform
coverage; Linux runs common daemon/file tests and trusted execution and sandbox-denial tests.
See [Verification](VERIFICATION.md) for versioned results and
[Linux containment](LINUX-SANDBOX.md) for the open backend decision. A Linux
development pass is not full host-execution acceptance.

Use [Contributing](../CONTRIBUTING.md#required-checks) for focused commands and
[manual release acceptance](VERIFICATION.md#scenes-and-native-checks) for native
and live-model checks. Do not substitute scripted lifecycle fixtures for the
actual platform containment tests.

## Cleanup confirmation and quarantine

Stop revokes approvals immediately. Completion of a request, an SDK `close()`
call, a signal being sent, and an elapsed timeout are not evidence that the
processes stopped. Plait checks the process groups it starts and retains a
checkout-scoped cleanup obligation until it has positive evidence. Ordinary
completion and a clean Stop need no additional action.

Cleanup receipts use a private inherited descriptor, explicitly enabled by
`PLAIT_CLEANUP_RECEIPT_FD=8` when the runtime launches a guardian or record
helper. Without that marker, an open descriptor 8 is never treated as a receipt
channel. The descriptor is close-on-exec and the marker is removed before
launching commands, servers, or record workers.

| Path | Ordinary finish | Stop or timeout | Harness/helper crash | Host restart mid-run |
| --- | --- | --- | --- | --- |
| Pi model and file tools | Pi closes; an admitted atomic file helper exits; recording finishes | Authorization is revoked; Pi closes; any admitted atomic edit finishes before release | A closed helper has no writer descendants; failed cleanup remains blocked | Durable marker blocks the checkout; no effect is replayed |
| Pi trusted command | Guardian kills the shell's remaining group, waits for absence, and acknowledges | EOF on the lifetime pipe triggers group kill; the 15-minute deadline uses the same path; a child still alive after the exit grace leaves cleanup unconfirmed | Known group is checked after guardian exit; missing or unverifiable group information remains blocked | Guardian normally kills the group on Node death, but a new host cannot recover its private receipt; confirmation is required |
| Pi sandboxed command | Same group verification; descendants remain unable to write the checkout even if detached | Same lifetime/deadline path | Same verification; containment does not turn a missing receipt into an exit claim | Same durable exclusion; sandbox policy still restricts surviving descendants |
| Pi stdio MCP | Guardian checks the server group when its run closes | Revocation closes the lifetime pipe; group is killed and checked | Missing confirmation blocks the checkout | Durable marker survives; external HTTP service effects are not local process-exit proof |
| Codex | A terminal notification confirms the turn ended; the shared app-server can remain for other checkouts | Interrupt is requested; after 10 seconds without a terminal notification the checkout is blocked, without killing another checkout's shared server | App-server exit kills and checks its normal process group before satisfying cleanup | Marker blocks the checkout; no attempt is made to identify an old server by a reused PID |
| Claude Code | Session is closed; actual child exit and normal process-group absence are observed through the pinned SDK's spawn hook | Interrupt is requested; after 10 seconds the session closes; a close without observed exit remains blocked | Normal group is killed and checked after process exit | Marker blocks the checkout; an SDK close from the old host is not evidence |
| Checkout record and undo helpers | The supervisor verifies its worker group disappeared | The bounded deadline kills the group; an absent receipt remains uncertain | Worker lifetime EOF kills its group; known-group absence supplies proof | In-flight run/undo markers block new mutations; recovery does not replay an undo |
| Desktop Git writes | A normal Git exit clears its child record once the recorded group is gone; a hook job or helper still in that group keeps it until it exits | A timed-out or signalled writer retains its record until the group is observed absent | No new run or desktop write starts beside a surviving recorded group | A desktop crash leaves the child record; a later host checks the recorded group, without relying on the old desktop's locks |
| Lent worktree Git child | Normal completion clears its child record and the caller releases its lease | Same group evidence; inherited locks also retain exclusion | A child can keep its lent lease after the lender dies; its recorded group is shown | The stop action can reach a verified recorded group without acquiring the checkout lease it holds |

The precise boundary is cooperating Plait processes and the normal process
groups it supervises. Codex's shared-server protocol provides a terminal
acknowledgement for each turn, not a per-turn descendant inventory. Trusted
commands and external harnesses can deliberately detach writers into another
process group; Plait cannot prove those exited. Such writers remain an explicit
limit, as do unrelated editors and commands. A normal acknowledged Git exit
clears its record only once Git's process group is gone. A hook's background
job or a helper that stays in that group keeps the checkout blocked until it
exits, then the block clears by itself; Git's credential cache daemon is one
such helper and lives until its cache timeout (15 minutes by default). Hooks
and helpers that leave the group on purpose (Git's detached maintenance, for
example) remain outside what Plait can see. Sandboxed command descendants
retain Seatbelt restrictions even after detaching: they cannot write the
checkout, although Plait does not claim they all exited. This is not a general
process container. Tests use scripted harnesses and processes; they do not
establish the behavior of every installed harness, hook, or extension.

Before a run or undo starts, the host synchronously saves a private marker keyed
by the checkout directory's device and inode, beside the shared lease files.
Every new run and desktop Git mutation checks that marker under its OS lease.
A clean run removes it only after cleanup and the after record. An uncertain
run updates its reason, releases its OS handles, and retains exclusion through
the durable marker (quarantine). A host/store restart cannot erase it. A separate runtime-owned liveness
lock permits the existing parent lifecycle reader during a live run, and an
undo holds it too, so neither reads as a host that ended. Helpers
never inherit that lock, so its loss makes an abandoned crash marker block
readers as well; an uncertain marker always blocks them. Other checkouts
continue normally. Use current Plait versions for all cooperating
hosts and desktops; older versions do not understand these markers.

A late terminal acknowledgement, observed session exit, or proof that the
known group is absent clears that run's marker automatically. A failed probe,
missing helper receipt, or restart without transferable proof needs the person
to check for remaining commands, servers, and builds, stop them, and explicitly
confirm. Confirmation does not kill processes and cannot override a still-held
OS lease. Tokens prevent a stale confirmation or callback from clearing a newer
run's marker. A damaged marker fails closed and can be explicitly confirmed.
After a quarantine is resolved, any missing after record is marked
`after_restart`, since its observation includes the uncertain interval. Held
inputs are not automatically replayed by a confirmation.

Desktop Git uses a separate fixed-size crash record beside the locks, so a
worktree command on a parent can coexist with that parent's run marker. The
record contains the owner and child PID, their kernel birth identities, the
boot identity where needed, and a short Git command label. The desktop saves
the owner part before it starts Git and the child's identity right after, so
Git keeps the ordinary `posix_spawn` path instead of forking the whole desktop.
If the desktop dies between those two saves, the record names the command but
no process group: the checkout stays blocked and only the person can confirm
it, since Plait cannot check or stop a group it never learned. This adds no
subprocess or fsync: two small writes save the record and one truncate clears
an acknowledged completion. Git reads do not create child records. Power-loss durability is
required for run markers, not for these process-crash records.

A live desktop's record is hidden. After owner death or uncertain termination,
cleanup reads and admission checks clear it automatically only when the
recorded group is absent, or when the record comes from an earlier boot (the
kernel's boot ID on Linux, its boot session UUID on macOS). A changed leader birth
identity never authorizes a signal. The optional `process` field gives the
recorded `group`, `command`, and `can_stop` value. When `can_stop` is true, the
same control offers **Stop processes**. Its action rechecks the token, boot,
owner and leader identity, sends SIGTERM to that group, and waits up to two
seconds for absence. If the group remains, it sends SIGKILL to the same group
and checks for another two seconds: either the same leader still exists, or
the leader has exited and no process holds its PID. A live group's ID is never
reallocated, and Stop watched it without a gap since its verified SIGTERM, so
the survivors are still the recorded group. Any other process at that PID ends
the stop. Losing identity stops further signals; a
remaining group stays blocked, with no timeout-based clearance. If identity
cannot be proved, the control offers **Confirm stopped** instead and sends
nothing. The checkout's existing identity check also refuses a removed or
replaced folder; this is not a general process-management interface.

These owner-authenticated operations are additive and omit `protocol_version`:

```json
{"op":"checkout_cleanup","checkout":"/absolute/checkout"}
{"op":"resolve_checkout_cleanup","checkout":"/absolute/checkout","token":"TOKEN_FROM_READ","confirm_stopped":true}
{"op":"stop_checkout_cleanup","checkout":"/absolute/checkout","token":"GIT_TOKEN_FROM_READ","confirm_stop":true}
```

All return `{"checkout":"/canonical/checkout","cleanup":null}` when clear,
or `cleanup: {"token":"...","reason":"..."}` when unconfirmed, with the optional
`process: {"group":123,"command":"git commit","can_stop":true}` for a Git record. A live run's
or undo's crash marker is hidden while its runtime liveness lock is held, including when
read from another host store; an uncertain marker stays visible.
Unknown fields, relative paths, invalid tokens, and missing or false
`confirm_stopped` or `confirm_stop` are refused. Resolution acquires the exclusive checkout lease
and compares the token before clearing. Stopping a recorded Git group uses its
private record lock instead, since the group may hold the checkout lease.
A lock file or record that cannot be opened or read refuses that lease or
read with its own reason, never `cleanup_uncertain`, and holds nothing.
A quarantined admission returns
`cleanup_uncertain` with the specific reason; the desktop shows **Stop
unconfirmed**, then **Confirm stopped** when the person chooses that control.

## Checkout records

The runtime records the checkout before and after every run in Pi, Codex,
and Claude Code, regardless of permission mode. A run includes its steering
messages and every harness turn inside it; a queued follow-up gets a separate
pair. The before record completes before a harness starts. The after record
is taken after harness cleanup, including on Stop and failure, before releasing
the checkout lease or draining the next input. The records themselves add no
client operations or snapshot changes. [Version 5](#protocol-version-5)
exposes them, and the desktop draws them at each run's closing line.

If the before record cannot be made, the run ends in `error` with
`record_failed` and its queue remains held. Versions 1 and 2 see the existing
`runtime_error` code with the same explanation. Nothing in that run reaches a
harness. A failed after record is stored as failed, so future undo cannot use
it. After a host crash, a missing after record waits while checkout cleanup is
unconfirmed. Resolving cleanup schedules recovery in the background under the
checkout lease. If another writer holds the lease, the next run
retries the recovery immediately before its own before record. Recovery is
marked `after_restart`: the recorded changes may include work done while the
host was down. Interrupted work is never resumed or repeated automatically.

### Contents and limits

A record is a Git commit whose tree contains working-tree bytes, executable
bits, and symlink targets. Clean filters, LFS conversion, textconv, external
diff, hooks, and line-ending conversion do not run. Private attributes override
the checkout's conversion attributes; copied index entries needing conversion
are rehashed. Split, sparse, assume-unchanged, skip-worktree, and autocrlf
indexes fall back to hashing tracked paths and the untracked listing. A
checkout without a first commit is supported. HEAD's commit and symbolic
branch are recorded separately; recording never moves them.

Tracked files and untracked files are included, with untracked directories
visited file by file. Limits use binary units: an individually ignored file
is included up to 1 MiB; ignored files together are limited to 2,000 files and
64 MiB per record. Any single file over 100 MiB is excluded. Ignored
directories, nested repositories, submodules, unsupported file types, names
that cannot be represented as UTF-8, and unreadable files are not covered.
Each omitted path or directory and its reason is stored, without truncating
the coverage list. A tracked exception inside an ignored directory remains
covered. Ignore rules and size limits are reconsidered on later records.
Files detected changing during capture are marked, and excluded from the
run's usable changed-path list. Recording is not an atomic filesystem snapshot;
a separate writer can change files while the recorder reads them.

The after record stores each changed path's before/after mode and object ID,
plus added/removed line counts (unknown for binaries). It also carries the
coverage exclusions from both sides. A file created and deleted within a run
has no net change. The host copies earlier content for changed paths into its
own object store; subsequent repository pruning cannot remove those copies.
Unchanged borrowed blobs are not a complete repository backup. Restoring
files, executable bits, and links is available in the version 5 host operation;
other mode bits, extended attributes, and hard-link relationships are not restored.
Git history and effects outside the checkout are not restorable this way.

### Store and read-only enforcement

Within the owner-only host store:

```text
host.sqlite                               records and record_paths tables
records/sha1/                             private Git object store and refs
records/sha256/                           separate SHA-256 object store
records/<format>/refs/records/<id>         one ref per record
records/<format>/worktrees/<checkout-key>/
  HEAD, index, commondir, gitdir           private Git bookkeeping
  exclude, plait-source.json, plait-ready  local excludes, origin, seed marker
  source-index, sharedindex.*              optional split-index read copies
```

The checkout key hashes its canonical path, device and inode. The private
linked-worktree directory points at the checkout for reads; nothing is added
to the person's Git worktree registry. Only the after commit has the run's
before commit as parent. Other run ordering, identity, state, recovery marker,
and time live in SQLite. SHA-1 and SHA-256 repositories never share objects.

The runtime starts the Rust host's internal `record` and `prune` helpers with
the store lock inherited; records also inherit the checkout lease. On macOS,
the helpers use the host's Seatbelt invocation machinery and a recorder profile
that denies network access and all writes outside the records directory and
`/dev/null`. The checkout, common Git directory, worktree Git directory, and
borrowed object directories remain read-only. Store and checkout overlap is
rejected before creating the records directory. Git reads use a cleared
environment, no optional locks on the person's repository, and disabled
fsmonitor, untracked cache, replacement objects, and hooks. Private object
writes have no alternates, preventing even Git's object-freshening timestamp
write in the person's store. Object copies stream through Git pack/unpack
commands from read-only alternates to the private store. Linux exercises this
same recording code but has no recorder sandbox; it is not macOS containment
evidence and does not establish sandboxed Linux agent commands.

Records of archived threads are retained too. Startup and daily pruning remove
pairs older than 30 days, waiting for active record helpers and holding back
new helpers. A pair expires together after its later record; active runs keep
their before record. Pruning removes refs, expired checkout caches and indexes,
rebuilds private packs, then deletes unreachable loose objects. Known repository
objects are read-only alternates during this work. Pruning failures are retried
at the next interval; they do not authorize repository writes. Stores can thus
retain expired data longer if pruning fails. No thread-deletion policy exists.

## Protocol version 5

Version 5 adds `turn_records`, `undo_turn`, and `records_revision` on snapshots.
It is negotiated per request. Versions 1–4 retain their reply shapes; an older
host rejects version 5 with `invalid_request`. Clients negotiate status from 5
downward, and must tell the person that undo needs a newer host when 5 is not
available. Undo requests are never retried at an older version. These
operations and their desktop controls shipped in v0.1.0-preview.15.

### Listing a thread's records

```json
{"op":"turn_records","protocol_version":5,"thread_id":"THREAD_ID","before":"CURSOR","limit":20}
```

`before` and `limit` are optional. The default limit is 20 record fragments,
maximum 50. Results are newest first, in the existing success envelope:

```json
{"records":[{"id":"RECORD_ID","request_ids":["INITIAL_REQUEST","STEERING_REQUEST"],"request_ids_offset":0,"request_ids_total":2,"kind":"turn","undoes":null,"state":"ready","head_moved":false,"after_restart":false,"paths":[{"path":"src/main.rs","change":"modified","added":3,"removed":1}],"paths_offset":0,"paths_total":1,"not_covered":[{"path":"build/","reason":"ignored_directory"}],"not_covered_offset":0,"not_covered_total":1,"created_at":1795000000000}],"before":null}
```

- `kind` is `turn` or `undo`. `undoes` names the record an undo targeted,
  otherwise null. IDs are the pair's later record; while a run has no later
  record yet, its earlier record ID stands in. Match conversation closing
  lines by `request_ids`, not by this provisional ID. IDs include the initial
  request and inputs actually placed as steering, read from durable transcript
  positions. A queued follow-up belongs to its own run. A turn's IDs end at the
  next turn's record, or at the next input stored as starting its own run (see
  [Which run a restart interrupts](#protocol-version-7)), so a run an older
  build started without a record does not join the earlier turn.
- `state` is `recording`, `ready`, `failed`, `no_effect`, or `undone`.
  `no_effect` is a failed undo whose terminal journal proves that no file changed.
  It remains listed for diagnosis and request deduplication, but clients must skip
  it when choosing the next Undo or Redo target and counting undo depth.
  An interrupted undo with uncertain effects is never classified this way.
  Such a failed request returns `undo_no_effect`; repeating its request ID never
  replays it, but a new request ID can retry the original target. Failed pairs cannot be
  undone. `undone` records that an undo was applied, including “Undo the rest”;
  it does not assert that every path currently equals the earlier tree. The
  safety checks run again on every new request, including for an `undone` row.
- `change` is `added`, `modified`, `deleted`, or `mode`. Binary line counts are
  null. `head_moved` compares HEAD and its symbolic ref across the pair; undo
  does not put either back. `after_restart` means the later capture happened
  after a restart and can include changes made while the host was down.
- Coverage reasons are `ignored_directory`, `ignored_large`, `ignored_limit`,
  `too_large`, `nested_repository`, `submodule`, `special`, `name`, `changing`,
  and `unreadable`. Directory entries end in `/`. Coverage comes from both
  records, not just the later tree. Absence from coverage is not evidence that
  an excluded file was absent on disk.
- Lists are complete through paging: each fragment includes at most 500 items
  in each list, with offsets and totals, and the response targets 800,000 bytes
  plus its small envelope, below the 1 MiB frame. A large record repeats its ID
  on successive pages. Append at the given offsets. `before` is null at the
  end; otherwise pass it unchanged. It contains a row position and offsets
  (`row:paths:coverage:requests`), not a timestamp. Restart listing when the
  snapshot's integer `records_revision` changes. Old cursors are not snapshot
  isolation, and retention can remove their records.

Version 5 `changed_files` returns up to 500 sorted unique paths from the union
of tool provenance and retained record paths, including command writes and
undo records. Its reply stays `{"files":["relative/path"]}`; versions 3 and 4
retain their tool-provenance-only list.

### Undoing a run or an undo

```json
{"op":"undo_turn","protocol_version":5,"thread_id":"THREAD_ID","record_id":"RECORD_ID","request_id":"CLIENT_UUID","leave_changed":true}
```

`leave_changed` is optional and defaults to false. The result is:

```json
{"state":"undone","changed_since":[],"changed_since_total":0,"not_covered":[],"not_covered_total":0,"record_id":"UNDO_RECORD_ID","after_restart":false}
```

A preflight conflict returns `state:"blocked"`, the target `record_id`, and
`changed_since`; it changes no working files. An applied result names the new
undo record, which can itself be undone. `after_restart` carries the target's
warning. Each reply list is a preview of at most 500 entries and shares the
800,000-byte budget; totals make omissions explicit. Record paths and coverage
can be read in full with `turn_records`; blocked-path previews have no separate
continuation operation. A client must display truncation, not imply it has
shown every blocked path.

The operation holds the checkout lease and returns `busy` if another host
writer, active run in any thread in that checkout, recording, or pruning is
in progress. The runtime supplies the complete stored path metadata through a private
manifest, outside the small helper request frame; it is never a client-supplied
path list. A fresh capture precedes validation. A path is eligible only if
its fresh entry (mode and object, or absence) equals the target's after entry,
its current bytes and executable bit still match, every ancestor is a real
directory or absent, it is not a nonempty directory, and its name does not
resolve through a different current spelling on the filesystem. The latter
uses actual directory names and the filesystem's name equivalence, covering
case-insensitive and Unicode aliases without changing the person's Git config.
Hard-linked current files are also refused: replacing a link changes metadata
of its siblings, which could include Git metadata.

If any path is blocked, the default applies nothing. With `leave_changed:true`,
only eligible paths are restored. Excluded coverage remains excluded. Created
files are removed deepest first; directories absent from the earlier tree are
removed only if empty. Other files are restored shallowest first using a
sibling temporary file and rename. Symlinks are read and created as links.
Descriptor-relative traversal never follows a symlink ancestor; root and parent
identities and file content are checked again immediately before each mutation.
Restoring an absent path uses an exclusive rename, so a newly appeared file
cannot be overwritten. Git's index, HEAD, refs, objects, and other metadata are
never written; on macOS the helper's sandbox explicitly denies these writes,
while permitting working-file and private-store writes.

The runtime gives the helper the remaining part of a 10-second operation
budget, within the sidecar's 20-second limit. Git commands also share a deadline.
A failure before applying is `undo_failed`, with no working files changed.
A late failure, timeout, or interrupted apply is `undo_incomplete`: some files
may already have been restored. Refresh records and inspect the checkout;
do not automatically issue another undo. A durable private journal holds U0
and the paths whose restores completed. Recovery takes a missing U1 under the
lease, marks it after restart, and limits the undo record to those completed
paths. A process death between a mutation and saving its completion can leave
that mutation outside the recovered list. It is not replayed.

Applied results are persisted by `(thread_id, request_id)`. Repeating the same
ID and arguments returns the saved result without writing files, even after a
restart. Different arguments with an applied or interrupted ID return
`conflict`. Blocked and preflight-failed requests do not retain deduplication
results and can be retried. Interrupted requests retain an uncertainty marker
and return `undo_incomplete` instead of replaying possible effects. These
markers and applied replies outlive the 30-day file-record retention.

Undo restores bytes, Git's executable bit, and symlink targets. It does not
restore other permissions, extended attributes, hard-link relationships,
empty-directory history, Git history, or effects outside the checkout. It is
not an atomic filesystem transaction: unrelated programs do not honor the host
lease. An external write or rename in the small interval between the final
check and replacing/removing an existing path can still race the mutation and
lose that external change. A writer can also change an already restored path
before U1 is captured. Detected races stop with `undo_incomplete`; these checks
do not establish a guarantee against arbitrary concurrent external writers or
power loss. Undo records exclude unrelated paths and failed restores, so their
redo does not claim those concurrent edits.


## Protocol version 6

Version 6 adds image attachments. Like version 5, it is negotiated per request:
ask for status at 6, then step down on `invalid_request`. Versions 1–5 keep
their request and reply shapes, omit image references and capability fields,
and reject image operations and the `submit.images` field. Text still works
with an older desktop or host. A desktop connected below 6 says **Update host**
before sending images; image operations must never be retried at an older
version. Storage migration is additive; downgrading the store is not supported.

A version 6 model in `status.models` or `harnesses` has `image_input`, one of
`yes`, `no`, or `unknown`. A thread snapshot has the same field for its selected
model. Pi reports its catalog's input modalities; Codex reports its app-server
model's `inputModalities`. The pinned Claude Agent SDK model catalog has no
image capability field, so Claude Code reports `unknown`. Missing Codex
modalities also mean `unknown`. A known `no` rejects submission with
`model_unavailable`; `unknown` permits it, and a harness refusal ends the run
with its usual safe error. The desktop has no separate model allowlist.

Upload original bytes before submitting, in consecutive chunks of at most
32,768 decoded bytes, encoded as canonical base64:

```json
{"op":"image_put","protocol_version":6,"thread_id":"THREAD_ID","image_id":"SHA256_HEX","offset":0,"total":1032,"data":"BASE64_CHUNK"}
```

`image_id` is the lowercase SHA-256 of the complete bytes. `total` is the
original byte count. The result is `{"offset":1032,"image":REFERENCE}` on
completion, or `{"offset":32768,"image":null}` while incomplete. The offset
is the saved length. Retrying an identical chunk is safe; changed bytes,
changed total, gaps, and a mismatched final hash are refused. The thread must
exist. No path from the client is accepted or opened.

A reference is `{"id":"SHA256_HEX","media_type":"image/png","size":1032,
"width":320,"height":180}`. `media_type` is `image/png` or `image/jpeg`,
determined from the bytes and their dimension header, never the filename.
The host does not decompress pixels. The desktop additionally decodes under
allocation and dimension limits before retaining an attachment.

```json
{"op":"submit","protocol_version":6,"thread_id":"THREAD_ID","request_id":"CLIENT_UUID","text":"Compare these screens","images":["SHA256_HEX"],"approval":"ask","when_busy":"follow_up"}
```

`images` is optional. With images, text may be empty. Every ID must name a
complete upload to this thread. Acceptance durably associates the IDs with
the request in the same transaction as its text and deduplication key. That
key includes the ordered image IDs: repeating the request with different
images returns `conflict`. Queued and steering inputs carry the same originals.
An upload alone never starts a run.

Version 6 snapshot and history user messages add optional `images:[REFERENCE]`.
They never carry base64 or pixel bytes. Fetch only when preparing a visible
thumbnail or larger view:

```json
{"op":"image_read","protocol_version":6,"thread_id":"THREAD_ID","image_id":"SHA256_HEX","offset":0}
```

The result is `{"image":REFERENCE,"offset":0,"data":"BASE64_CHUNK"}` with
at most 524,288 decoded bytes (512 KiB). Continue at the returned offset plus decoded
length, stopping at `size`. Offsets outside the image are refused. The desktop
checks every returned reference, offset, chunk length, and the completed hash.

### Limits and retention

Both sides enforce at most four different images, 5 MiB (5,242,880 bytes) per
image and total per message, 8,192 pixels per axis, and
16,777,216 pixels per image. Oversized images are refused with a reason; Plait
never recompresses or resizes the sent original. Thumbnails are separate.

These are conservative Plait limits, not a promise about every provider. All
three pinned harness interfaces accept PNG/JPEG content blocks. Five MiB of
raw bytes becomes 6,990,508 base64 bytes (at most 6,990,516 split across four
blocks). Adding 65,536 bytes for serialized message text, 4,096 bytes for
framing, and a newline gives 7,060,149 bytes, leaving 1,328,459 bytes under
Plait's 8 MiB Codex adapter line bound. Six MiB would fill that entire line
with base64 alone, so 5 MiB is the largest whole-MiB allowance with headroom.
The runtime's complete submitted JSON
request is actually limited to 65,000 bytes; these reserves are conservative.
Tests send a maximum image with maximum text through all three scripted
harnesses and check the resulting content and frame sizes.

A 32 KiB upload chunk becomes 43,692 base64 bytes, leaving room below the
unchanged 64 KiB host request limit. A 512 KiB download chunk becomes 699,052
base64 bytes, leaving about 341 KiB for metadata and framing below the 1 MiB
reply limit. A maximum-size original takes ten read requests. The image count,
dimensions, and per-image bound limit decode memory and concurrent work. The SDK schemas specify no shared provider
byte limit; a provider can still refuse an otherwise valid message.

Originals are BLOBs in `STORE/host.sqlite`, keyed by thread and hash, with
associations in `message_images`. The owner-only host store defaults to
`~/.local/share/plait`; images are never written to the checkout, repository,
or the checkout records used for Undo. They survive host restart. Accepted
images that entered the conversation last as long as the host store, including
after archive or stopping a run; removing the store removes them. Cancelling a
queued message (also through Stop) releases its associations. Images with no
remaining accepted owner become unattached, with a fresh 24-hour grace period
for Edit or retry recovery. Incomplete and unattached uploads expire
after 24 hours, pruned on startup and upload, with at most 16 per thread and
128 across the store. Harnesses may also retain content in their own native
conversation storage as part of their ordinary history.

Pi receives `{type:"image",mimeType,data}` alongside text. Codex receives
`{type:"image",url:"data:image/png;base64,..."}` in `turn/start` or `turn/steer`.
Claude Code receives Anthropic base64 image source blocks through the pinned
Agent SDK's streaming `SDKUserMessage` input. Image-only inputs contain no empty
text block in any of the three adapters. [Verification](VERIFICATION.md#known-gaps)
records scripted coverage and the absence of live-model acceptance evidence.

## Protocol version 7

Clients request status at version 7 and step down on `invalid_request`. At
version 7, each fragment of a `turn_records` turn record includes the same
optional `outcome`: `finished`, `failed`, `stopped`, or `interrupted`. It
describes how the run ended. The existing `state` describes checkout capture
and undo. The host advances `records_revision` when the outcome becomes visible.

Versions 5 and 6 omit the field. Version 7 also omits it for active runs,
undo records, and runs whose outcome is unknown, including runs completed
before this change. Only Git projects have checkout records, and the host
prunes them after 30 days. A turn without a retained record has no reported outcome.

**Which run a restart interrupts.** After a restart, the host closes the
unfinished run that owns the latest placed input as `interrupted`, and closes
any other unfinished run with no outcome. Each request row stores how its input
was used: `started` its own run, or `steered` into one.

- An input that started a run ends the turn before it, so an earlier run is
  never given its outcome.
- A steering input belongs to the nearest earlier input marked `started`, with
  or without checkout records, so plain folders resolve too.
- Codex and Claude Code store a steering placement before the thread's snapshot
  shows it. If the host stops in between, the snapshot still names an earlier
  input of that run: its own input or an earlier steer. Recovery accepts that
  when both resolve to the same run; if they resolve to different runs, no run
  gets the outcome.
- For the same reason the saved head can trail a stored placement. A message
  that starts a run is placed after the later of the two, so it ranks above
  that placement if the host stops again before its own placement.
- An input consumed before this column existed is unknown. For those rows the
  host infers as before: a direct admission, or a follow-up with a before
  record, started a turn, and other placed follow-ups count as steering.

Two triggers stored in the schema fill the column for every writer, including
an older build after a rollback. Every build writes the three cases the same way:

- A direct admission saves the thread, then inserts its text input with no
  state. Receipts such as thread creation use another scope and stay unmarked.
- Starting a queued follow-up clears its state without an entry.
- Placing a steering input clears its state and sets its entry.

Each trigger only fills an empty value on the row being written, so an older
build's writes still succeed. The host recreates both triggers each time it
opens the store, so an earlier definition does not persist.
A request still queued when the column was added is marked when it is consumed.
The host itself marks a steering input placed after it was cancelled, which
happens when a run took the message just before the cancellation.

Verification: tests use the exact request statements of older builds, before
and after the migration, including a follow-up queued across the upgrade. By
hand, the store code of the previous release (revision `efefb79`) steered a
message into, and separately started a follow-up after, a run in a store this
change had migrated. Its writes were marked `steered` and `started`, and this
build then gave the earlier run `interrupted` and no outcome respectively. That
check drove the store directly, not a running previous host. No build without
runs rows was run.

## Recorded turn review

These additive, read-only operations omit `protocol_version`. Versions 1–6
retain their existing shapes. An older host rejects the operation with
`invalid_request`; the desktop reports “Turn review needs a newer host.”
Other failures retain their own unavailable or offline explanation.

- `{"op":"turn_review_info","thread_id":"THREAD"}` returns
  `{schema:1, checkout, features, retention_days:30}`. `checkout` contains
  opaque `host_id`, `id`, and unsigned `catalog_revision` and `review_revision`.
  Features are `record_diff:true`, `attribution:false`, `stage_turn:false`,
  `since_review:false`. Review revision is zero until baselines are supported.
- `{"op":"turn_diff","thread_id":"THREAD","record_id":"RECORD","path":"src/a.rs"}`
  compares that exact record's saved before and after entries. The record may
  belong to another thread on the same saved checkout identity. It never uses
  today's status, index, or file content. Existing v5 `turn_records` pages
  supply the complete file list.
- `{"op":"turn_review_page","token":"TOKEN","offset":NEXT}` continues an
  immutable document. It returns `{token,offset,total,items,next_offset}`.
  Offsets must have been issued by the host; retries return identical content.
  Tokens work across sockets for the same authenticated host owner, expire
  after five idle minutes, and are lost on host restart. `expired_view` means
  refresh the view. Pages contain at most 500 items and 750,000 JSON bytes.
  The initial patch and hunk pages share the reply's 1 MiB transport budget
  after metadata and framing overhead; their smaller page limits persist
  for continuation requests and identical retries.

A diff reply has these keys (nullable keys are always present):

```text
schema: 1, checkout, view_id, path, side: null, basis: "record",
source_record_id, before, after, status, reason,
patch_bytes, patch: Page, hunks: Page,
review: {state:"unreviewed",revision:0,reviewed_at:null,expires_at:null,
         source_record_id:null,can_mark:false},
truncated, elapsed_ms
```

An entry is null for absence, otherwise `{mode,fingerprint,bytes}`. Modes are
`100644`, `100755`, or `120000`. Fingerprints are opaque. Status is `ready`,
`empty`, `binary`, `mode`, or `unavailable`. A missing blob is unavailable,
never an empty successful diff. Patch items are `{byte_offset,bytes_base64}`:
consecutive chunks of at most 64 KiB, preserving exact line endings.
`patch_bytes` counts decoded bytes, whereas page totals count items.

Hunk items contain `id`, `first_line`, `last_line`, `old_start`, `old_count`,
`new_start`, `new_count`, `origin`, `stageable:false`, and
`refusal:"unsupported"`. Line indices include raw file headers. Binary and
mode changes have a file-level descriptor. `origin` identifies the observed
record interval with `kind:"turn"`, `record_id`, `thread_id`, `request_id`,
nullable `title`, and `created_at` in host milliseconds. This does not establish
who wrote the change. The desktop draws no origin or review controls in this
stage. `reason` is null on success and a typed unavailable reason otherwise.

Requests are strictly validated and bounded to 65,000 bytes. Paths are relative
UTF-8, outside Git metadata, without traversal or control characters. Clients
supply no store paths, object IDs, or patches. The runtime checks saved root
identity and pins blobs against pruning while its Rust helper reads them.
Changed blobs remain readable even if borrowed unchanged trees were pruned.
The read helper does not open the recorder's private index or remove locks;
its only possible store write creates an empty blob for an absent side.
On macOS its sandbox denies checkout and Git metadata writes.

Read caches are bounded to 32 MiB per checkout and 128 MiB per host. A preview
at 512 KiB or 10,000 lines is limited. A byte-clipped preview ends on a complete
line and omits its unfinished trailing hunk. Empty file additions and deletions
are Ready; Mode requires two present sides with equal content. Binary follows
the record's Git line-count classification, so non-UTF-8 text stays text and its
raw bytes remain in the patch. Complete page validation and patch preparation
run off the UI thread. These operations provide no staging,
attribution proof, or reviewed baseline, and make no change to undo semantics.

## Workspace

The first step of [Host-first Plait](HOST-FIRST.md). These additive
operations omit `protocol_version`; an older host answers `invalid_request`.
The desktop reads its sidebar from this stream. With an older host it uses
this Mac's saved lists for this Mac, and only what the window opened for
another machine.

- `{"op":"workspace_view"}` returns `{schema:1, host, revision, projects,
  worktrees, seen, status}`.
  - `host` is `{id, name, platform}`. The ID is stable for the store and
    is the same one turn review reports. The name defaults to the machine's
    hostname, and `{"op":"host_set_name","name":N}` changes it.
  - `projects` lists `{path, hidden}` in the order they were opened. Hosts
    with `project_fingerprint_v1` add, when known, `vcs` (`git` or
    `folder`), `fingerprint` `{roots, remote}`, `branch`
    `{branch, named, tracking}` and `main_checkout` (a path), from the
    `checkout_summary` read below. Clients match copies of one project on
    several machines by shared `roots`; `remote` is only a hint, since it
    changes when an account or repository is renamed. The host reads at
    most two summaries at a time, walks root commits only when a project
    is opened or its roots are unknown, refreshes the branch when the
    checkout's refs change, and persists fingerprints so a restarted host
    serves them at once. A summary change moves the revision.
  - `worktrees` lists `{folder, main, branch, creation_id, identity}`:
    which project each worktree belongs to.
  - `status` maps watched checkout paths to in-memory change counters, starting
    at zero when a watch starts. The host watches visible projects and listed
    worktrees, up to 64 folders; failed watches are omitted. Counters signal
    that Git status may have changed and reset when the host restarts.
  - `seen` lists `{thread_id, updated_at, state, unread}`, newest first,
    with at most 1000 entries.
- `{"op":"workspace_subscribe"}` replies with the same view, then sends
  `{"event":"workspace","seq":N,"workspace":VIEW}` each time the revision or status
  changes, numbered from 1. A thread change alone is not a workspace change. A
  subscriber that cannot keep up is disconnected and subscribes again for a
  fresh view.
- Workspace writes accept an optional `op_id`: retries with the same ID and input return the first outcome for up to 24 hours.
- Writes, each committed with a new revision, then announced to subscribers:
  - `workspace_open_project {path}` accepts an accessible Git working tree
    or plain folder on this host, and returns its top folder. Opening a
    hidden project shows it again. A path that does not exist is refused
    with `checkout_missing`, so a client can offer to choose the project
    again; other inaccessible paths are `invalid_request`. Checkout-taking
    operations that resolve a project, such as `command_policy` and
    `agent_resources`, refuse a missing path the same way.
  - `workspace_hide_project {path, hidden}`.
  - `workspace_set_worktree {folder, main, branch, creation_id?, identity?}`
    and `workspace_forget_worktree {folder}`. The host also records a
    worktree it creates itself, and forgets it when it removes it.
  - `workspace_mark_seen {entries}` takes up to 1000 entries. An entry older
    than the one kept for a thread is ignored, so a late write never marks a
    thread unread again.
  - `workspace_import {device, projects, hidden, worktrees, seen}` merges a
    device's lists once. A repeat from the same device changes nothing and
    returns `imported:false`.
- Paths are absolute, at most 4096 bytes, without control characters; IDs
  match `[A-Za-z0-9_-]{1,128}`; a request is at most 65,000 bytes.

## Device state

The additive `device_state_get {device}` and `device_state_set {device, revision,
state}` operations store an opaque JSON object per device. They omit
`protocol_version`. A missing device returns revision 0 and null state. A set
succeeds only at the current revision and returns the next revision; a stale
write returns `conflict`, so the client can reread and retry. Device IDs match
`[A-Za-z0-9_-]{1,128}` and serialized state is limited to 60,000 bytes. The
host retains at most 64 devices, evicting the least recently updated on insertion.
The desktop uses these operations for per-device tabs, layout, and selection.

## Folders

The additive read-only `browse_folder {path?, hidden?}` operation omits
`protocol_version`. An omitted path starts at the host user's home; a supplied
path must be absolute, at most 4096 bytes, and contain no control characters.
The reply contains the resolved `path`, its `parent` (null at the root), `home`,
a `git` marker, sorted subfolder `entries` with their own Git markers, and
`truncated` when more than 1000 folders match. Hidden names are omitted unless
`hidden` is true. Folder symlinks are included by their link names. Git markers
check for a `.git` file or directory without running Git; checks stop after two
seconds and remaining markers are false. Missing, unreadable, and non-directory
paths return `not_found`, `permission_denied`, and `invalid_request`, respectively.

The `checkout_cleanup`, `command_policy`, `agent_resources`, `create_thread`,
and `list_threads` requests also accept an optional `identity` object with
string `dev` and `ino`. When present, a mismatch returns `invalid_request`
with “This folder changed since it was opened.” Omitting it preserves the
previous behavior. `list_threads` requires a checkout when identity is given.

## Checkout Git reads for remote desktops

These additive, read-only operations omit `protocol_version`. They let a
desktop read a checkout on its host; see
[Remote hosts](REMOTE.md#working-remotely). An older host rejects them with
`invalid_request`, which the client reports as "Update this machine's host to
see its Git."

- `{"op":"git_read","checkout":B64,"identity":null|{"dev":"D","ino":"I"},"read":KIND,...}`
  runs one read. `checkout` is base64 of the absolute path's bytes. `KIND`
  and its fields are `status`, `files`, `branches`, `diff` (`change`,
  `mode`), `log` (`tip`, `skip`, `limit`), `show` (`id`), `line_counts`
  (`paths`), `graph` (`heads`), `file` (`path`), `file_stamp` (`path`),
  `checkout_summary` (`roots`, a boolean), and
  `worktree_size` (`worktree`, `worktree_identity`, `thread`, `removed`,
  `others`, `build_directories`). For `worktree_size` the runtime adds the
  host store, so a thread's retained build output is measured there; a
  client cannot send a store, and a thread must be an id.
  `main_checkout` returns the base64 path of the first worktree, including
  for an unborn repository. `identity` returns `{root, identity}` with the
  canonical root as a base64 path and its device/inode identity, using no Git
  process. `main_checkout` currently requires a UTF-8 checkout path.
  `checkout_summary` returns `{vcs, fingerprint, branch, main_checkout}`
  from refs alone, never the working tree. `fingerprint` is null when
  `roots` is false or for a plain folder; its `roots` are the sorted root
  commits reachable from HEAD (at most 32, empty when unborn) and its
  `remote` is the origin (or first) remote normalized to `host/path`
  without credentials, port or `.git`, or null for local remotes.
  Any other field is refused.
- The reply is `{total, data, token}`: `data` is base64 of the first part of
  the serialized outcome, and `token` is null when `data` is all of it.
  `{"op":"git_read_page","token":T,"offset":N}` returns the part starting at
  byte `N`. Pages are at most 600 KiB before encoding. Tokens expire two
  minutes after their last use, at most four are kept, and the last page ends
  one. An expired token is `not_found`.
- The outcome is `{"ok":{"identity":{dev,ino},"result":R}}`, with the same
  result the desktop's local read returns, or `{"error":{"message":M}}` with
  the same message. Paths inside results are base64 of their bytes.
- Except for `main_checkout` and `identity`, the `plait-host git-read` helper runs the read with the desktop's hardened
  Git builder and limits. It refuses a relative path, a folder that is not
  the top of its work tree, and a checkout whose inode differs from
  `identity` or that changes during the read. A different device number with
  the same inode is accepted only for the canonical path, since macOS can
  renumber a volume after a restart; the outcome's `identity` is then the
  current one, and clients store it. Files are opened beneath the
  checkout without following symlinks. A file over 8 MiB returns only its
  stamp.
- Results over 64 MiB, a helper that fails, and a read over 60 seconds are
  refused without helper diagnostics.
- `{"op":"git_write","checkout":B64,"identity":{"dev":"D","ino":"I"},"write":OP}` accepts an optional `op_id` to replay the first outcome for up to 24 hours.
  runs one of the desktop's agreed writes (`WriteOp`: stage, unstage, stage
  or unstage all, a hunk, commit, push, publish, fetch, pull, discard,
  restore, create or switch branch) through `plait-host git-write`. The
  identity is required. The checkout must be the top of its work tree, and
  the write takes the same checkout leases as a desktop write on this
  machine, so it is refused while a thread writes there. The reply has the
  same shape, with the write's outcome. A discard's backup is a blob in the
  repository's object store, so `restore` works on the host unchanged.
- A write runs with the host's environment: its Git configuration, hooks,
  credential helpers, and `SSH_AUTH_SOCK` if the host has one. Prompts are
  off, so a missing credential fails with Git's own message. A write is
  stopped after 240 seconds; a helper that fails reports only "The host could
  not finish this change. Refresh to see the checkout as it is."

## Linear

These additive host operations let the desktop manage Linear sign-in, read
bounded team and issue data through a local SQLite cache, persist local
thread-to-issue links, and perform explicit writes: comments, workflow
state, and creating and editing issues. No harness or agent receives a Linear tool; writes come only from explicit requests on the host socket, which any process running as you can reach. Like
[Recorded turn review](#recorded-turn-review), they omit `protocol_version`:
versions 1–6 retain their existing request, `ThreadSummary`, and `Snapshot`
shapes unchanged. New result fields and typed error fields apply only to Linear
operations, which the desktop calls only from Settings → Linear for sign-in;
existing desktop reply
shapes remain unchanged, so no protocol version bump is required. An older host rejects these operation names with
`invalid_request`. Unknown fields (including `protocol_version`) are refused.

No harness or agent receives a Linear tool, and nothing runs in the background
or polls Linear on a timer. Requests come only from explicit requests on the
owner-only host socket, which any process running as you can reach.

### What crosses the socket and where the key lives

- **Desktop to host, once:** a personal or restricted Linear API key in
  `linear_set_key`. The host validates it with one identity query (`viewer` and
  `organization`) against `https://api.linear.app/graphql` before saving it.
  If Linear rejects the key or cannot be reached, nothing is written.
- **Credential storage:** the key and non-secret workspace/viewer identity live
  in `STORE/linear-auth.json` with mode `0600`, written atomically through a
  sibling temporary file and synced rename. Symlinks are refused, and loosened
  permissions are restored to `0600` on read. The key is never written to
  `host.sqlite`. `linear_sign_out` deletes `linear-auth.json` and clears all
  Linear caches, project bindings, and full write receipts; retry tombstones and
  local thread links remain. Switching to a key for another workspace likewise clears caches, project bindings, and full
  receipts so data from a previous workspace is never served as live. Changing
  a key in the same workspace clears caches and full receipts too, but retains
  project bindings. Every key change retains retry tombstones.
- **Host to desktop, never:** the API key, credential file paths, raw Linear
  HTTP bodies, or raw Linear response headers. Error messages are fixed host
  text; validation errors name the field and never repeat its value.
- **Transport hardening:** the client calls only `https://api.linear.app/graphql`
  (overridable only via a constructor argument in tests, never from environment
  variables, settings, or protocol fields) using `Authorization: <key>` (no
  `Bearer` prefix for API keys), `redirect: 'error'`, a 10-second deadline, a
  2 MiB response body cap, and fixed GraphQL documents with variables. No
  attachment URL or link inside an issue is ever fetched. Every string from
  Linear passes display escaping (`escapeDisplay`) and field-length bounds
  before leaving the host.

### Operations

```json
{"op":"linear_info"}
{"op":"linear_get_write","request_id":"CLIENT_UUID"}
{"op":"linear_auth_status"}
{"op":"linear_set_key","key":"SECRET"}
{"op":"linear_sign_out"}
{"op":"linear_list_teams","refresh":false}
{"op":"linear_bind_project","checkout":"/absolute/checkout","team_id":"TEAM_ID","team_key":"PLA","team_name":"Plait"}
{"op":"linear_get_project_binding","checkout":"/absolute/checkout"}
{"op":"linear_list_issues","team_id":"TEAM_ID","filter":"active","search":"retry","cursor":"CURSOR","refresh":false}
{"op":"linear_get_issue","issue_id":"ISSUE_ID","comments_cursor":"CURSOR","refresh":false}
{"op":"linear_resolve_issue","checkout":"/absolute/checkout","reference":"PLA-142","refresh":false}
{"op":"linear_get_linked_issue","thread_id":"THREAD_ID","refresh":false}
{"op":"linear_link_thread","thread_id":"THREAD_ID","workspace_id":"ORG_ID","issue_id":"ISSUE_ID","identifier":"PLA-142","url":"https://linear.app/acme/issue/PLA-142","title":"Fix retry backoff"}
{"op":"linear_unlink_thread","thread_id":"THREAD_ID"}
{"op":"linear_get_thread_link","thread_id":"THREAD_ID"}
{"op":"linear_list_thread_links","limit":200,"cursor":"1763400000000:THREAD_ID"}
{"op":"linear_post_comment","issue_id":"ISSUE_ID","body":"Comment text","request_id":"CLIENT_UUID"}
{"op":"linear_update_issue_state","issue_id":"ISSUE_ID","state_id":"STATE_ID","request_id":"CLIENT_UUID"}
{"op":"linear_create_issue","team_id":"TEAM_ID","title":"Retry 429s","description":"Markdown","state_id":"STATE_ID","request_id":"CLIENT_UUID"}
{"op":"linear_update_issue","issue_id":"ISSUE_ID","expected_updated_at":"2026-11-18T12:00:00.000Z","title":"New title","description":"Markdown","request_id":"CLIENT_UUID"}
{"op":"linear_read_image","url":"https://uploads.linear.app/ORG/FILE","offset":0}
{"op":"linear_get_issue_text","issue_id":"ISSUE_ID"}
```

- **`linear_info`** is local and read-only. Returns
  `{"schema":1,"features":{"durable_write_outcomes":true,"exact_issue_resolution":true,"workspace_links":true,"typed_rate_limits":true,"issue_writes":true,"images":true},"write_history_limit":10000,"write_history_retention":"store_lifetime"}`.
  `issue_writes` and `images` are absent from older hosts; clients treat absence as false.
  Clients gate write UI on `features.durable_write_outcomes`, not a release
  number. Older hosts reject this operation with `invalid_request`.
- **`linear_get_write`** is local and read-only, including while signed out.
  Takes `request_id` and returns `{"request_id":"CLIENT_UUID","write":null}`
  if no retained record exists, otherwise `write` contains `request_id`, `op`,
  `workspace_id`, `issue_id`, `outcome`, `receipt`, `created_at`, and `updated_at`
  (host milliseconds). Outcomes are `not_sent`, `rejected`, `accepted`, or
  `unknown`. `receipt` is null except for accepted writes; it has the normal
  accepted reply fields with `issue:null` (historical, never live state).
  No body, state payload, hash, or credential is returned. Records migrated
  from an older host have null workspace/issue IDs. Absence is not proof that
  an older host never wrote: previously evicted receipts cannot be recovered.
- **`linear_auth_status`, `linear_set_key`, `linear_sign_out`** return:
  ```json
  {"signed_in":true,"status":"signed_in","workspace_id":"ORG_ID","workspace_name":"Acme","workspace_url_key":"acme","viewer_id":"USER_ID","viewer_name":"Alex"}
  ```
  `status` is `signed_out`, `signed_in`, or `key_rejected`. When signed out,
  `signed_in` is `false` and metadata fields are `null`. `key` must be
  printable ASCII without whitespace or control characters, at most 4,096 bytes.
- **`linear_list_teams`** (`refresh` optional, default `false`) returns
  `{"teams":[{"id":"TEAM_ID","key":"PLA","name":"Plait","states":[{"id":"STATE_ID","name":"In Progress","type":"started","position":3}],"states_complete":true}],"teams_complete":true,"cached":false,"stale":false,"cached_at":1763300000000,"cache_age_seconds":0,"partial":false,"warning":null}`.
  Bounded to 50 teams and 50 workflow states per team. The result also has
  `teams_complete:boolean|null`; each team has `states_complete:boolean|null`.
  `true` means the returned connection was complete at fetch time; `false`
  means more rows, a cap, partial/malformed data, or an unusable continuation
  prevented establishing completeness. `null` means an older cached entry has
  no completeness evidence. Cached/offline replies retain that evidence and
  their normal age/stale fields. There is no team/state continuation operation;
  these lists stop at the cap.
- **`linear_bind_project`** and **`linear_get_project_binding`** bind a
  canonical Git checkout (checked by canonical path, device, and inode, as in
  [Command policy](#command-policy)) to a team ID (pass `"team_id":null` to
  unbind; `team_key` and `team_name` are optional). A different directory at
  the same path does not inherit a binding; a directory removed and created
  again there is a different one only if the filesystem gives it a different
  inode. Both return
  `{"checkout":"/canonical/checkout","binding":null}` or
  `{"checkout":"/canonical/checkout","binding":{"workspace_id":"ORG_ID","team_id":"TEAM_ID","team_key":"PLA","team_name":"Plait","updated_at":1763300000000}}`.
- **`linear_list_issues`** takes `team_id`, preset `filter` (`active`,
  `assigned`, `backlog`, `completed`, or `all`), optional `search` (at most 200
  characters), optional `cursor` (at most 512 bytes), and optional `refresh`.
  Returns 50 summary rows per page (never descriptions, attachments, or
  comments). Search remains a broad title/number match inside the supplied
  team, including for identifier-shaped input; it is **not exact lookup**.
  Use `linear_resolve_issue` for a typed identifier or pasted URL.
  ```json
  {"issues":[{"id":"ISSUE_ID","identifier":"PLA-142","title":"Fix retry backoff","url":"https://linear.app/acme/issue/PLA-142","priority":2,"updated_at":"2026-11-18T12:00:00.000Z","team_id":"TEAM_ID","state":{"id":"STATE_ID","name":"In Progress","type":"started"},"assignee":{"name":"Alex"},"labels":[{"id":"LABEL_ID","name":"host"}]}],"has_more":true,"next_cursor":"CURSOR","cached":false,"stale":false,"cached_at":1763300000000,"cache_age_seconds":0,"partial":false,"warning":null}
  ```
- **`linear_get_issue`** takes `issue_id`, optional `comments_cursor`, optional
  `refresh`, and optional `workspace_id`. The latter must match the current
  signed-in workspace or returns `conflict`, and verifies the issue URL before
  returning or caching data. The reply adds `workspace_id` for the authenticated
  workspace. Unscoped old requests still work. `issue_id` accepts UUIDs or
  identifiers; Linear's [GraphQL guide](https://linear.app/developers/graphql)
  shows `issue(id: "BLA-123")`. This operation does not enforce the project's
  team or consult aliases for offline identifier resolution; use
  `linear_resolve_issue` for that. Complete fresh reads do populate aliases.
  Returns the issue summary fields plus `description` (at
  most 64 KiB), `attachments` (at most 20 `{id,title,url}` metadata records,
  never fetched), and paged `comments` (at most 25 per page, 16 KiB body each,
  with `comments_has_more` and `comments_next_cursor`), alongside `cached`,
  `stale`, `cached_at`, `cache_age_seconds`, `partial`, and `warning`.
  Comments have **no declared order or total**; the first page is not promised
  to contain the latest comments. Text, labels, and attachments are bounded
  without per-field truncation flags. A partial reply or a clipped field must
  not be presented as a complete issue history.
- **`linear_resolve_issue`** takes a bound project's `checkout`, `reference`,
  and optional `refresh` (default false). `reference` is a full identifier
  (`PLA-142`, case-insensitive) or a standard
  `https://linear.app/WORKSPACE/issue/PLA-142[/slug]` URL (optional query/fragment;
  no credentials, nondefault port, encoded identifier, or extra path segments).
  It is trimmed and bounded to 2,048 characters. Numeric-only input is refused.
  The project binding must have the current workspace ID; otherwise returns
  `invalid_request`. A URL from another workspace returns `conflict` before
  fetching. One `issue(id:identifier)` query resolves identity; the returned
  identifier, team ID, and URL workspace must all match exactly or returns
  `not_found`. No first numeric/title-search hit is selected. A changed binding
  or sign-in while the request is pending invalidates the result.
  Returns `{"issue":SUMMARY,"workspace_id":"ORG_ID","cached":false,"stale":false,"cached_at":1763300000000,"cache_age_seconds":0,"partial":false,"warning":null}`.
  Complete list/detail/resolve reads populate workspace/team/identifier→ID
  aliases with a bounded summary, so a known issue can resolve offline without
  its detail cache. The alias cap is 500 recent entries per team, with a
  3-minute freshness TTL; stale aliases are served only on an offline failure.
  An uncached offline identifier returns `linear_offline`. Sign-out and every
  key change clear aliases. Accepted writes mark matching aliases stale until
  their issue is refreshed. Legacy caches gain aliases on their next fresh read.
- **`linear_get_linked_issue`** takes `thread_id`, optional `comments_cursor`,
  and optional `refresh`, and returns the `linear_get_issue` shape. It refuses
  null-workspace, signed-out, rejected-key, and other-workspace historical links
  with `conflict` **before any network request**, and returns `not_found` for
  absent threads/links. It verifies the stable issue ID and workspace and
  discards replies if the link or authentication changes in flight. Use this
  operation for live state/team on a link; never hydrate a historical label
  with an unscoped `linear_get_issue` call. Explicitly relinking an issue in the
  signed-in workspace is required to establish identity for a legacy link.
- **`linear_link_thread`, `linear_unlink_thread`, `linear_get_thread_link`, and
  `linear_list_thread_links`** manage local thread-to-issue links in
  `host.sqlite`. Links never write to Linear.
  - `linear_link_thread` persists `{thread_id,workspace_id,issue_id,identifier,url,title,linked_at}`
    for an existing thread (`identifier` matches `^[A-Z][A-Z0-9]{0,15}-\d{1,9}$`,
    `url` is an `https:` URL without credentials, `title` is at most 512
    characters) and returns `{"link":LINK}`. Optional `workspace_id` must match
    the authenticated workspace and the URL's workspace/identifier or returns
    `conflict`. New callers should provide the resolver's workspace ID. Old
    requests omitting it still work: matching URLs get the current workspace
    ID; signed-out or mismatched URLs get null. Each returned link adds
    `historical:boolean`, true when workspace identity is absent, different
    from the current workspace, or authentication is signed out/rejected.
    Legacy rows migrate with null workspace; sign-out never deletes links.
    Link metadata contains no live state or team, and a project rebind never
    retargets a saved link.
  - `linear_unlink_thread` returns `{"thread_id":"THREAD_ID","removed":true}`.
  - `linear_get_thread_link` returns `{"thread_id":"THREAD_ID","link":LINK|null}`.
  - `linear_list_thread_links` returns every link in stable order
    (`linked_at DESC, thread_id ASC`), paged with optional `limit` (1–500,
    default 200) and `cursor` (`<linked_at>:<thread_id>`), capped at 750,000
    JSON bytes per page: `{"links":[LINK],"next_cursor":null}`.
  - Deleting a thread row removes its link automatically via an SQLite
    `AFTER DELETE ON threads` trigger.
  - **Thread summaries unchanged:** `ThreadSummary` and `Snapshot` do not carry
    the linked issue in protocol versions 1–6; clients use
    `linear_list_thread_links` to load links in one request instead of per
    thread. Whether summaries carry a linked-issue field is left to the desktop
    stage, with a protocol version bump if needed. No change notifications are
    emitted for links in this stage.
- **`linear_post_comment`** posts one comment on `issue_id` with `body` (at
  most 16 KiB UTF-8, nonempty) and caller-chosen `request_id`
  (`^[A-Za-z0-9_-]{1,128}$`). Invalidates cached entries for the issue and
  returns `{"comment_id":"COMMENT_ID","issue":DETAIL|null,"partial":false,"warning":null,"outcome":"accepted"}`.
  Accepted retries return the saved receipt, optionally refreshing only the
  issue read when the full receipt was evicted or its refresh failed.
  Reusing `request_id` with a different operation, issue, exact body/state, or
  workspace returns `conflict`. Legacy records with unknown workspace identity
  return only their historical receipt and never trigger a mutation or read.
- **`linear_update_issue_state`** moves `issue_id` to `state_id` with
  caller-chosen `request_id`. Before sending the mutation, the host verifies
  against team workflow states that `state_id` belongs to the issue's team
  (refreshing teams if the cache is missing, expired, or lacks the state, and
  rejecting mismatches with `invalid_request`). Invalidates cached entries for
  the issue and its team, deduplicates by `request_id` as above, and returns
  `{"issue":DETAIL|null,"partial":false,"warning":null,"outcome":"accepted"}`.
- **`linear_get_issue_text`** reads an issue's title and description exactly
  as Linear stores them, for an edit: issue reads bound and escape text for
  display, and writing that back would lose text. Returns
  `{"issue_id":ID,"updated_at":TIME,"editable":true,"title":TEXT,"description":TEXT}`.
  When the text is longer than Plait can write back (512 characters of title,
  32 KiB of description), holds characters the write operations refuse,
  contains the key, or Linear's answer was partial (GraphQL errors, or a field
  missing or null), `editable` is false and both fields are null; edit that
  issue in Linear. A missing field is never read as empty text.
- **`linear_create_issue`** creates an issue in `team_id` with `title` (1–512
  characters), an optional Markdown `description` (at most 32 KiB, tabs and
  newlines allowed) and an optional `state_id` of that team, checked against
  the team's states before sending. The host sends Linear a UUID v4 issue `id`
  derived from team and `request_id` (`IssueCreateInput.id` in Linear's SDK
  schema), so a retried request names the same issue. Returns
  `{"issue_id":"ISSUE_UUID","identifier":"PLA-201"|null,"issue":DETAIL|null,"partial":false,"warning":null,"outcome":"accepted"}`
  and deduplicates by `request_id` as above.
- **`linear_update_issue`** changes an issue's `title`, `description`, or both
  (at least one; same bounds). Linear's `issueUpdate` has no conditional guard,
  so before sending the host reads the issue and refuses with
  `linear_issue_changed` (`outcome:"not_sent"`) when its `updatedAt` differs
  from `expected_updated_at`. A change landing between that read and the
  mutation is still possible. Returns the same shape without `issue_id`.
- **`linear_read_image`** reads one image from Linear's file storage, which
  Linear documents at `uploads.linear.app` with the same `authorization`
  header as the API. `url` must be https on exactly that host, with no port or
  credentials. The host sends the key only to that host. A redirect is
  followed once, without the key, and only when it stays on that host over
  https on the default port; any other destination is refused before
  connecting. Only PNG and JPEG within
  the thread image bounds (5 MiB, 8192 pixels a side; streamed and
  decompressed size both count) are kept, in a cache of at most 64 images and
  64 MiB. Every read, cached or not, needs the current sign-in, and an entry
  is served only to the sign-in that read it (bound to the key file's save
  time and workspace, never the key), so a crash between removing the key
  and clearing the cache leaves nothing readable. Sign-out clears it. Returns
  `{"url":URL,"size":BYTES,"offset":OFFSET,"data":BASE64}` in 512 KiB chunks;
  read from offset 0 until `size`. Unreadable or missing files are `not_found`
  and never mark the key rejected.

### Durable write outcomes

The host saves operation, issue, workspace, exact body/state, and request ID
before attempting a mutation. It commits `unknown` immediately before sending;
a crash even between that commit and sending remains uncertain. An explicit
acceptance is saved before the optional refresh read, even if sign-out or a key
change happened in flight. A failed refresh still reports accepted, with a null
issue and fixed warning. Nothing retries writes on startup, reconnect, or timers.

- `not_sent`: intent saved but the mutation has not been attempted.
- `rejected`: an explicit refusal (HTTP 401/403/429, a classified GraphQL
  rejection without data, or mutation `success:false`).
- `accepted`: Linear reported success; return the saved receipt.
- `unknown`: a sent mutation may have landed (lost reply, timeout, ambiguous
  response, or crash before receipt persistence). Retrying returns
  `linear_write_unknown` with `outcome:"unknown"` and **never sends again**.
  Inspect the result in Linear; matching comment text or absence from one
  comments page cannot settle it. This also prevents a retried state move from
  overwriting another person's later change.

Only explicit requests for `not_sent` or `rejected` records may send again.
Write operations still require sign-in; use `linear_get_write` to inspect
receipts while signed out. Classified write errors retain `code` and `message`, adding `outcome` once the
request has passed validation; an unclassified `internal` failure requires
reconciling with `linear_get_write`. A conflict describes the existing request's outcome.
Malformed requests have no recorded outcome. Full receipts remain capped at
500, but compact accepted/rejected tombstones and uncertain intents are retained
for the lifetime of the store across eviction, key changes, and sign-out. Exact
payloads are released on acceptance/rejection; operation, payload digest, issue,
workspace, request ID, and compact receipt remain. At 10,000 identities, new
writes fail with `linear_write_history_full` and `outcome:"not_sent"`; existing
IDs remain readable/retryable under the same rules. No time-based pruning occurs.

Comments send a deterministic UUID-shaped ID derived from workspace and request
ID (SHA-256 domain separation, UUID v4 version/variant bits) through
`CommentCreateInput.id`. Linear's [published schema](https://github.com/linear/linear/blob/2ca00918e3ae699a0aa1a6b07330b433397cd201/packages/sdk/src/schema.graphql)
documents this input; duplicate-ID refusal is defense in depth, unverified
against Linear. Its `issueUpdate(id,input)` / `IssueUpdateInput` has no
expected-state or revision condition. State updates therefore rely on the local
non-replay rule, not remote compare-and-swap. No live API behavior is claimed.

### Caching, limits, and errors

- **SQLite tables (`host.sqlite`, additive):** `linear_projects`,
  `linear_cache_meta`, `linear_cache_teams`, `linear_cache_issues`,
  `linear_cache_pages`, `linear_cache_details`, `linear_cache_aliases`,
  `linear_thread_links`, `linear_writes`, and `linear_write_intents`. Existing stores gain these
  tables idempotently on open; existing full receipts migrate to accepted
  tombstones without inventing a workspace or issue ID. Additive columns on
  `linear_thread_links` and `linear_cache_meta` preserve old rows, with null
  workspace identity/completeness until explicitly established.
- **TTLs and bounds:**
  - Teams and workflow states: 15-minute TTL (50 teams, 50 states/team).
  - Issue list summaries: 3-minute page TTL; `linear_cache_issues` holds at
    most 500 rows per team; `linear_cache_pages` holds at most 100 pages (LRU).
    Exact aliases: 500 recent entries per workspace/team, 3-minute freshness TTL.
  - Issue details (first comments page): 2-minute TTL, LRU cap of 200 issues.
  - Full write receipts: latest 500 writes; permanent retry identities: 10,000
    (new writes refused at capacity). Uncertain bodies remain bounded to 16 KiB
    per intent; terminal tombstones omit the body.
  - All Linear requests are bounded to 65,000 bytes.
- **Offline fallback and async guards:** When Linear cannot be reached (network
  error, timeout, HTTP 5xx, refused redirect, or oversized response), read
  operations serve any existing cached entry with `"cached":true,"stale":true`
  and `cache_age_seconds`. Superseded queries and requests that were in flight
  during `linear_sign_out` or `linear_set_key` never overwrite or repopulate
  the cache.
- **Error codes:** `linear_not_signed_in`, `linear_key_rejected` (HTTP 401,
  HTTP 403 on read, or GraphQL `AUTHENTICATION_ERROR`), `linear_read_only_key`
  (HTTP 403 or GraphQL `FORBIDDEN` on a write), `linear_rate_limited` (HTTP 429
  or GraphQL `RATELIMITED`, reporting the wait seconds derived from
  `Retry-After`, `X-RateLimit-Complexity-Reset`, and
  `X-RateLimit-Requests-Reset`), `linear_offline`, `invalid_request`,
  `not_found`, `conflict`, `linear_write_unknown`, `linear_write_history_full`,
  and `internal`. HTTP 200 responses carrying GraphQL `errors`
  beside usable `data` succeed with `"partial":true` and a fixed `warning`, and
  are not cached as complete responses. A mutation `success:true` is accepted
  even when errors affect its selected fields; an ambiguous write response
  remains unknown. Rate-limit errors add `retry_after_seconds` (positive integer
  seconds, 1–86,400; default 60 when no usable header is present), alongside the
  existing fixed-template `code` and `message`. For example:
  `{"ok":false,"error":{"code":"linear_rate_limited","message":"Linear rate limit reached. Retry in 12 seconds.","retry_after_seconds":12}}`.
  Write refusals also include their `outcome`. Other operations' errors retain
  their existing two-field shape; clients need not parse the message for a wait.

The desktop supports Settings key entry, a project issue view, draft
references, and a source-issue thread label. See [Linear](LINEAR.md).

### Not built in this stage
- No browser OAuth sign-in or token refresh yet.
- No automatic writes, background polling, or Linear change notifications.
- No issue-triggered autonomous runs (their rules are not yet agreed).

## Desktop host identity

The additive, read-only `{"op":"host_identity"}` request returns schema 1,
the persisted `host_id` used by turn review, and the absolute host `store`
path on the owner-authenticated local connection. It works before any thread
exists. Unknown request fields are refused; no protocol version is added.
Desktop tabs use this identity to validate saved tabs. An older host may
refuse the operation; session tabs still work, and durable restore waits for
a verified identity from turn review once a thread exists.

## Host-owned worktrees

These additive operations omit `protocol_version`, like recorded turn review
and Linear. Versions 1–6 retain their request, summary and snapshot shapes.
No version bump is needed: the explicit `worktree_threads_v1` capability is
returned only by `worktree_info`. Older hosts reject these operation names;
the desktop reports that host-owned worktrees need a newer host. Unknown
fields, including `protocol_version`, and unknown lifecycle states are refused.

- `worktree_info {checkout}` returns `{schema:1,worktree_threads_v1:true,parent}`.
  `parent` contains canonical `root`, `main`, `common`, their `identity`,
  `main_identity`, `common_identity` objects (`dev` and `ino` as decimal strings),
  and `base`, a full 40- or 64-digit commit OID. An unborn repository is refused.
  The first registered Git worktree identifies the main root; linked worktrees
  therefore create siblings beside the main repository, not nested projects.
- `create_worktree_thread {request_id,parent,harness,provider,model_id,approval}`
  optionally takes `thinking` and `mode`. Harness is `pi`, `codex` or `claude`;
  provider/model and mode must belong to that harness and be available before
  admission. Approval is `ask` or `auto`; Pi uses it for the thread, while the
  other harnesses retain their own mode. The parent command policy is captured
  for the new checkout. No run or model request is started by this operation.
- `worktree_status {request_id}` returns the creation or removal receipt.
  `list_worktrees {}` returns `{schema:1,operations:[...]}`, newest first,
  bounded to 256 operations and 750,000 bytes. These are recorded lifecycle facts, not live
  dirty-file or publication scans; removal always makes fresh checks.
- `project_worktrees {checkout}` returns
  `{schema:1,worktree_inventory_v1:true,checkouts:[...]}`. The main checkout is
  first, followed by registered linked checkouts and retained removed records.
  Each row has `path`, `main`, nullable `branch`, `head`, recorded `base`,
  `thread_id`, directory `identity`, `creation_id`, host-millisecond `last_used`,
  `missing`, `removed`, and `preservation`. Preservation contains nullable
  counts `changed`, `unpushed`, `exclusive`, nullable exact `refusal`, and
  `can_keep_local`. That boolean explicitly permits the confirmed local-branch
  choice: only the publication refusal is waived, with every ownership,
  preservation and busy check still required. Older replies omit it; readers
  default it to false. No extra Git reads are needed to compute it.
  Unknown counts remain null. Changed includes untracked and ignored status
  entries; ignored directories are counted as Git reports them. Publication
  uses the known remote-tracking ref. Exclusive counts commits reachable only
  from worktree-local refs or reflogs. These are the same checks removal uses,
  supplemented by identity, ownership, cleanup-state, operation and admission
  refusals. A null refusal requires all counts known and zero and a verified
  owned checkout. The operation takes no checkout lease, because a run takes
  its own without waiting and would be refused beside it; a run held by
  another host process is therefore not seen here. Removal rechecks everything
  under its leases and refuses then. The pass reads each checkout's
  configuration key names once, not once per Git command. External checkouts have
  no creation authority. Missing folders remain rows; removed records keep
  branch/history/cache attribution with no deletion action. The result is
  bounded to 256 rows and 750,000 bytes; reading creation records is bounded
  to 4096 records and 900,000 bytes across the host store. Excess is an explicit
  error. Optional `only`, an absolute path, returns exactly that checkout's row
  with the same fields and checks (`main:true` when it is the main checkout), so
  a run's end re-reads one checkout, not all of them. An `only` that is not one
  of the project's checkouts is `invalid_request`. Every host with this
  operation accepts `only`. This
  operation probes its own capability, preserving all previous reply shapes.
- `remove_worktree {request_id,creation_id,identity,allow_unpushed}` requires a
  recorded host-owned checkout and explicit confirmation. `allow_unpushed:false`
  refuses a missing/local upstream or commits absent from its known remote
  tracking ref. This reflects the last fetch, not a live remote claim.
  `allow_unpushed:true` explicitly accepts keeping those commits only on the
  retained local branch. Neither value permits deleting uncommitted files or
  ignored content outside
  the host’s currently approved build directories. Approved ignored output goes
  with the checkout; the current list is read under the removal lease and common
  metadata lock, which also serializes policy edits. There is no force flag,
  branch deletion or automatic cleanup.

### Approved build output

Build output approval is host policy, never repository configuration. The
owner-only `build-output-policy/<device>-<inode>.json` file contains
`{schema:1,project,identity,revision,paths}`. `project` is the canonical main
checkout, `identity` its decimal-string device/inode pair, and `paths` at most
32 exact relative directories, each at most 1024 bytes. Revisions reject stale
edits. Policy reads use no-follow, owner/mode/link-count and size checks; writes
use a synced temporary file and atomic rename. Editing tracked configuration
cannot widen this list.

The additive `build_output_info {checkout,refresh?}` and
`set_build_output {checkout,identity,revision,paths}` return
`{schema:1,worktree_build_output_v1:true,policy,suggestions,seed}`. `seed` is
null or `{bytes,files,verified_at}`: the published seed's logical bytes and file
count as recorded when it was taken, and when it was taken or last found
unchanged (Unix seconds). It is read from the seed's record, never by walking
it, and its blocks are shared copy-on-write, so it is not reclaimable space. A
suggestion is never approval. Manifests are bounded data reads, never executed: Cargo suggests
an existing ignored `target`; a package manifest with a lockfile suggests an
existing ignored `node_modules`. Custom output directories are supported.
Approval refuses dot components, absolute paths, Git data, linked directories,
volume boundaries, nested repositories and Python virtual environments
(`venv`, `.venv` or `pyvenv.cfg`). An approval covers the whole directory:
every file inside it, whatever its name, is cloned and goes with a removed
worktree; only what sits outside an approved directory is never copied. The entire directory must be ignored and
contain no tracked or non-ignored files. Git reads retain their hardening;
`check-ignore` alone disables literal *pathspec* mode because it accepts
filenames and rejects that flag, receiving NUL-delimited directory names.

`watch_build_output {request_id,kind}` subscribes to one `worktree` or
`admission` creation. Its initial reply and subsequent `build_output` events
contain the capability, schema, request ID, nullable thread ID, phase
(`waiting`, `cloning`, `warm`, `cold`), cumulative file count and nullable
outcome. `skip_build_output` takes the same identity and cancels preparation.
An outcome contains `warm`, `reason`, `directories`, and `counts` with `files`,
`links`, `skipped`, logical `bytes`, `elapsed_ms` and up to 20 diagnostic notes.
These are pushed on state changes, coalesced by the existing subscription
channel. No desktop progress polling or render-path filesystem work is added.
Older hosts make the list unavailable. Existing creation-status retries retain
their behavior and wait up to 330 seconds for accepted preparation.

`project_worktrees_build {checkout,only?}` adds `worktree_build_output_v1:true`
and nullable `seed_bytes` (the project's seed, as in `seed.bytes`) to inventory,
and `build_directories`, nullable `build_output` outcome, and nullable
`preservation.build_output` to its rows. Older inventory reply shapes
stay unchanged. Known approved ignored content does not count as `changed`;
unknown ignored content still blocks removal. Preview and removal use the
same validation function. Revoking approval immediately restores that
preservation requirement. Logical size classification includes approved custom
paths; shared copy-on-write blocks are not a promise of reclaimable storage.

On macOS, the helper uses descriptor-based `fclonefileat` for each regular file
on the same volume. It creates directories, verifies every opened directory
against the listed identity, never follows a substituted link, and never makes
hard links or falls back to copying bytes. It retains only relative links that
resolve inside the approved directory; escaping, absolute and unresolved links,
sockets, devices and immutable or append-only files are skipped and reported
(a clone keeps file flags, so such a clone could never be removed). Nested repositories and changed
source identities fail preparation. Another volume, unsupported cloning,
insufficient space, cancellation or a failed check starts cold with a reason.
Other platforms start cold because cloning is unavailable.

The project's main checkout, while idle, supplies an immutable seed under
`build-output-seeds` while Plait holds its writer lease. A linked worktree never
supplies one: one thread's output must not replace a fuller seed or become every
later thread's starting cache.

A seed records when it was taken or last found unchanged, and a digest of every
entry it cloned: path, device and inode, type, mode, size, flags, modification
and change times, and link targets. A run ending in the main checkout starts no
helper while the seed matches the approval and is less than an hour old; after
that, a refresh first walks the source with `stat` only, under the same writer
lease, and clones again only if the digest differs. An unchanged walk resets the
seed's age. Dependency output, the part worth cloning, changes with a lockfile,
not every run, and a new worktree rebuilds the workspace's own artifacts anyway
because their fingerprints carry the checkout path. Approval changes refresh at
once in the background, and `refresh:true`, sent only by the Refresh control,
refreshes whatever the seed's age; both still skip an unchanged source.
Creation clones the published seed that matches the approval, however old, and
never waits for a refresh; only with no usable seed at all does it wait for one
capture from an idle main checkout. After the thread is admitted, an old seed is
refreshed in the background by the same rule. A local admission or run aborts a
refresh helper and waits for its process exit before taking its writer lease. A private, verified FIFO beside the shared
checkout lease lets another host's admission or writer wake the seed too; it
allows at most 20 ms for the seed to release its lease before applying the
ordinary lease check.
Only the seed helper listens, so no PID is signaled. The listener blocks on an
OS event, with no polling or clone-length wait. The helper exits
on host lifetime-pipe EOF, so a host crash cannot leave it cloning indefinitely.
Another host's writer lease prevents capture too. A busy checkout is never the source: creation
uses the last published matching seed, or starts cold when none exists. The
seed is never written by later runs. Capture is bounded to 120 seconds and one
million entries per directory, with a 150-second helper deadline. A manual
terminal build is outside Plait's leases and can still change a source during
capture; identity and timestamp checks detect some changes, not every possible
external write.

For trusted projects, Codex and Claude Code, approved output is installed in the
new checkout. Sandboxed Pi receives approved `target` in its private per-thread
build cache; other approved output stays in the checkout. A successful sandboxed
Pi run in the main checkout can supply the next seed's target from its private
cache. Credentials,
Cargo home and unapproved caches are never seeded. Python environments are
refused; other caches with absolute paths should be recreated instead.

The clone is an idempotent creation step after Git and before the first message.
Owner-only `build-output-operations/<thread>.json` records staging identity and
installation intentions before exclusive renames. An interrupted installation
is rolled back on retry or host restart before admission; a completed outcome
is reused. Partial output is removed by descriptor and recorded identity;
replaced directories are preserved and admission fails pending inspection.
Existing destination output is never overwritten. One refresh runs per project
at a time (`<key>.seeding.lock`), and seeds publish atomically. Each seed has its
own `<seed>.lock`: a creation holds it shared while it clones, rereading the
pointer if it moves, so neither a refresh nor a deletion makes a creation start
cold. A pending journal distinguishes an abandoned capture from a published one;
the retired list (`<key>.retired.json`) names earlier and abandoned seeds, which
are deleted after the new seed is published and its source lease released,
each once no creation still clones from it. An interrupted deletion resumes at
the next refresh. Refresh replaces only earlier seeds, never a worktree. Clearing every approval
removes the project's seed at the refresh that follows. There is no host
operation that removes a project, so a project that is never opened again keeps
its seed until its approvals are cleared; a seed whose main checkout is gone or
replaced is never used, because seeds are found through that checkout's
identity. Worktree cleanup remains manual, with no eviction or storage budget.

Creation/removal return `{schema:1,operation}` promptly. An operation includes
`request_id`, allocated `thread_id`, `state` (`preparing`, `ready`, `failed`,
`removing`, `removed`), nullable `worktree` and nullable typed `error`. The
worktree record contains `thread_id`, the immutable `parent`, `path`, `branch`,
nullable `identity`, `stage` (`reserved`, `creating`, `ready`, `failed`,
`removing`, `removed`) and nullable diagnostic `error`. Removal status retains
the original creation ID inside this record; its separate request ID selects
the immutable removal receipt. Failed removal leaves the creation available
and returns its refusal. Repeating a request with different fields is refused.

IDs are 1–128 ASCII letters, digits, underscores or hyphens. Paths must be
absolute UTF-8, at most 4096 bytes, without control characters or dot components.
Identity values are unsigned 64-bit decimals. Model/provider fields are at most
256 bytes; modes are 1–32 lowercase letters/underscores. Requests are at most
65,000 bytes. Nested parent and identity objects reject unknown fields. Core
reply parsers validate schema, UUID, derived branch/path, identities and lengths.

The host records the request, UUID and selected thread settings in additive
`worktree_operations`/`worktree_removals` SQLite tables before starting effects.
Private `worktrees/<thread-UUID>.json` journals are owner-only, no-follow,
size-bounded files replaced atomically and synced. Branch names start with
`plait/<first-12-UUID-hex-digits>` and extend by four digits on collision up to
the full UUID. Existing branches, paths and symlinks are never reused. The root
is `<main-repository>.worktrees/<suffix>`. Dirty files are never copied; approved
build output may be cloned after Git completes, before thread admission.
Git writes the checkout under the usual `022` umask, not the host's owner-only
one, so its files get the modes an ordinary checkout would have.

Creation holds the parent's shared lifecycle lease and the common Git metadata
lease to reserve the destination. It then releases metadata exclusion, takes
the new root's exclusive lifecycle/writer leases and reacquires common metadata
before Git starts, retaining the parent lifecycle lease throughout. This keeps
lock ordering intact and prevents another host from running in a partial checkout.
Removal holds exclusive lifecycle and writer leases on the recorded
checkout, followed by common metadata exclusion. The record must name the
UUID-derived folder and branch; the main checkout and common-directory
identities, worktree membership and branch are rechecked under those leases
(the checkout a worktree was started from need not still exist). Removal also
refuses queued work, record recovery, uncommitted/untracked/ignored content,
unfinished merge/rebase/sequencer state, tracked files whose edits are hidden
by an assume-unchanged or present skip-worktree flag, and commits reachable
only from the worktree's own HEAD reflog or per-worktree refs, whichever
confirmation was given. Desktop busy summaries grant no
permission. Both helper and Git supervisors use lifetime pipes and bounded
process groups; checkout hooks and filters have a 120-second limit and are
reclaimed before the outer worker releases its leases. The worker lends its
parent, target and common-metadata lease descriptors to the supervised Git
processes. If that worker is killed, surviving Git descendants retain exclusion
until they exit. This lending is limited to worktree add/remove; ordinary
completion still unlocks the lease after supervised cleanup.

A lost reply is recovered by status or the identical request, with the same
UUID and base. Host restart resumes admitted creation. A verified complete,
clean checkout at its recorded base can be bound to its saved thread after a
crash; hooks are never replayed against an existing checkout. A partial or
ambiguous creation is reported as failed, retaining its files and branch for
inspection rather than guessing ownership or deleting unknown work. When a
checkout hook or filter fails after Git made the worktree, Git keeps it; the
error names its folder and branch and says no thread was started. A crash
between directory reservation and identity persistence likewise refuses reuse.
A removal interrupted after Git completed is reconciled only when its recorded
parent is available and Git no longer registers the missing checkout; a missing
volume alone is not success. Branches and thread history remain after removal.
Legacy desktop worktree associations remain imported presentation metadata and
never authorize deletion. External programs do not participate in these leases.


## Busy-project admission

These additive operations omit `protocol_version`. They do not change existing
request, snapshot or summary shapes, so this operation requires no protocol bump. Probe
`admission_info` before offering placement or linked continuation; an older host's
`invalid_request` means unavailable, never permission to imitate the operation.
An older desktop continues to receive the existing `busy` refusal.

- `admission_info {checkout}` returns schema 1, `worktree_at_start_v1:true`,
  `linked_continuation_v1:true`, `queue:true`, `strict_reader:false`, canonical
  `checkout`, its `identity`, main `project`, `project_identity`, `busy`,
  `auto_worktree`, nullable Stage 7 `parent` and nullable `refusal`. Busy means a
  run: this host's active or recovering run, or another host's run holding the
  writer lease. A desktop Git write (stage, commit, push, fetch, pull) or a
  removal holds the checkout exclusively and is not a busy project. Concurrent
  probes of one checkout share a single lease check, and a run start waits for
  an in-flight probe instead of colliding with it. A stale summary is not
  authority. An unborn repository can use its own checkout but has no worktree
  base until its first commit.
- `set_project_isolation {checkout,identity,auto_worktree}` returns that info.
  `identity` is the main project's identity from the probe. Authorization is
  stored against canonical main path plus device/inode; replacing a repository
  cannot inherit it. Default is false, meaning ask each time. It applies to new
  threads only, including requests from the project's linked worktrees.
- `create_admitted_thread {request_id,checkout,placement,harness,provider,
  model_id,approval,mode?,thinking?,source_thread_id?}` returns
  `{schema:1,admission}`. Placement is `project`, `worktree`, or explicit `wait`. Wait creates an
  in-place thread even if the project has automatic worktrees enabled; it never
  submits a message. Clients probe `checkout_queue_v1` before using Wait. Approval is `ask`
  or `auto`; harness/model/mode/thinking are preserved. A busy project with no
  authorization returns `choice` without creating a receipt or thread. The
  same request ID may then be explicitly resubmitted with `worktree`.
  Otherwise the exact input, checkout identity, explicit HEAD base and chosen
  placement are recorded before effects. After that, changing the payload under
  that ID is refused. Automatic placement happens only when this project is
  busy and its switch authorizes it. A race after an in-place creation may still
  make its first submission return `busy`; the bound thread is never moved.
- `admission_status {request_id}` recovers an unanswered operation. A receipt
  has `request_id`, state (`choice`, `preparing`, `ready`, `failed`), nullable
  `thread_id`, actual `checkout`, nullable managed `worktree`, nullable
  `source_thread_id`, and nullable `error`. Ready requires a thread. Startup
  resumes recorded preparation using the same Stage 7 request; it never creates
  a second worktree or sends a model prompt. A failed receipt remains failed;
  inspect the cause before a new request. Removal continues to use the original
  Stage 7 creation receipt, not the admission request ID.
- `thread_origin {thread_id}` returns `{schema:1,admission}` with a nullable
  receipt. A linked continuation requires explicit worktree placement and a
  source thread bound to the same verified checkout. It creates a new harness
  session; source history, records and draft are untouched. No old session or
  prompt is replayed.
- `run_preparation {thread_id}` returns `{schema:1,thread_id,phase}`; phase is
  `preparing`, `resource_wait`, `running`, or null after release. This is a
  resource-preparation state of an admitted run, not a queued checkout writer.
  The desktop reads it only for the selected running thread until preparation
  ends; it adds no hidden transcript watch.

Admission requests reject unknown fields, protocol versions, invalid enum values
and relative/control/dot-component paths. They are bounded to 16,384 bytes;
IDs are 1–128 ASCII letters, digits, underscores or hyphens, model fields at
most 256 bytes, modes at most 32 lowercase letters/underscores, paths at most
4096 UTF-8 bytes, and identities canonical unsigned 64-bit decimals. Parsers
reject unknown reply states, inconsistent ready receipts and forged worktree
paths. No live reader, build-output copy or automatic removal is offered.

A checkout held for unconfirmed cleanup is refused with `cleanup_uncertain`
before placement is offered, including when a cached active-run summary says
busy or this project authorizes automatic worktrees. The refusal names the
recorded cleanup reason. It creates no admission, directory or branch and never
clears, removes or reuses protected checkout state. A held checkout must be
resolved through the existing cleanup controls before a fresh placement request.
Worktree effects also recheck the checkout under their lifecycle and repository
metadata leases, closing the gap after the admission probe.

### Durable checkout queue

The checkout queue is host state, separate from the desktop's
`queued_submissions` recovery cache and the snapshot's per-thread `queued`
follow-ups. A person explicitly sends each checkout waiter. No prompt analysis,
automatic new-thread queue choice, or agent orchestration is involved. Every
run remains a writer.

These additive operations omit `protocol_version`; versions 1–6 and their
existing reply shapes are unchanged:

| Operation | Result and meaning |
| --- | --- |
| `checkout_queue_info {}` | `{schema:1,checkout_queue_v1:true,limit:32,payload_bytes:33554432}`. An older host's `invalid_request`, any other refusal, or a capability with other values makes Wait unavailable; the desktop's thread listing, which probes first, still loads. |
| `submit_checkout {thread_id,request_id,text,approval?,images?}` | The existing acceptance fields `{thread_id,request_id,accepted:true}` after a durable commit. Approval defaults to `ask`; image IDs refer to completed thread-owned uploads. An available checkout may start immediately. |
| `checkout_queue {thread_id}` | `{schema:1,thread_id,checkout,waiters}` with up to 32 checkout waiter receipts in acceptance order: pending inputs first for inclusion, then recent terminal receipts. Each has `request_id`, `state`, `position` (zero when terminal), and nullable `error`. Cancellation replies always include the requested receipt. |
| `cancel_checkout_wait {thread_id,request_id}` | The same queue receipt. Waiting or held input becomes cancelled. If that exact input already started and is still the active run, the operation requests the existing Stop. It never stops a later run or silently discards an executing input. Repeating cancellation is safe. |
| `resume_checkout_queue {thread_id}` | The same queue receipt. Explicitly releases every held checkout waiter for this thread's checkout in its original order. A held follow-up is not released: it keeps its own thread's next-send rule. It does not replay an interrupted run. |
| `list_threads_queue {checkout?,limit?,cursor?,archived?}` | The version 6 thread-page shape, adding nullable `checkout_queue` to each summary. The normal listing's pagination and bounds apply. |
| `watch_threads_queue {archived?}` | The existing shared summary-event stream, with the same queue addition. No separate watcher per waiter is required. |

The compact summary is `{request_id,state,position,count}` for a thread's first
checkout waiter; state is `waiting` or `held`. Count is that thread's checkout
waiters, position includes preceding pending inputs in the checkout. Summaries
contain no prompts or image bytes. Unknown states, invalid identities, unknown
fields, excessive counts, and out-of-range positions are rejected by the core
parsers. The current summary stream retains its newest-200-thread window; older
threads remain available through paged listing and explicit refresh.

Acceptance stores the original request ID, exact message and image references,
canonical checkout plus directory device/inode, chosen harness, model, thinking,
permission mode, approval, command policy, immutable writing class, acceptance
order and cancellation state. Images remain in the host's private store. Same
ID and payload returns the original acceptance even after cancellation or start;
changed text, images or approval is a conflict. A busy refusal creates no accepted
input. The new operation cannot reuse an ID from a different submission kind.

There are at most **32 pending inputs and 32 MiB of text plus image payload per
checkout**, counting held inputs and follow-ups. The existing 16-follow-up limit
per thread also applies. Capacity errors say `queue_full`, describe the reached
bound, and preserve the unsent draft. Counts release on start, cancellation or
terminal rejection; permanent deduplication receipts remain. Existing per-message
image and request-frame bounds still apply.

**Fair order.** Every pending input receives the same monotonically increasing
request-row order at durable acceptance. For one canonical checkout, the lowest
eligible order gets the next writer lease. A later follow-up cannot bypass an
earlier checkout waiter, and a direct submit cannot jump an eligible line.
Steering explicitly joins the current run; a steering input that was not taken
uses its original place for a later run. Follow-ups held after their own run
fails, or at a restart, remain ineligible until their own thread's next send;
another thread's Resume does not release them, and they do not prevent other threads' waiters from advancing. A restart-held checkout
waiter keeps its place and holds later inputs until Resume or Cancel. Independent
checkouts have independent drains and never share a queue head.

| State | Transition |
| --- | --- |
| `waiting` | Durable acceptance; the host considers it when the checkout frees. |
| `held` | Host startup changes every not-started input to held, and so does an unconfirmed cleanup met at admission, which records its reason on the head's thread. Resume changes the checkout's held checkout waiters to waiting. |
| `started` | Revalidation and lease acquisition succeed; moving the input into the conversation and recording start are one transaction. Normal run/Stop/recovery states now own it. |
| `cancelled` | Cancel or Stop removes a pending input and its image associations. Its request ID stays used; it never runs. |
| `failed` | A checkout waiter fails revalidation or admission, records its reason, and leaves the line. The next waiter is considered. |

Before start the host rechecks the saved folder identity, cleanup exclusion,
harness/model availability, cancellation and settings. A changed model, thinking
level, permission mode or command policy fails the waiter explicitly; it does
not silently substitute new settings. Send again with the intended choices.
Cleanup uncertainty never grants a lease; it holds the checkout's line rather
than failing each waiter in turn. No before-record is taken while an
input waits: recording begins only after normal run admission. A failed
before-record is a normal failed run, with no harness execution.

A host restart starts no queued work. Resume applies to the whole checkout line
so its original order is retained without asking separately for each message.
A run interrupted after admission is never re-queued; its existing recovery and
cleanup controls remain authoritative. Cleanup confirmation alone does not
resume held messages. Closing a desktop or tab does not hold an otherwise live
host's queue.

Run completion and queue operations wake drains. Local completion wakes other
checkouts waiting for configured capacity. A single shared host timer retries
only lease/capacity contention, including a writer in another host store; it is
not a desktop poll or one timer per waiter. Actual resource failures are reported
rather than silently retried. FIFO is within this host's durable line; other
host stores participate in OS write exclusion, not a shared distributed queue.

### Run capacity and preparation

`run-capacity.ts` replaces the fixed eight-checkout limit. Execution has no
default count ceiling, but operating-system processes, file descriptors, memory,
disk space and provider limits remain finite. `PLAIT_MAX_ACTIVE_CHECKOUTS`, when
set, must be a positive safe integer; reaching it refuses admission with
`resource_exhausted` and explicitly says nothing was queued. One before-record
scan per filesystem device runs at a time; independent devices can prepare
concurrently. Stop while waiting removes the preparation ticket and releases
its writer lease; no harness starts. A failed before record also starts no
harness. This semaphore is not a durable queue and does not promise a future run
across restart.

Recognized process/descriptor/memory/space/quota errors produce
`resource_exhausted`, identify the exhausted resource and say no automatic retry
was scheduled. Before acceptance, reservations and descriptors are released and
no input is recorded. After acceptance, the run records an error and closes the harness. Confirmed
cleanup releases the checkout; uncertain cleanup retains its durable hold and
reports that reason instead. The person's draft and recorded input remain
recoverable. Errors are never converted into automatic
worktree creation or a claim that work is queued.

A host-supplied run header states actual cwd, branch and HEAD at run start,
writer capability, private-output guidance, changing external observations and
host ownership of worktree management. Pi retains user/project instructions and
gets its private Cargo paths. Codex receives `developerInstructions` on both
start and resume, verified with the pinned 0.169.0 generated protocol. Claude
uses the pinned 0.3.326 SDK's `claude_code` preset with `append` and
`snapshot:false`, refreshing metadata on resume while retaining its normal
settings sources and project instructions. These instructions describe policy;
leases, harness permissions and containment enforce it.

The saved admission binds the checkout directory identity. The host rechecks it
after model discovery, before saving a thread, so replacement at the same path
fails explicitly instead of redirecting the draft.

## Remote bridge

`plait-host bridge` multiplexes an authenticated byte stream onto the host's
owner-only socket. It connects only to the invoking owner's socket, using the
default store or `--store PATH`; it does not expose a network listener. The
[remote transport](REMOTE.md#transport-limits-and-verification) describes the
limits. Each link allows at most 32 streams; requests are limited to 64 KiB, replies to 1 MiB,
and a partial frame has a 15-second deadline. SSH uses the bridge.

### Host release information and draining

These additive operations are versionless and do not change protocol versions 1–6. Requests accept only the `op` field; unknown fields return `invalid_request`.

- `{"op":"host_info"}` returns `{"schema":1,"release":null,"build_id":null,"protocol_min":1,"protocol_max":7,"capabilities":["drain_v1","checkout_identity_v1","project_fingerprint_v1"],"storage_schema":1}`. Release and build ID are strings when packaged metadata is available.
- `{"op":"drain_for_update"}` returns `{"draining":true,"active_runs":N}`. Running and approval-waiting threads count as active. New work returns `busy` while accepted work, Stop, approvals, reads and watches continue. Repeating the request is safe.
- `{"op":"cancel_drain"}` returns `{"draining":false}` and wakes the checkout queue. Drain state is in memory and does not survive a host restart.
- While draining, a retried request that would admit work also returns `busy`, even if an earlier attempt was already accepted; after the update the same request ID returns its saved result as usual, so a client retries after `busy` rather than treating it as a refusal of the original.
