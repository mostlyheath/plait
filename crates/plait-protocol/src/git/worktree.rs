use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub dev: String,
    pub ino: String,
}

use std::path::PathBuf;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Parent {
    pub root: PathBuf,
    pub identity: Identity,
    pub main: PathBuf,
    pub main_identity: Identity,
    pub common: PathBuf,
    pub common_identity: Identity,
    pub base: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedStage {
    Reserved,
    Creating,
    Ready,
    Failed,
    Removing,
    Removed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedWorktree {
    pub thread_id: String,
    pub parent: Parent,
    pub path: PathBuf,
    pub branch: String,
    pub identity: Option<Identity>,
    pub stage: ManagedStage,
    pub error: Option<String>,
}

pub mod inventory {
    use serde::{Deserialize, Serialize};
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(deny_unknown_fields)]
    pub struct Preservation {
        pub changed: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub build_output: Option<u64>,
        pub unpushed: Option<u64>,
        pub exclusive: Option<u64>,
        pub refusal: Option<String>,
        /// The same checks with the person's explicit publication exception.
        #[serde(default)]
        pub can_keep_local: bool,
    }
    impl Preservation {
        pub fn require_safe(&self) -> Result<(), String> {
            self.refusal.clone().map_or(Ok(()), Err)
        }
    }
}
