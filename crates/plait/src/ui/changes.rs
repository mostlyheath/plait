use super::{OpenRepository, Plait, Refresh, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{
    git::WriteOp,
    model::{Change, DiffKind, DiffLine, DiffMode},
    patch::PatchRow,
    state::Load,
};
use std::{path::Path, sync::Arc};

/// Row height for diff lines and file headings: 9 points of leading over the
/// code size (21 at 12, 22 at 13, 23 at 14) so 13- and 14-point faces fit
/// without clipping while keeping uniform_list rows equal height.
fn diff_line_height() -> f32 {
    code_size() + 9.
}

pub(super) fn diff_limit_notice(recorded: bool) -> (&'static str, &'static str) {
    if recorded {
        (
            "Limited",
            "This recorded preview reached the 512 KiB or 10,000-line limit.",
        )
    } else {
        (
            "Preview limited to 512 KiB / 10,000 lines. Repository files are unchanged.",
            "The preview is limited; repository files are unchanged.",
        )
    }
}

/// Lines picked in the Changes diff by clicking line numbers. It applies only
/// while the same file and side are shown; another file ignores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LinePick {
    pub checkout: plait_client::state::CheckoutId,
    pub epoch: u64,
    pub path: std::path::PathBuf,
    pub mode: DiffMode,
    pub anchor: usize,
    pub end: usize,
}

impl LinePick {
    pub(super) fn range(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor.min(self.end)..=self.anchor.max(self.end)
    }
}

/// What a diff offers beyond reading. History passes the default: none.
#[derive(Default)]
pub(super) struct DiffControls {
    /// Hunk buttons on `@@` lines.
    hunks: Option<HunkTarget>,
    /// Line numbers can be clicked to pick lines; `Some(None)` before any pick.
    pick: Option<Option<std::ops::RangeInclusive<usize>>>,
    /// Shown above the footer while lines are picked.
    bar: Option<AnyElement>,
}

/// One-letter state and its tint. The accessible name carries the full word.
fn change_mark(label: &str) -> (&'static str, u32) {
    match label {
        "New" => ("N", colors().mint),
        "Added" => ("A", colors().mint),
        "Deleted" => ("D", colors().red),
        "Conflict" => ("!", colors().red),
        "Renamed" => ("R", colors().blue),
        _ => ("M", colors().blue),
    }
}

/// File name first, so truncation removes the directory before the name.
/// What staging does to this file, in words. For an unmerged file, staging
/// is how Git records the conflict as resolved.
fn stage_verb(change: &Change, stage: bool) -> &'static str {
    match (stage, change.conflicted()) {
        (true, true) => "Mark resolved",
        (true, false) => "Stage",
        (false, _) => "Unstage",
    }
}

/// Where a diff's hunks stage to, captured once per frame for visible rows.
pub(super) struct HunkTarget {
    checkout: plait_client::state::CheckoutId,
    epoch: u64,
    path: std::path::PathBuf,
    mode: DiffMode,
    busy: bool,
}

/// Shown on hover beside a hunk's `@@` line; built only for visible rows.
fn hunk_button(target: &HunkTarget, hunk: usize, header: &str, cx: &mut Context<Plait>) -> Control {
    let owner = target.checkout;
    let epoch = target.epoch;
    let stage = target.mode == DiffMode::Unstaged;
    let op = WriteOp::Hunk {
        path: target.path.clone(),
        from: target.mode,
        index: hunk,
        header: header.to_owned(),
    };
    quiet_button(
        ("hunk-stage", hunk),
        if stage { "Stage hunk" } else { "Unstage hunk" },
        "diff-hunk",
    )
    .h(px(diff_line_height() - 2.))
    .font_family(".SystemUIFont")
    .text_size(px(11.))
    .disabled(target.busy)
    .on_activate(cx, move |this, _, cx| {
        if this.state.checkout_id() == owner && this.state.repository.epoch() == epoch {
            this.git_write(op.clone(), cx);
        }
    })
}

/// Beside Stage hunk on unstaged hunks; asks before anything is lost.
fn discard_hunk_button(
    target: &HunkTarget,
    hunk: usize,
    header: &str,
    cx: &mut Context<Plait>,
) -> Control {
    let owner = target.checkout;
    let epoch = target.epoch;
    let op = WriteOp::Discard {
        path: target.path.clone(),
        untracked: false,
        hunk: Some((hunk, header.to_owned())),
    };
    let name = target.path.file_name().map_or_else(
        || target.path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    quiet_button(("hunk-discard", hunk), "Discard hunk…", "diff-hunk")
        .h(px(diff_line_height() - 2.))
        .font_family(".SystemUIFont")
        .text_size(px(11.))
        .disabled(target.busy)
        .on_activate(cx, move |this, window, cx| {
            if this.state.checkout_id() != owner || this.state.repository.epoch() != epoch {
                return;
            }
            this.confirm_discard(
                op.clone(),
                format!("Discard this hunk in {name}?"),
                window,
                cx,
            )
        })
}

/// A patch line's text. A hunk header's ranges are quieter than the context
/// Git adds after them, which usually names the enclosing function.
fn line_text(line: &DiffLine) -> AnyElement {
    let text = SharedString::from(line.text.clone());
    if line.kind != DiffKind::Hunk {
        return text.into_any_element();
    }
    let ranges = HighlightStyle {
        color: Some(rgb(colors().subtle).into()),
        ..Default::default()
    };
    let end = plait_client::patch::hunk_ranges_end(&text);
    StyledText::new(text)
        .with_highlights([(0..end, ranges)])
        .into_any_element()
}

/// Where one file of a commit's patch begins: its name, then its directory,
/// as the changed-file list shows them. It stands for the file's `diff` line.
fn file_heading(line: usize, name: &Arc<str>, directory: &Arc<str>) -> AnyElement {
    let path = if directory.is_empty() {
        name.to_string()
    } else {
        format!("{directory}/{name}")
    };
    row()
        .id(("diff-file", line))
        .role(Role::Heading)
        .aria_label(path)
        .audited()
        .w_full()
        .h(px(diff_line_height()))
        .px_4()
        .bg(rgb(colors().rail))
        .border_t_1()
        .border_color(rgb(colors().border))
        .text_size(px(12.))
        .child(
            div()
                .flex_shrink_0()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(colors().text))
                .child(SharedString::from(name.clone())),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(11.))
                .text_color(rgb(colors().subtle))
                .child(SharedString::from(directory.clone())),
        )
        .into_any_element()
}

fn split_path(change: &Change) -> (String, String) {
    let display = change.display_path();
    let path = Path::new(&display);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let parent = path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name {
        Some(name) => (name, parent),
        None => (display, String::new()),
    }
}

impl Plait {
    fn file_row(&self, index: usize, cx: &mut Context<Self>) -> Control {
        let Some(snapshot) = self.state.repository.snapshot().ready() else {
            return div().id(("file", index)).audited();
        };
        let change = &snapshot.changes[index];
        let selected = self
            .state
            .repository
            .selected_diff()
            .as_ref()
            .is_some_and(|key| key.file == index);
        let mode = if change.unstaged() {
            DiffMode::Unstaged
        } else {
            DiffMode::Staged
        };
        let label = change.label();
        let (mark, tint) = change_mark(label);
        let (name, parent) = split_path(change);
        let group = SharedString::from(format!("file-row-{index}"));
        nav_row(
            ("file", index),
            format!(
                "{}, {}, {}{}",
                change.display_path(),
                label,
                if change.staged() { "staged " } else { "" },
                if change.unstaged() { "unstaged" } else { "" }
            ),
            selected,
        )
        .aria_selected(selected)
        .group(group.clone())
        .relative()
        .rounded_none()
        .px_3()
        .child(
            div()
                .w(px(12.))
                .flex_shrink_0()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(tint))
                .child(mark),
        )
        .child(
            row()
                .flex_1()
                .min_w_0()
                .child(div().truncate().text_color(rgb(colors().text)).child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(rgb(colors().subtle))
                        .child(parent),
                ),
        )
        .child(tag(
            "file-change-location",
            if change.staged() && change.unstaged() {
                "index + worktree"
            } else if change.staged() {
                "index"
            } else {
                "worktree"
            },
            colors().subtle,
        ))
        // Anything left in the worktree stages; otherwise the row unstages.
        // It overlays the state tag on hover, so the path keeps its width.
        .child({
            let stage = change.unstaged();
            let verb = stage_verb(change, stage);
            quiet_button(("file-stage", index), verb, group.clone())
                .aria_label(format!("{verb} {}", change.display_path()))
                .absolute()
                .right(px(8.))
                .bg(rgb(colors().panel))
                .disabled(self.state.repository.write().busy())
                .on_activate(cx, move |this, _, cx| {
                    this.stage_change(index, stage, cx);
                    cx.stop_propagation();
                })
        })
        .on_click(cx.listener(move |this, _, window, cx| this.select_file(index, mode, window, cx)))
    }

    pub(super) fn file_panel(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        if !compact && self.workspace.turn_review.target.is_some() {
            return self.recorded_file_panel(cx);
        }
        let repo = &self.state.repository;
        let mut read = None;
        let body = match &repo.snapshot() {
            Load::Empty => empty(
                "Choose a repository",
                "Open a local Git working directory to inspect its changes.",
            )
            .into_any_element(),
            Load::Loading => empty(
                "Reading repository…",
                "Git is running in the background. You can keep browsing threads.",
            )
            .into_any_element(),
            Load::Error(error) => column()
                .flex_1()
                .items_center()
                .child(empty("Cannot read repository", error.clone()))
                .child(
                    toned_button("choose-another", "Choose another repository…", Tone::Filled)
                        .mb_4()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_repository(&OpenRepository, window, cx)
                        })),
                )
                .into_any_element(),
            Load::Ready(snapshot) => {
                let count = snapshot.changes.len();
                read = Some(snapshot.elapsed.as_millis());
                column()
                    .flex_1()
                    .min_h_0()
                    .child(
                        column()
                            .px_3()
                            .pt_1()
                            .pb_2()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                // The control's own padding completes the gap after the icon.
                                row()
                                    .gap_1()
                                    .child(icon("branch").text_color(rgb(colors().muted)))
                                    .child(self.branch_control(compact, &snapshot.branch, cx))
                                    .child(div().flex_1())
                                    .child(tag(
                                        if compact {
                                            "inspector-changed-count"
                                        } else {
                                            "changes-changed-count"
                                        },
                                        format!(
                                            "{} changed {}",
                                            count,
                                            if count == 1 { "file" } else { "files" }
                                        ),
                                        colors().subtle,
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors().subtle))
                                    .truncate()
                                    .child(home_relative(&snapshot.root)),
                            ),
                    )
                    .when(!compact, |el| el.child(self.review_scope_button(cx)))
                    .child(if count == 0 && snapshot.plain {
                        empty("Not a Git repository", super::PLAIN_FOLDER).into_any_element()
                    } else if count == 0 {
                        empty(
                            "Working tree clean",
                            "There are no staged, unstaged, or untracked changes.",
                        )
                        .into_any_element()
                    } else {
                        uniform_list(
                            if compact {
                                "inspector-files"
                            } else {
                                "changed-files"
                            },
                            count,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| this.file_row(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.workspace.file_scroll)
                        .flex_1()
                        .min_h_0()
                        .into_any_element()
                    })
                    .into_any_element()
            }
        };
        let ready = self.state.repository.snapshot().ready().is_some();
        column()
            .flex_1()
            .min_h_0()
            .child(body)
            .when(!compact && ready, |el| el.child(self.commit_box(cx)))
            .child(
                row()
                    .h(px(36.))
                    .px_2()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(rgb(colors().border))
                    .child(
                        button(
                            if compact {
                                "refresh-inspector"
                            } else {
                                "refresh-files"
                            },
                            "Refresh",
                        )
                        .aria_keyshortcuts("Meta+R")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.refresh(&Refresh, window, cx)),
                        ),
                    )
                    .child(tag(
                        if compact {
                            "inspector-refresh-shortcut"
                        } else {
                            "changes-refresh-shortcut"
                        },
                        "⌘R",
                        colors().subtle,
                    ))
                    .child(div().flex_1())
                    .when_some(read, |el, elapsed| {
                        el.child(
                            tag(
                                if compact {
                                    "inspector-read-elapsed"
                                } else {
                                    "changes-read-elapsed"
                                },
                                format!("Read in {elapsed} ms"),
                                colors().subtle,
                            )
                            .pr_1()
                            .truncate(),
                        )
                    }),
            )
    }

    fn diff_header(&self, cx: &mut Context<Self>) -> Option<Div> {
        let repo = &self.state.repository;
        let key = repo.selected_diff()?;
        let change = repo.snapshot().ready()?.changes.get(key.file)?;
        let index = key.file;
        let mode = |id: &'static str, label: &'static str, mode: DiffMode| {
            segment(id, label, key.mode == mode).on_click(
                cx.listener(move |this, _, window, cx| this.select_file(index, mode, window, cx)),
            )
        };
        /// Room the path and the side switch need before the actions wrap.
        const LEAD: f32 = 260.;
        // The file and the side being shown come first, then what can be
        // done to that side.
        let lead = row()
            .flex_wrap()
            .flex_1()
            .min_w(px(LEAD))
            .gap_2()
            .child(
                column()
                    .flex_1()
                    // The path stays readable when the header is crowded.
                    .min_w(px(96.))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(change.display_path()),
                    )
                    .when_some(change.original.as_ref(), |el, original| {
                        el.child(
                            tag(
                                "diff-renamed-from",
                                format!("Renamed from {}", original.to_string_lossy()),
                                colors().subtle,
                            )
                            .truncate(),
                        )
                    }),
            )
            .when(self.workspace.turn_review.target.is_some(), |el| {
                el.child(self.recorded_switch(cx))
            })
            .child(
                segmented()
                    .flex_shrink_0()
                    .child(mode("unstaged", "Unstaged", DiffMode::Unstaged))
                    .child(mode("staged", "Staged", DiffMode::Staged)),
            );
        let actions = row()
            .flex_shrink_0()
            .ml_auto()
            .gap_2()
            // A deleted file has nothing left to open.
            .when(change.worktree != b'D', |el| {
                let path = change.path.clone();
                el.child(
                    button("diff-open-file", "Open")
                        .aria_label(format!("Open {} in Files", change.display_path()))
                        .on_activate(cx, move |this, window, cx| {
                            this.open_file_in_view(path.clone(), window, cx)
                        }),
                )
            })
            .when(super::ways_out::on_disk(change), |el| {
                el.child(super::ways_out::editor_button(
                    "diff-editor",
                    &change.display_path(),
                    self.shortcut_aria("file.open_in_editor"),
                    self.host_is_local(),
                    cx,
                ))
            })
            .child({
                let stage = key.mode == DiffMode::Unstaged;
                let possible = if stage {
                    change.unstaged()
                } else {
                    change.staged()
                };
                button(
                    "diff-stage",
                    match stage_verb(change, stage) {
                        "Stage" => "Stage file",
                        "Unstage" => "Unstage file",
                        verb => verb,
                    },
                )
                .disabled(!possible || repo.write().busy())
                .on_activate(cx, move |this, _, cx| this.stage_change(index, stage, cx))
            })
            // Only the unstaged side can be thrown away; staged work is kept.
            .when(
                key.mode == DiffMode::Unstaged && change.unstaged() && !change.conflicted(),
                |el| {
                    el.child(
                        button(
                            "diff-discard",
                            if change.index == b'?' {
                                "Delete…"
                            } else {
                                "Discard…"
                            },
                        )
                        .text_color(rgb(colors().red))
                        .disabled(repo.write().busy())
                        .on_activate(cx, move |this, window, cx| {
                            this.discard_file(index, window, cx)
                        }),
                    )
                },
            );
        Some(
            // One 44-point line where there is room. In a narrow window the
            // actions wrap beneath, so no control runs off the edge.
            row()
                .min_h(px(44.))
                .px_4()
                .py(px(6.))
                .gap_x_2()
                .gap_y(px(2.))
                .flex_wrap()
                .flex_shrink_0()
                .border_b_1()
                .border_color(rgb(colors().border))
                .child(lead)
                .child(actions),
        )
    }

    fn diff_view(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.state.repository.diff() {
            Load::Empty => empty(
                "Nothing to compare",
                "Select a changed file to see its staged and unstaged changes separately.",
            )
            .into_any_element(),
            Load::Loading => empty(
                "Reading diff…",
                "The file preview is loading in the background.",
            )
            .into_any_element(),
            Load::Error(error) => empty(
                "Cannot read diff",
                format!("{error}\nRefresh to read the latest repository state."),
            )
            .into_any_element(),
            Load::Ready(diff) => {
                let picked = self.current_pick(diff.lines.len());
                let bar = picked.as_ref().map(|pick| self.pick_bar(pick, cx));
                self.diff_lines(
                    "diff-lines",
                    diff.clone(),
                    &self.workspace.diff_scroll,
                    Some("Click line numbers to ask · ⌥↑ ⌥↓ switch file"),
                    DiffControls {
                        hunks: self.hunk_target(),
                        pick: Some(picked.map(|pick| pick.range())),
                        bar,
                    },
                    cx,
                )
            }
        }
    }

    /// The pick, if it belongs to the file and side on screen.
    pub(super) fn current_pick(&self, lines: usize) -> Option<LinePick> {
        let pick = self.workspace.line_pick.as_ref()?;
        let repo = &self.state.repository;
        let key = repo.selected_diff()?;
        let change = repo.snapshot().ready()?.changes.get(key.file)?;
        (pick.checkout == self.state.checkout_id()
            && pick.epoch == repo.epoch()
            && change.path == pick.path
            && key.mode == pick.mode
            && pick.range().end() < &lines)
            .then(|| pick.clone())
    }

    /// Click picks a line, Shift-click extends from it, and clicking a lone
    /// picked line clears it.
    pub(super) fn pick_line(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        let repo = &self.state.repository;
        let Some(key) = repo.selected_diff() else {
            return;
        };
        let Some(change) = repo
            .snapshot()
            .ready()
            .and_then(|s| s.changes.get(key.file))
        else {
            return;
        };
        let lines = match &repo.diff() {
            Load::Ready(diff) => diff.lines.len(),
            _ => return,
        };
        let current = self.current_pick(lines);
        self.workspace.line_pick = match current {
            Some(pick) if extend => Some(LinePick { end: index, ..pick }),
            Some(pick) if pick.anchor == index && pick.end == index => None,
            _ => Some(LinePick {
                checkout: self.state.checkout_id(),
                epoch: repo.epoch(),
                path: change.path.clone(),
                mode: key.mode,
                anchor: index,
                end: index,
            }),
        };
        cx.notify();
    }

    /// Ask the selected thread about the picked lines: quote them into its
    /// composer and go there. Nothing is sent until the user sends it.
    pub(super) fn ask_about_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(diff) = &self.state.repository.diff() else {
            return;
        };
        let Some(pick) = self.current_pick(diff.lines.len()) else {
            return;
        };
        let Some(composer) = self
            .host
            .thread
            .selected
            .as_ref()
            .and_then(|id| self.workspace.composers.get(id))
            .cloned()
        else {
            return;
        };
        let side = match pick.mode {
            DiffMode::Unstaged => "unstaged",
            DiffMode::Staged => "staged",
        };
        let path = pick.path.to_string_lossy();
        let Some(quote) = plait_client::review::quote(&path, side, &diff.lines[pick.range()])
        else {
            return;
        };
        if !composer.update(cx, |composer, cx| composer.append_block(&quote, window, cx)) {
            self.notice = Some("This thread cannot take a message right now.".into());
            return cx.notify();
        }
        self.workspace.line_pick = None;
        self.show_tab(super::Workspace::Conversation, window, cx);
        cx.notify();
    }

    fn pick_bar(&self, pick: &LinePick, cx: &mut Context<Self>) -> AnyElement {
        let range = pick.range();
        let count = range.end() - range.start() + 1;
        let thread = self.host.thread.selected.is_some();
        row()
            .h(px(36.))
            .px_4()
            .gap_2()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(colors().border))
            .bg(rgb(colors().rail))
            .text_size(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(colors().muted))
                    .child(if thread {
                        format!(
                            "{count} {} picked · Shift-click a line number to extend",
                            if count == 1 { "line" } else { "lines" }
                        )
                    } else {
                        "Select a thread to ask about these lines.".into()
                    }),
            )
            .child(
                toned_button("pick-ask", "Ask about this", Tone::Filled)
                    .disabled(!thread)
                    .on_activate(cx, |this, window, cx| this.ask_about_pick(window, cx)),
            )
            .child(
                button("pick-clear", "Clear").on_activate(cx, |this, _, cx| {
                    this.workspace.line_pick = None;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The selected file, if its hunks can be staged one at a time. Renames,
    /// new files, and conflicts stage whole.
    fn hunk_target(&self) -> Option<HunkTarget> {
        let repo = &self.state.repository;
        let key = repo.selected_diff()?;
        let change = repo.snapshot().ready()?.changes.get(key.file)?;
        let whole = change.index == b'?' || change.original.is_some() || change.conflicted();
        (!whole).then(|| HunkTarget {
            checkout: self.state.checkout_id(),
            epoch: self.state.repository.epoch(),
            path: change.path.clone(),
            mode: key.mode,
            busy: repo.write().busy(),
        })
    }

    /// A patch as numbered, colored lines, virtualized. Shared by Changes and History.
    pub(super) fn diff_lines(
        &self,
        id: &'static str,
        diff: std::sync::Arc<plait_client::model::Diff>,
        scroll: &UniformListScrollHandle,
        hint: Option<&'static str>,
        controls: DiffControls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let DiffControls { hunks, pick, bar } = controls;
        {
            {
                // The footer counts the real patch; the list draws its rows.
                let count = diff.lines.len();
                let rows = diff.rows.len();
                let widest = Some(diff.widest_row);
                let elapsed = diff.elapsed;
                let truncated = diff.truncated;
                let (limit_label, limit_detail) = diff_limit_notice(id == "recorded-patch");
                let size = code_size();
                let line_height = diff_line_height();
                let gutter_width = (size * 3.7).round();
                let gutter = move || {
                    div()
                        .w(px(gutter_width))
                        .flex_shrink_0()
                        .text_right()
                        .text_color(rgb(colors().subtle))
                };
                column()
                    .flex_1()
                    .min_h_0()
                    .when(truncated, |el| {
                        el.child(
                            div()
                                .id("diff-limit")
                                .px_4()
                                .py_2()
                                .text_size(px(12.))
                                .bg(rgb(colors().warn_fill))
                                .text_color(rgb(colors().amber))
                                .child(limit_label)
                                .aria_description(limit_detail)
                                .tooltip(move |_, cx| {
                                    cx.new(|_| TextTooltip(limit_detail.into())).into()
                                }),
                        )
                    })
                    .child(
                        uniform_list(
                            id,
                            rows,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                let find = &this.workspace.diff_find;
                                let hit = (find.open && id != "commit-lines" && this.draws_work())
                                    .then(|| find.matches.get(find.current))
                                    .flatten();
                                range
                                    .map(|drawn| {
                                        // Clicks, hunks, and ids use the patch line.
                                        let index = match &diff.rows[drawn] {
                                            PatchRow::Line(line) => *line,
                                            PatchRow::File {
                                                line,
                                                name,
                                                directory,
                                            } => {
                                                return file_heading(*line, name, directory);
                                            }
                                        };
                                        let line = &diff.lines[index];
                                        let c = colors();
                                        // Line tints are the accent faintly over the background.
                                        let tint = |accent| mix(c.bg, accent, 0.09);
                                        let (fg, bg) = match line.kind {
                                            DiffKind::Added => (c.mint, tint(c.mint)),
                                            DiffKind::Removed => (c.red, tint(c.red)),
                                            // Neutral: blue is kept for links and focus.
                                            DiffKind::Hunk => (c.muted, c.rail),
                                            DiffKind::Header => (c.muted, c.rail),
                                            DiffKind::Context => (c.text, c.bg),
                                        };
                                        let pickable = pick.is_some()
                                            && matches!(
                                                line.kind,
                                                DiffKind::Added
                                                    | DiffKind::Removed
                                                    | DiffKind::Context
                                            );
                                        let picked = pick
                                            .as_ref()
                                            .and_then(|pick| pick.as_ref())
                                            .is_some_and(|range| range.contains(&index));
                                        let numbers = row()
                                            .id(("diff-pick", index))
                                            .gap_0()
                                            .flex_shrink_0()
                                            .child(
                                                gutter().pr_2().child(
                                                    line.old_line
                                                        .map_or(String::new(), |n| n.to_string()),
                                                ),
                                            )
                                            .child(
                                                gutter().pr_3().child(
                                                    line.new_line
                                                        .map_or(String::new(), |n| n.to_string()),
                                                ),
                                            )
                                            .when(picked, |el| el.text_color(rgb(colors().text)))
                                            .when(pickable, |el| {
                                                el.cursor_pointer()
                                                    .hover(|el| el.text_color(rgb(colors().text)))
                                                    .on_click(cx.listener(
                                                        move |this, event: &ClickEvent, _, cx| {
                                                            let extend = event.modifiers().shift;
                                                            this.pick_line(index, extend, cx)
                                                        },
                                                    ))
                                            });
                                        row()
                                            .id(("diff-line", index))
                                            .role(Role::Group)
                                            .aria_label(SharedString::from(line.text.clone()))
                                            .audited()
                                            .gap_0()
                                            .w_full()
                                            .h(px(line_height))
                                            .bg(rgb(if picked {
                                                mix(c.bg, c.blue, 0.22)
                                            } else {
                                                bg
                                            }))
                                            .font_family(mono())
                                            .text_size(px(size))
                                            .line_height(px(line_height))
                                            .when(
                                                hit.is_some_and(|r| r.contains(&drawn))
                                                    && this.draws_work(),
                                                |el| {
                                                    el.border_l_2()
                                                        .border_color(rgb(c.blue))
                                                        .bg(rgb(c.selected))
                                                },
                                            )
                                            .child(numbers)
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .pr_3()
                                                    .whitespace_nowrap()
                                                    .text_color(rgb(fg))
                                                    .child(line_text(line)),
                                            )
                                            .when_some(
                                                hunks.as_ref().and_then(|target| {
                                                    let hunk =
                                                        diff.hunks.binary_search(&index).ok()?;
                                                    Some((target, hunk))
                                                }),
                                                |el, (target, hunk)| {
                                                    el.group("diff-hunk")
                                                        .child(hunk_button(
                                                            target, hunk, &line.text, cx,
                                                        ))
                                                        .when(
                                                            target.mode == DiffMode::Unstaged,
                                                            |el| {
                                                                el.child(discard_hunk_button(
                                                                    target, hunk, &line.text, cx,
                                                                ))
                                                            },
                                                        )
                                                },
                                            )
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .with_width_from_item(widest)
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .track_scroll(scroll)
                        .flex_1()
                        .min_h_0(),
                    )
                    .children(bar)
                    .child(
                        row()
                            .h(px(28.))
                            .px_4()
                            .flex_shrink_0()
                            .justify_between()
                            .text_size(px(11.))
                            .text_color(rgb(colors().subtle))
                            .child(format!(
                                "{} patch lines · {} ms",
                                count,
                                elapsed.as_millis()
                            ))
                            .children(hint),
                    )
                    .into_any_element()
            }
        }
    }

    pub(super) fn count_work_files(&mut self, cx: &mut Context<Self>) {
        self.workspace.work_counts.clear();
        if self.panel_kind() != Some(plait_client::panel::PanelKind::Changes)
            && !self
                .tabs
                .model
                .work()
                .is_some_and(|w| w.open && !w.recorded)
        {
            return;
        }
        let Some(snapshot) = self
            .state
            .repository
            .snapshot()
            .ready()
            .cloned()
            .filter(|s| !s.changes.is_empty())
        else {
            return;
        };
        let source = snapshot.clone();
        let backend = self.repository_backend();
        let tab = self.tabs.model.active();
        let task = cx.background_executor().spawn(async move {
            let mut labels = std::collections::HashMap::new();
            // The host's bounded line-count read accepts 500 paths at a time.
            for changes in source.changes.chunks(500) {
                let paths = changes.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
                for count in backend.line_counts(&source.root, &paths)? {
                    if let (Some(added), Some(removed)) = (count.added, count.removed) {
                        labels.insert(count.path, format!("+{added} −{removed}"));
                    }
                }
            }
            Ok::<_, String>(labels)
        });
        cx.spawn(async move |this, cx| {
            let counts = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.tabs.model.active() == tab
                    && this
                        .state
                        .repository
                        .snapshot()
                        .ready()
                        .is_some_and(|s| std::sync::Arc::ptr_eq(s, &snapshot))
                    && let Ok(counts) = counts
                {
                    this.workspace.work_counts = counts;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn work_file_count(&self, recorded: bool) -> usize {
        if recorded {
            self.workspace
                .turn_review
                .record
                .as_ref()
                .map_or(0, |r| r.files.len())
        } else {
            self.state
                .repository
                .snapshot()
                .ready()
                .map_or(0, |s| s.changes.len())
        }
    }
    pub(super) fn work_select_file(
        &mut self,
        index: usize,
        recorded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.draws_work() {
            return;
        }
        if recorded {
            self.select_recorded_file(index, window, cx);
        } else if let Some(change) = self
            .state
            .repository
            .snapshot()
            .ready()
            .and_then(|s| s.changes.get(index))
        {
            self.select_file(
                index,
                if change.unstaged() {
                    DiffMode::Unstaged
                } else {
                    DiffMode::Staged
                },
                window,
                cx,
            );
        }
        self.route_work_tab(super::Workspace::Changes, window, cx);
    }
    pub(super) fn work_changes(&self, recorded: bool, width: f32, cx: &mut Context<Self>) -> Div {
        let count = self.work_file_count(recorded);
        let selected = if recorded {
            self.workspace.turn_review.selected
        } else {
            self.state.repository.selected_diff().map(|d| d.file)
        };
        // Page the chips around the selection; a large checkout only draws visible files.
        let page = 2 * (width / 120.).floor().max(1.) as usize;
        let start = selected.unwrap_or(0) / page * page;
        let end = (start + page).min(count);
        let mut chips = row()
            .id("panel-file-chips")
            .flex_wrap()
            .max_h(px(64.))
            .min_w_0()
            .overflow_y_scroll();
        if start > 0 {
            chips = chips.child(
                button("work-prev-files", "‹")
                    .aria_label("Previous files")
                    .on_activate(cx, move |this, w, cx| {
                        this.work_select_file(start - 1, recorded, w, cx)
                    }),
            );
        }
        for index in start..end {
            let (name, path) = if recorded {
                let file = &self.workspace.turn_review.record.as_ref().unwrap().files[index];
                (
                    format!("{} {}", file.name, file.counts),
                    file.detail.clone(),
                )
            } else {
                let file = &self.state.repository.snapshot().ready().unwrap().changes[index];
                let path = file.display_path();
                {
                    let name = path.rsplit('/').next().unwrap_or(&path);
                    let counts = self
                        .workspace
                        .work_counts
                        .get(&file.path)
                        .map_or("", String::as_str);
                    (format!("{name} {counts}"), format!("{path} {counts}"))
                }
            };
            chips = chips.child(
                nav_row(("work-file", index), path, selected == Some(index))
                    .w_auto()
                    .max_w(px(160.))
                    .aria_selected(selected == Some(index))
                    .min_w_0()
                    .child(div().truncate().child(name))
                    .on_activate(cx, move |this, w, cx| {
                        this.work_select_file(index, recorded, w, cx)
                    }),
            );
        }
        if end < count {
            chips = chips.child(
                button("work-next-files", "›")
                    .aria_label("Next files")
                    .on_activate(cx, move |this, w, cx| {
                        this.work_select_file(end, recorded, w, cx)
                    }),
            );
        }
        let mut switch = segmented();
        for (id, label, value) in [
            ("work-recorded", "This turn", true),
            ("work-uncommitted", "Uncommitted", false),
        ] {
            switch = switch.child(
                segment(id, label, value == recorded)
                    .on_activate(cx, move |this, w, cx| this.choose_work_scope(value, w, cx)),
            );
        }
        let body = column()
            .flex_1()
            .min_h_0()
            .child(
                row()
                    .min_h(px(40.))
                    .flex_wrap()
                    .gap_1()
                    .child(switch)
                    .child(tag("panel-count", format!("{count} files"), colors().muted))
                    .when_some(
                        self.workspace
                            .turn_review
                            .record
                            .as_ref()
                            .filter(|_| recorded),
                        |el, r| el.child(tag("panel-totals", r.counts.clone(), colors().subtle)),
                    )
                    .when(recorded, |el| el.children(self.panel_turn_heading(cx)))
                    .when(
                        !recorded && self.state.repository.selected_diff().is_some(),
                        |el| {
                            el.child(super::ways_out::editor_button(
                                "panel-editor",
                                "selected file",
                                self.shortcut_aria("file.open_in_editor"),
                                self.host_is_local(),
                                cx,
                            ))
                        },
                    ),
            )
            .child(chips)
            .when(self.workspace.diff_find.open, |el| {
                el.child(self.find_bar(super::file_view::FindIn::Changes, cx))
            });
        if recorded && self.workspace.turn_review.target.is_none() {
            body.child(empty(
                "No recorded turn",
                "This thread has no recorded diff to show.",
            ))
        } else if recorded {
            body.child(self.recorded_diff(cx))
        } else if let Load::Error(error) = self.state.repository.snapshot() {
            body.child(empty("Cannot read repository", error.clone()))
        } else if self.plain_folder_open() {
            body.child(empty("Not a Git repository", super::PLAIN_FOLDER))
        } else {
            body.children(self.diff_header(cx))
                .child(self.diff_view(cx))
                .when(self.state.repository.snapshot().ready().is_some(), |el| {
                    el.child(self.commit_box(cx))
                })
        }
    }

    pub(super) fn changes(&self, wide: bool, cx: &mut Context<Self>) -> impl IntoElement {
        row()
            .gap_0()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(
                column()
                    .w(px(if wide { 264. } else { 224. }))
                    .flex_shrink_0()
                    .pt_2()
                    .border_r_1()
                    .border_color(rgb(colors().border))
                    .bg(rgb(colors().rail))
                    .child(self.file_panel(false, cx)),
            )
            .child(column().flex_1().min_w_0().min_h_0().child(
                if self.workspace.turn_review.target.is_some() && !self.recorded_current_available()
                {
                    column()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(self.recorded_header(cx))
                        .child(
                            if self.workspace.turn_review.current && self.recorded_file().is_some()
                            {
                                let (label, detail) = self.recorded_current_message();
                                empty(label, detail.to_owned()).into_any_element()
                            } else {
                                self.recorded_diff(cx)
                            },
                        )
                        .into_any_element()
                } else {
                    column()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .children(self.diff_header(cx))
                        .child(self.diff_view(cx))
                        .into_any_element()
                },
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{change_mark, split_path, stage_verb};
    use crate::ui::theme::colors;
    use plait_client::model::Change;
    use std::path::PathBuf;

    fn change(path: &str) -> Change {
        Change {
            path: PathBuf::from(path),
            original: None,
            index: b' ',
            worktree: b'M',
        }
    }

    #[test]
    fn file_name_leads_and_directory_follows() {
        assert_eq!(
            split_path(&change("crates/plait/src/main.rs")),
            ("main.rs".into(), "crates/plait/src".into())
        );
        assert_eq!(
            split_path(&change("README.md")),
            ("README.md".into(), String::new())
        );
        // Control characters stay escaped, as in the accessible name.
        assert_eq!(
            split_path(&change("notes/a\nb.md")),
            ("a\\nb.md".into(), "notes".into())
        );
    }

    #[test]
    fn every_change_label_has_a_distinct_mark() {
        let marks = ["New", "Added", "Modified", "Deleted", "Renamed", "Conflict"]
            .map(|label| change_mark(label).0);
        for (index, mark) in marks.iter().enumerate() {
            assert!(!marks[..index].contains(mark), "{mark} repeats");
        }
        assert_eq!(change_mark("New").1, colors().mint);
        assert_eq!(change_mark("Conflict").1, colors().red);
        assert_eq!(change_mark("Modified").1, colors().blue);
    }

    #[test]
    fn staging_an_unmerged_file_is_called_marking_it_resolved() {
        let mut file = change("a.rs");
        file.index = b'U';
        file.worktree = b'U';
        assert_eq!(stage_verb(&file, true), "Mark resolved");
        file.index = b' ';
        file.worktree = b'M';
        assert_eq!(stage_verb(&file, true), "Stage");
        assert_eq!(stage_verb(&file, false), "Unstage");
    }
}
