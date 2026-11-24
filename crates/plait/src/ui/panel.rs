//! The review panel shares one conversation sheet with the active thread.
use super::{Plait, Workspace, panels::Panel as Edge, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    panel::{PanelKind, Scope},
    panes::PaneKind,
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
            && !self.draws_work()
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
        let measured = f32::from(self.workspace.work_area.size.width);
        if measured > 0. {
            return measured;
        }
        (self.panels.viewport
            - if self.visible_sidebar() {
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
            let focus = self.work_focus[kind as usize].clone();
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
                .work_changes(self.panel_recorded(), width, cx)
                .into_any_element(),
        };
        column()
            .id("review-panel")
            .role(Role::Group)
            .aria_label("Review panel")
            .audited()
            .tab_group()
            .track_focus(&self.work_focus[kind as usize])
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
                            this.sync_work_scope(true, w, cx);
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
