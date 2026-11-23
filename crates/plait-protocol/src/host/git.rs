//! Wire types for host Git operations.
use crate::{
    file_view::Stamp,
    git::WriteOp,
    lease::DirIdentity,
    model::{Change, DiffMode},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One read of a checkout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// Flattened beside `checkout`, so unknown fields are refused by the runtime.
#[serde(tag = "read", rename_all = "snake_case")]
pub enum Read {
    Status,
    CheckoutSummary,
    MainCheckout,
    Identity,
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
    /// One worktree's size, measured as the Worktrees view measures this Mac's.
    /// The host adds its own store, for the thread's retained build output.
    WorktreeSize {
        #[serde(with = "crate::wire::path")]
        worktree: PathBuf,
        worktree_identity: Option<WireIdentity>,
        thread: Option<String>,
        removed: bool,
        /// Other checkouts nested inside this one, measured as themselves.
        others: Vec<WireIdentity>,
        build_directories: Vec<String>,
    },
}

/// Lightweight identity and branch information for a checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutSummary {
    pub vcs: Vcs,
    #[serde(default)]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default)]
    pub branch: Option<BranchSummary>,
    #[serde(default, with = "crate::wire::opt_path")]
    pub main_checkout: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vcs {
    Git,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub roots: Vec<String>,
    /// Display hint and weak grouping signal; copies match by roots because remotes change when accounts or repositories are renamed.
    #[serde(default)]
    pub remote: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchSummary {
    pub branch: String,
    pub named: bool,
    #[serde(default)]
    pub tracking: Option<crate::model::Tracking>,
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
    pub fn parse(&self) -> Result<DirIdentity, String> {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        op_id: Option<String>,
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
    /// The host's store, added by the runtime; a client never names it.
    #[serde(default, with = "crate::wire::opt_path")]
    pub store: Option<PathBuf>,
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

#[cfg(test)]
mod summary_tests {
    use super::*;
    #[test]
    fn summary_round_trip_and_unknown_fields() {
        let summary = CheckoutSummary {
            vcs: Vcs::Git,
            fingerprint: Some(Fingerprint {
                roots: vec!["abc".into()],
                remote: None,
            }),
            branch: Some(BranchSummary {
                branch: "main".into(),
                named: true,
                tracking: None,
            }),
            main_checkout: Some(PathBuf::from("/tmp/main")),
        };
        let mut value = serde_json::to_value(&summary).unwrap();
        value["future"] = serde_json::json!(true);
        value["fingerprint"]["future"] = serde_json::json!(1);
        assert_eq!(
            serde_json::from_value::<CheckoutSummary>(value).unwrap(),
            summary
        );
        assert_eq!(
            serde_json::from_str::<Read>(&serde_json::to_string(&Read::CheckoutSummary).unwrap())
                .unwrap(),
            Read::CheckoutSummary
        );
        let minimal = serde_json::json!({"vcs":"folder"});
        let decoded: CheckoutSummary = serde_json::from_value(minimal).unwrap();
        assert_eq!(decoded.fingerprint, None);
    }
}
