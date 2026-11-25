use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HostIdentity {
    pub id: String,
    pub name: String,
    pub platform: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub path: PathBuf,
    pub hidden: bool,
    #[serde(default)]
    pub vcs: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<super::git::Fingerprint>,
    #[serde(default)]
    pub branch: Option<super::git::BranchSummary>,
    #[serde(default)]
    pub main_checkout: Option<PathBuf>,
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
    /// Counts provider registries the host has published; a change means the model catalog
    /// changed. Hosts before it send none.
    #[serde(default)]
    pub providers_revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const VIEW: &str = r#"{"schema":1,"host":{"id":"h","name":"workstation","platform":"linux"},"revision":2,"projects":[{"path":"/srv/a","hidden":false}],"worktrees":[],"seen":[]}"#;

    #[test]
    fn project_summary_decodes_and_accepts_future_vcs() {
        let mut value: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        value["projects"][0] = serde_json::json!({"path":"/srv/a","hidden":false,"vcs":"future", "fingerprint":{"roots":["abc"],"remote":null},"branch":{"branch":"main","named":true,"tracking":null},"main_checkout":"/srv/main"});
        let view: View = serde_json::from_value(value).unwrap();
        let project = &view.projects[0];
        assert_eq!(project.vcs.as_deref(), Some("future"));
        assert_eq!(project.fingerprint.as_ref().unwrap().roots, ["abc"]);
        assert_eq!(project.branch.as_ref().unwrap().branch, "main");
        assert_eq!(
            project.main_checkout.as_deref(),
            Some(Path::new("/srv/main"))
        );
    }

    #[test]
    fn status_is_optional_and_parses_paths() {
        let old: View = serde_json::from_str(VIEW).unwrap();
        assert!(old.status.is_empty());
        assert_eq!(old.projects[0].vcs, None);
        assert_eq!(old.projects[0].fingerprint, None);
        let mut value: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        value["status"] = serde_json::json!({"/srv/a": 7});
        let view: View = serde_json::from_value(value).unwrap();
        assert_eq!(view.status.get(Path::new("/srv/a")), Some(&7));
    }

    #[test]
    fn providers_revision_is_optional() {
        let old: View = serde_json::from_str(VIEW).unwrap();
        assert_eq!(old.providers_revision, 0);
        let mut value: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        value["providers_revision"] = serde_json::json!(3);
        let view: View = serde_json::from_value(value).unwrap();
        assert_eq!(view.providers_revision, 3);
    }
}
