//! Recorded Changes is bound to a record ID, independently of current Git state.
use super::{
    Plait, Workspace,
    changes::DiffControls,
    commands::Command,
    palette::{Item, Options, Palette},
    theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_client::{
    history::records,
    host::{self, review},
    model::{Diff, DiffMode},
    review::recorded::{Record, Stamp, Target},
};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct TurnReview {
    pub target: Option<Target>,
    pub window: Option<AnyWindowHandle>,
    pub record: Option<Arc<Record>>,
    pub diff: Option<Arc<Diff>>,
    pub status: Option<review::Status>,
    pub selected: Option<usize>,
    pub current: bool,
    pub error: Option<(&'static str, String)>,
    pub info: Option<review::Info>,
    pub older: bool,
    pub epoch: Option<u64>,
    pub generation: u64,
    pub running: bool,
    pub pending: bool,
    pub loading: bool,
}
struct Loaded {
    info: review::Info,
    record: Option<Record>,
    selected: Option<usize>,
    diff: Option<Diff>,
    status: Option<review::Status>,
    error: Option<(&'static str, String)>,
}
impl Plait {
    pub(super) fn review_stamp(&self) -> Option<Stamp> {
        Some(Stamp {
            thread: self.host.thread.selected.clone()?,
            checkout: self.host.creation.checkout.clone(),
            connection: self.host.connection.epoch,
            watch: self.host.thread.watch_epoch,
            repository: self.state.repository.epoch(),
            generation: self.workspace.turn_review.generation,
            target: self.workspace.turn_review.target.clone(),
            current: self.workspace.turn_review.current,
        })
    }
    pub(super) fn clear_turn_review(&mut self) {
        self.workspace.clear_turn_review();
    }
    pub(super) fn refresh_turn_review(&mut self, cx: &mut Context<Self>) {
        if !self.tab_identity_current()
            || !matches!(
                self.workspace.tab,
                Workspace::Conversation | Workspace::Changes
            )
        {
            return;
        }
        if cfg!(test) && self.workspace.turn_review.target.is_none() {
            return;
        }
        if self.workspace.turn_review.epoch != Some(self.host.connection.epoch) {
            self.workspace.turn_review.epoch = Some(self.host.connection.epoch);
            self.workspace.turn_review.older = false;
            self.workspace.turn_review.info = None;
        }
        if self.workspace.turn_review.older || self.host.thread.selected.is_none() {
            return;
        }
        self.workspace.turn_review.generation += 1;
        self.workspace.turn_review.pending = true;
        self.pump_turn_review(cx);
    }
    fn pump_turn_review(&mut self, cx: &mut Context<Self>) {
        if self.workspace.turn_review.running
            || !self.workspace.turn_review.pending
            || !matches!(
                self.workspace.tab,
                Workspace::Conversation | Workspace::Changes
            )
        {
            return;
        }
        let Some(stamp) = self.review_stamp() else {
            return;
        };
        if !self.host.connection.connected {
            if self.workspace.turn_review.target.is_some() {
                self.workspace.turn_review.error =
                    Some(("Offline", "Reconnect to read turn changes.".into()));
            }
            return;
        }
        self.workspace.turn_review.running = true;
        self.workspace.turn_review.pending = false;
        self.workspace.turn_review.loading =
            self.workspace.turn_review.target.is_some() && !self.workspace.turn_review.current;
        let records = self.workspace.turn_records.data.clone();
        let complete = self.records_current();
        let snapshot = self.state.repository.snapshot().ready().cloned();
        let history = self.host.thread.snapshot.clone();
        let task_stamp = stamp.clone();
        let read_record = self.panel_kind() == Some(plait_client::panel::PanelKind::Changes)
            || self.workspace.tab == Workspace::Changes;
        let client = self.host_client();
        let protocol = self.host.protocol();
        let task = cx.background_executor().spawn(async move {
            let info = review::info(&client, &task_stamp.thread)?;
            let mut result = Loaded {
                info,
                record: None,
                selected: None,
                diff: None,
                status: None,
                error: None,
            };
            let Some(target) = task_stamp.target.as_ref().filter(|_| read_record) else {
                return Ok::<_, host::Error>(result);
            };
            if !result.info.features.record_diff {
                result.error = Some((
                    "Unavailable",
                    "Recorded diffs are unavailable on this host.".into(),
                ));
                return Ok(result);
            }
            let data = if complete {
                records
            } else {
                Arc::new(records::read(&client, &task_stamp.thread, protocol)?)
            };
            let mut record = match Record::prepare(&data, target) {
                Ok(record) => record,
                Err(message) => {
                    result.error = Some((
                        if message.contains("still recording") {
                            "Recording"
                        } else {
                            "Unavailable"
                        },
                        message.into(),
                    ));
                    return Ok(result);
                }
            };
            record.current_layers(snapshot.as_deref());
            record.describe(&data, history.as_deref());
            let selected = record.selected(target);
            if !task_stamp.current
                && let Some(index) = selected
            {
                match review::read(
                    &client,
                    &target.thread,
                    &target.record,
                    &record.files[index].path.to_string_lossy(),
                    &result.info.checkout,
                ) {
                    Ok(prepared) => {
                        result.status = Some(prepared.reply.status);
                        if prepared.reply.status == review::Status::Unavailable {
                            result.error =
                                Some(("Unavailable", "The saved content is unavailable.".into()));
                        } else {
                            result.diff = Some(prepared.diff);
                        }
                    }
                    Err(host::Error::Transport(_)) => {
                        result.error = Some(("Offline", "Reconnect to read turn changes.".into()));
                    }
                    Err(error) => result.error = Some(("Unavailable", error.to_string())),
                }
            }
            result.selected = selected;
            result.record = Some(record);
            Ok(result)
        });
        let window = self.workspace.turn_review.window;
        cx.spawn(async move |this, cx| {
            let result = task.await;
            if let Some(window) = window {
                let _ = window.update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        this.review_loaded(stamp, read_record, result, Some(window), cx)
                    })
                });
            } else {
                let _ = this.update(cx, |this, cx| {
                    this.review_loaded(stamp, read_record, result, None, cx)
                });
            }
        })
        .detach();
        cx.notify();
    }
    fn review_loaded(
        &mut self,
        stamp: Stamp,
        read_record: bool,
        result: Result<Loaded, host::Error>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.workspace.turn_review.running = false;
        if self.review_stamp().as_ref() != Some(&stamp) {
            self.workspace.turn_review.pending = self.workspace.turn_review.target.is_some()
                || self.workspace.tab == Workspace::Changes;
            self.pump_turn_review(cx);
            return;
        }
        self.workspace.turn_review.loading = false;
        match result {
            Ok(result) => {
                self.workspace.turn_review.info = Some(result.info);
                // An info-only read cannot replace the previously loaded patch.
                if read_record {
                    let offline = result.error.as_ref().is_some_and(|e| e.0 == "Offline");
                    self.workspace.turn_review.error = result.error;
                    if let Some(record) = result.record {
                        self.workspace.turn_review.record = Some(Arc::new(record));
                        self.workspace.turn_review.selected = result.selected;
                    }
                    if !self.workspace.turn_review.current {
                        self.workspace.turn_review.status = result.status;
                        if !offline {
                            self.workspace.turn_review.diff = result.diff.map(Arc::new);
                        }
                    }
                }
            }
            Err(error) if review::needs_host_update(&error) => {
                self.workspace.turn_review.older = true;
                if let Some(path) = stamp.target.as_ref().and_then(|target| target.path.clone())
                    && let Some(window) = window
                {
                    self.open_thread_file(path, window, cx);
                    return;
                }
                self.workspace.turn_review.error = Some((
                    "Older host",
                    "Turn review needs a newer host. Current Git changes are still available."
                        .into(),
                ));
                self.workspace.turn_review.diff = None;
            }
            Err(host::Error::Transport(_)) => {
                self.workspace.turn_review.error =
                    Some(("Offline", "Reconnect to read turn changes.".into()))
            }
            Err(error) => {
                self.workspace.turn_review.error = Some(("Unavailable", error.to_string()));
                self.workspace.turn_review.diff = None;
            }
        }
        self.sync_recorded_current(cx);
        self.search_panel_diff(cx);
        self.restore_ready_bookmarks(cx);
        self.pump_turn_review(cx);
        cx.notify();
    }
    pub(super) fn open_turn_review(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.thread.selected.as_deref() != Some(&target.thread)
            || self.host.creation.checkout != target.checkout
        {
            return;
        }
        if self.workspace.turn_review.older
            && let Some(path) = &target.path
        {
            self.open_thread_file(path.clone(), window, cx);
            return;
        }
        self.workspace
            .turn_records
            .selection
            .earlier(&target.original);
        if let Some(panel) = self.tabs.model.panel_mut() {
            panel.select_scope(plait_client::panel::Scope::ThisTurn);
            panel.recorded = Some(target.clone());
        }
        self.clear_turn_review();
        self.workspace.turn_review.window = Some(window.window_handle());
        self.workspace.turn_review.target = Some(target);
        self.workspace.diff_scroll = UniformListScrollHandle::new();
        self.workspace.file_scroll = UniformListScrollHandle::new();
        self.show_tab(Workspace::Changes, window, cx);
        if self.workspace.turn_review.older {
            self.workspace.turn_review.error = Some((
                "Older host",
                "Turn review needs a newer host. Current Git changes are still available.".into(),
            ));
        }
        cx.notify();
    }
    pub(super) fn select_recorded_file(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self
            .workspace
            .turn_review
            .record
            .as_ref()
            .and_then(|r| r.files.get(index))
        else {
            return;
        };
        let path = file.path.clone();
        self.workspace.restore_pending[1] = false;
        if let Some(target) = self.workspace.turn_review.target.as_mut() {
            target.path = Some(path);
            if let Some(panel) = self.tabs.model.panel_mut() {
                panel.recorded = Some(target.clone());
            }
        }
        self.workspace.turn_review.selected = Some(index);
        self.workspace.turn_review.diff = None;
        self.workspace.turn_review.error = None;
        self.workspace.diff_scroll = UniformListScrollHandle::new();
        window.focus(&self.focus, cx);
        self.refresh_turn_review(cx);
        self.sync_recorded_current(cx);
        cx.notify();
    }
    pub(super) fn review_comparison(&mut self, current: bool, cx: &mut Context<Self>) {
        if current && self.workspace.turn_review.older {
            self.clear_turn_review();
            cx.notify();
            return;
        }
        if self.recorded_file().is_none() {
            return;
        }
        self.workspace.turn_review.current = current;
        self.workspace.restore_pending[1] = false;
        self.workspace.diff_scroll = UniformListScrollHandle::new();
        self.workspace.line_pick = None;
        self.refresh_turn_review(cx);
        self.sync_recorded_current(cx);
        cx.notify();
    }
    pub(super) fn sync_recorded_current(&mut self, cx: &mut Context<Self>) {
        if !self.workspace.turn_review.current {
            return;
        }
        let Some(file) = self.recorded_file() else {
            return;
        };
        let path = file.path.clone();
        if let Some((index, mode)) = self.state.repository.snapshot().ready().and_then(|s| {
            s.changes
                .iter()
                .enumerate()
                .find(|(_, c)| c.path == path)
                .map(|(i, c)| {
                    (
                        i,
                        self.state
                            .repository
                            .selected_diff()
                            .as_ref()
                            .filter(|key| {
                                key.file == i
                                    && match key.mode {
                                        DiffMode::Staged => c.staged(),
                                        DiffMode::Unstaged => c.unstaged(),
                                    }
                            })
                            .map_or_else(
                                || {
                                    if c.unstaged() {
                                        DiffMode::Unstaged
                                    } else {
                                        DiffMode::Staged
                                    }
                                },
                                |key| key.mode,
                            ),
                    )
                })
        }) && !self
            .state
            .repository
            .selected_diff()
            .as_ref()
            .is_some_and(|s| s.file == index && s.mode == mode)
        {
            self.state.repository.select(index, mode);
            self.pump(cx);
        }
    }
    pub(super) fn recorded_file(&self) -> Option<&plait_client::review::recorded::File> {
        self.workspace
            .turn_review
            .record
            .as_ref()?
            .files
            .get(self.workspace.turn_review.selected?)
    }
    pub(super) fn recorded_current_available(&self) -> bool {
        // Selection is prepared by event handlers; no status scan in render.
        self.workspace.turn_review.current
            && self.recorded_file().is_some_and(|file| {
                self.state
                    .repository
                    .selected_diff()
                    .as_ref()
                    .and_then(|key| {
                        self.state
                            .repository
                            .snapshot()
                            .ready()?
                            .changes
                            .get(key.file)
                    })
                    .is_some_and(|c| c.path == file.path)
            })
    }
    pub(super) fn recorded_current_message(&self) -> (&'static str, &str) {
        use plait_client::state::Load;
        match &self.state.repository.snapshot() {
            Load::Empty | Load::Loading => (
                "Reading diff…",
                "The file preview is loading in the background.",
            ),
            Load::Error(error) => ("Unavailable", error),
            Load::Ready(_) => (
                "No current changes",
                "This file has no current staged or unstaged changes.",
            ),
        }
    }
    pub(super) fn review_scope_text(&self) -> (String, String) {
        if let Some(record) = &self.workspace.turn_review.record {
            (record.label.clone(), record.detail.clone())
        } else if self.workspace.turn_review.target.is_some() {
            let detail = self
                .workspace
                .turn_review
                .error
                .as_ref()
                .map(|e| e.1.clone())
                .unwrap_or_else(|| {
                    if self.workspace.turn_review.loading {
                        "Reading this run's recorded changes from the host."
                    } else {
                        "Show this run's recorded changes."
                    }
                    .into()
                });
            ("Recorded".into(), detail)
        } else {
            (
                "All changes".into(),
                "Show all current staged and unstaged files in this checkout.".into(),
            )
        }
    }
    pub(super) fn review_scope_button(&self, cx: &mut Context<Self>) -> Control {
        let (label, detail) = self.review_scope_text();
        button("review-scope", "")
            .aria_label(label.clone())
            .h(px(28.))
            .min_w_0()
            .flex_shrink_1()
            .aria_description(detail.clone())
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
            .child(div().min_w_0().truncate().child(label))
            .child(icon("down").size(px(12.)).flex_shrink_0())
            .on_activate(cx, |this, window, cx| this.open_review_scopes(window, cx))
    }
    pub(super) fn open_review_scopes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stamp) = self.review_stamp() else {
            return;
        };
        let data = self.workspace.turn_records.data.clone();
        let history = self.host.thread.snapshot.clone();
        let task = cx.background_executor().spawn(async move {
            let mut choices = Vec::new();
            let mut newest: Vec<_> = data.iter().collect();
            newest.sort_by_key(|r| std::cmp::Reverse(r.created_at));
            for r in newest {
                let original = if r.kind == host::RecordKind::Turn {
                    r.id.clone()
                } else {
                    let mut at = r;
                    let mut seen = std::collections::HashSet::new();
                    while let Some(parent) = at
                        .undoes
                        .as_ref()
                        .and_then(|id| data.iter().find(|r| &r.id == id))
                    {
                        if !seen.insert(parent.id.clone()) {
                            break;
                        }
                        at = parent;
                    }
                    at.id.clone()
                };
                let next = Target {
                    thread: stamp.thread.clone(),
                    checkout: stamp.checkout.clone(),
                    record: r.id.clone(),
                    original,
                    path: None,
                };
                if let Ok(mut record) = Record::prepare(&data, &next) {
                    record.describe(&data, history.as_deref());
                    let detail = review_picker_detail(&record);
                    let label = if record.restored {
                        "Restored"
                    } else {
                        "Original"
                    };
                    choices.push((label, detail, next));
                }
            }
            (stamp, choices)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (stamp, choices) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.review_stamp().as_ref() != Some(&stamp) {
                    return;
                }
                let mut items = vec![
                    Item::new("all-changes", "All changes", Command::ReviewAll)
                        .detail("Current staged and unstaged files"),
                ];
                items.extend(choices.into_iter().map(|(label, detail, target)| {
                    Item::new(target.record.clone(), label, Command::ReviewRecord(target))
                        .detail(detail)
                }));
                let palette = cx.new(|cx| {
                    Palette::new(
                        Options {
                            name: "Changes scope".into(),
                            placeholder: "Find a retained run…".into(),
                            empty: "No retained runs.".into(),
                            initial: None,
                            mark: None,
                            aside: None,
                            note: None,
                        },
                        items,
                        window,
                        cx,
                    )
                });
                this.present(palette, None, window, cx);
            });
        })
        .detach();
    }
    pub(super) fn recorded_files(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self
            .workspace
            .turn_review
            .record
            .as_ref()
            .map_or(0, |r| r.files.len());
        if count == 0 {
            return div().flex_1().into_any_element();
        }
        uniform_list(
            "recorded-files",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let file =
                            &this.workspace.turn_review.record.as_ref().unwrap().files[index];
                        let detail = file.detail.clone();
                        nav_row(
                            ("recorded-file", index),
                            file.detail.clone(),
                            this.workspace.turn_review.selected == Some(index),
                        )
                        .h(px(28.))
                        .min_w_0()
                        .gap_1()
                        .child(
                            tag(
                                "recorded-file-status",
                                match file.label {
                                    "Added" => "A",
                                    "Deleted" => "D",
                                    _ => "M",
                                },
                                colors().subtle,
                            )
                            .w(px(16.)),
                        )
                        .child(div().min_w_0().truncate().child(file.name.clone()))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_color(rgb(colors().subtle))
                                .text_size(px(11.))
                                .child(file.directory.clone()),
                        )
                        .child(
                            tag("recorded-file-counts", file.counts.clone(), colors().subtle)
                                .flex_shrink_0(),
                        )
                        .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
                        .on_activate(cx, move |this, window, cx| {
                            this.select_recorded_file(index, window, cx)
                        })
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.workspace.file_scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
    pub(super) fn recorded_file_panel(&self, cx: &mut Context<Self>) -> Div {
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
                    .when_some(self.state.repository.snapshot().ready(), |el, snapshot| {
                        el.child(
                            row()
                                .gap_1()
                                .child(icon("branch").text_color(rgb(colors().muted)))
                                .child(self.branch_control(false, &snapshot.branch, cx)),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors().subtle))
                                .truncate()
                                .child(home_relative(&snapshot.root)),
                        )
                    }),
            )
            .child(self.review_scope_button(cx))
            .child(self.recorded_files(cx))
            .when(self.state.repository.snapshot().ready().is_some(), |el| {
                el.child(self.commit_box(cx))
            })
            .child(
                row()
                    .h(px(36.))
                    .px_2()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(rgb(colors().border))
                    .child(
                        button("refresh-record", "Refresh")
                            .aria_keyshortcuts("Meta+R")
                            .on_activate(cx, |this, window, cx| {
                                this.refresh(&super::Refresh, window, cx)
                            }),
                    )
                    .child(key_hint("recorded-refresh-shortcut", "⌘R")),
            )
    }
    pub(super) fn recorded_header(&self, cx: &mut Context<Self>) -> Div {
        let path = self
            .recorded_file()
            .map_or("Recorded changes".into(), |f| f.display_path.clone());
        row()
            .min_h(px(44.))
            .px_4()
            .py(px(6.))
            .flex_wrap()
            .gap_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(colors().border))
            .child(
                div()
                    .min_w(px(96.))
                    .flex_1()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(path),
            )
            .when(self.recorded_file().is_some(), |el| {
                el.child(self.recorded_switch(cx))
            })
    }
    pub(super) fn recorded_switch(&self, cx: &Context<Self>) -> Div {
        segmented()
            .child(
                segment(
                    "review-recorded",
                    "Recorded",
                    !self.workspace.turn_review.current,
                )
                .aria_description("Compare this record's before and after versions.")
                .tooltip(|_, cx| {
                    cx.new(|_| {
                        TextTooltip("Compare this record's before and after versions.".into())
                    })
                    .into()
                })
                .on_activate(cx, |this, _, cx| this.review_comparison(false, cx)),
            )
            .child(
                segment(
                    "review-current",
                    "Current",
                    self.workspace.turn_review.current,
                )
                .aria_description("Review this file's current staged and unstaged changes.")
                .tooltip(|_, cx| {
                    cx.new(|_| {
                        TextTooltip(
                            "Review this file's current staged and unstaged changes.".into(),
                        )
                    })
                    .into()
                })
                .on_activate(cx, |this, _, cx| this.review_comparison(true, cx)),
            )
    }
    pub(super) fn recorded_notice(&self) -> Option<(&'static str, &'static str)> {
        (self.workspace.turn_review.status == Some(review::Status::Mode))
            .then_some(("Mode", "The recorded executable bit or file type changed."))
    }
    pub(super) fn recorded_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.workspace.turn_review;
        if state.loading && state.diff.is_none() {
            return empty("Loading", "Reading this file's diff from the host.").into_any_element();
        }
        if let Some((label, detail)) = &state.error {
            if *label == "Offline"
                && let Some(diff) = &state.diff
            {
                return column()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("recorded-offline")
                            .px_4()
                            .py_1()
                            .child(tag("recorded-offline-label", "Offline", colors().amber))
                            .aria_description("Reconnect to read turn changes.")
                            .tooltip(|_, cx| {
                                cx.new(|_| TextTooltip("Reconnect to read turn changes.".into()))
                                    .into()
                            }),
                    )
                    .child(self.diff_lines(
                        "recorded-patch",
                        diff.clone(),
                        &self.workspace.diff_scroll,
                        None,
                        DiffControls::default(),
                        cx,
                    ))
                    .into_any_element();
            }
            return column()
                .flex_1()
                .items_center()
                .child(empty(label, detail.clone()))
                .child(
                    button(
                        "review-retry",
                        if state.older { "Current" } else { "Retry" },
                    )
                    .mb_4()
                    .on_activate(cx, |this, _, cx| {
                        if this.workspace.turn_review.older {
                            this.clear_turn_review();
                        } else {
                            this.refresh_turn_review(cx);
                        }
                        cx.notify();
                    }),
                )
                .into_any_element();
        }
        if state.status == Some(review::Status::Binary) {
            return empty(
                "Binary",
                "Text comparison is unavailable. The recorded content or mode changed.",
            )
            .into_any_element();
        }
        if let Some(diff) = &state.diff {
            let patch = self.diff_lines(
                "recorded-patch",
                diff.clone(),
                &self.workspace.diff_scroll,
                None,
                DiffControls::default(),
                cx,
            );
            if let Some((label, detail)) = self.recorded_notice() {
                return column()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("recorded-limit")
                            .px_4()
                            .py_1()
                            .text_color(rgb(colors().subtle))
                            .child(label)
                            .aria_description(detail)
                            .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.into())).into()),
                    )
                    .child(patch)
                    .into_any_element();
            }
            return patch;
        }
        if state.record.as_ref().is_some_and(|r| r.files.is_empty()) {
            empty(
                "No recorded changes",
                "This run left no covered file changes.",
            )
            .into_any_element()
        } else {
            empty(
                "Select a file",
                "Choose a file in this turn to review its recorded changes.",
            )
            .into_any_element()
        }
    }
}

pub(super) fn review_picker_detail(record: &Record) -> String {
    // Scope rows have one fixed-height line; keep coverage searchable on it.
    record.detail.lines().collect::<Vec<_>>().join(" · ")
}
