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
        self, Branches, FileList, LineCount,
        log::{GraphCommit, History},
    },
    lease::DirIdentity,
    model::{Change, Diff, DiffMode, Snapshot},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    os::unix::fs::MetadataExt,
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

/// One read of a checkout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// Flattened beside `checkout`, so unknown fields are refused by the runtime.
#[serde(tag = "read", rename_all = "snake_case")]
pub enum Read {
    Status,
    Diff {
        change: WireChange,
        mode: DiffMode,
    },
    Files,
    Log {
        tip: Option<String>,
        skip: usize,
        limit: usize,
    },
    Show {
        id: String,
    },
    Branches,
    LineCounts {
        #[serde(with = "crate::wire::paths")]
        paths: Vec<PathBuf>,
    },
    Graph {
        heads: Vec<String>,
    },
    File {
        #[serde(with = "crate::wire::path")]
        path: PathBuf,
    },
    FileStamp {
        #[serde(with = "crate::wire::path")]
        path: PathBuf,
    },
}

/// `Change` without the fields a diff does not use, comparable in tests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireChange {
    #[serde(with = "crate::wire::path")]
    pub path: PathBuf,
    #[serde(with = "crate::wire::opt_path")]
    pub original: Option<PathBuf>,
    pub index: u8,
    pub worktree: u8,
}

impl From<&Change> for WireChange {
    fn from(change: &Change) -> Self {
        Self {
            path: change.path.clone(),
            original: change.original.clone(),
            index: change.index,
            worktree: change.worktree,
        }
    }
}

impl From<WireChange> for Change {
    fn from(change: WireChange) -> Self {
        Self {
            path: change.path,
            original: change.original,
            index: change.index,
            worktree: change.worktree,
        }
    }
}

/// A checkout's device and inode, as decimal strings so JavaScript keeps them exact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireIdentity {
    pub dev: String,
    pub ino: String,
}

impl From<DirIdentity> for WireIdentity {
    fn from(id: DirIdentity) -> Self {
        Self {
            dev: id.dev.to_string(),
            ino: id.ino.to_string(),
        }
    }
}

impl WireIdentity {
    fn parse(&self) -> Result<DirIdentity, String> {
        let number = |text: &str| {
            text.parse::<u64>()
                .map_err(|_| "Invalid checkout identity.")
        };
        Ok(DirIdentity {
            dev: number(&self.dev)?,
            ino: number(&self.ino)?,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    GitRead {
        #[serde(with = "crate::wire::path")]
        checkout: PathBuf,
        identity: Option<WireIdentity>,
        #[serde(flatten)]
        read: Read,
    },
    GitReadPage {
        token: String,
        offset: usize,
    },
    GitWrite {
        #[serde(with = "crate::wire::path")]
        checkout: PathBuf,
        identity: WireIdentity,
        write: WriteOp,
    },
}

/// What the helper writes: the checkout's identity with the result, or the
/// same message the desktop's local read would show.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome<T> {
    Ok { identity: WireIdentity, result: T },
    Error { message: String },
}

/// A file as the host read it: stamp and bytes, or no bytes when too large.
#[derive(Debug, Serialize, Deserialize)]
pub struct FileBytes {
    pub stamp: Stamp,
    pub base64: Option<String>,
}

/// The host's input to the helper, after the runtime's checks.
#[derive(Debug, Deserialize)]
pub struct HelperInput {
    #[serde(with = "crate::wire::path")]
    pub checkout: PathBuf,
    pub identity: Option<WireIdentity>,
    #[serde(flatten)]
    pub read: Read,
}

/// The host's input to the write helper. A write always names the identity
/// the desktop last read, as a local write does.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteInput {
    #[serde(with = "crate::wire::path")]
    pub checkout: PathBuf,
    pub identity: WireIdentity,
    pub write: WriteOp,
}

/// Blocking, on the host. Runs one write under the checkout's leases, as the
/// desktop does for this Mac's checkouts, and serializes its outcome.
pub fn execute_write(input: &WriteInput) -> Vec<u8> {
    let outcome = match write_checked(input) {
        Ok((identity, result)) => Outcome::Ok {
            identity: identity.into(),
            result,
        },
        Err(message) => Outcome::Error { message },
    };
    serde_json::to_vec(&outcome).unwrap_or_default()
}

fn write_checked(input: &WriteInput) -> Result<(DirIdentity, WriteOutcome), String> {
    if !input.checkout.is_absolute() {
        return Err("The checkout path must be absolute.".into());
    }
    let expected = input.identity.parse()?;
    if git::toplevel(&input.checkout).and_then(|top| identity(&top))? != expected {
        return Err("Open the top folder of the checkout.".into());
    }
    let outcome = git::write_checked(&input.checkout, Some(expected), &input.write)?;
    Ok((expected, outcome))
}

/// Blocking, on the host. Runs one read and serializes its outcome.
pub fn execute(input: &HelperInput) -> Vec<u8> {
    let outcome = checked(input);
    serde_json::to_vec(&outcome).unwrap_or_else(|_| {
        serde_json::to_vec(&Outcome::<()>::Error {
            message: "The host could not encode this read.".into(),
        })
        .unwrap_or_default()
    })
}

fn checked(input: &HelperInput) -> Outcome<serde_json::Value> {
    match run_checked(input) {
        Ok((identity, result)) => Outcome::Ok {
            identity: identity.into(),
            result,
        },
        Err(message) => Outcome::Error { message },
    }
}

/// The checkout is the top of a work tree with the expected identity, before
/// and after the read, so a replaced folder never answers for the old one.
fn run_checked(input: &HelperInput) -> Result<(DirIdentity, serde_json::Value), String> {
    let root = &input.checkout;
    if !root.is_absolute() {
        return Err("The checkout path must be absolute.".into());
    }
    let before = identity(root)?;
    if let Some(expected) = &input.identity
        && expected.parse()? != before
    {
        return Err("Checkout identity changed.".into());
    }
    let top = git::toplevel(root)?;
    if identity(&top)? != before {
        return Err("Open the top folder of the checkout.".into());
    }
    let result = dispatch(root, &input.read)?;
    if identity(root)? != before {
        return Err("Checkout identity changed.".into());
    }
    Ok((before, result))
}

fn identity(path: &Path) -> Result<DirIdentity, String> {
    let meta =
        std::fs::symlink_metadata(path).map_err(|_| "The checkout is missing.".to_string())?;
    if !meta.is_dir() {
        return Err("The checkout is not a folder.".into());
    }
    Ok(DirIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

fn value<T: Serialize>(result: Result<T, String>) -> Result<serde_json::Value, String> {
    serde_json::to_value(result?).map_err(|_| "The host could not encode this read.".into())
}

fn dispatch(root: &Path, read: &Read) -> Result<serde_json::Value, String> {
    match read {
        Read::Status => value(git::status(root)),
        Read::Diff { change, mode } => value(git::diff(root, &change.clone().into(), *mode)),
        Read::Files => value(git::files(root)),
        Read::Log { tip, skip, limit } => value(git::log::log(root, tip.as_deref(), *skip, *limit)),
        Read::Show { id } => value(git::log::show(root, id)),
        Read::Branches => value(git::branches(root)),
        Read::LineCounts { paths } => value(git::line_counts(root, paths)),
        Read::Graph { heads } => value(git::log::graph(root, heads)),
        Read::File { path } => {
            value(
                file_view::read_bytes(root, path).map(|(stamp, bytes)| FileBytes {
                    stamp,
                    base64: bytes.map(|b| STANDARD.encode(b)),
                }),
            )
        }
        Read::FileStamp { path } => value(file_view::stamp(root, path)),
    }
}

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
        result: file_view::classify(remote.result.stamp, bytes, path),
    })
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
    let first: Page = client
        .send_within(
            &Request::GitWrite {
                checkout: checkout.to_owned(),
                identity: expected.into(),
                write: op.clone(),
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
