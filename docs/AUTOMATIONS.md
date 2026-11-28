# Automations

**The host scheduler is built behind `PLAIT_AUTOMATIONS=1`, off by default and not ready to turn on: issues labelled [`blocks-flag`](https://github.com/mostlyheath/plait/issues?q=label%3Ablocks-flag) must be resolved before rollout. The desktop controls below are planned.** The project owner agreed the product
decisions and the host contract below. Each pull request that changes storage
or the protocol still needs its independent review (see
[Contributing](../CONTRIBUTING.md)).
Tracked in [#13](https://github.com/mostlyheath/plait/issues/13).

An automation is a thread that runs on a schedule. You give it instructions
once, say where it works and when, and the host runs it while you are away.
Results arrive the way any thread's do: an unread mark and an alert.

For timer requests, see [Background requests (built)](HOST.md#background-requests-built).

## What a person sees

The sidebar has a second section, **Automations**, below **Projects**. Each
automation is one thread with a card that shows its name, where it works, its
next run, and how its last run ended.

An automation has:

- **Instructions:** the message sent at each run.
- **Where it works:** a project, or any directory on that machine.
- **When:** once at a time, hourly, daily at a time, on weekdays at a time, or
  weekly on chosen days. The time zone is shown and defaults to this Mac's.
  Before you save, the form shows the next three run times.
- **Harness, model and mode:** chosen like a new thread's.
- **Catch-up window:** how late a missed run may still start. The default
  follows the schedule: 30 minutes for hourly, 12 hours for daily, weekday
  and weekly schedules, and 12 hours for a one-time run.

Its controls are **Run now**, **Pause** and **Resume**, **Edit**, and
**Delete**. A new automation starts paused until you press **Create**.

### Each run is a turn

Every scheduled run is a new turn in the automation's one thread, so the agent
sees what it did last time and you read the results top to bottom. A run's
message is marked as scheduled, with the time it was due. You can also type
in the thread between runs to steer or ask about a result. Those turns are
ordinary turns with the thread's normal approval mode. Long threads rely on
each harness's own context compaction; Plait does not reset the conversation.

### Where it works

- **A Git project:** the automation gets its own worktree, created from the
  project's current commit when you create the automation, and every later run continues
  there. Your checkout is never touched. Its changes stay in that worktree
  until you review them in Changes or the Worktrees tab. An automation never
  commits or pushes.
- **A plain folder:** runs work in place. If you choose a folder inside a broader Git checkout, creation asks you to choose the project instead. If the host reports the directory is busy, the occurrence is skipped.

### Making one

- **New automation…** in the Automations section of the sidebar opens the form
  on the current machine.
- **Make automation from this thread** (the thread's ⋯ menu or the command
  palette) opens it prefilled with the thread's project, harness, model, mode,
  approval and last message. This is the way to make one from a Codex or Claude
  Code thread: Plait adds no tools to those harnesses (see the
  [roadmap](ROADMAP.md#harnesses-and-tools)).

The form asks for a name, the instructions, where it works and when it runs.
**This project (own worktree)** gives each run its own worktree of the project;
**An existing worktree** and **Folder** run in the place you choose. The
schedule uses the kinds listed above; **Once** defaults to today's date in the
chosen time zone. The form reads the command policy for that place before it
creates anything, and refuses if it cannot, rather than guessing a broader one.

Nothing is scheduled until the host confirms **Create**. Each open form has one
creation request, so pressing Create again after a lost reply never makes a
second automation. If Plait couldn't confirm an attempt, the form says so and
offers **Retry**, which resends exactly what was sent. Changing the form after
that asks first whether to retry the earlier attempt or discard it; discarding
checks the host and warns when a duplicate is possible. Closing the form keeps
the attempt: the next time the form opens on that machine, it offers it again.

Planned: in a Pi thread, saying "do this every weekday at 9" will let the agent
draft an automation as a card you can edit. Nothing is scheduled until you press
**Create**.

### When it needs you

A run in progress pauses whenever it reaches something it may not do alone.
That includes an approval in Ask mode, a call its sandbox refuses to allow
without approval, or a Codex or Claude Code permission request. The usual
approval card appears in the automation's thread, and the thread shows in
**Needs you** with an alert. Nothing is lost.

While a run waits, or is still working, a scheduled run that falls due is
skipped. The skip is recorded on the card ("skipped: previous run still
waiting"). Runs never overlap.

Finished runs mark the thread unread and raise the usual thread alert. Failed
and interrupted runs show in **Needs you**.

### Missed runs

The host runs automations; the desktop does not need to be open. A run cannot
start while the host is down or the Mac is asleep. When the host comes back:

- If the latest missed run is still inside the catch-up window, it starts
  once.
- An unsubmitted scheduled claim can start only if it is still the latest occurrence and within its saved catch-up window.
- Every other missed run is recorded as missed. Missed runs never pile up into
  a burst.

After a wake, the host waits a short settling time (about a minute, for the
network and disks to return) before it starts a run. That wait counts against
the catch-up window.

### When its project or directory is gone

If the project or directory an automation works in has been moved or deleted,
or its worktree is missing, the automation pauses. Its card says why, and the
next run is not attempted until you fix the location and press **Resume**.

## Permissions

Saving an automation is your consent to its scheduled runs exactly as shown:
its instructions, where it works, its harness, model and mode, and its
schedule. Changing any of these is a new save, so you see the new version
before it runs.

- **Pi** runs use Automatic approval and the project's own command policy. If
  the project uses the sandbox, it applies. Pi's file tools still refuse to
  change files that were already dirty.
- **Codex and Claude Code** runs use the mode the automation names, under
  their own rules. The form says plainly that Plait's approvals and sandbox do
  not cover them.
- Scheduled runs never commit, push, create other worktrees, change
  credentials or settings, or weaken a sandbox.
- A scheduled run cannot create or change automations. Only a turn you send
  can draft one, and only **Create** schedules it.
- The consent covers scheduled runs and **Run now** only. Turns you type in the
  thread use the thread's normal approval mode.
- The planned **Pause** and **Delete** controls also stop an active automation
  run through the existing Stop and cleanup path.

## Host contract

The host owns automations. The desktop is a client of them, like threads.

- **Storage:** `STORE/automations.sqlite` holds definitions, their thread IDs,
  schedule start times, occurrence claims, click IDs and receipts. The ordinary host store owns
  the thread and its turn history.
- **Creation:** `create_automation` takes a creation request ID and a definition.
  It persists the definition and returns `state: "preparing"` and
  `admission_request_id` immediately. The service requests worktree placement
  for Git projects, including Git roots selected as directories, and in-place placement for plain folders. Plain folders use an idempotent thread creation request that checks the folder again before saving the thread. A pending folder check leaves no in-place admission plan for an older host to recover. For worktrees, the service checks
  `admission_status` in the background, with one outstanding request per
  placement. With automations enabled, the background socket operation
  `advance_automation_placement` takes the automation ID and starts or polls that saved placement.
  Its reply does not wait for checkout checks or admission. Poll `automation_status` by automation ID, or refresh the list.
  Pending placements survive host restart. Failed placement exposes the
  reason through status and does not enable the schedule.
- **Runs:** the timer checks the thread settings and command policy against
  the saved definition. A mismatch pauses the schedule. An active thread or
  busy checkout skips the occurrence. Otherwise the host accepts the message
  through the desktop's durable Wait operation, `submit_checkout`. The
  host checks the pending claim, current automation revision and saved approval
  immediately before enqueueing. The occurrence receipt says `queued` when the host accepts it. If another run
  takes the checkout during admission, the message waits with its saved
  approval and command policy. Desktop requests continue during that wait.
  A missing or replaced checkout pauses the schedule.
- **Run now:** when an occurrence is due within the catch-up window, the click
  uses that occurrence's scheduled request ID. If the host accepts it and
  restarts before saving the automation receipt, catch-up can submit the same
  ID safely. A click delivered after the scheduled receipt was saved returns
  that receipt if the host received the click within the catch-up window in effect when the occurrence was claimed.
  A retry of an already recorded click returns its receipt even if the original window is unknown.
  When an older store has no record of the window and the match is uncertain, Run now returns a conflict. After downgrade, a manual click overlapping a protected occurrence can return a skipped receipt identifying the scheduled claim.
  Edits preserve that receipt. If an edit overtakes a click and the host cannot
  match it to an occurrence, Run now returns a conflict.
  Other clicks create manual claims. The host saves the click and claim in
  one transaction before submission. A retry looks up the original click,
  even when another occurrence has since become due. Claims and receipts
  track progress from the schedule start time.
  Run now returns the claim ID as `occurrence.request_id` with outcome
  `pending` while work is outstanding. Read the final receipt with
  `get_automation`, or repeat the same click ID.
- **Request ordering:** with automations enabled, desktop requests and individual
  background host requests take turns on the host’s sidecar mutex. A desktop edit can run
  between the scheduler's checkout check and submission. Placement, polling
  and run completion happen in background steps.
- **Operations:** `list_automations` returns each automation's ID, name,
  schedule, target path, state, next run and last result. Use `get_automation`
  with an ID to read the full instructions and occurrence receipts.
  `update_automation` takes the ID
  and a full definition. `pause_automation`, `resume_automation` and
  `delete_automation` take the ID. `run_automation_now` also requires a
  request ID. A create request from a thread supplies `caller.thread_id`;
  the host refuses creation from an automation's own thread. Pause and delete
  stop scheduling. Deletion keeps the ordinary thread and its history. To
  stop a turn already in progress, use the thread's Stop control. The current
  host Stop request addresses the thread as a whole.
- **No replay:** an interrupted run is shown as interrupted and is never
  rerun. The next occurrence runs on schedule.
- **Timer:** one host timer is armed for the earliest next occurrence. It is
  computed again after a wake, a clock change, or an edit. Startup and wake
  both allow one minute for settling.
- **Planned desktop protocol:** a new `automations` capability and its operations. An older
  client does not see the Automations list. If it opens an automation's
  thread, it must not present scheduled turns as approved one call at a time.
  How it shows them is part of the compatibility review.
- **Machines:** an automation belongs to the host it was created on. The
  sidebar shows the selected machine's automations.

## Not in the first version

Cron or RRULE expressions; script jobs and shell prechecks; event triggers
(for example a new Linear issue); scheduling one automation across machines;
alerts beyond the existing thread alerts. Linux command containment and remote
execution keep their existing gates.

## Build order

1. Host: storage, occurrence identity, timer, and admission behind a setting,
   with tests and no UI.
2. Protocol, the Automations list, the automation thread, and the form.
3. The Pi draft-an-automation tool and **Make automation from this thread**.
4. Inspection in the native macOS app before calling it done.

## Acceptance checks

- Schedules: time zones and daylight-saving boundaries, clock changes, sleep
  and wake, missed runs inside and outside the catch-up window.
- No duplicates: lost replies, **Run now** at the same time as a scheduled
  run, crashes before and after admission.
- Sharing: a busy directory, concurrent typed turns, a waiting approval
  skipping the next run, Stop and cleanup.
- Edits racing runs: pause, edit, delete during a run; an unavailable model;
  a moved or deleted project pauses with its reason.
- A scheduled run that tries to draft an automation is refused.
- Settling after wake, and its wait counted against the catch-up window.
- Consent: a run never uses an older revision, and typed turns do not
  inherit the scheduled-run consent.
- With the desktop closed, a Pi edit-and-command run completes under a
  scripted provider. The desktop reconnects to the same history.

## Decided

- **One worktree per automation,** not one per run. Each run continues the
  automation's own work, and there is one branch to review.
- **Automations is a second sidebar section** below Projects, not a switch.
