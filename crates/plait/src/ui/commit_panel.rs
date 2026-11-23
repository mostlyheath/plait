//! Staging and committing from Changes.
//!
//! plait-client runs each write on the background executor and refreshes status
//! afterwards; this file asks for writes and shows how they went. A write is
//! refused while another is queued or running, so a double click is one write.

use super::{Plait, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    git::WriteOp,
    model::{Change, Snapshot, Tracking},
};
use std::path::PathBuf;

pub(super) struct CommitAttempt {
    pub(super) thread: Option<plait_client::ownership::ThreadKey>,
    pub(super) revision: u64,
    pub(super) message: String,
}

/// What sending commits to the remote means for this branch right now.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Sync {
    pub label: String,
    /// The write the Push button runs, if the branch can push at all.
    pub op: Option<WriteOp>,
    /// Why that write is unavailable now, in words.
    pub blocker: Option<&'static str>,
    /// There is an upstream to fetch from.
    pub fetch: bool,
    /// Such as `origin/main`, for the accessible description.
    pub upstream: Option<String>,
}

/// Counts come from the status read; nothing here runs Git.
pub(super) fn sync(named: bool, tracking: Option<&Tracking>) -> Sync {
    let publish = |label: String| Sync {
        label,
        op: Some(WriteOp::Publish),
        blocker: None,
        fetch: false,
        upstream: None,
    };
    let Some(tracking) = tracking.filter(|_| named) else {
        return if named {
            publish("Not on the remote yet".into())
        } else {
            Sync {
                label: "No branch to push".into(),
                op: None,
                blocker: None,
                fetch: false,
                upstream: None,
            }
        };
    };
    let up = &tracking.upstream;
    if tracking.gone {
        return publish(format!("{up} was deleted"));
    }
    let upstream = Some(up.clone());
    let (ahead, behind) = (tracking.ahead, tracking.behind);
    let label = match (ahead, behind) {
        (0, 0) => "Up to date".to_owned(),
        (a, 0) => format!("{a} to push"),
        (0, b) => format!("{b} to pull"),
        (a, b) => format!("{a} to push, {b} to pull"),
    };
    // Only behind: the one useful move is a fast-forward pull.
    let (op, blocker) = match (ahead, behind) {
        (0, 0) => (WriteOp::Push, Some("Nothing to push.")),
        (0, _) => (WriteOp::Pull, None),
        (_, 0) => (WriteOp::Push, None),
        _ => (
            WriteOp::Push,
            Some(
                "This branch and its upstream have each moved on. Merge or rebase them in Terminal.",
            ),
        ),
    };
    Sync {
        label,
        op: Some(op),
        blocker,
        fetch: true,
        upstream,
    }
}

fn sync_of(snapshot: &Snapshot) -> Sync {
    sync(snapshot.named, snapshot.tracking.as_ref())
}

/// A change's paths for staging. A rename also names where it came from, so
/// both sides of it move together.
pub(super) fn change_paths(change: &Change) -> Vec<PathBuf> {
    let mut paths = vec![change.path.clone()];
    paths.extend(change.original.clone());
    paths
}

impl Plait {
    /// Ask for a write and start it if nothing else is running.
    pub(super) fn git_write(&mut self, op: WriteOp, cx: &mut Context<Self>) {
        if !self.checkout_matches_thread() {
            self.notice = Some("Checkout identity is not verified.".into());
            cx.notify();
            return;
        }
        self.git_write_for(self.state.checkout_id(), op, cx);
    }

    pub(super) fn git_write_for(
        &mut self,
        owner: plait_client::state::CheckoutId,
        op: WriteOp,
        cx: &mut Context<Self>,
    ) {
        if self.state.request_write_for(owner, op) {
            self.pump(cx);
        }
        cx.notify();
    }

    /// Stage or unstage one changed file.
    pub(super) fn stage_change(&mut self, index: usize, stage: bool, cx: &mut Context<Self>) {
        let Some(change) = self
            .state
            .repository
            .snapshot()
            .ready()
            .and_then(|snapshot| snapshot.changes.get(index))
        else {
            return;
        };
        let paths = change_paths(change);
        self.git_write(
            if stage {
                WriteOp::Stage(paths)
            } else {
                WriteOp::Unstage(paths)
            },
            cx,
        );
    }

    /// Counted once per status read, not on every frame.
    fn staged_count(&self) -> usize {
        self.state.repository.staged()
    }

    fn commit_message_text(&self, cx: &App) -> String {
        self.workspace
            .commit_message
            .read(cx)
            .text()
            .trim_end()
            .to_owned()
    }

    /// Why Commit is unavailable, in words, or `None` when it can run.
    fn commit_blocker(&self, cx: &App) -> Option<&'static str> {
        if self.state.repository.write().busy() {
            Some("Git is still working.")
        } else if self.state.repository.conflicts() > 0 {
            Some("Resolve the conflicts, then mark each file resolved.")
        } else if self.staged_count() == 0 {
            Some("Stage a change to commit it.")
        } else if self
            .workspace
            .commit_message
            .read(cx)
            .text()
            .trim()
            .is_empty()
        {
            Some("Write a commit message.")
        } else {
            None
        }
    }

    pub(super) fn commit_staged(&mut self, cx: &mut Context<Self>) {
        if self.commit_blocker(cx).is_some() {
            return;
        }
        let message = self.commit_message_text(cx);
        self.commit_sends.insert(
            self.state.checkout_id(),
            CommitAttempt {
                thread: self.host.read_stamp().thread,
                revision: self.workspace.commit_message.read(cx).revision(),
                message: message.clone(),
            },
        );
        self.git_write(WriteOp::Commit { message }, cx);
    }

    /// Clear the message once its commit lands, never on a failure, and not
    /// if it was edited while the commit ran.
    pub(super) fn after_git_write(
        &mut self,
        owner: plait_client::state::CheckoutId,
        cx: &mut Context<Self>,
    ) {
        if owner == self.state.checkout_id() {
            self.refresh_cleanup(cx);
        }
        let committed = self
            .state
            .write_for(owner)
            .and_then(|write| write.committed.take())
            .is_some();
        let Some(sent) = self.commit_sends.remove(&owner) else {
            return;
        };
        if committed
            && owner == self.state.checkout_id()
            && sent.thread == self.host.read_stamp().thread
            && sent.revision == self.workspace.commit_message.read(cx).revision()
            && sent.message == self.commit_message_text(cx)
        {
            self.workspace
                .commit_message
                .update(cx, |field, cx| field.set_text("", cx));
        }
    }

    /// Ask in a native alert, then discard. Undo stays offered afterwards.
    pub(super) fn confirm_discard(
        &mut self,
        op: WriteOp,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = self.state.checkout_id();
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some("Plait keeps a copy, so you can undo this until your next Git action."),
            &[
                PromptButton::Other("Discard".into()),
                PromptButton::Cancel("Cancel".into()),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| this.git_write_for(owner, op, cx));
            }
        })
        .detach();
    }

    /// Discard the selected file's unstaged changes, or delete it if untracked.
    pub(super) fn discard_file(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(change) = self
            .state
            .repository
            .snapshot()
            .ready()
            .and_then(|snapshot| snapshot.changes.get(index))
        else {
            return;
        };
        let untracked = change.index == b'?';
        let title = if untracked {
            format!("Delete {}?", change.display_path())
        } else {
            format!("Discard unstaged changes to {}?", change.display_path())
        };
        let op = WriteOp::Discard {
            path: change.path.clone(),
            untracked,
            hunk: None,
        };
        self.confirm_discard(op, title, window, cx);
    }

    /// Push, pull, or publish, whichever the branch's state calls for.
    /// The Push and Pull menu items each run only their own.
    pub(super) fn push(&mut self, cx: &mut Context<Self>) {
        let Some(sync) = self.state.repository.snapshot().ready().map(|s| sync_of(s)) else {
            return;
        };
        if let (Some(op), None) = (sync.op, sync.blocker) {
            self.git_write(op, cx);
        }
    }

    /// The Pull menu item: only when the branch is behind and has not diverged.
    pub(super) fn pull(&mut self, cx: &mut Context<Self>) {
        let Some(sync) = self.state.repository.snapshot().ready().map(|s| sync_of(s)) else {
            return;
        };
        if sync.op == Some(WriteOp::Pull) {
            self.git_write(WriteOp::Pull, cx);
        }
    }

    /// The branch against its upstream, with Fetch and Push or Publish.
    fn sync_row(&self, busy: bool, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let sync = sync_of(self.state.repository.snapshot().ready()?);
        let (label, description) = match sync.op {
            Some(WriteOp::Publish) => (
                "Publish",
                "Push this branch to its remote and track it there.",
            ),
            Some(WriteOp::Pull) => (
                "Pull",
                "Fetch and move this branch forward to its upstream. Never merges or rebases.",
            ),
            _ => ("Push", "Push commits to the remote. Hooks run."),
        };
        Some(
            row()
                .gap_1()
                .child(
                    div()
                        .id("sync-state")
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(rgb(colors().muted))
                        .when_some(sync.upstream, |el, up| {
                            el.aria_description(format!("Upstream {up}"))
                        })
                        .child(sync.label),
                )
                .when(sync.fetch, |el| {
                    el.child(
                        button("fetch", "Fetch")
                            .aria_description("Fetch from the remote. Nothing local changes.")
                            .disabled(busy)
                            .on_activate(cx, |this, _, cx| this.git_write(WriteOp::Fetch, cx)),
                    )
                })
                .when(sync.op.is_some(), |el| {
                    el.child(
                        button("push", label)
                            .aria_description(sync.blocker.unwrap_or(description))
                            .disabled(busy || sync.blocker.is_some())
                            .on_activate(cx, |this, _, cx| this.push(cx)),
                    )
                }),
        )
    }

    /// Put the cursor in the commit message, on the Changes tab.
    pub(super) fn focus_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(super::Workspace::Changes, window, cx);
        window.focus(&self.workspace.commit_message.focus_handle(cx), cx);
        cx.notify();
    }

    /// A thread is running in this checkout, so its edits may be half done.
    pub(super) fn running_here(&self) -> bool {
        let Some(root) = self.state.repository.snapshot().ready().map(|s| &s.root) else {
            return false;
        };
        self.host.connection.checkout_has_run(root)
    }

    /// Stage-all controls, the message field, Commit, and the last result.
    pub(super) fn commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let write = self.state.repository.write();
        let busy = write.busy();
        let staged = self.staged_count();
        let changed = self
            .state
            .repository
            .snapshot()
            .ready()
            .map_or(0, |snapshot| snapshot.changes.len());
        let blocker = self.commit_blocker(cx);
        let note = write.note.clone();
        column()
            .id("commit-box")
            .role(Role::Group)
            .aria_label("Commit")
            .audited()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .gap_2()
            .border_t_1()
            .border_color(rgb(colors().border))
            .children(self.sync_row(busy, cx))
            .when(
                self.workspace.turn_review.target.is_none() || self.workspace.turn_review.current,
                |el| {
                    el.child(
                        row()
                            .gap_1()
                            .child(
                                button("stage-all", "Stage all")
                                    .disabled(busy || changed == staged)
                                    .on_activate(cx, |this, _, cx| {
                                        this.git_write(WriteOp::StageAll, cx)
                                    }),
                            )
                            .child(
                                button("unstage-all", "Unstage all")
                                    .disabled(busy || staged == 0)
                                    .on_activate(cx, |this, _, cx| {
                                        this.git_write(WriteOp::UnstageAll, cx)
                                    }),
                            ),
                    )
                },
            )
            .child(
                div()
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if this.workspace.commit_message.read(cx).is_composing() {
                            return;
                        }
                        // Plain Return only; modified Return is left to the field.
                        if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
                            this.commit_staged(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(self.workspace.commit_message.clone()),
            )
            .child(
                // The count lives on the button, where it fits at any width.
                toned_button(
                    "commit",
                    match staged {
                        0 => "Commit".to_owned(),
                        1 => "Commit 1 file".to_owned(),
                        n => format!("Commit {n} files"),
                    },
                    // The filled button is the one that can act. A faded fill
                    // reads as a smudge, so one that cannot act is an outline
                    // in hint text, and hover says what is missing.
                    if blocker.is_some() {
                        Tone::Filled
                    } else {
                        Tone::Primary
                    },
                )
                .w_full()
                .justify_center()
                .aria_description(blocker.unwrap_or("Commit the staged changes. Hooks run."))
                .when_some(blocker, |button, blocker| {
                    button
                        .text_color(rgb(colors().subtle))
                        .cursor_default()
                        .a11y_synthetic_children(|builder| {
                            builder.parent_node().set_disabled();
                        })
                        .tooltip(move |_, cx| {
                            cx.new(|_| super::thread_controls::TextTooltip(blocker.to_owned()))
                                .into()
                        })
                })
                .on_activate(cx, |this, _, cx| this.commit_staged(cx)),
            )
            .when(self.running_here(), |el| {
                el.child(tag(
                    "commit-running-warning",
                    "A thread is running in this checkout; its edits may be unfinished.",
                    colors().amber,
                ))
            })
            .when(busy, |el| {
                el.child(tag(
                    "commit-git-working",
                    "Git is working…",
                    colors().subtle,
                ))
            })
            .when_some(note.filter(|_| !busy), |el, (note, failed)| {
                el.child(
                    row()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(11.))
                                .text_color(rgb(if failed {
                                    colors().red
                                } else {
                                    colors().subtle
                                }))
                                .child(note),
                        )
                        .when_some(write.undo.clone(), |el, backup| {
                            el.child(
                                button("undo-discard", "Undo")
                                    .aria_description("Put the discarded file back as it was.")
                                    .on_activate(cx, move |this, _, cx| {
                                        this.git_write(WriteOp::Restore(backup.clone()), cx)
                                    }),
                            )
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{change_paths, sync};
    use plait_client::{
        git::WriteOp,
        model::{Change, Tracking},
    };
    use std::path::PathBuf;

    #[test]
    fn a_rename_stages_both_of_its_paths() {
        let mut change = Change {
            path: PathBuf::from("new.rs"),
            original: None,
            index: b'R',
            worktree: b' ',
        };
        assert_eq!(change_paths(&change), [PathBuf::from("new.rs")]);
        change.original = Some(PathBuf::from("old.rs"));
        assert_eq!(
            change_paths(&change),
            [PathBuf::from("new.rs"), PathBuf::from("old.rs")]
        );
    }

    #[test]
    fn push_follows_the_branch_and_its_upstream() {
        let up = |ahead, behind, gone| Tracking {
            upstream: "origin/main".into(),
            ahead,
            behind,
            gone,
        };
        let none = sync(false, None);
        assert_eq!((none.op, none.fetch), (None, false));
        let local = sync(true, None);
        assert_eq!(local.op, Some(WriteOp::Publish));
        assert_eq!(sync(true, Some(&up(0, 0, true))).op, Some(WriteOp::Publish));
        let even = sync(true, Some(&up(0, 0, false)));
        assert_eq!(even.label, "Up to date");
        assert_eq!(even.upstream.as_deref(), Some("origin/main"));
        assert!(even.blocker.is_some() && even.fetch);
        let ahead = sync(true, Some(&up(2, 0, false)));
        assert_eq!(ahead.label, "2 to push");
        assert_eq!((ahead.op, ahead.blocker), (Some(WriteOp::Push), None));
        let behind = sync(true, Some(&up(0, 3, false)));
        assert_eq!((behind.op, behind.blocker), (Some(WriteOp::Pull), None));
        let diverged = sync(true, Some(&up(1, 3, false)));
        assert_eq!(diverged.label, "1 to push, 3 to pull");
        assert!(diverged.blocker.unwrap().contains("Merge or rebase"));
    }
}
