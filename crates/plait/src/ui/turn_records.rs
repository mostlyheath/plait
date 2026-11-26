//! Host-owned run records, their guarded reads, and undo at the closing line.
use super::{Plait, Workspace, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{
    git::record::Uncovered,
    history::records::{self, Availability, Line, Lines},
    host::{self, Request, TurnRecord, UndoState, UndoTurnReply},
    host_state::request_id,
    preferences::Turns,
    review::recorded::Target,
    transcript,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    pub(super) thread: String,
    pub(super) epoch: u64,
    pub(super) watch: u64,
    pub(super) revision: Option<u64>,
    pub(super) generation: u64,
}
pub(super) use plait_client::operations::UndoAttempt as Attempt;
pub(super) struct Blocked {
    run: String,
    pub(super) reply: UndoTurnReply,
    coverage: Vec<SharedString>,
    changed: Vec<SharedString>,
    omitted: usize,
    changed_omitted: usize,
    host_omitted: usize,
}
impl Blocked {
    pub(super) fn new(run: String, reply: UndoTurnReply, coverage: &[Uncovered]) -> Self {
        let host_omitted = reply
            .not_covered_total
            .saturating_sub(reply.not_covered.len());
        let coverage: Vec<_> = coverage
            .iter()
            .chain(&reply.not_covered)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let omitted = coverage.len().saturating_sub(records::PREVIEW_LIMIT);
        let coverage = coverage
            .iter()
            .take(records::PREVIEW_LIMIT)
            .map(|item| {
                format!(
                    "{} — {}",
                    records::preview_name(&item.path),
                    records::coverage_reason(item.reason)
                )
                .into()
            })
            .collect();
        let changed: Vec<SharedString> = reply
            .changed_since
            .iter()
            .take(records::PREVIEW_LIMIT)
            .map(|path| records::preview_name(&path.to_string_lossy()).into())
            .collect();
        let changed_omitted = reply.changed_since_total.saturating_sub(changed.len());
        Self {
            run,
            reply,
            coverage,
            changed,
            omitted,
            changed_omitted,
            host_omitted,
        }
    }
}
#[derive(Default)]
pub(super) struct TurnRecords {
    pub(super) measured_work: std::cell::RefCell<super::live_conversation::MeasuredWork>,
    pub(super) open_files: HashSet<String>,
    pub(super) totals: HashMap<String, String>,
    pub(super) selection: plait_client::history::this_turn::Selection,
    pub(super) stamp: Option<Stamp>,
    pub data: Arc<Vec<TurnRecord>>,
    pub lines: Arc<Lines>,
    pub(super) generation: u64,
    pub(super) data_generation: u64,
    pub(super) projected_generation: u64,
    pub(super) loaded: bool,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) pending: Option<Attempt>,
    pub(super) retry: Option<Attempt>,
    pub(super) blocked: Option<Blocked>,
    pub(super) note: Option<(String, &'static str, String)>,
}
impl TurnRecords {
    pub(super) fn file_totals(records: &[TurnRecord]) -> HashMap<String, String> {
        records
            .iter()
            .filter(|record| record.paths_total > 0 && record.paths_total == record.paths.len())
            .filter_map(|record| {
                let (added, removed) =
                    record.paths.iter().try_fold((0u64, 0u64), |(a, r), path| {
                        Some((a.checked_add(path.added?)?, r.checked_add(path.removed?)?))
                    })?;
                Some((record.id.clone(), format!("+{added} −{removed}")))
            })
            .collect()
    }
}

impl Plait {
    pub(super) fn closing_file_detail<'a>(
        &self,
        file: &'a records::File,
        line: &'a Line,
    ) -> &'a str {
        if self.workspace.turn_review.older {
            &file.current_detail
        } else if line.availability == Availability::Ready {
            &file.review_detail
        } else {
            &line.review_detail
        }
    }

    fn record_stamp(&self) -> Option<Stamp> {
        let snapshot = self.host.thread.snapshot.as_ref()?;
        (self.host.thread.selected.as_deref() == Some(snapshot.id.as_str())).then(|| Stamp {
            thread: snapshot.id.clone(),
            epoch: self.host.connection.epoch,
            watch: self.host.thread.watch_epoch,
            revision: snapshot.records_revision,
            generation: self.workspace.turn_records.generation,
        })
    }
    pub(super) fn records_current(&self) -> bool {
        self.host.protocol() >= 5
            && self.workspace.turn_records.loaded
            && self.workspace.turn_records.projected_generation
                == self.workspace.turn_records.generation
            && self.workspace.turn_records.stamp == self.record_stamp()
    }
    pub(super) fn turn_records_snapshot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stamp) = self.record_stamp() else {
            return;
        };
        if self.workspace.turn_records.stamp.as_ref() == Some(&stamp) {
            return;
        }
        let same_thread = self
            .workspace
            .turn_records
            .stamp
            .as_ref()
            .is_some_and(|old| {
                old.thread == stamp.thread && old.epoch == stamp.epoch && old.watch == stamp.watch
            });
        if !same_thread {
            let generation = self.workspace.turn_records.generation + 1;
            self.workspace.turn_records = TurnRecords {
                generation,
                ..Default::default()
            };
        }
        self.restore_undo_operation();
        if self.host.protocol() < 5 {
            return;
        }
        self.fetch_turn_records(window, cx);
    }
    pub(super) fn fetch_turn_records(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.host.connection.connected || self.host.protocol() < 5 {
            return;
        }
        self.workspace.turn_records.generation += 1;
        let Some(stamp) = self.record_stamp() else {
            return;
        };
        self.workspace.turn_records.stamp = Some(stamp.clone());
        self.workspace.turn_records.loading = true;
        self.workspace.turn_records.loaded = false;
        self.workspace.turn_records.error = None;
        let thread = stamp.thread.clone();
        let client = self.host_client();
        let protocol = self.host.protocol();
        let task = cx.background_executor().spawn(async move {
            let data = records::read(&client, &thread, protocol)?;
            let totals = TurnRecords::file_totals(&data);
            Ok::<_, host::Error>((Arc::new(data), totals))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.record_stamp().as_ref() != Some(&stamp) {
                    return;
                }
                this.workspace.turn_records.loading = false;
                match result {
                    Ok((records, totals)) => {
                        this.workspace.turn_records.totals = totals;
                        this.record_thread_files(&records, cx);
                        this.workspace.turn_records.data = records;
                        this.workspace.turn_records.data_generation = stamp.generation;
                        this.workspace.turn_records.loaded = true;
                        this.conversation_changed(window, cx);
                    }
                    Err(error) => this.workspace.turn_records.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Called with the background conversation projection, never from render.
    pub(super) fn install_record_lines(&mut self, lines: Arc<Lines>, generation: u64) {
        self.workspace.turn_records.projected_generation = generation;
        self.workspace.turn_records.selection.retain(&lines);
        self.workspace
            .turn_records
            .open_files
            .retain(|run| lines.runs.contains_key(run));
        let old = std::mem::replace(&mut self.workspace.turn_records.lines, lines);
        let Some(snapshot) = self.conversation_snapshot() else {
            return;
        };
        for (index, row) in self.workspace.live_layout.rows.iter().enumerate() {
            let slot = match row {
                plait_client::folding::DisplayRow::Slot(slot) => Some(*slot),
                plait_client::folding::DisplayRow::FoldedTurn { closing_slot, .. } => *closing_slot,
            };
            if let Some(key) = slot
                .and_then(|slot| transcript::resolve(snapshot, slot))
                .and_then(records::closing_key)
                && (old.rows.get(&key) != self.workspace.turn_records.lines.rows.get(&key)
                    || old.covered.contains(&key)
                        != self.workspace.turn_records.lines.covered.contains(&key))
            {
                self.workspace.live_scroll.remeasure_items(index..index + 1);
            }
        }
    }
    fn remeasure_undo_panel(&self, run: &str) {
        let Some(snapshot) = self.conversation_snapshot() else {
            return;
        };
        for (index, row) in self.workspace.live_layout.rows.iter().enumerate() {
            let slot = match row {
                plait_client::folding::DisplayRow::Slot(slot) => Some(*slot),
                plait_client::folding::DisplayRow::FoldedTurn { closing_slot, .. } => *closing_slot,
            };
            if let Some(key) = slot
                .and_then(|slot| transcript::resolve(snapshot, slot))
                .and_then(records::closing_key)
                && self
                    .workspace
                    .turn_records
                    .lines
                    .rows
                    .get(&key)
                    .is_some_and(|line| line.run == run)
            {
                self.workspace.live_scroll.remeasure_items(index..index + 1);
            }
        }
    }
    pub(super) fn undo_context(&self) -> plait_client::history::this_turn::UndoContext<'_> {
        plait_client::history::this_turn::UndoContext {
            note: self.workspace.turn_records.note.as_ref(),
            connected: self.host.connection.connected,
            current: self.records_current(),
            pending: self.workspace.turn_records.pending.is_some(),
            busy: self.state.repository.write().busy()
                || self.host.thread.controlling
                || self.host.thread.snapshot.as_ref().is_some_and(|s| {
                    s.status.busy() || self.host.connection.checkout_has_run(&s.checkout)
                }),
        }
    }
    pub(super) fn undo_refusal(
        &self,
        line: &Line,
    ) -> Option<plait_client::history::this_turn::Refusal> {
        self.undo_context().refusal(line)
    }
    pub(super) fn shortcut_undo(&self) -> Option<String> {
        if self.settings_open
            || self.workspace.tab != Workspace::Conversation
            || self.overlay.is_some()
        {
            return None;
        }
        let line = self.workspace.turn_records.lines.latest.as_ref()?;
        (self.undo_refusal(line).is_none()
            && self.workspace.turn_records.blocked.is_none()
            && self.workspace.turn_records.retry.is_none()
            && self.workspace.turn_records.note.is_none())
        .then(|| line.run.clone())
    }
    pub(super) fn undo_last_turn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(run) = self.shortcut_undo() {
            self.undo_run(&run, false, window, cx);
        }
    }
    pub(super) fn undo_run(
        &mut self,
        run: &str,
        leave_changed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(line) = self.workspace.turn_records.lines.runs.get(run) else {
            return;
        };
        if self.undo_refusal(line).is_some() {
            return;
        }
        let Some(target) = line.target.clone() else {
            return;
        };
        if leave_changed
            && !self
                .workspace
                .turn_records
                .blocked
                .as_ref()
                .is_some_and(|blocked| blocked.run == run && blocked.reply.record_id == target)
        {
            return;
        }
        let attempt = self
            .workspace
            .turn_records
            .retry
            .take()
            .filter(|old| {
                old.run == run && old.target == target && old.leave_changed == leave_changed
            })
            .unwrap_or_else(|| Attempt {
                run: run.into(),
                target,
                request: request_id(),
                leave_changed,
            });
        self.send_undo(attempt, window, cx);
    }
    fn send_undo(&mut self, attempt: Attempt, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stamp) = self.record_stamp() else {
            return;
        };
        let owner = self.host.thread_key(&stamp.thread);
        let checkout = self.state.checkout_id();
        let expected = self.host.expected_checkout(&stamp.thread).cloned();
        let coverage = self
            .workspace
            .turn_records
            .lines
            .runs
            .get(&attempt.run)
            .map(|line| line.not_covered.clone())
            .unwrap_or_default();
        if !self
            .host
            .undo
            .begin(owner.clone(), checkout, attempt.clone(), coverage)
        {
            return;
        }
        let request = Request::undo_turn(
            stamp.thread.clone(),
            attempt.target.clone(),
            attempt.request.clone(),
            attempt.leave_changed,
        );
        self.restore_undo_operation();
        self.remeasure_undo_panel(&attempt.run);
        let client = self.host_client();
        let supported = self.host.connection.has_checkout_identity;
        let task = cx.background_executor().spawn(async move {
            let expected = expected.ok_or_else(|| {
                host::Error::Protocol("Checkout identity is not verified.".into())
            })?;
            if supported {
                host::git::identity(
                    &client,
                    &expected.root,
                    Some(plait_protocol::lease::DirIdentity {
                        dev: expected.device,
                        ino: expected.inode,
                    }),
                )?;
            } else {
                return Err(host::Error::Protocol(
                    "Update the host to verify the checkout.".into(),
                ));
            }
            client.request::<UndoTurnReply>(&request)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.map_err(|error| match error {
                host::Error::Rejected(error) => plait_client::operations::UndoFailure {
                    code: Some(error.code),
                    message: error.message,
                },
                error => plait_client::operations::UndoFailure {
                    code: None,
                    message: error.to_string(),
                },
            });
            let changed = !matches!(&result, Ok(reply) if reply.state == UndoState::Blocked)
                && !matches!(&result, Err(error) if error.code.is_none());
            let _ = this.update_in(cx, |this, window, cx| {
                if this.host.identity != owner.host
                    || !this.host.undo.finish(&owner, &attempt.request, result)
                {
                    return;
                }
                if changed {
                    this.state.invalidate_checkout(checkout);
                    this.pump(cx);
                }
                if this.host.read_stamp().thread.as_ref() != Some(&owner) {
                    return;
                }
                this.restore_undo_operation();
                if changed {
                    this.fetch_turn_records(window, cx);
                }
                this.remeasure_undo_panel(&attempt.run);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn restore_undo_operation(&mut self) {
        let Some(owner) = self.host.read_stamp().thread else {
            return;
        };
        let Some(operation) = self.host.undo.get(&owner).cloned() else {
            return;
        };
        let state = &mut self.workspace.turn_records;
        state.pending = operation.sending.then(|| operation.attempt.clone());
        state.retry = None;
        state.blocked = None;
        state.note = None;
        match operation.result {
            Some(Ok(reply)) if reply.state == UndoState::Blocked => {
                state.blocked = Some(Blocked::new(
                    operation.attempt.run,
                    reply,
                    &operation.coverage,
                ));
            }
            Some(Err(error)) => {
                let (label, detail) = match error.code.as_deref() {
                    None => {
                        state.retry = Some(operation.attempt.clone());
                        ("Retry", format!("The reply was not received. Retry checks the same operation without applying it twice. {}", error.message))
                    }
                    Some("undo_incomplete") => ("Incomplete", "Undo was interrupted; some files may have changed. Inspect Changes before another undo.".into()),
                    Some("busy") => ("Wait", error.message),
                    Some("undo_no_effect") => ("Retry", error.message),
                    Some(_) => ("Unavailable", error.message),
                };
                state.note = Some((operation.attempt.run, label, detail));
            }
            _ => {}
        }
    }

    pub(super) fn cancel_undo_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(blocked) = self.workspace.turn_records.blocked.take() {
            if let Some(owner) = self.host.read_stamp().thread {
                self.host.undo.dismiss(&owner);
            }
            self.remeasure_undo_panel(&blocked.run);
        }
        cx.notify();
    }

    pub(super) fn toggle_record_files(&mut self, run: &str, cx: &mut Context<Self>) {
        if !self.workspace.turn_records.open_files.remove(run) {
            self.workspace.turn_records.open_files.insert(run.into());
        }
        self.remeasure_undo_panel(run);
        cx.notify();
    }

    pub(super) fn record_line(
        &self,
        line: &Line,
        latest: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let run = &line.run;
        let state = &self.workspace.turn_records;
        let target = self.host.thread.selected.as_ref().map(|thread| Target {
            thread: thread.clone(),
            checkout: self.host.creation.checkout.clone(),
            record: line.displayed.clone(),
            original: run.clone(),
            path: None,
        });
        let available = line.availability == Availability::Ready;
        let open = state.open_files.contains(run);
        let live = latest
            .then(|| self.host.thread.snapshot.as_ref().map(|s| s.status))
            .flatten();
        let (glyph, word, tint) = closing_state(line, live);
        let steps = line
            .steps
            .filter(|_| self.conversation_snapshot().is_some_and(|s| !s.truncated))
            .map(|n| format!("{n} {}", if n == 1 { "step" } else { "steps" }));
        let name = steps
            .as_ref()
            .map_or_else(|| word.to_owned(), |steps| format!("{word} · {steps}"));
        let tip = line.detail.clone();
        let content = row()
            .id(SharedString::from(format!("run-record-{run}")))
            .role(Role::Group)
            .aria_label(name)
            .audited()
            .w_full()
            .min_w_0()
            .min_h(px(super::live_conversation::CLOSING_LINE))
            .flex_wrap()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(rgb(colors().subtle))
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(tip.clone())).into())
            .child(
                row()
                    .gap(px(4.))
                    .flex_shrink_0()
                    .child(div().text_color(rgb(tint)).child(glyph))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(if word == "Finished" {
                                colors().strong
                            } else {
                                tint
                            }))
                            .child(word),
                    ),
            )
            .when_some(steps, |el, steps| el.child(format!("· {steps}")))
            .when(line.paths_total > 0, |el| {
                el.child(self.record_file_count(line, open, cx))
            })
            .when(line.paths_total == 0 && available, |el| {
                el.child("no files changed")
            })
            .when(line.after_restart, |el| {
                el.child(div().text_color(rgb(colors().amber)).child("After restart"))
            })
            .when(line.head_moved, |el| el.child("HEAD moved"))
            .child(
                row()
                    .ml_auto()
                    .gap(px(4.))
                    .child(self.record_undo_button(
                        line,
                        self.undo_context().action(line),
                        None,
                        cx,
                    ))
                    .child(self.review_button(line, target.clone(), latest, cx)),
            );
        let missing = line.paths_total.saturating_sub(line.files.len());
        column()
            .w_full()
            .min_w_0()
            .when(self.preferences.value.turns == Turns::Bubbles, |el| {
                el.border_t_1().border_color(rgb(colors().border))
            })
            .child(content)
            .when(open, |el| {
                el.when_some(target.filter(|_| !line.files.is_empty()), |el, target| {
                    el.child(self.record_file_list(target, line.files.len(), cx))
                })
                .when(missing > 0, |el| {
                    el.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors().subtle))
                            .child(format!("{missing} more")),
                    )
                })
            })
            .children(self.undo_panel(line, cx))
            .into_any_element()
    }

    fn record_file_count(&self, line: &Line, open: bool, cx: &mut Context<Self>) -> Control {
        let run = line.run.clone();
        let count = line.paths_total;
        let label = format!("{count} {}", if count == 1 { "file" } else { "files" });
        let records = &self.workspace.turn_records;
        let totals = records
            .totals
            .get(&line.displayed)
            .filter(|_| records.projected_generation == records.data_generation);
        let name = totals.map_or_else(|| label.clone(), |totals| format!("{label} {totals}"));
        button(SharedString::from(format!("record-files-{run}")), "")
            .h(px(24.))
            .px_1()
            .gap(px(4.))
            .aria_expanded(open)
            .aria_label(name)
            .child(icon(if open { "down" } else { "right" }))
            .child(label)
            .when_some(totals, |el, counts| {
                el.child(
                    div()
                        .font_family(mono())
                        .text_color(rgb(colors().subtle))
                        .child(counts.clone()),
                )
            })
            .on_activate(cx, move |this, _, cx| this.toggle_record_files(&run, cx))
    }

    fn record_file_list(&self, target: Target, count: usize, cx: &Context<Self>) -> UniformList {
        let lines = self.workspace.turn_records.lines.clone();
        uniform_list(
            SharedString::from(format!("closing-paths-{}", target.original)),
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(line) = lines.runs.get(&target.original) else {
                    return Vec::new();
                };
                range
                    .map(|i| this.closing_file(&line.files[i], line, &target, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .w_full()
        .h(px(28. * count.min(6) as f32))
    }

    fn review_button(
        &self,
        line: &Line,
        target: Option<Target>,
        latest: bool,
        cx: &mut Context<Self>,
    ) -> Control {
        let older = self.workspace.turn_review.older;
        let available = line.availability == Availability::Ready;
        let inspect_earlier = !latest && !older && !available;
        let detail = if older {
            "Turn review needs a newer host.".into()
        } else {
            line.review_detail.clone()
        };
        toned_button(
            SharedString::from(format!("review-{}", line.run)),
            "Review",
            Tone::Filled,
        )
        .h(px(24.))
        .aria_description(detail.clone())
        .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
        .disabled(target.is_none() || older || (!available && !inspect_earlier))
        .on_activate(cx, move |this, window, cx| {
            if let Some(target) = &target {
                if inspect_earlier {
                    this.show_earlier_turn(&target.original, window, cx);
                } else if available && !older {
                    this.open_turn_review(target.clone(), window, cx);
                }
            }
        })
    }

    fn closing_file(
        &self,
        file: &records::File,
        line: &Line,
        target: &Target,
        cx: &mut Context<Self>,
    ) -> Control {
        let (letter, tint) = match file.change {
            "Added" => ("A", colors().mint),
            "Deleted" => ("D", colors().red),
            _ => ("M", colors().blue),
        };
        let available = line.availability == Availability::Ready;
        let document = (file.change != "Deleted"
            && plait_client::markdown::document::markdown_path(&file.path))
        .then(|| file.path.clone());
        let detail = if document.is_some() && available {
            "Open the current checkout file.".into()
        } else {
            self.closing_file_detail(file, line).to_owned()
        };
        let mut target = target.clone();
        target.path = Some(file.path.clone());
        nav_row(
            SharedString::from(format!("run-file-{}-{}", line.run, file.path.display())),
            format!("{} · {detail}", file.change),
            false,
        )
        .h(px(28.))
        .min_w_0()
        .gap(px(6.))
        .disabled(!available)
        .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
        .child(
            div()
                .w(px(12.))
                .font_family(mono())
                .text_color(rgb(tint))
                .child(letter),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(mono())
                .child(file.name.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(colors().subtle))
                .child(file.folder.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .font_family(mono())
                .child(file.counts.clone()),
        )
        .on_activate(cx, move |this, window, cx| {
            if available && let Some(path) = &document {
                this.open_file_in_view(path.clone(), window, cx);
            } else if available {
                this.open_turn_review(target.clone(), window, cx);
            }
        })
    }

    pub(super) fn record_undo_button(
        &self,
        line: &Line,
        model: plait_client::history::this_turn::UndoAction<'_>,
        group: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Control {
        let run = &line.run;
        let label = model.label;
        let detail = model.detail.to_owned();
        let action = if let Some(group) = group {
            quiet_button(SharedString::from(format!("undo-{run}")), label, group)
        } else {
            button(SharedString::from(format!("undo-{run}")), label)
        };
        action
            .h(px(22.))
            .aria_description(detail.clone())
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
            .when(!model.enabled, |el| {
                el.text_color(rgb(colors().subtle))
                    .cursor_default()
                    .a11y_synthetic_children(|builder| builder.parent_node().set_disabled())
            })
            .when(model.enabled, |el| {
                el.on_activate(cx, {
                    let run = run.clone();
                    move |this, window, cx| {
                        if let Some(attempt) = this
                            .workspace
                            .turn_records
                            .retry
                            .clone()
                            .filter(|p| p.run == run)
                        {
                            if this.workspace.turn_records.pending.is_none() {
                                this.send_undo(attempt, window, cx);
                            }
                        } else {
                            this.undo_run(&run, false, window, cx);
                        }
                    }
                })
            })
    }
    pub(super) fn undo_panel(&self, line: &Line, cx: &mut Context<Self>) -> Option<AnyElement> {
        let blocked = self
            .workspace
            .turn_records
            .blocked
            .as_ref()
            .filter(|blocked| blocked.run == line.run)?;
        let covered = &blocked.coverage;
        let run = line.run.clone();
        let list = column()
            .id("undo-paths")
            .max_h(px(132.))
            .overflow_y_scroll()
            .gap_1()
            .children(blocked.changed.iter().map(|path| {
                div()
                    .font_family(mono())
                    .text_size(px(code_size()))
                    .line_height(px(code_line_height()))
                    .child(path.clone())
            }))
            .when(!covered.is_empty(), |el| {
                el.child(div().mt_2().child("Not covered"))
            })
            .children(covered.iter().map(|text| div().child(text.clone())));
        Some(column().mt_2().mb_1().p_3().gap_2().border_1().border_color(rgb(colors().edge)).rounded_md()
            .text_size(px(12.)).text_color(rgb(colors().muted))
            .child(div().text_color(rgb(colors().text)).child("Changed since this turn"))
            .child("Files found changed by you or a later run are left alone; Undo the rest restores only the other covered files.")
            .child(list)
            .when(blocked.changed_omitted > 0, |el| {
                el.child(format!("{} more changed paths", blocked.changed_omitted))
            })
            .when(blocked.omitted > 0, |el| {
                el.child(format!("{} more not-covered paths", blocked.omitted))
            })
            .when(blocked.host_omitted > 0, |el| {
                el.child(format!(
                    "The host preview omitted {} further coverage entries.",
                    blocked.host_omitted
                ))
            })
            .child(row().child(button("undo-rest", "Undo the rest").disabled(self.undo_refusal(line).is_some())
                .on_activate(cx, move |this, window, cx| this.undo_run(&run, true, window, cx)))
                .child(button("undo-cancel", "Cancel").on_activate(cx, |this, _, cx| this.cancel_undo_panel(cx))))
            .into_any_element())
    }
    pub(super) fn legacy_record_tip(&self) -> String {
        if self.host.protocol() < 5 {
            "Undo needs a newer host (version 5 or later).".into()
        } else if let Some(error) = &self.workspace.turn_records.error {
            format!("Records could not be read. {error}")
        } else if self.workspace.turn_records.loading {
            "Loading the host's run records.".into()
        } else {
            "No retained host record is available for this turn.".into()
        }
    }
}

fn closing_state(
    line: &Line,
    live: Option<host::ThreadStatus>,
) -> (&'static str, &'static str, u32) {
    use host::{ThreadStatus, TurnOutcome};
    if line.undone {
        return ("↶", "Undone", colors().subtle);
    }
    // Pre-outcome hosts can still identify the latest run through the live snapshot.
    let outcome = line.outcome.or(match live {
        Some(ThreadStatus::Idle) => Some(TurnOutcome::Finished),
        Some(ThreadStatus::Error) => Some(TurnOutcome::Failed),
        Some(ThreadStatus::Stopped) => Some(TurnOutcome::Stopped),
        Some(ThreadStatus::Interrupted) => Some(TurnOutcome::Interrupted),
        _ => None,
    });
    match outcome {
        Some(TurnOutcome::Finished) => ("✓", "Finished", colors().mint),
        Some(TurnOutcome::Failed) => ("⚠", "Failed", colors().red),
        Some(TurnOutcome::Stopped) => ("■", "Stopped", colors().amber),
        Some(TurnOutcome::Interrupted) => ("!", "Interrupted", colors().amber),
        None => ("·", "Outcome unavailable", colors().subtle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[core::prelude::v1::test]
    fn blocked_previews_bound_both_lists_before_rendering() {
        let coverage: Vec<_> = (0..10_000)
            .map(|n| Uncovered {
                path: format!("excluded-{n:05}/"),
                reason: plait_client::git::record::Reason::Unreadable,
            })
            .collect();
        let reply = UndoTurnReply {
            state: UndoState::Blocked,
            changed_since: (0..500).map(|n| format!("changed-{n}").into()).collect(),
            changed_since_total: 1000,
            not_covered: vec![],
            not_covered_total: 0,
            record_id: "target".into(),
            after_restart: false,
        };
        let blocked = Blocked::new("run".into(), reply, &coverage);
        assert_eq!(blocked.coverage.len(), records::PREVIEW_LIMIT);
        assert_eq!(blocked.changed.len(), records::PREVIEW_LIMIT);
        assert_eq!(blocked.omitted, 9988);
        assert_eq!(blocked.changed_omitted, 988);
    }
}
