#![cfg(unix)]

use plait_core::{
    file_view::Contents,
    host::{Client, git},
};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Host(Child);
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn client_reads_file_and_observes_changed_stamp_through_host() {
    let base = std::env::temp_dir().join(format!("plait-interop-file-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let store = base.join("store");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("notes.txt"), b"before\n").unwrap();
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/test/fixture.ts")
        .canonicalize()
        .unwrap();
    let _host = Host(
        Command::new(env!("CARGO_BIN_EXE_plait-host"))
            .args(["run", "--store"])
            .arg(&store)
            .arg("--runtime")
            .arg(runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // This integration test is the only test in its process and owns this environment.
    unsafe { std::env::set_var("PLAIT_HOST_STORE", &store) };
    let client = Client::from_environment().unwrap();
    let path = Path::new("notes.txt");
    let deadline = Instant::now() + Duration::from_secs(15);
    let first = loop {
        match git::file(&client, &repo, None, path) {
            Ok(file) => break file,
            Err(error) => {
                assert!(Instant::now() < deadline, "host did not answer: {error}");
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    };
    match &first.result.contents {
        Contents::Text(text) => assert_eq!(text.text.as_bytes(), b"before\n"),
        other => panic!("unexpected contents: {other:?}"),
    }
    let stamp = git::file_stamp(&client, &repo, Some(first.identity), path).unwrap();
    assert_eq!(stamp.result, first.result.stamp);
    fs::write(repo.join(path), b"after, changed\n").unwrap();
    let changed = git::file(&client, &repo, Some(first.identity), path).unwrap();
    assert_ne!(changed.result.stamp, first.result.stamp);
    assert_eq!(
        git::file_stamp(&client, &repo, Some(first.identity), path)
            .unwrap()
            .result,
        changed.result.stamp
    );
    match changed.result.contents {
        Contents::Text(text) => assert_eq!(text.text.as_bytes(), b"after, changed\n"),
        other => panic!("unexpected contents: {other:?}"),
    }
    drop(_host);
    fs::remove_dir_all(base).unwrap();
}
