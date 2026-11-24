//! Controls for the next message and host-owned inputs waiting behind a run.
use super::{
    Plait,
    commands::Command,
    composer::Composer,
    harnesses::{ModeLine, harness_of},
    model_picker::ModelPurpose,
    palette::{Item, Options, Palette},
    permissions::Permissions,
    theme::*,
};
use gpui::{prelude::*, *};
use plait_client::{
    compose::{
        self, ContextPressure, ContextUsage, approval_label, approval_tip, run_mode_note,
        thinking_levels, thinking_value,
    },
    host::{
        self, ApprovalMode, Model, ModelChoice, QueueMode, Request, ThinkingLevel,
        harness::HarnessId,
    },
};
use std::{collections::HashMap, path::PathBuf, rc::Rc, sync::Arc};

/// Controls and notices that join the draft inside the message box.
#[derive(Default)]
pub(super) struct MessageBoxParts {
    /// Left side of the settings row (harness mark, model, effort, approval/permissions).
    pub(super) left: Vec<AnyElement>,
    /// Right side of the settings row, before Stop and Send (context use).
    pub(super) right: Vec<AnyElement>,
    /// Notices shown inside the box beneath the settings row only when present.
    pub(super) notices: Vec<AnyElement>,
}

#[derive(Default)]
pub(super) struct ThreadControls {
    serial: u64,
    pending: Option<(String, u64)>,
    pending_epoch: u64,
    pub picker_thread: Option<(String, u64)>,
    pub(super) note: Option<(String, String, bool)>,
    cancelling: HashMap<String, u64>,
    pub(super) editing: HashMap<String, u64>,
    pub(super) usage_open: bool,
    /// Approval choices made this session, by thread. `None` is an explicit
    /// return to the default, so it shadows an older saved choice.
    approvals: HashMap<String, Option<ApprovalMode>>,
    /// A harness mode change waiting on the host's reply.
    mode: Option<ModeRequest>,
}

/// A harness thread's mode change, from the click until the host answers.
/// A reply is used only while this is still the latest request and the same
/// thread is open on the same connection.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModeRequest {
    thread: String,
    checkout: PathBuf,
    serial: u64,
    epoch: u64,
    watch_epoch: u64,
    mode: String,
}

/// Plain text shown on hover, for controls whose label is a summary.
pub(super) struct TextTooltip(pub String);
impl Render for TextTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(360.))
            .p_3()
            .bg(rgb(colors().panel))
            .border_1()
            .border_color(rgb(colors().border))
            .rounded_md()
            .text_size(px(12.))
            .text_color(rgb(colors().text))
            .child(self.0.clone())
    }
}

impl Plait {
    pub(super) fn set_thread_notice(&mut self, id: String, message: String, failed: bool) {
        self.workspace.thread_controls.note = Some((id, message, failed));
    }

    /// The open thread's model, from its harness's list when it does not run in Pi.
    fn thread_model(&self) -> Option<&Model> {
        let snapshot = self.host.thread.snapshot.as_ref()?;
        let harness = harness_of(snapshot);
        if harness != HarnessId::Pi {
            return self.harness_model(harness, &snapshot.model);
        }
        self.host
            .connection
            .status
            .as_ref()?
            .models
            .iter()
            .find(|model| model.choice() == snapshot.model)
    }

    pub(super) fn thread_configuration_pending(&self) -> bool {
        self.workspace.thread_controls.pending_epoch == self.host.connection.epoch
            && self
                .workspace
                .thread_controls
                .pending
                .as_ref()
                .is_some_and(|(id, _)| Some(id) == self.host.thread.selected.as_ref())
    }

    fn configure_reason(&self) -> Option<&'static str> {
        compose::configure_reason(
            self.host.protocol(),
            self.host.connection.connected,
            self.host.thread.snapshot.is_some(),
            self.host
                .thread
                .snapshot
                .as_ref()
                .is_some_and(|s| s.status.busy()),
            self.thread_configuration_pending(),
        )
    }

    pub(super) fn open_thread_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = self.configure_reason() {
            if let Some(id) = &self.host.thread.selected {
                self.workspace.thread_controls.note = Some((id.clone(), reason.into(), false));
            }
            cx.notify();
            return;
        }
        self.workspace.thread_controls.picker_thread = self
            .host
            .thread
            .selected
            .clone()
            .map(|id| (id, self.host.connection.epoch));
        self.open_model_picker(ModelPurpose::ThreadModel, window, cx);
    }

    pub(super) fn choose_thread_model(
        &mut self,
        choice: ModelChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, epoch)) = self.workspace.thread_controls.picker_thread.take() else {
            return;
        };
        if epoch != self.host.connection.epoch || self.host.thread.selected.as_ref() != Some(&id) {
            return;
        }
        self.configure_selected(Some(choice), None, window, cx);
    }

    pub(super) fn configure_selected(
        &mut self,
        model: Option<ModelChoice>,
        thinking: Option<ThinkingLevel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.configure_reason().is_some() {
            return;
        }
        let Some(snapshot) = &self.host.thread.snapshot else {
            return;
        };
        if let Some(level) = thinking
            && !thinking_levels(self.thread_model()).contains(&level)
        {
            return;
        }
        let id = snapshot.id.clone();
        let checkout = snapshot.checkout.clone();
        let watch_epoch = self.host.thread.watch_epoch;
        self.workspace.thread_controls.serial += 1;
        let serial = self.workspace.thread_controls.serial;
        self.workspace.thread_controls.pending = Some((id.clone(), serial));
        self.workspace.thread_controls.pending_epoch = self.host.connection.epoch;
        // The controls show the new choice; only a failure needs words.
        self.workspace.thread_controls.note = None;
        self.sync_composer(cx);
        self.host_request::<host::ThreadReply>(
            Request::configure_thread(id.clone(), model.clone(), thinking),
            window,
            cx,
            move |this, result, _, cx| {
                if this.workspace.thread_controls.pending.as_ref() != Some(&(id.clone(), serial)) {
                    return;
                }
                this.workspace.thread_controls.pending = None;
                if !this
                    .host
                    .accepts_watch(this.host.connection.epoch, watch_epoch, &id)
                {
                    return;
                }
                let result = match result {
                    Ok(reply)
                        if reply.thread.id == id
                            && reply.thread.checkout == checkout
                            && model.as_ref().is_none_or(|m| *m == reply.thread.model)
                            && thinking.is_none_or(|t| Some(t) == reply.thread.thinking_level) =>
                    {
                        Ok(())
                    }
                    Ok(_) => {
                        Err("Host returned different settings. Reconnect to check them.".into())
                    }
                    Err(error) => Err(error.to_string()),
                };
                this.workspace.thread_controls.note = result
                    .err()
                    .map(|error| (id, format!("Not saved: {error}"), true));
                this.capture_session(cx);
            },
        );
    }

    pub(super) fn open_thinking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.configure_reason().is_some() {
            return;
        }
        let thread_id = self.host.thread.selected.clone().unwrap_or_default();
        let epoch = self.host.connection.epoch;
        // As the thread's chip shows it: no level is thinking off.
        let current = self
            .host
            .thread
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.thinking_level.unwrap_or(ThinkingLevel::Off));
        let levels = thinking_levels(self.thread_model()).to_vec();
        self.present_thinking(
            &levels,
            current,
            "Applies from the next run.",
            move |level| Command::Thinking {
                thread_id: thread_id.clone(),
                epoch,
                level,
            },
            window,
            cx,
        );
    }

    /// The thinking-level menu, for an open thread or the new-thread draft.
    /// `command` is what choosing each level runs.
    pub(super) fn present_thinking(
        &mut self,
        levels: &[ThinkingLevel],
        current: Option<ThinkingLevel>,
        note: &'static str,
        command: impl Fn(ThinkingLevel) -> Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = |level: ThinkingLevel| format!("thinking-{level:?}");
        let items = levels
            .iter()
            .map(|level| {
                let item = Item::new(key(*level), level.label(), command(*level));
                if Some(*level) == current {
                    item.badge("Current", colors().blue)
                } else {
                    item
                }
            })
            .collect();
        let palette = cx.new(|cx| {
            Palette::new(
                Options {
                    name: "Thinking level".into(),
                    placeholder: "Choose thinking level…".into(),
                    empty: "This model has no thinking levels.".into(),
                    // Start on the current level, so Return keeps it.
                    initial: current.map(|level| key(level).into()),
                    mark: None,
                    aside: None,
                    note: Some(note.into()),
                },
                items,
                window,
                cx,
            )
            .picker()
        });
        self.present(palette, None, window, cx);
    }

    /// One raised outline (12-point corners, capped at 760) holding the draft
    /// and its settings row, shared by open threads and draft tabs.
    pub(super) fn message_box(
        &self,
        composer: Entity<Composer>,
        parts: MessageBoxParts,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (box_focus, can_send, busy, queue_enabled, can_stop, is_blank, hint) = {
            let state = composer.read(cx);
            (
                state.box_focus.clone(),
                state.can_send(),
                state.busy(),
                state.queue_enabled(),
                state.can_stop(),
                state.is_blank(),
                state.hint(),
            )
        };
        let focused = box_focus.contains_focused(window, cx);
        let drop_composer = composer.clone();
        let stop_composer = composer.clone();
        let send_composer = composer.clone();
        let show_stop = busy && (is_blank || !queue_enabled);
        let left_controls = row()
            .flex_wrap()
            .items_center()
            .gap_x_1()
            .gap_y_1()
            .min_w_0()
            .children(parts.left);
        let right_controls = row()
            .flex_shrink_0()
            .items_center()
            .gap_x_2()
            .ml_auto()
            .children(parts.right)
            .when(show_stop, |el| {
                let tip = if can_stop {
                    "Stop this run (Esc twice)"
                } else {
                    "Cannot stop this run right now."
                };
                el.child(
                    // While a run is active and the draft is empty, the action
                    // slot becomes Stop in coral.
                    toned_icon_button("composer-stop", "stop", "Stop", Tone::Primary)
                        .rounded(px(8.))
                        .bg(rgb(colors().red))
                        .disabled(!can_stop)
                        .tooltip(move |_, cx| cx.new(|_| TextTooltip(tip.into())).into())
                        .on_activate(cx, move |_, _, cx| {
                            stop_composer.update(cx, |composer, cx| composer.stop_from_button(cx));
                        }),
                )
            })
            .when(!show_stop, |el| {
                let disabled_tip = if hint.is_empty() {
                    "Write a message to send.".to_owned()
                } else {
                    hint.clone()
                };
                el.child(
                    // Send is the primary action: 28 square, 8 corners, disabled when empty.
                    toned_icon_button(
                        "composer-send",
                        "send",
                        if busy { "Queue message" } else { "Send" },
                        if can_send {
                            Tone::Primary
                        } else {
                            Tone::Filled
                        },
                    )
                    .rounded(px(8.))
                    .tab_stop(can_send)
                    .disabled(!can_send)
                    .when(!can_send, |button| {
                        button.tooltip(move |_, cx| {
                            cx.new(|_| TextTooltip(disabled_tip.clone())).into()
                        })
                    })
                    .on_activate(cx, move |_, _, cx| {
                        send_composer.update(cx, |composer, cx| composer.submit_from_button(cx));
                    }),
                )
            });
        let settings_row = row()
            .flex_wrap()
            .w_full()
            .min_w_0()
            .items_center()
            .justify_between()
            .gap_x_2()
            .gap_y_1()
            .pl(px(8.))
            .pr(px(6.))
            .pt(px(4.))
            .pb(px(6.))
            .text_size(px(12.))
            .text_color(rgb(colors().muted))
            .child(left_controls)
            .child(right_controls);
        let mut notices = Vec::new();
        if !hint.is_empty() {
            notices.push(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(rgb(colors().subtle))
                    .child(a11y_text("composer-hint", hint))
                    .into_any_element(),
            );
        }
        notices.extend(parts.notices);
        let message_box = column()
            .id("message-box")
            .track_focus(&box_focus)
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .rounded(px(SURFACE_CORNER))
            .shadow(message_box_shadows(focused))
            .border_1()
            .border_color(rgb(if focused {
                colors().subtle
            } else {
                colors().edge
            }))
            .bg(rgb(colors().panel))
            .on_drop(cx.listener(move |_, paths: &ExternalPaths, _, cx| {
                drop_composer.update(cx, |composer, cx| {
                    composer.drop_images(paths.paths().to_vec(), cx)
                });
            }))
            .child(composer)
            .child(settings_row)
            .when(!notices.is_empty(), |el| {
                el.child(
                    column()
                        .w_full()
                        .min_w_0()
                        .gap_1()
                        .px(px(14.))
                        .pb_2()
                        .children(notices),
                )
            });
        if self.host.thread.selected.is_some() && self.preferences.value.shows_thread_tabs() {
            column()
                .id("message-box-with-environment")
                .w_full()
                .max_w(px(CONTENT_WIDTH))
                .mx_auto()
                .gap_1()
                .child(message_box)
                .child(self.environment(cx))
        } else {
            message_box
        }
    }

    pub(super) fn thread_message_box_parts(&self, cx: &mut Context<Self>) -> MessageBoxParts {
        let Some(snapshot) = &self.host.thread.snapshot else {
            return MessageBoxParts::default();
        };
        let model = self.thread_model();
        let model_name = self.model_name(&snapshot.model);
        let reason = self.configure_reason();
        let levels = thinking_levels(model);
        let level = snapshot.thinking_level.unwrap_or(ThinkingLevel::Off);
        let usage = snapshot
            .usage
            .as_ref()
            .and_then(|u| ContextUsage::new(u, model));
        let harness = self.thread_harness_line();
        let pi = harness_of(snapshot) == HarnessId::Pi;
        // A harness thread runs under the harness's own mode; Plait's approvals are Pi's.
        let mode = harness.and_then(|line| line.mode.as_ref());
        let mut left = Vec::new();
        if let Some((label, tip)) = match harness {
            // With no Permissions chip to carry it, the logo's hover
            // text says what protects the checkout.
            Some(line) => Some((
                line.label.clone(),
                match line.protection.as_ref().filter(|_| mode.is_none()) {
                    Some(protection) => format!("{} {protection}", line.tip).into(),
                    None => line.tip.clone(),
                },
            )),
            None if pi => Some(("Pi".into(), "Pi, built into the host".into())),
            None => None,
        } {
            left.push(harness_mark(harness_of(snapshot), label, tip).into_any_element());
        }
        left.push(
            locked(
                box_control(button("thread-model", ""))
                    .max_w(px(240.))
                    .flex_shrink_1()
                    .aria_label(format!("Change thread model: {model_name}"))
                    .aria_description(reason.unwrap_or("Choose a model for the next run."))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(a11y_text("thread-model-name", model_name)),
                    )
                    .relative()
                    .child(self.anchor_probe("thread-model"))
                    .on_activate(cx, |this, window, cx| {
                        this.anchor_next("thread-model");
                        this.open_thread_models(window, cx)
                    }),
                reason,
            )
            .into_any_element(),
        );
        if !levels.is_empty() {
            left.push(
                locked(
                    box_control(button("thread-thinking", thinking_value(level)))
                        .aria_label(format!("Thinking level: {}", level.label()))
                        .aria_description(reason.unwrap_or("Choose how much this model thinks."))
                        .relative()
                        .child(self.anchor_probe("thread-thinking"))
                        .on_activate(cx, |this, window, cx| {
                            this.anchor_next("thread-thinking");
                            this.open_thinking(window, cx)
                        }),
                    reason,
                )
                .into_any_element(),
            );
        }
        if let Some(mode) = mode {
            if let Some(chip) = self.thread_permissions(mode).and_then(|permissions| {
                let full_access = is_full_access_mode(&permissions.current, &mode.label);
                self.permissions_chip("thread-permissions", permissions, cx)
                    .map(|chip| {
                        box_control(chip).when(full_access, |el| el.text_color(rgb(colors().amber)))
                    })
            }) {
                left.push(chip.into_any_element());
            }
        } else if pi {
            left.push(
                self.run_settings_button(snapshot.id.clone(), cx)
                    .into_any_element(),
            );
        }
        let mut right = Vec::new();
        if let Some(usage) = &usage {
            let detail = usage.detail.clone();
            let tint = match usage.pressure {
                ContextPressure::Normal => colors().muted,
                ContextPressure::High => colors().amber,
                ContextPressure::NearlyFull => colors().red,
            };
            right.push(
                box_control(button("thread-usage", usage.label.clone()))
                    .text_color(rgb(tint))
                    .aria_description(detail.clone())
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
                    .on_activate(cx, |this, _, cx| {
                        this.workspace.thread_controls.usage_open =
                            !this.workspace.thread_controls.usage_open;
                        cx.notify();
                    })
                    .into_any_element(),
            );
        }
        let mut notices = Vec::new();
        // What a mode allows and why a control is locked are hover text.
        // One thing is said in the open, in amber: the run is not under
        // the mode the control names.
        if let Some(differs) = mode
            .filter(|mode| mode.overridden)
            .map(|mode| mode.note.clone())
            .or_else(|| self.run_differs(snapshot).filter(|_| pi))
        {
            notices.push(
                tag("thread-mode-differs", differs, colors().amber)
                    .line_height(px(16.))
                    .into_any_element(),
            );
        }
        if self.workspace.thread_controls.usage_open
            && let Some(usage) = usage
        {
            notices.push(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors().muted))
                    .child(a11y_text("thread-usage-detail", usage.detail))
                    .into_any_element(),
            );
        }
        if let Some((id, note, failed)) = &self.workspace.thread_controls.note
            && id == &snapshot.id
        {
            notices.push(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(if *failed {
                        colors().amber
                    } else {
                        colors().muted
                    }))
                    .child(a11y_text(
                        "thread-control-note",
                        self.identity_recovery_message(note),
                    ))
                    .when(self.has_identity_recovery(note), |el| {
                        el.child(self.identity_recovery_button(cx))
                    })
                    .into_any_element(),
            );
        }
        if let Some(line) = self.worktree_context_line(cx) {
            notices.push(line);
        }
        if let Some(line) = self.continuation_control(cx) {
            notices.push(line);
        }
        if let Some(line) = self.admission_context(cx) {
            notices.push(line);
        }
        MessageBoxParts {
            left,
            right,
            notices,
        }
    }

    fn queued_text(&self, request: &host::Queued) -> Option<String> {
        let id = self.host.thread.selected.as_deref()?;
        self.host
            .submissions
            .accepted(id)
            .iter()
            .find(|p| p.request_id == request.request_id)
            .map(|p| p.text.clone())
    }

    pub(super) fn cancel_input(
        &mut self,
        request_id: String,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = &self.host.thread.snapshot else {
            return;
        };
        if !self.host.connection.connected
            || (restore
                && self.workspace.thread_controls.editing.get(&snapshot.id)
                    == Some(&self.host.connection.epoch))
            || self.workspace.thread_controls.cancelling.get(&request_id)
                == Some(&self.host.connection.epoch)
        {
            return;
        }
        let Some(queued) = snapshot.queued.iter().find(|q| q.request_id == request_id) else {
            return;
        };
        let images = self
            .host
            .submissions
            .accepted(&snapshot.id)
            .iter()
            .find(|pending| pending.request_id == request_id)
            .map(|pending| pending.images.clone())
            .unwrap_or_default();
        let text = restore.then(|| self.queued_text(queued)).flatten();
        if restore && text.is_none() {
            return;
        }
        let id = snapshot.id.clone();
        let checkout = snapshot.checkout.clone();
        let composer = self.workspace.composers.get(&id).cloned();
        let revision = composer.as_ref().map(|c| c.read(cx).saved_draft().1);
        if restore && composer.as_ref().is_none_or(|c| !c.read(cx).is_empty()) {
            return;
        }
        self.workspace
            .thread_controls
            .cancelling
            .insert(request_id.clone(), self.host.connection.epoch);
        if restore {
            self.workspace
                .thread_controls
                .editing
                .insert(id.clone(), self.host.connection.epoch);
        }
        self.sync_composer(cx);
        self.host_request::<host::ThreadReply>(Request::cancel_queued(id.clone(), request_id.clone()), window, cx, move |this, result, window, cx| {
            this.workspace.thread_controls.cancelling.remove(&request_id);
            this.workspace.thread_controls.editing.remove(&id);
            match result {
                Ok(reply) if reply.thread.id == id && reply.thread.checkout == checkout && !reply.thread.queued.iter().any(|q| q.request_id == request_id) => {
                    this.host.submissions.forget_accepted(&id, &request_id);
                    if let (Some(text), Some(composer), Some(revision)) = (text, composer, revision) {
                        let client = this.host_client();
                        let restored = composer.update(cx, |composer, cx|  { let restored=composer.restore_cancelled(&text,revision,cx);if restored { composer.restore_thread_images_with_client(images,id.clone(),client,cx); } restored });
                        if restored && this.host.thread.selected.as_ref() == Some(&id) { this.focus_composer(window, cx); }
                        else if !restored { cx.write_to_clipboard(ClipboardItem::new_string(text)); this.set_thread_notice(id.clone(), "Message cancelled. The draft changed, so its text was copied to the clipboard.".into(), false); }
                    }
                    this.capture_session(cx);
                }
                Ok(_) => this.set_thread_notice(id.clone(), "Cancellation was not confirmed. Reconnect to check the queue.".into(), true),
                Err(error) => this.set_thread_notice(id.clone(), format!("Message not cancelled: {error}"), true),
            }
        });
    }

    pub(super) fn queued_inputs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.host.protocol() < 3 {
            return None;
        }
        let snapshot = self.host.thread.snapshot.as_ref()?;
        let pending = self.host.submissions.all(&snapshot.id);
        if pending.is_empty() && snapshot.queued.is_empty() {
            return None;
        }
        let mut stack = column()
            .id("queued-messages")
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .max_h(px(144.))
            .overflow_y_scroll()
            .gap_1()
            .py_1()
            .text_size(px(12.));
        if !snapshot.status.busy() && !snapshot.queued.is_empty() {
            stack = stack.child(tag(
                "queued-held",
                "Queue held · next send resumes from the first message",
                colors().amber,
            ));
        }
        let draft_empty = self
            .workspace
            .composers
            .get(&snapshot.id)
            .is_some_and(|c| c.read(cx).is_empty());
        for queued in &snapshot.queued {
            let request_id = queued.request_id.clone();
            let cancel_id = request_id.clone();
            let edit_id = request_id.clone();
            let cancelling = self.workspace.thread_controls.cancelling.get(&request_id)
                == Some(&self.host.connection.epoch);
            let mode = match queued.mode {
                QueueMode::FollowUp => "Follow-up",
                QueueMode::Steer => "Steering",
            };
            let edit = draft_empty
                && self.queued_text(queued).is_some()
                && self.workspace.thread_controls.editing.get(&snapshot.id)
                    != Some(&self.host.connection.epoch);
            let image_count = self
                .host
                .submissions
                .accepted(&snapshot.id)
                .iter()
                .find(|p| p.request_id == queued.request_id)
                .map_or(0, |p| p.images.len());
            stack = stack.child(
                row()
                    .h(px(ROW_HEIGHT))
                    .px(px(9.))
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .rounded(px(CONTROL_CORNER))
                    .bg(rgba(colors().hover))
                    .text_color(rgb(colors().muted))
                    .child(tag(
                        SharedString::from(format!("queued-mode-{request_id}")),
                        mode,
                        colors().subtle,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(pending_description(&queued.text, image_count)),
                    )
                    .when(edit, |el| {
                        el.child(
                            button(
                                SharedString::from(format!("edit-queued-{request_id}")),
                                "Edit",
                            )
                            .h(px(22.))
                            .px(px(7.))
                            .rounded(px(ITEM_CORNER))
                            .aria_label("Cancel queued message into draft")
                            .disabled(cancelling || !self.host.connection.connected)
                            .on_activate(
                                cx,
                                move |this, window, cx| {
                                    this.cancel_input(edit_id.clone(), true, window, cx)
                                },
                            ),
                        )
                    })
                    .child(
                        button(
                            SharedString::from(format!("cancel-queued-{request_id}")),
                            if cancelling {
                                "Cancelling…"
                            } else {
                                "Cancel"
                            },
                        )
                        .h(px(22.))
                        .px(px(7.))
                        .rounded(px(ITEM_CORNER))
                        .aria_label("Cancel queued message")
                        .disabled(cancelling || !self.host.connection.connected)
                        .on_activate(cx, move |this, window, cx| {
                            this.cancel_input(cancel_id.clone(), false, window, cx)
                        }),
                    ),
            );
        }
        for pending in pending {
            let id = snapshot.id.clone();
            let request_id = pending.request_id.clone();
            let sending = self.host.submissions.is_sending(&request_id);
            let detail = self
                .host
                .submissions
                .error(&request_id)
                .unwrap_or("Retry uses the same request ID.");
            stack = stack.child(
                row()
                    .h(px(ROW_HEIGHT))
                    .px(px(9.))
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .rounded(px(CONTROL_CORNER))
                    .bg(rgba(colors().hover))
                    .text_color(rgb(colors().muted))
                    .child(
                        button(
                            SharedString::from(format!("pending-{request_id}")),
                            if sending && !pending.images.is_empty() {
                                "Uploading"
                            } else if sending {
                                "Sending"
                            } else {
                                "Unconfirmed"
                            },
                        )
                        .h(px(22.))
                        .px(px(7.))
                        .rounded(px(ITEM_CORNER))
                        .text_color(rgb(if sending {
                            colors().blue
                        } else {
                            colors().amber
                        }))
                        .tooltip({
                            let detail = if sending {
                                "Sending this message and its images to the host.".to_owned()
                            } else {
                                detail.to_owned()
                            };
                            move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into()
                        }),
                    )
                    .child(
                        div()
                            .when(pending.text.is_empty(), |el| el.flex_none())
                            .when(!pending.text.is_empty(), |el| el.flex_1())
                            .min_w_0()
                            .truncate()
                            .child(pending_description(&pending.text, pending.images.len())),
                    )
                    .child(
                        button(
                            SharedString::from(format!("retry-{request_id}")),
                            "Retry safely",
                        )
                        .h(px(22.))
                        .px(px(7.))
                        .rounded(px(ITEM_CORNER))
                        .aria_description(detail.to_owned())
                        .disabled(
                            sending || !self.host.connection.connected || !self.session.ready(),
                        )
                        .on_activate(cx, move |this, window, cx| {
                            this.retry_submission_id(&id, &request_id, window, cx)
                        }),
                    ),
            );
        }
        Some(stack.into_any_element())
    }
}

/// A control that cannot be used now, such as the model during a run. It
/// keeps its place and stays readable, in hint color, and hover says why.
/// Pressing it still answers: its action reports the same reason.
pub(super) fn locked(control: Control, reason: Option<&'static str>) -> Control {
    let Some(reason) = reason else {
        return control;
    };
    control
        .text_color(rgb(colors().subtle))
        .cursor_default()
        .a11y_synthetic_children(|builder| {
            builder.parent_node().set_disabled();
        })
        .tooltip(move |_, cx| cx.new(|_| TextTooltip(reason.to_owned())).into())
}

impl Plait {
    /// The open thread's run is under another approval mode than its control
    /// names: the mode was changed while the run was under way.
    fn run_differs(&self, snapshot: &host::Snapshot) -> Option<SharedString> {
        if !snapshot.status.busy() {
            return None;
        }
        let chosen = self
            .thread_approval(&snapshot.id)
            .unwrap_or_else(|| self.default_approval());
        run_mode_note(snapshot.approval_mode, chosen).map(Into::into)
    }

    /// Opens the run-settings menu. The label is this thread's approval mode
    /// and nothing more: the command policy belongs to the project, and its
    /// own settings page says what it is.
    fn run_settings_button(&self, id: String, cx: &mut Context<Self>) -> Control {
        let own = self.thread_approval(&id);
        let label = approval_label(own.unwrap_or_else(|| self.default_approval()));
        let tip = approval_tip(own.is_some());
        box_control(button("thread-run-settings", ""))
            .min_w_0()
            .flex_shrink_1()
            .aria_label(format!("Run settings: {label}"))
            .aria_description(tip)
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(tip.into())).into())
            .child(div().min_w_0().truncate().child(label))
            .child(icon("down").size(px(12.)))
            .relative()
            .child(self.anchor_probe("thread-run-settings"))
            .on_activate(cx, |this, window, cx| {
                this.anchor_next("thread-run-settings");
                this.open_run_settings(window, cx)
            })
    }

    /// This thread's own approval choice, if it has one.
    pub(super) fn thread_approval(&self, id: &str) -> Option<ApprovalMode> {
        if let Some(chosen) = self.workspace.thread_controls.approvals.get(id) {
            return *chosen;
        }
        let summary = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)?;
        self.saved_thread(summary)?.approval
    }

    /// Give a thread its own approval mode, or `None` to follow the default.
    pub(super) fn set_thread_approval(
        &mut self,
        id: String,
        mode: Option<ApprovalMode>,
        cx: &mut Context<Self>,
    ) {
        self.workspace.thread_controls.approvals.insert(id, mode);
        self.capture_session(cx);
        cx.notify();
    }

    /// The menu under the message box for how this thread runs: only this
    /// thread's approval mode. The project's command policy has its own page.
    pub(super) fn open_run_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread_id) = self.host.thread.selected.clone() else {
            return;
        };
        let chosen = self.thread_approval(&thread_id);
        let default = self.default_approval();
        let protocol = self.host.protocol();
        let approvals = "Approvals for this thread";
        let choice = |key: &str, title: String, detail: &str, mode: Option<ApprovalMode>| {
            let item = Item::new(
                key.to_owned(),
                title,
                Command::Approval {
                    thread_id: thread_id.clone(),
                    mode,
                },
            )
            .section(approvals)
            .detail(detail.to_owned())
            .enabled(protocol >= 2);
            if chosen == mode {
                item.badge("Current", colors().blue)
            } else {
                item
            }
        };
        let current = match chosen {
            None => "approval-default",
            Some(ApprovalMode::Ask) => "approval-ask",
            Some(ApprovalMode::Auto) => "approval-auto",
        };
        let items = vec![
            choice(
                "approval-default",
                format!("Use default ({})", approval_label(default)),
                "Follow Settings → New threads.",
                None,
            ),
            choice(
                "approval-ask",
                approval_label(ApprovalMode::Ask).into(),
                "Each edit and command waits for Allow once.",
                Some(ApprovalMode::Ask),
            ),
            choice(
                "approval-auto",
                approval_label(ApprovalMode::Auto).into(),
                "Edits and commands run without asking.",
                Some(ApprovalMode::Auto),
            ),
        ];
        let palette = cx.new(|cx| {
            Palette::new(
                Options {
                    name: "Thread run settings".into(),
                    placeholder: "Choose how this thread runs…".into(),
                    empty: "No run settings.".into(),
                    initial: Some(current.into()),
                    mark: None,
                    aside: None,
                    note: Some(
                        if protocol < 2 {
                            "This host asks before every edit and command. Update the host to choose."
                        } else {
                            "Applies from your next message. A running task keeps its settings."
                        }
                        .into(),
                    ),
                },
                items,
                window,
                cx,
            )
            .picker()
        });
        self.present(palette, None, window, cx);
    }
}

impl Plait {
    /// Why the open harness thread's permissions cannot change now. A run does not stop it.
    fn mode_reason(&self) -> Option<&'static str> {
        compose::mode_reason(
            self.host.protocol(),
            self.host.connection.connected,
            self.host.thread.snapshot.is_some(),
            self.mode_pending().is_some(),
        )
    }

    /// The mode sent for the open thread and not yet answered.
    fn mode_pending(&self) -> Option<&str> {
        let request = self.workspace.thread_controls.mode.as_ref()?;
        (request.epoch == self.host.connection.epoch
            && self.host.thread.selected.as_ref() == Some(&request.thread))
        .then_some(request.mode.as_str())
    }

    /// The open harness thread's Permissions chip and menu. While a change is
    /// saving, the chip shows where it is going.
    fn thread_permissions(&self, line: &ModeLine) -> Option<Permissions> {
        let snapshot = self.host.thread.snapshot.as_ref()?;
        let (thread_id, epoch) = (snapshot.id.clone(), self.host.connection.epoch);
        Some(Permissions {
            harness: harness_of(snapshot),
            current: self
                .mode_pending()
                .map_or_else(|| line.id.clone(), str::to_owned),
            blocked: self.mode_reason(),
            tip: line.tip.clone(),
            note: if line.overridden {
                line.note.clone()
            } else {
                line.applies.clone()
            },
            on_select: Rc::new(move |this, mode, window, cx| {
                this.choose_thread_mode(thread_id.clone(), epoch, mode, window, cx)
            }),
        })
    }

    /// Opens the open harness thread's Permissions menu, as its chip does. For scenes.
    #[cfg(test)]
    pub(super) fn open_thread_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let permissions = self
            .thread_harness_line()
            .and_then(|line| line.mode.as_ref())
            .and_then(|line| self.thread_permissions(line));
        if let Some(permissions) = permissions {
            self.open_permissions(&permissions, window, cx);
        }
    }

    /// A mode picked from the Permissions menu, for the thread it was opened on.
    pub(super) fn choose_thread_mode(
        &mut self,
        thread_id: String,
        epoch: u64,
        mode: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if epoch != self.host.connection.epoch
            || self.host.thread.selected.as_ref() != Some(&thread_id)
        {
            return;
        }
        let unchanged = self
            .thread_harness_line()
            .and_then(|line| line.mode.as_ref())
            .is_some_and(|line| line.id == mode);
        if unchanged {
            return;
        }
        let Some(request) = self.begin_thread_mode(mode) else {
            return;
        };
        self.host_request::<host::ThreadReply>(
            Request::set_thread_mode(request.thread.clone(), request.mode.clone()),
            window,
            cx,
            move |this, result, _, cx| this.finish_thread_mode(request, result, cx),
        );
        cx.notify();
    }

    /// Record a mode change for the open thread, if it can be sent now.
    pub(super) fn begin_thread_mode(&mut self, mode: String) -> Option<ModeRequest> {
        if self.mode_reason().is_some() {
            return None;
        }
        let snapshot = self.host.thread.snapshot.as_ref()?;
        // Only a mode the harness listed.
        self.harness_row(harness_of(snapshot))?.mode(&mode)?;
        self.workspace.thread_controls.serial += 1;
        let request = ModeRequest {
            thread: snapshot.id.clone(),
            checkout: snapshot.checkout.clone(),
            serial: self.workspace.thread_controls.serial,
            epoch: self.host.connection.epoch,
            watch_epoch: self.host.thread.watch_epoch,
            mode,
        };
        self.workspace.thread_controls.mode = Some(request.clone());
        // The chip shows the new choice; only a failure needs words.
        self.workspace.thread_controls.note = None;
        Some(request)
    }

    /// The host answered `request`. A reply for a thread no longer open, or
    /// one a later change superseded, changes nothing.
    pub(super) fn finish_thread_mode(
        &mut self,
        request: ModeRequest,
        result: Result<host::ThreadReply, host::Error>,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.thread_controls.mode.as_ref() != Some(&request) {
            return;
        }
        self.workspace.thread_controls.mode = None;
        if !self
            .host
            .accepts_watch(request.epoch, request.watch_epoch, &request.thread)
        {
            return;
        }
        match result {
            Ok(reply)
                if reply.thread.id == request.thread
                    && reply.thread.checkout == request.checkout
                    && reply
                        .thread
                        .harness
                        .as_ref()
                        .and_then(|harness| harness.mode.as_deref())
                        == Some(request.mode.as_str()) =>
            {
                if let Some(snapshot) = self
                    .host
                    .thread
                    .snapshot
                    .as_mut()
                    .filter(|snapshot| snapshot.id == request.thread)
                {
                    // The watch brings the same change, with the old report cleared as the
                    // host clears it; show it now instead of the old mode until then.
                    if let Some(harness) = Arc::make_mut(snapshot).harness.as_mut() {
                        harness.mode = Some(request.mode.clone());
                        harness.reported = None;
                    }
                }
            }
            Ok(_) => self.set_thread_notice(
                request.thread,
                "Permissions not confirmed: the host returned others. Reconnect to check them."
                    .into(),
                true,
            ),
            Err(error) => self.set_thread_notice(
                request.thread,
                format!("Permissions not changed: {error}"),
                true,
            ),
        }
        self.sync_composer(cx);
        cx.notify();
    }
}

/// Settings-row controls inside the message box: 24 high, 6 corners, hover fill.
fn box_control(control: Control) -> Control {
    control
        .h(px(24.))
        .rounded(px(ITEM_CORNER))
        .px(px(7.))
        .font_weight(FontWeight::NORMAL)
}

/// Box shadow at rest; when focused, adds a 3-point ring in the focus color at 13%.
fn message_box_shadows(focused: bool) -> Vec<BoxShadow> {
    let mut shadows = box_shadow();
    if focused {
        shadows.push(
            BoxShadow::new(px(0.), px(0.), rgba((colors().blue << 8) | 0x21).into())
                .spread_radius(px(3.)),
        );
    }
    shadows
}

fn is_full_access_mode(id: &str, label: &str) -> bool {
    id == "full_access" || label.eq_ignore_ascii_case("Full access")
}

/// The harness's logo where the composer row starts; its name and version are
/// the hover text and what assistive technology reads. Without a logo, the name.
fn harness_mark(id: HarnessId, label: SharedString, tip: SharedString) -> Control {
    let hover = tip.clone();
    div()
        .id("thread-harness")
        .flex_shrink_0()
        .h(px(24.))
        .px_1()
        .flex()
        .items_center()
        .role(Role::Image)
        .aria_label(label.clone())
        .aria_description(tip)
        .audited()
        .tooltip(move |_, cx| cx.new(|_| TextTooltip(hover.to_string())).into())
        .map(|el| match harness_logo(id) {
            Some(logo) => el.child(logo.text_color(rgb(colors().muted))),
            None => el.child(label),
        })
}

pub(super) fn pending_description(text: &str, images: usize) -> String {
    let count = match images {
        0 => return text.into(),
        1 => "1 image".into(),
        n => format!("{n} images"),
    };
    if text.is_empty() {
        count
    } else {
        format!("{text} · {count}")
    }
}
