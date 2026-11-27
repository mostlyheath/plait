//! Project thread graph tab: revision-polled layout of linked threads and drafts.
//!
//! Host capability probing, revision polling, and draft mutations all run on the
//! background executor. Render reads prepared geometry from `plait-client`.

use super::{Plait, tab_lifecycle::Intent, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{
    graph_view::{CardBox, CardState, GraphViewState, PollOutcome, card_state},
    host::{
        self, ThreadSummary,
        graph::{EdgeKind, GraphNode, NodeKind, NodeLifecycle},
    },
    tabs::{TabTarget, bookmark::BookmarkRecord},
};
use std::{path::PathBuf, time::Duration};

actions!(plait, [Graph]);

/// Retry delay after a transient failure while probing host graph support.
const CAPABILITY_RETRY: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct GraphUi {
    pub view: GraphViewState,
    /// `(connection_epoch, supported)` once probed for the active host connection.
    probing: Option<u64>,
    pub capability: Option<(u64, bool)>,
    /// Tracks whether the native `View` menu currently includes `Graph`.
    pub menu_shown: bool,
    /// Draft selection restored from the tab bookmark on activation.
    pub restored: Option<String>,
    /// Bumped whenever a scheduled poll timer is replaced or cancelled.
    pub timer_generation: u64,
    pub scroll: ScrollHandle,
}

impl GraphUi {
    pub(super) fn park(&mut self) {
        self.timer_generation += 1;
        self.view.park();
    }

    pub(super) fn reset_for_reconnect(&mut self) {
        self.probing = None;
        self.capability = None;
        self.timer_generation += 1;
        self.view.park();
    }
}

#[derive(Clone, Copy)]
struct PaintEdge {
    from_x: f32,
    from_y: f32,
    to_x: f32,
    to_y: f32,
    dashed: bool,
    color: u32,
}

impl PaintEdge {
    fn clipped(
        &self,
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
    ) -> Option<((f32, f32), (f32, f32))> {
        let dx = self.to_x - self.from_x;
        let dy = self.to_y - self.from_y;
        let (mut enter, mut exit) = (0.0_f32, 1.0_f32);
        // Intersect the segment's parameter interval with both viewport axes.
        for (start, delta, min, max) in [
            (self.from_x, dx, left, right),
            (self.from_y, dy, top, bottom),
        ] {
            if delta == 0. {
                if start < min || start > max {
                    return None;
                }
            } else {
                let a = (min - start) / delta;
                let b = (max - start) / delta;
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        let at = |t| (self.from_x + t * dx, self.from_y + t * dy);
        (enter < exit).then(|| (at(enter), at(exit)))
    }
}

impl Plait {
    pub(super) fn graph_visible(&self) -> bool {
        self.tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .is_some_and(|tab| tab.session().target().is_graph())
    }

    /// True only once the active host connection has confirmed graph operations.
    pub(super) fn graph_supported(&self) -> bool {
        self.graph
            .capability
            .is_some_and(|(epoch, supported)| epoch == self.host.connection.epoch && supported)
    }

    pub(super) fn sync_graph_menu(&mut self, cx: &mut Context<Self>) {
        let supported = self.graph_supported();
        if !cfg!(test) && self.graph.menu_shown != supported {
            self.graph.menu_shown = supported;
            super::actions::set_graph_menu(cx, supported);
        }
    }

    /// Probes whether the connected host exposes graph operations (`PLAIT_GRAPH=1`).
    /// Older hosts and hosts without the flag return `invalid_request` and stay hidden.
    pub(super) fn sync_graph_capability(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(test) || !self.host.connection.connected {
            return;
        }
        let epoch = self.host.connection.epoch;
        if self.graph.probing == Some(epoch)
            || self
                .graph
                .capability
                .is_some_and(|(probed, _)| probed == epoch)
        {
            return;
        }
        let project = self.host.creation.checkout.clone();
        if !project.is_absolute() {
            return;
        }
        self.graph.probing = Some(epoch);
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { host::graph::probe(&client, project) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.host.connection.epoch != epoch {
                    return;
                }
                match result {
                    Ok(Some(_)) => {
                        this.graph.capability = Some((epoch, true));
                        this.graph.probing = None;
                        this.sync_graph_menu(cx);
                        this.try_restore_tabs(window, cx);
                    }
                    Ok(None) => {
                        this.graph.capability = Some((epoch, false));
                        let ids: Vec<_> = this
                            .tabs
                            .model
                            .tabs()
                            .iter()
                            .filter(|t| t.session().target().is_graph())
                            .map(|t| t.id())
                            .collect();
                        for id in ids {
                            this.request_tab(Intent::Close(id), window, cx);
                        }
                        this.graph.probing = None;
                        this.sync_graph_menu(cx);
                        this.try_restore_tabs(window, cx);
                    }
                    Err(_) => {
                        this.retry_graph_capability(epoch, CAPABILITY_RETRY, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retry_graph_capability(&mut self, epoch: u64, delay: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                if this.graph.probing == Some(epoch) {
                    this.graph.probing = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn open_graph(
        &mut self,
        project: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.graph_supported() {
            return;
        }
        self.navigation.menu = None;
        if self.settings_open {
            self.show_settings(false, window, cx);
        }
        let project = self.project_of(&project).to_owned();
        let target = TabTarget::Graph {
            checkout: self.tab_checkout(project),
        };
        if self.tabs.identity.verified && target.checkout().directory.is_some() {
            self.request_tab(Intent::Open(target), window, cx);
        } else {
            self.open_tab_target(target, window, cx);
        }
    }

    pub(super) fn restore_graph_bookmark(&mut self) {
        let bookmark = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .map(|tab| tab.session().bookmark());
        self.graph.restored = match bookmark {
            Some(BookmarkRecord::Graph { selected }) => selected.clone(),
            _ => None,
        };
    }

    fn save_graph_bookmark(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.model.active() {
            let result = self.tabs.model.save_bookmark(
                id,
                BookmarkRecord::Graph {
                    selected: self.graph.view.selected.clone(),
                },
            );
            self.tab_error(result);
            self.queue_tab_save(cx);
        }
    }

    /// Keeps the Graph tab targeted at the active project and kicks off its initial poll.
    pub(super) fn sync_graph_view(&mut self, cx: &mut Context<Self>) {
        if !self.graph_visible() {
            return;
        }
        let checkout = self.visible_project_checkout().to_owned();
        let restored = self.graph.restored.take();
        let changed = self.graph.view.target(
            self.host.connection.epoch,
            self.tabs.host.clone(),
            checkout,
            restored,
        );
        if cfg!(test) || !self.host.connection.connected {
            return;
        }
        if changed
            || (self.graph.view.snapshot.is_none()
                && !self.graph.view.polling
                && self.graph.view.error.is_none())
        {
            self.poll_graph(false, cx);
        }
    }

    fn poll_graph(&mut self, force_full: bool, cx: &mut Context<Self>) {
        if cfg!(test)
            || !self.graph_supported()
            || !self.graph_visible()
            || !self.host.connection.connected
        {
            return;
        }
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        let Some(host) = self.verified_host().cloned() else {
            return;
        };
        let machine = self.machines.active.clone();
        let Some((epoch, generation, project, revision)) = self.graph.view.begin_poll(force_full)
        else {
            return;
        };
        self.graph.timer_generation += 1;
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            let reply = host::graph::graph(&client, project.clone(), revision)?;
            let pending = journal
                .reconcile_graph_actions(&machine, &host, &project, &reply)
                .map_err(host::Error::Protocol)?;
            Ok((reply, pending))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let unsupported = matches!(
                    &result,
                    Err(host::Error::Rejected(error)) if error.code == "invalid_request"
                );
                if unsupported && this.host.connection.epoch == epoch {
                    this.graph.capability = None;
                    this.sync_graph_menu(cx);
                }
                let mapped = result.map_err(|error| {
                    if unsupported {
                        "Thread graph operations are not enabled on this host.".to_owned()
                    } else {
                        error.to_string()
                    }
                });
                match this.graph.view.apply_poll(epoch, generation, mapped) {
                    PollOutcome::Ignored => {}
                    PollOutcome::Applied {
                        changed,
                        refresh_now,
                        next_delay,
                    } => {
                        if changed {
                            cx.notify();
                        }
                        if !this.graph_visible() || unsupported {
                            return;
                        }
                        if refresh_now {
                            this.poll_graph(false, cx);
                        } else {
                            this.schedule_graph_poll(epoch, generation, next_delay, cx);
                        }
                    }
                }
            });
        })
        .detach();
    }

    fn schedule_graph_poll(
        &mut self,
        epoch: u64,
        generation: u64,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        self.graph.timer_generation += 1;
        let timer_generation = self.graph.timer_generation;
        let timer = cx.background_executor().timer(delay);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.graph.timer_generation == timer_generation
                    && this.graph.view.epoch == epoch
                    && this.graph.view.generation == generation
                    && this.graph_visible()
                    && this.host.connection.connected
                {
                    this.poll_graph(false, cx);
                }
            });
        })
        .detach();
    }

    /// Whether a thread-watch update touched the visible Graph tab's project.
    pub(super) fn graph_summaries_changed(&self, live: &[ThreadSummary]) -> bool {
        if !self.graph_visible() || !self.host.connection.connected {
            return false;
        }
        let visible = self.visible_project_checkout();
        live.iter().any(|thread| {
            thread.checkout == visible || self.project_of(&thread.checkout) == visible
        })
    }

    pub(super) fn refresh_graph_after_summaries(&mut self, cx: &mut Context<Self>) {
        if self.graph.view.on_project_threads_changed() {
            self.poll_graph(false, cx);
        }
    }

    pub(super) fn refresh_graph_now(&mut self, cx: &mut Context<Self>) {
        if !self.host.connection.connected {
            return;
        }
        self.graph.view.backoff.reset();
        self.poll_graph(true, cx);
        cx.notify();
    }

    fn open_graph_thread(
        &mut self,
        thread_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let checkout = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == thread_id)
            .map_or_else(
                || self.visible_project_checkout().to_owned(),
                |thread| thread.checkout.clone(),
            );
        self.close_overview(window, cx);
        self.open_tab_target(
            TabTarget::Existing {
                id: thread_id,
                checkout: self.tab_checkout(checkout),
            },
            window,
            cx,
        );
    }

    fn select_graph_node(&mut self, node_id: Option<String>, cx: &mut Context<Self>) {
        self.graph.view.select_node(node_id);
        self.save_graph_bookmark(cx);
        cx.notify();
    }

    fn start_graph_draft(&mut self, node_id: String, cx: &mut Context<Self>) {
        self.mutate_graph_draft(node_id, false, cx);
    }

    fn delete_graph_draft(&mut self, node_id: String, cx: &mut Context<Self>) {
        self.mutate_graph_draft(node_id, true, cx);
    }

    fn mutate_graph_draft(&mut self, node_id: String, delete: bool, cx: &mut Context<Self>) {
        if !self.host.connection.connected || !self.graph_supported() || !self.host_verified_here()
        {
            return;
        }
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        let Some(project) = self.graph.view.project.clone() else {
            return;
        };
        let retry = self.graph.view.retry_operation(&node_id) == Some(delete);
        let Some(host) = self.graph.view.begin_mutation(&node_id, delete) else {
            return;
        };
        let action =
            plait_client::session::GraphAction::new(host.clone(), node_id.clone(), project, delete);
        let machine = self.machines.active.clone();
        let submitted = node_id.clone();
        cx.notify();
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { journal.dispatch_graph_action(&machine, &action, retry, &client) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .graph
                    .view
                    .finish_mutation(host, submitted, result, delete)
                {
                    this.save_graph_bookmark(cx);
                    this.poll_graph(false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn graph_view(&self, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let body = if !self.host.connection.connected && self.graph.view.snapshot.is_none() {
            column()
                .flex_1()
                .min_h_0()
                .child(empty(
                    "Host offline",
                    "Connect the host to view this project's thread graph.",
                ))
                .into_any_element()
        } else if self.graph.view.loading && self.graph.view.snapshot.is_none() {
            column()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .child(tag(
                    "graph-loading",
                    "Loading thread graph…",
                    colors().subtle,
                ))
                .into_any_element()
        } else if let Some(error) = self
            .graph
            .view
            .error
            .clone()
            .filter(|_| self.graph.view.snapshot.is_none())
        {
            column()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_3()
                .child(empty("Could not load thread graph", error))
                .child(
                    toned_button("graph-retry", "Try again", Tone::Filled)
                        .on_activate(cx, |this, _, cx| this.refresh_graph_now(cx)),
                )
                .into_any_element()
        } else if self
            .graph
            .view
            .snapshot
            .as_ref()
            .is_none_or(|view| view.nodes.is_empty())
        {
            let unlinked = self.graph.view.snapshot.as_ref().map_or(0, |v| v.unlinked);
            column()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_3()
                .child(empty(
                    "No linked threads yet",
                    "Fork a thread or create a draft with dependencies to see the project's graph here.",
                ))
                .when(unlinked > 0, |el| {
                    el.child(
                        toned_button(
                            "graph-empty-overview",
                            format!("View {unlinked} unlinked in Every thread"),
                            Tone::Filled,
                        )
                        .on_activate(cx, |this, window, cx| this.toggle_overview(window, cx)),
                    )
                })
                .into_any_element()
        } else {
            let canvas_area = self.graph_canvas(cx);
            if let Some(selected) = self.graph.view.selected_node() {
                let detail = self.graph_draft_detail(selected, cx);
                if wide {
                    row()
                        .gap_0()
                        .flex_1()
                        .min_h_0()
                        .items_stretch()
                        .child(canvas_area)
                        .child(
                            detail
                                .w(px(340.))
                                .flex_shrink_0()
                                .border_l_1()
                                .border_color(rgb(colors().border)),
                        )
                        .into_any_element()
                } else {
                    column()
                        .flex_1()
                        .min_h_0()
                        .child(canvas_area)
                        .child(
                            detail
                                .h(px(236.))
                                .flex_shrink_0()
                                .border_t_1()
                                .border_color(rgb(colors().border)),
                        )
                        .into_any_element()
                }
            } else {
                canvas_area.into_any_element()
            }
        };

        column()
            .id("graph-view")
            .flex_1()
            .min_h_0()
            .child(self.graph_toolbar(cx))
            .when_some(
                self.graph
                    .view
                    .error
                    .clone()
                    .filter(|_| self.graph.view.snapshot.is_some()),
                |el, error| {
                    el.child(
                        div()
                            .px_4()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(colors().border))
                            .text_size(px(TYPE_CONTROL_SIZE))
                            .text_color(rgb(colors().amber))
                            .child(a11y_text("graph-refresh-error", error)),
                    )
                },
            )
            .child(body)
            .into_any_element()
    }

    fn graph_toolbar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let snapshot = self.graph.view.snapshot.as_ref();
        let node_count = snapshot.map_or(0, |view| view.nodes.len());
        let unlinked = snapshot.map_or(0, |view| view.unlinked);
        let truncated = snapshot.is_some_and(|view| view.truncated);
        let count_label = if node_count == 1 {
            "1 node".to_owned()
        } else {
            format!("{node_count} nodes")
        };

        row()
            .id("graph-toolbar")
            .h(px(44.))
            .flex_shrink_0()
            .px_4()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors().border))
            .child(
                div()
                    .text_size(px(TYPE_HEADER_SIZE))
                    .font_weight(TYPE_HEADER_WEIGHT)
                    .text_color(rgb(colors().strong))
                    .child(a11y_text("graph-heading", "Graph")),
            )
            .when(snapshot.is_some(), |el| {
                el.child(
                    tag("graph-count", count_label, colors().subtle).font_features(tabular_nums()),
                )
            })
            .when(unlinked > 0, |el| {
                el.child(
                    button("graph-every-thread", format!("{unlinked} unlinked"))
                        .h(px(24.))
                        .px(px(8.))
                        .aria_description("Open Every thread to see unlinked project threads.")
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                TextTooltip(
                                    "Threads without graph links appear in Every thread.".into(),
                                )
                            })
                            .into()
                        })
                        .on_activate(cx, |this, window, cx| this.toggle_overview(window, cx)),
                )
            })
            .when(truncated, |el| {
                el.child(tag("graph-truncated", "Showing first 500", colors().amber))
            })
            .child(div().flex_1().min_w_0())
            .child(
                button("graph-refresh", "Refresh")
                    .disabled(!self.host.connection.connected)
                    .when(!self.host.connection.connected, |el| {
                        el.aria_description("Connect the host to refresh the thread graph.")
                    })
                    .tooltip(|_, cx| {
                        cx.new(|_| TextTooltip("Read the project's thread graph again.".into()))
                            .into()
                    })
                    .on_activate(cx, |this, _, cx| this.refresh_graph_now(cx)),
            )
    }

    fn graph_canvas(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(snapshot) = self.graph.view.snapshot.as_ref() else {
            return div().id("graph-canvas").flex_1().min_w_0().min_h_0();
        };
        let geometry = &self.graph.view.geometry;
        let palette = colors();
        let paint_edges: Vec<PaintEdge> = geometry
            .edges
            .iter()
            .map(|edge| PaintEdge {
                from_x: edge.from_x,
                from_y: edge.from_y,
                to_x: edge.to_x,
                to_y: edge.to_y,
                dashed: edge.kind == EdgeKind::DependsOn,
                color: match edge.kind {
                    EdgeKind::Fork => palette.edge,
                    EdgeKind::DependsOn => palette.subtle,
                    EdgeKind::Unsupported => palette.faint,
                },
            })
            .collect();

        let edges_layer = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let origin = bounds.origin;
                let mask = window.content_mask().bounds;
                for edge in &paint_edges {
                    let Some((from, to)) = edge.clipped(
                        f32::from(mask.left() - origin.x),
                        f32::from(mask.top() - origin.y),
                        f32::from(mask.right() - origin.x),
                        f32::from(mask.bottom() - origin.y),
                    ) else {
                        continue;
                    };
                    let from = origin + point(px(from.0), px(from.1));
                    let to = origin + point(px(to.0), px(to.1));
                    let mut builder = PathBuilder::stroke(px(1.5));
                    if edge.dashed {
                        builder = builder.dash_array(&[px(5.), px(4.)]);
                    }
                    builder.move_to(from);
                    builder.line_to(to);
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, rgb(edge.color));
                    }
                }
            },
        )
        .absolute()
        .size_full();

        let mut surface = div()
            .relative()
            .w(px(geometry.width))
            .h(px(geometry.height))
            .flex_shrink_0()
            .child(edges_layer);

        for card in &geometry.cards {
            let node = &snapshot.nodes[card.node_index];
            surface = surface.child(self.graph_card(node, card, cx));
        }

        div()
            .id("graph-canvas")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&self.graph.scroll)
            .child(surface)
    }

    fn graph_card(&self, node: &GraphNode, placed: &CardBox, cx: &mut Context<Self>) -> AnyElement {
        let summary = node
            .thread_id
            .as_deref()
            .and_then(|id| self.thread_index(id))
            .and_then(|index| self.host.connection.threads().get(index));
        let state = card_state(node, summary);
        let state_label = state.label();
        let state_color = graph_state_color(&state);
        let selected = self.graph.view.selected.as_deref() == Some(node.id.as_str());
        let palette = colors();
        let border = if selected {
            rgb(palette.blue)
        } else if matches!(state, CardState::Failed | CardState::Blocked { .. }) {
            rgba((palette.red << 8) | 0x8c)
        } else if state.needs_attention() {
            rgba((palette.amber << 8) | 0x8c)
        } else if state.is_running() {
            rgba((palette.blue << 8) | 0x73)
        } else {
            rgb(palette.edge)
        };

        let title = if node.title.trim().is_empty() {
            "Untitled".to_owned()
        } else {
            plait_client::display_text::escape(&node.title)
        };
        let is_thread = node.lifecycle == NodeLifecycle::Thread && node.thread_id.is_some();
        let aria_label = format!(
            "{title}. {}, {}. {}",
            node.kind.label(),
            state.spoken(),
            if is_thread {
                "Open thread"
            } else {
                "Inspect draft"
            }
        );

        let activate_thread = node
            .thread_id
            .clone()
            .filter(|_| node.lifecycle == NodeLifecycle::Thread);
        let activate_node_id = node.id.clone();
        let key_thread = activate_thread.clone();
        let key_node_id = activate_node_id.clone();
        let tip_title = title.clone();
        let reason_text = state
            .blocked_reason()
            .map(plait_client::display_text::escape);
        let foot_model = node
            .model_id
            .as_deref()
            .map(plait_client::display_text::escape);

        column()
            .id(SharedString::from(format!("graph-node-{}", node.id)))
            .role(Role::Button)
            .aria_label(aria_label)
            .focusable()
            .tab_stop(true)
            .cursor_pointer()
            .absolute()
            .left(px(placed.x))
            .top(px(placed.y))
            .w(px(placed.width))
            .h(px(placed.height))
            .px_3()
            .py_2()
            .gap_1()
            .rounded(px(SURFACE_CORNER))
            .border_1()
            .border_color(border)
            .bg(rgb(if selected {
                palette.selected
            } else {
                palette.panel
            }))
            .hover(move |style| {
                if selected {
                    style
                } else {
                    style
                        .bg(rgb(palette.raised))
                        .border_color(rgb(palette.subtle))
                }
            })
            .focus_visible(focused_control)
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(tip_title.clone())).into())
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    if let Some(thread_id) = &key_thread {
                        this.open_graph_thread(thread_id.clone(), window, cx);
                    } else {
                        this.select_graph_node(Some(key_node_id.clone()), cx);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .on_activate(cx, move |this, window, cx| {
                if let Some(thread_id) = &activate_thread {
                    this.open_graph_thread(thread_id.clone(), window, cx);
                } else {
                    this.select_graph_node(Some(activate_node_id.clone()), cx);
                }
            })
            .child(
                row()
                    .h(px(16.))
                    .gap(px(5.))
                    .child(graph_state_mark(
                        &state,
                        state_color,
                        self.host.connection.connected,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(TYPE_META_SIZE))
                            .font_weight(TYPE_META_WEIGHT)
                            .text_color(rgb(state_color))
                            .child(a11y_text(
                                SharedString::from(format!("graph-node-{}-state", node.id)),
                                shared_cow(state_label),
                            )),
                    )
                    .child(
                        tag(
                            SharedString::from(format!("graph-node-{}-kind", node.id)),
                            node.kind.label(),
                            palette.subtle,
                        )
                        .flex_shrink_0(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_size(px(TYPE_ROW_SIZE))
                    .line_height(px(18.))
                    .font_weight(TYPE_CARD_TITLE_WEIGHT)
                    .text_color(rgb(palette.strong))
                    .line_clamp(if reason_text.is_some() { 1 } else { 2 })
                    .child(a11y_text(
                        SharedString::from(format!("graph-node-{}-title", node.id)),
                        title,
                    )),
            )
            .when_some(reason_text, |el, reason| {
                let full_reason = reason.clone();
                el.child(
                    div()
                        .id(SharedString::from(format!(
                            "graph-node-{}-reason-box",
                            node.id
                        )))
                        .min_w_0()
                        .truncate()
                        .text_size(px(TYPE_META_SIZE))
                        .text_color(rgb(palette.red))
                        .tooltip(move |_, cx| cx.new(|_| TextTooltip(full_reason.clone())).into())
                        .child(a11y_text(
                            SharedString::from(format!("graph-node-{}-reason", node.id)),
                            reason,
                        )),
                )
            })
            .child(
                row()
                    .h(px(16.))
                    .gap(px(6.))
                    .text_size(px(TYPE_META_SIZE))
                    .text_color(rgb(palette.subtle))
                    .when_some(node.harness.and_then(harness_logo), |el, logo| {
                        el.child(logo.size(px(11.)).text_color(rgb(palette.subtle)))
                    })
                    .when_some(foot_model, |el, model| {
                        el.child(div().flex_1().min_w_0().truncate().child(a11y_text(
                            SharedString::from(format!("graph-node-{}-model", node.id)),
                            model,
                        )))
                    })
                    .when(node.model_id.is_none(), |el| el.child(div().flex_1()))
                    .when(node.unblocks > 0, |el| {
                        el.child(
                            tag(
                                SharedString::from(format!("graph-node-{}-unblocks", node.id)),
                                format!("unblocks {}", node.unblocks),
                                palette.subtle,
                            )
                            .flex_shrink_0(),
                        )
                    })
                    .when(node.finished_children > 0, |el| {
                        el.child(
                            tag(
                                SharedString::from(format!("graph-node-{}-children", node.id)),
                                format!("{} done", node.finished_children),
                                palette.mint,
                            )
                            .flex_shrink_0(),
                        )
                    }),
            )
            .audited()
            .into_any_element()
    }

    fn graph_draft_detail(&self, node: &GraphNode, cx: &mut Context<Self>) -> Stateful<Div> {
        let palette = colors();
        let state = card_state(node, None);
        let state_color = graph_state_color(&state);
        let title = if node.title.trim().is_empty() {
            "Untitled draft".to_owned()
        } else {
            plait_client::display_text::escape(&node.title)
        };
        let retry = self.graph.view.retry_operation(&node.id);
        let connected = self.host.connection.connected;
        let start_blocker = if !connected {
            Some("Connect the host to start this draft.")
        } else if self.graph.view.is_pending(&node.id, false) {
            Some("Updating draft…")
        } else if node.kind == NodeKind::Discuss {
            Some("Read-only Discuss execution is not available on this host yet.")
        } else if node.kind == NodeKind::Unsupported {
            Some("This draft uses a node kind this desktop build does not support.")
        } else {
            None
        };
        let delete_blocker = if !connected {
            Some("Connect the host to delete this draft.")
        } else if self.graph.view.is_pending(&node.id, true) {
            Some("Updating draft…")
        } else {
            None
        };

        let start_id = node.id.clone();
        let delete_id = node.id.clone();
        let prompt_text = node
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(plait_client::display_text::escape);
        let blocked_reason = state
            .blocked_reason()
            .map(plait_client::display_text::escape);

        column()
            .id("graph-draft-detail")
            .p_4()
            .gap_3()
            .overflow_y_scroll()
            .child(
                row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(TYPE_HEADER_SIZE))
                            .line_height(px(TYPE_HEADER_LINE))
                            .font_weight(TYPE_HEADER_WEIGHT)
                            .text_color(rgb(palette.strong))
                            .child(a11y_text("graph-detail-title", title)),
                    )
                    .child(
                        icon_button("graph-detail-close", "close", "Close draft details")
                            .on_activate(cx, |this, _, cx| this.select_graph_node(None, cx)),
                    ),
            )
            .child(
                row()
                    .flex_wrap()
                    .gap_2()
                    .child(graph_state_mark(&state, state_color, connected))
                    .child(tag(
                        "graph-detail-state",
                        shared_cow(state.label()),
                        state_color,
                    ))
                    .child(tag("graph-detail-kind", node.kind.label(), palette.muted))
                    .when_some(node.harness, |el, harness| {
                        el.child(tag(
                            "graph-detail-harness",
                            harness.label(),
                            palette.subtle,
                        ))
                    })
                    .when_some(node.model_id.as_deref(), |el, model| {
                        el.child(tag(
                            "graph-detail-model",
                            plait_client::display_text::escape(model),
                            palette.subtle,
                        ))
                    })
                    .child(tag(
                        "graph-detail-auto",
                        if node.start_when_ready {
                            "Starts when ready"
                        } else {
                            "Manual start"
                        },
                        palette.subtle,
                    )),
            )
            .when_some(blocked_reason, |el, reason| {
                el.child(
                    div()
                        .p_2()
                        .rounded(px(CONTROL_CORNER))
                        .bg(rgba((palette.red << 8) | 0x1c))
                        .border_1()
                        .border_color(rgba((palette.red << 8) | 0x66))
                        .text_size(px(TYPE_CONTROL_SIZE))
                        .line_height(px(TYPE_CONTROL_LINE))
                        .text_color(rgb(palette.red))
                        .child(a11y_text("graph-detail-blocked", reason)),
                )
            })
            .when(node.waiting.total > 0, |el| {
                el.child(tag(
                    "graph-detail-waiting",
                    format!(
                        "Dependencies complete: {} of {}",
                        node.waiting.done, node.waiting.total
                    ),
                    palette.muted,
                ))
            })
            .child(
                column()
                    .gap_1()
                    .child(section_label("graph-detail-prompt-label", "Prompt"))
                    .child(
                        div()
                            .p_3()
                            .rounded(px(CONTROL_CORNER))
                            .bg(rgb(palette.rail))
                            .border_1()
                            .border_color(rgb(palette.border))
                            .text_size(px(TYPE_CONTROL_SIZE))
                            .line_height(px(TYPE_CONTROL_LINE))
                            .text_color(rgb(if prompt_text.is_some() {
                                palette.text
                            } else {
                                palette.subtle
                            }))
                            .child(a11y_text(
                                "graph-detail-prompt",
                                prompt_text.unwrap_or_else(|| {
                                    "Prompt text is stored on the host and not included in this graph snapshot."
                                        .to_owned()
                                }),
                            )),
                    ),
            )
            .when_some(self.graph.view.action_error.clone(), |el, error| {
                el.child(
                    div()
                        .id("graph-detail-error")
                        .role(Role::Alert)
                        .audited()
                        .text_size(px(TYPE_CONTROL_SIZE))
                        .text_color(rgb(palette.red))
                        .child(error),
                )
            })
            .child(
                row()
                    .gap_2()
                    .child(
                        toned_button("graph-draft-start", if retry == Some(false) {"Retry Start"} else {"Start"}, Tone::Primary)
                            .disabled(start_blocker.is_some())
                            .when_some(start_blocker, |el, reason| {
                                el.aria_description(reason).tooltip(move |_, cx| {
                                    cx.new(|_| TextTooltip(reason.to_owned())).into()
                                })
                            })
                            .on_activate(cx, move |this, _, cx| {
                                if start_blocker.is_none() {
                                    this.start_graph_draft(start_id.clone(), cx);
                                }
                            }),
                    )
                    .child(
                        button("graph-draft-delete", if retry == Some(true) {"Retry Delete"} else {"Delete"})
                            .text_color(rgb(palette.red))
                            .disabled(delete_blocker.is_some())
                            .when_some(delete_blocker, |el, reason| {
                                el.aria_description(reason).tooltip(move |_, cx| {
                                    cx.new(|_| TextTooltip(reason.to_owned())).into()
                                })
                            })
                            .on_activate(cx, move |this, _, cx| {
                                if delete_blocker.is_none() {
                                    this.delete_graph_draft(delete_id.clone(), cx);
                                }
                            }),
                    ),
            )
    }
}

fn shared_cow(text: std::borrow::Cow<'static, str>) -> SharedString {
    match text {
        std::borrow::Cow::Borrowed(s) => SharedString::new_static(s),
        std::borrow::Cow::Owned(s) => SharedString::from(s),
    }
}

fn graph_state_color(state: &CardState) -> u32 {
    let palette = colors();
    match state {
        CardState::Draft | CardState::Waiting { .. } => palette.subtle,
        CardState::Ready | CardState::Done => palette.mint,
        CardState::Blocked { .. } | CardState::Failed => palette.red,
        CardState::Running { .. } => palette.blue,
        CardState::NeedsApproval | CardState::NeedsYou { .. } => palette.amber,
        CardState::Unsupported => palette.faint,
    }
}

fn graph_state_mark(state: &CardState, color: u32, connected: bool) -> AnyElement {
    match state {
        CardState::Running { .. } if connected => running_mark(px(11.)).into_any_element(),
        CardState::Draft | CardState::Waiting { .. } | CardState::Unsupported => div()
            .size(px(7.))
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .border_color(rgb(color))
            .into_any_element(),
        _ => dot(color).into_any_element(),
    }
}

#[cfg(test)]
mod edge_tests {
    use super::PaintEdge;
    use plait_client::graph_view::{
        CANVAS_PADDING, CARD_HEIGHT, CARD_WIDTH, COLUMN_GAP, LAYER_GAP,
    };

    #[test]
    fn shared_prerequisites_bound_visible_dashes() {
        let center =
            |column| CANVAS_PADDING + CARD_WIDTH / 2. + column as f32 * (CARD_WIDTH + COLUMN_GAP);
        let mut dashes = 0;
        for prerequisite in 0..4 {
            for draft in 0..496 {
                let edge = PaintEdge {
                    from_x: center(prerequisite),
                    from_y: CANVAS_PADDING + CARD_HEIGHT,
                    to_x: center(draft),
                    to_y: CANVAS_PADDING + CARD_HEIGHT + LAYER_GAP,
                    dashed: true,
                    color: 0,
                };
                if let Some(((x, y), (u, v))) = edge.clipped(0., 0., 900., 600.) {
                    dashes += ((u - x).hypot(v - y) / 9.).ceil() as usize;
                }
            }
        }
        assert!((1..100_000).contains(&dashes), "{dashes} visible dashes");
    }
}
