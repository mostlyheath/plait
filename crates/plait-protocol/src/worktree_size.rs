#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Size {
    pub source: u64,
    pub build: u64,
    pub incomplete: bool,
    pub entries: usize,
}
impl Size {
    pub fn total(&self) -> u64 {
        self.source.saturating_add(self.build)
    }
}
pub const BUILD_DIRS: &[&str] = &["target", "build", "dist", ".next", "node_modules"];
/// A directory's device and inode, as the walk compares them.
pub type DirId = (u64, u64);
