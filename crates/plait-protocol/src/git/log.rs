/// One commit in the history list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Commit {
    /// Full object ID in hex (40 for SHA-1, 64 for SHA-256 repositories).
    pub id: String,
    /// Abbreviated ID as Git prints it.
    pub short: String,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub subject: String,
}

/// A page of history, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct History {
    pub commits: Vec<Commit>,
    /// More commits exist after this page.
    pub more: bool,
}

/// A single bounded, hardened read for the project graph; parents retain topology.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphCommit {
    pub commit: Commit,
    pub parents: Vec<String>,
}
pub const GRAPH_LIMIT: usize = 300;
