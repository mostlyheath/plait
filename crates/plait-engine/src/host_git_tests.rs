use super::*;
use crate::git::{Branches, FileList, LineCount};
use plait_protocol::{
    file_view::Stamp,
    git::{WriteOp, log::History},
    host::git::*,
    model::{Diff, DiffMode, Snapshot},
};
use serde::de::DeserializeOwned;
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
#[derive(Debug)]
enum Error {
    Protocol(String),
}
#[derive(Debug)]
struct Remote<T> {
    identity: DirIdentity,
    result: T,
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<Remote<T>, Error> {
    match serde_json::from_slice::<Outcome<T>>(bytes).unwrap() {
        Outcome::Ok { identity, result } => Ok(Remote {
            identity: identity.parse().unwrap(),
            result,
        }),
        Outcome::Error { message } => Err(Error::Protocol(message)),
    }
}
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("plait-git-read-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir.canonicalize().unwrap()
}

fn repository(name: &str) -> PathBuf {
    let dir = scratch(name).join("repo");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=Plait",
                "-c",
                "user.email=plait@example.invalid",
            ])
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("src/lib.rs"), "fn one() {}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "First"]);
    std::fs::write(dir.join("src/lib.rs"), "fn one() {}\nfn two() {}\n").unwrap();
    std::fs::write(dir.join("new.txt"), "hello\n").unwrap();
    dir
}

fn run<T: DeserializeOwned>(
    checkout: &Path,
    identity: Option<WireIdentity>,
    read: Read,
) -> Result<Remote<T>, Error> {
    let input = HelperInput {
        checkout: checkout.into(),
        store: None,
        identity,
        read,
    };
    decode(&execute(&input))
}

fn local_identity(path: &Path) -> DirIdentity {
    let meta = std::fs::metadata(path).unwrap();
    DirIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    }
}

#[test]
fn reads_match_the_local_reads() {
    let repo = repository("match");
    let local = git::status(&repo).unwrap();
    let remote: Remote<Snapshot> = run(&repo, None, Read::Status).unwrap();
    assert_eq!(remote.identity, local_identity(&repo));
    assert_eq!(remote.result.root, local.root);
    assert_eq!(remote.result.branch, local.branch);
    let paths = |s: &Snapshot| s.changes.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&remote.result), paths(&local));

    let change = local
        .changes
        .iter()
        .find(|c| c.path.ends_with("lib.rs"))
        .unwrap();
    let here = git::diff(&repo, change, DiffMode::Unstaged).unwrap();
    let there: Remote<Diff> = run(
        &repo,
        None,
        Read::Diff {
            change: change.into(),
            mode: DiffMode::Unstaged,
        },
    )
    .unwrap();
    assert_eq!(there.result.fingerprint(), here.fingerprint());
    assert_eq!(there.result.rows, here.rows);
    assert_eq!(there.result.hunks, here.hunks);

    let history: Remote<History> = run(
        &repo,
        None,
        Read::Log {
            tip: None,
            skip: 0,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(history.result, git::log(&repo, None, 0, 10).unwrap());
    let files: Remote<FileList> = run(&repo, None, Read::Files).unwrap();
    assert_eq!(files.result.paths, git::files(&repo).unwrap().paths);
    let branches: Remote<Branches> = run(&repo, None, Read::Branches).unwrap();
    assert_eq!(branches.result, git::branches(&repo).unwrap());
    let paths = vec![PathBuf::from("src/lib.rs"), PathBuf::from("new.txt")];
    let counts: Remote<Vec<LineCount>> = run(
        &repo,
        None,
        Read::LineCounts {
            paths: paths.clone(),
        },
    )
    .unwrap();
    assert_eq!(counts.result, git::line_counts(&repo, &paths).unwrap());
    let stamp: Remote<Stamp> = run(
        &repo,
        None,
        Read::FileStamp {
            path: "new.txt".into(),
        },
    )
    .unwrap();
    assert_eq!(
        stamp.result,
        file_view::stamp(&repo, Path::new("new.txt")).unwrap()
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[test]
fn the_checkout_must_be_the_expected_top_folder() {
    let repo = repository("checks");
    let message = |result: Result<Remote<Snapshot>, Error>| match result.unwrap_err() {
        Error::Protocol(message) => message,
    };
    assert!(message(run(&repo.join("src"), None, Read::Status)).contains("top folder"));
    assert!(message(run(Path::new("relative"), None, Read::Status)).contains("absolute"));
    let wrong = WireIdentity {
        dev: "1".into(),
        ino: "2".into(),
    };
    assert_eq!(
        message(run(&repo, Some(wrong), Read::Status)),
        "Checkout identity changed."
    );
    let right = Some(local_identity(&repo).into());
    assert!(run::<Snapshot>(&repo, right, Read::Status).is_ok());
    // A folder replaced at the same path is a different checkout.
    let id = Some(local_identity(&repo).into());
    std::fs::rename(&repo, repo.with_extension("old")).unwrap();
    std::fs::create_dir(&repo).unwrap();
    assert_eq!(
        message(run(&repo, id, Read::Status)),
        "Checkout identity changed."
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[test]
fn non_utf8_names_survive_a_remote_status() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let repo = repository("bytes");
    let name = PathBuf::from(OsString::from_vec(b"caf\xe9.txt".to_vec()));
    // Some file systems refuse such names; there is nothing to check there.
    if std::fs::write(repo.join(&name), "x").is_err() {
        return;
    }
    let remote: Remote<Snapshot> = run(&repo, None, Read::Status).unwrap();
    assert!(remote.result.changes.iter().any(|c| c.path == name));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

fn write_here(repo: &Path, identity: DirIdentity, op: WriteOp) -> Result<WriteOutcome, String> {
    let input = WriteInput {
        checkout: repo.into(),
        identity: identity.into(),
        write: op,
    };
    match serde_json::from_slice::<Outcome<WriteOutcome>>(&execute_write(&input)).unwrap() {
        Outcome::Ok { result, .. } => Ok(result),
        Outcome::Error { message } => Err(message),
    }
}

#[test]
fn writes_run_on_the_host_checkout_with_its_identity() {
    let repo = repository("write");
    let id = local_identity(&repo);
    write_here(&repo, id, WriteOp::Stage(vec!["new.txt".into()])).unwrap();
    let staged = git::status(&repo).unwrap();
    assert!(
        staged
            .changes
            .iter()
            .any(|c| c.path == Path::new("new.txt") && c.staged())
    );

    // A discard saves the file in the repository, so restoring it works here too.
    let discarded = write_here(
        &repo,
        id,
        WriteOp::Discard {
            path: "src/lib.rs".into(),
            untracked: false,
            hunk: None,
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("src/lib.rs")).unwrap(),
        "fn one() {}\n"
    );
    write_here(&repo, id, WriteOp::Restore(discarded.backup.unwrap())).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("src/lib.rs")).unwrap(),
        "fn one() {}\nfn two() {}\n"
    );

    let wrong = DirIdentity {
        dev: id.dev,
        ino: id.ino.wrapping_add(1),
    };
    assert_eq!(
        write_here(&repo, wrong, WriteOp::StageAll).unwrap_err(),
        "Checkout identity changed."
    );
    let sub = local_identity(&repo.join("src"));
    let refused = write_here(&repo.join("src"), sub, WriteOp::StageAll).unwrap_err();
    assert!(refused.contains("top folder"), "{refused}");
    let status = git::status(&repo).unwrap();
    assert!(
        !status
            .changes
            .iter()
            .any(|c| c.path == Path::new("src/lib.rs") && c.staged()),
        "a refused write staged something"
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[test]
fn a_worktree_is_measured_with_the_hosts_own_store() {
    let repo = repository("size");
    let store = repo.parent().unwrap().join("store");
    let cache = store.join("build-cache").join("thr_1");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("out.o"), vec![0u8; 4096]).unwrap();
    let id = local_identity(&repo);
    let size = |thread: Option<&str>, removed: bool, store: Option<PathBuf>| {
        let input = HelperInput {
            checkout: repo.clone(),
            store,
            identity: None,
            read: Read::WorktreeSize {
                worktree: repo.clone(),
                worktree_identity: Some(id.into()),
                thread: thread.map(str::to_owned),
                removed,
                others: Vec::new(),
                build_directories: Vec::new(),
            },
        };
        decode::<crate::worktree_size::Size>(&execute(&input)).map(|r| r.result)
    };
    let here = size(Some("thr_1"), false, Some(store.clone())).unwrap();
    assert!(here.source > 0 && here.build >= 4096, "{here:?}");
    let retained = size(Some("thr_1"), true, Some(store.clone())).unwrap();
    assert_eq!((retained.source, retained.build >= 4096), (0, true));
    let message = |r: Result<_, Error>| match r {
        Err(Error::Protocol(m)) => m,
        Ok(value) => panic!("{value:?}"),
    };
    assert_eq!(
        message(size(Some("../thr_1"), true, Some(store.clone()))),
        "Invalid thread."
    );
    assert!(message(size(None, false, None)).contains("store"));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
