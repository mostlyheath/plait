/// More branches than this are not listed; the list says so.
pub const BRANCH_LIMIT: usize = 500;

/// One local branch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Branch {
    /// The short name, such as `main` or `feature/login`.
    pub name: String,
    /// HEAD is on this branch.
    pub current: bool,
    /// Committer time of its tip, seconds since the Unix epoch.
    pub time: i64,
    /// Another worktree has this branch checked out; Git refuses to switch to it.
    pub elsewhere: Option<String>,
}

/// Local branches, most recently committed first.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Branches {
    pub list: Vec<Branch>,
    /// More branches exist than `BRANCH_LIMIT`.
    pub more: bool,
}
