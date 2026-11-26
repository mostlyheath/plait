# Thread graph

**Proposed design; not implemented.** The project owner made the product
decisions below. The host contract still needs agreement, and each pull
request that changes storage or the protocol needs its independent review (see
[Contributing](../CONTRIBUTING.md)).

Some work is too big for one thread. The thread graph lets you split a topic
into child threads, say which ones must wait for others, and see the whole
thing at once: what is running, what is waiting, and what needs you.

This is different from the [prompt tree](HOST-FIRST.md#the-prompt-tree),
which is about retries and edits inside one thread. The graph is about links
between threads.

## What a person sees

Each project has a **Graph** tab, opened like Worktrees: from the project's
menu, the command palette, or **View → Graph**. It closes the same way. The
sidebar is unchanged; the graph is another view of the same threads.

The graph draws a project's linked threads as cards, top to bottom:

- **Lines** run from a parent thread to the children it was forked into.
- **Dashed lines** run from a thread to a thread that waits for it.
- A thread nobody linked does not appear. The tab lists how many unlinked
  threads the project has, with a link to Every thread.

Each card shows the thread's title, its kind, its harness and model, its state,
and **unblocks N** when other threads wait for it. A card that needs you is
outlined in the accent color. Click a card to open its thread; Escape or
**Back** returns to the graph.

### States

A card shows one state, taken from the thread:

| State | Meaning |
| --- | --- |
| Draft | Written, not started, and not waiting for anything. |
| Waiting k/n | Waits for n threads; k of them are done. |
| Blocked | A thread it waits for failed or was interrupted. Names which. |
| Queued #n | Its checkout is busy; it is nth in line. |
| Discussing / Building | Running. |
| Needs you | Waiting on an approval, or failed. |
| Done | Its last run finished, and nothing has been sent since. |

## Two kinds of thread

- **Discuss** threads answer, plan and read. Their tools cannot change files
  or run commands that write. Any number run at once, even in one checkout.
- **Build** threads change files. They take the checkout like any thread
  today: a busy project offers its own worktree or a place in line, following
  **Project settings → When this project is busy**.

A thread's kind is chosen when it is created and shown on its card. You can
change it while the thread is not running. Existing threads are Build.

## Making a graph

- **Fork from here.** On any message in any thread, **Fork from here** opens a
  child draft. You choose what it starts with: a brief you write, the
  conversation up to that message, or both. Long conversations are clipped
  to a bounded excerpt with a marker where text was left out.
- **Ask for a split.** In a Pi thread, say "split this into parts". The agent
  drafts the child threads, with their kinds and which waits for which, as a
  card in the thread. Nothing is created until you press **Create**, and you
  can edit the draft first. Plait adds no tools to Codex or Claude Code (see
  the [roadmap](ROADMAP.md#harnesses-and-tools)), so in those threads you
  fork by hand.
- **Dependencies** are set on a draft: **Waits for…** picks other threads in
  the project. A link that would make a loop is refused.

## When it can start

A draft that waits for other threads starts on its own once all of them are
done. You choose this per draft with **Start when ready**, which is on by
default. Turn it off, and the draft turns **Ready** and alerts you instead.

When it starts, the results of the threads it waited for are added to its
first message: each one's final answer, bounded in length, with a link to
the thread. Starting follows the usual rules: a Build thread in a busy
checkout takes its own worktree or a place in line, and an approval waits for
you as it does in any thread.

If a thread it waits for fails or is interrupted, the draft is **Blocked** and
does not start. Fix or rerun that thread, or remove the link.

## Sending results back

When a parent's children finish, the parent's card and its thread show
"2 children finished". **Send results** puts their final answers, each
bounded and linked, into the parent's message box. You edit and send it
yourself. Plait makes no model call of its own to summarize them.

## Host contract (proposed)

The host owns the graph; clients render what it streams. This builds on the
host model from the closed [#86](https://github.com/mostlyheath/plait/pull/86).

- **Storage:**
  - nodes, which are threads and drafts;
  - kind (discuss or build);
  - `start_when_ready`;
  - edges: `fork` (parent to child) and `depends_on`;
  - a graph revision per project.

  One additive migration. Existing threads need no migration: an unlinked
  thread is a Build thread with no edges.
- **State is cheap.** It comes from thread columns that already exist or are
  backfilled once, never from parsing transcripts.
- **Operations:**
  - `create_draft` and `delete_draft`;
  - `fork_thread`, which seeds the child from bounded pages of the parent;
  - `set_dependencies`, which refuses loops;
  - `set_node_kind`, which is refused while the thread runs;
  - `send_results`;
  - `graph`, a bounded read of one project with a truncation flag.

  Each operation has an optional operation ID. An older host answers
  `invalid_request`, and the app hides the Graph tab.
- **Starting is the host's job.** When a thread finishes, the host checks the
  drafts waiting for it. A draft whose waits are all done and whose
  `start_when_ready` is on is admitted through the same path as a typed
  message. The start is recorded before admission, so a crash or a lost reply
  never starts a draft twice. Interrupted runs are never replayed.
- **Discuss threads are enforced, not requested.** Their file-writing tools
  are not offered, and their commands run read-only in the sandbox. On a
  machine without the sandbox, discuss threads cannot run commands.
- **Layout is the host's job too.** The host lays the graph out in layers,
  in a fixed order, so every client draws it the same way. A state change
  never moves a card. Only a new or removed link moves cards.
- **Stream:** the workspace stream carries each project's graph revision. A
  client refetches only when the revision changes and the tab is open.

## Speed

Targets:

- zero cost while no Graph tab is open;
- the host reads a 500-card graph in under 5 ms;
- opening a cached graph takes under 50 ms;
- a state change patches one card without new layout;
- frames draw only the visible cards, in under 8 ms at 500 cards.

Panning and zooming never wait on the host.

## Not in the first version

- dragging cards to place them by hand;
- links between projects or machines;
- automatic roll-up turns;
- a separate "meta agent" that manages the graph;
- graphs for Codex or Claude Code threads drawn by those agents.

## Build order

1. **Host:** storage, operations, state, layout, and starting on dependencies,
   with tests at 500 nodes.
2. **Discuss enforcement** for Pi, with tests that writes and writing commands
   are refused.
3. **Protocol and the Graph tab:** cards, lines, states, opening a thread, and
   Back.
4. **Fork from here,** dependencies on drafts, **Send results,** and the Pi
   **Ask for a split** card.
5. **Inspection** in the native macOS app, with the speed targets measured.

## Open questions

1. **Should Done be explicit?** A discuss thread may answer, then be asked a
   follow-up. Is "last run finished" enough, or do threads need a **Mark
   done** button?
2. **How does an archived thread count?** Archiving a thread others wait for
   could count as done, count as removed, or be refused until the links are
   removed.
