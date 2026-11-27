//! The review panel shares one conversation sheet with the active thread.
use super::{Plait, Workspace, panels::Panel as Edge, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    panel::{PanelKind, Scope},
    panes::PaneKind,
    review::recorded::Target,
};

impl Plait {
    pub(super) fn selected_view(&self) -> Workspace {
        match self.panel_kind() {
            Some(PanelKind::Changes) => Workspace::Changes,
            Some(PanelKind::History) => Workspace::History,
            Some(PanelKind::Files) => Workspace::Files,
            None => self.workspace.tab,
        }
    }

    pub(super) fn panel_is_open(&self) -> bool {
        !self.worktrees_visible()
            && !self.graph_visible()
            && self.tabs.model.panel().is_some_and(|p| p.open)
    }

    pub(super) fn panel_kind(&self) -> Option<PanelKind> {
        self.tabs.model.panel().filter(|p| p.open).map(|p| p.kind)
    }

    pub(super) fn panel_fills(&self) -> bool {
        self.panel_is_open()
            && self
                .tabs
                .model
                .panel()
                .is_some_and(|p| p.fills(self.sheet_width()))
    }

    pub(super) fn sheet_width(&self) -> f32 {
        let measured = f32::from(self.workspace.sheet_body.size.width);
        if measured > 0. {
            return measured;
        }
        (self.panels.viewport
            - if self.sidebar {
                self.panel_width(Edge::Sidebar)
            } else {
                0.
            })
        .max(0.)
    }

    pub(super) fn panel_focus_kind(&self) -> PaneKind {
        match self.panel_kind() {
            Some(PanelKind::Changes) => PaneKind::Changes,
            Some(PanelKind::Files) => PaneKind::Files,
            Some(PanelKind::History) => PaneKind::History,
            None => PaneKind::Thread,
        }
    }

    pub(super) fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.tabs.model.panel() {
            Some(p) if !p.open => match p.kind {
                PanelKind::Changes => Workspace::Changes,
                PanelKind::Files => Workspace::Files,
                PanelKind::History => Workspace::History,
            },
            None => Workspace::Changes,
            _ => Workspace::Conversation,
        };
        self.show_tab(target, window, cx);
    }

    pub(super) fn fill_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overview(window, cx);
        if !self.panel_is_open() {
            self.toggle_panel(window, cx);
        }
        if let Some(p) = self.tabs.model.panel_mut() {
            p.set_fill(!p.fill);
        }
        if self.panel_fills() {
            let kind = self.panel_focus_kind();
            let focus = self.panel_focus[kind as usize].clone();
            if !focus.contains_focused(window, cx) {
                self.leave_conversation_focus(window, cx);
                self.panels.focused = Some(kind);
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn review_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.panel_width(Edge::Review);
        let kind = self.panel_focus_kind();
        let n = self
            .state
            .repository
            .snapshot()
            .ready()
            .map(|s| s.changes.len());
        let mut tabs = row()
            .h(px(36.))
            .px_2()
            .gap_1()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(colors().border));
        for (id, title, view, k) in [
            (
                "panel-changes",
                n.map_or_else(|| "Changes".into(), |n| format!("Changes {n}")),
                Workspace::Changes,
                PaneKind::Changes,
            ),
            (
                "panel-files",
                "Files".into(),
                Workspace::Files,
                PaneKind::Files,
            ),
            (
                "panel-history",
                "History".into(),
                Workspace::History,
                PaneKind::History,
            ),
        ] {
            tabs = tabs.child(
                toned_button(
                    id,
                    title,
                    if k == kind {
                        Tone::Selected
                    } else {
                        Tone::Ghost
                    },
                )
                .role(Role::Tab)
                .aria_selected(k == kind)
                .rounded(px(6.))
                .h(px(26.))
                .on_activate(cx, move |this, w, cx| this.show_tab(view, w, cx)),
            );
        }
        let body = match kind {
            PaneKind::History if self.plain_folder_open() => {
                empty("Not a Git repository", super::PLAIN_FOLDER).into_any_element()
            }
            PaneKind::History => self.history_view(width, cx).into_any_element(),
            PaneKind::Files => self.file_view(cx),
            _ => self
                .panel_changes(self.panel_recorded(), width, cx)
                .into_any_element(),
        };
        column()
            .id("review-panel")
            .role(Role::Group)
            .aria_label("Review panel")
            .audited()
            .tab_group()
            .track_focus(&self.panel_focus[kind as usize])
            .relative()
            .w(px(width))
            .flex_shrink_0()
            .min_h_0()
            .overflow_hidden()
            .border_l_1()
            .border_color(rgb(colors().border))
            .bg(rgb(colors().rail))
            .child(
                tabs.child(div().flex_1()).child(
                    icon_button("close-panel", "close", "Close panel")
                        .on_activate(cx, |this, w, cx| {
                            this.show_tab(Workspace::Conversation, w, cx)
                        }),
                ),
            )
            .child(body)
            .when(!self.panel_fills(), |el| {
                el.child(self.resize_handle(Edge::Review, cx))
            })
    }

    pub(super) fn panel_recorded(&self) -> bool {
        self.panel_kind() == Some(PanelKind::Changes)
            && self
                .tabs
                .model
                .panel()
                .is_some_and(|p| p.scope == Scope::ThisTurn)
            && self.has_panel_record()
    }

    pub(super) fn has_panel_record(&self) -> bool {
        !self.workspace.turn_review.older
            && self.host.protocol() >= 5
            && !self.plain_folder_open()
            && (self.workspace.turn_review.target.is_some()
                || self
                    .workspace
                    .turn_records
                    .selection
                    .review_line(&self.workspace.turn_records.lines)
                    .is_some())
    }

    pub(super) fn panel_turn_heading(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = self.conversation_snapshot()?;
        let records = &self.workspace.turn_records;
        let model = plait_client::history::this_turn::panel(
            &records.lines,
            &records.selection,
            snapshot.status,
            self.undo_context(),
        )?;
        let mut heading = row()
            .gap_1()
            .child(tag("turn-heading", model.heading, colors().muted))
            .child(tag("turn-state", model.state, colors().subtle));
        if let Some(line) = model.line {
            heading = heading.child(tag(
                "turn-age",
                super::commits::age((line.created_at / 1000) as i64, super::commits::now()),
                colors().subtle,
            ));
            if let Some(action) = model.action {
                heading = heading.child(self.record_undo_button(line, action, None, cx).px_1());
            }
        }
        Some(
            heading
                .when(model.heading == "Earlier turn", |el| {
                    el.child(button("turn-latest", "Latest").px_1().on_activate(
                        cx,
                        |this, w, cx| {
                            this.workspace.turn_records.selection.latest();
                            if let Some(panel) = this.tabs.model.panel_mut() {
                                panel.recorded = None;
                            }
                            this.sync_panel_scope(true, w, cx);
                        },
                    ))
                })
                .into_any_element(),
        )
    }

    pub(super) fn show_earlier_turn(
        &mut self,
        run: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let (Some(line), Some(thread)) = (
            self.workspace.turn_records.lines.runs.get(run),
            self.host.thread.selected.as_ref(),
        ) {
            let target = plait_client::review::recorded::Target {
                thread: thread.clone(),
                checkout: self.host.creation.checkout.clone(),
                record: line.displayed.clone(),
                original: run.to_owned(),
                path: None,
            };
            self.open_turn_review(target, window, cx);
            return;
        }
        self.clear_turn_review();
        self.workspace.turn_records.selection.earlier(run);
        if let Some(p) = self.tabs.model.panel_mut() {
            p.select_scope(Scope::ThisTurn);
            p.recorded = None;
        }
        self.show_tab(Workspace::Changes, window, cx);
    }
}

impl Plait {
    pub(super) fn watch_panel_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        for (handle, kind) in self.panel_focus.iter().zip([
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
            })
            .detach();
        }
    }
    pub(super) fn panel_area_probe(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.weak_entity();
        canvas(
            move |bounds, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.workspace.sheet_body != bounds {
                        this.workspace.sheet_body = bounds;
                        if this.panel_is_open() {
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
    pub(super) fn choose_panel_scope(
        &mut self,
        recorded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.tabs.model.panel_mut().filter(|p| p.open) {
            panel.select_scope(if recorded {
                Scope::ThisTurn
            } else {
                Scope::Uncommitted
            });
        }
        self.sync_panel_scope(recorded, window, cx);
    }
    pub(super) fn sync_panel_scope(
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
            self.count_panel_files(cx);
        }
        self.search_panel_diff(cx);
        cx.notify();
    }

    pub(super) fn focused_panel_kind(&self) -> Option<PaneKind> {
        self.panels
            .focused
            .filter(|kind| self.panel_is_open() && *kind == self.panel_focus_kind())
    }
    pub(super) fn shows_kind(&self, kind: PaneKind) -> bool {
        self.panel_is_open() && self.panel_focus_kind() == kind
    }
    pub(super) fn cycle_panel_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = &self.panel_focus[self.panel_focus_kind() as usize];
        if handle.contains_focused(window, cx) && !self.panel_fills() {
            window.focus(&self.focus, cx);
        } else {
            window.focus(handle, cx);
        }
        cx.notify();
    }
}
