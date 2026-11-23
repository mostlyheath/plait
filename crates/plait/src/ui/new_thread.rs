//! The new-thread draft: a composer with nothing created on the host. The
//! thread is created when its first message is sent, and that message is then
//! submitted to it, so an abandoned New Thread leaves nothing behind.
//!
//! The draft lands in the active checkout. Its harness, model, thinking level,
//! and permissions are chosen in the row above its composer; until then each
//! follows its default, and the harness follows Settings → New threads. Leaving the
//! draft keeps its text; Discard removes it and closes its tab.

use super::{
    Plait, Workspace,
    composer::{Composer, ComposerEvent},
};
use gpui::*;
use plait_client::{
    compose::thinking_levels,
    host::{ApprovalMode, Model, ModelChoice, ThinkingLevel, harness::HarnessId},
};

#[derive(Default)]
pub(super) struct Draft {
    pub(super) id: Option<String>,
    pub(super) saved: plait_client::session::Draft,
    pub(super) pending: Option<super::draft_tabs::Pending>,
    /// On screen in the conversation area, in place of a thread.
    pub(super) open: bool,
    /// Made when the draft first opens; it becomes the thread's composer once created.
    pub(super) composer: Option<Entity<Composer>>,
    /// Chosen in the header; `None` follows the default harness, which can
    /// change while the draft is open as the host's list arrives.
    pub(super) harness: Option<HarnessId>,
    /// Chosen in the header; `None` follows the harness's default.
    pub(super) model: Option<ModelChoice>,
    /// A harness's own mode; `None` follows its default. Pi has none.
    pub(super) mode: Option<String>,
    /// Chosen in the header; `None` follows Settings' default. Either way the
    /// thread starts at the nearest level its model has.
    pub(super) thinking: Option<ThinkingLevel>,
    /// Pi only: Plait's approvals for the thread; `None` follows Settings.
    pub(super) approval: Option<ApprovalMode>,
    /// Bumped on discard and on delivery, so an earlier draft's reply cannot reach this one.
    pub(super) generation: u64,
    pub(super) sending: Option<Sending>,
    /// Why the last send did not create a thread.
    pub(super) error: Option<String>,
}

/// A first message waiting for its thread to be created.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Sending {
    pub(super) generation: u64,
    /// The creation request it waits on.
    pub(super) request_id: String,
    pub(super) text: String,
    pub(super) images: Vec<plait_client::images::ImageRef>,
    /// The composer revision sent, so a later edit is not cleared by acceptance.
    pub(super) revision: u64,
    /// The approval mode the first run asks for, and the thread's own choice.
    pub(super) approval: Option<ApprovalMode>,
    pub(super) choice: Option<ApprovalMode>,
}

/// What a disabled header chip says while a creation is unanswered.
pub(super) const LOCKED: &str = "Resolve the pending thread creation first.";

impl Plait {
    /// The draft is what the conversation area shows now, including under a
    /// settings modal, which leaves the workspace as it is.
    pub(super) fn draft_visible(&self) -> bool {
        self.draft.open
            && self.host.thread.selected.is_none()
            && self.workspace.tab == Workspace::Conversation
    }

    /// Show the draft, with any text it kept, in the active project.
    pub(super) fn open_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab_target(
            plait_client::tabs::TabTarget::Draft {
                id: plait_client::host_state::request_id(),
                checkout: self.tab_checkout(self.visible_project_checkout().to_owned()),
            },
            window,
            cx,
        );
    }

    pub(super) fn open_draft_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(false, window, cx);
        self.settings_open = false;
        self.workspace.tab = Workspace::Conversation;
        if self.host.thread.selected.is_some() {
            self.workspace.host_watch = None;
            self.host.select(None);
            self.workspace.history = Default::default();
            self.refresh_live_list(None);
            self.workspace.formatted = Default::default();
        }
        // A saved selection arriving later must not replace the draft.
        self.session.restore_thread = None;
        self.navigation.menu = None;
        self.navigation.rename = None;
        self.draft.open = true;
        if self.draft.composer.is_none() {
            self.draft.composer = Some(self.draft_composer(window, cx));
        }
        self.sync_composer(cx);
        self.capture_session(cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Open the draft in project `checkout`, moving to it first.
    pub(super) fn open_draft_in(
        &mut self,
        checkout: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = if self.project_of(&self.host.creation.checkout) == checkout {
            self.host.creation.checkout.clone()
        } else {
            checkout
        };
        self.open_tab_target(
            plait_client::tabs::TabTarget::Draft {
                id: plait_client::host_state::request_id(),
                checkout: self.tab_checkout(root),
            },
            window,
            cx,
        );
    }

    fn draft_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Composer> {
        let composer = cx.new(|cx| Composer::new(window, cx));
        let draft = self
            .session
            .data
            .as_ref()
            .and_then(|data| {
                data.draft_tab(self.draft.id.as_deref()?, &self.host.creation.checkout)
            })
            .map(|tab| &tab.draft)
            .unwrap_or(&self.draft.saved)
            .clone();
        {
            composer.update(cx, |composer, cx| {
                composer.restore_draft(draft.text, draft.revision, draft.submitted, cx);
                composer.restore_images(draft.images, cx);
            });
        }
        cx.observe(&composer, |this, _, cx| this.capture_session(cx))
            .detach();
        cx.subscribe_in(&composer, window, |this, emitter, event, window, cx| {
            // Once delivered, the composer belongs to its thread.
            if this.draft.composer.as_ref() != Some(emitter) || !this.draft.open {
                return;
            }
            match event {
                ComposerEvent::Submit { text, revision, .. } => {
                    this.send_draft(text.clone(), *revision, window, cx)
                }
                ComposerEvent::Stop => {}
                other => this.composer_event(other, window, cx),
            }
        })
        .detach();
        composer
    }

    /// Clear the draft and its choices. A creation already sent still
    /// finishes on the host, but its reply opens nothing and sends nothing.
    pub(super) fn discard_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.model.active().filter(|id| {
            self.tabs.model.get(*id).is_some_and(|tab| {
                matches!(tab.session().target(),
                plait_client::tabs::TabTarget::Draft { id, .. } if self.draft.id.as_ref() == Some(id))
            })
        }) else {
            return;
        };
        // Closing the front tab is refused while another creation is unanswered;
        // refuse first, so the draft is not erased under a tab that stays open.
        if self.host.creation.creating || self.host.creation.pending_create.is_some() {
            self.notice =
                Some("Resolve the pending thread creation before discarding this draft.".into());
            cx.notify();
            return;
        }
        self.cancel_isolation(cx);
        window.focus(&self.focus, cx);
        self.clear_new_draft(cx);
        self.draft = super::new_thread::Draft {
            generation: self.draft.generation + 1,
            ..Default::default()
        };
        self.request_tab(super::tab_lifecycle::Intent::Close(tab), window, cx);
        cx.notify();
    }

    /// Escape is pressed by reflex, so it closes only a draft that holds nothing,
    /// as closing an empty draft does; a draft with anything in it stays.
    pub(super) fn escape_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty = self.draft.pending.is_none()
            && !self.draft_isolation_choice()
            && self
                .draft
                .composer
                .as_ref()
                .is_some_and(|c| c.read(cx).holds_nothing());
        let Some(tab) = self.tabs.model.active().filter(|_| empty) else {
            return;
        };
        if self.tabs.model.get(tab).is_some_and(|tab| {
            matches!(tab.session().target(),
            plait_client::tabs::TabTarget::Draft { id, .. } if self.draft.id.as_ref() == Some(id))
        }) {
            self.request_tab(super::tab_lifecycle::Intent::Close(tab), window, cx);
        }
    }

    /// The harness the draft starts its thread in: the one chosen, else the default.
    pub(super) fn draft_harness(&self) -> HarnessId {
        self.draft.harness.unwrap_or_else(|| self.default_harness())
    }

    /// An unavailable configured harness stays selected until the person changes it.
    pub(super) fn default_harness(&self) -> HarnessId {
        self.preferences.value.default_harness
    }

    /// The model the draft starts with: the one chosen, else the harness's default.
    pub(super) fn draft_model(&self) -> Option<ModelChoice> {
        self.draft
            .model
            .clone()
            .or_else(|| self.default_model_for(self.draft_harness()))
    }

    /// Pi's saved default. Another harness: the model of it used most
    /// recently, else the first it lists.
    pub(super) fn default_model_for(&self, harness: HarnessId) -> Option<ModelChoice> {
        if harness == HarnessId::Pi {
            return self.host.connection.status.as_ref()?.default_model.clone();
        }
        let models = &self.harness_row(harness)?.models;
        self.preferences
            .value
            .recent_models
            .iter()
            .find_map(|key| {
                models
                    .iter()
                    .find(|model| model.provider == key.provider && model.model_id == key.model_id)
            })
            .or_else(|| models.first())
            .map(|model| model.choice())
    }

    /// The harness mode the thread starts in; `None` for Pi.
    pub(super) fn draft_mode(&self) -> Option<String> {
        let row = self.harness_row(self.draft_harness())?;
        self.draft
            .mode
            .clone()
            .filter(|mode| row.mode(mode).is_some())
            .or_else(|| row.default_mode.clone())
    }

    /// Pi's approval for the first run: the draft's choice, else Settings.
    pub(super) fn draft_approval(&self) -> ApprovalMode {
        self.draft
            .approval
            .unwrap_or_else(|| self.default_approval())
    }

    /// The draft's model as its harness lists it: Pi's catalog, else the harness entry.
    fn draft_model_entry(&self, choice: &ModelChoice) -> Option<&Model> {
        let harness = self.draft_harness();
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

    /// The display name of the draft's model, from its harness's list.
    pub(super) fn draft_model_name(&self) -> Option<SharedString> {
        let choice = self.draft_model()?;
        Some(self.draft_model_entry(&choice).map_or_else(
            || super::model_picker::model_label(&choice).into(),
            |model| model.name.clone().into(),
        ))
    }

    /// The levels the draft's model supports; empty when it has none or is unknown.
    pub(super) fn draft_thinking_levels(&self) -> &[ThinkingLevel] {
        let Some(choice) = self.draft_model() else {
            return &[];
        };
        thinking_levels(self.draft_model_entry(&choice))
    }

    /// The level the thread starts at: the draft's choice, else Settings'
    /// default, moved to the nearest level the model has. `None` when it has none.
    pub(super) fn draft_thinking(&self) -> Option<ThinkingLevel> {
        let wanted = self
            .draft
            .thinking
            .unwrap_or_else(|| self.preferences.value.default_thinking());
        nearest_level(self.draft_thinking_levels(), wanted)
    }

    /// After a model or harness change, a level chosen earlier stays when the
    /// new model has it and otherwise moves to its nearest one. `shown` is the
    /// level before the change. An unchosen level keeps following the default.
    fn carry_draft_thinking(&mut self, shown: Option<ThinkingLevel>) {
        if self.draft.thinking.is_none() {
            return;
        }
        // A model without levels leaves the choice for the next one that has them.
        if let Some(level) =
            shown.and_then(|shown| nearest_level(self.draft_thinking_levels(), shown))
        {
            self.draft.thinking = Some(level);
        }
    }

    /// The draft's first message is waiting on creation `request_id`.
    pub(super) fn draft_sent(&self, request_id: &str) -> bool {
        self.draft.sending.as_ref().is_some_and(|sending| {
            sending.request_id == request_id && sending.generation == self.draft.generation
        })
    }

    /// The pending creation is the draft's own.
    pub(super) fn draft_pending(&self) -> bool {
        self.draft.pending.is_some()
    }

    pub(super) fn draft_creating(&self) -> bool {
        self.draft
            .pending
            .as_ref()
            .is_some_and(|pending| pending.creating)
    }

    pub(super) fn draft_locked(&self) -> Option<&'static str> {
        self.draft.pending.as_ref().map(|_| LOCKED)
    }

    /// Why the draft cannot be sent now, beyond what disables its composer.
    pub(super) fn draft_blocked(&self) -> Option<String> {
        if !self.tab_identity_current() {
            return Some("Checking the reconnected host…".into());
        }
        if self.draft_pending() {
            // Sending again retries the same creation.
            return None;
        }
        let harness = self.draft_harness();
        if harness != HarnessId::Pi {
            match self.harness_row(harness) {
                None => {
                    return Some(format!(
                        "{} is not available on this host.",
                        harness.label()
                    ));
                }
                Some(row) if !row.ready => {
                    return Some(format!("{}: {}.", row.name, row.state.to_lowercase()));
                }
                Some(_) => {}
            }
        }
        let Some(choice) = self.draft_model() else {
            return Some("Choose a model above to send.".into());
        };
        if !self.usable_in(harness, &choice) {
            let name = self.draft_model_name().unwrap_or_default();
            return Some(format!(
                "{name} is not signed in · choose another model above."
            ));
        }
        None
    }

    /// Keep the draft composer's state in step with the host. Never in render.
    pub(super) fn sync_draft_composer(&mut self, cx: &mut Context<Self>) {
        let Some(composer) = self.draft.composer.clone() else {
            return;
        };
        let reason = if self.navigation.available.get(&self.host.creation.checkout) == Some(&false)
        {
            Some("Checkout folder unavailable. Choose another project above.".to_owned())
        } else if !self.session.ready() {
            Some("Restoring saved work…".into())
        } else if !self.host.connection.connected {
            Some("Host disconnected. Reconnect to send.".into())
        } else if self.draft_creating() {
            // The text being sent stays as it is until the thread has it.
            Some("Creating thread…".into())
        } else {
            None
        };
        let blocked = self.draft_blocked();
        let agent = self.agent_commands();
        composer.update(cx, |composer, cx| {
            let support = self
                .draft_model()
                .and_then(|choice| self.draft_model_entry(&choice))
                .and_then(|model| model.image_input.as_deref());
            composer.set_image_support(self.host.protocol(), support, cx);
            composer.set_agent_commands(agent, cx);
            composer.set_send_blocked(blocked, cx);
            composer.set_enabled(reason.is_none(), reason, cx);
            composer.set_busy(false, cx);
            composer.set_queue_enabled(false, false, cx);
        });
    }

    /// The draft's first message: create its thread, unless a creation for it
    /// is already unconfirmed, which is retried with the same request ID.
    pub(super) fn send_draft(
        &mut self,
        text: String,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_draft_tab(text, revision, window, cx);
    }

    /// Retry only this draft's captured creation and message.
    pub(super) fn retry_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_draft_creation(window, cx);
    }

    /// Retry the pending creation from wherever the person is.
    pub(super) fn retry_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft_pending() {
            return self.retry_draft(window, cx);
        }
        self.new_thread_open = true;
        self.create_host_thread(window, cx);
    }

    /// The host refused to create the draft's thread. Its text stays.
    pub(super) fn draft_rejected(&mut self, error: String) {
        self.draft.sending = None;
        self.draft.error = Some(format!("Thread not created. {error}"));
    }

    /// Choose the draft's harness. Its model and permissions return to that
    /// harness's defaults. The default harness in Settings stays as it is.
    pub(super) fn choose_draft_harness(&mut self, id: HarnessId, cx: &mut Context<Self>) {
        if self.draft_locked().is_some() {
            return;
        }
        if id == self.draft_harness() {
            // Chosen now, so a later change to the default leaves it in place.
            self.draft.harness = Some(id);
            return;
        }
        let usable = id == HarnessId::Pi
            || self
                .harness_row(id)
                .is_some_and(super::harnesses::HarnessRow::startable);
        if !usable {
            return;
        }
        let shown = self.draft_thinking();
        self.draft.harness = Some(id);
        self.draft.model = None;
        self.draft.mode = None;
        self.draft.approval = None;
        self.draft.error = None;
        self.carry_draft_thinking(shown);
        self.sync_composer(cx);
        self.refresh_tab_rows();
        self.capture_session(cx);
        cx.notify();
    }

    /// A model from the draft's harness's list.
    pub(super) fn choose_draft_model(
        &mut self,
        harness: HarnessId,
        choice: ModelChoice,
        cx: &mut Context<Self>,
    ) {
        if self.draft_locked().is_some() || harness != self.draft_harness() {
            return;
        }
        let listed =
            if harness == HarnessId::Pi {
                self.host.connection.status.as_ref().is_some_and(|status| {
                    status.models.iter().any(|model| model.choice() == choice)
                })
            } else {
                self.harness_model(harness, &choice).is_some()
            };
        if !listed {
            return;
        }
        // The model belongs to this harness, so the draft keeps both.
        let shown = self.draft_thinking();
        self.draft.harness = Some(harness);
        self.draft.model = Some(choice);
        self.draft.error = None;
        self.carry_draft_thinking(shown);
        self.sync_composer(cx);
        self.refresh_tab_rows();
        self.capture_session(cx);
        cx.notify();
    }

    /// One of the draft harness's own modes, from the menu opened for it.
    pub(super) fn choose_draft_mode(
        &mut self,
        harness: HarnessId,
        mode: String,
        cx: &mut Context<Self>,
    ) {
        if self.draft_locked().is_some() || harness != self.draft_harness() {
            return;
        }
        if self
            .harness_row(harness)
            .is_none_or(|row| row.mode(&mode).is_none())
        {
            return;
        }
        self.draft.harness = Some(harness);
        self.draft.mode = Some(mode);
        self.capture_session(cx);
        cx.notify();
    }

    /// A thinking level from the menu the draft's thinking chip opens.
    pub(super) fn choose_draft_thinking(&mut self, level: ThinkingLevel, cx: &mut Context<Self>) {
        if self.draft_locked().is_some() || !self.draft_thinking_levels().contains(&level) {
            return;
        }
        self.draft.thinking = Some(level);
        self.capture_session(cx);
        cx.notify();
    }

    /// Pi's approvals for the new thread; `None` follows Settings → New threads.
    pub(super) fn choose_draft_approval(
        &mut self,
        mode: Option<ApprovalMode>,
        cx: &mut Context<Self>,
    ) {
        if self.draft_locked().is_some() || self.draft_harness() != HarnessId::Pi {
            return;
        }
        self.draft.harness = Some(HarnessId::Pi);
        self.draft.approval = mode;
        self.capture_session(cx);
        cx.notify();
    }
}

/// The level in `levels` closest to `wanted`, the lower one on a tie, as the
/// host chooses; `None` when there are none.
pub(super) fn nearest_level(
    levels: &[ThinkingLevel],
    wanted: ThinkingLevel,
) -> Option<ThinkingLevel> {
    let rank = |level: ThinkingLevel| {
        ThinkingLevel::ALL
            .iter()
            .position(|other| *other == level)
            .unwrap_or_default()
    };
    levels
        .iter()
        .copied()
        .min_by_key(|level| (rank(*level).abs_diff(rank(wanted)), rank(*level)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ThinkingLevel::*;

    #[core::prelude::v1::test]
    fn nearest_level_keeps_a_supported_level_and_breaks_ties_low() {
        assert_eq!(nearest_level(&[Low, Medium, High], Medium), Some(Medium));
        assert_eq!(nearest_level(&[Low, Medium, High], Max), Some(High));
        assert_eq!(nearest_level(&[Low, Medium, High], Off), Some(Low));
        // Medium is as far from Low as from High.
        assert_eq!(nearest_level(&[Off, Low, High], Medium), Some(Low));
        assert_eq!(nearest_level(&[High, Off], Xhigh), Some(High));
        assert_eq!(nearest_level(&[], Medium), None);
    }
}
