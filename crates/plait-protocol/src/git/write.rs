//! Git write request and outcome types.
use crate::model::DiffMode;
use std::path::PathBuf;

/// One write. Paths are relative to the repository root, as `Change::path`
/// reports them; a rename lists both its path and its original.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WriteOp {
    Stage(#[serde(with = "crate::wire::paths")] Vec<PathBuf>),
    Unstage(#[serde(with = "crate::wire::paths")] Vec<PathBuf>),
    StageAll,
    UnstageAll,
    /// Commit what is staged. The message is used as written, apart from
    /// trailing whitespace; it must not be blank.
    Commit {
        message: String,
    },
    /// Push as the user's own `git push` would, to the configured destination.
    Push,
    /// Push a branch with no upstream to its remote and make that its upstream.
    Publish,
    /// Fetch from the current branch's remote, or `origin`.
    Fetch,
    /// Fetch, then move the branch forward to its upstream. Never merges or
    /// rebases: diverged branches and conflicting local edits are refused.
    Pull,
    /// Stage one hunk of a file's unstaged diff, or unstage one hunk of its
    /// staged diff. `header` is the hunk's `@@` line as shown; the write
    /// rereads the diff and refuses if hunk `index` no longer matches it.
    Hunk {
        #[serde(with = "crate::wire::path")]
        path: PathBuf,
        from: DiffMode,
        index: usize,
        header: String,
    },
    /// Throw away a file's unstaged changes, or one unstaged hunk, after
    /// saving its bytes so `Restore` can undo it. An untracked file is deleted.
    /// `hunk` is an index and shown header, as for `Hunk`.
    Discard {
        #[serde(with = "crate::wire::path")]
        path: PathBuf,
        untracked: bool,
        hunk: Option<(usize, String)>,
    },
    /// Put back a file exactly as `Discard` saved it.
    Restore(Backup),
    /// Create a local branch at the current commit and switch to it. The name
    /// is checked by Git first; an existing name is refused.
    CreateBranch(String),
    /// Switch to an existing local branch. Git refuses a switch that would
    /// overwrite uncommitted changes; others are carried along.
    SwitchBranch(String),
}

/// A file's bytes saved in Git's object store before a discard. The object is
/// unreferenced, so Git's own garbage collection removes it in time.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Backup {
    #[serde(with = "crate::wire::path")]
    pub path: PathBuf,
    pub blob: String,
    pub executable: bool,
}

/// What a write did, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WriteOutcome {
    /// Such as "Staged 2 files" or "Committed 1a2b3c4: Fix the parser".
    pub summary: String,
    /// The new commit's full ID, after a commit.
    pub commit: Option<String>,
    /// What a discard saved, so it can be undone.
    pub backup: Option<Backup>,
}
