# Thread graph

**Agreed design.** The host operations are built behind `PLAIT_GRAPH=1`, off by default and not ready to turn on: issues labelled [`blocks-flag`](https://github.com/mostlyheath/plait/issues?q=label%3Ablocks-flag) (#265, #269–#271) must be fixed first. The desktop Graph tab and Discuss enforcement are planned. The project owner agreed the product
decisions and the host contract below. Each pull request that changes storage or the protocol needs its independent review (see
[Contributing](../CONTRIBUTING.md)).

Some work is too big for one thread. The thread graph lets you split a topic
into child threads, say which ones must wait for others, and see the whole
thing at once: what is running, what is waiting, and what needs you.

This is different from the [prompt tree](HOST-FIRST.md#the-prompt-tree),
which is about retries and edits inside one thread. The graph is about links
between threads.

For background graph starts, see [Background requests (agreed)](HOST.md#background-requests-agreed).

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
| Ready | Its dependencies are done, or its first message is in the checkout queue. |
| Discussing / Building | Running. |
| Needs you | Waiting on an approval, or failed. |
| Done | Its last run finished without error, and nothing has been sent since. |

## Two kinds of thread

- **Discuss** threads answer, plan and read. Their tools cannot change files
  or run commands that write. Any number run at once, even in one checkout.
- **Build** threads change files. They take the checkout like any thread
  today. Graph admission follows **Project settings → When this project is busy**
  for automatic worktrees. While its message waits in the desktop checkout queue, the draft stays Ready.

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
checkout uses ordinary project placement and the durable checkout queue. An approval waits
for you as it does in any thread.

If a thread it waits for fails or is interrupted, the draft is **Blocked** and
does not start. Fix or rerun that thread, or remove the link.

## Sending results back

When a parent's children finish, the parent's card and its thread show
"2 children finished". **Send results** puts their final answers, each
bounded and linked, into the parent's message box. You edit and send it
yourself. Plait makes no model call of its own to summarize them.

## Host contract

The graph is a client of the host's ordinary request handler. Enable it with
`PLAIT_GRAPH=1`. With the flag unset, graph operations receive the ordinary
unknown-operation error.

- **Storage.** `STORE/graph.sqlite` contains nodes, fork and dependency edges,
  captured draft settings, and a single graph revision. Drafts have stable
  node IDs. An admitted node also has a unique thread ID. A required
  `op_id` on create or fork replays the original node ID. Reusing it with
  different input or after deleting the draft is refused.
- **Operations.** Use `create_draft`, `fork_thread`, `delete_draft`,
  `set_dependencies`, `set_start_when_ready`, `set_node_kind`, `start_draft`,
  and `send_results`. Dependencies must stay within a project and cannot
  create cycles. Discuss drafts can be represented. Starting one requires
  the planned read-only execution support.
- **Starting.** Draft creation records the checkout device and inode. Before admission,
  a missing or changed checkout blocks the draft with “The project at this path changed”.
  Manual and automatic starts use project placement, falling back to Wait when
  admission offers a choice. The admitted checkout, or a worktree’s parent root,
  must match the requested checkout. The graph saves the first message and uses
  a request ID derived from the node ID to enqueue it once with `submit_checkout`.
  The desktop queue captures approval and command policy, preserves renames while
  waiting, and holds pending messages for Resume after a host restart. A queue
  refusal makes the draft Blocked and supplies the reason shown on its card.
- **Completion.** A summary change refreshes linked threads in that project in
  batches of up to 50, yielding to host requests between batches. Waiting
  drafts use the same batch size and read dependencies once per batch. An archived
  thread with a clean finish can satisfy a dependency. An archived unfinished
  thread blocks it. Forks and dependency answers require the source thread’s
  saved checkout identity to match the draft’s. A replacement directory at the
  same path cannot inherit the previous project’s conversation.
- **Text.** Fork and Send results use `history` and `history_text`. Fork
  searches up to 20 pages. Each graph request shares one four-second deadline across
  its reads. Send results returns completed answers and counts the children left out.
  A brief or assembled draft prompt over 24,576 JSON-escaped content bytes is
  refused with `invalid_request` (“Brief is too long”). Send results includes at most 20
  finished children and budgets the text including links and JSON escaping.
- **Polling.** Request `graph` with a project checkout path. Pass the last
  `graph_revision` as `revision` on the next poll. A matching revision
  returns `unchanged: true`. The workspace event stays the same. The view
  uses graph storage and one unfiltered-by-checkout `list_threads` page,
  including archived threads. The current list protocol identifies threads
  by checkout. Unlinked counts cover that page. A partial page or graph
  sets `truncated`. The host returns a layout for up to 500 cards and
  2,000 edges.

With the graph enabled, each background host request shares the desktop
request FIFO. Admission, result collection, and submission take separate
turns. To edit a draft waiting in the checkout queue, first withdraw its
submission with `cancel_queued`. A graph poll selects unstarted drafts by
the project device and inode captured at creation. A worktree admission
must match the captured checkout path, device, and inode before submission.

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

## Decided

- **Done is not a button.** A thread is done when its last run finished
  without error. Sending it another message makes it running again, and the
  threads waiting for it wait again unless they have already started.
- **Archiving a thread others wait for** asks first. Once archived, it counts
  as done if its last run finished without error. Otherwise the threads
  waiting for it are **Blocked** and name it.
