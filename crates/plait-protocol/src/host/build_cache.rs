use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    BuildCacheStatus,
    ClearBuildCaches,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub schema: u32,
    pub total_bytes: u64,
    pub idle_bytes: u64,
    pub thread_count: u64,
    pub active_count: u64,
    pub cap_bytes: u64,
    #[serde(default)]
    pub measuring: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearReply {
    pub schema: u32,
    pub cleared_threads: u64,
    pub cleared_bytes: u64,
    pub skipped_active: u64,
    pub total_bytes: u64,
    pub idle_bytes: u64,
    pub thread_count: u64,
    pub active_count: u64,
    pub cap_bytes: u64,
    #[serde(default)]
    pub measuring: bool,
}

impl ClearReply {
    pub fn status(&self) -> Status {
        Status {
            schema: self.schema,
            total_bytes: self.total_bytes,
            idle_bytes: self.idle_bytes,
            thread_count: self.thread_count,
            active_count: self.active_count,
            cap_bytes: self.cap_bytes,
            measuring: self.measuring,
        }
    }
}
