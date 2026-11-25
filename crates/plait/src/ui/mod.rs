pub(crate) mod a11y;
mod actions;
mod admission;
mod anchors;
mod appearance;
mod branches;
mod build_output;
mod changes;
mod checkout_chips;
mod checkout_queue;
mod cleanup;
mod commands;
mod commit_panel;
mod commits;
mod composer;
mod conversation;
mod conversation_find;
mod draft_tabs;
mod draft_titles;
mod file_view;
mod harnesses;
mod history;
mod host;
mod host_workspace;
mod images;
mod issue_text;
mod issues_view;
mod jump;
mod live_conversation;
mod machine_install;
mod machine_update;
mod markdown;
mod model_picker;
mod navigation;
mod navigation_host;
mod new_thread;
mod new_thread_view;
mod overview;
mod palette;
mod panel;
mod panels;
mod permissions;
mod remote_browser;
mod saved_work;
mod selection;
mod server_setup;
mod session;
mod settings;
mod settings_agent;
mod settings_editor;
mod settings_isolation;
mod settings_keyboard;
mod settings_linear;
mod settings_machines;
mod settings_modal;
mod settings_naming;
mod settings_policy;
mod settings_project;
mod settings_providers;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod snapshot;
mod tab_bookmark;
mod tab_identity;
mod tab_lifecycle;
mod tab_persistence;
mod tabs;
mod text_field;
pub mod theme;
mod thread_controls;
mod thread_files;
mod thread_workspace;
mod toasts;
mod tool_card;
mod turn_records;
mod turn_review;
mod update;
mod ways_out;
mod window_bar;
mod worktree;
mod worktrees_view;

use gpui::{prelude::*, *};
use plait_client::{
    host_state::HostState,
    model::DiffMode,
    state::{AppState, Load},
};
use std::{path::PathBuf, time::Instant};
use theme::*;
pub use window_bar::TRAFFIC_LIGHTS;

actions!(
    plait,
    [
        Quit,
        Close,
        Minimize,
        OpenRepository,
        Settings,
        ProviderSettings,
        ProjectSettings,
        CheckForUpdates,
        Refresh,
        ToggleSidebar,
        ToggleOverview,
        ToggleDiagnostics,
        Conversation,
        Changes,
        CommitHistory,
        ShowFiles,
        QuickOpen,
        FindInFile,
        FindNext,
        FindPrevious,
        StageAll,
        UnstageAll,
        CommitStaged,
        Push,
        Pull,
        Fetch,
        SwitchBranch,
        NewBranch,
        NextThread,
        NextThreadNeedingMe,
        PreviousThread,
        UndoLastTurn,
        Tab,
        BackTab,
        NextFile,
        PreviousFile,
        NewThread,
        NewThreadWithModel,
        NewWorktreeThread,
        SwitchThreadModel,
        CommandPalette,
        ToggleInspector,
        FillPanel,
        FocusNextPane,
        FocusPreviousPane,
        ToggleChangedFiles,
        RenameThread,
        ArchiveThread,
        ToggleArchived,
        FilterThreads,
        ToggleNotifications,
        RefreshThreadList,
        ShowHiddenProjects,
        DenyOnce,
        AllowOnce
    ]
);

/// The workspace beside the sidebar (presentation-independent view kind from core).
pub(super) use plait_client::thread_view::ThreadView as Workspace;

/// Breakpoint for the wider workspace layouts.
const EXPANDED_CHROME_WIDTH: f32 = 1150.;

pub struct Plait {
    tabs: tabs::Tabs,
    state: AppState,
    host: HostState,
    host_client: plait_client::host::Client,
    host_local: bool,
    navigation: navigation::Navigation,
    overview: Option<overview::OverviewView>,
    host_workspace: host_workspace::HostWorkspace,
    /// What a window on a machine with no projects yet shows as its project.
    no_project: plait_client::model::Project,
    /// Presentation state for the single selected thread workspace.
    workspace: thread_workspace::ThreadWorkspace,
    commit_sends:
        std::collections::HashMap<plait_client::state::CheckoutId, commit_panel::CommitAttempt>,
    /// Window modal, independent of the thread workspace.
    settings_open: bool,
    settings_page: settings::SettingsPage,
    settings_focus: FocusHandle,
    settings_restore: Option<FocusHandle>,
    settings_project: Option<settings_project::ProjectSettings>,
    settings_tab: settings::SettingsTab,
    /// Shared by both pages: opening either starts it at the top.
    settings_scroll: ScrollHandle,
    keyboard: settings_keyboard::KeyboardState,
    /// Settings → Editor: the custom command fields.
    editor_settings: settings_editor::EditorSettings,
    /// Open in Editor, Show in Finder, and Open Terminal Here.
    ways_out: ways_out::WaysOut,
    policy: settings_policy::PolicyState,
    cleanup: cleanup::CleanupState,
    agent: settings_agent::AgentState,
    harnesses: harnesses::HarnessesState,
    providers: settings_providers::ProviderState,
    naming: plait_client::host::thread_naming::State,
    linear: settings_linear::LinearState,
    issues: issues_view::IssuesUi,
    machines: settings_machines::MachinesUi,
    /// A thread was created at once (a worktree thread): open it when the host
    /// confirms creation, unless the user has gone elsewhere in the meantime.
    new_thread_open: bool,
    /// The new thread being written, before anything exists on the host.
    draft: new_thread::Draft,
    /// A worktree is being created or removed.
    admission: admission::State,
    build_output: build_output::State,
    worktree_busy: bool,
    worktrees: worktrees_view::View,
    worktree_pending: Option<worktree::Pending>,
    approval_keys: live_conversation::ApprovalKeys,
    /// The command palette or model picker, above the workspace.
    overlay: Option<commands::Overlay>,
    /// Where pickable controls were last drawn, so a picker opens beside its control.
    anchors:
        std::rc::Rc<std::cell::RefCell<std::collections::HashMap<&'static str, Bounds<Pixels>>>>,
    /// The control the next picker belongs to.
    pending_anchor: Option<Bounds<Pixels>>,
    /// Whether the open remote-folder picker's host can list folders.
    remote_browser: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    remote_browser_generation: u64,
    remote_listing: Option<plait_client::host::folders::Listing>,
    panels: panels::Panels,
    focus: FocusHandle,
    /// The conversation field that gets focus back after changing views.
    panel_focus: [FocusHandle; 4],
    conversation_focus: Option<FocusHandle>,
    /// The window control that had focus before conversation find opened.
    conversation_find_focus: Option<FocusHandle>,
    sidebar: bool,
    /// A left press on the window bar may become a window move.
    window_drag: bool,
    diagnostics: bool,
    preferences: jump::PreferenceState,
    themes: appearance::Themes,
    code_font: appearance::CodeFont,
    updates: update::UpdateState,
    server_setup: server_setup::ServerSetup,
    toasts: plait_client::toasts::Toasts,
    toast_timer: Option<Task<()>>,
    session: session::SessionState,
    saved_work: saved_work::SavedWork,
    picker_open: bool,
    notice: Option<String>,
    machine_restore_notice: Option<String>,
    last_render_us: u128,
    max_render_us: u128,
    startup: Instant,
    first_render_ms: Option<u128>,
    interaction: Option<Instant>,
    switch_us: u128,
}

impl Plait {
    fn host_client(&self) -> plait_client::host::Client {
        self.host_client.clone()
    }
    fn host_is_local(&self) -> bool {
        self.host_local
    }
    /// The open project is a plain folder, as its last status read found.
    pub(super) fn plain_folder_open(&self) -> bool {
        self.state
            .repository
            .snapshot()
            .ready()
            .is_some_and(|snapshot| snapshot.plain)
    }

    /// Repository operations run on the selected machine’s host.
    pub(super) fn repository_backend(&self) -> plait_client::state::Backend {
        plait_client::state::Backend::Host(self.host_client())
    }
    #[allow(dead_code)]
    fn use_machine_client(
        &mut self,
        client: plait_client::host::Client,
        machine: plait_client::machines::MachineId,
        local: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let another = self.machines.active != machine;
        if another {
            // Save this machine's work and tab document, with its own client and
            // locality, and put its workspace away before anything changes: a
            // remote folder must not be saved as this computer's.
            self.capture_session(cx);
            self.flush_machine_tabs(window, cx);
            self.park_workspace(cx);
            self.forget_saved_work();
        }
        self.host_client = client;
        self.host_local = local;
        // A new link to the same machine keeps what is on screen; another
        // machine starts empty, so nothing of the previous one shows as its.
        if another {
            self.machines.active = machine;
            self.host.connection.threads_mut().clear();
            self.navigation.available.clear();
            self.host_workspace = self.host_workspace.for_another_machine();
            self.switch_machine_tabs(window, cx);
            self.rebuild_projects(cx);
        }
        self.update_window_title(window);
        // This client was just made: the reconnect below must not make another.
        self.machines.fresh_link = true;
        self.host.connection.connecting = false;
        self.reconnect_host(window, cx);
    }
    pub fn new(
        path: PathBuf,
        startup: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_host_client(
            path,
            startup,
            plait_client::host::Client::local_or_unavailable(),
            window,
            cx,
        )
    }

    fn with_host_client(
        path: PathBuf,
        startup: Instant,
        host_client: plait_client::host::Client,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let host = HostState::new(path.clone());
        let state = AppState::new(path);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let commit_message =
            cx.new(|cx| text_field::TextField::new("Commit message", "Commit message", cx));
        let mut this = Self {
            tabs: Default::default(),
            state,
            host,
            host_client,
            host_local: true,
            navigation: navigation::Navigation::new(),
            overview: None,
            host_workspace: Default::default(),
            no_project: plait_client::model::Project {
                checkout: PathBuf::new(),
                name: "No project".into(),
                machines: vec![plait_client::model::Machine {
                    name: plait_client::machine::local_name(),
                    local: true,
                }],
            },
            workspace: thread_workspace::ThreadWorkspace::new(commit_message),
            commit_sends: Default::default(),
            settings_open: false,
            settings_page: Default::default(),
            settings_focus: cx.focus_handle(),
            settings_restore: None,
            settings_project: None,
            settings_tab: Default::default(),
            settings_scroll: ScrollHandle::new(),
            keyboard: Default::default(),
            editor_settings: Default::default(),
            ways_out: Default::default(),
            policy: Default::default(),
            cleanup: Default::default(),
            agent: Default::default(),
            harnesses: Default::default(),
            providers: Default::default(),
            naming: Default::default(),
            linear: Default::default(),
            issues: Default::default(),
            machines: Default::default(),
            new_thread_open: false,
            draft: Default::default(),
            admission: Default::default(),
            build_output: Default::default(),
            worktree_busy: false,
            worktrees: Default::default(),
            worktree_pending: None,
            approval_keys: Default::default(),
            overlay: None,
            anchors: Default::default(),
            pending_anchor: None,
            remote_browser: None,
            remote_browser_generation: 0,
            remote_listing: None,
            panels: Default::default(),
            focus,
            panel_focus: std::array::from_fn(|_| cx.focus_handle()),
            conversation_focus: None,
            conversation_find_focus: None,
            sidebar: true,
            window_drag: false,
            diagnostics: false,
            preferences: jump::PreferenceState::default(),
            themes: Default::default(),
            code_font: Default::default(),
            updates: update::UpdateState::default(),
            server_setup: server_setup::ServerSetup::default(),
            toasts: Default::default(),
            toast_timer: None,
            session: session::SessionState::default(),
            saved_work: Default::default(),
            picker_open: false,
            notice: None,
            machine_restore_notice: None,
            last_render_us: 0,
            max_render_us: 0,
            startup,
            first_render_ms: None,
            interaction: None,
            switch_us: 0,
        };
        this.workspace
            .live_scroll
            .set_scroll_handler(cx.listener(Self::scroll_history));
        this.initialize_navigation(window, cx);
        this.pump(cx);
        this.load_preferences(cx);
        this.observe_appearance(window, cx);
        this.apply_theme(cx);
        this.apply_code_size(cx);
        this.load_themes(cx);
        // An open picker shows host state; keep it current as replies land.
        cx.observe_self(|this, cx| {
            this.sync_picker(cx);
            this.sync_policy(cx);
            this.sync_cleanup(cx);
            this.sync_admission_capability(cx);
            this.sync_admission_context(cx);
            this.sync_agent(cx);
            this.sync_project_settings(cx);
            this.sync_harnesses(cx);
            this.sync_provider_settings(cx);
            this.sync_naming_settings(cx);
            this.sync_linear_settings(cx);
            this.sync_issues_view(cx);
            this.sync_issue_links(cx);
        })
        .detach();
        // The machine saved last time, once preferences have loaded.
        let entity = cx.entity();
        cx.observe_in(&entity, window, |this, _, window, cx| {
            this.restore_machine(window, cx)
        })
        .detach();
        // Keys dispatch from the focused element. One that is no longer drawn,
        // such as a field on a tab that was left, sends them from outside the
        // app's key context, where no shortcut or menu action is handled.
        cx.on_focus_lost(window, |this, window, cx| {
            window.focus(
                if this.settings_open {
                    &this.settings_focus
                } else {
                    &this.focus
                },
                cx,
            )
        })
        .detach();
        this.watch_panel_focus(window, cx);
        this.keep_focus_in_settings(window, cx);
        this.watch_approval_keys(window, cx);
        this.reconnect_host(window, cx);
        // Test scenes stay offline.
        if !cfg!(test) {
            this.check_for_updates(true, cx);
        }
        this
    }

    fn pump(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.state.next_repository_job() else {
            return;
        };
        // A remote machine's checkouts are read and changed by its host.
        let backend = self.repository_backend();
        let task = cx.background_executor().spawn(async move {
            let reply = job.run_on(&backend);
            (job, reply)
        });
        cx.spawn(async move |this, cx| {
            let (job, reply) = task.await;
            let _ = this.update(cx, |this, cx| {
                let status = matches!(job.job, plait_client::state::Job::Status { .. });
                let write = matches!(job.job, plait_client::state::Job::Write { .. });
                let owner = job.owner;
                let active = this.state.complete_repository_job(job, reply);
                if write {
                    this.after_git_write(owner, cx);
                }
                if !active {
                    this.pump(cx);
                    return;
                }
                if write {
                    this.refresh_turn_review(cx);
                }
                this.sync_recorded_current(cx);
                if status {
                    this.count_panel_files(cx);
                    this.thread_files_status(cx);
                    this.check_file(cx);
                }
                this.search_panel_diff(cx);
                this.normalize_checkout(cx);
                this.bind_selected_checkout();
                this.deliver_files(cx);
                this.deliver_quick_open(cx);
                this.deliver_branches(cx);
                this.restore_ready_bookmarks(cx);
                this.pump(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn bind_selected_checkout(&mut self) -> bool {
        let Some(id) = self.host.thread.selected.clone() else {
            return true;
        };
        let Some(key) = self.state.checkout_key() else {
            return false;
        };
        // A thread is bound only to its own checkout. While the window still
        // shows another one, wait for the thread's checkout to load.
        if self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .is_some_and(|thread| thread.checkout != key.root)
        {
            return false;
        }
        match self.host.bind_checkout(&id, key) {
            Ok(()) => true,
            Err(error) => {
                self.notice = Some(error);
                false
            }
        }
    }

    /// The open checkout is known to differ from the selected thread's. An
    /// identity not read yet is not a mismatch: Git status may be slow or
    /// fail, and a send verifies the bound directory before it leaves.
    fn checkout_mismatch(&self) -> bool {
        let Some(id) = self.host.thread.selected.as_deref() else {
            return false;
        };
        match (self.state.checkout_key(), self.host.expected_checkout(id)) {
            (Some(key), Some(expected)) => key != expected,
            _ => false,
        }
    }

    fn checkout_matches_thread(&self) -> bool {
        let Some(id) = self.host.thread.selected.as_deref() else {
            return true;
        };
        self.state
            .checkout_key()
            .is_some_and(|key| self.host.expected_checkout(id) == Some(key))
    }

    /// Move to the next or previous host thread, wrapping. With no selection,
    /// forward starts at the first thread and backward at the last.
    fn step_thread(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let indices = self.filtered_threads(cx);
        let count = indices.len();
        if count == 0 {
            return;
        }
        let current = self.host.thread.selected.as_ref().and_then(|id| {
            indices
                .iter()
                .position(|&index| &self.host.connection.threads_mut()[index].id == id)
        });
        let next = match (current, forward) {
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => (index + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        self.interaction = Some(std::time::Instant::now());
        let id = self.host.connection.threads()[indices[next]].id.clone();
        self.select_host_thread(id, window, cx);
    }

    fn select_file(
        &mut self,
        file: usize,
        mode: DiffMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same_recorded_file = self.workspace.turn_review.current
            && self.recorded_file().is_some_and(|recorded| {
                self.state
                    .repository
                    .snapshot()
                    .ready()
                    .and_then(|s| s.changes.get(file))
                    .is_some_and(|current| current.path == recorded.path)
            });
        if !same_recorded_file {
            self.clear_turn_review();
        }
        self.show_file(file, mode, None, window, cx);
    }

    /// `step` carries the direction when moving through the list by keyboard,
    /// which reads the following file ahead.
    fn show_file(
        &mut self,
        file: usize,
        mode: DiffMode,
        step: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ensure_view_tab() {
            return;
        }
        self.workspace.restore_pending[1] = false;
        window.focus(&self.focus, cx);
        self.interaction = Some(Instant::now());
        match step {
            Some(forward) => self.state.repository.step(file, mode, forward),
            None => self.state.repository.select(file, mode),
        }
        self.workspace.diff_scroll = UniformListScrollHandle::new();
        if let Some(panel) = self.tabs.model.panel_mut() {
            panel.open_on(plait_client::panel::PanelKind::Changes);
            panel.select_scope(plait_client::panel::Scope::Uncommitted);
        }
        self.workspace.tab = Workspace::Conversation;
        self.panels.focused = Some(plait_client::panes::PaneKind::Changes);
        window.focus(
            &self.panel_focus[plait_client::panes::PaneKind::Changes as usize],
            cx,
        );
        self.refresh_turn_review(cx);
        self.pump(cx);
        cx.notify();
    }

    fn step_file(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(record) = &self.workspace.turn_review.record {
            let count = record.files.len();
            if count > 0 {
                let selected = self.workspace.turn_review.selected.unwrap_or(0);
                let next = if forward {
                    (selected + 1) % count
                } else {
                    (selected + count - 1) % count
                };
                self.workspace
                    .file_scroll
                    .scroll_to_item(next, ScrollStrategy::Top);
                self.select_recorded_file(next, window, cx);
            }
            return;
        }
        let repo = &self.state.repository;
        let Some(snapshot) = repo.snapshot().ready() else {
            return;
        };
        let count = snapshot.changes.len();
        if count == 0 {
            return;
        }
        let current = repo.selected_diff().as_ref().map_or(0, |s| s.file);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        let mode = if snapshot.changes[next].unstaged() {
            DiffMode::Unstaged
        } else {
            DiffMode::Staged
        };
        self.workspace
            .file_scroll
            .scroll_to_item(next, ScrollStrategy::Top);
        self.show_file(next, mode, Some(forward), window, cx);
    }

    fn refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktrees_visible() {
            self.refresh_worktrees_view(true, cx);
            return;
        }
        window.focus(&self.focus, cx);
        self.fetch_turn_records(window, cx);
        self.refresh_turn_review(cx);
        if !matches!(self.state.repository.snapshot(), Load::Loading) {
            self.state.repository.refresh();
            self.pump(cx);
            cx.notify();
        }
    }

    fn open_repository(&mut self, _: &OpenRepository, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.picker_open {
            return;
        }
        if self.host.creation.pending_create.is_some() || self.host.creation.creating {
            self.notice =
                Some("Resolve the pending thread creation before changing checkout.".into());
            cx.notify();
            return;
        }
        if !self.host_is_local() {
            self.open_remote_browser(window, cx);
            return;
        }
        self.picker_open = true;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            // Opening only reads the folder; threads and Git writes come later, by choice.
            prompt: Some("Open project".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.picker_open = false;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.change_checkout(path, window, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        this.notice = Some(format!("Cannot open folder picker: {error}"))
                    }
                    Err(error) => {
                        this.notice = Some(format!("Folder picker closed unexpectedly: {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Plait {
    /// Open the application's Settings, or close whichever settings page is open.
    fn show_settings(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_page(
            open.then_some(settings::SettingsPage::Application),
            window,
            cx,
        );
    }

    /// Open the active project's settings page.
    pub(super) fn show_project_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings_for_project(self.visible_project_checkout().to_owned(), window, cx);
    }

    /// Cmd-comma: the application page closes when it is the one open;
    /// from anywhere else, including the project page, it opens.
    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.settings_open && self.settings_page == settings::SettingsPage::Application;
        self.show_settings(!open, window, cx);
    }

    /// Show `page` over the workspace, or close settings with `None`.
    fn open_page(
        &mut self,
        page: Option<settings::SettingsPage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(true, window, cx);
        self.cancel_harness_path();
        if page.is_some() && !self.settings_open {
            self.settings_restore = window.focused(cx);
        }
        self.settings_open = page.is_some();
        self.dismiss_tab_controls();
        self.cover_workspace(window, cx);
        if self.settings_open {
            window.focus(&self.settings_focus, cx);
        } else {
            let restore = self
                .settings_restore
                .take()
                .filter(|focus| focus.is_rendered(window));
            window.focus(restore.as_ref().unwrap_or(&self.focus), cx);
        }
        self.settings_page = page.unwrap_or_default();
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
        self.cancel_recording(cx);
        match page {
            Some(settings::SettingsPage::Application) => {
                self.prepare_keyboard(cx);
                self.prepare_code_font(cx);
                self.prepare_editor_settings(cx);
                self.prepare_providers(cx);
                self.load_naming_settings(cx);
            }
            Some(settings::SettingsPage::Project) => {
                self.close_provider_form(cx);
                // Opened fresh, the page asks before trusting again.
                self.project_policy_mut().confirm = false;
                // Pi's files can change while the app is open.
                self.refresh_project_agent(cx);
            }
            None => self.close_provider_form(cx),
        }
        if page.is_none()
            && self.workspace.tab == Workspace::Conversation
            && let Some(id) = self.host.thread.selected.clone()
        {
            self.mark_thread_read(&id, cx);
        }
        cx.notify();
    }

    fn wide(window: &Window) -> bool {
        window.viewport_size().width > px(EXPANDED_CHROME_WIDTH)
    }

    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the shortcut context alive when its focused child is removed.
        window.focus(self.overview_focus().unwrap_or(&self.focus), cx);
        self.sidebar = !self.sidebar;
        cx.notify();
    }

    fn show_tab(&mut self, tab: Workspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktrees_visible() || !self.ensure_view_tab() {
            return;
        }
        if tab != Workspace::Conversation && !self.panel_is_open() {
            self.leave_conversation_focus(window, cx);
        }
        let Some(panel) = self.tabs.model.panel_mut() else {
            return;
        };
        use plait_client::panel::PanelKind;
        match tab {
            Workspace::Conversation => panel.close(),
            Workspace::Changes => panel.open_on(PanelKind::Changes),
            Workspace::History => panel.open_on(PanelKind::History),
            Workspace::Files => panel.open_on(PanelKind::Files),
        }
        self.present_workspace(Workspace::Conversation, window, cx);
        if tab == Workspace::Changes && self.workspace.turn_review.target.is_none() {
            self.sync_panel_scope(self.panel_recorded(), window, cx);
        }
        if tab == Workspace::Conversation {
            window.focus(&self.focus, cx);
            self.return_conversation_focus(window, cx);
        } else {
            self.panels.focused = Some(self.panel_focus_kind());
            window.focus(&self.panel_focus[self.panel_focus_kind() as usize], cx);
        }
    }

    // Restoring a tab presents its content without moving its remembered pane focus.
    fn present_workspace(&mut self, tab: Workspace, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overview(window, cx);
        if self.worktrees_visible() {
            return;
        }
        if !self.ensure_view_tab() {
            cx.notify();
            return;
        }
        if tab == Workspace::Conversation && self.draft.open && self.draft.composer.is_none() {
            self.open_draft_content(window, cx);
        }
        self.tabs.lifecycle.view_revision += 1;
        let conversation = Workspace::Conversation;
        let previous = self.workspace.tab;
        if self.workspace.tab == conversation && tab != conversation {
            self.pause_conversation(Some(window), cx);
            self.leave_conversation_focus(window, cx);
        } else if self.workspace.tab != conversation && tab == conversation {
            self.return_conversation_focus(window, cx);
        }
        self.workspace.tab = tab;
        if tab == conversation
            && previous != conversation
            && self.host.connection.connected
            && !cfg!(test)
            && let Some(id) = self.host.thread.selected.clone()
        {
            self.watch_thread(id, window, cx);
        }
        self.queue_tab_save(cx);
        self.load_workspace_locations(cx);
        if tab == Workspace::Conversation
            && self.conversation_snapshot().is_some()
            && window.is_window_active()
            && !self.settings_open
            && let Some(id) = self.host.thread.selected.clone()
        {
            self.mark_thread_read(&id, cx);
        }
        cx.notify();
    }

    fn load_workspace_locations(&mut self, cx: &mut Context<Self>) {
        let tab = self.selected_view();
        if tab == Workspace::Changes {
            if !self.panel_recorded()
                && self.workspace.restore_pending[1]
                && let Some(saved) = self.workspace.restore.as_ref()
                && let Some(path) = saved.changes.path.as_ref()
            {
                self.state.repository.restore_path(
                    path.into(),
                    if saved.changes.staged {
                        DiffMode::Staged
                    } else {
                        DiffMode::Unstaged
                    },
                );
                self.pump(cx);
            }
            self.refresh_turn_review(cx);
        }
        if tab == Workspace::History {
            self.state.repository.request_history();
            if self.workspace.restore_pending[2]
                && let Some(commit) = self
                    .workspace
                    .restore
                    .as_ref()
                    .and_then(|b| b.history.commit.clone())
            {
                self.state.repository.select_commit(commit);
            }
            self.pump(cx);
        } else {
            self.state.repository.leave_history();
        }
        if tab == Workspace::Files {
            self.check_file(cx);
        }
    }

    /// Where the conversation can hold the keyboard: its composer, and its
    /// find field while the bar is open.
    fn conversation_fields(&self, cx: &App) -> [Option<FocusHandle>; 2] {
        let find = &self.workspace.conversation_find;
        [
            self.visible_composer()
                .map(|composer| composer.focus_handle(cx)),
            find.field
                .as_ref()
                .filter(|_| find.open)
                .map(|field| field.focus_handle(cx)),
        ]
    }

    /// Another tab hides the conversation: the keyboard leaves its fields for
    /// the root, which every tab draws, and they remember it for the return.
    fn leave_conversation_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.conversation_focus = self
            .conversation_fields(cx)
            .into_iter()
            .flatten()
            .find(|field| field.contains_focused(window, cx));
        if self.conversation_focus.is_some() {
            window.focus(&self.focus, cx);
        }
    }

    /// The conversation is shown again: give the keyboard back to the field
    /// that had it, if it is still there.
    fn return_conversation_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.conversation_focus.take() else {
            return;
        };
        if !self.settings_open && self.conversation_fields(cx).contains(&Some(field.clone())) {
            window.focus(&field, cx);
        }
    }

    fn header(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar;
        let current_thread = !self.worktrees_visible()
            && !self.issues_visible()
            && self.host.thread.selected.is_some();
        let inset = if sidebar {
            16.
        } else {
            tabs::lights_inset(window.is_fullscreen())
        };
        let mut header = self
            .window_drag(row(), cx)
            .id("sheet-header")
            // The body is padded to clear the sheet corners. The unfilled header meets its edges.
            .when(sidebar, |el| el.mx(px(-4.)).mt(px(-4.)))
            .h(px(tabs::HEADER_HEIGHT))
            .flex_shrink_0()
            .min_w_0()
            .pl(px(inset))
            .pr(px(10.))
            .gap(px(8.))
            .border_b_1()
            .border_color(rgb(colors().border))
            .when(!sidebar, |el| el.child(self.sidebar_toggle(cx)));

        // Drafts, Worktrees and Linear issues keep their own title in both
        // settings; Show All Tabs and the tab keys still reach every tab.
        if current_thread && self.preferences.value.shows_thread_tabs() {
            header = header.child(self.tab_row(window, cx));
        } else {
            if self.tabs.list_open {
                self.measure_tabs(window);
            }
            let title = self.header_title();
            header = if self.tabs.model.active().is_some() && self.panels.viewport >= 760. {
                header.child(self.checkout_context(Some(title), cx).flex_1().min_w_0())
            } else {
                header
                    .child(checkout_chips::title_label(title))
                    .child(div().flex_1().min_w(px(16.)))
            };
        }
        header
            .children(self.cleanup_control(cx))
            .when_some(
                self.header_state().filter(|_| current_thread),
                |el, (word, tint, mark)| {
                    el.child(
                        row()
                            .id("header-state")
                            .h(px(20.))
                            .flex_shrink_0()
                            .px(px(7.))
                            .gap(px(4.))
                            .rounded_full()
                            .bg(rgba((tint << 8) | 36))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_nowrap()
                            .child(mark)
                            .child(tag("state-word", word, tint)),
                    )
                },
            )
            .when(current_thread, |el| {
                el.child(self.header_panel_toggle(cx))
                    .children(self.header_git_step(cx))
            })
    }

    fn current_git_step(&self) -> Option<(plait_client::git_step::GitStep, Option<&'static str>)> {
        use plait_client::git_step::{GitStep, next_git_step, sync};
        let repository = &self.state.repository;
        let snapshot = repository.snapshot().ready()?;
        if snapshot.plain || snapshot.root != *self.visible_project_checkout() {
            return None;
        }
        let (step, blocker) = next_git_step(
            snapshot.changes.len(),
            repository.staged(),
            &sync(snapshot.named, snapshot.tracking.as_ref()),
        )?;
        let blocker = if step != GitStep::Commit && repository.write().busy() {
            Some("Git is still working.")
        } else {
            blocker
        };
        Some((step, blocker))
    }

    fn header_git_step(&self, cx: &mut Context<Self>) -> Option<Control> {
        use plait_client::git_step::GitStep;
        let (step, blocker) = self.current_git_step()?;
        let tab = self.tabs.model.active();
        let owner = self.state.checkout_id();
        let epoch = self.state.repository.epoch();
        let label = match step {
            GitStep::Commit => "Commit…",
            GitStep::Publish => "Publish",
            GitStep::Push => "Push",
            GitStep::Pull => "Pull",
        };
        Some(
            toned_button("header-git-step", label, Tone::Primary)
                .h(px(24.))
                .flex_shrink_0()
                .when_some(blocker, |el, reason| {
                    el.aria_description(reason).tooltip(move |_, cx| {
                        cx.new(|_| thread_controls::TextTooltip(reason.into()))
                            .into()
                    })
                })
                .disabled(blocker.is_some())
                .on_activate(cx, move |this, window, cx| {
                    if this.tabs.model.active() != tab
                        || this.state.checkout_id() != owner
                        || this.state.repository.epoch() != epoch
                        || this.current_git_step() != Some((step, None))
                    {
                        return;
                    }
                    if step == GitStep::Commit {
                        if let Some(panel) = this.tabs.model.panel_mut() {
                            panel.select_scope(plait_client::panel::Scope::Uncommitted);
                        }
                        this.show_tab(Workspace::Changes, window, cx);
                        if this.workspace.turn_review.target.is_some() {
                            this.choose_panel_scope(false, window, cx);
                        }
                        window.focus(&this.workspace.commit_message.focus_handle(cx), cx);
                    } else {
                        this.push(cx);
                    }
                }),
        )
    }

    fn header_title(&self) -> String {
        if self.worktrees_visible() {
            "Worktrees".to_owned()
        } else if self.issues_visible() {
            "Linear issues".to_owned()
        } else if let Some(id) = &self.host.thread.selected {
            self.thread_title(id)
        } else if self.draft.open {
            self.current_draft_title().to_owned()
        } else {
            self.active_project().name.clone()
        }
        .replace(['\r', '\n'], " ")
    }

    fn header_state(&self) -> Option<(&'static str, u32, AnyElement)> {
        use plait_client::navigation::{ThreadRowState::*, thread_row_state};
        let id = self.host.thread.selected.as_deref()?;
        let summary = self.host.connection.threads().get(self.thread_index(id)?)?;
        let state = thread_row_state(summary, &self.lists().unread);
        match state {
            Working | Approval | Failed | Interrupted => {
                let tint = thread_row_colors(state, true).0;
                let mark = if state == Working && self.host.connection.connected {
                    running_mark(px(13.)).into_any_element()
                } else {
                    dot(tint).into_any_element()
                };
                Some((state.spoken(summary), tint, mark))
            }
            _ => None,
        }
    }

    fn header_panel_toggle(&self, cx: &mut Context<Self>) -> Control {
        let open = self.panel_is_open();
        let keys = self.shortcut_label("view.toggle_inspector");
        let totals = self.workspace.panel_change_total.as_ref().filter(|_| {
            self.state
                .repository
                .snapshot()
                .ready()
                .is_some_and(|snapshot| {
                    snapshot.root == self.host.creation.checkout
                        && !snapshot.plain
                        && !snapshot.changes.is_empty()
                })
        });
        let tip = format!("Toggle Panel  {keys}").trim_end().to_owned();
        toned_button(
            "header-panel-toggle",
            "",
            if open { Tone::Selected } else { Tone::Ghost },
        )
        .map(|el| self.covered_tab_control(el))
        .h(px(28.))
        .px(px(7.))
        .gap_1()
        .aria_label("Toggle Panel")
        .aria_toggled(if open { Toggled::True } else { Toggled::False })
        .aria_keyshortcuts(self.shortcut_aria("view.toggle_inspector"))
        .tooltip(move |_, cx| cx.new(|_| thread_controls::TextTooltip(tip.clone())).into())
        .child(div().text_size(px(16.)).child("±"))
        .when_some(totals, |el, totals| {
            el.aria_description(format!("Uncommitted changes {totals}"))
                .child(tag("header-panel-totals", totals.clone(), colors().muted))
        })
        .on_activate(cx, |this, window, cx| this.toggle_panel(window, cx))
    }

    /// Idle checkout context and the model and policy reported by older hosts.
    pub(super) fn connection_details(
        &self,
        model: Option<String>,
        note: Option<(&'static str, u32)>,
    ) -> impl IntoElement {
        // Host threads belong to this machine's checkout.
        let project = self.active_project();
        let machine = &project.machines[0];
        let branch = self
            .state
            .repository
            .snapshot()
            .ready()
            .map(|snapshot| snapshot.branch.clone());
        let chip = |id: &'static str, glyph: &'static str, label: String| {
            row()
                .gap(px(5.))
                .min_w_0()
                .child(icon(glyph).size(px(12.)).text_color(rgb(colors().subtle)))
                .child(div().truncate().child(a11y_text(id, label)))
        };
        row()
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .px_1()
            .gap_4()
            .text_size(px(11.))
            .text_color(rgb(colors().muted))
            .when(self.host.thread.selected.is_none(), |el| {
                el.child(chip("environment-project", "folder", project.name.clone()))
            })
            .child(chip("environment-machine", "machine", machine.name.clone()))
            .when_some(
                branch.filter(|_| self.host.thread.selected.is_none()),
                |el, branch| el.child(chip("environment-branch", "branch", branch)),
            )
            .when_some(model, |el, model| {
                el.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(colors().subtle))
                        .child(a11y_text("environment-model", model)),
                )
            })
            .child(div().flex_1())
            .when_some(note, |el, (note, tint)| {
                el.child(
                    tag("environment-note", note, tint)
                        .flex_shrink_0()
                        .whitespace_nowrap(),
                )
            })
    }

    fn diagnostics_bar(&self) -> impl IntoElement {
        row()
            .h(px(24.))
            .px_3()
            .flex_shrink_0()
            .justify_end()
            .bg(rgb(colors().rail))
            .border_t_1()
            .border_color(rgb(colors().border))
            .text_size(px(TYPE_META_SIZE))
            .font_features(tabular_nums())
            .text_color(rgb(colors().subtle))
            .child(a11y_text(
                "diagnostics-summary",
                format!(
                    "UI {} µs · peak {} µs · switch {} µs · first {} ms",
                    self.last_render_us,
                    self.max_render_us,
                    self.switch_us,
                    self.first_render_ms.unwrap_or(0)
                ),
            ))
    }
}

impl Render for Plait {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let start = Instant::now();
        self.first_render_ms
            .get_or_insert_with(|| self.startup.elapsed().as_millis());
        if let Some(interaction) = self.interaction.take() {
            self.switch_us = interaction.elapsed().as_micros();
        }
        let wide = Self::wide(window);
        self.cover_workspace(window, cx);
        self.panels.viewport = f32::from(window.viewport_size().width);
        self.advance_reveals(window);
        self.settle_find_reveal(window);
        let real = self.host.thread.selected.is_some();
        let drafting = !real && self.draft.open;
        let sidebar = self.sidebar;
        let show_overview = self.overview.is_some();
        let root = column()
            .id("plait")
            .role(Role::Application)
            .aria_label("Plait")
            .key_context("Plait")
            .track_focus(&self.focus)
            .map(|root| self.settings_shortcuts(root, cx))
            .on_action(cx.listener(Self::open_repository))
            .on_action(cx.listener(Self::refresh))
            .on_action(
                cx.listener(|this, _: &worktrees_view::Worktrees, window, cx| {
                    this.open_worktrees(this.visible_project_checkout().to_owned(), window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &issues_view::Issues, window, cx| {
                this.open_issues(this.visible_project_checkout().to_owned(), window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &Settings, window, cx| this.toggle_settings(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ProjectSettings, window, cx| {
                this.show_project_settings(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| {
                this.settings_tab = settings::SettingsTab::About;
                this.show_settings(true, window, cx);
                this.check_for_updates(false, cx);
            }))
            .on_action(
                cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleOverview, window, cx| {
                    this.toggle_overview(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FillPanel, window, cx| this.fill_panel(window, cx)))
            .when(
                !show_overview && !self.settings_open && self.panel_is_open(),
                |el| {
                    el.on_action(cx.listener(|this, _: &FocusNextPane, window, cx| {
                        this.cycle_panel_focus(window, cx)
                    }))
                    .on_action(cx.listener(
                        |this, _: &FocusPreviousPane, window, cx| {
                            this.cycle_panel_focus(window, cx)
                        },
                    ))
                },
            )
            .on_action(
                cx.listener(|this, _: &ToggleInspector, window, cx| this.toggle_panel(window, cx)),
            )
            .on_action(cx.listener(|this, _: &RenameThread, window, cx| {
                if let Some(id) = this.host.thread.selected.clone() {
                    this.begin_rename(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ArchiveThread, window, cx| {
                if let Some(thread) = this
                    .host
                    .connection
                    .threads()
                    .iter()
                    .find(|thread| Some(&thread.id) == this.host.thread.selected.as_ref())
                {
                    this.archive_thread(
                        thread.id.clone(),
                        thread.archived != Some(true),
                        window,
                        cx,
                    );
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleArchived, _, cx| {
                this.navigation.archived = !this.navigation.archived;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterThreads, window, cx| {
                this.toggle_command_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleNotifications, _, cx| {
                let mut value = this.preferences.value.clone();
                value.navigation.mute_notifications = !value.navigation.mute_notifications;
                this.set_preferences(value, jump::PreferenceGroup::View, cx);
            }))
            .on_action(cx.listener(|this, _: &RefreshThreadList, window, cx| {
                this.refresh_thread_list(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowHiddenProjects, _, cx| {
                this.show_hidden_projects(cx);
            }))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| {
                this.toggle_command_palette(window, cx)
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && this.workspace.images.enlarged.take().is_some()
                {
                    this.focus_composer(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                } else if event.keystroke.key == "escape"
                    && event.keystroke.modifiers == Modifiers::default()
                    && !this.settings_open
                    && this.overlay.is_none()
                    && this.overview.is_some()
                    // The rename field leaves Escape to an input method mid-composition.
                    && !this
                        .navigation
                        .rename
                        .as_ref()
                        .is_some_and(|(_, field)| field.read(cx).is_composing())
                {
                    this.close_overview(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &NewThread, window, cx| {
                this.close_overview(window, cx);
                this.open_draft(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleChangedFiles, _, cx| {
                let hidden = !this.preferences.value.hide_changed_files;
                this.set_changed_files_hidden(hidden, cx)
            }))
            .on_action(cx.listener(|this, _: &NewWorktreeThread, window, cx| {
                this.new_worktree_thread(window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewThreadWithModel, window, cx| {
                this.open_draft(window, cx);
                this.open_model_picker(model_picker::ModelPurpose::NewThread, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchThreadModel, window, cx| {
                this.open_thread_models(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleDiagnostics, _, cx| {
                this.diagnostics = !this.diagnostics;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Conversation, window, cx| {
                this.show_tab(Workspace::Conversation, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Changes, window, cx| {
                this.show_tab(Workspace::Changes, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CommitHistory, window, cx| {
                this.show_tab(Workspace::History, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowFiles, window, cx| {
                this.show_tab(Workspace::Files, window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.quick_open(window, cx)))
            .on_action(cx.listener(|this, _: &FindInFile, window, cx| {
                if let Some(target) = this.find_target() {
                    this.open_find_in(target, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| {
                if let Some(target) = this.find_target() {
                    this.step_find_in(target, true, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| {
                if let Some(target) = this.find_target() {
                    this.step_find_in(target, false, cx)
                }
            }))
            // Show Changes, where the result and any error appear.
            .on_action(cx.listener(|this, _: &StageAll, window, cx| {
                if this.panel_recorded() {
                    return;
                }
                this.show_tab(Workspace::Changes, window, cx);
                this.git_write(plait_client::git::WriteOp::StageAll, cx)
            }))
            .on_action(cx.listener(|this, _: &UnstageAll, window, cx| {
                if this.panel_recorded() {
                    return;
                }
                this.show_tab(Workspace::Changes, window, cx);
                this.git_write(plait_client::git::WriteOp::UnstageAll, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CommitStaged, window, cx| this.focus_commit(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Push, window, cx| {
                if this.panel_recorded() {
                    return;
                }
                this.show_tab(Workspace::Changes, window, cx);
                this.push(cx)
            }))
            .on_action(cx.listener(|this, _: &Pull, window, cx| {
                if this.panel_recorded() {
                    return;
                }
                this.show_tab(Workspace::Changes, window, cx);
                this.pull(cx)
            }))
            .on_action(cx.listener(|this, _: &Fetch, window, cx| {
                if this.panel_recorded() {
                    return;
                }
                this.show_tab(Workspace::Changes, window, cx);
                this.git_write(plait_client::git::WriteOp::Fetch, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchBranch, window, cx| {
                this.open_branches(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NewBranch, window, cx| this.open_branches(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextThreadNeedingMe, window, cx| {
                this.next_needing_me(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextThread, window, cx| this.step_thread(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &PreviousThread, window, cx| {
                this.step_thread(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextFile, window, cx| this.step_file(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousFile, window, cx| this.step_file(false, window, cx)),
            )
            .on_action(cx.listener(|_, _: &Tab, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &BackTab, window, cx| window.focus_prev(cx)))
            .on_action(
                cx.listener(|this, _: &Close, window, cx| this.close_desktop(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Quit, window, cx| this.close_desktop(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ProviderSettings, window, cx| {
                this.show_settings(true, window, cx);
                this.settings_tab = settings::SettingsTab::Providers;
            }))
            .on_action(cx.listener(|_, _: &Minimize, window, _| window.minimize_window()))
            .map(|root| self.on_jump_actions(root, cx))
            .map(|root| self.on_tab_actions(root, cx))
            .capture_key_up(cx.listener(|this, _, _, cx| {
                if this.overview.is_none() {
                    this.queue_tab_save(cx);
                }
            }))
            .capture_any_mouse_up(cx.listener(|_, _, window, cx| {
                // The Needs you button activates after capture.
                cx.defer_in(window, |this, _, cx| {
                    if this.overview.is_none() {
                        this.queue_tab_save(cx);
                    }
                });
            }))
            .map(|root| self.on_ways_out_actions(root, cx))
            .on_action(cx.listener(|this, action: &UndoLastTurn, window, cx| {
                if this.overview.is_none() && !this.defer_key_action(action, cx) {
                    this.undo_last_turn(window, cx);
                }
            }))
            .when(!show_overview, |root| self.on_approval_actions(root, cx))
            .when(!show_overview, |root| self.on_selection_actions(root, cx))
            .map(|root| self.on_keyboard_controls(root, cx))
            .map(|root| self.on_panel_resize(root, cx))
            .size_full()
            .overflow_hidden()
            .bg(rgb(colors().rail))
            .text_color(rgb(colors().text))
            .text_size(px(TYPE_ROW_SIZE))
            .font_family(ui_font())
            .when(!show_overview, |root| root.child(self.tab_save_input(cx)))
            .child(
                row()
                    .gap_0()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .when(sidebar, |el| el.child(self.navigation(cx)))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .bg(rgb(colors().bg))
                            .overflow_hidden()
                            .when(sidebar, |sheet| {
                                sheet
                                    .mt(px(8.))
                                    .mr(px(8.))
                                    .mb(px(8.))
                                    .rounded(px(SURFACE_CORNER))
                                    .border_1()
                                    .border_color(rgb(colors().border))
                                    // GPUI clips children to a rectangle. Keep opaque views inside the arc.
                                    .p(px(4.))
                            })
                            .when(!show_overview, |sheet| sheet.child(self.header(window, cx)))
                            .when_some(
                                self.session.note.clone().filter(|_| !show_overview),
                                |el, note| {
                                    el.child(
                                        div()
                                            .px_4()
                                            .py_2()
                                            .text_size(px(12.))
                                            .text_color(rgb(colors().amber))
                                            .child(note),
                                    )
                                },
                            )
                            .when_some(
                                self.machine_restore_notice
                                    .clone()
                                    .filter(|_| !show_overview),
                                |el, notice| {
                                    el.child(
                                        row().px_4().py_2().gap_2().child(notice).child(
                                            button(
                                                "restore-machine-settings",
                                                "Open Machines settings…",
                                            )
                                            .on_activate(cx, |this, window, cx| {
                                                this.machine_restore_notice = None;
                                                this.settings_tab = settings::SettingsTab::Machines;
                                                this.show_settings(true, window, cx);
                                            }),
                                        ),
                                    )
                                },
                            )
                            .when_some(
                                self.notice.clone().filter(|_| !show_overview),
                                |el, notice| {
                                    el.child(
                                        div()
                                            .px_4()
                                            .py_2()
                                            .text_size(px(12.))
                                            .text_color(rgb(colors().red))
                                            .child(notice),
                                    )
                                },
                            )
                            .when_some(
                                (!show_overview)
                                    .then(|| self.build_progress_bar(cx))
                                    .flatten(),
                                |el, bar| el.child(bar),
                            )
                            .when_some(
                                (!show_overview).then(|| self.creation_bar(cx)).flatten(),
                                |el, bar| el.child(bar),
                            )
                            .child(
                                column()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .when(!show_overview, |el| el.child(self.panel_area_probe(cx)))
                                    .child(if show_overview {
                                        self.overview_view(cx).into_any_element()
                                    } else if self.worktrees_visible() {
                                        self.worktrees_view(wide, cx)
                                    } else if self.issues_visible() {
                                        self.issues_tab(wide, cx)
                                    } else if self.plain_folder_open()
                                        && matches!(
                                            self.workspace.tab,
                                            Workspace::Changes | Workspace::History
                                        )
                                    {
                                        theme::column()
                                            .flex_1()
                                            .justify_center()
                                            .items_center()
                                            .child(theme::empty(
                                                "Not a Git repository",
                                                PLAIN_FOLDER,
                                            ))
                                            .into_any_element()
                                    } else if self.workspace.tab == Workspace::History {
                                        self.history_view(
                                            self.workspace.sheet_body.size.width.into(),
                                            cx,
                                        )
                                        .into_any_element()
                                    } else if self.workspace.tab == Workspace::Files {
                                        self.file_view(cx)
                                    } else if self.workspace.tab == Workspace::Changes {
                                        self.changes(wide, cx).into_any_element()
                                    } else if real {
                                        self.live_conversation(wide, window, cx).into_any_element()
                                    } else if drafting {
                                        self.draft_view(wide, window, cx).into_any_element()
                                    } else {
                                        self.idle(wide, cx).into_any_element()
                                    }),
                            )
                            .when(self.diagnostics, |sheet| {
                                sheet.child(self.diagnostics_bar())
                            }),
                    ),
            )
            .when_some(self.tab_list(cx), |el, list| el.child(list))
            // Above the workspace, beneath images, Settings, and pickers.
            .when_some(self.toast_stack(cx), |el, toasts| el.child(toasts))
            .when_some(self.image_overlay(window, cx), |el, overlay| {
                el.child(overlay)
            })
            .when(self.settings_open, |el| {
                el.child(self.settings_modal(window, cx))
            })
            // Pickers remain above Settings.
            .when_some(self.overlay.as_ref(), |el, overlay| {
                el.child(overlay.palette.clone())
            });

        self.last_render_us = start.elapsed().as_micros();
        self.max_render_us = self.max_render_us.max(self.last_render_us);
        root.audited()
    }
}

/// Bind the default shortcuts and build the menu bar from the action table.
pub fn menus(cx: &mut App) {
    actions::install(cx, &actions::KeyOverrides::new());
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// What a plain folder does without: everything built on Git.
pub(super) const PLAIN_FOLDER: &str = "This folder is not a Git repository. Threads and files work here; Changes, History, branches, worktrees, and undo need Git. Nothing an agent changes here can be undone.";

/// Handoffs open this Mac's apps on this Mac's paths, so a remote checkout has none.
const REMOTE_UNAVAILABLE: &str =
    "Editor, Terminal, and Finder open this Mac's files, not another machine's.";

fn valid_remote_path(path: &str) -> bool {
    !path.is_empty() && path.starts_with('/') && !path.chars().any(char::is_control)
}
