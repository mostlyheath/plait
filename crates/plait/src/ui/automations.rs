//! Sidebar section, cards, thread header, and background host actions for automations.
//!
//! All socket I/O runs on the background executor; render reads only cached state.

use super::{Plait, navigation::now_ms, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{
    automations::{
        AutomationItem, format_last_result, format_next_run, outcome_label, schedule_with_zone,
        schedule_words, state_label,
    },
    host::{
        self,
        automations::{OccurrenceOutcome, State},
    },
};
use std::time::Duration;

fn state_tint(state: State) -> u32 {
    let palette = colors();
    match state {
        State::Active => palette.mint,
        State::Paused => palette.amber,
        State::Preparing => palette.blue,
        State::Failed => palette.red,
        State::Unsupported => palette.subtle,
    }
}

fn outcome_tint(outcome: OccurrenceOutcome) -> u32 {
    let palette = colors();
    match outcome {
        OccurrenceOutcome::Failed => palette.red,
        OccurrenceOutcome::Paused | OccurrenceOutcome::Skipped | OccurrenceOutcome::Missed => {
            palette.amber
        }
        OccurrenceOutcome::Pending | OccurrenceOutcome::Queued | OccurrenceOutcome::Other => {
            palette.subtle
        }
    }
}

fn state_mark(state: State, color: u32, connected: bool) -> AnyElement {
    if state == State::Preparing && connected {
        running_mark(px(13.)).into_any_element()
    } else {
        dot(color).size(px(7.)).into_any_element()
    }
}

fn detail_line() -> Div {
    row()
        .pl(px(15.))
        .gap(px(5.))
        .line_height(px(14.))
        .text_size(px(11.))
        .text_color(rgb(colors().subtle))
}

fn occurrence_note(outcome: OccurrenceOutcome, detail: Option<&str>) -> (String, bool) {
    let detail = detail.map(str::trim).filter(|text| !text.is_empty());
    match outcome {
        OccurrenceOutcome::Pending => ("Run requested…".into(), false),
        OccurrenceOutcome::Queued => ("Run queued.".into(), false),
        OccurrenceOutcome::Skipped => (
            detail.map_or_else(|| "Run skipped.".into(), |d| format!("Skipped: {d}")),
            false,
        ),
        OccurrenceOutcome::Paused => (
            detail.map_or_else(|| "Automation paused.".into(), |d| format!("Paused: {d}")),
            true,
        ),
        OccurrenceOutcome::Failed => (
            detail.map_or_else(|| "Run failed.".into(), |d| format!("Failed: {d}")),
            true,
        ),
        OccurrenceOutcome::Missed => ("Occurrence missed.".into(), false),
        OccurrenceOutcome::Other => (format!("Occurrence {}.", outcome_label(outcome)), false),
    }
}

impl Plait {
    /// Probes and refreshes the host's automations list on the background executor.
    /// Older hosts and hosts without `PLAIT_AUTOMATIONS=1` return `Ok(None)`, hiding the section.
    pub(super) fn refresh_automations(&mut self, cx: &mut Context<Self>) {
        if !self.host.connection.connected {
            return;
        }
        let epoch = self.host.connection.epoch;
        let Some(generation) = self.automations.begin_refresh(epoch) else {
            return;
        };
        self.automation_timer = None;
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            let Some(summaries) = host::automations::list(&client)? else {
                return Ok::<_, host::Error>(None);
            };
            let mut items = Vec::with_capacity(summaries.len());
            for summary in summaries {
                let id = summary.id.clone();
                let mut item = AutomationItem::from_summary(summary);
                if !item.supported() {
                    items.push(item);
                    continue;
                }
                match host::automations::get(&client, &id) {
                    Ok(detail) => item.apply_detail(detail),
                    Err(host::Error::Rejected(reason)) if reason.code == "not_found" => continue,
                    Err(host::Error::Protocol(_)) => item.summary.state = State::Unsupported,
                    Err(_) => {}
                }
                items.push(item);
            }
            Ok(Some(items))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch != epoch {
                    return;
                }
                let result = result.map_err(|error| error.to_string());
                if this.automations.apply_list(epoch, generation, result) {
                    this.navigation.invalidate_rows();
                    if this.automations.refresh_queued {
                        this.refresh_automations(cx);
                    } else if let Some(delay) = this.automations.next_refresh_delay(now_ms()) {
                        this.schedule_automation_refresh(epoch, generation, delay, cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn schedule_automation_refresh(
        &mut self,
        epoch: u64,
        generation: u64,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        self.automation_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.connected
                    && this.host.connection.epoch == epoch
                    && this.automations.generation == generation
                {
                    this.refresh_automations(cx);
                }
            });
        }));
    }

    pub(super) fn refresh_automation_detail(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.host.connection.connected || !self.automations.supported {
            return;
        }
        let epoch = self.host.connection.epoch;
        let client = self.host_client();
        let revision = self.automations.card_revision(&id);
        let request_id = id.clone();
        let task = cx
            .background_executor()
            .spawn(async move { host::automations::get(&client, &request_id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch != epoch
                    || this.automations.card_revision(&id) != revision
                {
                    return;
                }
                match result {
                    Ok(detail) => {
                        if this.automations.apply_detail(epoch, detail) {
                            this.navigation.invalidate_rows();
                            cx.notify();
                        }
                    }
                    Err(host::Error::Rejected(reason)) if reason.code == "not_found" => {
                        if this.automations.apply_deleted(epoch, &id) {
                            this.navigation.invalidate_rows();
                            cx.notify();
                        }
                    }
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    pub(super) fn open_automation(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.automations.by_id(&id).cloned() else {
            return;
        };
        if !item.supported() {
            return;
        }
        match item.summary.state {
            State::Unsupported => {}
            State::Preparing => {
                self.refresh_automations(cx);
            }
            State::Failed => {
                let next =
                    (self.automations.confirm_delete.as_deref() != Some(id.as_str())).then_some(id);
                self.automations.set_confirm_delete(next);
                self.navigation.invalidate_rows();
                cx.notify();
            }
            State::Active | State::Paused => {
                if let Some(thread_id) = item.effective_thread_id().map(str::to_owned) {
                    self.refresh_automation_detail(id, cx);
                    self.open_automation_thread(thread_id, window, cx);
                    return;
                }
                if !self.host.connection.connected {
                    return;
                }
                let epoch = self.host.connection.epoch;
                let client = self.host_client();
                let revision = self.automations.card_revision(&id);
                let fetch_id = id.clone();
                let task = cx
                    .background_executor()
                    .spawn(async move { host::automations::get(&client, &fetch_id) });
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.host.connection.epoch != epoch
                            || this.automations.card_revision(&id) != revision
                        {
                            return;
                        }
                        if let Ok(detail) = result {
                            let thread_id = detail.automation.thread_id.clone();
                            this.automations.apply_detail(epoch, detail);
                            this.navigation.invalidate_rows();
                            if let Some(thread_id) = thread_id {
                                this.open_automation_thread(thread_id, window, cx);
                            }
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn open_automation_thread(
        &mut self,
        thread_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overview(window, cx);
        if self
            .host
            .connection
            .threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            self.automations.pending_open_thread = None;
            self.select_host_thread(thread_id, window, cx);
        } else {
            self.automations.pending_open_thread = Some(thread_id);
            self.refresh_thread_list(window, cx);
        }
    }

    pub(super) fn open_pending_automation_thread(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(thread_id) = self.automations.pending_open_thread.clone() else {
            return;
        };
        if self
            .host
            .connection
            .threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            self.automations.pending_open_thread = None;
            self.select_host_thread(thread_id, window, cx);
        }
    }

    fn toggle_automation_pause(&mut self, id: String, pause: bool, cx: &mut Context<Self>) {
        if !self.host.connection.connected {
            return;
        }
        let epoch = self.host.connection.epoch;
        let Some(generation) = self.automations.begin_action(epoch, id.clone()) else {
            return;
        };
        self.automations.clear_note(&id);
        cx.notify();
        let client = self.host_client();
        let target_id = id.clone();
        let task = cx.background_executor().spawn(async move {
            let summary = if pause {
                host::automations::pause(&client, &target_id)?
            } else {
                host::automations::resume(&client, &target_id)?
            };
            let detail = host::automations::get(&client, &target_id).ok();
            Ok::<_, host::Error>((summary, detail))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if !this.automations.finish_action(epoch, generation) {
                    return;
                }
                match result {
                    Ok((summary, detail)) => {
                        this.automations.record_successful_action(&id);
                        this.automations.apply_summary(epoch, summary);
                        if let Some(detail) = detail {
                            this.automations.apply_detail(epoch, detail);
                        }
                        this.refresh_automations(cx);
                        this.navigation.invalidate_rows();
                    }
                    Err(error) => {
                        this.automations.set_note(id, error.to_string(), true);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn settle_run_click(
        &mut self,
        generation: u64,
        id: String,
        click: String,
        cx: &mut Context<Self>,
    ) {
        let epoch = self.automations.epoch;
        let machine = self.machines.active.clone();
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        let target = id.clone();
        let task = cx
            .background_executor()
            .spawn(async move { journal.clear_run_click(&machine, &target, &click) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.automations.finish_action(epoch, generation) {
                    if let Err(error) = result {
                        this.automations.set_note(id.clone(), error, true);
                    }
                    this.refresh_automation_detail(id, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn run_automation_now(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.host.connection.connected || !self.session.ready() {
            return;
        }
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        let epoch = self.host.connection.epoch;
        let Some(generation) = self.automations.begin_action(epoch, id.clone()) else {
            return;
        };
        self.automations.clear_note(&id);
        cx.notify();
        let machine = self.machines.active.clone();
        let client = self.host_client();
        let target_id = id.clone();
        let task = cx.background_executor().spawn(async move {
            let click = journal
                .run_click(&machine, &target_id)
                .map_err(host::Error::Protocol)?;
            host::automations::run_now(&client, &target_id, &click)
                .map(|occurrence| (occurrence, click))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.automations.epoch != epoch
                    || this.automations.action_generation != generation
                {
                    return;
                }
                match result {
                    Ok((occurrence, click)) => {
                        this.automations.record_successful_action(&id);
                        let (message, is_error) =
                            occurrence_note(occurrence.outcome, occurrence.detail.as_deref());
                        this.automations.set_note(id.clone(), message, is_error);
                        if matches!(
                            occurrence.outcome,
                            OccurrenceOutcome::Pending | OccurrenceOutcome::Other
                        ) {
                            this.poll_run_receipt(
                                generation,
                                id,
                                click,
                                occurrence.request_id,
                                0,
                                cx,
                            );
                        } else {
                            this.settle_run_click(generation, id, click, cx);
                        }
                    }
                    Err(error) => {
                        this.automations.finish_action(epoch, generation);
                        this.automations.set_note(id, error.to_string(), true);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn poll_run_receipt(
        &mut self,
        generation: u64,
        id: String,
        click: String,
        occurrence_request_id: String,
        attempt: usize,
        cx: &mut Context<Self>,
    ) {
        let epoch = self.automations.epoch;
        if attempt >= 8 || !self.host.connection.connected {
            self.automations.finish_action(epoch, generation);
            self.automations.set_note(
                id,
                "Run unconfirmed. Retry to check the same request.".into(),
                false,
            );
            cx.notify();
            return;
        }
        let client = self.host_client();
        let target_id = id.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let detail = cx
                .background_executor()
                .spawn(async move { host::automations::get(&client, &target_id) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch != epoch
                    || this.automations.action_generation != generation
                {
                    return;
                }
                if let Ok(detail) = detail {
                    let receipt = detail
                        .occurrences
                        .iter()
                        .find(|occ| occ.request_id == occurrence_request_id)
                        .cloned();
                    this.automations.apply_detail(epoch, detail);
                    this.navigation.invalidate_rows();
                    if let Some(receipt) = receipt.filter(|r| {
                        !matches!(
                            r.outcome,
                            OccurrenceOutcome::Pending | OccurrenceOutcome::Other
                        )
                    }) {
                        let (message, is_error) =
                            occurrence_note(receipt.outcome, receipt.detail.as_deref());
                        this.automations.set_note(id.clone(), message, is_error);
                        this.settle_run_click(generation, id, click, cx);
                        this.refresh_automations(cx);
                        cx.notify();
                        return;
                    }
                }
                this.poll_run_receipt(
                    generation,
                    id,
                    click,
                    occurrence_request_id,
                    attempt + 1,
                    cx,
                );
            });
        })
        .detach();
    }

    fn delete_automation(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.host.connection.connected {
            return;
        }
        let epoch = self.host.connection.epoch;
        let Some(generation) = self.automations.begin_action(epoch, id.clone()) else {
            return;
        };
        self.automations.clear_note(&id);
        cx.notify();
        let client = self.host_client();
        let target_id = id.clone();
        let task = cx
            .background_executor()
            .spawn(async move { host::automations::delete(&client, &target_id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if !this.automations.finish_action(epoch, generation) {
                    return;
                }
                match result {
                    Ok(_) => {
                        this.automations.record_successful_action(&id);
                        this.automations.apply_deleted(epoch, &id);
                        this.navigation.invalidate_rows();
                    }
                    Err(error) => {
                        this.automations.set_note(id, error.to_string(), true);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn automations_header_row(
        &self,
        after: bool,
        folded: Option<usize>,
        focus: &FocusHandle,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = !self.automations.collapsed;
        let count = self.automations.items.len();
        let group = SharedString::from("automations-section-row");
        column()
            .w_full()
            .when(after, |el| {
                el.mt(px(8.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(rgb(colors().border))
            })
            .child(
                row()
                    .group(group.clone())
                    .h(px(26.))
                    .gap_0()
                    .child(
                        nav_row(
                            "automations-header",
                            format!("Automations section, {count}"),
                            false,
                        )
                        .w_auto()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .px(px(9.))
                        .track_focus(focus)
                        .role(Role::TreeItem)
                        .aria_level(1)
                        .aria_expanded(expanded)
                        .child(icon(if expanded { "down" } else { "right" }).size(px(9.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(rgb(colors().subtle))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Automations"),
                        )
                        .when_some(folded, |el, count| {
                            el.child(
                                tag(
                                    "automations-folded-count",
                                    count.to_string(),
                                    colors().subtle,
                                )
                                .flex_shrink_0(),
                            )
                        })
                        .on_activate(cx, |this, _, cx| {
                            this.automations.collapsed = !this.automations.collapsed;
                            this.navigation.invalidate_rows();
                            cx.notify();
                        }),
                    )
                    .child(
                        icon_button("automations-refresh", "refresh", "Refresh automations")
                            .size(px(22.))
                            .opacity(if focused { 1. } else { 0. })
                            .group_hover(group, |s| s.opacity(1.))
                            .focus_visible(|s| s.border_color(rgb(colors().blue)).opacity(1.))
                            .tooltip(|_, cx| {
                                cx.new(|_| TextTooltip("Refresh automations".into())).into()
                            })
                            .on_activate(cx, |this, _, cx| this.refresh_automations(cx)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn automations_empty_row(&self, focus: &FocusHandle) -> AnyElement {
        let (label, tint) = match &self.automations.error {
            Some(error) => (error.clone(), colors().amber),
            None => ("No automations yet.".into(), colors().subtle),
        };
        div()
            .id("automations-empty")
            .track_focus(focus)
            .role(Role::TreeItem)
            .aria_level(2)
            .aria_label(label.clone())
            .audited()
            .px(px(9.))
            .py(px(6.))
            .text_size(px(11.))
            .text_color(rgb(tint))
            .child(label)
            .into_any_element()
    }

    pub(super) fn automation_card(
        &self,
        index: usize,
        focus: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(item) = self.automations.items.get(index) else {
            return div().into_any_element();
        };
        let palette = colors();
        let now = now_ms();
        let id = item.summary.id.clone();
        let state = item.summary.state;
        let state_word = state_label(state);
        let state_color = state_tint(state);
        let pause_reason = item.effective_pause_reason().map(str::to_owned);
        let error = item.effective_error().map(str::to_owned);
        let reason_line = error
            .as_deref()
            .map(|text| (text.to_owned(), palette.red))
            .or_else(|| {
                pause_reason
                    .as_deref()
                    .map(|text| (text.to_owned(), palette.amber))
            });
        let schedule_short = schedule_words(&item.summary.schedule);
        let schedule_full = schedule_with_zone(&item.summary.schedule);
        let target = item.summary.target_path.display().to_string();
        let next_run = format_next_run(item.summary.next_run, now);
        let last_result = format_last_result(item.summary.last_result.as_ref(), now);
        let selected = item
            .effective_thread_id()
            .is_some_and(|tid| self.host.thread.selected.as_deref() == Some(tid));
        let emphasis = state != State::Paused || pause_reason.is_some();
        let confirming_failed = state == State::Failed
            && self.automations.confirm_delete.as_deref() == Some(id.as_str());
        let busy = self.automations.busy.is_some();
        let can_delete = item.supported() && self.host.connection.connected && !busy;

        let mut tooltip_parts = vec![format!("{} · {}", item.summary.name, state_word)];
        if let Some(reason) = &pause_reason {
            tooltip_parts.push(format!("Paused: {reason}"));
        }
        if let Some(err) = &error {
            tooltip_parts.push(format!("Error: {err}"));
        }
        tooltip_parts.push(format!("Schedule: {schedule_full}"));
        tooltip_parts.push(format!("Target: {target}"));
        if let Some(next) = &next_run {
            tooltip_parts.push(next.clone());
        }
        if let Some(last) = &last_result {
            tooltip_parts.push(format!("Last: {}", last.full));
        }
        let tooltip = tooltip_parts.join("\n");

        let mut accessible = format!(
            "Automation {} · {}. {}",
            item.summary.name, state_word, schedule_short
        );
        if let Some(reason) = &pause_reason {
            accessible.push_str(&format!(". {reason}"));
        }
        if let Some(err) = &error {
            accessible.push_str(&format!(". {err}"));
        }
        accessible.push_str(&format!(". Target {target}"));
        if let Some(next) = &next_run {
            accessible.push_str(&format!(". {next}"));
        }
        if let Some(last) = &last_result {
            accessible.push_str(&format!(". Last {}", last.full));
        }

        let open_id = id.clone();
        let delete_id = id.clone();

        column()
            .id(("automation-item", index))
            .w_full()
            .pb(px(2.))
            .child(
                nav_row(("automation-card", index), accessible, selected)
                    .role(Role::TreeItem)
                    .disabled(!item.supported())
                    .aria_level(2)
                    .aria_selected(selected)
                    .track_focus(focus)
                    .h_auto()
                    .flex_col()
                    .items_stretch()
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(7.))
                    .gap(px(3.))
                    .when(selected, |el| {
                        el.border_color(rgb(palette.edge))
                            .hover(|s| s.bg(rgba(colors().hover)))
                            .active(|s| s.opacity(0.6))
                    })
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(tooltip.clone())).into())
                    .child(
                        row()
                            .line_height(px(16.))
                            .gap(px(2.))
                            .child(row().w(px(13.)).justify_center().flex_shrink_0().child(
                                state_mark(state, state_color, self.host.connection.connected),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .when(emphasis, |el| {
                                        el.text_color(rgb(palette.strong))
                                            .font_weight(FontWeight::MEDIUM)
                                    })
                                    .when(!emphasis, |el| el.text_color(rgb(palette.muted)))
                                    .child(item.summary.name.clone()),
                            )
                            .child(
                                tag(("automation-state", index), state_word, state_color)
                                    .font_weight(FontWeight::MEDIUM)
                                    .flex_shrink_0(),
                            ),
                    )
                    .when_some(reason_line, |el, (reason, tint)| {
                        el.child(
                            detail_line().text_color(rgb(tint)).child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(a11y_text(("automation-reason", index), reason)),
                            ),
                        )
                    })
                    .child(
                        detail_line()
                            .child(icon("clock").size(px(11.)).text_color(rgb(palette.subtle)))
                            .child(div().flex_1().min_w_0().truncate().child(schedule_short))
                            .when_some(next_run, |el, next| {
                                el.child(
                                    div()
                                        .flex_shrink_0()
                                        .font_family(mono())
                                        .font_features(tabular_nums())
                                        .child(next),
                                )
                            }),
                    )
                    .child(
                        detail_line()
                            .child(icon("folder").size(px(11.)).text_color(rgb(palette.subtle)))
                            .child(div().flex_1().min_w_0().truncate().child(target))
                            .when_some(last_result, |el, last| {
                                el.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(rgb(outcome_tint(last.outcome)))
                                        .child(last.compact),
                                )
                            }),
                    )
                    .on_activate(cx, move |this, window, cx| {
                        this.open_automation(open_id.clone(), window, cx)
                    }),
            )
            .when(confirming_failed, |el| {
                el.child(
                    row()
                        .pl(px(23.))
                        .pr(px(8.))
                        .py(px(4.))
                        .gap_1()
                        .child(
                            toned_button(
                                ("automation-failed-delete-confirm", index),
                                "Delete",
                                Tone::Filled,
                            )
                            .h(px(22.))
                            .text_color(rgb(palette.red))
                            .disabled(!can_delete)
                            .when(can_delete, |el| {
                                el.on_activate(cx, move |this, _, cx| {
                                    this.delete_automation(delete_id.clone(), cx)
                                })
                            }),
                        )
                        .child(
                            button(("automation-failed-delete-cancel", index), "Cancel")
                                .h(px(22.))
                                .on_activate(cx, |this, _, cx| {
                                    this.automations.set_confirm_delete(None);
                                    this.navigation.invalidate_rows();
                                    cx.notify();
                                }),
                        ),
                )
            })
            .into_any_element()
    }

    /// Compact header shown at the top of an automation's thread with schedule and controls.
    pub(super) fn automation_header(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selected_thread = self.host.thread.selected.as_deref()?;
        let item = self.automations.by_thread(selected_thread)?;
        let palette = colors();
        let now = now_ms();
        let id = item.summary.id.clone();
        let state = item.summary.state;
        let state_word = state_label(state);
        let state_color = state_tint(state);
        let schedule = schedule_with_zone(&item.summary.schedule);
        let next_run = format_next_run(item.summary.next_run, now);
        let last_result = format_last_result(item.summary.last_result.as_ref(), now);
        let pause_reason = item.effective_pause_reason().map(str::to_owned);
        let note = self
            .automations
            .note
            .as_ref()
            .filter(|(note_id, _, _)| note_id == &id)
            .map(|(_, message, is_error)| (message.clone(), *is_error));

        let paused = state == State::Paused;
        let confirming = self.automations.confirm_delete.as_deref() == Some(id.as_str());
        let busy = self.automations.busy.is_some();
        let connected = self.host.connection.connected;
        let can_schedule_action = item.supported()
            && connected
            && !busy
            && matches!(state, State::Active | State::Paused);
        let can_delete = item.supported() && connected && !busy && state != State::Preparing;

        let disabled_reason = if !item.supported() {
            Some("This automation requires a newer client.")
        } else if !connected {
            Some("Reconnect to the host to manage this automation.")
        } else if busy {
            Some("Waiting for the previous automation action to finish.")
        } else if state == State::Preparing {
            Some("Automation is still preparing.")
        } else if state == State::Failed {
            Some("Automation placement failed.")
        } else {
            None
        };

        let run_tip = disabled_reason
            .unwrap_or("Start a run now using this automation's saved instructions.")
            .to_owned();
        let pause_tip = disabled_reason
            .unwrap_or(if paused {
                "Resume scheduled runs for this automation."
            } else {
                "Pause future scheduled runs. Any turn already running keeps going."
            })
            .to_owned();
        let delete_tip = if !item.supported() {
            "This automation requires a newer client."
        } else if !connected {
            "Reconnect to the host to delete this automation."
        } else if busy {
            "Waiting for the previous automation action to finish."
        } else if state == State::Preparing {
            "Wait for preparation to finish before deleting."
        } else {
            "Delete this automation. The thread and its history stay as an ordinary thread."
        }
        .to_owned();

        let run_id = id.clone();
        let toggle_id = id.clone();
        let ask_delete_id = id.clone();
        let confirm_delete_id = id.clone();

        Some(
            column()
                .id("automation-thread-header")
                .role(Role::Group)
                .aria_label(format!(
                    "Automation {} · {}. {}",
                    item.summary.name, state_word, schedule
                ))
                .audited()
                .w_full()
                .flex_shrink_0()
                .border_b_1()
                .border_color(rgb(palette.border))
                .child(
                    column()
                        .w_full()
                        .max_w(px(CONTENT_WIDTH + 48.))
                        .mx_auto()
                        .px_6()
                        .py_2()
                        .gap_1()
                        .child(
                            row()
                                .w_full()
                                .flex_wrap()
                                .gap_x(px(8.))
                                .gap_y(px(6.))
                                .child(
                                    row()
                                        .flex_1()
                                        .min_w_0()
                                        .flex_wrap()
                                        .gap_x(px(8.))
                                        .gap_y(px(4.))
                                        .child(
                                            row()
                                                .gap(px(5.))
                                                .flex_shrink_0()
                                                .child(state_mark(state, state_color, connected))
                                                .child(
                                                    tag(
                                                        "automation-header-state",
                                                        state_word,
                                                        state_color,
                                                    )
                                                    .font_weight(FontWeight::MEDIUM),
                                                ),
                                        )
                                        .child(
                                            row()
                                                .gap(px(4.))
                                                .min_w_0()
                                                .child(
                                                    icon("clock")
                                                        .size(px(12.))
                                                        .text_color(rgb(palette.subtle)),
                                                )
                                                .child(
                                                    tag(
                                                        "automation-header-schedule",
                                                        schedule,
                                                        palette.text,
                                                    )
                                                    .text_size(px(12.)),
                                                ),
                                        )
                                        .when_some(next_run, |el, next| {
                                            el.child(
                                                tag("automation-header-next", next, palette.subtle)
                                                    .font_family(mono())
                                                    .font_features(tabular_nums())
                                                    .flex_shrink_0(),
                                            )
                                        })
                                        .when_some(last_result, |el, last| {
                                            el.child(
                                                tag(
                                                    "automation-header-last",
                                                    format!("Last: {}", last.full),
                                                    outcome_tint(last.outcome),
                                                )
                                                .flex_shrink_0(),
                                            )
                                        }),
                                )
                                .when(!confirming, |el| {
                                    el.child(
                                        row()
                                            .flex_shrink_0()
                                            .gap_1()
                                            .child(
                                                toned_button(
                                                    "automation-run-now",
                                                    "Run now",
                                                    Tone::Filled,
                                                )
                                                .h(px(24.))
                                                .disabled(!can_schedule_action)
                                                .aria_description(run_tip.clone())
                                                .tooltip(move |_, cx| {
                                                    cx.new(|_| TextTooltip(run_tip.clone())).into()
                                                })
                                                .when(can_schedule_action, |el| {
                                                    el.on_activate(cx, move |this, _, cx| {
                                                        this.run_automation_now(run_id.clone(), cx)
                                                    })
                                                }),
                                            )
                                            .child(
                                                button(
                                                    "automation-toggle-pause",
                                                    if paused { "Resume" } else { "Pause" },
                                                )
                                                .h(px(24.))
                                                .disabled(!can_schedule_action)
                                                .aria_description(pause_tip.clone())
                                                .tooltip(move |_, cx| {
                                                    cx.new(|_| TextTooltip(pause_tip.clone()))
                                                        .into()
                                                })
                                                .when(can_schedule_action, |el| {
                                                    el.on_activate(cx, move |this, _, cx| {
                                                        this.toggle_automation_pause(
                                                            toggle_id.clone(),
                                                            !paused,
                                                            cx,
                                                        )
                                                    })
                                                }),
                                            )
                                            .child(
                                                button("automation-delete", "Delete")
                                                    .h(px(24.))
                                                    .text_color(rgb(palette.red))
                                                    .disabled(!can_delete)
                                                    .aria_description(delete_tip.clone())
                                                    .tooltip(move |_, cx| {
                                                        cx.new(|_| TextTooltip(delete_tip.clone()))
                                                            .into()
                                                    })
                                                    .when(can_delete, |el| {
                                                        el.on_activate(cx, move |this, _, cx| {
                                                            this.automations.set_confirm_delete(
                                                                Some(ask_delete_id.clone()),
                                                            );
                                                            cx.notify();
                                                        })
                                                    }),
                                            ),
                                    )
                                }),
                        )
                        .when_some(pause_reason, |el, reason| {
                            el.child(tag(
                                "automation-header-pause-reason",
                                format!("Paused: {reason}"),
                                palette.amber,
                            ))
                        })
                        .when(confirming, |el| {
                            el.child(
                                row()
                                    .w_full()
                                    .flex_wrap()
                                    .gap_2()
                                    .pt(px(2.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .text_color(rgb(palette.muted))
                                            .child(a11y_text(
                                                "automation-delete-confirm-prompt",
                                                "Delete this automation? The thread and its history stay as an ordinary thread.",
                                            )),
                                    )
                                    .child(
                                        toned_button(
                                            "automation-delete-confirm",
                                            "Delete automation",
                                            Tone::Filled,
                                        )
                                        .h(px(24.))
                                        .text_color(rgb(palette.red))
                                        .disabled(!can_delete)
                                        .when(can_delete, |el| {
                                            el.on_activate(cx, move |this, _, cx| {
                                                this.delete_automation(
                                                    confirm_delete_id.clone(),
                                                    cx,
                                                )
                                            })
                                        }),
                                    )
                                    .child(
                                        button("automation-delete-cancel", "Cancel")
                                            .h(px(24.))
                                            .on_activate(cx, |this, _, cx| {
                                                this.automations.set_confirm_delete(None);
                                                cx.notify();
                                            }),
                                    ),
                            )
                        })
                        .when_some(note, |el, (message, is_error)| {
                            el.child(
                                tag(
                                    "automation-header-note-text",
                                    message,
                                    if is_error {
                                        palette.red
                                    } else {
                                        palette.subtle
                                    },
                                )
                                .id("automation-header-note")
                                .role(if is_error { Role::Alert } else { Role::Status })
                                .audited(),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}
