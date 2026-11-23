//! Pane labels, kind picker, and layout controls.
use super::*;

impl Plait {
    fn pane_labels(&self, kind: PaneKind) -> (String, String) {
        use plait_client::state::Load;
        let repo = &self.state.repository;
        match kind {
            PaneKind::Thread => (
                self.host
                    .thread
                    .selected
                    .as_deref()
                    .map(|id| self.thread_title(id))
                    .unwrap_or_default(),
                self.host
                    .thread
                    .snapshot
                    .as_ref()
                    .map_or(if self.draft.open { "Draft" } else { "Loading" }, |s| {
                        s.status.label()
                    })
                    .into(),
            ),
            PaneKind::History => (
                repo.snapshot()
                    .ready()
                    .map(|s| s.branch.clone())
                    .unwrap_or_default(),
                match repo.commit_list() {
                    Load::Ready(list) => format!("{} commits", list.commits.len()),
                    Load::Loading => "Loading".into(),
                    Load::Error(_) => "Unavailable".into(),
                    Load::Empty => String::new(),
                },
            ),
            PaneKind::Files => {
                let viewer = &self.workspace.file_viewer;
                (
                    viewer
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    if viewer.loading {
                        "Loading"
                    } else if viewer.error.is_some() {
                        "Unavailable"
                    } else if viewer.opened.is_some() {
                        "Read-only"
                    } else {
                        ""
                    }
                    .into(),
                )
            }
            PaneKind::Changes => self.changes_labels(),
        }
    }

    fn changes_labels(&self) -> (String, String) {
        let recorded = self.tabs.model.work().is_some_and(|w| w.recorded);
        let scope = if recorded { "This turn" } else { "Uncommitted" };
        let count = self.work_file_count(recorded);
        let noun = if count == 1 { "file" } else { "files" };
        let state = if recorded {
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
        (format!("{scope} · {count} {noun}"), state)
    }

    pub(super) fn work_header(&self, pane: PaneRect, cx: &mut Context<Self>) -> Div {
        let thread = pane.kind == PaneKind::Thread;
        let (detail, state) = self.pane_labels(pane.kind);
        let work = self.tabs.model.work().unwrap();
        let tree = work.tree.as_ref().unwrap();
        let size = self.workspace.work_area.size;
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
            .child(self.kind_trigger(pane, tree, cx))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_color(rgb(colors().muted))
                    .child(detail),
            )
            .child(div().whitespace_nowrap().child(state))
            .children(
                [
                    (SplitDirection::SideBySide, "Split right", "◫"),
                    (SplitDirection::Stacked, "Split down", "⊟"),
                ]
                .map(|(dir, name, glyph)| {
                    let refusal = tree
                        .can_split(pane.id, dir, size.width.into(), size.height.into())
                        .err();
                    self.pane_button(pane, name, glyph, PaneCommand::Split(dir), refusal, cx)
                }),
            )
            .when(!thread, |el| {
                el.child(self.pane_button(pane, "Close pane", "×", PaneCommand::Close, None, cx))
            })
    }
    fn kind_trigger(&self, pane: PaneRect, tree: &PaneTree, cx: &Context<Self>) -> AnyElement {
        if pane.kind.is_thread() {
            return div().child("Thread").into_any_element();
        }
        let no_kinds = tree.available_kinds_for(pane.id).is_empty();
        button(
            ("kind-menu", pane.id.get() as usize),
            format!("{} ⌄", pane.kind.label()),
        )
        .aria_label(format!("{} pane kind", pane.kind.label()))
        .disabled(no_kinds)
        .when(no_kinds, |el| {
            el.tooltip(|_, cx| {
                cx.new(|_| TextTooltip(PaneRefusal::NoKindAvailable.reason().into()))
                    .into()
            })
        })
        .when(!no_kinds, |el| {
            el.on_activate(cx, move |this, window, cx| {
                this.toggle_work_kind_menu(pane.id, window, cx);
            })
        })
        .into_any_element()
    }

    pub(in crate::ui) fn toggle_work_kind_menu(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.draws_work() {
            return;
        }
        if let Some(work) = self.tabs.model.work_mut() {
            work.toggle_kind_menu(pane);
            if work.kind_menu.is_some() {
                window.focus(&self.kind_menu_focus, cx);
            } else {
                self.focus_work_pane(window, cx);
            }
        }
        cx.notify();
    }

    fn pane_button(
        &self,
        pane: PaneRect,
        name: &'static str,
        glyph: &'static str,
        cmd: PaneCommand,
        refusal: Option<PaneRefusal>,
        cx: &mut Context<Self>,
    ) -> Control {
        let tip = refusal.map_or(name, PaneRefusal::reason);
        button((name, pane.id.get() as usize), glyph)
            .px_0()
            .size(px(22.))
            .justify_center()
            .aria_label(name)
            .aria_description(tip)
            .disabled(refusal.is_some())
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(tip.into())).into())
            .when(refusal.is_none(), |el| {
                el.on_activate(cx, move |this, window, cx| {
                    if !this.draws_work() {
                        return;
                    }
                    if let Some(tree) = this.tabs.model.work_mut().and_then(|w| w.tree.as_mut()) {
                        let _ = tree.focus(pane.id);
                    }
                    this.pane_command(cmd, window, cx);
                })
            })
    }

    pub(super) fn kind_menu(&self, pane: PaneRect, cx: &Context<Self>) -> Option<AnyElement> {
        let work = self.tabs.model.work()?;
        if work.kind_menu != Some(pane.id) {
            return None;
        }
        Some(
            column()
                .id(("pane-kind-list", pane.id.get() as usize))
                .role(Role::Menu)
                .aria_label("Pane kind")
                .audited()
                .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let origin = this.workspace.work_area.origin;
                    let x = f32::from(e.position.x - origin.x) - pane.rect.x;
                    let y = f32::from(e.position.y - origin.y) - pane.rect.y;
                    // The header trigger handles its own second click.
                    if (0.0..pane.rect.width).contains(&x) && (0.0..PANE_HEADER_HEIGHT).contains(&y)
                    {
                        return;
                    }
                    if let Some(work) = this.tabs.model.work_mut() {
                        work.kind_menu = None;
                    }
                    cx.notify();
                }))
                .on_key_down(cx.listener(|this, e: &KeyDownEvent, w, cx| {
                    if e.keystroke.key == "escape" {
                        if let Some(work) = this.tabs.model.work_mut() {
                            work.kind_menu = None;
                        }
                        this.focus_work_pane(w, cx);
                        cx.stop_propagation();
                    }
                }))
                .absolute()
                .top(px(PANE_HEADER_HEIGHT))
                .left(px(4.))
                .bg(rgb(colors().panel))
                .occlude()
                .children(
                    work.tree
                        .as_ref()
                        .unwrap()
                        .available_kinds_for(pane.id)
                        .into_iter()
                        .enumerate()
                        .map(|(i, kind)| {
                            button(("choose-kind", kind as usize), kind.label())
                                .role(Role::MenuItem)
                                .when(i == 0, |el| el.track_focus(&self.kind_menu_focus))
                                .on_activate(cx, move |this, w, cx| {
                                    if !this.draws_work() {
                                        return;
                                    }
                                    if let Some(tree) =
                                        this.tabs.model.work_mut().and_then(|w| w.tree.as_mut())
                                    {
                                        let _ = tree.focus(pane.id);
                                    }
                                    this.pane_command(PaneCommand::Kind(kind), w, cx);
                                })
                        }),
                )
                .into_any_element(),
        )
    }
}
