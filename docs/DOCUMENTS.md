# Documents

**Agreed direction; the details below are proposed and not implemented.**
The project owner decided that Plait takes on what
[Sidelit](https://github.com/mostlyheath/sidelit) does for documents. That
means reading files as they will appear, pointing at a passage to ask the
agent about it, and editing them, all natively, with no web view. The
storage and host-contract parts need agreement and an independent review
before they are built (see [Contributing](../CONTRIBUTING.md)). Tracked in
[#195](https://github.com/mostlyheath/plait/issues/195).

Agents produce plans, designs, diagrams and docs. Reading them as raw text
in a code viewer is the wrong tool. In Plait, a Markdown file opens rendered,
you select what needs attention, and you send the agent a question, a review
request, or an edit instruction in the thread you are working in.

## Decisions

1. **Native only.** Everything is drawn with GPUI. HTML and MDX files open as
   source with **Open in Browser**. No web view, no live pages, no running
   page scripts. Web previews may be revisited later.
2. **Plait edits documents.** This reverses the read-only file viewer for
   Markdown and plain text. Saving is always explicit, and Plait never
   silently replaces a draft or a file.

## Reading

Files in the Files panel, Changes, and a turn's changed files open rendered
when they are Markdown, with a **Rendered / Source** switch. Rendered is the
default. Source is the existing highlighted viewer.

- **Markdown:**
  - CommonMark with GitHub extensions: tables, task lists, strikethrough,
    footnotes and autolinks.
  - Code blocks are highlighted with the existing highlighter.
  - Frontmatter is folded behind a **Metadata** row.
  - Raw HTML and comments are shown as text, never run.
- **Images:** relative and absolute paths inside the checkout, read through
  the host, so they work on remote machines. PNG, JPEG, GIF and WebP, bounded
  in size. Remote image URLs are not fetched; they show as links.
- **Math:** `$…$` and `$$…$$` are drawn natively. Money and shell variables
  such as `$5` and `$HOME` stay text. Anything the renderer cannot draw falls
  back to its source.
- **Mermaid:** fenced `mermaid` blocks are drawn natively, starting with
  flowcharts and sequence diagrams. Other diagram types, and anything over the
  size limits, show their source with a note. A diagram that fails to draw
  shows its source, and the rest of the document is unaffected.
- **Links:** links to other documents open them in Plait and follow heading
  anchors (`other.md#setup`), using GitHub's anchor rules. Web links open in
  the browser.
- **Find your way:** an **Outline** of the document's headings, **Find**
  (`Cmd-F`) across rendered text, and **Check Links** for local links whose
  file or heading is missing.

Rendering runs on the background executor and is cached per file version.
The view is virtualized, so long documents scroll at the frame budget.

## Asking about a passage

Select text in a rendered or source document. A small card offers
**Explain**, **Review** and **Edit**:

| Action | Asks the agent to | Instruction |
| --- | --- | --- |
| Explain | Answer a question about the selection | Optional |
| Review | Report problems and suggestions | Optional |
| Edit | Change the file | Required |

The request goes to the thread you choose. That is the current thread by
default, or **New thread** in the file's project. It arrives as an ordinary
message with the selection attached as a chip that shows the file, the line
range, and a bounded excerpt. It follows the thread's normal approvals and
queue: a busy thread takes it as a follow-up.

- **Explain and Review ask for no edits.** This is an instruction, not a
  permission boundary, and the card says so. A person who wants it enforced
  sends it to a Discuss thread (see the [thread graph](THREAD-GRAPH.md)).
- **What was captured.** The selection is tied to the file version it was
  made on. If the file has changed since, or the selection is in an unsaved
  draft, the chip says so before you send.
- **Review notes.** Instead of sending at once, **Add to review** keeps the
  request. **Send review** later sends several together, as one message to
  one thread.

## Editing

Markdown and plain-text files can be edited, rendered or in Source.

- **Saving is explicit.** **Save** (`Cmd-S`) writes through the host, so it
  works on any machine. Opening, switching modes, or focusing never writes.
- **Rendered editing keeps what it doesn't understand.** Frontmatter, math,
  raw HTML, comments and footnotes stay byte-for-byte unchanged when you edit
  around them. A rendered edit can normalize ordinary Markdown syntax, such as
  list markers or table padding. Source is for exact control, and the editor
  says when a document can only be edited in Source.
- **Agent changes to a clean document** refresh it, and the changed passages
  are marked until you choose **Mark reviewed**. **Previous** and **Next**
  step through the changes.
- **Agent changes to a document you are editing** never replace your draft.
  The document shows **Compare**, **Reload from Disk** and **Save Copy**.
  Compare puts the file and your draft side by side. You take changes into
  the draft piece by piece, then **Save Merged**.
- **Drafts survive.** Unsaved document drafts are kept in the desktop
  journal, like message drafts, so quitting or a crash does not lose them.
  Quitting with unsaved documents asks: **Save**, **Discard** or **Cancel**.
- Edit requests to the agent use the saved file. If your draft has unsaved
  changes, the card asks you to save first, or to attach the draft's changes
  to the request.

## Host contract (proposed)

- **Reading a document** uses the existing bounded, symlink-safe file read.
  An image read returns bytes with a size cap, for paths inside the
  checkout only.
- **Writing a document** is new. `write_file` writes one file inside the
  checkout. It takes the content and the hash of the version the draft was
  based on, and is refused if the file on disk no longer has that hash. The
  write goes to a temporary file in the same folder, which then replaces the
  original. It never follows symlinks out of the checkout and never writes
  inside `.git`. Each write has a request ID, so a retry never writes twice.
  It is a file write, not a Git write: nothing is staged or committed.
- **Changes** reuse the existing checkout watcher's events.
- **Compatibility:** older hosts refuse `write_file` cleanly. The app then
  shows documents read-only, with a note.

## Speed

Targets:

- a cached document opens in under 50 ms;
- a 1 MB Markdown file renders off the render thread with no dropped frames;
- scrolling holds 60 FPS or better;
- a keystroke while editing costs less than one frame.

Diagrams and math are laid out in the background and cached by their source.

## Not in this design

- HTML, MDX and live-page previews; a web view.
- A general code editor. Source editing is for Markdown and plain text;
  other files keep **Open in Editor**.
- Agent walkthroughs (Sidelit's `--find` and `--note`). These could come
  later as a Pi tool.

## Build order

1. Rendered Markdown in Files, Changes and changed files: blocks, tables,
   images through the host, links and anchors, Outline, and Find.
2. Select and ask: the Explain, Review and Edit card, the attachment chip,
   and review notes.
3. Host `write_file`, then Source editing with explicit save, conflicts,
   Compare, and drafts in the journal.
4. Rendered editing, and change marks with **Mark reviewed**.
5. Math, then Mermaid flowcharts and sequence diagrams.
6. Inspection in the native macOS app, with the speed targets measured.

## Decided later

- **Documents is part of v0.1.0,** beside automations and the thread graph.
  Rendered reading and asking about a passage come first, since they change
  no storage. Editing follows once its host contract is reviewed.

## Open question

Once Plait covers documents, does Sidelit keep its own Pi delivery?
