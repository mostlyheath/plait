//! Desktop form modal for creating an automation (`docs/AUTOMATIONS.md` "Making one").
//!
//! Opens from **New automation…** in the Automations sidebar section, from
//! **Make automation from this thread** in a thread's `⋯` menu, or from the
//! command palette. All host requests, policy lookups, local time zone
//! detection, and request-ID persistence run on the background executor.

use super::{
    Plait, actions,
    commands::Command,
    composer::{Composer, ComposerEvent},
    harnesses::harness_of,
    model_picker::ModelPurpose,
    new_thread::nearest_level,
    palette::{Item, Options, Palette},
    permissions::{Permissions, menu_chip},
    text_field::{TextField, TextFieldEvent},
    theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_client::{
    automation_form::{
        AutomationDraftInput, POLICY_UNAVAILABLE_ERROR, ScheduleKind, TargetKind, WEEKDAY_LABELS,
        detect_local_time_zone, preview_schedule, today_date_in_time_zone, today_utc_date,
        validate_and_build_with_policy, validate_fields, validate_time_zone,
    },
    automations::schedule_with_zone,
    compose::{approval_label, thinking_levels, thinking_value},
    host::{
        self, ApprovalMode, MessageRole, Model, ModelChoice, ThinkingLevel,
        automations::{
            CommandPolicy as AutomationCommandPolicy, CreatedAutomation, Definition, DetailReply,
            ExistingCheck, Summary,
        },
        harness::HarnessId,
        policy::{CommandPolicy as HostCommandPolicy, PolicyReply, Request as PolicyRequest},
    },
    host_state::request_id as new_request_id,
    machines::MachineId,
};
use std::{path::PathBuf, rc::Rc};

const UNCONFIRMED_RECONNECT_MESSAGE: &str =
    "Plait couldn't confirm this automation was created. Retry is safe.";
const DUPLICATE_POSSIBLE_WARNING: &str = "Plait couldn't verify whether the previous attempt created an automation. Discarding it now may create a duplicate.";

/// One counter for every form's request generations, so a reply from a closed
/// form (or from before a machine switch) can never match a newer form's.
static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UnconfirmedAttempt {
    pub(super) request_id: String,
    pub(super) definition: Definition,
    pub(super) caller_thread_id: Option<String>,
    pub(super) discard_warning: bool,
}

pub(super) struct AutomationForm {
    /// Identifies this opened form; async replies check it before touching the form.
    token: u64,
    pub(super) focus: FocusHandle,
    restore: Option<FocusHandle>,
    pub(super) scroll: ScrollHandle,
    pub(super) name: Entity<TextField>,
    pub(super) instructions: Entity<Composer>,
    pub(super) target_kind: TargetKind,
    pub(super) project_checkout: PathBuf,
    pub(super) directory: Entity<TextField>,
    pub(super) schedule_kind: ScheduleKind,
    pub(super) hourly_minute: Entity<TextField>,
    pub(super) time: Entity<TextField>,
    pub(super) once_date: Entity<TextField>,
    once_date_edited: bool,
    default_once_date: Option<String>,
    once_date_generation: u64,
    pub(super) weekly_days: Vec<u8>,
    pub(super) time_zone: Entity<TextField>,
    time_zone_edited: bool,
    detected_time_zone: Option<String>,
    pub(super) harness: HarnessId,
    pub(super) model: Option<ModelChoice>,
    pub(super) mode: Option<String>,
    pub(super) thinking: Option<ThinkingLevel>,
    pub(super) approval: ApprovalMode,
    mode_edited: bool,
    approval_edited: bool,
    thinking_edited: bool,
    pub(super) command_policy: Option<HostCommandPolicy>,
    policy_path: Option<PathBuf>,
    policy_generation: u64,
    pub(super) catch_up: Entity<TextField>,
    pub(super) caller_thread_id: Option<String>,
    pub(super) source_thread_title: Option<String>,
    instructions_edited: bool,
    prefilled_instructions: Option<String>,
    prefill_applied: bool,
    prefill_generation: u64,
    /// The single creation request ID bound to this open form (replaced only
    /// when discarding an unconfirmed attempt or after a host conflict).
    pub(super) request_id: String,
    /// Exact persisted attempt that did not receive a definitive outcome yet.
    pub(super) unconfirmed_attempt: Option<UnconfirmedAttempt>,
    pub(super) submitted_once: bool,
    pub(super) submitting: bool,
    submit_generation: u64,
    /// `(message, is_warning)` — amber when unconfirmed/warning, red on definitive refusal.
    pub(super) submit_error: Option<(String, bool)>,
    _subscriptions: Vec<Subscription>,
}

fn to_automation_policy(policy: HostCommandPolicy) -> AutomationCommandPolicy {
    match policy {
        HostCommandPolicy::Sandboxed => AutomationCommandPolicy::Sandboxed,
        HostCommandPolicy::Trusted => AutomationCommandPolicy::Trusted,
    }
}

fn field_label(id: &'static str, text: &'static str) -> Div {
    div()
        .text_size(px(TYPE_CONTROL_SIZE))
        .line_height(px(TYPE_CONTROL_LINE))
        .font_weight(TYPE_MEDIUM)
        .text_color(rgb(colors().muted))
        .child(a11y_text(id, text))
}

fn field_hint(id: &'static str, text: impl Into<SharedString>) -> Div {
    tag(id, text, colors().subtle).line_height(px(16.))
}

fn field_error(id: &'static str, text: impl Into<SharedString>) -> Control {
    tag(id, text, colors().red)
        .id((id, 1usize))
        .role(Role::Alert)
        .line_height(px(16.))
        .audited()
}

fn modal_action_allowed(id: &str) -> bool {
    matches!(
        id,
        "app.quit" | "window.minimize" | "focus.next" | "focus.previous"
    )
}

impl Plait {
    /// Opens an empty automation creation form for the current machine.
    #[cfg(test)]
    pub(super) fn automation_form_guard_values(&self) -> Option<[u64; 5]> {
        self.automation_form.as_ref().map(|form| {
            [
                form.token,
                form.prefill_generation,
                form.policy_generation,
                form.once_date_generation,
                form.submit_generation,
            ]
        })
    }

    pub(super) fn open_automation_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.automations.supported {
            return;
        }
        let project = self.active_project().checkout.clone();
        let harness = self.default_harness();
        let model = self.default_model_for(harness);
        let mode = self.default_mode_for_harness(harness);
        let thinking = self.default_thinking_for_model(harness, model.as_ref());
        let approval = ApprovalMode::Auto;
        self.install_automation_form(
            FormSeed {
                name: String::new(),
                instructions: String::new(),
                project_checkout: project,
                harness,
                model,
                mode,
                thinking,
                approval,
                caller_thread_id: None,
                source_thread_title: None,
                needs_prefill: false,
            },
            window,
            cx,
        );
    }

    /// Opens the automation form prefilled from `thread_id`'s project, harness,
    /// model, mode, and last user message.
    pub(super) fn open_automation_form_from_thread(
        &mut self,
        thread_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.automations.supported {
            return;
        }
        let snapshot = self
            .host
            .thread
            .snapshot
            .as_ref()
            .filter(|snap| snap.id == thread_id)
            .cloned();
        let summary = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == thread_id)
            .cloned();
        let (checkout, harness, model, mode, thinking, approval, last_user, needs_fetch) =
            if let Some(snap) = snapshot {
                let harness = harness_of(&snap);
                let mode = if harness == HarnessId::Pi {
                    None
                } else {
                    snap.harness
                        .as_ref()
                        .and_then(|h| h.mode.clone())
                        .or_else(|| self.default_mode_for_harness(harness))
                };
                let last_user = snap
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.role == MessageRole::User && !m.text.trim().is_empty())
                    .map(|m| m.text.clone())
                    .unwrap_or_default();
                let approval = snap.approval_mode.unwrap_or(ApprovalMode::Auto);
                (
                    self.project_of(&snap.checkout).to_path_buf(),
                    harness,
                    Some(snap.model.clone()),
                    mode,
                    snap.thinking_level,
                    approval,
                    last_user,
                    false,
                )
            } else if let Some(summary) = summary {
                let harness = summary.harness.unwrap_or(HarnessId::Pi);
                let mode = self.default_mode_for_harness(harness);
                let thinking = self.default_thinking_for_model(harness, Some(&summary.model));
                (
                    self.project_of(&summary.checkout).to_path_buf(),
                    harness,
                    Some(summary.model),
                    mode,
                    thinking,
                    ApprovalMode::Auto,
                    String::new(),
                    true,
                )
            } else {
                return;
            };

        let title = self.thread_title(&thread_id);
        let source_title = (!title.trim().is_empty()).then_some(title);
        let seed_name = source_title.clone().unwrap_or_default();
        self.install_automation_form(
            FormSeed {
                name: seed_name,
                instructions: last_user,
                project_checkout: checkout,
                harness,
                model,
                mode,
                thinking,
                approval,
                caller_thread_id: Some(thread_id.clone()),
                source_thread_title: source_title,
                needs_prefill: needs_fetch,
            },
            window,
            cx,
        );
        if needs_fetch && self.host.connection.connected && !cfg!(test) {
            self.fetch_thread_prefill(thread_id, cx);
        }
    }

    fn install_automation_form(
        &mut self,
        seed: FormSeed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(false, window, cx);
        if self.settings_open {
            self.show_settings(false, window, cx);
        }
        self.navigation.menu = None;
        self.navigation.confirm_archive = None;
        self.dismiss_tab_controls();

        let restore = self
            .automation_form
            .as_ref()
            .and_then(|form| form.restore.clone())
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();

        let name = cx.new(|cx| {
            let mut field = TextField::new("Automation name", "e.g. Daily dependency audit", cx);
            if !seed.name.is_empty() {
                field.set_text(&seed.name, cx);
            }
            field
        });
        let instructions_text = seed.instructions;
        let prefilled_instructions = instructions_text.clone();
        let instructions = cx.new(|cx| {
            let mut editor = Composer::detached(cx);
            editor.set_enabled(true, None, cx);
            editor.set_purpose(
                "Automation instructions",
                "What the agent should do on each scheduled run…",
            );
            editor.set_plain("Automations take text instructions only.");
            if !instructions_text.is_empty() {
                editor.restore_draft(instructions_text, 0, None, cx);
            }
            editor
        });
        let directory =
            cx.new(|cx| TextField::new("Automation folder path", "/path/to/folder", cx));
        let hourly_minute = cx.new(|cx| {
            let mut field = TextField::new("Minute of the hour", "00", cx);
            field.set_text("00", cx);
            field
        });
        let time = cx.new(|cx| {
            let mut field = TextField::new("Run time", "09:00", cx);
            field.set_text("09:00", cx);
            field
        });
        let initial_once_date = today_utc_date();
        let once_date_seed = initial_once_date.clone();
        let once_date = cx.new(|cx| {
            let mut field = TextField::new("Run date", "YYYY-MM-DD", cx);
            field.set_text(&once_date_seed, cx);
            field
        });
        let time_zone = cx.new(|cx| {
            let mut field = TextField::new("Time zone", "America/New_York", cx);
            // Updated asynchronously from local zoneinfo once opened.
            field.set_text("UTC", cx);
            field
        });
        let catch_up = cx.new(|cx| {
            TextField::new(
                "Catch-up window",
                "Leave blank for default (e.g. 30m, 2h, or 0)",
                cx,
            )
        });

        let mut subscriptions = Vec::new();
        for field in [&name, &directory, &hourly_minute, &time, &catch_up] {
            let is_directory = *field == directory;
            subscriptions.push(cx.subscribe(field, move |this, _, _: &TextFieldEvent, cx| {
                if let Some(form) = &mut this.automation_form {
                    form.submit_error = None;
                }
                if is_directory {
                    this.invalidate_stale_automation_policy(cx);
                    this.fetch_automation_policy(cx);
                }
                cx.notify();
            }));
        }
        subscriptions.push(
            cx.subscribe(&once_date, |this, field, _: &TextFieldEvent, cx| {
                let text = field.read(cx).text().to_owned();
                if let Some(form) = &mut this.automation_form {
                    if form.default_once_date.as_deref() != Some(text.as_str()) {
                        form.once_date_edited = true;
                    }
                    form.submit_error = None;
                }
                cx.notify();
            }),
        );
        subscriptions.push(
            cx.subscribe(&time_zone, |this, field, _: &TextFieldEvent, cx| {
                let text = field.read(cx).text().to_owned();
                let sync_zone = if let Some(form) = &mut this.automation_form {
                    let is_detected = form.detected_time_zone.as_deref() == Some(text.as_str());
                    if !is_detected {
                        form.time_zone_edited = true;
                        form.detected_time_zone = None;
                    }
                    form.submit_error = None;
                    (!form.once_date_edited && !is_detected)
                        .then(|| validate_time_zone(&text).ok())
                        .flatten()
                } else {
                    None
                };
                if let Some(zone) = sync_zone {
                    this.sync_once_date_for_time_zone(zone, cx);
                }
                cx.notify();
            }),
        );
        subscriptions.push(cx.observe(&instructions, |this, editor, cx| {
            let text = editor.read(cx).saved_draft().0.to_owned();
            if let Some(form) = &mut this.automation_form {
                if form.prefilled_instructions.as_deref() != Some(text.as_str()) {
                    form.instructions_edited = true;
                }
                form.submit_error = None;
            }
            cx.notify();
        }));
        subscriptions.push(cx.subscribe_in(
            &instructions,
            window,
            |this, _, event: &ComposerEvent, window, cx| {
                if matches!(event, ComposerEvent::Submit { .. }) {
                    this.submit_automation_form(window, cx);
                }
            },
        ));
        subscriptions.push(cx.on_focus_out(&focus, window, |this, _, window, cx| {
            if let Some(form) = &this.automation_form
                && this.overlay.is_none()
                && this.folder_prompt.is_none()
                && window.is_window_active()
                && !form.focus.contains_focused(window, cx)
            {
                window.focus(&form.focus, cx);
            }
        }));

        let initial_policy = if seed.project_checkout == self.host.creation.checkout {
            self.command_policy()
        } else {
            None
        };
        let initial_policy_path = initial_policy.map(|_| seed.project_checkout.clone());

        let token = next_generation();
        self.automation_form = Some(AutomationForm {
            focus,
            restore,
            scroll: ScrollHandle::new(),
            name: name.clone(),
            instructions,
            target_kind: TargetKind::Project,
            project_checkout: seed.project_checkout,
            directory,
            schedule_kind: ScheduleKind::Weekdays,
            hourly_minute,
            time,
            once_date,
            once_date_edited: false,
            default_once_date: Some(initial_once_date),
            once_date_generation: 0,
            weekly_days: vec![1],
            time_zone,
            time_zone_edited: false,
            detected_time_zone: Some("UTC".into()),
            harness: seed.harness,
            model: seed.model,
            mode: seed.mode,
            thinking: seed.thinking,
            approval: seed.approval,
            mode_edited: false,
            approval_edited: false,
            thinking_edited: false,
            command_policy: initial_policy,
            policy_path: initial_policy_path,
            policy_generation: 0,
            catch_up,
            caller_thread_id: seed.caller_thread_id,
            source_thread_title: seed.source_thread_title,
            instructions_edited: false,
            prefilled_instructions: Some(prefilled_instructions),
            prefill_applied: !seed.needs_prefill,
            prefill_generation: 0,
            token,
            request_id: new_request_id(),
            unconfirmed_attempt: None,
            submitted_once: false,
            submitting: false,
            submit_generation: 0,
            submit_error: None,
            _subscriptions: subscriptions,
        });

        self.cover_workspace(window, cx);
        window.focus(&name.focus_handle(cx), cx);

        if !cfg!(test) {
            self.detect_automation_time_zone(cx);
            self.fetch_automation_policy(cx);
        }
        self.restore_unconfirmed_automation_attempt(cx);
        cx.notify();
    }

    /// An unconfirmed creation attempt on this machine is never silently
    /// forgotten: a newly opened form adopts it, offering Retry (same ID and
    /// definition) or Discard before anything new is sent.
    pub(super) fn restore_unconfirmed_automation_attempt(&mut self, cx: &mut Context<Self>) {
        let (Some(journal), Some(form)) = (self.session.journal.clone(), &self.automation_form)
        else {
            return;
        };
        let token = form.token;
        let machine = self.machines.active.clone();
        let task = cx
            .background_executor()
            .spawn(async move { journal.unresolved_automation_creates(&machine) });
        cx.spawn(async move |this, cx| {
            let Ok(mut pending) = task.await else {
                return;
            };
            let Some((request_id, entry)) = pending.pop() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let Some(form) = &mut this.automation_form else {
                    return;
                };
                if form.token != token || form.submitting || form.unconfirmed_attempt.is_some() {
                    return;
                }
                let name = entry.definition.name.clone();
                form.request_id = request_id.clone();
                form.unconfirmed_attempt = Some(UnconfirmedAttempt {
                    request_id,
                    definition: entry.definition,
                    caller_thread_id: entry.caller_thread_id,
                    discard_warning: false,
                });
                form.submitted_once = true;
                form.submit_error = Some((
                    format!(
                        "An automation you created may not have been confirmed: {name}. Retry is safe."
                    ),
                    true,
                ));
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn close_automation_form(
        &mut self,
        restore_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(false, window, cx);
        self.remote_browser_for_automation = false;
        let Some(form) = self.automation_form.take() else {
            return;
        };
        self.cover_workspace(window, cx);
        if restore_focus {
            let restore = form.restore.filter(|focus| focus.is_rendered(window));
            window.focus(restore.as_ref().unwrap_or(&self.focus), cx);
        }
        cx.notify();
    }

    fn detect_automation_time_zone(&mut self, cx: &mut Context<Self>) {
        let Some(token) = self.automation_form.as_ref().map(|form| form.token) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let zone = detect_local_time_zone();
            let today = today_date_in_time_zone(&zone);
            (zone, today)
        });
        cx.spawn(async move |this, cx| {
            let (zone, today) = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(form) = &mut this.automation_form else {
                    return;
                };
                if form.token != token {
                    return;
                }
                if !form.time_zone_edited {
                    form.detected_time_zone = Some(zone.clone());
                    let field = form.time_zone.clone();
                    field.update(cx, |field, cx| field.set_text(&zone, cx));
                    if !form.once_date_edited
                        && let Some(date) = today
                    {
                        form.default_once_date = Some(date.clone());
                        let date_field = form.once_date.clone();
                        date_field.update(cx, |field, cx| field.set_text(&date, cx));
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn sync_once_date_for_time_zone(&mut self, zone: String, cx: &mut Context<Self>) {
        let Some(form) = &mut self.automation_form else {
            return;
        };
        if form.once_date_edited {
            return;
        }
        form.once_date_generation = next_generation();
        let generation = form.once_date_generation;
        let task = cx
            .background_executor()
            .spawn(async move { today_date_in_time_zone(&zone) });
        cx.spawn(async move |this, cx| {
            let today = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(date) = today else {
                    return;
                };
                let Some(form) = &mut this.automation_form else {
                    return;
                };
                if form.once_date_generation != generation || form.once_date_edited {
                    return;
                }
                form.default_once_date = Some(date.clone());
                let field = form.once_date.clone();
                field.update(cx, |field, cx| field.set_text(&date, cx));
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_thread_prefill(&mut self, thread_id: String, cx: &mut Context<Self>) {
        let Some(form) = &mut self.automation_form else {
            return;
        };
        form.prefill_generation = next_generation();
        let generation = form.prefill_generation;
        let epoch = self.host.connection.epoch;
        let client = self.host_client();
        let protocol = self.host.protocol();
        let task = cx.background_executor().spawn(async move {
            let mut watch = client.watch(thread_id, protocol)?;
            watch.next_snapshot()
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch != epoch {
                    return;
                }
                let Ok(snapshot) = result else {
                    return;
                };
                if this
                    .automation_form
                    .as_ref()
                    .is_none_or(|form| form.prefill_generation != generation)
                {
                    return;
                }
                this.apply_thread_prefill(&snapshot, cx);
            });
        })
        .detach();
    }

    /// Applies a thread snapshot's settings at most once when the form opens.
    /// Never overwrites mode, approval, or thinking after the person changed
    /// them, and only sets them when the snapshot's harness matches `form.harness`.
    pub(super) fn apply_thread_prefill(
        &mut self,
        snapshot: &host::Snapshot,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.automation_form else {
            return;
        };
        if form.prefill_applied {
            return;
        }
        form.prefill_applied = true;
        let harness = harness_of(snapshot);
        if harness == form.harness {
            if !form.mode_edited
                && harness != HarnessId::Pi
                && let Some(mode) = snapshot.harness.as_ref().and_then(|h| h.mode.clone())
            {
                form.mode = Some(mode);
            }
            if !form.approval_edited
                && let Some(approval) = snapshot.approval_mode
            {
                form.approval = approval;
            }
            if !form.thinking_edited && snapshot.thinking_level.is_some() {
                form.thinking = snapshot.thinking_level;
            }
        }
        if !form.instructions_edited
            && let Some(last_user) = snapshot
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::User && !m.text.trim().is_empty())
        {
            let text = last_user.text.clone();
            form.prefilled_instructions = Some(text.clone());
            let editor = form.instructions.clone();
            editor.update(cx, |editor, cx| editor.restore_draft(text, 0, None, cx));
        }
        cx.notify();
    }

    fn current_target_path(&self, cx: &App) -> Option<PathBuf> {
        let form = self.automation_form.as_ref()?;
        let path = match form.target_kind {
            TargetKind::Project | TargetKind::Worktree => form.project_checkout.clone(),
            TargetKind::Directory => PathBuf::from(form.directory.read(cx).text().trim()),
        };
        path.is_absolute().then_some(path)
    }

    fn invalidate_stale_automation_policy(&mut self, cx: &App) {
        let target = self.current_target_path(cx);
        if let Some(form) = &mut self.automation_form
            && form.policy_path != target
        {
            form.command_policy = None;
            form.policy_path = None;
        }
    }

    pub(super) fn fetch_automation_policy(&mut self, cx: &mut Context<Self>) {
        if !self.host.connection.connected || cfg!(test) {
            return;
        }
        let Some(target_path) = self.current_target_path(cx) else {
            if let Some(form) = &mut self.automation_form {
                form.command_policy = None;
                form.policy_path = None;
            }
            return;
        };
        let Some(form) = &mut self.automation_form else {
            return;
        };
        if form.policy_path.as_ref() == Some(&target_path) && form.command_policy.is_some() {
            return;
        }
        form.policy_generation = next_generation();
        let generation = form.policy_generation;
        form.command_policy = None;
        form.policy_path = Some(target_path.clone());
        let epoch = self.host.connection.epoch;
        let client = self.host_client();
        let request_path = target_path.clone();
        let task = cx.background_executor().spawn(async move {
            client.send::<_, PolicyReply>(&PolicyRequest::CommandPolicy {
                checkout: request_path,
                identity: None,
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch != epoch {
                    return;
                }
                let Some(form) = &mut this.automation_form else {
                    return;
                };
                if form.policy_generation != generation {
                    return;
                }
                if let Ok(reply) = result {
                    form.command_policy = Some(reply.policy);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn default_mode_for_harness(&self, harness: HarnessId) -> Option<String> {
        if harness == HarnessId::Pi {
            return None;
        }
        self.harness_row(harness)
            .and_then(|row| row.default_mode.clone())
    }

    fn automation_model_entry(&self, harness: HarnessId, choice: &ModelChoice) -> Option<&Model> {
        if harness != HarnessId::Pi {
            return self.harness_model(harness, choice);
        }
        self.host
            .connection
            .status
            .as_ref()?
            .models
            .iter()
            .find(|model| model.choice() == *choice)
    }

    fn default_thinking_for_model(
        &self,
        harness: HarnessId,
        choice: Option<&ModelChoice>,
    ) -> Option<ThinkingLevel> {
        let levels = thinking_levels(choice.and_then(|c| self.automation_model_entry(harness, c)));
        nearest_level(levels, self.preferences.value.default_thinking())
    }

    pub(super) fn automation_form_harness(&self) -> HarnessId {
        self.automation_form
            .as_ref()
            .map_or_else(|| self.default_harness(), |form| form.harness)
    }

    pub(super) fn automation_form_model(&self) -> Option<ModelChoice> {
        self.automation_form
            .as_ref()
            .and_then(|form| form.model.clone())
    }

    fn automation_thinking_levels(&self) -> &[ThinkingLevel] {
        let Some(form) = &self.automation_form else {
            return &[];
        };
        let Some(choice) = form.model.as_ref() else {
            return &[];
        };
        thinking_levels(self.automation_model_entry(form.harness, choice))
    }

    pub(super) fn choose_automation_project(&mut self, checkout: PathBuf, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.automation_form {
            form.project_checkout = checkout;
            form.submit_error = None;
        }
        self.invalidate_stale_automation_policy(cx);
        self.fetch_automation_policy(cx);
        cx.notify();
    }

    pub(super) fn choose_automation_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.remote_browser_for_automation = false;
        let Some(field) = self
            .automation_form
            .as_ref()
            .map(|form| form.directory.clone())
        else {
            return;
        };
        let text = path.display().to_string();
        field.update(cx, |field, cx| field.set_text(&text, cx));
        if let Some(form) = &mut self.automation_form {
            form.target_kind = TargetKind::Directory;
            form.submit_error = None;
        }
        self.invalidate_stale_automation_policy(cx);
        self.fetch_automation_policy(cx);
        cx.notify();
    }

    pub(super) fn choose_automation_harness(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        let next_model = self.default_model_for(harness);
        let next_mode = self.default_mode_for_harness(harness);
        let next_thinking = self.default_thinking_for_model(harness, next_model.as_ref());
        if let Some(form) = &mut self.automation_form {
            form.harness = harness;
            form.model = next_model;
            form.mode = next_mode;
            form.thinking = next_thinking;
            form.mode_edited = true;
            form.thinking_edited = true;
            form.submit_error = None;
        }
        cx.notify();
    }

    pub(super) fn choose_automation_model(
        &mut self,
        harness: HarnessId,
        choice: ModelChoice,
        cx: &mut Context<Self>,
    ) {
        let levels = thinking_levels(self.automation_model_entry(harness, &choice));
        let previous = self
            .automation_form
            .as_ref()
            .and_then(|form| form.thinking)
            .unwrap_or_else(|| self.preferences.value.default_thinking());
        let next_thinking = nearest_level(levels, previous);
        let next_mode = if harness == HarnessId::Pi {
            None
        } else {
            self.automation_form
                .as_ref()
                .and_then(|form| form.mode.clone())
                .or_else(|| self.default_mode_for_harness(harness))
        };
        if let Some(form) = &mut self.automation_form {
            if form.harness != harness {
                form.mode_edited = true;
            }
            form.harness = harness;
            form.model = Some(choice);
            form.mode = next_mode;
            form.thinking = next_thinking;
            form.thinking_edited = true;
            form.submit_error = None;
        }
        cx.notify();
    }

    pub(super) fn choose_automation_mode(
        &mut self,
        harness: HarnessId,
        mode: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = &mut self.automation_form
            && form.harness == harness
        {
            form.mode = Some(mode);
            form.mode_edited = true;
            form.submit_error = None;
        }
        cx.notify();
    }

    pub(super) fn choose_automation_approval(
        &mut self,
        approval: ApprovalMode,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = &mut self.automation_form {
            form.approval = approval;
            form.approval_edited = true;
            form.submit_error = None;
        }
        cx.notify();
    }

    pub(super) fn choose_automation_thinking(
        &mut self,
        level: ThinkingLevel,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = &mut self.automation_form {
            form.thinking = Some(level);
            form.thinking_edited = true;
            form.submit_error = None;
        }
        cx.notify();
    }

    fn open_automation_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self
            .automation_form
            .as_ref()
            .filter(|form| !form.submitting)
            .map(|form| form.project_checkout.clone())
        else {
            return;
        };
        let items: Vec<Item> = self
            .state
            .projects
            .iter()
            .map(|project| {
                let missing = self.navigation.available.get(&project.checkout) == Some(&false);
                let item = Item::new(
                    format!("automation-project:{}", project.checkout.display()),
                    project.name.clone(),
                    Command::AutomationProject(project.checkout.clone()),
                )
                .detail(home_relative(&project.checkout))
                .section("Projects")
                .glyph(
                    "folder",
                    if missing {
                        colors().amber
                    } else {
                        colors().mint
                    },
                );
                if missing {
                    item.badge("Missing", colors().amber).enabled(false)
                } else if project.checkout == current {
                    item.badge("Current", colors().blue)
                } else {
                    item
                }
            })
            .collect();
        let options = Options {
            name: "Project for the automation".into(),
            placeholder: "Search projects…".into(),
            empty: "No projects open on this machine.".into(),
            initial: Some(format!("automation-project:{}", current.display()).into()),
            mark: None,
            aside: None,
            note: Some("The automation gets its own worktree from the project you choose.".into()),
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx).picker());
        self.present(palette, None, window, cx);
    }

    fn open_automation_folder_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.host_is_local() {
            self.open_remote_browser_for_automation(window, cx);
            return;
        }
        if self.folder_prompt.is_some() {
            return;
        }
        let origin = Rc::new(self.machines.active.clone());
        self.folder_prompt = Some(origin.clone());
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.machines.active != *origin
                    || !this
                        .folder_prompt
                        .as_ref()
                        .is_some_and(|open| Rc::ptr_eq(open, &origin))
                {
                    return;
                }
                this.folder_prompt = None;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.choose_automation_directory(path, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        if let Some(form) = &mut this.automation_form {
                            form.submit_error =
                                Some((format!("Cannot open folder picker: {error}"), false));
                        }
                    }
                    Err(error) => {
                        if let Some(form) = &mut this.automation_form {
                            form.submit_error = Some((
                                format!("Folder picker closed unexpectedly: {error}"),
                                false,
                            ));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_automation_harnesses(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.automation_form.as_ref().is_none_or(|f| f.submitting) {
            return;
        }
        let current = self.automation_form_harness();
        let items = super::harnesses::harness_items_for(
            self.harness_rows(),
            current,
            Command::AutomationHarness,
        );
        let note = if self.host.connection.connected && self.host.protocol() < 4 {
            "This host runs every thread in Pi. Update the host to choose another harness."
        } else {
            "An automation stays in the harness it starts in."
        };
        let options = Options {
            name: "Harness for the automation".into(),
            placeholder: "Search harnesses…".into(),
            empty: "No harness matches.".into(),
            initial: Some(format!("harness\u{1f}{}", current.label()).into()),
            mark: None,
            aside: None,
            note: Some(note.into()),
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx).picker());
        self.present(palette, None, window, cx);
    }

    fn open_automation_approvals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self
            .automation_form
            .as_ref()
            .filter(|form| !form.submitting)
            .map(|form| form.approval)
        else {
            return;
        };
        let section = "Approvals for scheduled runs";
        let item = |key: &str, title: &str, detail: &str, mode: ApprovalMode| {
            let item = Item::new(
                key.to_owned(),
                title.to_owned(),
                Command::AutomationApproval(mode),
            )
            .section(section)
            .detail(detail.to_owned());
            if current == mode {
                item.badge("Current", colors().blue)
            } else {
                item
            }
        };
        let items = vec![
            item(
                "automation-approval-auto",
                approval_label(ApprovalMode::Auto),
                "Scheduled runs edit and run commands without asking, under the project's command policy.",
                ApprovalMode::Auto,
            ),
            item(
                "automation-approval-ask",
                approval_label(ApprovalMode::Ask),
                "Scheduled runs pause for Allow once before each edit or command.",
                ApprovalMode::Ask,
            ),
        ];
        let initial = match current {
            ApprovalMode::Auto => "automation-approval-auto",
            ApprovalMode::Ask => "automation-approval-ask",
        };
        let options = Options {
            name: "Permissions".into(),
            placeholder: "Choose how scheduled runs ask for approval…".into(),
            empty: "No choices.".into(),
            initial: Some(initial.into()),
            mark: None,
            aside: None,
            note: Some(
                "Applies to scheduled runs and Run now. Turns you type in the thread keep your normal approval mode."
                    .into(),
            ),
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx).picker());
        self.present(palette, None, window, cx);
    }

    fn open_automation_thinking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.automation_form.as_ref().is_none_or(|f| f.submitting) {
            return;
        }
        let levels = self.automation_thinking_levels().to_vec();
        if levels.is_empty() {
            return;
        }
        let current = self.automation_form.as_ref().and_then(|form| form.thinking);
        self.present_thinking(
            &levels,
            current,
            "Applies to every scheduled run of this automation.",
            Command::AutomationThinking,
            window,
            cx,
        );
    }

    fn automation_form_input(&self, cx: &App) -> Option<AutomationDraftInput> {
        let form = self.automation_form.as_ref()?;
        Some(AutomationDraftInput {
            name: form.name.read(cx).text().to_owned(),
            instructions: form.instructions.read(cx).saved_draft().0.to_owned(),
            target_kind: form.target_kind,
            project_checkout: form.project_checkout.clone(),
            directory_path: form.directory.read(cx).text().to_owned(),
            schedule_kind: form.schedule_kind,
            hourly_minute: form.hourly_minute.read(cx).text().to_owned(),
            time: form.time.read(cx).text().to_owned(),
            once_date: form.once_date.read(cx).text().to_owned(),
            weekly_days: form.weekly_days.clone(),
            time_zone: form.time_zone.read(cx).text().to_owned(),
            harness: form.harness,
            model: form.model.clone(),
            mode: form.mode.clone(),
            thinking: form.thinking,
            approval: form.approval,
            command_policy: form.command_policy.map(to_automation_policy),
            catch_up: form.catch_up.read(cx).text().to_owned(),
        })
    }

    pub(super) fn form_edited_after_unconfirmed(&self, cx: &App) -> bool {
        let Some(form) = &self.automation_form else {
            return false;
        };
        let Some(unconfirmed) = &form.unconfirmed_attempt else {
            return false;
        };
        let Some(input) = self.automation_form_input(cx) else {
            return true;
        };
        // Use the unconfirmed attempt's policy only for comparison when no
        // policy is loaded yet; any target change still differs in `target`.
        let policy = input
            .command_policy
            .unwrap_or(unconfirmed.definition.command_policy);
        validate_and_build_with_policy(&input, policy).ok().as_ref()
            != Some(&unconfirmed.definition)
    }

    pub(super) fn submit_automation_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.automation_form.as_ref().is_none_or(|f| f.submitting) {
            return;
        }
        if self.form_edited_after_unconfirmed(cx) {
            // Require an explicit choice between retrying the previous attempt
            // and discarding it before sending anything.
            if let Some(form) = &mut self.automation_form {
                form.submitted_once = true;
            }
            cx.notify();
            return;
        }
        if self
            .automation_form
            .as_ref()
            .is_some_and(|form| form.unconfirmed_attempt.is_some())
        {
            self.retry_previous_automation_attempt(window, cx);
            return;
        }
        if !self.host.connection.connected {
            if let Some(form) = &mut self.automation_form {
                form.submitted_once = true;
                form.submit_error = Some((
                    "Reconnect to the host before creating an automation.".into(),
                    false,
                ));
            }
            cx.notify();
            return;
        }
        let Some(input) = self.automation_form_input(cx) else {
            return;
        };
        if let Some(form) = &mut self.automation_form {
            form.submitted_once = true;
            form.submit_error = None;
        }
        if !validate_fields(&input).is_empty() {
            cx.notify();
            return;
        }
        self.submit_draft_with_discard(input, None, window, cx);
    }

    /// Resends the exact persisted definition with the same creation request ID
    /// from an unconfirmed attempt.
    pub(super) fn retry_previous_automation_attempt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.automation_form.as_ref().is_none_or(|f| f.submitting) {
            return;
        }
        if !self.host.connection.connected {
            if let Some(form) = &mut self.automation_form {
                form.submit_error = Some((
                    "Reconnect to the host before creating an automation.".into(),
                    false,
                ));
            }
            cx.notify();
            return;
        }
        let Some(unconfirmed) = self
            .automation_form
            .as_ref()
            .and_then(|form| form.unconfirmed_attempt.clone())
        else {
            return;
        };
        self.send_automation_create(
            unconfirmed.request_id,
            SendSpec::Retry {
                definition: Box::new(unconfirmed.definition),
                caller_thread_id: unconfirmed.caller_thread_id,
            },
            window,
            cx,
        );
    }

    /// Handles "Discard it and create with my changes" after an unconfirmed
    /// attempt: asks the host once with the old ID first (via status/list);
    /// warns plainly if a duplicate is possible; creates with a fresh ID once
    /// confirmed absent or explicitly confirmed after warning.
    pub(super) fn discard_and_create_automation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.automation_form.as_ref().is_none_or(|f| f.submitting) {
            return;
        }
        if !self.host.connection.connected {
            if let Some(form) = &mut self.automation_form {
                form.submitted_once = true;
                form.submit_error = Some((
                    "Reconnect to the host before creating an automation.".into(),
                    false,
                ));
            }
            cx.notify();
            return;
        }
        let Some(input) = self.automation_form_input(cx) else {
            return;
        };
        if let Some(form) = &mut self.automation_form {
            form.submitted_once = true;
            form.submit_error = None;
        }
        if !validate_fields(&input).is_empty() {
            cx.notify();
            return;
        }
        if self.session.journal.is_none() {
            if let Some(form) = &mut self.automation_form {
                form.submit_error = Some((
                    "Desktop session storage is not ready yet. Try again in a moment.".into(),
                    false,
                ));
            }
            cx.notify();
            return;
        }
        let Some(unconfirmed) = self
            .automation_form
            .as_ref()
            .and_then(|form| form.unconfirmed_attempt.clone())
        else {
            self.submit_draft_with_discard(input, None, window, cx);
            return;
        };

        if unconfirmed.discard_warning {
            if let Some(form) = &mut self.automation_form {
                form.unconfirmed_attempt = None;
                form.request_id = new_request_id();
            }
            self.submit_draft_with_discard(input, Some(unconfirmed.request_id), window, cx);
            return;
        }

        let Some(form) = &mut self.automation_form else {
            return;
        };
        form.submitting = true;
        form.submit_generation = next_generation();
        let generation = form.submit_generation;
        let epoch = self.host.connection.epoch;
        let machine = self.machines.active.clone();
        let client = self.host_client();
        let old_request_id = unconfirmed.request_id;
        cx.notify();

        let check_id = old_request_id.clone();
        let task = cx
            .background_executor()
            .spawn(async move { host::automations::check_existing(&client, &check_id) });

        cx.spawn_in(window, async move |this, cx| {
            let check = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.machines.active != machine {
                    return;
                }
                let Some(form) = &mut this.automation_form else {
                    return;
                };
                if form.submit_generation != generation {
                    return;
                }
                if this.host.connection.epoch != epoch {
                    form.submitting = false;
                    if let Some(attempt) = &mut form.unconfirmed_attempt
                        && attempt.request_id == old_request_id
                    {
                        attempt.discard_warning = true;
                    }
                    cx.notify();
                    return;
                }
                match check {
                    ExistingCheck::Found { summary, detail } => {
                        this.settle_discard_found(
                            machine,
                            old_request_id,
                            epoch,
                            generation,
                            *summary,
                            detail.map(|d| *d),
                            cx,
                        );
                    }
                    ExistingCheck::NotFound => {
                        form.submitting = false;
                        form.unconfirmed_attempt = None;
                        form.request_id = new_request_id();
                        let fresh_input = this.automation_form_input(cx).unwrap_or(input);
                        this.submit_draft_with_discard(
                            fresh_input,
                            Some(old_request_id),
                            window,
                            cx,
                        );
                    }
                    ExistingCheck::Unknown => {
                        form.submitting = false;
                        if let Some(attempt) = &mut form.unconfirmed_attempt
                            && attempt.request_id == old_request_id
                        {
                            attempt.discard_warning = true;
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn submit_draft_with_discard(
        &mut self,
        input: AutomationDraftInput,
        discard_previous: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_path = self.current_target_path(cx);
        let Some((request_id, caller_thread_id, cached_policy)) =
            self.automation_form.as_ref().map(|form| {
                let policy = form
                    .command_policy
                    .filter(|_| form.policy_path.is_none() || form.policy_path == target_path);
                (
                    form.request_id.clone(),
                    form.caller_thread_id.clone(),
                    policy,
                )
            })
        else {
            return;
        };
        self.send_automation_create(
            request_id,
            SendSpec::Draft {
                input: Box::new(input),
                cached_policy,
                caller_thread_id,
                discard_previous,
            },
            window,
            cx,
        );
    }

    fn send_automation_create(
        &mut self,
        request_id: String,
        spec: SendSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(journal) = self.session.journal.clone() else {
            if let Some(form) = &mut self.automation_form {
                form.submit_error = Some((
                    "Desktop session storage is not ready yet. Try again in a moment.".into(),
                    false,
                ));
            }
            cx.notify();
            return;
        };
        let Some(form) = &mut self.automation_form else {
            return;
        };
        form.submitting = true;
        form.submit_generation = next_generation();
        form.submit_error = None;
        let generation = form.submit_generation;
        let epoch = self.host.connection.epoch;
        let machine = self.machines.active.clone();
        let client = self.host_client();
        cx.notify();

        let task_machine = machine.clone();
        let task = cx.background_executor().spawn(async move {
            let (definition, caller_thread_id, fetched_policy) = match spec {
                SendSpec::Draft {
                    input,
                    cached_policy,
                    caller_thread_id,
                    discard_previous,
                } => {
                    if let Some(old_id) = discard_previous {
                        let _ = journal.remove_automation_create(&task_machine, &old_id);
                    }
                    let (policy, fetched_policy) = match cached_policy {
                        Some(policy) => (policy, None),
                        None => {
                            let target_path = match input.target_kind {
                                TargetKind::Project | TargetKind::Worktree => {
                                    input.project_checkout.clone()
                                }
                                TargetKind::Directory => PathBuf::from(input.directory_path.trim()),
                            };
                            match client.send::<_, PolicyReply>(&PolicyRequest::CommandPolicy {
                                checkout: target_path,
                                identity: None,
                            }) {
                                Ok(reply) => (reply.policy, Some(reply.policy)),
                                Err(_) => return SubmitOutcome::PolicyUnavailable,
                            }
                        }
                    };
                    let Ok(definition) =
                        validate_and_build_with_policy(&input, to_automation_policy(policy))
                    else {
                        return SubmitOutcome::StorageError("Invalid automation settings.".into());
                    };
                    (definition, caller_thread_id, fetched_policy)
                }
                SendSpec::Retry {
                    definition,
                    caller_thread_id,
                } => (*definition, caller_thread_id, None),
            };
            let saved = match journal.save_automation_create(
                &task_machine,
                &request_id,
                &definition,
                caller_thread_id.as_deref(),
            ) {
                Ok(saved) => saved,
                Err(message) => return SubmitOutcome::StorageError(message),
            };
            let result = host::automations::create(
                &client,
                &saved.definition,
                &request_id,
                saved.caller_thread_id.as_deref(),
            );
            // Settle the saved attempt here, not only in the UI reply: the person may
            // have switched machines meanwhile, and an answered attempt must not be
            // offered again as unconfirmed.
            let answered = match &result {
                Ok(_) => true,
                Err(error) => host::automations::is_definitive_create_error(error),
            };
            if answered {
                let _ = journal.remove_automation_create(&task_machine, &request_id);
            }
            SubmitOutcome::Sent(Box::new(SentOutcome {
                request_id,
                definition: saved.definition,
                caller_thread_id: saved.caller_thread_id,
                fetched_policy,
                result,
            }))
        });

        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.machines.active != machine {
                    return;
                }
                let matches_form = this
                    .automation_form
                    .as_ref()
                    .is_some_and(|form| form.submit_generation == generation);
                match outcome {
                    SubmitOutcome::PolicyUnavailable => {
                        if matches_form && let Some(form) = &mut this.automation_form {
                            form.submitting = false;
                            form.submit_error = Some((POLICY_UNAVAILABLE_ERROR.into(), false));
                            cx.notify();
                        }
                    }
                    SubmitOutcome::StorageError(message) => {
                        if matches_form && let Some(form) = &mut this.automation_form {
                            form.submitting = false;
                            form.submit_error = Some((message, false));
                            cx.notify();
                        }
                    }
                    SubmitOutcome::Sent(sent) => {
                        let SentOutcome {
                            request_id,
                            definition,
                            caller_thread_id,
                            fetched_policy,
                            result,
                        } = *sent;
                        if matches_form
                            && let Some(policy) = fetched_policy
                            && this.current_target_path(cx).as_deref()
                                == Some(definition.target.path())
                            && let Some(form) = &mut this.automation_form
                        {
                            form.command_policy = Some(policy);
                            form.policy_path = Some(definition.target.path().to_path_buf());
                        }
                        // An accepted reply is created whatever the connection did since;
                        // only an error that arrives after a reconnect is unconfirmed.
                        if this.host.connection.epoch != epoch && result.is_err() {
                            if matches_form && let Some(form) = &mut this.automation_form {
                                form.submitting = false;
                                form.unconfirmed_attempt = Some(UnconfirmedAttempt {
                                    request_id,
                                    definition,
                                    caller_thread_id,
                                    discard_warning: false,
                                });
                                form.submit_error =
                                    Some((UNCONFIRMED_RECONNECT_MESSAGE.into(), true));
                                cx.notify();
                            }
                            return;
                        }
                        match result {
                            Ok(created) => {
                                this.settle_automation_create_success(
                                    machine, request_id, epoch, generation, created, definition,
                                    window, cx,
                                );
                            }
                            Err(error) => {
                                let definitive =
                                    host::automations::is_definitive_create_error(&error);
                                let message = match &error {
                                    host::Error::Rejected(reason) => reason.message.clone(),
                                    other => other.to_string(),
                                };
                                if definitive {
                                    let mint_new_id = matches!(
                                        &error,
                                        host::Error::Rejected(reason)
                                            if matches!(
                                                reason.code.as_str(),
                                                "conflict" | "not_found"
                                            )
                                    );
                                    this.settle_automation_create_refusal(
                                        machine,
                                        request_id,
                                        generation,
                                        message,
                                        mint_new_id,
                                        cx,
                                    );
                                } else if matches_form && let Some(form) = &mut this.automation_form
                                {
                                    form.submitting = false;
                                    form.unconfirmed_attempt = Some(UnconfirmedAttempt {
                                        request_id,
                                        definition,
                                        caller_thread_id,
                                        discard_warning: false,
                                    });
                                    form.submit_error =
                                        Some((format!("{message} Retry is safe."), true));
                                    cx.notify();
                                }
                            }
                        }
                    }
                }
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn settle_automation_create_success(
        &mut self,
        machine: MachineId,
        request_id: String,
        epoch: u64,
        generation: u64,
        created: CreatedAutomation,
        definition: Definition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(journal) = self.session.journal.clone() else {
            if let Some(form) = &mut self.automation_form
                && form.submit_generation == generation
            {
                form.submitting = false;
                cx.notify();
            }
            return;
        };
        let task_machine = machine.clone();
        let task = cx
            .background_executor()
            .spawn(async move { journal.remove_automation_create(&task_machine, &request_id) });
        cx.spawn_in(window, async move |this, cx| {
            let _ = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.machines.active != machine {
                    return;
                }
                if this.host.connection.epoch == epoch {
                    let summary = created.into_summary(&definition);
                    let id = summary.id.clone();
                    this.automations.record_successful_action(&id);
                    this.automations.apply_summary(epoch, summary);
                    this.automations.collapsed = false;
                    this.navigation.invalidate_rows();
                    if this
                        .automation_form
                        .as_ref()
                        .is_some_and(|form| form.submit_generation == generation)
                    {
                        this.close_automation_form(true, window, cx);
                    }
                    this.refresh_automations(cx);
                } else {
                    // The host accepted it; a reconnect since then doesn't undo that.
                    // Close the form so a second Create can't make a duplicate, and
                    // let the fresh connection's list show the new automation.
                    if this
                        .automation_form
                        .as_ref()
                        .is_some_and(|form| form.submit_generation == generation)
                    {
                        this.close_automation_form(true, window, cx);
                    }
                    this.refresh_automations(cx);
                }
            });
        })
        .detach();
    }

    fn settle_automation_create_refusal(
        &mut self,
        machine: MachineId,
        request_id: String,
        generation: u64,
        message: String,
        mint_new_id: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(journal) = self.session.journal.clone() else {
            if let Some(form) = &mut self.automation_form
                && form.submit_generation == generation
            {
                form.submitting = false;
                cx.notify();
            }
            return;
        };
        let task_machine = machine.clone();
        let task = cx
            .background_executor()
            .spawn(async move { journal.remove_automation_create(&task_machine, &request_id) });
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.machines.active != machine {
                    return;
                }
                if let Some(form) = &mut this.automation_form
                    && form.submit_generation == generation
                {
                    form.submitting = false;
                    form.unconfirmed_attempt = None;
                    if mint_new_id {
                        form.request_id = new_request_id();
                    }
                    form.submit_error = Some((message, false));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn settle_discard_found(
        &mut self,
        machine: MachineId,
        old_request_id: String,
        epoch: u64,
        generation: u64,
        summary: Summary,
        detail: Option<DetailReply>,
        cx: &mut Context<Self>,
    ) {
        let Some(journal) = self.session.journal.clone() else {
            if let Some(form) = &mut self.automation_form
                && form.submit_generation == generation
            {
                form.submitting = false;
                cx.notify();
            }
            return;
        };
        let task_machine = machine.clone();
        let task = cx
            .background_executor()
            .spawn(async move { journal.remove_automation_create(&task_machine, &old_request_id) });
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.machines.active != machine {
                    return;
                }
                let name = summary.name.clone();
                if this.host.connection.epoch == epoch {
                    let id = summary.id.clone();
                    this.automations.record_successful_action(&id);
                    this.automations.apply_summary(epoch, summary);
                    if let Some(detail) = detail {
                        this.automations.apply_detail(epoch, detail);
                    }
                    this.automations.collapsed = false;
                    this.navigation.invalidate_rows();
                    this.refresh_automations(cx);
                }
                if let Some(form) = &mut this.automation_form
                    && form.submit_generation == generation
                {
                    form.submitting = false;
                    form.unconfirmed_attempt = None;
                    form.request_id = new_request_id();
                    form.submit_error = Some((
                        format!(
                            "The previous attempt already created \"{name}\". Close this form, or press Create again to create a duplicate with your changes."
                        ),
                        true,
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Shields workspace shortcuts and traps Tab / Escape while the form modal is open.
    pub(super) fn automation_form_shortcuts(
        &self,
        mut root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if self.automation_form.is_none() {
            return root;
        }
        for action in actions::ACTIONS
            .iter()
            .filter(|action| !modal_action_allowed(action.id))
        {
            root = root.on_boxed_action(action.action().as_ref(), |_, _, _| {});
        }
        root.on_action(cx.listener(|this, _: &super::Tab, window, cx| {
            this.automation_form_focus_step(true, window, cx);
        }))
        .on_action(cx.listener(|this, _: &super::BackTab, window, cx| {
            this.automation_form_focus_step(false, window, cx);
        }))
        .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape"
                && this.overlay.is_none()
                && this.folder_prompt.is_none()
                && !this.automation_form_is_composing(cx)
            {
                if this
                    .automation_form
                    .as_ref()
                    .is_some_and(|form| !form.submitting)
                {
                    this.close_automation_form(true, window, cx);
                }
                cx.stop_propagation();
            }
        }))
    }

    fn automation_form_is_composing(&self, cx: &App) -> bool {
        let Some(form) = &self.automation_form else {
            return false;
        };
        [
            &form.name,
            &form.directory,
            &form.hourly_minute,
            &form.time,
            &form.once_date,
            &form.time_zone,
            &form.catch_up,
        ]
        .into_iter()
        .any(|field| field.read(cx).is_composing())
    }

    fn automation_form_focus_step(
        &self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &self.automation_form else {
            return;
        };
        if !form.focus.is_rendered(window) {
            window.focus(&form.focus, cx);
            return;
        }
        let start = window.focused(cx);
        loop {
            if forward {
                window.focus_next(cx);
            } else {
                window.focus_prev(cx);
            }
            if form.focus.contains_focused(window, cx) {
                break;
            }
            if window.focused(cx) == start {
                window.focus(&form.focus, cx);
                break;
            }
        }
    }

    pub(super) fn automation_form_modal(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let _above = super::theme::AboveModal::enter();
        let Some(form) = &self.automation_form else {
            return div();
        };
        let palette = colors();
        let viewport = window.viewport_size();
        let input = self
            .automation_form_input(cx)
            .expect("automation form is open");
        let validation = validate_fields(&input);
        let schedule_preview = preview_schedule(&input)
            .ok()
            .map(|s| schedule_with_zone(&s));
        let edited_after_unconfirmed = self.form_edited_after_unconfirmed(cx);
        let unconfirmed_discard_warning = form
            .unconfirmed_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.discard_warning);
        let retry_same_attempt = form.unconfirmed_attempt.is_some() && !edited_after_unconfirmed;
        let show_required_errors = form.submitted_once;

        let name_error = show_required_errors
            .then(|| validation.name.clone())
            .flatten();
        let instructions_error = show_required_errors
            .then(|| validation.instructions.clone())
            .flatten();
        let target_error = if show_required_errors
            || (form.target_kind == TargetKind::Directory
                && !input.directory_path.trim().is_empty())
        {
            validation.target.clone()
        } else {
            None
        };
        let schedule_time_error = validation.schedule_time.clone();
        let schedule_days_error = if form.schedule_kind == ScheduleKind::Weekly {
            validation.schedule_days.clone()
        } else {
            None
        };
        let time_zone_error = validation.time_zone.clone();
        let agent_error = show_required_errors
            .then(|| validation.agent.clone())
            .flatten();
        let catch_up_error = validation.catch_up.clone();

        let title = if form.caller_thread_id.is_some() {
            "Make automation from thread"
        } else {
            "New automation"
        };
        let project_name = if form.project_checkout.as_os_str().is_empty() {
            "Choose a project".to_owned()
        } else {
            plait_client::state::project_name(&form.project_checkout)
        };
        let project_path_label = (!form.project_checkout.as_os_str().is_empty())
            .then(|| home_relative(&form.project_checkout));

        let harness = form.harness;
        let harness_name = self.harness_row(harness).map_or_else(
            || SharedString::from(harness.label()),
            |row| row.name.clone(),
        );
        let model_label = form
            .model
            .as_ref()
            .map(|choice| {
                self.automation_model_entry(harness, choice).map_or_else(
                    || SharedString::from(super::model_picker::model_label(choice)),
                    |model| SharedString::from(model.name.clone()),
                )
            })
            .unwrap_or_else(|| SharedString::from("Choose a model"));
        let levels = self.automation_thinking_levels();
        let locked = form.submitting.then_some("Creating automation…");
        let connected = self.host.connection.connected;
        let can_create = connected && !form.submitting;
        let create_tip = if form.submitting {
            "Waiting for the host to accept this automation."
        } else if !connected {
            "Reconnect to the host before creating an automation."
        } else {
            validation
                .first_message()
                .unwrap_or("Create this automation and start its schedule.")
        }
        .to_owned();

        let agent_hint = if harness == HarnessId::Pi {
            let policy = plait_client::host::policy::context_label(form.command_policy);
            format!(
                "{policy}. Scheduled runs use this approval mode and project command policy and never commit or push."
            )
        } else {
            format!(
                "Runs under {harness_name}'s chosen permission mode. Plait's approvals and sandbox do not cover it."
            )
        };

        let permissions_control = if harness == HarnessId::Pi {
            Some(
                menu_chip(
                    "automation-approval",
                    Some("Permissions"),
                    None,
                    approval_label(form.approval).into(),
                    "Choose whether scheduled runs ask before each edit and command or run automatically."
                        .into(),
                    locked,
                )
                .relative()
                .child(self.anchor_probe("automation-approval"))
                .on_activate(cx, |this, window, cx| {
                    this.anchor_next("automation-approval");
                    this.open_automation_approvals(window, cx);
                })
                .into_any_element(),
            )
        } else {
            let current_mode = form
                .mode
                .clone()
                .or_else(|| self.default_mode_for_harness(harness))
                .unwrap_or_default();
            let permissions = Permissions {
                harness,
                current: current_mode,
                blocked: locked,
                tip: format!("Choose what {harness_name} may do during scheduled runs.").into(),
                note: "Applies to every scheduled run of this automation.".into(),
                on_select: Rc::new(move |this, mode, _, cx| {
                    this.choose_automation_mode(harness, mode, cx);
                }),
            };
            self.permissions_chip("automation-permissions", permissions, true, cx)
                .map(IntoElement::into_any_element)
        };

        let cx: &Context<Self> = cx;
        let panel = column()
            .id("automation-form-modal")
            .role(Role::Dialog)
            .aria_label(title)
            .audited()
            .track_focus(&form.focus)
            .tab_group()
            .tab_stop(false)
            .w(px(640.).min(viewport.width - px(48.)))
            .max_h(px(740.).min(viewport.height - px(64.)))
            .overflow_hidden()
            .rounded(px(SURFACE_CORNER))
            .shadow(floating_shadow())
            .border_1()
            .border_color(rgb(palette.edge))
            .bg(rgb(palette.raised))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.overlay.is_none()
                    && this.folder_prompt.is_none()
                    && this
                        .automation_form
                        .as_ref()
                        .is_some_and(|form| !form.submitting)
                {
                    this.close_automation_form(true, window, cx);
                }
            }))
            // Header
            .child(
                row()
                    .flex_shrink_0()
                    .px(px(24.))
                    .py(px(14.))
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(super::settings::heading(title))
                            .when_some(form.source_thread_title.clone(), |el, source| {
                                el.child(
                                    tag(
                                        "automation-form-source",
                                        format!("Prefilled from {source}"),
                                        palette.subtle,
                                    )
                                    .truncate(),
                                )
                            }),
                    )
                    .child(
                        icon_button("automation-form-close", "close", "Close automation form")
                            .disabled(form.submitting)
                            .tooltip(|_, cx| {
                                cx.new(|_| TextTooltip("Close (Escape)".into())).into()
                            })
                            .when(!form.submitting, |el| {
                                el.on_activate(cx, |this, window, cx| {
                                    this.close_automation_form(true, window, cx);
                                })
                            }),
                    ),
            )
            // Scrollable form body
            .child(
                column()
                    .id("automation-form-body")
                    .overflow_y_scroll()
                    .track_scroll(&form.scroll)
                    .flex_1()
                    .min_h_0()
                    .px(px(24.))
                    .py(px(16.))
                    .gap(px(14.))
                    // 1. Name
                    .child(
                        column()
                            .gap(px(5.))
                            .child(field_label("automation-name-label", "Name"))
                            .child(form.name.clone())
                            .when_some(name_error, |el, error| {
                                el.child(field_error("automation-name-error", error))
                            }),
                    )
                    // 2. Instructions
                    .child(
                        column()
                            .gap(px(5.))
                            .child(field_label("automation-instructions-label", "Instructions"))
                            .child(
                                div()
                                    .w_full()
                                    .min_h(px(96.))
                                    .px_2()
                                    .py_1()
                                    .rounded(px(ITEM_CORNER))
                                    .border_1()
                                    .border_color(rgb(palette.edge))
                                    .bg(rgb(palette.bg))
                                    .child(form.instructions.clone()),
                            )
                            .child(field_hint(
                                "automation-instructions-hint",
                                "The message sent at each scheduled run. Cmd-Enter creates the automation.",
                            ))
                            .when_some(instructions_error, |el, error| {
                                el.child(field_error("automation-instructions-error", error))
                            }),
                    )
                    // 3. Where
                    .child(
                        column()
                            .gap(px(6.))
                            .child(
                                row()
                                    .justify_between()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(field_label("automation-where-label", "Where"))
                                    .child(segmented().children(TargetKind::ALL.into_iter().map(
                                        |kind| {
                                            segment(
                                                ("automation-target-kind", kind as usize),
                                                kind.label(),
                                                form.target_kind == kind,
                                            )
                                            .disabled(form.submitting)
                                            .when(!form.submitting, |el| {
                                                el.on_activate(cx, move |this, _, cx| {
                                                    if let Some(form) = &mut this.automation_form {
                                                        form.target_kind = kind;
                                                        form.submit_error = None;
                                                    }
                                                    this.invalidate_stale_automation_policy(cx);
                                                    this.fetch_automation_policy(cx);
                                                    cx.notify();
                                                })
                                            })
                                        },
                                    ))),
                            )
                            .when(form.target_kind != TargetKind::Directory, |el| {
                                el.child(
                                    row()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(
                                            menu_chip(
                                                "automation-project",
                                                None,
                                                Some(
                                                    icon("folder")
                                                        .size(px(13.))
                                                        .text_color(rgb(palette.mint)),
                                                ),
                                                project_name.into(),
                                                "Choose which project this automation works in."
                                                    .into(),
                                                locked,
                                            )
                                            .relative()
                                            .child(self.anchor_probe("automation-project"))
                                            .on_activate(cx, |this, window, cx| {
                                                this.anchor_next("automation-project");
                                                this.open_automation_projects(window, cx);
                                            }),
                                        )
                                        .when_some(project_path_label, |el, path| {
                                            el.child(
                                                tag("automation-project-path", path, palette.subtle)
                                                    .font_family(mono())
                                                    .min_w_0()
                                                    .truncate(),
                                            )
                                        }),
                                )
                            })
                            .when(form.target_kind == TargetKind::Directory, |el| {
                                el.child(
                                    row()
                                        .gap_2()
                                        .child(div().flex_1().min_w_0().child(form.directory.clone()))
                                        .child(
                                            toned_button(
                                                "automation-choose-folder",
                                                "Choose folder…",
                                                Tone::Filled,
                                            )
                                            .disabled(form.submitting)
                                            .when(!form.submitting, |el| {
                                                el.on_activate(cx, |this, window, cx| {
                                                    this.open_automation_folder_browser(window, cx);
                                                })
                                            }),
                                        ),
                                )
                            })
                            .child(field_hint("automation-where-hint", form.target_kind.hint()))
                            .when_some(target_error, |el, error| {
                                el.child(field_error("automation-where-error", error))
                            }),
                    )
                    // 4. Schedule
                    .child(
                        column()
                            .gap(px(6.))
                            .child(
                                row()
                                    .justify_between()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(field_label("automation-schedule-label", "Schedule"))
                                    .child(segmented().children(
                                        ScheduleKind::ALL.into_iter().map(|kind| {
                                            segment(
                                                ("automation-schedule-kind", kind as usize),
                                                kind.label(),
                                                form.schedule_kind == kind,
                                            )
                                            .disabled(form.submitting)
                                            .when(!form.submitting, |el| {
                                                el.on_activate(cx, move |this, _, cx| {
                                                    if let Some(form) = &mut this.automation_form {
                                                        form.schedule_kind = kind;
                                                        if kind == ScheduleKind::Weekly
                                                            && form.weekly_days.is_empty()
                                                        {
                                                            form.weekly_days = vec![1];
                                                        }
                                                        form.submit_error = None;
                                                    }
                                                    cx.notify();
                                                })
                                            })
                                        }),
                                    )),
                            )
                            .when(form.schedule_kind == ScheduleKind::Weekly, |el| {
                                el.child(row().flex_wrap().gap(px(4.)).children(
                                    WEEKDAY_LABELS.into_iter().map(|(day, label)| {
                                        let selected = form.weekly_days.contains(&day);
                                        chip(("automation-day", day as usize), label, selected)
                                            .disabled(form.submitting)
                                            .when(!form.submitting, |el| {
                                                el.on_activate(cx, move |this, _, cx| {
                                                    if let Some(form) = &mut this.automation_form {
                                                        if let Some(pos) = form
                                                            .weekly_days
                                                            .iter()
                                                            .position(|d| *d == day)
                                                        {
                                                            form.weekly_days.remove(pos);
                                                        } else {
                                                            form.weekly_days.push(day);
                                                            form.weekly_days.sort_unstable();
                                                        }
                                                        form.submit_error = None;
                                                    }
                                                    cx.notify();
                                                })
                                            })
                                    }),
                                ))
                            })
                            .child(
                                row()
                                    .flex_wrap()
                                    .items_end()
                                    .gap_3()
                                    .when(form.schedule_kind == ScheduleKind::Hourly, |el| {
                                        el.child(
                                            column()
                                                .w(px(110.))
                                                .gap(px(4.))
                                                .child(field_hint(
                                                    "automation-minute-label",
                                                    "Minute (0–59)",
                                                ))
                                                .child(form.hourly_minute.clone()),
                                        )
                                    })
                                    .when(form.schedule_kind == ScheduleKind::Once, |el| {
                                        el.child(
                                            column()
                                                .w(px(140.))
                                                .gap(px(4.))
                                                .child(field_hint(
                                                    "automation-date-label",
                                                    "Date (YYYY-MM-DD)",
                                                ))
                                                .child(form.once_date.clone()),
                                        )
                                    })
                                    .when(form.schedule_kind != ScheduleKind::Hourly, |el| {
                                        el.child(
                                            column()
                                                .w(px(110.))
                                                .gap(px(4.))
                                                .child(field_hint(
                                                    "automation-time-label",
                                                    "Time (HH:mm)",
                                                ))
                                                .child(form.time.clone()),
                                        )
                                    })
                                    .child(
                                        column()
                                            .flex_1()
                                            .min_w(px(170.))
                                            .gap(px(4.))
                                            .child(field_hint(
                                                "automation-tz-label",
                                                "Time zone (IANA)",
                                            ))
                                            .child(form.time_zone.clone()),
                                    ),
                            )
                            .when_some(schedule_preview, |el, preview| {
                                el.child(
                                    row()
                                        .gap(px(5.))
                                        .child(
                                            icon("clock")
                                                .size(px(12.))
                                                .text_color(rgb(palette.subtle)),
                                        )
                                        .child(tag(
                                            "automation-schedule-preview",
                                            preview,
                                            palette.muted,
                                        )),
                                )
                            })
                            .when_some(schedule_days_error, |el, error| {
                                el.child(field_error("automation-schedule-days-error", error))
                            })
                            .when_some(schedule_time_error, |el, error| {
                                el.child(field_error("automation-schedule-time-error", error))
                            })
                            .when_some(time_zone_error, |el, error| {
                                el.child(field_error("automation-tz-error", error))
                            }),
                    )
                    // 5. Agent
                    .child(
                        column()
                            .gap(px(6.))
                            .child(field_label("automation-agent-label", "Agent"))
                            .child(
                                row()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        menu_chip(
                                            "automation-harness",
                                            None,
                                            harness_logo(harness)
                                                .map(|logo| logo.text_color(rgb(palette.muted))),
                                            harness_name.clone(),
                                            "Choose which agent harness runs this automation."
                                                .into(),
                                            locked,
                                        )
                                        .aria_label(format!("Harness: {harness_name}"))
                                        .relative()
                                        .child(self.anchor_probe("automation-harness"))
                                        .on_activate(cx, |this, window, cx| {
                                            this.anchor_next("automation-harness");
                                            this.open_automation_harnesses(window, cx);
                                        }),
                                    )
                                    .child(
                                        menu_chip(
                                            "automation-model",
                                            None,
                                            None,
                                            model_label.clone(),
                                            format!(
                                                "Model: {model_label}. Choose among {harness_name}'s models."
                                            )
                                            .into(),
                                            locked,
                                        )
                                        .max_w(px(240.))
                                        .aria_label(format!("Model: {model_label}"))
                                        .relative()
                                        .child(self.anchor_probe("automation-model"))
                                        .on_activate(cx, |this, window, cx| {
                                            if this
                                                .automation_form
                                                .as_ref()
                                                .is_none_or(|f| f.submitting)
                                            {
                                                return;
                                            }
                                            this.anchor_next("automation-model");
                                            this.open_model_picker(
                                                ModelPurpose::Automation,
                                                window,
                                                cx,
                                            );
                                        }),
                                    )
                                    .when(!levels.is_empty(), |el| {
                                        let level = form.thinking.unwrap_or(ThinkingLevel::Off);
                                        el.child(
                                            menu_chip(
                                                "automation-thinking",
                                                None,
                                                None,
                                                thinking_value(level).into(),
                                                "Choose how much the model thinks on each run."
                                                    .into(),
                                                locked,
                                            )
                                            .aria_label(format!("Thinking level: {}", level.label()))
                                            .relative()
                                            .child(self.anchor_probe("automation-thinking"))
                                            .on_activate(cx, |this, window, cx| {
                                                this.anchor_next("automation-thinking");
                                                this.open_automation_thinking(window, cx);
                                            }),
                                        )
                                    })
                                    .when_some(permissions_control, |el, control| el.child(control)),
                            )
                            .child(field_hint("automation-agent-hint", agent_hint))
                            .when_some(agent_error, |el, error| {
                                el.child(field_error("automation-agent-error", error))
                            }),
                    )
                    // 6. Optional catch-up window
                    .child(
                        column()
                            .gap(px(5.))
                            .child(field_label(
                                "automation-catchup-label",
                                "Catch-up window (optional)",
                            ))
                            .child(form.catch_up.clone())
                            .child(field_hint(
                                "automation-catchup-hint",
                                format!(
                                    "How late a missed run may still start (e.g. 30m, 2h, or 0 to skip). Default for {}: {}.",
                                    form.schedule_kind.label().to_lowercase(),
                                    form.schedule_kind.default_catch_up_label()
                                ),
                            ))
                            .when_some(catch_up_error, |el, error| {
                                el.child(field_error("automation-catchup-error", error))
                            }),
                    ),
            )
            // Footer
            .child(
                column()
                    .flex_shrink_0()
                    .px(px(24.))
                    .py(px(12.))
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .when_some(form.submit_error.clone(), |el, (message, is_warning)| {
                        let tint = if is_warning {
                            palette.amber
                        } else {
                            palette.red
                        };
                        el.child(
                            row()
                                .items_start()
                                .gap_2()
                                .child(dot(tint).mt(px(5.)))
                                .child(
                                    tag("automation-submit-error-text", message, tint)
                                        .id("automation-submit-error")
                                        .role(Role::Alert)
                                        .flex_1()
                                        .min_w_0()
                                        .line_height(px(17.))
                                        .audited(),
                                ),
                        )
                    })
                    .when(edited_after_unconfirmed, |el| {
                        let choice_text = if unconfirmed_discard_warning {
                            DUPLICATE_POSSIBLE_WARNING
                        } else {
                            "Your previous creation attempt wasn't confirmed. Choose whether to retry it as sent, or discard it and create with your changes."
                        };
                        el.child(
                            column()
                                .gap_2()
                                .child(
                                    row()
                                        .items_start()
                                        .gap_2()
                                        .child(dot(palette.amber).mt(px(5.)))
                                        .child(
                                            tag(
                                                "automation-unconfirmed-choice-text",
                                                choice_text,
                                                palette.amber,
                                            )
                                            .id("automation-unconfirmed-choice-banner")
                                            .role(Role::Alert)
                                            .flex_1()
                                            .min_w_0()
                                            .line_height(px(17.))
                                            .audited(),
                                        ),
                                )
                                .child(
                                    row()
                                        .flex_wrap()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            toned_button(
                                                "automation-form-retry-previous",
                                                "Retry the previous attempt",
                                                Tone::Filled,
                                            )
                                            .disabled(!can_create)
                                            .when(can_create, |el| {
                                                el.on_activate(cx, |this, window, cx| {
                                                    this.retry_previous_automation_attempt(
                                                        window, cx,
                                                    );
                                                })
                                            }),
                                        )
                                        .child(
                                            toned_button(
                                                "automation-form-discard-create",
                                                "Discard it and create with my changes",
                                                Tone::Primary,
                                            )
                                            .disabled(!can_create)
                                            .when(can_create, |el| {
                                                el.on_activate(cx, |this, window, cx| {
                                                    this.discard_and_create_automation(window, cx);
                                                })
                                            }),
                                        ),
                                ),
                        )
                    })
                    .child(
                        row()
                            .justify_between()
                            .gap_2()
                            .child(
                                tag(
                                    "automation-consent-note",
                                    "Nothing is scheduled until you press Create.",
                                    palette.subtle,
                                )
                                .flex_1()
                                .min_w_0(),
                            )
                            .child(
                                row()
                                    .flex_shrink_0()
                                    .gap_2()
                                    .child(
                                        button("automation-form-cancel", "Cancel")
                                            .disabled(form.submitting)
                                            .when(!form.submitting, |el| {
                                                el.on_activate(cx, |this, window, cx| {
                                                    this.close_automation_form(true, window, cx);
                                                })
                                            }),
                                    )
                                    .when(!edited_after_unconfirmed, |el| {
                                        el.child(
                                            toned_button(
                                                "automation-form-create",
                                                if form.submitting {
                                                    "Creating…"
                                                } else if retry_same_attempt {
                                                    "Retry"
                                                } else {
                                                    "Create"
                                                },
                                                Tone::Primary,
                                            )
                                            .disabled(!can_create)
                                            .aria_description(create_tip.clone())
                                            .tooltip(move |_, cx| {
                                                cx.new(|_| TextTooltip(create_tip.clone())).into()
                                            })
                                            .when(can_create, |el| {
                                                el.on_activate(cx, |this, window, cx| {
                                                    this.submit_automation_form(window, cx);
                                                })
                                            }),
                                        )
                                    }),
                            ),
                    ),
            );

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., if palette.light { 0.25 } else { 0.45 }))
            .child(panel)
    }
}

enum SendSpec {
    Draft {
        input: Box<AutomationDraftInput>,
        cached_policy: Option<HostCommandPolicy>,
        caller_thread_id: Option<String>,
        discard_previous: Option<String>,
    },
    Retry {
        definition: Box<Definition>,
        caller_thread_id: Option<String>,
    },
}

struct SentOutcome {
    request_id: String,
    definition: Definition,
    caller_thread_id: Option<String>,
    fetched_policy: Option<HostCommandPolicy>,
    result: Result<CreatedAutomation, host::Error>,
}

enum SubmitOutcome {
    PolicyUnavailable,
    StorageError(String),
    Sent(Box<SentOutcome>),
}

struct FormSeed {
    name: String,
    instructions: String,
    project_checkout: PathBuf,
    harness: HarnessId,
    model: Option<ModelChoice>,
    mode: Option<String>,
    thinking: Option<ThinkingLevel>,
    approval: ApprovalMode,
    caller_thread_id: Option<String>,
    source_thread_title: Option<String>,
    needs_prefill: bool,
}
