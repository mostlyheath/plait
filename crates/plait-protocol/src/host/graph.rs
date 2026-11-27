//! Wire types for the project thread graph operations (`runtime/src/graph/`).
//!
//! Unknown enum values decode to `Unsupported` so a newer host adding a node
//! kind, status, lifecycle, edge kind, or seed mode never hides the graph.

use super::{ApprovalMode, ThinkingLevel, harness::HarnessId};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::path::PathBuf;

/// Whether a graph node reads/plans (`Discuss`) or changes files (`Build`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    #[default]
    Build,
    Discuss,
    #[serde(other, skip_serializing)]
    Unsupported,
}

impl NodeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Discuss => "Discuss",
            Self::Unsupported => "Unsupported",
        }
    }
}

/// Host-reported status of a card in the project graph (`runtime/src/graph/view.ts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    #[default]
    Draft,
    Waiting,
    Blocked,
    Ready,
    Discussing,
    Building,
    NeedsYou,
    Done,
    #[serde(other, skip_serializing)]
    Unsupported,
}

/// Whether a node is still an unstarted draft or an admitted host thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLifecycle {
    #[default]
    Draft,
    Thread,
    #[serde(other, skip_serializing)]
    Unsupported,
}

/// Relationship between two nodes (`runtime/src/graph/layout.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Fork,
    DependsOn,
    #[serde(other, skip_serializing)]
    Unsupported,
}

/// What a forked draft starts with (`runtime/src/graph/host.ts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedMode {
    Excerpt,
    #[default]
    Both,
    Brief,
    #[serde(other, skip_serializing)]
    Unsupported,
}

/// Dependency progress for a waiting draft (`done` of `total`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingCount {
    pub done: u32,
    pub total: u32,
}

/// A failed or unfinished dependency blocking a draft.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedBy {
    pub id: String,
    pub title: String,
}

/// One node (started thread or unstarted draft) in a `graph` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    #[serde(default)]
    pub order: u64,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub kind: NodeKind,
    #[serde(default)]
    pub status: NodeStatus,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub harness: Option<HarnessId>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub lifecycle: NodeLifecycle,
    #[serde(default)]
    pub start_when_ready: bool,
    #[serde(default)]
    pub waiting: WaitingCount,
    #[serde(default)]
    pub blocked_by: Option<BlockedBy>,
    #[serde(default)]
    pub unblocks: u64,
    #[serde(default)]
    pub finished_children: u64,
    /// Draft prompt text when supplied by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// One directed link between nodes. `to` is drawn below `from`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// Layer and column placement computed by `runtime/src/graph/layout.ts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedNode {
    pub id: String,
    pub layer: u32,
    pub column: u32,
}

fn decode_records<T: de::DeserializeOwned, E: de::Error>(
    records: Vec<serde_json::Value>,
    limit: usize,
) -> Result<Vec<T>, E> {
    if records.len() > limit {
        return Err(E::custom("too many graph records"));
    }
    Ok(records
        .into_iter()
        .filter_map(|record| serde_json::from_value(record).ok())
        .collect())
}

fn placements<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PlacedNode>, D::Error> {
    decode_records(Vec::deserialize(d)?, 500)
}

/// Layered grid placement for the visible graph nodes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphLayout {
    #[serde(deserialize_with = "placements")]
    pub placed: Vec<PlacedNode>,
    pub layers: u32,
    pub columns: u32,
}

/// Full snapshot returned by `graph` when the revision differs or was omitted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphView {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub layout: GraphLayout,
    pub graph_revision: u64,
    pub unlinked: u64,
    pub truncated: bool,
}

/// Reply to the `graph` operation: either unchanged at `graph_revision`, or a full view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphReply {
    Unchanged { graph_revision: u64 },
    View(GraphView),
}

impl GraphReply {
    pub fn graph_revision(&self) -> u64 {
        match self {
            Self::Unchanged { graph_revision } => *graph_revision,
            Self::View(view) => view.graph_revision,
        }
    }
}

impl<'de> Deserialize<'de> for GraphReply {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct RawGraphReply {
            graph_revision: u64,
            #[serde(default)]
            unchanged: Option<bool>,
            nodes: Option<Vec<serde_json::Value>>,
            edges: Option<Vec<serde_json::Value>>,
            layout: Option<GraphLayout>,
            unlinked: Option<u64>,
            truncated: Option<bool>,
        }

        let raw = RawGraphReply::deserialize(deserializer)?;
        if raw.unchanged == Some(true) {
            return Ok(Self::Unchanged {
                graph_revision: raw.graph_revision,
            });
        }
        let nodes = raw.nodes.ok_or_else(|| de::Error::missing_field("nodes"))?;
        let edges = raw.edges.ok_or_else(|| de::Error::missing_field("edges"))?;
        let layout = raw
            .layout
            .ok_or_else(|| de::Error::missing_field("layout"))?;
        let unlinked = raw
            .unlinked
            .ok_or_else(|| de::Error::missing_field("unlinked"))?;
        let truncated = raw
            .truncated
            .ok_or_else(|| de::Error::missing_field("truncated"))?;
        Ok(Self::View(GraphView {
            nodes: decode_records(nodes, 500)?,
            edges: decode_records(edges, 2000)?,
            layout,
            graph_revision: raw.graph_revision,
            unlinked,
            truncated,
        }))
    }
}

impl Serialize for GraphReply {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct UnchangedWire {
            graph_revision: u64,
            unchanged: bool,
        }
        match self {
            Self::Unchanged { graph_revision } => UnchangedWire {
                graph_revision: *graph_revision,
                unchanged: true,
            }
            .serialize(serializer),
            Self::View(view) => view.serialize(serializer),
        }
    }
}

/// Reply returned by graph mutations (`create_draft`, `fork_thread`, `start_draft`,
/// `delete_draft`, `set_dependencies`, `set_node_kind`, `set_start_when_ready`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationReply {
    pub node_id: String,
    pub graph_revision: u64,
}

/// Reply returned by `send_results`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendResultsReply {
    pub text: String,
}

/// Wire requests accepted by `runtime/src/graph/host.ts` and `requests.ts`.
///
/// The host rejects any field outside `requests.ts` (`op`, `op_id`, and each
/// op's listed fields), so these requests never include `protocol_version`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Graph {
        project: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        revision: Option<u64>,
    },
    CreateDraft {
        op_id: String,
        checkout: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        project: Option<PathBuf>,
        title: String,
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<NodeKind>,
        #[serde(skip_serializing_if = "Option::is_none")]
        start_when_ready: Option<bool>,
        harness: HarnessId,
        provider: String,
        model_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<ThinkingLevel>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval: Option<ApprovalMode>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        depends_on: Vec<String>,
    },
    ForkThread {
        op_id: String,
        parent_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        at_message_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        brief: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        seed_mode: Option<SeedMode>,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<NodeKind>,
        #[serde(skip_serializing_if = "Option::is_none")]
        start_when_ready: Option<bool>,
        harness: HarnessId,
        provider: String,
        model_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<ThinkingLevel>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval: Option<ApprovalMode>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        depends_on: Vec<String>,
    },
    StartDraft {
        node_id: String,
    },
    DeleteDraft {
        node_id: String,
    },
    SetDependencies {
        node_id: String,
        depends_on: Vec<String>,
    },
    SetNodeKind {
        node_id: String,
        kind: NodeKind,
    },
    SetStartWhenReady {
        node_id: String,
        start_when_ready: bool,
    },
    SendResults {
        parent_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn graph_requests_match_host_field_lists() {
        assert_eq!(
            serde_json::to_value(Request::Graph {
                project: PathBuf::from("/repo"),
                revision: None,
            })
            .unwrap(),
            json!({"op": "graph", "project": "/repo"})
        );
        assert_eq!(
            serde_json::to_value(Request::Graph {
                project: PathBuf::from("/repo"),
                revision: Some(4),
            })
            .unwrap(),
            json!({"op": "graph", "project": "/repo", "revision": 4})
        );
        assert_eq!(
            serde_json::to_value(Request::CreateDraft {
                op_id: "op-1".into(),
                checkout: PathBuf::from("/repo"),
                project: None,
                title: "Build parser".into(),
                prompt: "Implement parser".into(),
                kind: Some(NodeKind::Build),
                start_when_ready: Some(true),
                harness: HarnessId::Pi,
                provider: "openai".into(),
                model_id: "gpt-5".into(),
                mode: None,
                thinking: Some(ThinkingLevel::Medium),
                approval: Some(ApprovalMode::Ask),
                depends_on: vec!["n0".into()],
            })
            .unwrap(),
            json!({
                "op": "create_draft",
                "op_id": "op-1",
                "checkout": "/repo",
                "title": "Build parser",
                "prompt": "Implement parser",
                "kind": "build",
                "start_when_ready": true,
                "harness": "pi",
                "provider": "openai",
                "model_id": "gpt-5",
                "thinking": "medium",
                "approval": "ask",
                "depends_on": ["n0"]
            })
        );
        assert_eq!(
            serde_json::to_value(Request::ForkThread {
                op_id: "op-2".into(),
                parent_id: "thr-1".into(),
                at_message_id: Some("msg-3".into()),
                brief: Some("Check edge cases".into()),
                seed_mode: Some(SeedMode::Both),
                title: "Edge cases".into(),
                prompt: None,
                kind: Some(NodeKind::Discuss),
                start_when_ready: Some(false),
                harness: HarnessId::Claude,
                provider: "claude".into(),
                model_id: "default".into(),
                mode: Some("ask".into()),
                thinking: None,
                approval: None,
                depends_on: Vec::new(),
            })
            .unwrap(),
            json!({
                "op": "fork_thread",
                "op_id": "op-2",
                "parent_id": "thr-1",
                "at_message_id": "msg-3",
                "brief": "Check edge cases",
                "seed_mode": "both",
                "title": "Edge cases",
                "kind": "discuss",
                "start_when_ready": false,
                "harness": "claude",
                "provider": "claude",
                "model_id": "default",
                "mode": "ask"
            })
        );
        assert_eq!(
            serde_json::to_value(Request::StartDraft {
                node_id: "n1".into(),
            })
            .unwrap(),
            json!({"op": "start_draft", "node_id": "n1"})
        );
        assert_eq!(
            serde_json::to_value(Request::DeleteDraft {
                node_id: "n1".into(),
            })
            .unwrap(),
            json!({"op": "delete_draft", "node_id": "n1"})
        );
        assert_eq!(
            serde_json::to_value(Request::SetDependencies {
                node_id: "n2".into(),
                depends_on: Vec::new(),
            })
            .unwrap(),
            json!({"op": "set_dependencies", "node_id": "n2", "depends_on": []})
        );
        assert_eq!(
            serde_json::to_value(Request::SetNodeKind {
                node_id: "n2".into(),
                kind: NodeKind::Discuss,
            })
            .unwrap(),
            json!({"op": "set_node_kind", "node_id": "n2", "kind": "discuss"})
        );
        assert_eq!(
            serde_json::to_value(Request::SetStartWhenReady {
                node_id: "n2".into(),
                start_when_ready: false,
            })
            .unwrap(),
            json!({"op": "set_start_when_ready", "node_id": "n2", "start_when_ready": false})
        );
        assert_eq!(
            serde_json::to_value(Request::SendResults {
                parent_id: "thr-1".into(),
            })
            .unwrap(),
            json!({"op": "send_results", "parent_id": "thr-1"})
        );
    }

    #[test]
    fn graph_reply_decodes_full_unchanged_and_unknown_enum_variants() {
        let unchanged: GraphReply =
            serde_json::from_value(json!({"graph_revision": 9, "unchanged": true})).unwrap();
        assert_eq!(unchanged, GraphReply::Unchanged { graph_revision: 9 });
        assert_eq!(
            serde_json::to_value(&unchanged).unwrap(),
            json!({"graph_revision": 9, "unchanged": true})
        );

        assert!(
            serde_json::from_value::<GraphReply>(json!({"graph_revision": 9, "unchanged": false}))
                .is_err()
        );

        let view: GraphReply = serde_json::from_value(json!({
            "nodes": [null, {"id":42},
                {
                    "id": "n1",
                    "order": 1,
                    "thread_id": "thr-1",
                    "title": "Plan architecture",
                    "kind": "discuss",
                    "status": "done",
                    "reason": null,
                    "archived": false,
                    "harness": "pi",
                    "model_id": "gpt-5",
                    "lifecycle": "thread",
                    "start_when_ready": false,
                    "waiting": {"done": 0, "total": 0},
                    "blocked_by": null,
                    "unblocks": 1,
                    "finished_children": 0
                },
                {
                    "id": "n2",
                    "order": 2,
                    "thread_id": null,
                    "title": "Future review node",
                    "kind": "review_v2",
                    "status": "scheduled_future",
                    "reason": "Needs review",
                    "archived": false,
                    "lifecycle": "agent_future",
                    "start_when_ready": true,
                    "waiting": {"done": 1, "total": 2},
                    "blocked_by": {"id": "n1", "title": "Plan architecture"},
                    "unblocks": 0,
                    "finished_children": 0,
                    "prompt": "Check invariants",
                    "future_extra_field": 42
                }
            ],
            "edges": [42,
                {"from": "n1", "to": "n2", "kind": "depends_on"},
                {"from": "n1", "to": "n2", "kind": "future_edge"}
            ],
            "layout": {
                "placed": [false,
                    {"id": "n1", "layer": 0, "column": 0},
                    {"id": "n2", "layer": 1, "column": 0}
                ],
                "layers": 2,
                "columns": 1
            },
            "graph_revision": 12,
            "unlinked": 3,
            "truncated": false,
            "future_top_level": true
        }))
        .unwrap();

        let GraphReply::View(view) = view else {
            panic!("expected full graph view");
        };
        assert_eq!(view.graph_revision, 12);
        assert_eq!(view.unlinked, 3);
        assert!(!view.truncated);
        assert_eq!(view.nodes[0].kind, NodeKind::Discuss);
        assert_eq!(view.nodes[0].status, NodeStatus::Done);
        assert_eq!(view.nodes[0].lifecycle, NodeLifecycle::Thread);
        assert_eq!(view.nodes[0].prompt, None);
        assert_eq!(view.nodes[1].kind, NodeKind::Unsupported);
        assert_eq!(view.nodes[1].status, NodeStatus::Unsupported);
        assert_eq!(view.nodes[1].lifecycle, NodeLifecycle::Unsupported);
        assert_eq!(view.nodes[1].prompt.as_deref(), Some("Check invariants"));
        assert_eq!(view.edges[0].kind, EdgeKind::DependsOn);
        assert_eq!(view.edges[1].kind, EdgeKind::Unsupported);
        assert_eq!(
            serde_json::from_value::<SeedMode>(json!("future_seed")).unwrap(),
            SeedMode::Unsupported
        );
    }

    #[test]
    fn malformed_entries_still_count_toward_envelope_limits() {
        let view = json!({"nodes":[],"edges":[],"layout":{"placed":[],"layers":0,"columns":0},"unlinked":0,"graph_revision":0,"truncated":false});
        for (path, limit) in [("/nodes", 500), ("/edges", 2000), ("/layout/placed", 500)] {
            let mut oversized = view.clone();
            *oversized.pointer_mut(path).unwrap() = json!(vec![serde_json::Value::Null; limit + 1]);
            assert!(serde_json::from_value::<GraphReply>(oversized).is_err());
        }
    }

    #[test]
    fn mutation_and_send_results_replies_decode() {
        let mutation: MutationReply =
            serde_json::from_value(json!({"node_id": "n-1", "graph_revision": 5})).unwrap();
        assert_eq!(mutation.node_id, "n-1");
        assert_eq!(mutation.graph_revision, 5);

        let results: SendResultsReply =
            serde_json::from_value(json!({"text": "[Thread](plait://thread/t1)\nDone"})).unwrap();
        assert!(results.text.contains("plait://thread/t1"));
    }
}
