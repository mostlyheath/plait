//! Thread graph tab scenes with injected sample state (never in shipped views).
use super::*;
use plait_client::{
    host::graph::{
        BlockedBy, EdgeKind, GraphEdge, GraphLayout, GraphNode, GraphView, NodeKind, NodeLifecycle,
        NodeStatus, PlacedNode, WaitingCount,
    },
    tabs::TabTarget,
};

fn sample_graph() -> GraphView {
    GraphView {
        nodes: vec![
            GraphNode {
                id: "node-root".into(),
                order: 1,
                thread_id: Some(THREAD.into()),
                title: "Plan retry backoff".into(),
                kind: NodeKind::Discuss,
                status: NodeStatus::Done,
                reason: None,
                archived: false,
                harness: Some(HarnessId::Pi),
                model_id: Some("gpt-5".into()),
                lifecycle: NodeLifecycle::Thread,
                start_when_ready: false,
                waiting: WaitingCount::default(),
                blocked_by: None,
                unblocks: 2,
                finished_children: 1,
                prompt: None,
            },
            GraphNode {
                id: "node-build".into(),
                order: 2,
                thread_id: Some("thr_graph_build".into()),
                title: "Add backoff to host client".into(),
                kind: NodeKind::Build,
                status: NodeStatus::Building,
                reason: None,
                archived: false,
                harness: Some(HarnessId::Codex),
                model_id: Some("o4-mini".into()),
                lifecycle: NodeLifecycle::Thread,
                start_when_ready: true,
                waiting: WaitingCount::default(),
                blocked_by: None,
                unblocks: 1,
                finished_children: 0,
                prompt: None,
            },
            GraphNode {
                id: "node-failed".into(),
                order: 3,
                thread_id: Some("thr_graph_failed".into()),
                title: "Verify 429 retry limits".into(),
                kind: NodeKind::Build,
                status: NodeStatus::NeedsYou,
                reason: None,
                archived: false,
                harness: Some(HarnessId::Claude),
                model_id: Some("claude-sonnet-4".into()),
                lifecycle: NodeLifecycle::Thread,
                start_when_ready: true,
                waiting: WaitingCount::default(),
                blocked_by: None,
                unblocks: 1,
                finished_children: 0,
                prompt: None,
            },
            GraphNode {
                id: "draft-ready".into(),
                order: 4,
                thread_id: None,
                title: "Wire retry metrics".into(),
                kind: NodeKind::Build,
                status: NodeStatus::Ready,
                reason: None,
                archived: false,
                harness: Some(HarnessId::Pi),
                model_id: Some("gpt-5".into()),
                lifecycle: NodeLifecycle::Draft,
                start_when_ready: false,
                waiting: WaitingCount { done: 1, total: 1 },
                blocked_by: None,
                unblocks: 0,
                finished_children: 0,
                prompt: Some(
                    "Add structured retry counters to the host client and test backoff clamping."
                        .into(),
                ),
            },
            GraphNode {
                id: "draft-blocked".into(),
                order: 5,
                thread_id: None,
                title: "Update host protocol docs".into(),
                kind: NodeKind::Build,
                status: NodeStatus::Blocked,
                reason: Some("Upstream thread failed: Verify 429 retry limits".into()),
                archived: false,
                harness: Some(HarnessId::Pi),
                model_id: Some("gpt-5".into()),
                lifecycle: NodeLifecycle::Draft,
                start_when_ready: true,
                waiting: WaitingCount { done: 0, total: 2 },
                blocked_by: Some(BlockedBy {
                    id: "node-failed".into(),
                    title: "Verify 429 retry limits".into(),
                }),
                unblocks: 0,
                finished_children: 0,
                prompt: Some(
                    "Document the retry bounds and queue recovery guarantees in docs/HOST.md."
                        .into(),
                ),
            },
            GraphNode {
                id: "draft-plain".into(),
                order: 6,
                thread_id: None,
                title: "Audit reconnect edge cases".into(),
                kind: NodeKind::Unsupported,
                status: NodeStatus::Draft,
                reason: None,
                archived: false,
                harness: Some(HarnessId::Pi),
                model_id: Some("gpt-5".into()),
                lifecycle: NodeLifecycle::Draft,
                start_when_ready: false,
                waiting: WaitingCount::default(),
                blocked_by: None,
                unblocks: 0,
                finished_children: 0,
                prompt: Some(
                    "Check what happens when a host reconnects while a draft is waiting to start."
                        .into(),
                ),
            },
        ],
        edges: vec![
            GraphEdge {
                from: "node-root".into(),
                to: "node-build".into(),
                kind: EdgeKind::Fork,
            },
            GraphEdge {
                from: "node-root".into(),
                to: "node-failed".into(),
                kind: EdgeKind::Fork,
            },
            GraphEdge {
                from: "node-root".into(),
                to: "draft-ready".into(),
                kind: EdgeKind::DependsOn,
            },
            GraphEdge {
                from: "node-build".into(),
                to: "draft-blocked".into(),
                kind: EdgeKind::DependsOn,
            },
            GraphEdge {
                from: "node-failed".into(),
                to: "draft-blocked".into(),
                kind: EdgeKind::DependsOn,
            },
            GraphEdge {
                from: "node-root".into(),
                to: "draft-plain".into(),
                kind: EdgeKind::DependsOn,
            },
        ],
        layout: GraphLayout {
            placed: vec![
                PlacedNode {
                    id: "node-root".into(),
                    layer: 0,
                    column: 0,
                },
                PlacedNode {
                    id: "node-build".into(),
                    layer: 1,
                    column: 0,
                },
                PlacedNode {
                    id: "node-failed".into(),
                    layer: 1,
                    column: 1,
                },
                PlacedNode {
                    id: "draft-ready".into(),
                    layer: 2,
                    column: 0,
                },
                PlacedNode {
                    id: "draft-blocked".into(),
                    layer: 2,
                    column: 1,
                },
                PlacedNode {
                    id: "draft-plain".into(),
                    layer: 2,
                    column: 2,
                },
            ],
            layers: 3,
            columns: 3,
        },
        graph_revision: 14,
        unlinked: 3,
        truncated: false,
    }
}

pub(super) fn install(h: &mut Harness, selected_draft: Option<&str>) {
    h.update(|p, _, cx| {
        p.park_workspace(cx);
        p.notice = None;
        p.settings_open = false;
        let root = p.host.creation.checkout.clone();
        let epoch = p.host.connection.epoch;
        p.graph.capability = Some((epoch, true));
        p.host
            .connection
            .threads_mut()
            .retain(|t| !t.id.starts_with("thr_graph_"));
        p.host.connection.threads_mut().extend([
            ThreadSummary {
                id: "thr_graph_build".into(),
                checkout: root.clone(),
                title: Some("Add backoff to host client".into()),
                status: ThreadStatus::Running,
                harness: Some(HarnessId::Codex),
                ..Default::default()
            },
            ThreadSummary {
                id: "thr_graph_failed".into(),
                checkout: root.clone(),
                title: Some("Verify 429 retry limits".into()),
                status: ThreadStatus::Error,
                harness: Some(HarnessId::Claude),
                ..Default::default()
            },
        ]);
        let tab = p
            .tabs
            .model
            .open(
                TabTarget::Graph {
                    checkout: p.tab_checkout(root.clone()),
                },
                Some(epoch),
            )
            .unwrap();
        p.tabs.model.activate(tab).unwrap();
        p.sync_graph_view(cx);
        p.graph.view.install_scene(root, sample_graph());
        p.graph.view.select_node(selected_draft.map(str::to_owned));
        p.refresh_tab_rows();
        cx.notify();
    });
}

pub(super) fn walk(repo: &Path, out: Option<&Path>, suffix: &str, width: f32, height: f32) {
    let mut h = Harness::with_host(repo, out, width, height, None);
    h.connect(repo, false);
    for light in [false, true] {
        h.update(|p, _, cx| {
            p.themes.forced = Some(light);
            p.apply_theme(cx);
        });
        let theme = if light { "light" } else { "dark" };
        install(&mut h, None);
        h.capture(&format!("graph-{theme}-{suffix}"));
        install(&mut h, Some("draft-ready"));
        h.capture(&format!("graph-draft-selected-{theme}-{suffix}"));
    }
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn graph_hides_without_capability_and_draws_cards_and_draft_detail() {
    if !isolated("graph::graph_hides_without_capability_and_draws_cards_and_draft_detail") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-graph-scene-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    let mut h = Harness::with_host(&repo, output.as_deref(), 1380., 860., None);
    h.connect(&repo, false);

    // Without a confirmed graph probe, older hosts hide Graph from the project menu.
    h.update(|p, _, cx| {
        assert!(!p.graph_supported());
        p.navigation.menu = Some(format!("project:{}", repo.display()));
        cx.notify();
    });
    let older_frame = h.draw_a11y();
    older_frame.assert_no_duplicates("graph-hidden-older-host");
    assert!(
        !older_frame
            .nodes
            .iter()
            .any(|node| node.path.contains("project-graph")),
        "older host must hide Graph from the project menu"
    );

    // With capability confirmed, Graph appears and draws cards + draft detail cleanly.
    install(&mut h, Some("draft-ready"));
    let frame = h.draw_a11y();
    frame.assert_no_duplicates("graph-draft-selected");
    assert!(
        frame
            .nodes
            .iter()
            .any(|node| node.path.contains("graph-node-node-root"))
    );
    assert!(
        frame
            .nodes
            .iter()
            .any(|node| node.path.contains("graph-draft-start"))
    );
    assert!(
        frame
            .nodes
            .iter()
            .any(|node| node.path.contains("graph-draft-delete"))
    );

    h.capture("graph-selected");
    h.update(|p, window, cx| {
        use crate::ui::{tab_lifecycle::Intent, tabs::TabNavigation};
        let active = p.tabs.model.active().unwrap();
        let mut target = p.tabs.model.get(active).unwrap().session().target().clone();
        let TabTarget::Graph { checkout } = &mut target else {
            panic!("expected Graph")
        };
        let identity = plait_client::tabs::DirectoryIdentity {
            device: 1,
            inode: 2,
        };
        p.tabs
            .identity
            .checkouts
            .insert(checkout.root.clone(), identity);
        checkout.directory = Some(identity);
        p.tabs.model.close(active, false).unwrap();
        let tab = p
            .tabs
            .model
            .open(target, Some(p.host.connection.epoch))
            .unwrap();
        p.tabs.model.close(tab, false).unwrap();
        p.graph.capability = Some((p.host.connection.epoch, false));
        p.request_tab(Intent::Navigate(TabNavigation::Reopen), window, cx);
    });
    h.update(|p, _, _| {
        assert!(
            !p.tabs
                .model
                .tabs()
                .iter()
                .any(|t| t.session().target().is_graph())
        )
    });

    drop(h);
    std::fs::remove_dir_all(&scratch).unwrap();
}
