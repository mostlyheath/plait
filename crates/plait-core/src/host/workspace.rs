//! The person's workspace on a host: its identity, which projects are open or
//! hidden, worktree ownership, and what has been seen. Additive operations;
//! no protocol version is sent. A host that does not know them answers
//! `invalid_request`, which `needs_host_update` names.
use super::{Client, Error, Frames, decode};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How long a host may take to send the first snapshot.
const SNAPSHOT_WAIT: Duration = Duration::from_secs(20);

pub use plait_protocol::host::workspace::{HostIdentity, Project, Seen};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub folder: PathBuf,
    pub main: PathBuf,
    pub branch: String,
    pub creation_id: Option<String>,
    pub identity: Option<crate::git::worktree::Identity>,
}

/// One consistent view of a host's workspace.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct View {
    pub schema: u32,
    pub host: HostIdentity,
    pub revision: u64,
    #[serde(default)]
    pub status: BTreeMap<PathBuf, u64>,
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub seen: Vec<Seen>,
}
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request<'a> {
    WorkspaceView,
    WorkspaceSubscribe,
    WorkspaceOpenProject {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        path: &'a Path,
    },
    WorkspaceHideProject {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        path: &'a Path,
        hidden: bool,
    },
    WorkspaceSetWorktree {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        #[serde(flatten)]
        worktree: &'a Worktree,
    },
    WorkspaceForgetWorktree {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        folder: &'a Path,
    },
    WorkspaceMarkSeen {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        entries: &'a [Seen],
    },
    WorkspaceImport {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        device: &'a str,
        projects: &'a [PathBuf],
        hidden: &'a [PathBuf],
        worktrees: &'a [Worktree],
        seen: &'a [Seen],
    },
    HostSetName {
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<&'a str>,
        name: &'a str,
    },
}

pub fn needs_host_update(error: &Error) -> bool {
    matches!(error, Error::Rejected(error) if error.code == "invalid_request")
}

pub fn view(client: &Client) -> Result<View, Error> {
    client.send(&Request::WorkspaceView)
}

#[derive(Deserialize)]
struct Revision {
    revision: u64,
}

/// Opens a project the host has checked is a Git checkout; returns its top folder.
pub fn open_project(client: &Client, path: &Path) -> Result<PathBuf, Error> {
    open_project_with_op(client, path, Some(&crate::host_state::request_id()))
}

pub fn open_project_with_op(
    client: &Client,
    path: &Path,
    op_id: Option<&str>,
) -> Result<PathBuf, Error> {
    #[derive(Deserialize)]
    struct Opened {
        path: PathBuf,
    }
    let opened: Opened = client.send(&Request::WorkspaceOpenProject { path, op_id })?;
    Ok(opened.path)
}

pub fn hide_project(client: &Client, path: &Path, hidden: bool) -> Result<u64, Error> {
    hide_project_with_op(client, path, hidden, Some(&crate::host_state::request_id()))
}

pub fn hide_project_with_op(
    client: &Client,
    path: &Path,
    hidden: bool,
    op_id: Option<&str>,
) -> Result<u64, Error> {
    let reply: Revision = client.send(&Request::WorkspaceHideProject {
        path,
        hidden,
        op_id,
    })?;
    Ok(reply.revision)
}

pub fn set_worktree(client: &Client, worktree: &Worktree) -> Result<u64, Error> {
    set_worktree_with_op(client, worktree, Some(&crate::host_state::request_id()))
}

pub fn set_worktree_with_op(
    client: &Client,
    worktree: &Worktree,
    op_id: Option<&str>,
) -> Result<u64, Error> {
    let reply: Revision = client.send(&Request::WorkspaceSetWorktree { worktree, op_id })?;
    Ok(reply.revision)
}

pub fn forget_worktree(client: &Client, folder: &Path) -> Result<u64, Error> {
    forget_worktree_with_op(client, folder, Some(&crate::host_state::request_id()))
}

pub fn forget_worktree_with_op(
    client: &Client,
    folder: &Path,
    op_id: Option<&str>,
) -> Result<u64, Error> {
    let reply: Revision = client.send(&Request::WorkspaceForgetWorktree { folder, op_id })?;
    Ok(reply.revision)
}

pub fn mark_seen(client: &Client, entries: &[Seen]) -> Result<u64, Error> {
    mark_seen_with_op(client, entries, Some(&crate::host_state::request_id()))
}

pub fn mark_seen_with_op(
    client: &Client,
    entries: &[Seen],
    op_id: Option<&str>,
) -> Result<u64, Error> {
    let reply: Revision = client.send(&Request::WorkspaceMarkSeen { entries, op_id })?;
    Ok(reply.revision)
}

/// This device's lists before the host kept them. A host merges them once
/// per device and reports whether it did.
pub struct Import<'a> {
    pub device: &'a str,
    pub projects: &'a [PathBuf],
    pub hidden: &'a [PathBuf],
    pub worktrees: &'a [Worktree],
    pub seen: &'a [Seen],
}

pub fn import(client: &Client, lists: &Import<'_>) -> Result<bool, Error> {
    import_with_op(client, lists, Some(&crate::host_state::request_id()))
}

pub fn import_with_op(
    client: &Client,
    lists: &Import<'_>,
    op_id: Option<&str>,
) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct Imported {
        imported: bool,
    }
    let reply: Imported = client.send(&Request::WorkspaceImport {
        op_id,
        device: lists.device,
        projects: lists.projects,
        hidden: lists.hidden,
        worktrees: lists.worktrees,
        seen: lists.seen,
    })?;
    Ok(reply.imported)
}

pub fn set_host_name(client: &Client, name: &str) -> Result<HostIdentity, Error> {
    set_host_name_with_op(client, name, Some(&crate::host_state::request_id()))
}

pub fn set_host_name_with_op(
    client: &Client,
    name: &str,
    op_id: Option<&str>,
) -> Result<HostIdentity, Error> {
    #[derive(Deserialize)]
    struct Named {
        host: HostIdentity,
    }
    let reply: Named = client.send(&Request::HostSetName { name, op_id })?;
    Ok(reply.host)
}

/// Views delivered on a dedicated thread, latest first: each is complete,
/// so a slow reader needs only the newest.
pub type Views = super::Updates<View>;

/// Subscribes and reads on a dedicated thread. Dropping the result, or its
/// cancel handle, unsubscribes.
pub fn watch(client: &Client) -> Result<Views, Error> {
    let mut stream = subscribe(client)?;
    let socket = stream.frames.reader.get_ref().try_clone()?;
    super::Updates::spawn(socket, move || stream.next_view())
}

/// A snapshot, then each later view in order. Dropping it unsubscribes.
pub struct Stream {
    frames: Frames,
    first: bool,
    seq: u64,
}

pub fn subscribe(client: &Client) -> Result<Stream, Error> {
    let bytes = serde_json::to_vec(&Request::WorkspaceSubscribe)
        .map_err(|_| Error::Protocol("Cannot encode host request.".into()))?;
    Ok(Stream {
        frames: client.send_encoded(&bytes)?,
        first: true,
        seq: 0,
    })
}

impl Stream {
    /// Blocking. The first call returns the snapshot; later calls wait for the
    /// next change. A gap in the numbering is an error, so the caller
    /// subscribes again rather than trusting a view that skipped a change.
    pub fn next_view(&mut self) -> Result<View, Error> {
        if self.first {
            self.first = false;
            let line = self.frames.next(Some(Instant::now() + SNAPSHOT_WAIT))?;
            return decode(&line);
        }
        let line = self.frames.next(None)?;
        parse_event(&line, &mut self.seq)
    }
}

fn parse_event(line: &[u8], seq: &mut u64) -> Result<View, Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Event {
        event: String,
        seq: u64,
        workspace: View,
    }
    let event: Event = match serde_json::from_slice(line) {
        Ok(event) => event,
        // A failure reply ends the stream with the host's reason.
        Err(_) => {
            return decode::<View>(line)
                .and(Err(Error::Protocol("The workspace stream ended.".into())));
        }
    };
    if event.event != "workspace" || event.seq != *seq + 1 {
        return Err(Error::Protocol(
            "The workspace stream skipped a change; subscribe again.".into(),
        ));
    }
    *seq = event.seq;
    Ok(event.workspace)
}

#[cfg(test)]
mod tests {
    #[test]
    fn operation_id_is_optional_in_write_requests() {
        for id in [None, Some("retry_1")] {
            let value = serde_json::to_value(super::Request::HostSetName {
                name: "host",
                op_id: id,
            })
            .unwrap();
            assert_eq!(value.get("op_id").and_then(|v| v.as_str()), id);
        }
    }

    use super::*;

    const VIEW: &str = r#"{"schema":1,"host":{"id":"h","name":"workstation","platform":"linux"},"revision":2,"projects":[{"path":"/srv/a","hidden":false}],"worktrees":[],"seen":[]}"#;

    #[test]
    fn requests_carry_no_version_and_name_their_operation() {
        assert_eq!(
            serde_json::to_string(&Request::WorkspaceSubscribe).unwrap(),
            r#"{"op":"workspace_subscribe"}"#
        );
        let tree = Worktree {
            folder: "/w/t".into(),
            main: "/w/m".into(),
            branch: "plait/t".into(),
            creation_id: Some("c".into()),
            identity: None,
        };
        let value = serde_json::to_value(Request::WorkspaceSetWorktree {
            worktree: &tree,
            op_id: None,
        })
        .unwrap();
        assert_eq!(value["op"], "workspace_set_worktree");
        assert_eq!(value["folder"], "/w/t");
        assert!(value.get("protocol_version").is_none());
    }

    #[test]
    fn status_is_optional_and_parses_paths() {
        let old: View = serde_json::from_str(VIEW).unwrap();
        assert!(old.status.is_empty());
        let mut value: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        value["status"] = serde_json::json!({"/srv/a": 7});
        let view: View = serde_json::from_value(value).unwrap();
        assert_eq!(view.status.get(Path::new("/srv/a")), Some(&7));
    }

    #[test]
    fn events_must_arrive_in_order() {
        let mut seq = 0;
        let event = |n: u64| format!(r#"{{"event":"workspace","seq":{n},"workspace":{VIEW}}}"#);
        let view = parse_event(event(1).as_bytes(), &mut seq).unwrap();
        assert_eq!(
            (view.host.name.as_str(), view.revision, seq),
            ("workstation", 2, 1)
        );
        assert!(
            parse_event(event(3).as_bytes(), &mut seq).is_err(),
            "a skipped event was accepted"
        );
        let failure = r#"{"ok":false,"error":{"code":"internal","message":"Sidecar stopped."}}"#;
        assert!(
            matches!(parse_event(failure.as_bytes(), &mut seq), Err(Error::Rejected(e)) if e.message == "Sidecar stopped.")
        );
    }
}
