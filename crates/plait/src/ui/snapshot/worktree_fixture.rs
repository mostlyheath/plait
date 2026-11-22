//! Minimal scripted-host worktree fixtures, created with Git.
use super::*;
use serde::Serialize;
use std::os::unix::fs::MetadataExt;

#[derive(Clone, Serialize)]
pub(super) struct Identity {
    pub dev: String,
    pub ino: String,
}
#[derive(Clone, Serialize)]
pub(super) struct Parent {
    pub root: PathBuf,
    pub identity: Identity,
    pub main: PathBuf,
    pub main_identity: Identity,
    pub common: PathBuf,
    pub common_identity: Identity,
    pub base: String,
}
#[derive(Serialize)]
pub(super) struct Tree {
    pub thread_id: String,
    pub parent: Parent,
    pub path: PathBuf,
    pub branch: String,
    pub identity: Option<Identity>,
    pub stage: &'static str,
    pub error: Option<String>,
}
fn identity(path: &Path) -> Identity {
    let metadata = std::fs::metadata(path).unwrap();
    Identity {
        dev: metadata.dev().to_string(),
        ino: metadata.ino().to_string(),
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
pub(super) fn parent(root: &Path) -> Parent {
    let root = root.canonicalize().unwrap();
    let common = root
        .join(git(&root, &["rev-parse", "--git-common-dir"]))
        .canonicalize()
        .unwrap();
    Parent {
        identity: identity(&root),
        main_identity: identity(&root),
        common_identity: identity(&common),
        base: git(&root, &["rev-parse", "HEAD^{commit}"]),
        main: root.clone(),
        root,
        common,
    }
}
pub(super) fn create(parent: &Parent, id: &str) -> Tree {
    let hex = id.replace('-', "");
    let suffix = &hex[..12];
    let mut name = parent.main.file_name().unwrap().to_owned();
    name.push(".worktrees");
    let base = parent.main.parent().unwrap().join(name);
    std::fs::create_dir_all(&base).unwrap();
    let path = base.join(suffix);
    let branch = format!("plait/{suffix}");
    git(
        &parent.root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            path.to_str().unwrap(),
            &parent.base,
        ],
    );
    Tree {
        thread_id: id.into(),
        parent: parent.clone(),
        identity: Some(identity(&path)),
        path,
        branch,
        stage: "ready",
        error: None,
    }
}
