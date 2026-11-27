//! Additive Git and file reads run by a host for a remote desktop. No protocol
//! version is sent or negotiated.
//!
//! The host runs the same `git` and `file_view` functions the desktop runs for
//! this Mac's checkouts, so both share one hardened command builder and one set
//! of limits. The whole result is computed once, which keeps one snapshot even
//! when it is paged. The runtime keeps the serialized result and the client
//! reads it in bounded pages, because a reply is limited to 1 MiB.
use super::{Client, Error};
use crate::{
    file_view::{self, Stamp},
    git::WriteOp,
    git::WriteOutcome,
    git::{
        Branches, FileList, LineCount,
        log::{GraphCommit, History},
    },
    lease::DirIdentity,
    model::{Change, Diff, DiffMode, Snapshot},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub const OLDER_HOST: &str = "Update this machine's host to see its Git.";
/// The runtime stops a read after 60 seconds; wait a little longer for its answer.
const READ_WAIT: Duration = Duration::from_secs(65);
/// A write may wait for the checkout's lease and then run a 120-second network
/// command; the runtime stops it at 240 seconds.
const WRITE_WAIT: Duration = Duration::from_secs(245);
/// The most a client reassembles; a larger result is refused, not truncated.
pub const RESULT_LIMIT: usize = 64 * 1024 * 1024;

pub use plait_protocol::host::git::{
    FileBytes, HelperInput, Outcome, Read, Request, WireChange, WireIdentity, WriteInput,
};

/// The runtime's reply: the first page and, when there are more, a token.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    total: usize,
    data: String,
    token: Option<String>,
}

/// A read's result from a remote host, with the identity it was read under.
#[derive(Debug)]
pub struct Remote<T> {
    pub identity: DirIdentity,
    pub result: T,
}

pub fn needs_host_update(error: &Error) -> bool {
    matches!(error, Error::Rejected(error) if error.code == "invalid_request")
}

/// Blocking. One read of `checkout` on the host behind `client`, reassembled
/// from pages. A local read's message comes back as `Error::Protocol`, and an
/// older host as `OLDER_HOST`.
pub fn read<T: DeserializeOwned>(
    client: &Client,
    checkout: &Path,
    expected: Option<DirIdentity>,
    read: Read,
) -> Result<Remote<T>, Error> {
    let first: Page = client
        .send_within(
            &Request::GitRead {
                checkout: checkout.to_owned(),
                identity: expected.map(Into::into),
                read,
            },
            READ_WAIT,
        )
        .map_err(|e| {
            if needs_host_update(&e) {
                Error::Protocol(OLDER_HOST.into())
            } else {
                e
            }
        })?;
    let bytes = assemble(first, |token, offset| {
        client.send(&Request::GitReadPage {
            token: token.into(),
            offset,
        })
    })?;
    decode(&bytes)
}

fn assemble(
    first: Page,
    mut next: impl FnMut(&str, usize) -> Result<Page, Error>,
) -> Result<Vec<u8>, Error> {
    let total = first.total;
    if total > RESULT_LIMIT {
        return Err(Error::Protocol("This read is too large to show.".into()));
    }
    let mut bytes = Vec::with_capacity(total);
    let mut page = first;
    loop {
        let data = STANDARD
            .decode(&page.data)
            .map_err(|_| Error::Protocol("The host sent a malformed page.".into()))?;
        if data.is_empty() && bytes.len() < total {
            return Err(Error::Protocol("The host sent an empty page.".into()));
        }
        bytes.extend_from_slice(&data);
        if bytes.len() > total || page.total != total {
            return Err(Error::Protocol("The host's pages do not agree.".into()));
        }
        if bytes.len() == total {
            return Ok(bytes);
        }
        let token = page
            .token
            .ok_or_else(|| Error::Protocol("The host ended the read early.".into()))?;
        page = next(&token, bytes.len())?;
    }
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<Remote<T>, Error> {
    let outcome: Outcome<T> = serde_json::from_slice(bytes)
        .map_err(|_| Error::Protocol("The host sent an unreadable result.".into()))?;
    match outcome {
        Outcome::Ok { identity, result } => Ok(Remote {
            identity: identity.parse().map_err(Error::Protocol)?,
            result,
        }),
        Outcome::Error { message } => Err(Error::Protocol(message)),
    }
}

/// Canonical checkout path and identity reported by the host.
#[derive(Debug, Deserialize)]
pub struct CheckoutIdentity {
    #[serde(with = "crate::wire::path")]
    pub root: PathBuf,
    pub identity: WireIdentity,
}

pub fn identity(
    c: &Client,
    root: &Path,
    expected: Option<DirIdentity>,
) -> Result<Remote<CheckoutIdentity>, Error> {
    let result: Remote<CheckoutIdentity> = read(c, root, expected, Read::Identity)?;
    if result.result.identity.parse().map_err(Error::Protocol)? != result.identity {
        return Err(Error::Protocol(
            "Host returned inconsistent checkout identity.".into(),
        ));
    }
    Ok(result)
}

pub fn main_checkout(
    c: &Client,
    root: &Path,
    expected: Option<DirIdentity>,
) -> Result<Remote<PathBuf>, Error> {
    #[derive(Deserialize)]
    struct PathResult(#[serde(with = "crate::wire::path")] PathBuf);
    let result: Remote<PathResult> = read(c, root, expected, Read::MainCheckout)?;
    Ok(Remote {
        identity: result.identity,
        result: result.result.0,
    })
}

/// Typed reads, so callers cannot pair a read with the wrong result type.
pub fn status(c: &Client, root: &Path, id: Option<DirIdentity>) -> Result<Remote<Snapshot>, Error> {
    read(c, root, id, Read::Status)
}
pub fn diff(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    change: &Change,
    mode: DiffMode,
) -> Result<Remote<Diff>, Error> {
    let change = change.into();
    read(c, root, id, Read::Diff { change, mode })
}
pub fn files(c: &Client, root: &Path, id: Option<DirIdentity>) -> Result<Remote<FileList>, Error> {
    read(c, root, id, Read::Files)
}
pub fn log(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    tip: Option<&str>,
    skip: usize,
    limit: usize,
) -> Result<Remote<History>, Error> {
    let tip = tip.map(str::to_owned);
    read(c, root, id, Read::Log { tip, skip, limit })
}
pub fn show(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    commit: &str,
) -> Result<Remote<Diff>, Error> {
    read(c, root, id, Read::Show { id: commit.into() })
}
pub fn branches(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
) -> Result<Remote<Branches>, Error> {
    read(c, root, id, Read::Branches)
}
pub fn line_counts(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    paths: &[PathBuf],
) -> Result<Remote<Vec<LineCount>>, Error> {
    let paths = paths.to_vec();
    read(c, root, id, Read::LineCounts { paths })
}
pub fn graph(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    heads: &[String],
) -> Result<Remote<(Vec<GraphCommit>, bool)>, Error> {
    let heads = heads.to_vec();
    read(c, root, id, Read::Graph { heads })
}
/// The file classified and highlighted here, from the host's bytes.
pub fn file(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    path: &Path,
) -> Result<Remote<file_view::Opened>, Error> {
    let remote = file_bytes(c, root, id, path)?;
    Ok(Remote {
        identity: remote.identity,
        result: file_view::classify(remote.result.0, remote.result.1, path),
    })
}
type FileRead = (file_view::Stamp, Option<Vec<u8>>);
pub(crate) fn file_bytes(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    path: &Path,
) -> Result<Remote<FileRead>, Error> {
    let remote: Remote<FileBytes> = read(c, root, id, Read::File { path: path.into() })?;
    let bytes = match remote.result.base64 {
        Some(text) => Some(
            STANDARD
                .decode(text)
                .map_err(|_| Error::Protocol("The host sent malformed file bytes.".into()))?,
        ),
        None => None,
    };
    Ok(Remote {
        identity: remote.identity,
        result: (remote.result.stamp, bytes),
    })
}
/// One worktree's size, measured by the host that owns it.
pub fn worktree_size(
    c: &Client,
    project: &Path,
    key: &crate::worktree_size::Key,
    thread: Option<&str>,
    others: &std::collections::HashSet<crate::worktree_size::DirId>,
) -> Result<crate::worktree_size::Size, Error> {
    let size = Read::WorktreeSize {
        worktree: key.root.clone(),
        worktree_identity: key.identity.as_ref().map(|id| WireIdentity {
            dev: id.dev.clone(),
            ino: id.ino.clone(),
        }),
        thread: thread.map(str::to_owned),
        removed: key.removed,
        others: others
            .iter()
            .map(|&(dev, ino)| DirIdentity { dev, ino }.into())
            .collect(),
        build_directories: key.build_directories.clone(),
    };
    read(c, project, None, size).map(|r| r.result)
}
pub fn file_stamp(
    c: &Client,
    root: &Path,
    id: Option<DirIdentity>,
    path: &Path,
) -> Result<Remote<Stamp>, Error> {
    read(c, root, id, Read::FileStamp { path: path.into() })
}

/// Blocking. One write to `checkout` on the host behind `client`. The host
/// finishes a write it started even if this connection drops, so a caller
/// rereads status rather than assuming either outcome.
pub fn write(
    client: &Client,
    checkout: &Path,
    expected: DirIdentity,
    op: &WriteOp,
) -> Result<WriteOutcome, Error> {
    write_with_op(
        client,
        checkout,
        expected,
        op,
        Some(&crate::host_state::request_id()),
    )
}

pub fn write_with_op(
    client: &Client,
    checkout: &Path,
    expected: DirIdentity,
    op: &WriteOp,
    op_id: Option<&str>,
) -> Result<WriteOutcome, Error> {
    let mut write = op.clone();
    if let WriteOp::Discard { guard, .. } = &mut write
        && guard.is_some()
        && !super::release::host_info(client)?
            .is_some_and(|info| info.capabilities.iter().any(|c| c == "guarded_discard_v1"))
    {
        *guard = None;
    }
    let first: Page = client
        .send_within(
            &Request::GitWrite {
                op_id: op_id.map(str::to_owned),
                checkout: checkout.to_owned(),
                identity: expected.into(),
                write,
            },
            WRITE_WAIT,
        )
        .map_err(|e| {
            if needs_host_update(&e) {
                Error::Protocol(OLDER_HOST_WRITE.into())
            } else {
                e
            }
        })?;
    let bytes = assemble(first, |token, offset| {
        client.send(&Request::GitReadPage {
            token: token.into(),
            offset,
        })
    })?;
    Ok(decode::<WriteOutcome>(&bytes)?.result)
}

pub const OLDER_HOST_WRITE: &str = "Update this machine's host to change its Git.";

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
