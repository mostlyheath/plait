use std::path::PathBuf;

/// One path that differs from HEAD. Counts are `None` for binary files and
/// untracked files too large to count.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LineCount {
    #[serde(with = "crate::wire::path")]
    pub path: PathBuf,
    pub added: Option<usize>,
    pub removed: Option<usize>,
    /// Not tracked by Git yet.
    pub untracked: bool,
}
