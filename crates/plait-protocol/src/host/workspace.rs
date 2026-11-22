use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HostIdentity {
    pub id: String,
    pub name: String,
    pub platform: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub path: PathBuf,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub thread_id: String,
    pub updated_at: u64,
    pub state: String,
    pub unread: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub folder: PathBuf,
    pub main: PathBuf,
    pub branch: String,
    pub creation_id: Option<String>,
    pub identity: Option<crate::git::worktree::Identity>,
}

/// One consistent view of a host's workspace.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct View {
    pub schema: u32,
    pub host: HostIdentity,
    pub revision: u64,
    #[serde(default)]
    pub status: BTreeMap<PathBuf, u64>,
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub seen: Vec<Seen>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const VIEW: &str = r#"{"schema":1,"host":{"id":"h","name":"workstation","platform":"linux"},"revision":2,"projects":[{"path":"/srv/a","hidden":false}],"worktrees":[],"seen":[]}"#;

    #[test]
    fn status_is_optional_and_parses_paths() {
        let old: View = serde_json::from_str(VIEW).unwrap();
        assert!(old.status.is_empty());
        let mut value: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        value["status"] = serde_json::json!({"/srv/a": 7});
        let view: View = serde_json::from_value(value).unwrap();
        assert_eq!(view.status.get(Path::new("/srv/a")), Some(&7));
    }
}
