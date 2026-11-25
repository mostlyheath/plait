use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Change {
    #[serde(with = "crate::wire::path")]
    pub path: PathBuf,
    #[serde(with = "crate::wire::opt_path")]
    pub original: Option<PathBuf>,
    pub index: u8,
    pub worktree: u8,
}

impl Change {
    pub fn staged(&self) -> bool {
        !matches!(self.index, b' ' | b'?')
    }
    pub fn unstaged(&self) -> bool {
        self.worktree != b' '
    }
    /// An unmerged path: staging it is how Git records it as resolved.
    pub fn conflicted(&self) -> bool {
        self.index == b'U'
            || self.worktree == b'U'
            || matches!((self.index, self.worktree), (b'A', b'A') | (b'D', b'D'))
    }
    pub fn label(&self) -> &'static str {
        if self.index == b'?' {
            "New"
        } else if self.conflicted() {
            "Conflict"
        } else if self.index == b'R' || self.worktree == b'R' {
            "Renamed"
        } else if self.index == b'D' || self.worktree == b'D' {
            "Deleted"
        } else if self.index == b'A' {
            "Added"
        } else {
            "Modified"
        }
    }
    pub fn display_path(&self) -> String {
        self.path
            .to_string_lossy()
            .chars()
            .flat_map(|c| {
                if c.is_control() {
                    c.escape_default().collect::<Vec<_>>()
                } else {
                    vec![c]
                }
            })
            .collect()
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    #[serde(with = "crate::wire::path")]
    pub root: PathBuf,
    pub branch: String,
    /// On a branch with at least one commit, so there is something to push.
    pub named: bool,
    /// The branch's upstream, if it has one.
    pub tracking: Option<Tracking>,
    pub changes: Vec<Change>,
    pub elapsed: Duration,
    /// Not a Git repository: a plain folder has no changes, history, or undo.
    #[serde(default)]
    pub plain: bool,
}

/// Where the current branch stands against its upstream, as of the last fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tracking {
    /// Such as `origin/main`.
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
    /// The upstream branch no longer exists on the remote.
    pub gone: bool,
}

/// The current branch, from the first line of `git status --branch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchHeader {
    pub branch: String,
    pub named: bool,
    pub tracking: Option<Tracking>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DiffMode {
    Unstaged,
    Staged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
    Hunk,
    Header,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DiffLine {
    pub text: Arc<str>,
    pub kind: DiffKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Diff {
    /// The patch as Git wrote it, clipped for display.
    pub lines: Vec<DiffLine>,
    /// What is drawn, each row pointing into `lines` (see `crate::patch`).
    pub rows: Vec<crate::patch::PatchRow>,
    /// The widest drawn row, as an index into `rows`.
    pub widest_row: usize,
    /// Indexes into `lines` of each `@@` hunk header, in order, so a row
    /// finds its hunk by binary search instead of scanning.
    pub hunks: Vec<usize>,
    pub truncated: bool,
    pub elapsed: Duration,
}

impl Diff {
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        size_of::<Self>()
            + self.lines.capacity() * size_of::<DiffLine>()
            + self.lines.iter().map(|line| line.text.len()).sum::<usize>()
            + self.rows.capacity() * size_of::<crate::patch::PatchRow>()
            + self
                .rows
                .iter()
                .map(|row| match row {
                    crate::patch::PatchRow::File {
                        name, directory, ..
                    } => name.len() + directory.len(),
                    _ => 0,
                })
                .sum::<usize>()
            + self.hunks.capacity() * size_of::<usize>()
    }
    /// Semantic bookmark guard; independent of presentation indexes.
    pub fn fingerprint(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for line in &self.lines {
            line.text.hash(&mut hash);
            line.old_line.hash(&mut hash);
            line.new_line.hash(&mut hash);
        }
        format!("patch-{:016x}", hash.finish())
    }
    pub fn message(text: &str, elapsed: Duration) -> Self {
        Self {
            lines: vec![DiffLine {
                text: Arc::from(text),
                kind: DiffKind::Header,
                old_line: None,
                new_line: None,
            }],
            rows: vec![crate::patch::PatchRow::Line(0)],
            widest_row: 0,
            hunks: Vec::new(),
            truncated: false,
            elapsed,
        }
    }
}
