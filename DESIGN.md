# Plait desktop design

Plait is a native tool for working with coding agents. Design for clear
execution state, informed approvals, keyboard access, and review of real
changes. Use original application design and the [contribution rules](CONTRIBUTING.md#originality-and-licenses).
Show only data the host or client actually has; test scenes may inject sample
state. Do not imply that an action ran or succeeded without evidence.

## Layout

The compact navigation rail groups projects and threads. Keep project, machine,
checkout, and thread context visible. The workspace presents conversation,
Changes, History, or Files; a Git inspector is optional. Side panels resize
from their inner edges and can be hidden. Preserve a readable workspace at
narrow widths.

- A 40-point tab bar serves as the window bar, with native traffic lights,
  sidebar toggle, tabs, overflow, and New tab. Tabs show titles and meaningful
  state in words and color. Empty bar space moves the window.
- A thread's 36-point workspace bar shows project, machine and branch, view
  switcher, and inspector toggle. Omit machine at narrow widths. Content with
  no subviews omits this bar.
- Tabs retain their own view, scroll position, find state, and draft. A new
  thread action creates a separate draft. Closing a tab does not stop its run.
  Virtualize long lists and hidden tab content.
- Worktrees use a branch graph with neutral lanes and nodes, state colors,
  readable branch and thread labels, and a detail pane. Place detail beside the
  graph when wide and below it when narrow. Name unknown or bounded measurements
  rather than displaying a false zero.
- Settings and Project settings are centered window modals over the workspace.
  Preserve the underlying selection and draft, keep focus in the modal, and
  return focus on dismissal. Opening another project's settings does not select
  that checkout.

## Interaction

- Use native menus, traffic lights, and folder picker. Show keyboard focus and
  accessible names, including full paths and state where visible text truncates.
  Do not rely on color alone. Keep long lists virtualized.
- Use neutral selection and hover fills. Reserve pills for meaningful metadata.
  Name loading, offline, failure, and refusal states and give a useful recovery
  action where one exists. Explain disabled controls in hover text.
- The conversation is a log of turns. Requests and replies have visible author
  labels; requests use a raised neutral panel. Reasoning and tool calls use
  compact disclosures. Preserve selection, copy, links, and scroll position
  when history loads. Do not invent durations or outcomes.
- A recorded turn shows the host-reported steps and changed files, with links
  to its recorded diff. Undo and redo controls reflect availability; explain
  conflicts and excluded paths. Recorded diffs remain distinct from current Git
  changes and do not offer file or hunk writes.
- Image attachments show thumbnails and removal before sending. Show upload,
  failure, and refusal states without silently losing a draft. Follow the
  [host contract](docs/HOST.md#protocol-version-6) for limits.
- The composer holds the draft and Send; Stop is separate during a run. Show
  pending messages and their available actions. Suggestions keep focus in the
  draft. Keep IME composition from submitting and preserve unsent revisions.
- Approvals show the exact effect, risk, protected paths, and explicit choices.
  Keyboard shortcuts use the same availability checks as the buttons.
- Changes separates staged and unstaged diffs. File rows name status and path;
  diff actions appear where applicable. Keep commit results and unavailable
  branch actions legible. History shows commit identity, message, and patch.
- Files is read-only. Editor and terminal handoffs use the configured choice or
  a custom command with placeholders; report failures in plain language. Remote
  file handoffs are unavailable as described in the [remote guide](docs/REMOTE.md).
- Settings place controls at their scope: thread choices near the composer,
  project choices in Project settings, and app or host choices in Settings.
  Mask credentials and make reveal explicit. Confirm trusted command access
  with its consequences.
- Find shows match count, navigation, and current match without relying on color.
  State when conversation results cover only loaded messages. The palette and
  pickers use a centered search field, compact rows, and keyboard hints.

## Tokens

Colors are semantic roles in `plait_client::theme::Palette`. The default dark
palette is Stillwater:

| Role | Color |
| --- | --- |
| Background / navigation / panel | `#1f222e` / `#242835` / `#2b303f` |
| Border / control outline / selection | `#313547` / `#464d66` / `#2e3243` |
| Text / secondary / hint | `#dfe3ed` / `#b0b7cb` / `#9da5bf` |
| Addition / selected option | mint `#70d689` |
| Running / modification / link / focus | blue `#74cde2` |
| Thread identity / keyword | violet `#ec8ac7` |
| Attention / error | sand `#eec96e` / coral `#ec7368` |

Themes map to the same roles. Maintain contrast floors of 7:1 for body text,
4.5:1 for secondary text, and 3:1 for hints and accents. Accent colors carry
state rather than decoration. Diff lines use a gutter mark and a faint tint.
Code uses semantic syntax colors; plain identifiers remain main text.

Use system sans-serif at 11–15 points, with 13-point body and navigation text.
Conversation text uses 21-point line height. Code, paths, and commands use a
monospaced font at 12 points. Conversation, composer, and settings content
are capped at 760 points. Spacing follows a 4-point base. Preserve gradients
in supplied brand artwork only; avoid decorative gradients, shadows, and cards.
Render timings are an optional diagnostic, not permanent chrome.
