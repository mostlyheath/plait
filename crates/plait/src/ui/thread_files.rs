//! Files the selected thread wrote, with their uncommitted line counts, in a
//! card above the composer. The host lists the paths; Git counts the lines.
//! Both reads run in the background, and only when the list could change.

use super::{Plait, Workspace, theme::*};
use gpui::{prelude::*, *};
use plait_core::{
    git::LineCount,
    host::{self, Request},
    model::DiffMode,
};
use std::path::PathBuf;

/// Rows shown before the card scrolls.
const VISIBLE_ROWS: usize = 6;
const ROW_HEIGHT: f32 = 26.;

#[derive(Default)]
pub(super) struct ThreadFiles {
    /// The thread and checkout the paths and counts belong to.
    thread: Option<String>,
    root: PathBuf,
    paths: Vec<PathBuf>,
    provenance: Vec<PathBuf>,
    record_paths: Vec<PathBuf>,
    request_generation: u64,
    requested: bool,
    counts: Vec<LineCount>,
    /// Finished write tools last seen, so streaming text does not refetch.
    tools_done: usize,
    /// Guards each answer against a newer question.
    generation: u64,
    /// The host epoch whose host does not know the operation.
    unsupported: Option<u64>,
    pub collapsed: bool,
}

/// Tools that can change files in the checkout.
fn writes(name: &str) -> bool {
    matches!(name, "edit" | "write" | "command")
}

impl ThreadFiles {
    fn select(&mut self, id: &str, root: &std::path::Path) {
        if self.thread.as_deref() != Some(id) || self.root != root {
            self.reset();
            self.thread = Some(id.into());
            self.root = root.into();
        }
    }

    fn merge_paths(&mut self) {
        self.paths = self
            .provenance
            .iter()
            .chain(&self.record_paths)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
    }

    fn accept_paths(&mut self, generation: u64, files: Vec<PathBuf>) -> bool {
        if self.request_generation != generation {
            return false;
        }
        self.provenance = files;
        self.merge_paths();
        true
    }

    /// Scene state, as if the host and Git had answered.
    #[cfg(test)]
    pub fn inject(&mut self, thread: &str, root: PathBuf, counts: Vec<LineCount>) {
        self.thread = Some(thread.into());
        self.root = root;
        self.paths = counts.iter().map(|count| count.path.clone()).collect();
        self.counts = counts;
    }

    #[cfg(test)]
    pub fn set_paths(&mut self, paths: Vec<PathBuf>) {
        self.paths = paths;
    }

    #[cfg(test)]
    pub fn counted(&self) -> impl Iterator<Item = &LineCount> {
        self.counts.iter()
    }

    /// Forget what was read, so the next snapshot asks again.
    pub fn reset(&mut self) {
        let collapsed = self.collapsed;
        *self = Self {
            collapsed,
            generation: self.generation + 1,
            request_generation: self.request_generation + 1,
            ..Self::default()
        };
    }
}

impl Plait {
    fn shows_thread_files(&self) -> bool {
        !self.preferences.value.hide_changed_files
            && self.host.connection.connected
            && self.host.protocol() >= 3
            && self.workspace.thread_files.unsupported != Some(self.host.connection.epoch)
    }

    /// After a snapshot: ask the host again when the thread changed or a
    /// write tool finished since the last answer.
    pub(super) fn thread_files_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.host.thread.snapshot.as_ref() else {
            return;
        };
        let done = snapshot
            .tools
            .iter()
            .filter(|tool| writes(&tool.name) && tool.status != host::ToolStatus::Running)
            .count();
        let files = &self.workspace.thread_files;
        if files.thread.as_deref() == Some(snapshot.id.as_str())
            && files.root == snapshot.checkout
            && files.tools_done == done
            && files.requested
        {
            return;
        }
        let id = snapshot.id.clone();
        let root = snapshot.checkout.clone();
        self.fetch_thread_files(id, root, cx);
        self.workspace.thread_files.tools_done = done;
    }

    /// Full paged records include command writes and are not capped by the
    /// older changed_files preview. Git still supplies uncommitted counts.
    pub(super) fn record_thread_files(
        &mut self,
        records: &[host::TurnRecord],
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.host.thread.snapshot.as_ref() else {
            return;
        };
        self.workspace
            .thread_files
            .select(&snapshot.id, &snapshot.checkout);
        self.workspace.thread_files.record_paths = records
            .iter()
            .flat_map(|record| record.paths.iter().map(|path| path.path.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        self.workspace.thread_files.merge_paths();
        self.count_thread_files(cx);
    }

    /// After a Git status read: recount, since commits and edits move the numbers.
    pub(super) fn thread_files_status(&mut self, cx: &mut Context<Self>) {
        if self.shows_thread_files()
            && self.workspace.thread_files.thread.is_some()
            && !self.workspace.thread_files.paths.is_empty()
        {
            self.count_thread_files(cx);
        }
    }

    fn fetch_thread_files(&mut self, id: String, root: PathBuf, cx: &mut Context<Self>) {
        self.workspace.thread_files.select(&id, &root);
        if !self.shows_thread_files() {
            return;
        }
        self.workspace.thread_files.requested = true;
        self.workspace.thread_files.request_generation += 1;
        let generation = self.workspace.thread_files.request_generation;
        let epoch = self.host.connection.epoch;
        let watch = self.host.thread.watch_epoch;
        let thread = id.clone();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            client.request::<host::ChangedFilesReply>(&Request::changed_files(id))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.workspace.thread_files.request_generation != generation
                    || !this.host.accepts_watch(epoch, watch, &thread)
                {
                    return;
                }
                match result {
                    Ok(reply) => {
                        if this
                            .workspace
                            .thread_files
                            .accept_paths(generation, reply.files)
                        {
                            this.count_thread_files(cx);
                        }
                    }
                    // A host from before this operation; stop asking it.
                    Err(host::Error::Rejected(_)) => {
                        this.workspace.thread_files.unsupported = Some(epoch);
                        cx.notify();
                    }
                    // Transport trouble shows elsewhere; keep the last list.
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    fn count_thread_files(&mut self, cx: &mut Context<Self>) {
        self.workspace.thread_files.generation += 1;
        if self.workspace.thread_files.paths.is_empty() {
            self.workspace.thread_files.counts.clear();
            return cx.notify();
        }
        let generation = self.workspace.thread_files.generation;
        let thread = self.workspace.thread_files.thread.clone();
        let root = self.workspace.thread_files.root.clone();
        let paths = self.workspace.thread_files.paths.clone();
        let counted = root.clone();
        let backend = self.repository_backend();
        let task = cx
            .background_executor()
            .spawn(async move { backend.line_counts(&counted, &paths) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // A parked tab's workspace restarts this counter, so the owner must match too.
                let files = &this.workspace.thread_files;
                if files.generation != generation || files.thread != thread || files.root != root {
                    return;
                }
                // A failed count keeps the last one; Changes reports Git errors.
                if let Ok(counts) = result {
                    this.workspace.thread_files.counts = counts;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Show a file in Changes, on the side that still has edits.
    pub(super) fn open_thread_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_turn_review();
        let found = self
            .state
            .repository
            .snapshot()
            .ready()
            .and_then(|snapshot| {
                let index = snapshot
                    .changes
                    .iter()
                    .position(|change| change.path == path)?;
                let change = &snapshot.changes[index];
                let mode = if change.unstaged() {
                    DiffMode::Unstaged
                } else {
                    DiffMode::Staged
                };
                Some((index, mode))
            });
        match found {
            Some((index, mode)) => self.select_file(index, mode, window, cx),
            None => {
                self.show_tab(Workspace::Changes, window, cx);
                self.state.repository.refresh();
                self.pump(cx);
            }
        }
    }

    /// The card above the composer, or nothing when the thread has no
    /// uncommitted changes of its own.
    pub(super) fn thread_files_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let files = &self.workspace.thread_files;
        if !self.shows_thread_files()
            || files.counts.is_empty()
            || files.thread.as_ref() != self.host.thread.selected.as_ref()
        {
            return None;
        }
        let added: usize = files.counts.iter().filter_map(|c| c.added).sum();
        let removed: usize = files.counts.iter().filter_map(|c| c.removed).sum();
        let count = files.counts.len();
        let collapsed = files.collapsed;
        let header = row()
            .id("thread-files-toggle")
            .role(Role::Button)
            .aria_expanded(!collapsed)
            .audited()
            .aria_label(format!(
                "{count} {} changed by this thread, {added} lines added, {removed} removed",
                if count == 1 { "file" } else { "files" }
            ))
            .h(px(ROW_HEIGHT))
            .px_3()
            .gap_2()
            .cursor_pointer()
            .child(icon(if collapsed { "right" } else { "down" }).size(px(12.)))
            .child(div().text_color(rgb(colors().muted)).child(format!(
                "{count} {} changed",
                if count == 1 { "file" } else { "files" }
            )))
            .child(counts(Some(added), Some(removed)))
            .child(div().flex_1())
            .child(div().text_color(rgb(colors().subtle)).child("Uncommitted"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.workspace.thread_files.collapsed = !this.workspace.thread_files.collapsed;
                cx.notify();
            }));
        let rows = (!collapsed).then(|| {
            column()
                .id("thread-files")
                .max_h(px(ROW_HEIGHT * VISIBLE_ROWS as f32))
                .overflow_y_scroll()
                .pb_1()
                .children(
                    files
                        .counts
                        .iter()
                        .enumerate()
                        .map(|(index, count)| file_row(index, count, cx)),
                )
        });
        Some(
            column()
                .w_full()
                .max_w(px(CONTENT_WIDTH))
                .mx_auto()
                .border_1()
                .border_color(rgb(colors().edge))
                .rounded_md()
                .bg(rgb(colors().panel))
                .text_size(px(12.))
                .child(header)
                .children(rows)
                .into_any_element(),
        )
    }
}

fn counts(added: Option<usize>, removed: Option<usize>) -> Div {
    row()
        .gap_1()
        .flex_shrink_0()
        .font_family(mono())
        .text_size(px(11.))
        .when_some(added.filter(|n| *n > 0), |el, n| {
            el.child(div().text_color(rgb(colors().mint)).child(format!("+{n}")))
        })
        .when_some(removed.filter(|n| *n > 0), |el, n| {
            el.child(div().text_color(rgb(colors().red)).child(format!("−{n}")))
        })
}

fn file_row(index: usize, count: &LineCount, cx: &mut Context<Plait>) -> Control {
    let name = count
        .path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let directory = count
        .path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = count.path.clone();
    let binary = count.added.is_none();
    row()
        .id(("thread-file", index))
        .role(Role::Button)
        .aria_label(format!("Show {} in Changes", count.path.display()))
        .audited()
        .h(px(ROW_HEIGHT))
        .px_3()
        .gap_2()
        .rounded_sm()
        .cursor_pointer()
        .hover(|el| el.bg(rgb(colors().rail)))
        .child(div().w(px(12.)).flex_shrink_0())
        .child(
            div()
                .flex_shrink_0()
                .text_color(rgb(colors().text))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(colors().subtle))
                .child(directory),
        )
        .when(count.untracked, |el| {
            el.child(tag("thread-file-new", "New", colors().subtle).flex_shrink_0())
        })
        .when(binary, |el| {
            el.child(tag("thread-file-binary", "Binary", colors().subtle).flex_shrink_0())
        })
        .child(counts(count.added, count.removed))
        .on_click(
            cx.listener(move |this, _, window, cx| this.open_thread_file(path.clone(), window, cx)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn records_merge_with_provenance_in_either_reply_order_and_when_expired() {
        for records_first in [false, true] {
            let mut files = ThreadFiles::default();
            files.select("thread", std::path::Path::new("/repo"));
            files.request_generation += 1;
            let request = files.request_generation;
            if !records_first {
                assert!(files.accept_paths(request, vec!["legacy".into()]));
            }
            files.record_paths = vec!["command".into()];
            files.merge_paths();
            if records_first {
                assert!(files.accept_paths(request, vec!["legacy".into()]));
            }
            assert_eq!(
                files.paths,
                vec![PathBuf::from("command"), PathBuf::from("legacy")]
            );
            files.record_paths.clear();
            files.merge_paths();
            assert_eq!(files.paths, vec![PathBuf::from("legacy")]);
            files.select("other", std::path::Path::new("/repo"));
            assert!(!files.accept_paths(request, vec!["late".into()]));
            assert!(files.paths.is_empty());
            let old = files.request_generation;
            files.reset();
            assert!(!files.accept_paths(old, vec!["late after reset".into()]));
        }
    }
}
