//! What the palette can run, the sources that list it, and the overlay that
//! hosts a palette above the workspace.
//!
//! To list something new: add a `Command` variant, a function that returns its
//! `Item`s, and one line in `command_items`. The palette view is unchanged.

use super::{
    Plait,
    actions::{ACTIONS, KEY_LABELS, Place},
    model_picker::{self, ModelPurpose, PickerStamp},
    palette::{Item, Options, Palette, PaletteEvent},
    settings::SettingsTab,
    theme::*,
};
use gpui::{prelude::*, *};
use plait_client::host::{ModelChoice, harness::HarnessId};
use std::{collections::HashMap, time::Instant};

pub(super) enum Command {
    /// An application action, dispatched as its menu item or shortcut would be.
    Action(Box<dyn Action>),
    OpenThread(String),
    ReviewAll,
    ReviewRecord(plait_client::review::recorded::Target),
    SignIn(String),
    OpenProject(std::path::PathBuf),
    OpenRemoteProject(String),
    /// Show this folder of the remote machine in the open picker.
    BrowseRemote(std::path::PathBuf),
    NewThread(ModelChoice),
    /// A model chosen in the model picker. The picker's purpose gives it meaning.
    Model(ModelChoice),
    NamingModel {
        choice: ModelChoice,
        epoch: u64,
    },
    /// A model of a harness other than Pi, chosen in the model picker.
    HarnessModel(HarnessId, ModelChoice),
    /// Open Settings → Agents, where harnesses are listed.
    HarnessSettings,
    /// Open Settings → Agents and set up a harness there, where its progress
    /// and any failure are shown.
    SetUpHarness(HarnessId),
    Thinking {
        thread_id: String,
        epoch: u64,
        level: plait_client::host::ThinkingLevel,
    },
    /// A thread's own approval mode; `None` follows the default.
    Approval {
        thread_id: String,
        mode: Option<plait_client::host::ApprovalMode>,
    },
    /// A harness's own permission mode, chosen from a Permissions menu.
    Permissions {
        mode: String,
        select: super::permissions::OnPermissions,
    },
    /// The new-thread draft's project, harness, and Pi approvals.
    DraftProject(std::path::PathBuf),
    DraftHarness(HarnessId),
    DraftApproval(Option<plait_client::host::ApprovalMode>),
    /// The thinking level the draft's thread starts at.
    DraftThinking(plait_client::host::ThinkingLevel),
    /// The thinking level new threads start at.
    DefaultThinking(plait_client::host::ThinkingLevel),
    /// A checkout-relative path, from quick open.
    OpenFile(String),
    /// A local branch to switch to, from the branch picker.
    SwitchBranch(String),
    /// A new branch's name, as typed in the branch picker.
    CreateBranch(String),
    /// A thread to add the offered Linear issue to.
    IssueToThread(String),
    /// The workflow state to move the shown Linear issue to.
    IssueState(String),
}

impl Clone for Command {
    fn clone(&self) -> Self {
        match self {
            Self::Action(action) => Self::Action(action.boxed_clone()),
            Self::OpenThread(id) => Self::OpenThread(id.clone()),
            Self::ReviewAll => Self::ReviewAll,
            Self::ReviewRecord(target) => Self::ReviewRecord(target.clone()),
            Self::OpenFile(path) => Self::OpenFile(path.clone()),
            Self::SwitchBranch(name) => Self::SwitchBranch(name.clone()),
            Self::CreateBranch(name) => Self::CreateBranch(name.clone()),
            Self::IssueToThread(id) => Self::IssueToThread(id.clone()),
            Self::IssueState(id) => Self::IssueState(id.clone()),
            Self::SignIn(id) => Self::SignIn(id.clone()),
            Self::OpenProject(path) => Self::OpenProject(path.clone()),
            Self::OpenRemoteProject(path) => Self::OpenRemoteProject(path.clone()),
            Self::BrowseRemote(path) => Self::BrowseRemote(path.clone()),
            Self::NewThread(choice) => Self::NewThread(choice.clone()),
            Self::Model(choice) => Self::Model(choice.clone()),
            Self::NamingModel { choice, epoch } => Self::NamingModel {
                choice: choice.clone(),
                epoch: *epoch,
            },
            Self::HarnessModel(id, choice) => Self::HarnessModel(*id, choice.clone()),
            Self::HarnessSettings => Self::HarnessSettings,
            Self::SetUpHarness(id) => Self::SetUpHarness(*id),
            Self::Thinking {
                thread_id,
                epoch,
                level,
            } => Self::Thinking {
                thread_id: thread_id.clone(),
                epoch: *epoch,
                level: *level,
            },
            Self::Approval { thread_id, mode } => Self::Approval {
                thread_id: thread_id.clone(),
                mode: *mode,
            },
            Self::Permissions { mode, select } => Self::Permissions {
                mode: mode.clone(),
                select: select.clone(),
            },
            Self::DraftProject(path) => Self::DraftProject(path.clone()),
            Self::DraftHarness(id) => Self::DraftHarness(*id),
            Self::DraftApproval(mode) => Self::DraftApproval(*mode),
            Self::DraftThinking(level) => Self::DraftThinking(*level),
            Self::DefaultThinking(level) => Self::DefaultThinking(*level),
        }
    }
}

/// The palette or picker shown above the workspace. One at a time.
pub(super) struct Overlay {
    pub palette: Entity<Palette>,
    /// What a chosen model is for. `None` is the command palette.
    pub purpose: Option<ModelPurpose>,
    /// Where keyboard focus was, to put it back on Escape.
    restore: Option<FocusHandle>,
    /// The host state a model picker's rows were built from.
    pub stamp: Option<PickerStamp>,
    /// Quick open: its rows fill in when the file list arrives.
    pub files: bool,
    /// The branch picker: its rows fill in when the branches arrive.
    pub branches: bool,
    _events: Subscription,
}

/// The shortcut that runs `action` from the workspace, as the menu shows it.
fn shortcut(action: &dyn Action, window: &Window) -> String {
    let Ok(context) = KeyContext::parse("Plait") else {
        return String::new();
    };
    window
        .highest_precedence_binding_for_action_in_context(action, context)
        .map(|binding| {
            binding
                .keystrokes()
                .iter()
                .map(|keystroke| keystroke.display(KEY_LABELS).to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// Every action in the table that has a menu item, with the shortcut it is
/// bound to now. The palette lists itself nowhere: it is already open.
fn action_items(view: &Plait, window: &Window) -> Vec<Item> {
    ACTIONS
        .iter()
        .filter(|action| action.place != Place::Hidden && action.id != "palette.toggle")
        .map(|action| {
            let run = action.action();
            let hint = shortcut(run.as_ref(), window);
            let refusal = match action.id {
                "file.check_links"
                    if !view.workspace.file_viewer.rendered()
                        || view.selected_view() != super::Workspace::Files =>
                {
                    Some("Open a rendered Markdown file first.")
                }
                "pane.next" | "pane.previous" if view.overview.is_some() => {
                    Some("Close Every Thread to focus a pane.")
                }
                "pane.next" | "pane.previous" if view.settings_open || !view.panel_is_open() => {
                    Some("Open the panel first.")
                }
                _ => None,
            };
            Item::new(
                format!("action:{}", action.id),
                action.title,
                Command::Action(run),
            )
            .enabled(refusal.is_none())
            .detail(refusal.unwrap_or(action.category))
            .section("Commands")
            .hint(hint)
        })
        .collect()
}

impl Plait {
    /// Threads of the open checkout, by title, with the Cmd-number that reaches each.
    fn thread_items(&self, cx: &App) -> Vec<Item> {
        let indices = self.filtered_threads(cx);
        let target = self.preferences.value.jump_target;
        let mut numbers = HashMap::new();
        for number in 1..=9 {
            if let Some(jump) = plait_client::navigation::jump(
                self.lists(),
                &self.state.projects,
                self.host.connection.threads(),
                &indices,
                target,
                number,
            ) {
                numbers.entry(jump.thread).or_insert(number);
            }
        }
        let threads = self
            .host
            .connection
            .threads()
            .iter()
            .enumerate()
            .filter(|(_, thread)| self.navigation.archived || thread.archived != Some(true));
        threads
            .map(|(index, thread)| {
                let model = model_picker::model_label(&thread.model);
                let item = Item::new(
                    format!("thread:{}", thread.id),
                    self.summary_title(thread),
                    Command::OpenThread(thread.id.clone()),
                )
                // The folder's path is last: two projects of one name are
                // told apart, and searched, by where they are.
                .detail(format!(
                    "{} · {} · {model} · {}",
                    plait_client::state::project_name(&thread.checkout),
                    plait_client::navigation::state_label(thread),
                    thread.checkout.display()
                ))
                .section("Threads")
                .glyph("thread", colors().violet)
                .hint(
                    numbers
                        .get(&index)
                        .map(|number| self.shortcut_label(&format!("jump.{number}")))
                        .unwrap_or_default(),
                );
                if self.host.thread.selected.as_ref() == Some(&thread.id) {
                    item.badge("Open", colors().subtle)
                } else {
                    item
                }
            })
            .collect()
    }

    /// "New thread with <model>" for each signed-in model.
    fn new_thread_items(&self) -> Vec<Item> {
        let Some(status) = self
            .host
            .connection
            .status
            .as_ref()
            .filter(|_| self.host.connection.connected)
        else {
            return Vec::new();
        };
        model_picker::usable_models(&status.models, &self.preferences.value)
            .into_iter()
            .map(|model| {
                let choice = model.choice();
                let label = model_picker::model_label(&choice);
                let default = status.default_model.as_ref() == Some(&choice);
                let item = Item::new(
                    format!("new-thread:{label}"),
                    format!("New thread with {}", model.name),
                    Command::NewThread(choice),
                )
                .detail(label)
                .section("New thread")
                .glyph("plus", colors().mint);
                if default {
                    item.badge("Default", colors().mint)
                } else {
                    item
                }
            })
            .collect()
    }

    /// Everything the command palette lists, in the order shown before typing.
    fn command_items(&self, window: &Window, cx: &App) -> Vec<Item> {
        let mut items = self.thread_items(cx);
        let mut preferences = self.lists().clone();
        preferences.hidden.clear();
        for project in plait_client::navigation::projects(
            self.host.connection.threads(),
            &preferences,
            &self.host.creation.checkout,
            &self.active_project().machines[0],
            self.host_is_local(),
        ) {
            items.push(
                Item::new(
                    format!("project:{}", project.checkout.display()),
                    project.name,
                    Command::OpenProject(project.checkout.clone()),
                )
                .detail(project.checkout.display().to_string())
                .section("Projects")
                .glyph("folder", colors().mint),
            );
        }
        items.extend(action_items(self, window));
        items.extend(self.new_thread_items());
        items.extend(
            self.providers
                .rows
                .iter()
                .filter(|provider| provider.sign_in.is_some())
                .map(|provider| {
                    Item::new(
                        format!("sign-in:{}", provider.id),
                        format!("Sign in to {}", provider.name),
                        Command::SignIn(provider.id.clone()),
                    )
                    .section("Providers")
                }),
        );
        items.extend(super::harnesses::setup_items(self.harness_rows()));
        items
    }

    /// Cmd-K opens the palette, and closes it when it is already open.
    pub(super) fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.purpose.is_none());
        self.close_overlay(true, window, cx);
        if open {
            return;
        }
        let items = self.command_items(window, cx);
        let options = Options {
            name: "Command palette".into(),
            placeholder: "Search threads, commands, and models…".into(),
            empty: "Nothing to run yet.".into(),
            initial: None,
            mark: None,
            aside: None,
            note: None,
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
    }

    /// Show `palette` above the workspace and give it the keyboard.
    pub(super) fn present(
        &mut self,
        palette: Entity<Palette>,
        purpose: Option<ModelPurpose>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(true, window, cx);
        if let Some(anchor) = self.pending_anchor.take() {
            palette.update(cx, |palette, _| palette.anchor = Some(anchor));
        }
        let restore = window.focused(cx);
        let events = cx.subscribe_in(&palette, window, Self::on_palette_event);
        window.focus(&palette.focus_handle(cx), cx);
        self.overlay = Some(Overlay {
            palette,
            purpose,
            restore,
            stamp: purpose.map(|_| self.picker_stamp()),
            files: false,
            branches: false,
            _events: events,
        });
        cx.notify();
    }

    /// Remove the overlay. `restore` returns focus to where it was.
    pub(super) fn close_overlay(
        &mut self,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        if restore {
            // Shortcuts need a focused element inside the root view.
            window.focus(overlay.restore.as_ref().unwrap_or(&self.focus), cx);
        }
        cx.notify();
    }

    /// A remark in the open overlay's footer, such as a failed save.
    pub(super) fn overlay_note(&mut self, note: Option<SharedString>, cx: &mut Context<Self>) {
        if let Some(overlay) = &self.overlay {
            overlay
                .palette
                .update(cx, |palette, cx| palette.set_note(note, cx));
        }
    }

    fn on_palette_event(
        &mut self,
        palette: &Entity<Palette>,
        event: &PaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A palette that was already replaced can still report a late blur.
        let Some(overlay) = self
            .overlay
            .as_ref()
            .filter(|open| open.palette == *palette)
        else {
            return;
        };
        let purpose = overlay.purpose;
        match event {
            PaletteEvent::Dismiss { restore } => self.close_overlay(*restore, window, cx),
            PaletteEvent::Mark(Command::Model(choice)) => self.toggle_favorite_model(choice, cx),
            PaletteEvent::Mark(_) => {}
            // The picker stays open; its rows and footer report the save.
            PaletteEvent::Aside(Command::Model(choice)) => {
                self.choose_default_model(choice.clone(), window, cx)
            }
            PaletteEvent::Aside(_) => {}

            // Browsing a remote machine's folders keeps the picker open.
            PaletteEvent::Run(Command::BrowseRemote(path)) => {
                self.browse_remote((!path.as_os_str().is_empty()).then(|| path.clone()), cx)
            }
            PaletteEvent::Run(command) => {
                // Restore focus first, so an action runs where the user was.
                self.close_overlay(true, window, cx);
                self.run_command(command.clone(), purpose, window, cx);
            }
        }
    }

    pub(super) fn run_command(
        &mut self,
        command: Command,
        purpose: Option<ModelPurpose>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            Command::Action(action) => window.dispatch_action(action, cx),
            Command::SignIn(provider) => {
                self.show_settings(true, window, cx);
                self.settings_tab = SettingsTab::Providers;
                self.close_provider_form(cx);
                self.start_provider_sign_in(provider, cx);
            }
            Command::OpenThread(id) => {
                // The list can change while the palette is open.
                if self
                    .host
                    .connection
                    .threads()
                    .iter()
                    .any(|thread| thread.id == id)
                {
                    self.interaction = Some(Instant::now());
                    self.select_host_thread(id, window, cx);
                    self.focus_composer(window, cx);
                }
            }
            Command::OpenFile(path) => self.open_file_in_view(path.into(), window, cx),
            Command::SwitchBranch(name) => {
                self.branch_write(plait_client::git::WriteOp::SwitchBranch(name), window, cx)
            }
            Command::ReviewAll => {
                self.clear_turn_review();
                self.refresh_turn_review(cx);
                cx.notify();
            }
            Command::ReviewRecord(target) => self.open_turn_review(target, window, cx),
            Command::CreateBranch(name) => {
                self.branch_write(plait_client::git::WriteOp::CreateBranch(name), window, cx)
            }
            Command::DefaultThinking(level) => {
                let mut preferences = self.preferences.value.clone();
                preferences.default_thinking = Some(level);
                self.set_preferences(preferences, super::jump::PreferenceGroup::Models, cx);
            }
            Command::Thinking {
                thread_id,
                epoch,
                level,
            } => {
                if self.host.connection.epoch == epoch
                    && self.host.thread.selected.as_ref() == Some(&thread_id)
                {
                    self.configure_selected(None, Some(level), window, cx);
                }
            }
            Command::Approval { thread_id, mode } => {
                // The thread may have closed while the menu was open.
                if self
                    .host
                    .connection
                    .threads()
                    .iter()
                    .any(|thread| thread.id == thread_id)
                {
                    self.set_thread_approval(thread_id, mode, cx);
                }
            }
            Command::Permissions { mode, select } => select(self, mode, window, cx),
            Command::HarnessSettings => {
                self.show_settings(true, window, cx);
                self.settings_tab = SettingsTab::Agents;
            }
            Command::SetUpHarness(id) => {
                self.show_settings(true, window, cx);
                self.settings_tab = SettingsTab::Agents;
                // The list can change while the menu is open; a harness set
                // up meanwhile is not downloaded again.
                if self.harness_row(id).is_some_and(|row| row.setup) {
                    self.set_up_harness(id, cx);
                }
            }
            Command::DraftProject(path) => self.choose_draft_project(path, window, cx),
            Command::IssueToThread(id) => self.add_issue_to_thread(id, window, cx),
            Command::IssueState(id) => self.change_issue_state(&id, cx),
            Command::DraftHarness(id) => self.choose_draft_harness(id, cx),
            Command::DraftApproval(mode) => self.choose_draft_approval(mode, cx),
            Command::DraftThinking(level) => self.choose_draft_thinking(level, cx),
            Command::HarnessModel(id, choice) => match purpose {
                Some(ModelPurpose::NewThread) => self.choose_draft_model(id, choice, cx),
                Some(ModelPurpose::ThreadModel) => self.choose_thread_model(choice, window, cx),
                _ => {}
            },
            // Handled while the picker stays open; reaching here means it closed first.
            Command::BrowseRemote(_) => {}
            Command::OpenRemoteProject(path) => {
                if !self.host_is_local() && super::valid_remote_path(&path) {
                    let path = std::path::PathBuf::from(path);
                    self.update_lists(|lists| lists.open(&path), cx);
                    self.rebuild_projects(cx);
                    self.change_checkout(path, window, cx);
                }
            }
            Command::OpenProject(path) => {
                if self.host.creation.pending_create.is_none() && !self.host.creation.creating {
                    self.change_checkout(path, window, cx);
                }
            }
            Command::NewThread(choice) => {
                self.open_draft(window, cx);
                self.choose_draft_harness(HarnessId::Pi, cx);
                self.choose_draft_model(HarnessId::Pi, choice, cx);
            }
            Command::NamingModel { choice, epoch } => self.choose_naming_model(choice, epoch, cx),
            Command::Model(choice) => match purpose {
                Some(ModelPurpose::NewThread) => self.choose_draft_model(HarnessId::Pi, choice, cx),
                Some(ModelPurpose::DefaultModel) => self.choose_default_model(choice, window, cx),
                Some(ModelPurpose::ThreadModel) => self.choose_thread_model(choice, window, cx),
                None => {}
            },
        }
    }
}
