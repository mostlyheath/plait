//! What the window does for its composers: supply sent prompts and the
//! checkout's file list, and perform the local commands a composer reports.

use super::{CommandId, Composer, ComposerEvent, MentionFiles};
use crate::ui::Plait;
use gpui::*;
use plait_client::{host::MessageRole, state::Load};

impl Plait {
    /// The open thread's composer, or the draft's when it is open instead.
    pub(super) fn selected_composer(&self) -> Option<&Entity<Composer>> {
        match &self.host.thread.selected {
            Some(id) => self.workspace.composers.get(id),
            None => self.draft.composer.as_ref().filter(|_| self.draft.open),
        }
    }

    /// Handle the composer events that are not a submission or Stop.
    pub(in crate::ui) fn composer_event(
        &mut self,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Command(id) => self.run_composer_command(*id, window, cx),
            ComposerEvent::FilesNeeded => self.supply_files(cx),
            ComposerEvent::Submit { .. } | ComposerEvent::Stop => {}
        }
    }

    /// Each command is something a menu item or button already does.
    fn run_composer_command(&mut self, id: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            CommandId::NewThread => self.open_draft(window, cx),
            CommandId::ShowChanges => self.show_tab(super::super::Workspace::Changes, window, cx),
            CommandId::ShowConversation => {
                self.show_tab(super::super::Workspace::Conversation, window, cx)
            }
            CommandId::StopRun => self.control_thread(None, window, cx),
            CommandId::OpenSettings => self.show_settings(true, window, cx),
            // Unlike the menu item, keep focus in the composer that asked.
            CommandId::RefreshGit => {
                if !matches!(self.state.repository.snapshot(), Load::Loading) {
                    self.state.repository.refresh();
                    self.pump(cx);
                    cx.notify();
                }
            }
        }
    }

    /// Start the read-only file listing if this epoch has none, then report
    /// what is known. The read itself runs on the background executor.
    pub(in crate::ui) fn supply_files(&mut self, cx: &mut Context<Self>) {
        self.state.repository.request_files();
        self.pump(cx);
        self.deliver_files(cx);
    }

    /// Pass the repository's file list to the selected composer. Called after
    /// every Git job; the composer ignores a list it already has.
    pub(in crate::ui) fn deliver_files(&self, cx: &mut Context<Self>) {
        let files = match &self.state.repository.files() {
            // Nothing asked for the list in this epoch; a composer keeps its last one.
            Load::Empty => return,
            Load::Loading => MentionFiles::Loading,
            Load::Ready(files) => MentionFiles::Ready(files.clone()),
            Load::Error(error) => MentionFiles::Unavailable(error.clone()),
        };
        if let Some(composer) = self.selected_composer() {
            composer.update(cx, |composer, cx| composer.set_files(files, cx));
        }
    }

    /// Give the selected composer the prompts the host lists for its thread.
    pub(in crate::ui) fn sync_prompt_history(&self, cx: &mut Context<Self>) {
        let Some(composer) = self.selected_composer() else {
            return;
        };
        let Some(snapshot) = self
            .host
            .thread
            .snapshot
            .as_ref()
            .filter(|snapshot| Some(&snapshot.id) == self.host.thread.selected.as_ref())
        else {
            return;
        };
        composer.update(cx, |composer, _| {
            composer.set_history(
                snapshot
                    .messages
                    .iter()
                    .filter(|message| message.role == MessageRole::User)
                    .map(|message| message.text.as_str()),
            )
        });
    }
}
