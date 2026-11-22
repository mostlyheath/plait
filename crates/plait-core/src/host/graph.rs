//! Wire types for the host-owned thread graph. Graph operations are versionless.
use super::{Client, Error};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    NeedsYou,
    Failed,
    Discussing,
    Building,
    Queued,
    Waiting,
    Draft,
    Done,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Waiting {
    pub done: u32,
    pub total: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Node {
    pub id: String,
    pub project: PathBuf,
    pub checkout: PathBuf,
    pub lifecycle: String,
    pub kind: String,
    pub pinned_root: bool,
    pub title: String,
    pub status: Status,
    pub waiting: Option<Waiting>,
    pub queue_position: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Edge {
    pub id: String,
    pub kind: String,
    pub from_node: String,
    pub to_node: String,
    pub source_message_id: Option<String>,
    pub target_request_id: Option<String>,
    pub metadata: Option<String>,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct View {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub graph_revision: u64,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NodeReply {
    pub node: Node,
    pub graph_revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DependenciesReply {
    pub node: Node,
    pub edges: Vec<Edge>,
    pub graph_revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RevisionReply {
    pub graph_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request<'a> {
    Graph {
        #[serde(skip_serializing_if = "Option::is_none")]
        project: Option<&'a Path>,
    },
    CreateDraft {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        project: Option<&'a Path>,
        #[serde(skip_serializing_if = "Option::is_none")]
        seed_text: Option<&'a str>,
        checkout: &'a Path,
        title: &'a str,
        prompt: &'a str,
        kind: &'a str,
        placement: &'a str,
        harness: &'a str,
        provider: &'a str,
        model_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<&'a str>,
        depends_on: &'a [String],
        auto_start: bool,
    },
    ForkThread {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        parent_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        at_message_id: Option<&'a str>,
        seed_mode: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        brief: Option<&'a str>,
        title: &'a str,
        prompt: &'a str,
        kind: &'a str,
        placement: &'a str,
        harness: &'a str,
        provider: &'a str,
        model_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        depends_on: Option<&'a [String]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        auto_start: Option<bool>,
        start_immediately: bool,
    },
    SetDependencies {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        node_id: &'a str,
        depends_on: &'a [String],
        auto_start: bool,
    },
    SetNodeKind {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        node_id: &'a str,
        kind: &'a str,
        pinned_root: bool,
    },
    DeleteDraft {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        node_id: &'a str,
    },
}

pub fn view(client: &Client, project: Option<&Path>) -> Result<View, Error> {
    client.send(&Request::Graph { project })
}

/// Send a versionless graph request and decode its operation-specific reply.
pub fn request<T: serde::de::DeserializeOwned>(
    client: &Client,
    request: &Request<'_>,
) -> Result<T, Error> {
    client.send(request)
}

pub fn needs_host_update(error: &Error) -> bool {
    matches!(error, Error::Rejected(error) if error.code == "invalid_request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fork_omits_unspecified_fields() {
        let value = serde_json::to_value(Request::ForkThread {
            op_id: None,
            parent_id: "parent",
            at_message_id: None,
            seed_mode: "excerpt",
            brief: None,
            title: "title",
            prompt: "prompt",
            kind: "build",
            placement: "project",
            harness: "pi",
            provider: "p",
            model_id: "m",
            thinking: None,
            mode: None,
            approval: None,
            depends_on: None,
            auto_start: None,
            start_immediately: false,
        })
        .unwrap();
        assert!(value.get("at_message_id").is_none());
        assert!(value.get("brief").is_none());
        assert!(value.get("depends_on").is_none());
    }

    #[test]
    fn graph_reply_shapes_decode() {
        // Shapes produced by graph.test.ts: create and fork return a node,
        // dependency changes also return edges, and deletion returns a revision.
        let node = json!({"id":"a","project":"/p","checkout":"/p","lifecycle":"draft","kind":"build","pinned_root":false,"title":"A","status":"draft","waiting":null,"queue_position":null});
        let edge = json!({"id":"e","kind":"depends_on","from_node":"a","to_node":"b","source_message_id":null,"target_request_id":null,"metadata":null,"created_at":1});
        let view: View = serde_json::from_value(
            json!({"nodes":[node],"edges":[edge],"graph_revision":2,"extra":true}),
        )
        .unwrap();
        assert_eq!(view.nodes[0].status, Status::Draft);
        for _op in ["create_draft", "fork_thread", "set_node_kind"] {
            let _: NodeReply =
                serde_json::from_value(json!({"node":node,"graph_revision":1})).unwrap();
        }
        let _: DependenciesReply =
            serde_json::from_value(json!({"node":node,"edges":[edge],"graph_revision":2})).unwrap();
        let _: RevisionReply = serde_json::from_value(json!({"graph_revision":3})).unwrap();
        let mut future = node;
        future["status"] = json!("future");
        assert_eq!(
            serde_json::from_value::<Node>(future).unwrap().status,
            Status::Other
        );
        assert_eq!(
            serde_json::to_value(Request::Graph { project: None }).unwrap(),
            json!({"op":"graph"})
        );
    }
}
