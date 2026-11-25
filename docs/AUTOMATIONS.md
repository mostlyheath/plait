# Automations

**Proposed design; not implemented, and the host contract is not agreed yet.**
The product decisions below were made by the project owner. The host contract
section is a sketch: it needs a compatibility review before anything is built.
Tracked in [#13](https://github.com/mostlyheath/plait/issues/13).

An automation is a thread that runs on a schedule. You give it instructions
once, say where it works and when, and the host runs it while you are away.
Results arrive the way any thread's do: an unread mark and an alert.

## What a person sees

The sidebar has two lists: **Projects**, as today, and **Automations**. Each
automation is one thread with a card that shows its name, where it works, its
next run, and how its last run ended.

An automation has:

- **Instructions:** the message sent at each run.
- **Where it works:** a project, or any directory on that machine.
- **When:** once at a time, hourly, daily at a time, on weekdays at a time, or
  weekly on chosen days. The time zone is shown and defaults to this Mac's.
  Before you save, the form shows the next three run times.
- **Harness, model and mode:** chosen like a new thread's.
- **Catch-up window:** how late a missed run may still start (default 1 hour).

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
  project's current commit at its first run, and every later run continues
  there. Your checkout is never touched. Its changes stay in that worktree
  until you review them in Changes or the Worktrees tab. An automation never
  commits or pushes.
- **A plain folder or other directory:** runs work in place. If something else
  is running there, the run waits in the existing checkout queue.

### Making one

- **New automation…** in the Automations list opens the form.
- **From a project thread, ask for it.** Say "do this every weekday at 9". The
  agent drafts an automation, shown as a card with the instructions, schedule
  and where it works. Nothing is scheduled until you press **Create**, and you
  can edit the draft first. This works in Pi threads, where Plait supplies the
  tools. Plait adds no tools to Codex or Claude Code (see the
  [roadmap](ROADMAP.md#harnesses-and-tools)), so for those threads:
- **Make automation from this thread** opens the form prefilled with the
  thread's project, harness, model and last message.

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
- Every other missed run is recorded as missed. Missed runs never pile up into
  a burst.

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
- The consent covers scheduled runs and **Run now** only. Turns you type in the
  thread use the thread's normal approval mode.
- **Pause** and **Delete** stop an active run through the existing Stop and
  cleanup path.

## Host contract (sketch, needs agreement)

The host owns automations. The desktop is a client of them, like threads.

- **Storage:** durable automation records (revision, schedule, time zone,
  target, harness settings, catch-up window, paused) and occurrence records
  (due time, outcome: started, skipped, missed, failed). One SQLite migration
  in the existing store.
- **No duplicate runs:** an occurrence's identity is its automation ID, its
  revision and its due time. It is written durably before the run is
  admitted, so a crash or a lost reply cannot start the same occurrence twice.
- **One admission path:** a run is admitted through the same path as a typed
  message. It shares checkout leases, model validation, dirty-file
  protection, the checkout queue, Stop, and request deduplication. The run
  record names the automation revision that allowed it.
- **No replay:** an interrupted run is shown as interrupted and is never
  rerun. The next occurrence runs on schedule.
- **Timer:** one host timer is armed for the earliest next occurrence. It is
  computed again after a wake, a clock change, or an edit. It adds no work to
  host startup's critical path.
- **Protocol:** a new `automations` capability and its operations. An older
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
  a moved or deleted project.
- Consent: a run never uses an older revision, and typed turns do not
  inherit the scheduled-run consent.
- With the desktop closed, a Pi edit-and-command run completes under a
  scripted provider. The desktop reconnects to the same history.

## Open questions

1. **One worktree per automation**, as above, or a fresh worktree per run?
   One worktree matches "each run is a turn": the agent continues its own
   work. Fresh worktrees isolate runs but leave one branch per run to review.
2. **Where the Automations list sits:** a second section under Projects, or a
   switch at the top of the sidebar?
