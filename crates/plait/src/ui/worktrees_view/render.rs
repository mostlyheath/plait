//! Virtualized graph and selected-checkout detail; consumes prepared state only.
use super::*;

impl Plait {
    pub(in crate::ui) fn worktrees_view(&self, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let graph = column().flex_1().min_w_0().min_h_0().child(
            uniform_list(
                "worktrees-graph",
                self.worktrees.items.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|i| this.worktree_graph_row(i, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.worktrees.scroll)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .flex_1()
            .min_h_0()
            .w_full(),
        );
        let content = if wide {
            row()
                .flex_1()
                .min_h_0()
                .items_stretch()
                .child(graph)
                .child(
                    self.worktree_detail(cx)
                        .w(px(340.))
                        .border_l_1()
                        .border_color(rgb(colors().border)),
                )
                .into_any_element()
        } else {
            column()
                .flex_1()
                .min_h_0()
                .child(graph)
                .child(
                    self.worktree_detail(cx)
                        .h(px(245.))
                        .border_t_1()
                        .border_color(rgb(colors().border)),
                )
                .into_any_element()
        };
        column().id("worktrees-view").flex_1().min_h_0()
            .child(self.worktrees_toolbar(cx))
            .when(self.worktrees.loading, |el| {
                el.child(div().px_4().child(a11y_text("worktrees-loading", "Loading")))
            })
            .when_some(self.worktrees.error.clone(), |el, error| {
                el.child(div().p_4().text_color(rgb(colors().muted)).child(a11y_text("worktrees-error", error)))
            })
            .when(self.worktrees.rows.len() == 1, |el| {
                el.child(div().px_4().py_2().text_color(rgb(colors().muted)).child(a11y_text(
                    "worktrees-empty", "No worktrees yet. Choose Own worktree when a project is busy, or New Thread in Worktree."
                )))
            })
            .when_some(self.worktrees.removal.message.clone().filter(|_| self.removal_here()), |el, result| {
                el.child(div().px_4().py_2().child(a11y_text("worktrees-result", result)))
            })
            .child(content)
            .when(self.worktrees.graph.more, |el| {
                el.child(div().px_4().child(a11y_text("worktrees-older", "Older commits not shown · 300 commit limit")))
            }).into_any_element()
    }
    fn worktrees_toolbar(&self, cx: &mut Context<Self>) -> Div {
        row()
            .h(px(44.))
            .px_4()
            .gap_3()
            .child(a11y_text("worktrees-heading", "Worktrees"))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(colors().subtle))
                    .child(hover_text("worktrees-total", self.worktrees.total.clone(), match self.worktrees.seed_bytes {
                        Some(seed) => format!("Logical bytes of these checkouts and their private build caches, plus {} in this project's build seed, the copy new worktrees are cloned from. Copy-on-write blocks may be shared, so removing worktrees does not free all of this.", super::bytes(seed)),
                        None => "Logical bytes of these checkouts and their private build caches. Copy-on-write blocks may be shared, so removing worktrees does not free all of this.".into(),
                    })),
            )
            .child(
                toned_button(
                    "worktrees-sort-used",
                    "Last used",
                    if self.worktrees.sort_size {
                        Tone::Ghost
                    } else {
                        Tone::Selected
                    },
                )
                .aria_selected(!self.worktrees.sort_size)
                .tooltip(|_, cx| {
                    hover(
                        "Order lanes by last use; the main checkout stays first.".into(),
                        cx,
                    )
                })
                .on_activate(cx, |this, _, cx| this.sort_worktree_lanes(false, cx)),
            )
            .child(
                toned_button(
                    "worktrees-sort-size",
                    "Size",
                    if self.worktrees.sort_size {
                        Tone::Selected
                    } else {
                        Tone::Ghost
                    },
                )
                .aria_selected(self.worktrees.sort_size)
                .tooltip(|_, cx| {
                    hover(
                        "Order lanes by measured size; the main checkout stays first.".into(),
                        cx,
                    )
                })
                .on_activate(cx, |this, _, cx| this.sort_worktree_lanes(true, cx)),
            )
            .child(
                button("worktrees-refresh", "Refresh")
                    .tooltip(|_, cx| {
                        hover("Read worktree state and measure storage again.".into(), cx)
                    })
                    .on_activate(cx, |this, _, cx| this.refresh_worktrees_view(true, cx)),
            )
    }
    fn worktree_graph_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let item = &self.worktrees.items[index];
        let node_lane = match item {
            Item::Head(_, lane) | Item::Changed(_, lane) | Item::Detached(lane) => *lane,
            Item::Commit(r) => self.worktrees.graph.rows[*r].lane,
        };
        let node_x = 12. + node_lane as f32 * 14.;
        let lane = if node_lane < self.worktrees.rows.len() {
            node_lane
        } else {
            0
        };
        let width = 24. + self.worktrees.graph.columns as f32 * 14.;
        let mut drawing = div().relative().h(px(28.)).w(px(width)).flex_shrink_0();
        for stroke in &self.worktrees.strokes[index] {
            let tint = if self.worktrees.selected == stroke.lane {
                colors().text
            } else if self.worktrees.hovered == Some(stroke.lane) {
                colors().muted
            } else {
                colors().edge
            };
            drawing = drawing.child(graph_stroke(stroke, tint));
        }
        let changed = matches!(item, Item::Changed(..));
        let node_color = if matches!(item, Item::Head(..)) {
            state_tint(self.worktrees.words[lane])
        } else {
            colors().muted
        };
        drawing = drawing.child(graph_mark(
            node_x,
            changed,
            node_color,
            if self.worktrees.selected == lane {
                colors().selected
            } else {
                colors().bg
            },
        ));
        let text = match item {
            Item::Commit(r) => self.worktrees.commit_labels[*r].clone(),
            Item::Changed(..) => format!(
                "{} changed · uncommitted",
                self.worktrees.rows[lane].preservation.changed.unwrap_or(0)
            ),
            Item::Head(..) | Item::Detached(_) => self.worktrees.head_labels[lane].clone(),
        };
        let identity = self.worktrees.rows[lane].path.display();
        let id = match item {
            Item::Head(..) | Item::Detached(_) => format!("worktrees-head-{identity}"),
            Item::Changed(..) => format!("worktrees-changed-{identity}"),
            Item::Commit(r) => format!(
                "worktrees-commit-{}",
                self.worktrees.graph.rows[*r].commit.commit.id
            ),
        };
        let mut line = row()
            .id(SharedString::from(id.clone()))
            .role(Role::Button)
            .aria_label(text.clone())
            .focusable()
            .tab_stop(true)
            .h(px(28.))
            .w_full()
            .min_w(px(width + 300.))
            .focus_visible(|s| {
                s.bg(rgb(colors().selected))
                    .border_color(rgb(colors().blue))
                    .border_1()
            })
            .px_2()
            .bg(rgb(if self.worktrees.selected == lane {
                colors().selected
            } else {
                colors().bg
            }))
            .child(drawing)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .child(self.worktree_row_text(item, lane, format!("{id}-text"), text)),
            )
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                let next = if *hovered { Some(node_lane) } else { None };
                if *hovered || this.worktrees.hovered == Some(node_lane) {
                    this.worktrees.hovered = next;
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.select_worktree_lane(lane, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_activate(cx, move |this, _, cx| {
                this.select_worktree_lane(lane, cx);
            });
        if matches!(item, Item::Head(..) | Item::Detached(_)) {
            line = line.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(rgb(colors().subtle))
                    .child(a11y_text(
                        format!("{id}-size"),
                        size_label(self.worktrees.sizes[lane].as_ref()),
                    )),
            );
        }
        line.audited().into_any_element()
    }
    fn worktree_detail(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let panel = column()
            .id("worktrees-detail")
            .p_4()
            .gap_2()
            .overflow_y_scroll();
        let n = self.worktrees.selected;
        let Some(tree) = self.worktrees.rows.get(n) else {
            return panel;
        };
        let path = tree.path.clone();
        let finder = path.clone();
        let terminal = path.clone();
        let drawn = (path.clone(), tree.identity.clone());
        let missing = tree.missing;
        let title = if self.worktrees.words[n].is_empty() {
            self.worktrees.titles[n].clone()
        } else {
            format!("{} · {}", self.worktrees.titles[n], self.worktrees.words[n])
        };
        let facts = &tree.preservation;
        let can_remove = Choices::for_checkout(tree).any();
        let reason=facts.refusal.clone().unwrap_or_else(||"Clean, published, and no commits kept only by this worktree. The host checks again before removal.".into());
        let base = tree.base.as_deref().map(|s| &s[..12]).unwrap_or("Unknown");
        let size = self
            .worktrees
            .sizes
            .get(n)
            .and_then(|s| s.as_ref())
            .and_then(|s| s.as_ref().ok());
        let largest = self.worktrees.largest;
        let bar = row()
            .gap_0()
            .h(px(6.))
            .flex_shrink_0()
            .w_full()
            .bg(rgb(colors().panel));
        let bar = if let Some(size) = size {
            bar.child(
                div()
                    .h_full()
                    .w(relative(size.source as f32 / largest))
                    .bg(rgb(colors().muted)),
            )
            .child(
                div()
                    .h_full()
                    .w(relative(size.build as f32 / largest))
                    .bg(rgb(colors().edge)),
            )
        } else {
            bar
        };
        let thread = tree
            .thread_id
            .clone()
            .filter(|_| self.worktrees.openable[n]);
        let busy = self.run_in_worktree(tree);
        let remove_reason = if facts.refusal.is_some() {
            reason.clone()
        } else if busy {
            "This worktree still has active, queued, or recovering work.".into()
        } else if self.worktrees.removal.pending.is_some() && !self.removal_here() {
            format!(
                "Another project's removal is unconfirmed. {}",
                worktree_removal::RECOVERY_HINT
            )
        } else if self.worktrees.removal.pending.is_some() {
            self.worktrees
                .removal
                .message
                .clone()
                .unwrap_or_else(|| "The removal is still in progress.".into())
        } else {
            reason.clone()
        };
        let visible_reason = if busy {
            "Working"
        } else if facts.refusal.is_none() {
            "Removable"
        } else {
            removal_advice(&reason, facts.can_keep_local)
        }
        .to_owned();
        let path_text = safe_text(&path.display().to_string());
        // A remote checkout's path is on another machine; Finder and Terminal would open nothing here.
        let local = self.host_is_local();
        panel.min_w_0().child(column().min_w_0().flex_shrink_0().gap_2().child(a11y_text("worktrees-detail-title",title))
            .child(row().flex_wrap().gap_2()
                .when_some(thread,|el,id|el.child(button("worktrees-open","Open").tooltip(|_,cx|hover("Bring this worktree’s thread forward.".into(),cx)).on_activate(cx,move |this,window,cx|this.select_host_thread(id.clone(),window,cx))))
                .child(button("worktrees-finder","Finder").disabled(tree.missing || !local).tooltip(move |_,cx|hover(if local {"Show this checkout in Finder."} else {super::super::REMOTE_UNAVAILABLE}.into(),cx)).on_activate(cx,move |this,_,cx| { if !missing { this.hand_off(plait_core::handoff::show_in_finder(&finder),cx); } }))
                .child(button("worktrees-terminal","Terminal").disabled(tree.missing || !local).tooltip(move |_,cx|hover(if local {"Open Terminal Here using Settings → Editor."} else {super::super::REMOTE_UNAVAILABLE}.into(),cx)).on_activate(cx,move |this,_,cx| { if !missing { this.hand_off(plait_core::handoff::open_terminal(&this.preferences.value.open_terminals_with,&terminal),cx); } }))
                .when(tree.creation_id.is_some()&&!tree.main&&!tree.removed,|el|el.child(button("worktrees-remove","Remove").when(cfg!(test), |el| el.tooltip_show_delay(std::time::Duration::ZERO)).disabled(!can_remove || busy || self.worktrees.removal.pending.is_some() || self.worktree_busy || !self.host.connection.connected || self.worktrees.loading || self.worktrees.error.is_some()).text_color(rgb(if can_remove && !busy {colors().text}else{colors().subtle})).tooltip(move |_,cx|hover(remove_reason.clone(),cx)).on_activate(cx,move |this,window,cx|this.ask_remove_view_worktree(&drawn.0,drawn.1.as_ref(),window,cx)))))
            .child(div().text_size(px(12.)).text_color(rgb(colors().muted)).child(a11y_text("worktrees-detail-branch",format!("{} · from {base}",safe_text(tree.branch.as_deref().unwrap_or("Detached"))))))
            .child(hover_text("worktrees-detail-path",path_text.clone(),path_text).min_w_0().w_full().truncate().text_ellipsis_middle().text_size(px(11.)).text_color(rgb(colors().subtle)))
            .child(row().flex_wrap().gap_2().child(a11y_text("worktrees-detail-facts",presentation::preservation_label(facts)))
            .when(facts.build_output.is_some_and(|n|n>0), |el| el.child(hover_text("worktrees-build-output", "Build output".into(), "Ignored content inside the host’s currently approved directories. It goes with this worktree and does not block removal; other ignored content still does.".into())))
            .when_some(tree.build_output.as_ref(), |el, outcome| el.child(hover_text("worktrees-warmth", if outcome.warm {"Warm".into()}else{"Cold".into()}, format!("{} {} files · {} ms · {} skipped. {}", outcome.reason, outcome.counts.files, outcome.counts.elapsed_ms, outcome.counts.skipped, outcome.counts.notes.join("; "))))))
            .child(hover_text("worktrees-detail-reason",safe_text(&visible_reason),reason).text_size(px(12.)).text_color(rgb(colors().muted)))
            .child(bar)
            .child(hover_text("worktrees-detail-size",size.map_or_else(||size_label(self.worktrees.sizes[n].as_ref()),|s|format!("{} source · {} build output{}",bytes(s.source),bytes(s.build),if s.incomplete {" · lower bound"}else{""})),"Logical bytes, including private build cache. Shared Git storage excluded. Copy-on-write blocks may be shared; removing this worktree does not reclaim all these bytes.".into()))
            .child(a11y_text("worktrees-last-used",format!("Last used: {}",self.worktrees.ages[n]))))
    }

    fn worktree_row_text(&self, item: &Item, lane: usize, id: String, label: String) -> AnyElement {
        let (style, title_color) = match item {
            Item::Commit(r) => (&self.worktrees.commit_styles[*r], colors().muted),
            Item::Head(..) | Item::Detached(_) => {
                (&self.worktrees.head_styles[lane], colors().text)
            }
            Item::Changed(..) => {
                return div()
                    .text_color(rgb(colors().subtle))
                    .child(a11y_text(id, label))
                    .into_any_element();
            }
        };
        let highlights = [
            (style.title.clone(), title_color),
            (style.state.clone(), state_tint(self.worktrees.words[lane])),
        ]
        .into_iter()
        .filter(|(range, _)| !range.is_empty())
        .map(|(range, color)| {
            (
                range,
                HighlightStyle {
                    color: Some(rgb(color).into()),
                    ..Default::default()
                },
            )
        });
        div()
            .id(SharedString::from(id))
            .role(Role::Label)
            .aria_label(label.clone())
            .text_color(rgb(colors().subtle))
            .truncate()
            .child(
                StyledText::new(label)
                    .with_highlights(highlights)
                    .with_font_family_overrides([(style.code.clone(), mono())]),
            )
            .audited()
            .into_any_element()
    }
}

fn state_tint(word: &str) -> u32 {
    match word {
        "Working" => colors().blue,
        "Approve" | "Interrupted" => colors().amber,
        "Failed" => colors().red,
        "Ready" | "Done" => colors().mint,
        _ => colors().subtle,
    }
}

fn removal_advice(reason: &str, can_keep_local: bool) -> &str {
    if can_keep_local {
        "Confirm local"
    } else {
        reason
    }
}

fn hover_text(id: &'static str, label: String, explanation: String) -> Audited<Stateful<Div>> {
    div()
        .id(id)
        .role(Role::Label)
        .aria_label(label.clone())
        .tooltip(move |_, cx| hover(explanation.clone(), cx))
        .child(StyledText::new(label))
        .audited()
}

fn graph_stroke(stroke: &presentation::Stroke, tint: u32) -> Div {
    use presentation::Corner;
    let line = div()
        .absolute()
        .left(px(stroke.x))
        .top(px(stroke.y))
        .w(px(stroke.width))
        .h(px(stroke.height))
        .border_color(rgb(tint));
    match stroke.corner {
        None => line.bg(rgb(tint)),
        Some(Corner::TopLeft) => line.border_t_1().border_l_1().rounded_tl(px(6.)),
        Some(Corner::TopRight) => line.border_t_1().border_r_1().rounded_tr(px(6.)),
        Some(Corner::BottomLeft) => line.border_b_1().border_l_1().rounded_bl(px(6.)),
        Some(Corner::BottomRight) => line.border_b_1().border_r_1().rounded_br(px(6.)),
    }
}

// Match the conversation's one-point line, seven-point node and nine-point ring.
fn graph_mark(x: f32, hollow: bool, color: u32, ground: u32) -> Div {
    let size = if hollow { 9. } else { 7. };
    div()
        .absolute()
        .left(px(x + 0.5 - (size + 4.) / 2.))
        .top(px(14. - (size + 4.) / 2.))
        .size(px(size + 4.))
        .rounded_full()
        .bg(rgb(ground))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(size))
                .rounded_full()
                .bg(rgb(color))
                .flex()
                .items_center()
                .justify_center()
                .when(hollow, |el| {
                    el.child(div().size(px(size - 3.)).rounded_full().bg(rgb(ground)))
                }),
        )
}
