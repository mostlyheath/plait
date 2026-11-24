# Plait product context

<!-- impeccable:product-schema 1 -->

## Platform
Native macOS desktop built with Rust + GPUI, not a web surface.

## Product and users
A development environment for AI coding-agent work. Developers move between
Project → Machine → Thread, read conversations, approve actions, and inspect
Git changes. Execution belongs to an independent host, not the desktop window.

## Scope and truth
The [README](README.md) describes the current local preview;
[the roadmap](docs/ROADMAP.md) separates planned work from shipped features.
Views show real checkout, host, and thread data. Remote execution, Git-write
controls, a file editor, an embedded terminal, and automatic updates are not
available. Do not show placeholder machines or imply unperformed execution.

## Brand commitments
Original native layout, compact navigation, charcoal surfaces, and restrained
mint, blue, and violet state colors. Use the [Plait artwork](assets/branding/README.md),
native keyboard behavior, and text labels alongside status colors. Motion is
feedback only: nothing moves for decoration, and nothing but the mark of a
running thread moves on its own. Shadows mark floating surfaces, the message
box, and the approval that takes its place. See
[DESIGN.md](DESIGN.md#rules-for-drawing) for the rules, layout, and tokens.

## Principles
Responsiveness, explicit effects, and user control. No blocking I/O in render,
mandatory worktrees, hidden execution, or agent orchestration. Build small,
testable changes with clear state, presentation, and execution boundaries.

## Decisions
M2.5, the daily driver, shipped in v0.1.0-preview.8. v0.1.0-preview.9 gives Pi
threads the person's skills, prompt templates, and user instructions (M8 steps 1
and 2). v0.1.0-preview.10 runs threads in the person's own Codex or Claude Code
beside Pi, each with its own permissions (M7 steps 1 and 2). Current work is the
rest of M7, an ACP client next. Changes to shared contracts,
permissions, or persistence require agreement and compatibility tests. Follow
[Contributing](CONTRIBUTING.md) and the [originality policy](CONTRIBUTING.md#originality-and-licenses).
