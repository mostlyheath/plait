//! Headless thread-graph view state, layout projection, and poll backoff.
//!
//! No GPUI or I/O here: the UI reads prepared geometry and dispatches host
//! requests on the background executor, feeding results back with epoch and
//! generation checks.

use crate::host::{
    ThreadStatus, ThreadSummary,
    graph::{
        EdgeKind, GraphNode, GraphReply, GraphView, MutationReply, NodeKind, NodeLifecycle,
        NodeStatus,
    },
};
use crate::tabs::HostKey;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

pub const CARD_WIDTH: f32 = 240.0;
pub const CARD_HEIGHT: f32 = 104.0;
pub const COLUMN_GAP: f32 = 28.0;
pub const LAYER_GAP: f32 = 44.0;
pub const CANVAS_PADDING: f32 = 24.0;

/// Unchanged-poll backoff intervals in seconds.
const POLL_STEPS_SECS: [u64; 5] = [2, 4, 8, 15, 30];

/// Exponential backoff for revision-based graph polling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PollBackoff {
    step: usize,
}

impl PollBackoff {
    pub fn reset(&mut self) {
        self.step = 0;
    }

    pub fn current(&self) -> Duration {
        Duration::from_secs(POLL_STEPS_SECS[self.step.min(POLL_STEPS_SECS.len() - 1)])
    }

    pub fn on_changed(&mut self) -> Duration {
        self.reset();
        self.current()
    }

    pub fn on_unchanged(&mut self) -> Duration {
        let delay = self.current();
        self.step = (self.step + 1).min(POLL_STEPS_SECS.len() - 1);
        delay
    }

    pub fn on_error(&mut self) -> Duration {
        self.step = (self.step + 1).min(POLL_STEPS_SECS.len() - 1);
        self.current()
    }
}

/// Display state of a graph card, combining host graph status with the live thread catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardState {
    Draft,
    Waiting { done: u32, total: u32 },
    Ready,
    Blocked { reason: String },
    Running { kind: NodeKind },
    NeedsApproval,
    Failed,
    NeedsYou { label: &'static str },
    Done,
    Unsupported,
}

impl CardState {
    pub fn label(&self) -> Cow<'static, str> {
        match self {
            Self::Draft => Cow::Borrowed("Draft"),
            Self::Waiting { done, total } => Cow::Owned(format!("Waiting {done}/{total}")),
            Self::Ready => Cow::Borrowed("Ready"),
            Self::Blocked { .. } => Cow::Borrowed("Blocked"),
            Self::Running {
                kind: NodeKind::Discuss,
            } => Cow::Borrowed("Discussing"),
            Self::Running {
                kind: NodeKind::Build,
            } => Cow::Borrowed("Building"),
            Self::Running {
                kind: NodeKind::Unsupported,
            } => Cow::Borrowed("Running"),
            Self::NeedsApproval => Cow::Borrowed("Needs approval"),
            Self::Failed => Cow::Borrowed("Failed"),
            Self::NeedsYou { label } => Cow::Borrowed(label),
            Self::Done => Cow::Borrowed("Done"),
            Self::Unsupported => Cow::Borrowed("Unsupported"),
        }
    }

    pub fn blocked_reason(&self) -> Option<&str> {
        match self {
            Self::Blocked { reason } if !reason.is_empty() => Some(reason.as_str()),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            Self::NeedsApproval | Self::Failed | Self::NeedsYou { .. }
        )
    }

    pub fn spoken(&self) -> String {
        match self {
            Self::Blocked { reason } if !reason.is_empty() => format!("Blocked: {reason}"),
            Self::Running {
                kind: NodeKind::Discuss,
            } => "Running, Discussing".into(),
            Self::Running {
                kind: NodeKind::Build,
            } => "Running, Building".into(),
            _ => self.label().into_owned(),
        }
    }
}

/// Computes the card presentation state from the graph snapshot and live thread summary.
pub fn card_state(node: &GraphNode, summary: Option<&ThreadSummary>) -> CardState {
    match node.status {
        NodeStatus::Draft => CardState::Draft,
        NodeStatus::Waiting => CardState::Waiting {
            done: node.waiting.done,
            total: node.waiting.total,
        },
        NodeStatus::Ready => CardState::Ready,
        NodeStatus::Blocked => {
            let reason = node
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    node.blocked_by
                        .as_ref()
                        .map(|dep| format!("Waiting on {}", dep.title))
                })
                .unwrap_or_default();
            CardState::Blocked { reason }
        }
        NodeStatus::Discussing => CardState::Running {
            kind: NodeKind::Discuss,
        },
        NodeStatus::Building => CardState::Running {
            kind: NodeKind::Build,
        },
        NodeStatus::NeedsYou => match summary {
            Some(thread)
                if thread.pending_approval == Some(true)
                    || thread.status == ThreadStatus::AwaitingApproval =>
            {
                CardState::NeedsApproval
            }
            Some(thread) if thread.status == ThreadStatus::Error => CardState::Failed,
            Some(thread) if thread.status == ThreadStatus::Interrupted => CardState::NeedsYou {
                label: "Interrupted",
            },
            Some(thread) if thread.status == ThreadStatus::Stopped => {
                CardState::NeedsYou { label: "Stopped" }
            }
            _ => CardState::NeedsYou { label: "Needs you" },
        },
        NodeStatus::Done => CardState::Done,
        NodeStatus::Unsupported => CardState::Unsupported,
    }
}

/// Positioned card box in graph canvas coordinates (points).
#[derive(Clone, Debug, PartialEq)]
pub struct CardBox {
    pub node_index: usize,
    pub layer: u32,
    pub column: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Straight edge segment in graph canvas coordinates (points).
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeLine {
    pub kind: EdgeKind,
    pub from_x: f32,
    pub from_y: f32,
    pub to_x: f32,
    pub to_y: f32,
}

/// Precomputed layout geometry for the visible graph snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphGeometry {
    pub cards: Vec<CardBox>,
    pub edges: Vec<EdgeLine>,
    pub width: f32,
    pub height: f32,
}

impl GraphGeometry {
    pub fn from_view(view: &GraphView) -> Self {
        if view.nodes.is_empty() {
            return Self::default();
        }
        let node_indices: HashMap<&str, usize> = view
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();

        let mut cards = Vec::with_capacity(view.nodes.len());
        let mut by_id: HashMap<&str, usize> = HashMap::with_capacity(view.nodes.len());
        let mut seen: HashSet<usize> = HashSet::with_capacity(view.nodes.len());
        let mut max_layer = 0u32;
        let mut max_column = 0u32;

        for placed in &view.layout.placed {
            let Some(&node_index) = node_indices.get(placed.id.as_str()) else {
                continue;
            };
            if !seen.insert(node_index) {
                continue;
            }
            max_layer = max_layer.max(placed.layer);
            max_column = max_column.max(placed.column);
            let card = card_box(node_index, placed.layer, placed.column);
            by_id.insert(view.nodes[node_index].id.as_str(), cards.len());
            cards.push(card);
        }

        // Any node missing from `layout.placed` lands on a fallback bottom row so
        // a partial layout never hides a card.
        let fallback_layer = if cards.is_empty() { 0 } else { max_layer + 1 };
        let mut fallback_col = 0u32;
        for (node_index, node) in view.nodes.iter().enumerate() {
            if seen.contains(&node_index) {
                continue;
            }
            max_layer = max_layer.max(fallback_layer);
            max_column = max_column.max(fallback_col);
            let card = card_box(node_index, fallback_layer, fallback_col);
            by_id.insert(node.id.as_str(), cards.len());
            cards.push(card);
            fallback_col += 1;
        }

        let mut edges = Vec::with_capacity(view.edges.len());
        for edge in &view.edges {
            let (Some(&from_ix), Some(&to_ix)) =
                (by_id.get(edge.from.as_str()), by_id.get(edge.to.as_str()))
            else {
                continue;
            };
            let from = &cards[from_ix];
            let to = &cards[to_ix];
            let (from_x, from_y, to_x, to_y) = if to.y > from.y {
                (
                    from.x + from.width * 0.5,
                    from.y + from.height,
                    to.x + to.width * 0.5,
                    to.y,
                )
            } else if to.y < from.y {
                (
                    from.x + from.width * 0.5,
                    from.y,
                    to.x + to.width * 0.5,
                    to.y + to.height,
                )
            } else if to.x >= from.x {
                (
                    from.x + from.width,
                    from.y + from.height * 0.5,
                    to.x,
                    to.y + to.height * 0.5,
                )
            } else {
                (
                    from.x,
                    from.y + from.height * 0.5,
                    to.x + to.width,
                    to.y + to.height * 0.5,
                )
            };
            edges.push(EdgeLine {
                kind: edge.kind,
                from_x,
                from_y,
                to_x,
                to_y,
            });
        }

        let layers = view.layout.layers.max(max_layer + 1);
        let columns = view.layout.columns.max(max_column + 1);
        let width = CANVAS_PADDING * 2.0
            + columns as f32 * CARD_WIDTH
            + columns.saturating_sub(1) as f32 * COLUMN_GAP;
        let height = CANVAS_PADDING * 2.0
            + layers as f32 * CARD_HEIGHT
            + layers.saturating_sub(1) as f32 * LAYER_GAP;

        Self {
            cards,
            edges,
            width,
            height,
        }
    }
}

fn card_box(node_index: usize, layer: u32, column: u32) -> CardBox {
    CardBox {
        node_index,
        layer,
        column,
        x: CANVAS_PADDING + column as f32 * (CARD_WIDTH + COLUMN_GAP),
        y: CANVAS_PADDING + layer as f32 * (CARD_HEIGHT + LAYER_GAP),
        width: CARD_WIDTH,
        height: CARD_HEIGHT,
    }
}

/// Outcome of applying a completed poll reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// Stale epoch or generation: ignore completely.
    Ignored,
    /// Poll applied; schedule the next poll after `next_delay` (or immediately if `refresh_now`).
    Applied {
        changed: bool,
        refresh_now: bool,
        next_delay: Duration,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct PendingDraft {
    pub node: String,
    /// An unacknowledged action can replay its original operation.
    pub retry_delete: Option<bool>,
}

/// Headless state machine for the active project's Graph tab.
#[derive(Default)]
pub struct GraphViewState {
    pub epoch: u64,
    pub project: Option<PathBuf>,
    pub generation: u64,
    pub revision: Option<u64>,
    pub snapshot: Option<GraphView>,
    pub geometry: GraphGeometry,
    pub selected: Option<String>,
    pub loading: bool,
    pub polling: bool,
    pub pending: Vec<PendingDraft>,
    host: Option<HostKey>,
    in_flight: HashSet<(HostKey, String)>,
    acknowledged: HashMap<HostKey, u64>,
    pub error: Option<String>,
    pub action_error: Option<String>,
    pub backoff: PollBackoff,
    pub needs_refresh: bool,
}

impl GraphViewState {
    /// Points the view at `(epoch, project)`. Returns `true` when the target changed.
    pub fn target(
        &mut self,
        epoch: u64,
        host: HostKey,
        project: PathBuf,
        restored_selected: Option<String>,
    ) -> bool {
        if self.epoch == epoch
            && self.host.as_ref() == Some(&host)
            && self.project.as_ref() == Some(&project)
        {
            return false;
        }
        self.host = Some(host);
        self.epoch = epoch;
        self.project = Some(project);
        self.generation += 1;
        self.revision = None;
        self.snapshot = None;
        self.geometry = GraphGeometry::default();
        self.selected = restored_selected;
        self.loading = false;
        self.polling = false;
        self.pending.clear();
        self.error = None;
        self.action_error = None;
        self.backoff.reset();
        self.needs_refresh = false;
        true
    }

    /// Drops resident graph payload and retires any in-flight poll when the tab is parked.
    pub fn park(&mut self) {
        self.generation += 1;
        self.host = None;
        self.project = None;
        self.revision = None;
        self.snapshot = None;
        self.geometry = GraphGeometry::default();
        self.loading = false;
        self.polling = false;
        self.pending.clear();
        self.error = None;
        self.action_error = None;
        self.backoff.reset();
        self.needs_refresh = false;
    }

    /// Starts a revision-checked poll if none is already in flight.
    pub fn begin_poll(&mut self, force_full: bool) -> Option<(u64, u64, PathBuf, Option<u64>)> {
        if self.polling {
            return None;
        }
        let project = self.project.clone()?;
        self.polling = true;
        self.needs_refresh = false;
        if self.snapshot.is_none() {
            self.loading = true;
        }
        let revision = if force_full { None } else { self.revision };
        Some((self.epoch, self.generation, project, revision))
    }

    /// Applies a completed poll result, guarding against stale epochs/generations.
    pub fn apply_poll(
        &mut self,
        epoch: u64,
        generation: u64,
        result: Result<(GraphReply, Vec<PendingDraft>), String>,
    ) -> PollOutcome {
        if epoch != self.epoch || generation != self.generation {
            return PollOutcome::Ignored;
        }
        self.polling = false;
        self.loading = false;
        let refresh_now = std::mem::take(&mut self.needs_refresh);
        if let Ok((reply, _)) = &result
            && self
                .host
                .as_ref()
                .and_then(|host| self.acknowledged.get(host))
                .is_some_and(|revision| *revision > reply.graph_revision())
        {
            return PollOutcome::Applied {
                changed: false,
                refresh_now,
                next_delay: self.backoff.on_error(),
            };
        }
        match result {
            Ok((GraphReply::Unchanged { graph_revision }, pending)) => {
                let changed = self.pending != pending || self.error.is_some();
                self.pending = pending;
                self.revision = Some(graph_revision);
                self.error = None;
                let next_delay = self.backoff.on_unchanged();
                PollOutcome::Applied {
                    changed,
                    refresh_now,
                    next_delay,
                }
            }
            Ok((GraphReply::View(view), pending)) => {
                self.pending = pending;
                self.revision = Some(view.graph_revision);
                self.error = None;
                self.install_snapshot(view);
                let next_delay = self.backoff.on_changed();
                PollOutcome::Applied {
                    changed: true,
                    refresh_now,
                    next_delay,
                }
            }
            Err(message) => {
                self.error = Some(message);
                let next_delay = self.backoff.on_error();
                PollOutcome::Applied {
                    changed: true,
                    refresh_now: false,
                    next_delay,
                }
            }
        }
    }

    /// Called when thread-watch reports a thread change in the visible project.
    /// Returns `true` when the caller should start an immediate poll now.
    pub fn on_project_threads_changed(&mut self) -> bool {
        if self.project.is_none() {
            return false;
        }
        self.backoff.reset();
        if self.polling {
            self.needs_refresh = true;
            false
        } else {
            true
        }
    }

    pub fn select_node(&mut self, node_id: Option<String>) {
        self.selected = node_id;
        self.action_error = None;
    }

    pub fn selected_node(&self) -> Option<&GraphNode> {
        let selected = self.selected.as_deref()?;
        self.snapshot
            .as_ref()?
            .nodes
            .iter()
            .find(|node| node.id == selected)
    }

    pub fn retry_operation(&self, node: &str) -> Option<bool> {
        self.pending
            .iter()
            .find(|p| p.node == node)
            .and_then(|p| p.retry_delete)
    }

    pub fn is_pending(&self, node: &str, delete: bool) -> bool {
        self.pending
            .iter()
            .any(|p| p.node == node && p.retry_delete != Some(delete))
            || self
                .host
                .as_ref()
                .is_some_and(|host| self.in_flight.contains(&(host.clone(), node.to_owned())))
    }

    pub fn begin_mutation(&mut self, node: &str, delete: bool) -> Option<HostKey> {
        let host = self.host.clone()?;
        if self.is_pending(node, delete) {
            return None;
        }
        self.in_flight.insert((host.clone(), node.to_owned()));
        self.action_error = None;
        Some(host)
    }

    pub fn finish_mutation(
        &mut self,
        host: HostKey,
        node: String,
        result: Result<MutationReply, String>,
        deleted: bool,
    ) -> bool {
        if !self.in_flight.remove(&(host.clone(), node.clone())) {
            return false;
        }
        let visible = self.host.as_ref() == Some(&host);
        match result {
            Ok(reply) => {
                let floor = self.acknowledged.entry(host).or_default();
                *floor = (*floor).max(reply.graph_revision);
                if !visible {
                    return false;
                }
                self.pending.retain(|p| p.node != node);
                self.pending.push(PendingDraft {
                    node: node.clone(),
                    retry_delete: None,
                });
                self.action_error = None;
                if deleted && self.selected.as_deref() == Some(&node) {
                    self.selected = None;
                }
            }
            Err(message) if visible => self.action_error = Some(message),
            Err(_) => return false,
        }
        // Reload the reservation after every outcome, including a lost reply.
        self.generation += 1;
        self.polling = false;
        self.revision = None;
        self.backoff.reset();
        true
    }

    /// Installs an injected snapshot for test scenes (never called by shipped views).
    pub fn install_scene(&mut self, project: PathBuf, snapshot: GraphView) {
        self.project = Some(project);
        self.revision = Some(snapshot.graph_revision);
        self.loading = false;
        self.polling = false;
        self.error = None;
        self.action_error = None;
        self.install_snapshot(snapshot);
    }

    fn install_snapshot(&mut self, view: GraphView) {
        if let Some(selected) = self.selected.as_deref() {
            let keep = view
                .nodes
                .iter()
                .any(|node| node.id == selected && node.lifecycle != NodeLifecycle::Thread);
            if !keep {
                self.selected = None;
            }
        }
        self.geometry = GraphGeometry::from_view(&view);
        self.snapshot = Some(view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::graph::{BlockedBy, GraphEdge, GraphLayout, PlacedNode, WaitingCount};

    fn test_host() -> HostKey {
        HostKey {
            id: "host".into(),
            store: "/store".into(),
        }
    }

    fn sample_view() -> GraphView {
        GraphView {
            nodes: vec![
                GraphNode {
                    id: "root".into(),
                    order: 1,
                    thread_id: Some("thr-root".into()),
                    title: "Split work".into(),
                    kind: NodeKind::Discuss,
                    status: NodeStatus::Done,
                    lifecycle: NodeLifecycle::Thread,
                    unblocks: 1,
                    ..Default::default()
                },
                GraphNode {
                    id: "draft-1".into(),
                    order: 2,
                    title: "Implement parser".into(),
                    kind: NodeKind::Build,
                    status: NodeStatus::Waiting,
                    lifecycle: NodeLifecycle::Draft,
                    waiting: WaitingCount { done: 1, total: 2 },
                    prompt: Some("Write the parser and tests.".into()),
                    ..Default::default()
                },
            ],
            edges: vec![GraphEdge {
                from: "root".into(),
                to: "draft-1".into(),
                kind: EdgeKind::DependsOn,
            }],
            layout: GraphLayout {
                placed: vec![
                    PlacedNode {
                        id: "root".into(),
                        layer: 0,
                        column: 0,
                    },
                    PlacedNode {
                        id: "draft-1".into(),
                        layer: 1,
                        column: 1,
                    },
                ],
                layers: 2,
                columns: 2,
            },
            graph_revision: 7,
            unlinked: 2,
            truncated: false,
        }
    }

    #[test]
    fn backoff_increases_when_unchanged_and_resets_on_change() {
        let mut backoff = PollBackoff::default();
        assert_eq!(backoff.on_unchanged(), Duration::from_secs(2));
        assert_eq!(backoff.on_unchanged(), Duration::from_secs(4));
        assert_eq!(backoff.on_unchanged(), Duration::from_secs(8));
        assert_eq!(backoff.on_unchanged(), Duration::from_secs(15));
        assert_eq!(backoff.on_unchanged(), Duration::from_secs(30));
        assert_eq!(backoff.on_unchanged(), Duration::from_secs(30));
        assert_eq!(backoff.on_changed(), Duration::from_secs(2));
    }

    #[test]
    fn card_state_maps_all_states_and_live_thread_failures() {
        let blocked = GraphNode {
            status: NodeStatus::Blocked,
            blocked_by: Some(BlockedBy {
                id: "dep".into(),
                title: "Upstream task".into(),
            }),
            ..Default::default()
        };
        assert_eq!(
            card_state(&blocked, None),
            CardState::Blocked {
                reason: "Waiting on Upstream task".into()
            }
        );

        let needs_you = GraphNode {
            status: NodeStatus::NeedsYou,
            ..Default::default()
        };
        let failed = ThreadSummary {
            status: ThreadStatus::Error,
            ..Default::default()
        };
        assert_eq!(card_state(&needs_you, Some(&failed)), CardState::Failed);

        let approval = ThreadSummary {
            status: ThreadStatus::AwaitingApproval,
            pending_approval: Some(true),
            ..Default::default()
        };
        assert_eq!(
            card_state(&needs_you, Some(&approval)),
            CardState::NeedsApproval
        );

        let unsupported = GraphNode {
            status: NodeStatus::Unsupported,
            ..Default::default()
        };
        assert_eq!(card_state(&unsupported, None), CardState::Unsupported);
    }

    #[test]
    fn geometry_places_nodes_at_host_layer_and_column_and_connects_edges() {
        let geometry = GraphGeometry::from_view(&sample_view());
        assert_eq!(geometry.cards.len(), 2);
        assert_eq!(geometry.cards[0].x, CANVAS_PADDING);
        assert_eq!(geometry.cards[0].y, CANVAS_PADDING);
        assert_eq!(
            geometry.cards[1].x,
            CANVAS_PADDING + CARD_WIDTH + COLUMN_GAP
        );
        assert_eq!(
            geometry.cards[1].y,
            CANVAS_PADDING + CARD_HEIGHT + LAYER_GAP
        );
        assert_eq!(geometry.edges.len(), 1);
        assert_eq!(geometry.edges[0].kind, EdgeKind::DependsOn);
        assert_eq!(
            geometry.edges[0].from_x,
            geometry.cards[0].x + CARD_WIDTH * 0.5
        );
        assert_eq!(geometry.edges[0].from_y, geometry.cards[0].y + CARD_HEIGHT);
        assert_eq!(
            geometry.edges[0].to_x,
            geometry.cards[1].x + CARD_WIDTH * 0.5
        );
        assert_eq!(geometry.edges[0].to_y, geometry.cards[1].y);
    }

    #[test]
    fn state_guards_stale_polls_and_coalesces_thread_watch_refresh() {
        let mut state = GraphViewState::default();
        assert!(state.target(
            1,
            test_host(),
            PathBuf::from("/repo"),
            Some("draft-1".into())
        ));
        let (epoch, generation, _, rev) = state.begin_poll(false).unwrap();
        assert_eq!(rev, None);
        // Thread watch fires while poll is in flight: coalesces into needs_refresh.
        assert!(!state.on_project_threads_changed());
        assert!(state.needs_refresh);

        // A stale reply from an older generation is ignored.
        assert_eq!(
            state.apply_poll(
                epoch,
                generation + 1,
                Ok((GraphReply::Unchanged { graph_revision: 1 }, vec![])),
            ),
            PollOutcome::Ignored
        );

        let outcome = state.apply_poll(
            epoch,
            generation,
            Ok((GraphReply::View(sample_view()), vec![])),
        );
        assert_eq!(
            outcome,
            PollOutcome::Applied {
                changed: true,
                refresh_now: true,
                next_delay: Duration::from_secs(2),
            }
        );
        assert_eq!(state.revision, Some(7));
        assert_eq!(state.selected.as_deref(), Some("draft-1"));

        // Parking clears resident data and bumps generation so late replies drop.
        state.park();
        assert!(state.snapshot.is_none());
        assert_eq!(
            state.apply_poll(
                epoch,
                generation,
                Ok((GraphReply::View(sample_view()), vec![]))
            ),
            PollOutcome::Ignored
        );
    }

    #[test]
    fn parked_actions_guard_the_same_host_and_reject_pre_action_polls() {
        let mut state = GraphViewState::default();
        let project = PathBuf::from("/repo");
        state.target(1, test_host(), project.clone(), None);
        let host = state.begin_mutation("draft-1", false).unwrap();
        state.park();
        let other = HostKey {
            id: "other".into(),
            ..test_host()
        };
        state.target(2, other, project.clone(), None);
        assert!(!state.is_pending("draft-1", false));
        state.target(3, test_host(), project, None);
        assert!(state.begin_mutation("draft-1", false).is_none());
        let (epoch, old, _, _) = state.begin_poll(false).unwrap();
        assert!(state.finish_mutation(
            host,
            "draft-1".into(),
            Ok(MutationReply {
                node_id: "draft-1".into(),
                graph_revision: 8
            }),
            false
        ));
        assert_eq!(
            state.apply_poll(epoch, old, Ok((GraphReply::View(sample_view()), vec![]))),
            PollOutcome::Ignored
        );
        let (_, current, _, revision) = state.begin_poll(false).unwrap();
        assert_eq!(revision, None);
        state.apply_poll(
            epoch,
            current,
            Ok((GraphReply::View(sample_view()), vec![])),
        );
        assert!(state.snapshot.is_none());
        assert!(state.is_pending("draft-1", false));
    }

    #[test]
    fn delete_mutation_clears_selected_draft() {
        let mut state = GraphViewState::default();
        state.target(1, test_host(), PathBuf::from("/repo"), None);
        state.install_scene(PathBuf::from("/repo"), sample_view());
        state.select_node(Some("draft-1".into()));
        let host = state.begin_mutation("draft-1", false).unwrap();
        assert!(state.finish_mutation(
            host,
            "draft-1".into(),
            Ok(MutationReply {
                node_id: "draft-1".into(),
                graph_revision: 8,
            }),
            true,
        ));
        assert_eq!(state.selected, None);
    }
}
