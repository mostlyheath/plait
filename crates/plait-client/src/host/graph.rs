//! Additive thread-graph host operations (`runtime/src/graph/`).
//!
//! Blocking I/O belongs on the background executor. Hosts without `PLAIT_GRAPH=1`
//! (or older hosts) reject graph requests with `invalid_request`; `probe` maps
//! that rejection to `Ok(None)` so callers hide the Graph tab cleanly.

use super::{ApprovalMode, Client, Error, ThinkingLevel, harness::HarnessId};
use std::path::PathBuf;

pub use plait_protocol::host::graph::*;

const MAX_SAFE_INT: u64 = 9_007_199_254_740_991;
const MAX_NODES: usize = 500;
const MAX_EDGES: usize = 2_000;
const MAX_PROMPT_BYTES: usize = 65_536;

fn invalid() -> Error {
    Error::Protocol("Invalid thread graph reply.".into())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
}

fn valid_node(node: &GraphNode) -> bool {
    valid_id(&node.id)
        && node.thread_id.as_deref().is_none_or(valid_id)
        && node.title.len() <= 1024
        && node.reason.as_ref().is_none_or(|r| r.len() <= 4096)
        && node.model_id.as_ref().is_none_or(|m| m.len() <= 256)
        && node
            .blocked_by
            .as_ref()
            .is_none_or(|b| valid_id(&b.id) && b.title.len() <= 1024)
        && node
            .prompt
            .as_ref()
            .is_none_or(|p| p.len() <= MAX_PROMPT_BYTES)
        && node.unblocks <= MAX_SAFE_INT
        && node.finished_children <= MAX_SAFE_INT
}

fn verify_view(mut view: GraphView) -> Result<GraphView, Error> {
    let ok = view.graph_revision <= MAX_SAFE_INT
        && view.unlinked <= MAX_SAFE_INT
        && view.nodes.len() <= MAX_NODES
        && view.edges.len() <= MAX_EDGES
        && view.layout.placed.len() <= MAX_NODES;
    if !ok {
        return Err(invalid());
    }
    view.nodes.retain(valid_node);
    view.edges
        .retain(|edge| valid_id(&edge.from) && valid_id(&edge.to));
    let layout = &mut view.layout;
    layout.layers = layout.layers.min(MAX_NODES as u32);
    layout.columns = layout.columns.min(MAX_NODES as u32);
    layout
        .placed
        .retain(|p| valid_id(&p.id) && p.layer < layout.layers && p.column < layout.columns);
    Ok(view)
}

fn verify_reply(reply: GraphReply) -> Result<GraphReply, Error> {
    match reply {
        GraphReply::Unchanged { graph_revision } => {
            if graph_revision <= MAX_SAFE_INT {
                Ok(GraphReply::Unchanged { graph_revision })
            } else {
                Err(invalid())
            }
        }
        GraphReply::View(view) => verify_view(view).map(GraphReply::View),
    }
}

fn verify_mutation(reply: MutationReply) -> Result<MutationReply, Error> {
    let ok = valid_id(&reply.node_id) && reply.graph_revision <= MAX_SAFE_INT;
    if ok { Ok(reply) } else { Err(invalid()) }
}

fn send<T: serde::de::DeserializeOwned>(client: &Client, request: &Request) -> Result<T, Error> {
    client.send(request).map_err(|error| {
        if matches!(error, Error::Protocol(_)) {
            invalid()
        } else {
            error
        }
    })
}

/// Parameters for creating a standalone draft node in the thread graph.
#[derive(Clone, Debug)]
pub struct CreateDraftParams {
    pub op_id: String,
    pub checkout: PathBuf,
    pub project: Option<PathBuf>,
    pub title: String,
    pub prompt: String,
    pub kind: Option<NodeKind>,
    pub start_when_ready: Option<bool>,
    pub harness: HarnessId,
    pub provider: String,
    pub model_id: String,
    pub mode: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub approval: Option<ApprovalMode>,
    pub depends_on: Vec<String>,
}

/// Parameters for forking a started thread into a child draft node.
#[derive(Clone, Debug)]
pub struct ForkThreadParams {
    pub op_id: String,
    pub parent_id: String,
    pub at_message_id: Option<String>,
    pub brief: Option<String>,
    pub seed_mode: Option<SeedMode>,
    pub title: String,
    pub prompt: Option<String>,
    pub kind: Option<NodeKind>,
    pub start_when_ready: Option<bool>,
    pub harness: HarnessId,
    pub provider: String,
    pub model_id: String,
    pub mode: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub approval: Option<ApprovalMode>,
    pub depends_on: Vec<String>,
}

/// Probes whether the host supports graph operations for `project`.
///
/// Returns `Ok(None)` when the host rejects `graph` with `invalid_request`
/// (older host or `PLAIT_GRAPH` unset). Transient transport or busy errors stay `Err`.
pub fn probe(client: &Client, project: PathBuf) -> Result<Option<GraphView>, Error> {
    if !project.is_absolute() {
        return Err(Error::Protocol("Expected an absolute project path.".into()));
    }
    match send::<GraphReply>(
        client,
        &Request::Graph {
            project,
            revision: None,
        },
    ) {
        Ok(GraphReply::View(view)) => verify_view(view).map(Some),
        Ok(GraphReply::Unchanged { .. }) => Err(invalid()),
        Err(Error::Rejected(error)) if error.code == "invalid_request" => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reads the project's thread graph, passing `revision` to receive `Unchanged` when nothing moved.
pub fn graph(
    client: &Client,
    project: PathBuf,
    revision: Option<u64>,
) -> Result<GraphReply, Error> {
    if !project.is_absolute() {
        return Err(Error::Protocol("Expected an absolute project path.".into()));
    }
    verify_reply(send(client, &Request::Graph { project, revision })?)
}

/// Creates a new unstarted draft node in the graph.
pub fn create_draft(client: &Client, params: CreateDraftParams) -> Result<MutationReply, Error> {
    if params.kind == Some(NodeKind::Unsupported) || params.harness == HarnessId::Other {
        return Err(Error::Protocol(
            "Unsupported graph draft kind or harness.".into(),
        ));
    }
    let reply = send(
        client,
        &Request::CreateDraft {
            op_id: params.op_id,
            checkout: params.checkout,
            project: params.project,
            title: params.title,
            prompt: params.prompt,
            kind: params.kind,
            start_when_ready: params.start_when_ready,
            harness: params.harness,
            provider: params.provider,
            model_id: params.model_id,
            mode: params.mode,
            thinking: params.thinking,
            approval: params.approval,
            depends_on: params.depends_on,
        },
    )?;
    verify_mutation(reply)
}

/// Forks a started thread into a new child draft node.
pub fn fork_thread(client: &Client, params: ForkThreadParams) -> Result<MutationReply, Error> {
    if params.kind == Some(NodeKind::Unsupported)
        || params.seed_mode == Some(SeedMode::Unsupported)
        || params.harness == HarnessId::Other
    {
        return Err(Error::Protocol(
            "Unsupported graph fork kind, seed mode, or harness.".into(),
        ));
    }
    let reply = send(
        client,
        &Request::ForkThread {
            op_id: params.op_id,
            parent_id: params.parent_id,
            at_message_id: params.at_message_id,
            brief: params.brief,
            seed_mode: params.seed_mode,
            title: params.title,
            prompt: params.prompt,
            kind: params.kind,
            start_when_ready: params.start_when_ready,
            harness: params.harness,
            provider: params.provider,
            model_id: params.model_id,
            mode: params.mode,
            thinking: params.thinking,
            approval: params.approval,
            depends_on: params.depends_on,
        },
    )?;
    verify_mutation(reply)
}

/// A complete frame may have reached the host even if its reply was lost.
pub struct DraftFailure {
    pub error: Error,
    pub transmitted: bool,
}

pub fn mutate_draft(
    client: &Client,
    node_id: String,
    delete: bool,
) -> Result<MutationReply, DraftFailure> {
    let request = if delete {
        Request::DeleteDraft { node_id }
    } else {
        Request::StartDraft { node_id }
    };
    let mut transmitted = false;
    client
        .send_tracking_transmission(
            &request,
            std::time::Duration::from_secs(20),
            &mut transmitted,
        )
        .and_then(verify_mutation)
        .map_err(|error| DraftFailure { error, transmitted })
}

/// Requests immediate start of an unstarted draft node.
pub fn start_draft(client: &Client, node_id: String) -> Result<MutationReply, Error> {
    mutate_draft(client, node_id, false).map_err(|failure| failure.error)
}

/// Deletes an unstarted draft node and its edges.
pub fn delete_draft(client: &Client, node_id: String) -> Result<MutationReply, Error> {
    mutate_draft(client, node_id, true).map_err(|failure| failure.error)
}

/// Replaces the complete dependency list (`depends_on`) of an unstarted draft node.
pub fn set_dependencies(
    client: &Client,
    node_id: String,
    depends_on: Vec<String>,
) -> Result<MutationReply, Error> {
    let reply = send(
        client,
        &Request::SetDependencies {
            node_id,
            depends_on,
        },
    )?;
    verify_mutation(reply)
}

/// Adds one dependency to `current_depends_on` without duplicating an existing link.
pub fn with_dependency(current_depends_on: &[String], dependency_id: String) -> Vec<String> {
    let mut next = current_depends_on.to_vec();
    if !next.iter().any(|existing| existing == &dependency_id) {
        next.push(dependency_id);
    }
    next
}

/// Removes one dependency from `current_depends_on`.
pub fn without_dependency(current_depends_on: &[String], dependency_id: &str) -> Vec<String> {
    current_depends_on
        .iter()
        .filter(|existing| existing.as_str() != dependency_id)
        .cloned()
        .collect()
}

/// Links `node_id` so it waits for `dependency_id` via `set_dependencies`.
pub fn link(
    client: &Client,
    node_id: String,
    current_depends_on: &[String],
    dependency_id: String,
) -> Result<MutationReply, Error> {
    let depends_on = with_dependency(current_depends_on, dependency_id);
    set_dependencies(client, node_id, depends_on)
}

/// Unlinks `dependency_id` from `node_id` via `set_dependencies`.
pub fn unlink(
    client: &Client,
    node_id: String,
    current_depends_on: &[String],
    dependency_id: &str,
) -> Result<MutationReply, Error> {
    let depends_on = without_dependency(current_depends_on, dependency_id);
    set_dependencies(client, node_id, depends_on)
}

/// Changes a node's kind (`Build` or `Discuss`) when allowed by the host.
pub fn set_node_kind(
    client: &Client,
    node_id: String,
    kind: NodeKind,
) -> Result<MutationReply, Error> {
    if kind == NodeKind::Unsupported {
        return Err(Error::Protocol(
            "Cannot set an unsupported node kind.".into(),
        ));
    }
    let reply = send(client, &Request::SetNodeKind { node_id, kind })?;
    verify_mutation(reply)
}

/// Enables or disables automatic start once a draft's dependencies finish.
pub fn set_start_when_ready(
    client: &Client,
    node_id: String,
    start_when_ready: bool,
) -> Result<MutationReply, Error> {
    let reply = send(
        client,
        &Request::SetStartWhenReady {
            node_id,
            start_when_ready,
        },
    )?;
    verify_mutation(reply)
}

/// Collects bounded final answers from finished child threads of `parent_id`.
pub fn send_results(client: &Client, parent_id: String) -> Result<SendResultsReply, Error> {
    let reply: SendResultsReply = send(client, &Request::SendResults { parent_id })?;
    if reply.text.len() > MAX_PROMPT_BYTES {
        return Err(invalid());
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dependency_helpers_deduplicate_and_remove() {
        let base = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(with_dependency(&base, "b".into()), vec!["a", "b"]);
        assert_eq!(with_dependency(&base, "c".into()), vec!["a", "b", "c"]);
        assert_eq!(without_dependency(&base, "a"), vec!["b"]);
        assert_eq!(without_dependency(&base, "missing"), vec!["a", "b"]);
    }

    #[test]
    fn verify_view_rejects_oversized_and_bad_ids() {
        let good: GraphView = serde_json::from_value(json!({
            "nodes": [{"id": "n1", "order": 1, "title": "Root"}],
            "edges": [],
            "layout": {
                "placed": [{"id": "n1", "layer": 0, "column": 0}],
                "layers": 1,
                "columns": 1
            },
            "graph_revision": 3,
            "unlinked": 0,
            "truncated": false
        }))
        .unwrap();
        let mut misplaced = good.clone();
        misplaced.layout.placed[0].layer = u32::MAX;
        let geometry =
            crate::graph_view::GraphGeometry::from_view(&verify_view(misplaced).unwrap());
        assert_eq!((geometry.cards[0].layer, geometry.cards[0].column), (0, 0));

        let mut bad_id = good.clone();
        bad_id.nodes[0].id = String::new();
        bad_id.layout.placed[0].column = u32::MAX;
        let filtered = verify_view(bad_id).unwrap();
        assert!(filtered.nodes.is_empty() && filtered.layout.placed.is_empty());

        let mut too_many = good;
        too_many.nodes = (0..501)
            .map(|index| GraphNode {
                id: format!("n{index}"),
                ..Default::default()
            })
            .collect();
        assert!(verify_view(too_many).is_err());
    }

    #[test]
    fn verify_mutation_checks_canonical_node_id() {
        let reply = MutationReply {
            node_id: "n1".into(),
            graph_revision: 4,
        };
        assert!(verify_mutation(reply.clone()).is_ok());
        assert!(
            verify_mutation(MutationReply {
                node_id: String::new(),
                ..reply
            })
            .is_err()
        );
    }

    #[cfg(feature = "test-host")]
    #[test]
    fn probe_and_client_calls_handle_older_hosts_and_mutations() {
        let older = Client::with_test_reads(|request| {
            assert_eq!(request["op"], "graph");
            json!({
                "ok": false,
                "error": {"code": "invalid_request", "message": "Unknown operation."}
            })
        });
        assert_eq!(probe(&older, PathBuf::from("/repo")).unwrap(), None);

        let supported = Client::with_test_reads(|request| match request["op"].as_str() {
            Some("graph") if request.get("revision") == Some(&json!(2)) => {
                json!({"ok": true, "result": {"graph_revision": 2, "unchanged": true}})
            }
            Some("graph") => json!({
                "ok": true,
                "result": {
                    "nodes": [{
                        "id": "n1",
                        "order": 1,
                        "title": "Draft",
                        "kind": "future_kind",
                        "status": "draft",
                        "lifecycle": "draft"
                    }],
                    "edges": [],
                    "layout": {
                        "placed": [{"id": "n1", "layer": 0, "column": 0}],
                        "layers": 1,
                        "columns": 1
                    },
                    "graph_revision": 2,
                    "unlinked": 1,
                    "truncated": false
                }
            }),
            Some("set_node_kind") => {
                json!({"ok":true,"result":{"node_id":"n1","graph_revision":3}})
            }
            Some("start_draft" | "delete_draft" | "set_dependencies" | "set_start_when_ready") => {
                json!({"ok": true, "result": {"node_id": request["node_id"], "graph_revision": 3}})
            }
            Some("create_draft" | "fork_thread") => {
                json!({"ok": true, "result": {"node_id": "created-1", "graph_revision": 4}})
            }
            Some("send_results") => {
                json!({"ok": true, "result": {"text": "Finished summary"}})
            }
            other => panic!("unexpected op {other:?}"),
        });

        let initial = probe(&supported, PathBuf::from("/repo")).unwrap().unwrap();
        assert_eq!(initial.graph_revision, 2);
        assert_eq!(initial.nodes[0].kind, NodeKind::Unsupported);

        assert_eq!(
            graph(&supported, PathBuf::from("/repo"), Some(2)).unwrap(),
            GraphReply::Unchanged { graph_revision: 2 }
        );
        assert_eq!(
            start_draft(&supported, "n1".into()).unwrap().graph_revision,
            3
        );
        assert_eq!(delete_draft(&supported, "n1".into()).unwrap().node_id, "n1");
        assert_eq!(
            link(&supported, "n1".into(), &["dep-a".into()], "dep-b".into())
                .unwrap()
                .node_id,
            "n1"
        );
        assert_eq!(
            unlink(
                &supported,
                "n1".into(),
                &["dep-a".into(), "dep-b".into()],
                "dep-a"
            )
            .unwrap()
            .node_id,
            "n1"
        );
        assert_eq!(
            set_node_kind(&supported, "thread-alias".into(), NodeKind::Discuss)
                .unwrap()
                .node_id,
            "n1"
        );
        assert!(set_node_kind(&supported, "n1".into(), NodeKind::Unsupported).is_err());
        assert_eq!(
            send_results(&supported, "thr-1".into()).unwrap().text,
            "Finished summary"
        );
    }
}
