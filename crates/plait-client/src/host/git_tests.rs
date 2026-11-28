use super::*;
use crate::git;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
};

fn scratch(name: &str) -> PathBuf {
    let dir = plait_test_support::socket_temp_dir()
        .join(format!("plait-git-read-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir.canonicalize().unwrap()
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
            guard: None,
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

#[test]
fn checkout_identity_reads_use_wire_paths_and_expected_identity() {
    for (read, name) in [
        (Read::Identity, "identity"),
        (Read::MainCheckout, "main_checkout"),
    ] {
        let request = Request::GitRead {
            checkout: "/repo".into(),
            identity: Some(DirIdentity { dev: 7, ino: 9 }.into()),
            read,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "op": "git_read", "checkout": "L3JlcG8=", "identity": {"dev": "7", "ino": "9"}, "read": name
            })
        );
    }
    let read: Remote<CheckoutIdentity> = decode(br#"{"ok":{"identity":{"dev":"7","ino":"9"},"result":{"root":"L3JlcG8=","identity":{"dev":"7","ino":"9"}}}}"#).unwrap();
    assert_eq!(read.result.root, PathBuf::from("/repo"));
    assert_eq!(read.result.identity.parse().unwrap(), read.identity);
}
