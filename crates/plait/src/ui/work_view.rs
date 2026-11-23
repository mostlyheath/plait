//! Two-pane presentation of the active thread and its changes.
use super::{Plait, Workspace, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{panes::*, review::recorded::Target};

#[derive(Clone)]
struct Resize {
    tab: plait_client::tabs::TabId,
    divider: DividerId,
}

impl Plait {
    pub(super) fn watch_work_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        for (handle, kind) in self
            .work_focus
            .iter()
            .zip([PaneKind::Thread, PaneKind::Changes])
        {
            cx.on_focus_in(handle, window, move |this, _, cx| {
                if let Some(tree) = this
                    .tabs
                    .model
                    .work_mut()
                    .filter(|w| w.open)
                    .and_then(|w| w.tree.as_mut())
                {
                    tree.focus_kind(kind);
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
            .map_or(self.sidebar, |w| w.sidebar(self.sidebar))
    }
    pub(super) fn work_area_probe(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.weak_entity();
        canvas(
            move |bounds, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.workspace.work_area != bounds {
                        this.workspace.work_area = bounds;
                        if this.tabs.model.work().is_some_and(|w| w.open) {
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
        let size = self.workspace.work_area.size;
        self.tabs
            .model
            .work()?
            .layout(self.panels.viewport, size.width.into(), size.height.into())
    }
    pub(super) fn draws_work(&self) -> bool {
        let size = self.workspace.work_area.size;
        self.tabs
            .model
            .work()
            .is_some_and(|w| w.fits(self.panels.viewport, size.width.into(), size.height.into()))
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
        if check_work_view_width(window.viewport_size().width.into()).is_err() {
            return;
        }
        if self.workspace.tab != Workspace::Conversation
            && self.tabs.model.work().is_some_and(|work| !work.open)
        {
            self.show_tab(Workspace::Conversation, window, cx);
        }
        let Some(work) = self.tabs.model.work_mut() else {
            return;
        };
        if work
            .toggle(window.viewport_size().width.into(), has_record)
            .is_err()
        {
            return;
        }
        let open = work.open;
        let recorded = work.recorded;
        if open {
            self.workspace.tab = Workspace::Conversation;
            self.sync_work_scope(recorded, window, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }
    pub(super) fn choose_work_scope(
        &mut self,
        recorded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(work) = self.tabs.model.work_mut() else {
            return;
        };
        work.select_scope(recorded);
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
                let target = Target {
                    thread: thread.clone(),
                    checkout: self.host.creation.checkout.clone(),
                    record: line.displayed.clone(),
                    original: line.run.clone(),
                    path: None,
                };
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
        if !self
            .tabs
            .model
            .work_mut()
            .is_some_and(|w| w.route(tab, visible))
        {
            return false;
        }
        window.focus(&self.work_focus[usize::from(tab == Workspace::Changes)], cx);
        cx.notify();
        true
    }
    pub(super) fn work_recorded(&self) -> bool {
        self.draws_work() && self.tabs.model.work().is_some_and(|w| w.recorded)
    }
    pub(super) fn work_switch(&self, cx: &mut Context<Self>) -> Div {
        let work = self.draws_work();
        let refusal = check_work_view_width(self.panels.viewport).err();
        let mut bar = segmented();
        for (id, label, selected) in [("work-chat", "Chat", !work), ("work-work", "Work", work)] {
            let active = self.tabs.model.work().is_some_and(|w| w.open);
            bar = bar.child(
                segment(id, label, selected)
                    .disabled(refusal.is_some())
                    .aria_keyshortcuts(self.shortcut_aria("view.toggle_work"))
                    .when_some(refusal, |el, why| {
                        el.aria_description(why.reason()).tooltip(move |_, cx| {
                            cx.new(|_| TextTooltip(why.reason().into())).into()
                        })
                    })
                    .when(refusal.is_none() && active != (label == "Work"), |el| {
                        el.on_activate(cx, |this, window, cx| this.toggle_work(window, cx))
                    }),
            );
        }
        bar
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
                        (event.event.position.x - area.origin.x).into(),
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
    fn work_header(&self, pane: PaneRect) -> Div {
        let thread = pane.kind == PaneKind::Thread;
        let recorded = self.tabs.model.work().is_some_and(|w| w.recorded);
        let count = self.work_file_count(recorded);
        let detail = if thread {
            self.host
                .thread
                .selected
                .as_deref()
                .map(|id| self.thread_title(id))
                .unwrap_or_default()
        } else {
            let scope = if recorded { "This turn" } else { "Uncommitted" };
            let noun = if count == 1 { "file" } else { "files" };
            format!("{scope} · {count} {noun}")
        };
        let state = if thread {
            self.host
                .thread
                .snapshot
                .as_ref()
                .map_or(if self.draft.open { "Draft" } else { "Loading" }, |s| {
                    s.status.label()
                })
                .to_owned()
        } else if recorded {
            let review = &self.workspace.turn_review;
            if let Some((state, _)) = &review.error {
                (*state).into()
            } else if review.loading {
                "Loading".into()
            } else {
                review
                    .record
                    .as_ref()
                    .map_or_else(String::new, |r| r.counts.clone())
            }
        } else if let Some(s) = self.state.repository.snapshot().ready() {
            if s.plain {
                "No Git".into()
            } else if s.changes.is_empty() {
                "Clean".into()
            } else {
                format!("{} staged", self.state.repository.staged())
            }
        } else {
            "Unavailable".into()
        };
        // Step 9 adds the kind menu and split/close controls to this header.
        row()
            .px_3()
            .gap_2()
            .h(px(PANE_HEADER_HEIGHT))
            .flex_shrink_0()
            .bg(rgb(if pane.focused {
                colors().selected
            } else {
                colors().rail
            }))
            .text_size(px(12.))
            .child(pane.kind.label())
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_color(rgb(colors().muted))
                    .child(detail),
            )
            .child(state)
    }
    fn work_pane(
        &self,
        pane: PaneRect,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let thread = pane.kind.is_thread();
        let recorded = self.tabs.model.work().is_some_and(|w| w.recorded);
        let panel = column()
            .id(("work-pane", pane.id.get() as usize))
            .role(Role::Group)
            .aria_label(pane.kind.label())
            .audited()
            .track_focus(&self.work_focus[usize::from(!thread)])
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
            .child(self.work_header(pane));
        if thread {
            panel.child(if self.draft.open {
                self.draft_view(false, window, cx).into_any_element()
            } else {
                self.live_conversation(false, window, cx).into_any_element()
            })
        } else {
            panel.child(self.work_changes(recorded, pane.rect.width, cx))
        }
    }
    fn work_divider(
        &self,
        tab: plait_client::tabs::TabId,
        seam: DividerRect,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let description =
            "Use Left and Right to resize. Press Enter to give each pane equal space.";
        div()
            .id("work-divider")
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
            .cursor(CursorStyle::ResizeLeftRight)
            .on_drag(
                Resize {
                    tab,
                    divider: seam.id,
                },
                |_, _, _, cx| cx.new(|_| Empty),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                let size = this.workspace.work_area.size;
                let Some(tree) = this.tabs.model.work_mut().and_then(|w| w.tree.as_mut()) else {
                    return;
                };
                let result = match event.keystroke.key.as_str() {
                    "left" => tree.resize_divider(
                        seam.id,
                        seam.hairline.x - 16.,
                        size.width.into(),
                        size.height.into(),
                    ),
                    "right" => tree.resize_divider(
                        seam.id,
                        seam.hairline.x + 16.,
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
