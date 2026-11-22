use super::*;
use crate::git::WriteOutcome;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    process::Command,
};

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
fn additive_request_has_no_version_and_reaches_the_helper_intact() {
    let request = Request::GitRead {
        checkout: "/repo".into(),
        identity: None,
        read: Read::Status,
    };
    assert_eq!(
        serde_json::to_string(&request).unwrap(),
        r#"{"op":"git_read","checkout":"L3JlcG8=","identity":null,"read":"status"}"#
    );
    let read = Read::Diff {
        change: WireChange {
            path: "a b.rs".into(),
            original: Some("old.rs".into()),
            index: b'R',
            worktree: b' ',
        },
        mode: DiffMode::Staged,
    };
    let mut value = serde_json::to_value(Request::GitRead {
        checkout: "/repo".into(),
        identity: Some(WireIdentity {
            dev: "1".into(),
            ino: "18446744073709551615".into(),
        }),
        read: read.clone(),
    })
    .unwrap();
    // The runtime passes everything but `op` to the helper.
    value.as_object_mut().unwrap().remove("op");
    let input: HelperInput = serde_json::from_value(value).unwrap();
    assert_eq!(input.read, read);
    assert_eq!(input.identity.unwrap().parse().unwrap().ino, u64::MAX);
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
fn file_bytes_classify_as_the_local_viewer_does() {
    let repo = repository("file");
    let remote: Remote<FileBytes> = run(
        &repo,
        None,
        Read::File {
            path: "src/lib.rs".into(),
        },
    )
    .unwrap();
    let bytes = remote.result.base64.map(|b| STANDARD.decode(b).unwrap());
    let opened = file_view::classify(remote.result.stamp, bytes, Path::new("src/lib.rs"));
    assert_eq!(
        opened,
        file_view::read(&repo, Path::new("src/lib.rs")).unwrap()
    );
    std::os::unix::fs::symlink("src/lib.rs", repo.join("link")).unwrap();
    let error = run::<FileBytes>(
        &repo,
        None,
        Read::File {
            path: "link".into(),
        },
    )
    .unwrap_err();
    assert!(
        matches!(error, Error::Protocol(_)),
        "a symlink was followed"
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[test]
fn the_checkout_must_be_the_expected_top_folder() {
    let repo = repository("checks");
    let message = |result: Result<Remote<Snapshot>, Error>| match result.unwrap_err() {
        Error::Protocol(message) => message,
        other => panic!("{other:?}"),
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

fn page(total: usize, data: &[u8], token: Option<&str>) -> Page {
    Page {
        total,
        data: STANDARD.encode(data),
        token: token.map(str::to_owned),
    }
}

#[test]
fn pages_reassemble_exactly_and_refuse_disagreement() {
    let whole: Vec<u8> = (0..2500u32).map(|n| n as u8).collect();
    let mut asked = Vec::new();
    let bytes = assemble(
        page(whole.len(), &whole[..1000], Some("t")),
        |token, offset| {
            asked.push((token.to_owned(), offset));
            let end = (offset + 1000).min(whole.len());
            Ok(page(
                whole.len(),
                &whole[offset..end],
                (end < whole.len()).then_some("t"),
            ))
        },
    )
    .unwrap();
    assert_eq!(bytes, whole);
    assert_eq!(asked, vec![("t".into(), 1000), ("t".into(), 2000)]);

    let no_token = assemble(page(10, b"12345", None), |_, _| unreachable!());
    assert!(matches!(no_token, Err(Error::Protocol(m)) if m.contains("early")));
    let changed = assemble(page(10, b"12345", Some("t")), |_, _| {
        Ok(page(11, b"678901", None))
    });
    assert!(matches!(changed, Err(Error::Protocol(m)) if m.contains("agree")));
    let overlong = assemble(page(4, b"12345", None), |_, _| unreachable!());
    assert!(matches!(overlong, Err(Error::Protocol(m)) if m.contains("agree")));
    let empty = assemble(page(4, b"", Some("t")), |_, _| unreachable!());
    assert!(matches!(empty, Err(Error::Protocol(m)) if m.contains("empty")));
    let huge = assemble(
        page(RESULT_LIMIT + 1, b"1", Some("t")),
        |_, _| unreachable!(),
    );
    assert!(matches!(huge, Err(Error::Protocol(m)) if m.contains("too large")));
}

#[test]
fn an_older_host_is_named_and_pages_are_requested_through_the_client() {
    let dir = scratch("client");
    let path = dir.join("host.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let whole = serde_json::to_vec(&Outcome::Ok {
        identity: WireIdentity {
            dev: "7".into(),
            ino: "9".into(),
        },
        result: "x".repeat(3000),
    })
    .unwrap();
    let server = {
        let whole = whole.clone();
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..4 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                seen.push(request.clone());
                let reply = match request["op"].as_str().unwrap() {
                    _ if seen.len() == 1 => r#"{"ok":false,"error":{"code":"invalid_request","message":"Unknown operation or field."}}"#.to_string(),
                    "git_read" => format!(
                        r#"{{"ok":true,"result":{{"total":{},"data":"{}","token":"t"}}}}"#,
                        whole.len(),
                        STANDARD.encode(&whole[..1500])
                    ),
                    _ => {
                        let offset = request["offset"].as_u64().unwrap() as usize;
                        let end = (offset + 1500).min(whole.len());
                        let token = if end < whole.len() { r#""t""# } else { "null" };
                        format!(
                            r#"{{"ok":true,"result":{{"total":{},"data":"{}","token":{token}}}}}"#,
                            whole.len(),
                            STANDARD.encode(&whole[offset..end])
                        )
                    }
                };
                writeln!(socket, "{reply}").unwrap();
            }
            seen
        })
    };
    let client = Client {
        transport: super::super::Transport::Local(path),
    };
    let old = read::<String>(&client, Path::new("/repo"), None, Read::Status).unwrap_err();
    assert!(matches!(&old, Error::Protocol(m) if m == OLDER_HOST));
    let remote = read::<String>(&client, Path::new("/repo"), None, Read::Branches).unwrap();
    assert_eq!(remote.result, "x".repeat(3000));
    assert_eq!(remote.identity, DirIdentity { dev: 7, ino: 9 });
    let seen = server.join().unwrap();
    assert_eq!(seen[1]["read"], "branches");
    assert!(seen[1].get("protocol_version").is_none());
    assert_eq!(seen[2]["op"], "git_read_page");
    assert_eq!(seen[2]["offset"], 1500);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_write_survives_the_wire() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let odd = PathBuf::from(OsString::from_vec(b"caf\xe9.rs".to_vec()));
    for op in [
        WriteOp::Stage(vec![odd.clone(), "b.rs".into()]),
        WriteOp::Unstage(vec![odd.clone()]),
        WriteOp::StageAll,
        WriteOp::UnstageAll,
        WriteOp::Commit {
            message: "Fix the parser\n\nBody".into(),
        },
        WriteOp::Push,
        WriteOp::Publish,
        WriteOp::Fetch,
        WriteOp::Pull,
        WriteOp::Hunk {
            path: odd.clone(),
            from: DiffMode::Unstaged,
            index: 2,
            header: "@@ -1 +1 @@".into(),
        },
        WriteOp::Discard {
            path: odd.clone(),
            untracked: false,
            hunk: Some((0, "@@ -1 +1 @@".into())),
        },
        WriteOp::Restore(git::Backup {
            path: odd.clone(),
            blob: "a".repeat(40),
            executable: true,
        }),
        WriteOp::CreateBranch("topic".into()),
        WriteOp::SwitchBranch("main".into()),
    ] {
        let mut value = serde_json::to_value(Request::GitWrite {
            op_id: None,
            checkout: "/repo".into(),
            identity: WireIdentity {
                dev: "1".into(),
                ino: "2".into(),
            },
            write: op.clone(),
        })
        .unwrap();
        assert_eq!(value["op"], "git_write");
        value.as_object_mut().unwrap().remove("op");
        let input: WriteInput = serde_json::from_value(value).unwrap();
        assert_eq!(input.write, op);
    }
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
        other => panic!("{other:?}"),
    };
    assert_eq!(
        message(size(Some("../thr_1"), true, Some(store.clone()))),
        "Invalid thread."
    );
    assert!(message(size(None, false, None)).contains("store"));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[test]
fn write_request_operation_id_is_optional() {
    for id in [None, Some("retry_1".to_owned())] {
        let value = serde_json::to_value(Request::GitWrite {
            op_id: id.clone(),
            checkout: "/repo".into(),
            identity: WireIdentity {
                dev: "1".into(),
                ino: "2".into(),
            },
            write: WriteOp::CreateBranch("topic".into()),
        })
        .unwrap();
        assert_eq!(value.get("op_id").and_then(|v| v.as_str()), id.as_deref());
    }
}
