//! Presentation of the active tab’s Work view.
mod header;

use super::{Plait, Workspace, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{panes::*, review::recorded::Target};

#[derive(Clone)]
struct Resize {
    tab: plait_client::tabs::TabId,
    divider: DividerId,
    direction: SplitDirection,
}

impl Plait {
    pub(super) fn watch_work_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        for (handle, kind) in self.work_focus.iter().zip([
            PaneKind::Thread,
            PaneKind::Changes,
            PaneKind::History,
            PaneKind::Files,
        ]) {
            cx.on_focus_out(handle, window, move |this, _, _, cx| {
                if this.panels.focused == Some(kind) {
                    this.panels.focused = None;
                    cx.notify();
                }
            })
            .detach();
            cx.on_focus_in(handle, window, move |this, _, cx| {
                if this.panel_is_open() {
                    this.panels.focused = Some(kind);
                    cx.notify();
                }
                if this.draws_work()
                    && let Some(tree) = this
                        .tabs
                        .model
                        .work_mut()
                        .filter(|w| w.open)
                        .and_then(|w| w.tree.as_mut())
                {
                    if let Some(id) = tree.pane_of_kind(kind) {
                        let _ = tree.focus(id);
                    }
                    cx.notify();
                }
            })
            .detach();
        }
    }
    pub(super) fn conversation_padding(&self) -> f32 {
        if self.draws_work() { 16. } else { 24. }
    }
    pub(super) fn visible_sidebar(&self) -> bool {
        self.tabs
            .model
            .work()
            .map_or(self.sidebar, |w| w.sidebar(self.sidebar, self.draws_work()))
    }
    pub(super) fn work_area_probe(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.weak_entity();
        canvas(
            move |bounds, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.workspace.work_area != bounds {
                        this.workspace.work_area = bounds;
                        if this.panel_is_open() || this.tabs.model.work().is_some_and(|w| w.open) {
                            cx.notify();
                        }
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }
    pub(super) fn work_layout(&self) -> Option<PaneLayout> {
        if !self.draws_work() {
            return None;
        }
        let size = self.workspace.work_area.size;
        Some(
            self.tabs
                .model
                .work()?
                .tree
                .as_ref()?
                .layout(size.width.into(), size.height.into()),
        )
    }

    pub(super) fn draws_work(&self) -> bool {
        if self.workspace.tab != Workspace::Conversation {
            return false;
        }
        self.tabs.model.work().is_some_and(|w| {
            // Evaluate the proposed Work layout even when Chat currently owns the sidebar.
            let sidebar = if w.sidebar {
                self.panel_width(super::panels::Panel::Sidebar)
            } else {
                0.
            };
            w.fits(
                self.panels.viewport,
                self.panels.viewport - sidebar,
                self.workspace.work_area.size.height.into(),
            )
        })
    }

    pub(super) fn work_fallback(&self) -> bool {
        self.tabs.model.work().is_some_and(|w| w.open) && !self.draws_work()
    }

    pub(super) fn leave_work_fallback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.work_fallback() {
            self.toggle_work(window, cx);
        }
    }

    pub(super) fn toggle_work(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let lines = &self.workspace.turn_records.lines;
        let has_record = if !lines.runs.is_empty() {
            Some(true)
        } else if self.records_current() || self.draft.open || self.host.protocol() < 5 {
            Some(false)
        } else {
            None
        };
        if !self.tabs.model.work().is_some_and(|w| w.open)
            && check_work_view_width(window.viewport_size().width.into()).is_err()
        {
            return;
        }
        if self.workspace.tab != Workspace::Conversation
            && self.tabs.model.work().is_some_and(|work| !work.open)
        {
            self.show_tab(Workspace::Conversation, window, cx);
        }
        let previous_views = self.work_readers();
        let Some(work) = self.tabs.model.work_mut() else {
            return;
        };
        if work
            .toggle(window.viewport_size().width.into(), has_record)
            .is_err()
        {
            return;
        }
        if work.open {
            self.workspace.tab = Workspace::Conversation;
        }
        self.sync_work_locations(window, cx);
        self.sync_work_readers(previous_views, cx);
        cx.notify();
    }
    // A parked Chat bookmark means the live workspace currently belongs to the panes.
    pub(super) fn sync_work_locations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.draws_work();
        let Some(work) = self.tabs.model.work() else {
            return;
        };
        if visible == work.chat_locations.is_some() || self.tabs.lifecycle.hydrating {
            return;
        }
        let mut bookmark = self.capture_tab_bookmark(Some(window), cx);
        let live = PaneLocations::capture(&bookmark);
        let work = self.tabs.model.work_mut().unwrap();
        if visible {
            let review = &self.workspace.turn_review;
            work.chat_review = review.target.clone().map(|target| (target, review.current));
            work.chat_locations = Some(live);
            let recorded = work.recorded;
            let panes = work.pane_locations.take();
            self.sync_work_scope(recorded, window, cx);
            if let Some(panes) = panes {
                panes.restore(&mut bookmark);
                if let Some(target) = self.workspace.turn_review.target.as_mut()
                    && let plait_client::tabs::bookmark::ChangeScope::Recorded { record, .. } =
                        &bookmark.changes.scope
                    && target.record == *record
                {
                    target.path = bookmark.changes.path.as_ref().map(Into::into);
                }
                self.restore_view_locations(bookmark, cx);
            }
            self.focus_work_pane(window, cx);
            return;
        }
        work.pane_locations = Some(live);
        let chat = work.chat_locations.take().unwrap();
        chat.restore(&mut bookmark);
        let saved = work.chat_review.take();
        use plait_client::tabs::bookmark::FocusTarget;
        let review = &self.workspace.turn_review;
        if saved.as_ref().map(|(t, current)| (t, *current))
            != review.target.as_ref().map(|t| (t, review.current))
        {
            self.clear_turn_review();
            if let Some((target, current)) = saved {
                self.workspace.turn_review.target = Some(target);
                self.workspace.turn_review.current = current;
                self.workspace.turn_review.window = Some(window.window_handle());
            }
            self.refresh_turn_review(cx);
        }
        self.restore_view_locations(bookmark, cx);
        match chat.focus {
            FocusTarget::Composer => self.focus_composer(window, cx),
            FocusTarget::ConversationFind
                if let Some(field) = &self.workspace.conversation_find.field =>
            {
                window.focus(&field.focus_handle(cx), cx)
            }
            _ => window.focus(&self.focus, cx),
        }
        cx.notify();
    }
    pub(super) fn choose_work_scope(
        &mut self,
        recorded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panel_is_open() {
            use plait_client::panel::Scope;
            if let Some(panel) = self.tabs.model.panel_mut() {
                panel.select_scope(if recorded {
                    Scope::ThisTurn
                } else {
                    Scope::Uncommitted
                });
            }
        } else if let Some(work) = self.tabs.model.work_mut() {
            work.select_scope(recorded);
        }
        self.sync_work_scope(recorded, window, cx);
    }
    pub(super) fn sync_work_scope(
        &mut self,
        recorded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if recorded {
            let records = &self.workspace.turn_records;
            let line = records.selection.review_line(&records.lines);
            if let (Some(line), Some(thread)) = (line, self.host.thread.selected.as_ref()) {
                let remembered = self
                    .tabs
                    .model
                    .panel()
                    .filter(|p| p.open)
                    .and_then(|p| p.recorded.as_ref())
                    .filter(|t| {
                        &t.thread == thread
                            && t.checkout == self.host.creation.checkout
                            && t.original == line.run
                    });
                let target = remembered.cloned().unwrap_or_else(|| Target {
                    thread: thread.clone(),
                    checkout: self.host.creation.checkout.clone(),
                    record: line.displayed.clone(),
                    original: line.run.clone(),
                    path: None,
                });
                if !self
                    .workspace
                    .turn_review
                    .target
                    .as_ref()
                    .is_some_and(|t| t.record == target.record && t.thread == target.thread)
                    || self.workspace.turn_review.current
                {
                    self.clear_turn_review();
                    self.workspace.turn_review.target = Some(target);
                    self.workspace.turn_review.window = Some(window.window_handle());
                    self.refresh_turn_review(cx);
                }
            } else if self.workspace.turn_review.target.is_some() && self.records_current() {
                self.clear_turn_review();
            }
        } else {
            if self.workspace.turn_review.target.is_some() {
                self.clear_turn_review();
            }
            self.count_work_files(cx);
        }
        self.search_work_diff(cx);
        cx.notify();
    }
    pub(super) fn route_work_tab(
        &mut self,
        tab: Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let visible = self.draws_work();
        let previous_views = self.work_readers();
        let was_open = self.tabs.model.work().is_some_and(|w| w.open);
        if !self
            .tabs
            .model
            .work_mut()
            .is_some_and(|w| w.route(tab, visible))
        {
            if was_open && self.tabs.model.work().is_some_and(|w| !w.open) {
                self.sync_work_locations(window, cx);
                self.sync_work_readers(previous_views, cx);
            }
            return false;
        }
        self.sync_work_readers(previous_views, cx);
        self.focus_work_pane(window, cx);
        true
    }
    pub(super) fn focused_work_kind(&self) -> Option<PaneKind> {
        if self.panel_is_open() {
            return self
                .panels
                .focused
                .filter(|k| *k == self.panel_focus_kind());
        }
        let tree = self
            .tabs
            .model
            .work()
            .filter(|_| self.draws_work())?
            .tree
            .as_ref()?;
        tree.pane_kind(tree.focused())
    }

    pub(super) fn shows_kind(&self, kind: PaneKind) -> bool {
        self.panel_is_open() && self.panel_focus_kind() == kind
            || self.draws_work() && self.remembers_kind(kind)
    }

    pub(super) fn remembers_kind(&self, kind: PaneKind) -> bool {
        self.tabs
            .model
            .work()
            .filter(|w| w.open)
            .and_then(|w| w.tree.as_ref())
            .is_some_and(|t| t.pane_of_kind(kind).is_some())
    }

    fn work_readers(&self) -> (bool, bool) {
        (
            self.remembers_kind(PaneKind::History),
            self.remembers_kind(PaneKind::Files),
        )
    }

    // Loading follows pane visibility, including panes that did not take focus.
    fn sync_work_readers(&mut self, previous: (bool, bool), cx: &mut Context<Self>) {
        let (history, files) = self.work_readers();
        if history && !previous.0 {
            self.state.repository.request_history();
            self.pump(cx);
        } else if !history && previous.0 {
            self.state.repository.leave_history();
        }
        if files && !previous.1 {
            self.check_file(cx);
        }
    }

    fn focus_work_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(kind) = self
            .tabs
            .model
            .work()
            .filter(|w| w.open)
            .and_then(|w| w.tree.as_ref())
            .and_then(|t| t.pane_kind(t.focused()))
        {
            window.focus(&self.work_focus[kind as usize], cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    pub(super) fn pane_action_refusal(&self, id: &str) -> Option<PaneRefusal> {
        if !id.starts_with("pane.") {
            return None;
        }
        if self.panel_is_open()
            && matches!(id, "pane.next" | "pane.previous")
            && !self.settings_open
        {
            return None;
        }
        if self.settings_open || !self.draws_work() {
            return Some(PaneRefusal::WorkViewRequired);
        }
        let tree = self.tabs.model.work()?.tree.as_ref()?;
        let direction = match id {
            "pane.split_right" => SplitDirection::SideBySide,
            "pane.split_down" => SplitDirection::Stacked,
            "pane.close" if tree.focused() == tree.thread_pane() => {
                return Some(PaneRefusal::CannotCloseThread);
            }
            _ => return None,
        };
        let size = self.workspace.work_area.size;
        tree.can_split(
            tree.focused(),
            direction,
            size.width.into(),
            size.height.into(),
        )
        .err()
    }

    pub(super) fn pane_command(
        &mut self,
        command: PaneCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panel_is_open() && matches!(command, PaneCommand::Next | PaneCommand::Previous) {
            let handle = &self.work_focus[self.panel_focus_kind() as usize];
            if handle.contains_focused(window, cx) && !self.panel_fills() {
                window.focus(&self.focus, cx);
            } else {
                window.focus(handle, cx);
            }
            cx.notify();
            return;
        }
        let size = self.workspace.work_area.size;
        let previous_views = self.work_readers();
        if self.draws_work()
            && let Some(work) = self.tabs.model.work_mut()
        {
            if let Err(reason) = work.apply(command, size.width.into(), size.height.into()) {
                self.notice = Some(reason.reason().into());
            }
            self.sync_work_readers(previous_views, cx);
            if self.draws_work() {
                self.focus_work_pane(window, cx);
            } else {
                self.sync_work_locations(window, cx);
            }
        } else {
            self.notice = Some("Pane actions are available in Work view.".into());
            cx.notify();
        }
    }

    pub(super) fn work_recorded(&self) -> bool {
        self.panel_recorded()
            || self.draws_work() && self.tabs.model.work().is_some_and(|w| w.recorded)
    }
    pub(super) fn work_view(
        &self,
        layout: PaneLayout,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = column().relative().size_full().overflow_hidden();
        for pane in layout.panes {
            body = body.child(self.work_pane(pane, window, cx));
        }
        if let Some(tab) = self.tabs.model.active() {
            for seam in layout.dividers {
                body = body.child(self.work_divider(tab, seam, cx));
            }
        }
        body.on_drag_move(cx.listener(|this, event: &DragMoveEvent<Resize>, _, cx| {
            if this.tabs.model.active() != Some(event.drag(cx).tab) {
                return;
            }
            let area = this.workspace.work_area;
            if let Some(tree) = this.tabs.model.work_mut().and_then(|w| w.tree.as_mut())
                && tree
                    .resize_divider(
                        event.drag(cx).divider,
                        match event.drag(cx).direction {
                            SplitDirection::SideBySide => event.event.position.x - area.origin.x,
                            SplitDirection::Stacked => event.event.position.y - area.origin.y,
                        }
                        .into(),
                        area.size.width.into(),
                        area.size.height.into(),
                    )
                    .is_ok()
            {
                cx.notify();
            }
        }))
        .into_any_element()
    }
    fn work_pane(
        &self,
        pane: PaneRect,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let recorded = self.tabs.model.work().is_some_and(|w| w.recorded);
        let panel = column()
            .id(("work-pane", pane.id.get() as usize))
            .role(Role::Group)
            .aria_label(pane.kind.label())
            .audited()
            .tab_group()
            .tab_stop(false)
            .track_focus(&self.work_focus[pane.kind as usize])
            .absolute()
            .left(px(pane.rect.x))
            .top(px(pane.rect.y))
            .w(px(pane.rect.width))
            .h(px(pane.rect.height))
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                if let Some(tree) = this.tabs.model.work_mut().and_then(|w| w.tree.as_mut())
                    && tree.focused() != pane.id
                    && tree.focus(pane.id).is_ok()
                {
                    cx.notify();
                }
            }))
            .child(self.work_header(pane, cx));
        let content = match pane.kind {
            PaneKind::Thread => {
                if self.draft.open {
                    self.draft_view(false, window, cx).into_any_element()
                } else {
                    self.live_conversation(false, window, cx).into_any_element()
                }
            }
            PaneKind::Changes => {
                let changes = self.work_changes(recorded, pane.rect.width, cx);
                if !recorded && pane.rect.height < 400. {
                    column()
                        .id(("scroll-changes-pane", pane.id.get() as usize))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(changes.min_h(px(400.)))
                        .into_any_element()
                } else {
                    changes.into_any_element()
                }
            }
            PaneKind::History if self.plain_folder_open() => {
                empty("Not a Git repository", super::PLAIN_FOLDER).into_any_element()
            }
            PaneKind::History => self.history_view(pane.rect.width, cx).into_any_element(),
            PaneKind::Files => self.file_view(cx),
        };
        panel.child(content).children(self.kind_menu(pane, cx))
    }

    fn work_divider(
        &self,
        tab: plait_client::tabs::TabId,
        seam: DividerRect,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let stacked = seam.direction == SplitDirection::Stacked;
        let description = if stacked {
            "Use Up and Down to resize. Press Enter to give each pane equal space."
        } else {
            "Use Left and Right to resize. Press Enter to give each pane equal space."
        };
        div()
            .id(("work-divider", seam.id.get() as usize))
            .role(Role::Splitter)
            .aria_label("Resize panes")
            .aria_description(description)
            .focusable()
            .tab_stop(true)
            .focus_visible(|el| el.bg(rgb(colors().blue)))
            .audited()
            .absolute()
            .left(px(seam.drag.x))
            .top(px(seam.drag.y))
            .w(px(seam.drag.width))
            .h(px(seam.drag.height))
            .child(
                div()
                    .absolute()
                    .left(px(seam.hairline.x - seam.drag.x))
                    .top(px(seam.hairline.y - seam.drag.y))
                    .w(px(seam.hairline.width))
                    .h(px(seam.hairline.height))
                    .bg(rgb(colors().border)),
            )
            .cursor(if stacked {
                CursorStyle::ResizeUpDown
            } else {
                CursorStyle::ResizeLeftRight
            })
            .on_drag(
                Resize {
                    tab,
                    divider: seam.id,
                    direction: seam.direction,
                },
                |_, _, _, cx| cx.new(|_| Empty),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                let size = this.workspace.work_area.size;
                let Some(tree) = this.tabs.model.work_mut().and_then(|w| w.tree.as_mut()) else {
                    return;
                };
                let result = match event.keystroke.key.as_str() {
                    key if key == (if stacked { "up" } else { "left" }) => tree.resize_divider(
                        seam.id,
                        (if stacked {
                            seam.hairline.y
                        } else {
                            seam.hairline.x
                        }) - 16.,
                        size.width.into(),
                        size.height.into(),
                    ),
                    key if key == (if stacked { "down" } else { "right" }) => tree.resize_divider(
                        seam.id,
                        (if stacked {
                            seam.hairline.y
                        } else {
                            seam.hairline.x
                        }) + 16.,
                        size.width.into(),
                        size.height.into(),
                    ),
                    "enter" => tree.even_divider(seam.id),
                    _ => return,
                };
                if result.is_ok() {
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2
                    && let Some(tree) = this.tabs.model.work_mut().and_then(|w| w.tree.as_mut())
                    && tree.even_divider(seam.id).is_ok()
                {
                    cx.notify();
                }
            }))
    }
}
