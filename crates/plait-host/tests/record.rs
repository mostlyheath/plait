//! `plait-host record` and `prune` as the runtime calls them: the real binary,
//! contained by its sandbox on macOS, against real repositories.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

#[test]
fn record_and_prune_descendants_release_both_leases_when_the_helper_dies() {
    use std::os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, process::CommandExt},
    };
    use std::time::{Duration, Instant};
    for operation in ["record", "prune"] {
        for termination in ["helper", "runtime", "worker"] {
            let base = Scratch::new();
            let root = repository(&base.0);
            let store = base.0.join("store");
            let request = json!({"checkout":root,"store":store,"id":"seed"});
            assert!(helper("record", &request).0);
            let marker = store.join("records/descendants");
            let wrapper = base.0.join("git-wrapper");
            let real = plait_engine::git::record::git_binary();
            fs::write(&wrapper, format!("#!/bin/sh\nif [ -n \"$GIT_DIR\" ]; then\n sleep 30 &\n echo \"$$ $!\" > '{}'\n wait\nfi\nexec '{}' \"$@\"\n", marker.display(), real.display())).unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
            let store_lock = fs::File::create(store.join("test.lock")).unwrap();
            let checkout_lock = fs::File::open(&root).unwrap();
            for file in [&store_lock, &checkout_lock] {
                assert_eq!(
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                    0
                );
            }
            let mut command = Command::new(env!("CARGO_BIN_EXE_plait-host"));
            command
                .arg(operation)
                .env("PLAIT_GIT", &wrapper)
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let (store_fd, checkout_fd) = (store_lock.as_raw_fd(), checkout_lock.as_raw_fd());
            unsafe {
                command.pre_exec(move || {
                    let a = libc::fcntl(store_fd, libc::F_DUPFD_CLOEXEC, 10);
                    let b = libc::fcntl(checkout_fd, libc::F_DUPFD_CLOEXEC, 10);
                    if a < 0 || b < 0 || libc::dup2(a, 3) < 0 || libc::dup2(b, 4) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn().unwrap();
            let request = if operation == "prune" {
                json!({"store":store,"remove":["seed"]})
            } else {
                request
            };
            child
                .stdin
                .take()
                .unwrap()
                .write_all(request.to_string().as_bytes())
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !marker.exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            let pids: Vec<i32> = fs::read_to_string(&marker)
                .unwrap()
                .split_whitespace()
                .map(|p| p.parse().unwrap())
                .collect();
            if termination == "runtime" {
                unsafe {
                    libc::killpg(child.id() as i32, libc::SIGKILL);
                }
            } else if termination == "worker" {
                unsafe {
                    libc::kill(libc::getpgid(pids[0]), libc::SIGKILL);
                }
            } else {
                child.kill().unwrap();
            }
            child.wait().unwrap();
            drop(store_lock);
            drop(checkout_lock);
            for path in [&store.join("test.lock"), &root] {
                let file = fs::File::open(path).unwrap();
                while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                    assert!(
                        Instant::now() < deadline,
                        "{operation} leaked a writer lease"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            for pid in pids {
                let status = Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output()
                    .unwrap();
                let status = String::from_utf8_lossy(&status.stdout);
                assert!(
                    status.trim().is_empty() || status.trim().starts_with('Z'),
                    "descendant still running: {pid} {status}"
                );
            }
        }
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        isolate_leases();
        let path = std::env::temp_dir().join(format!(
            "plait-host-records-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
/// Lock files are never removed where Plait runs, so these tests keep theirs out
/// of the account's lease directory; the helpers they start inherit the setting.
fn isolate_leases() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let uid = unsafe { libc::geteuid() };
        let directory = std::env::temp_dir().join(format!("plait-host-test-leases-{uid}"));
        // SAFETY: set once, before any test here takes a lease (each makes a Scratch
        // first), and the standard library orders its own environment reads after it.
        unsafe { std::env::set_var("PLAIT_LEASE_DIR", directory) };
    });
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Git starts detached maintenance after a commit or a push. It briefly holds
/// a lock under `.git/objects`, which a before-and-after comparison of a
/// repository can catch. Fixtures turn it off, as the recorder's own commands do.
const NO_MAINTENANCE: [(&str, &str); 7] = [
    ("GIT_CONFIG_COUNT", "3"),
    ("GIT_CONFIG_KEY_0", "maintenance.auto"),
    ("GIT_CONFIG_VALUE_0", "false"),
    ("GIT_CONFIG_KEY_1", "gc.auto"),
    ("GIT_CONFIG_VALUE_1", "0"),
    ("GIT_CONFIG_KEY_2", "receive.autoGC"),
    ("GIT_CONFIG_VALUE_2", "false"),
];

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(NO_MAINTENANCE)
        .status()
        .unwrap();
    assert!(status.success());
}

fn repository(base: &Path) -> PathBuf {
    let root = base.join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("notes.txt"), "before\n").unwrap();
    fs::write(root.join(".gitignore"), "*.env\n").unwrap();
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e.invalid",
            "commit",
            "-qm",
            "x",
        ],
    );
    fs::write(root.join("local.env"), "SECRET=1\n").unwrap();
    root
}

fn helper(operation: &str, request: &Value) -> (bool, Value, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plait-host"))
        .arg(operation)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let reply = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (
        out.status.success(),
        reply,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

type Stat = (bool, u64, u32, i64, i64, i64, i64, u64, Option<Vec<u8>>);

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Stat> {
    let mut all = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = fs::symlink_metadata(&path).unwrap();
        all.insert(
            path.clone(),
            (
                meta.is_dir(),
                meta.len(),
                meta.mode(),
                meta.mtime(),
                meta.mtime_nsec(),
                meta.ctime(),
                meta.ctime_nsec(),
                meta.ino(),
                if meta.is_file() {
                    Some(fs::read(&path).unwrap())
                } else if meta.file_type().is_symlink() {
                    Some(
                        fs::read_link(&path)
                            .unwrap()
                            .as_os_str()
                            .as_bytes()
                            .to_vec(),
                    )
                } else {
                    None
                },
            ),
        );
        if meta.is_dir() {
            pending.extend(fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        }
    }
    all
}

#[test]
fn records_and_prunes_without_writing_to_the_repository() {
    let base = Scratch::new();
    let root = repository(&base.0);
    let store = base.0.join("store");
    fs::create_dir(&store).unwrap();
    let before_snapshot = snapshot(&root);
    let (ok, before, error) = helper(
        "record",
        &json!({"checkout": root, "store": store, "id": "before-1"}),
    );
    assert!(ok, "{error}");
    assert_eq!(before["head_ref"], "refs/heads/main");
    assert!(before["paths"].is_null());
    assert_eq!(
        snapshot(&root),
        before_snapshot,
        "a record wrote to the repository"
    );
    // What a run changes, untouched by the recorder on the way.
    fs::write(root.join("notes.txt"), "after\n").unwrap();
    fs::write(root.join("local.env"), "SECRET=2\n").unwrap();
    fs::write(root.join("new.txt"), "new\n").unwrap();
    let changed = snapshot(&root);
    let (ok, after, error) = helper(
        "record",
        &json!({"checkout": root, "store": store, "id": "after-1", "before": before["commit"]}),
    );
    assert!(ok, "{error}");
    let paths: Vec<&str> = after["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["local.env", "new.txt", "notes.txt"]);
    assert_eq!(after["paths"][2]["added"], 1);
    assert_eq!(after["paths"][2]["removed"], 1);
    assert_eq!(snapshot(&root), changed, "a record wrote to the repository");
    let git_before = snapshot(&root.join(".git"));
    let untouched_before = fs::read(root.join(".gitignore")).unwrap();
    let meta = fs::metadata(&root).unwrap();
    let (ok, undone, error) = helper(
        "undo",
        &json!({
            "checkout": root, "store": store, "before": before["commit"], "after": after["commit"],
            "before_id": "undo-before", "id": "undo-after", "identity": { "dev": meta.dev().to_string(), "ino": meta.ino().to_string() }
        }),
    );
    assert!(ok, "{error}");
    assert_eq!(undone["state"], "undone", "{undone}");
    assert_eq!(fs::read(root.join("notes.txt")).unwrap(), b"before\n");
    assert_eq!(fs::read(root.join("local.env")).unwrap(), b"SECRET=1\n");
    assert!(!root.join("new.txt").exists());
    assert_eq!(fs::read(root.join(".gitignore")).unwrap(), untouched_before);
    assert_eq!(
        snapshot(&root.join(".git")),
        git_before,
        "undo touched the person's Git metadata"
    );
    let changed = snapshot(&root);
    let (ok, pruned, error) = helper(
        "prune",
        &json!({"store": store, "remove": ["before-1", "after-1"]}),
    );
    assert!(ok, "{error}");
    assert_eq!(
        pruned,
        json!({"refs_removed": 2, "checkouts_removed": 0, "pruned": true})
    );
    assert_eq!(snapshot(&root), changed, "a prune wrote to the repository");
}

#[test]
fn a_record_that_cannot_be_made_fails_and_says_why() {
    let base = Scratch::new();
    let plain = base.0.join("plain");
    fs::create_dir(&plain).unwrap();
    let store = base.0.join("store");
    let (ok, _, error) = helper(
        "record",
        &json!({"checkout": plain, "store": store, "id": "x"}),
    );
    assert!(!ok);
    assert!(error.starts_with("plait-host: "), "{error}");
    for request in [
        json!({"checkout": plain, "store": "relative", "id": "x"}),
        json!({"checkout": plain}),
        json!("not an object"),
    ] {
        assert!(!helper("record", &request).0);
    }
    // Pruning a store that has never recorded does nothing.
    let (ok, pruned, error) = helper("prune", &json!({"store": store, "remove": ["x"]}));
    assert!(ok, "{error}");
    assert_eq!(pruned["pruned"], false);
}

/// A checkout of `files` small files in nested folders, committed.
fn checkout_of(base: &Path, files: usize) -> PathBuf {
    let root = base.join(format!("repo-{files}"));
    for n in 0..files {
        let dir = root.join(format!("src/d{}/m{}", n / 5000, n / 100));
        if n % 100 == 0 {
            fs::create_dir_all(&dir).unwrap();
        }
        fs::write(
            dir.join(format!("f{n}.rs")),
            format!("// file {n}\nfn f{n}() {{}}\n"),
        )
        .unwrap();
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e.invalid",
            "commit",
            "-qm",
            "x",
        ],
    );
    root
}

fn timed(request: &Value) -> (Value, std::time::Duration) {
    let start = std::time::Instant::now();
    let (ok, reply, error) = helper("record", request);
    assert!(ok, "{error}");
    (reply, start.elapsed())
}

/// What a record costs through the helper the runtime starts, sandbox and
/// all. Run by `scripts/bench-records.sh`.
#[test]
#[ignore = "timing run"]
fn host_record_timing() {
    let files: usize = std::env::var("PLAIT_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let base = Scratch::new();
    let store = base.0.join("store");
    for (size, label) in [(files, "large"), (200, "small")] {
        let root = checkout_of(&base.0, size);
        let request = |id: &str, before: Option<&Value>| json!({"checkout": root, "store": store, "id": id, "before": before});
        let (_, cold) = timed(&request(&format!("{label}-cold"), None));
        let mut warm: Vec<_> = (0..7)
            .map(|n| timed(&request(&format!("{label}-warm{n}"), None)).1)
            .collect();
        warm.sort();
        // A run that changes one file, after the first change has been recorded once.
        let run = |n: usize| {
            let (before, first) = timed(&request(&format!("{label}-run{n}-b"), None));
            fs::write(root.join("src/d0/m0/f0.rs"), format!("// run {n}\n")).unwrap();
            let (after, second) = timed(&request(
                &format!("{label}-run{n}-a"),
                Some(&before["commit"]),
            ));
            assert_eq!(after["paths"].as_array().unwrap().len(), 1);
            first + second
        };
        let first_run = run(1);
        let later_run = run(2);
        println!(
            "host {label}: files={size} cold_ms={} warm_median_ms={} warm_min_ms={} warm_max_ms={} first_run_ms={} later_run_ms={}",
            cold.as_millis(),
            warm[warm.len() / 2].as_millis(),
            warm[0].as_millis(),
            warm[warm.len() - 1].as_millis(),
            first_run.as_millis(),
            later_run.as_millis(),
        );
        if label == "large" {
            assert!(
                warm[warm.len() / 2] < std::time::Duration::from_millis(300),
                "a warm record took {:?}",
                warm[warm.len() / 2]
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn the_recorder_runs_inside_its_sandbox() {
    // The profile the host builds for this checkout stops any process under
    // it from writing the checkout or its Git directory, and leaves the store open.
    let base = Scratch::new();
    let root = repository(&base.0);
    let records = base.0.join("store/records");
    fs::create_dir_all(&records).unwrap();
    let located = plait_engine::git::record::locate(&root).unwrap();
    let profile = plait_engine::git::record::sandbox_profile(
        &plait_engine::git::record::protected_paths(&located),
        &records,
    )
    .unwrap();
    let write = |path: &Path| {
        Command::new("/usr/bin/sandbox-exec")
            .args(["-p", &profile, "/bin/sh", "-c", "printf x > \"$0\""])
            .arg(path)
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(!write(&root.join(".git/HEAD")));
    assert!(!write(&root.join("notes.txt")));
    assert!(write(&records.join("ok")));
    assert_eq!(
        fs::read_to_string(root.join(".git/HEAD")).unwrap(),
        "ref: refs/heads/main\n"
    );
}

#[test]
fn a_store_inside_the_checkout_is_refused_before_it_is_created() {
    let base = Scratch::new();
    let root = repository(&base.0);
    let before = snapshot(&root);
    for store in [root.join("store"), root.join(".git/private-store")] {
        let (ok, _, error) = helper(
            "record",
            &json!({"checkout": root, "store": store, "id": "refused"}),
        );
        assert!(!ok, "{error}");
        assert_eq!(snapshot(&root), before);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn the_real_helper_denies_git_timestamp_writes() {
    use std::os::unix::fs::PermissionsExt;
    let base = Scratch::new();
    let root = repository(&base.0);
    let store = base.0.join("store");
    fs::create_dir(&store).unwrap();
    let wrapper = base.0.join("git-probe");
    let real = plait_engine::git::record::git_binary();
    // Attempt the write only in private-store commands, after containment starts.
    fs::write(&wrapper, format!("#!/bin/sh\nif [ -n \"$GIT_DIR\" ]; then\n  if /usr/bin/touch '{}' 2>/dev/null; then exit 71; fi\n  /usr/bin/touch '{}/records/denied-write-observed' || exit 72\nfi\nexec '{}' \"$@\"\n", root.join(".git/index").display(), store.display(), real.display())).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let before = snapshot(&root);
    let mut child = Command::new(env!("CARGO_BIN_EXE_plait-host"))
        .arg("record")
        .env("PLAIT_GIT", &wrapper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({"checkout": root, "store": store, "id": "probe"})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(store.join("records/denied-write-observed").exists());
    assert_eq!(snapshot(&root), before);
    let recorded: Value = serde_json::from_slice(&out.stdout).unwrap();
    fs::write(root.join("notes.txt"), "changed by run\n").unwrap();
    let (ok, after, error) = helper(
        "record",
        &json!({"checkout":root,"store":store,"id":"probe-after","before":recorded["commit"]}),
    );
    assert!(ok, "{error}");
    fs::remove_file(store.join("records/denied-write-observed")).unwrap();
    let git_before = snapshot(&root.join(".git"));
    let meta = fs::metadata(&root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_plait-host"))
        .arg("undo")
        .env("PLAIT_GIT", &wrapper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json!({"checkout":root,"store":store,"before":recorded["commit"],"after":after["commit"],"before_id":"ub","id":"ua","identity":{"dev":meta.dev().to_string(),"ino":meta.ino().to_string()}}).to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let undone: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(undone["state"], "undone");
    assert!(store.join("records/denied-write-observed").exists());
    assert_eq!(snapshot(&root.join(".git")), git_before);
    assert_eq!(fs::read(root.join("notes.txt")).unwrap(), b"before\n");
}

#[test]
fn undo_refuses_hard_links_including_links_to_git_metadata() {
    // The command sandbox disallows creating hard links, even in scratch.
    // Exercise these fixtures at the real helper boundary, outside that sandbox.
    for git_link in [false, true] {
        let base = Scratch::new();
        let root = repository(&base.0);
        let store = base.0.join("store");
        fs::create_dir(&store).unwrap();
        let (ok, before, error) =
            helper("record", &json!({"checkout":root,"store":store,"id":"b"}));
        assert!(ok, "{error}");
        let target = root.join("notes.txt");
        if git_link {
            fs::remove_file(&target).unwrap();
            fs::hard_link(root.join(".git/description"), &target).unwrap();
        } else {
            fs::write(&target, "run").unwrap();
            fs::hard_link(&target, root.join("alias")).unwrap();
        }
        let (ok, after, error) = helper(
            "record",
            &json!({"checkout":root,"store":store,"id":"a","before":before["commit"]}),
        );
        assert!(ok, "{error}");
        let old = snapshot(&root);
        let meta = fs::metadata(&root).unwrap();
        let (ok, undo, error) = helper(
            "undo",
            &json!({"checkout":root,"store":store,"before":before["commit"],"after":after["commit"],"before_id":"ub","id":"ua","identity":{"dev":meta.dev().to_string(),"ino":meta.ino().to_string()}}),
        );
        assert!(ok, "{error}");
        assert_eq!(undo["state"], "blocked");
        assert_eq!(snapshot(&root), old);
    }
}

#[test]
fn distinct_hard_link_names_are_recorded_as_distinct_entries() {
    // The command sandbox deliberately denies hard-link creation. Construct
    // this fixture outside it, then exercise the real recorder's own sandbox.
    let base = Scratch::new();
    let root = repository(&base.0);
    let store = base.0.join("store");
    let name = "cafe\u{301}";
    fs::write(root.join(name), b"before\n").unwrap();
    fs::hard_link(root.join(name), root.join("other")).unwrap();
    let (ok, before, error) = helper(
        "record",
        &json!({"checkout":root,"store":store,"id":"before"}),
    );
    assert!(ok, "{error}");
    fs::write(root.join(name), b"after\n").unwrap();
    let (ok, after, error) = helper(
        "record",
        &json!({"checkout":root,"store":store,"id":"after","before":before["commit"]}),
    );
    assert!(ok, "{error}");
    let paths: Vec<_> = after["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, [name, "other"]);
}

#[test]
fn standalone_undo_respects_writer_reader_and_exclusive_leases() {
    use plait_engine::lease::{LeaseClass, acquire_checkout};
    let base = Scratch::new();
    let root = repository(&base.0);
    let store = base.0.join("store");
    fs::create_dir(&store).unwrap();
    let (ok, before, error) = helper("record", &json!({"checkout":root,"store":store,"id":"b"}));
    assert!(ok, "{error}");
    fs::write(root.join("notes.txt"), "after run\n").unwrap();
    let (ok, after, error) = helper(
        "record",
        &json!({"checkout":root,"store":store,"id":"a","before":before["commit"]}),
    );
    assert!(ok, "{error}");
    let meta = fs::metadata(&root).unwrap();
    let undo_req = |id: &str| {
        json!({
            "checkout": root,
            "store": store,
            "before": before["commit"],
            "after": after["commit"],
            "before_id": format!("ub-{id}"),
            "id": format!("ua-{id}"),
            "identity": {"dev": meta.dev().to_string(), "ino": meta.ino().to_string()}
        })
    };

    for (label, class) in [
        ("writer", LeaseClass::Writer),
        ("reader", LeaseClass::Reader),
        ("exclusive", LeaseClass::Exclusive),
    ] {
        let held = acquire_checkout(&root, None, class).unwrap();
        let (ok, _, stderr) = helper("undo", &undo_req(label));
        assert!(!ok, "undo should be refused while {label} holds lease");
        assert!(stderr.contains("busy"), "{label}: {stderr}");
        assert_eq!(
            fs::read_to_string(root.join("notes.txt")).unwrap(),
            "after run\n"
        );
        drop(held);
    }

    // Once all leases are released, standalone undo restores the file.
    let (ok, result, error) = helper("undo", &undo_req("free"));
    assert!(ok, "{error}");
    assert_eq!(result["state"], "undone");
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        "before\n"
    );
}

#[test]
fn crash_releases_leases_while_uncertain_descendant_keeps_them_held() {
    use plait_engine::lease::{
        LeaseClass, LeaseErrorKind, acquire_checkout, prepare_checkout_lock,
    };
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    use std::time::{Duration, Instant};

    let base = Scratch::new();
    let root = repository(&base.0);
    let prepared = prepare_checkout_lock(&root).unwrap();

    // Helper script: parent locks fd 3 (lifecycle) + fd 4 (writer) via plait-host lock-fd,
    // writes a ready marker, and either sleeps (plain crash) or spawns a detached descendant
    // that keeps fds 3 and 4 open after the parent is SIGKILLed.
    let host = env!("CARGO_BIN_EXE_plait-host");

    // Case 1: plain process crash releases BOTH writer and lifecycle leases immediately.
    let ready = base.0.join("plain-ready");
    let script = format!(
        "exec 3<>'{}' 4<'{}'\n'{}' lock-fd writer '{}' || exit 1\necho $$ > '{}'\nexec sleep 30\n",
        prepared.lock_path.display(),
        prepared.root.display(),
        host,
        prepared.root.display(),
        ready.display()
    );
    let mut child = Command::new("/bin/sh")
        .args(["-c", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "holder never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        acquire_checkout(&root, None, LeaseClass::Exclusive)
            .unwrap_err()
            .kind,
        LeaseErrorKind::CheckoutBusy
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let acquired = acquire_checkout(&root, None, LeaseClass::Exclusive).unwrap();
    drop(acquired);

    // Case 2: an uncertain descendant that inherited the lease FDs keeps both leases held
    // even after its parent crashes with SIGKILL, until that descendant itself dies.
    let child_pid_file = base.0.join("descendant-pid");
    let lifecycle = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&prepared.lock_path)
        .unwrap();
    let dir = fs::File::open(&prepared.root).unwrap();
    let mut lock_cmd = Command::new(host);
    lock_cmd.args(["lock-fd", "writer"]).arg(&prepared.root);
    let (lfd, dfd) = (lifecycle.as_raw_fd(), dir.as_raw_fd());
    unsafe {
        lock_cmd.pre_exec(move || {
            let a = libc::fcntl(lfd, libc::F_DUPFD_CLOEXEC, 10);
            let b = libc::fcntl(dfd, libc::F_DUPFD_CLOEXEC, 10);
            if a < 0 || b < 0 || libc::dup2(a, 3) < 0 || libc::dup2(b, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    assert!(lock_cmd.status().unwrap().success());

    // Spawn a detached descendant inheriting BOTH open file descriptions (fds 3 & 4).
    let mut descendant = Command::new("/bin/sh");
    descendant
        .args([
            "-c",
            &format!("echo $$ > '{}'; exec sleep 30", child_pid_file.display()),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        descendant.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let a = libc::fcntl(lfd, libc::F_DUPFD_CLOEXEC, 10);
            let b = libc::fcntl(dfd, libc::F_DUPFD_CLOEXEC, 10);
            if a < 0 || b < 0 || libc::dup2(a, 3) < 0 || libc::dup2(b, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut descendant = descendant.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !child_pid_file.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    // Close our parent handles WITHOUT calling LOCK_UN (simulating a parent crash).
    drop(lifecycle);
    drop(dir);

    // While the uncertain descendant is still alive, neither a writer nor an exclusive op can take the checkout.
    assert_eq!(
        acquire_checkout(&root, None, LeaseClass::Writer)
            .unwrap_err()
            .kind,
        LeaseErrorKind::CheckoutBusy
    );
    assert_eq!(
        acquire_checkout(&root, None, LeaseClass::Exclusive)
            .unwrap_err()
            .kind,
        LeaseErrorKind::CheckoutBusy
    );

    // Once the descendant is killed and reaped, the OS releases both leases.
    descendant.kill().unwrap();
    descendant.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(lease) = acquire_checkout(&root, None, LeaseClass::Exclusive) {
            drop(lease);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "lease did not release after descendant exited"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn record_helper_crash_releases_store_writer_and_lifecycle_leases() {
    use plait_engine::lease::{LeaseClass, acquire_checkout, prepare_checkout_lock};
    use std::os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, process::CommandExt},
    };
    use std::time::{Duration, Instant};

    let base = Scratch::new();
    let root = repository(&base.0);
    let store = base.0.join("store");
    let request = json!({"checkout":root,"store":store,"id":"seed"});
    assert!(helper("record", &request).0);
    let marker = store.join("records/lifecycle-descendants");
    let wrapper = base.0.join("git-wrapper-lifecycle");
    let real = plait_engine::git::record::git_binary();
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -n \"$GIT_DIR\" ]; then\n sleep 30 &\n echo \"$$ $!\" > '{}'\n wait\nfi\nexec '{}' \"$@\"\n",
            marker.display(),
            real.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let store_lock = fs::File::create(store.join("test-lifecycle.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(store_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let prepared = prepare_checkout_lock(&root).unwrap();
    let lifecycle_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&prepared.lock_path)
        .unwrap();
    let checkout_lock = fs::File::open(&prepared.root).unwrap();
    assert_eq!(
        unsafe { libc::flock(lifecycle_lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
        0
    );
    assert_eq!(
        unsafe { libc::flock(checkout_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_plait-host"));
    command
        .arg("record")
        .env("PLAIT_GIT", &wrapper)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (sfd, wfd, lfd) = (
        store_lock.as_raw_fd(),
        checkout_lock.as_raw_fd(),
        lifecycle_lock.as_raw_fd(),
    );
    unsafe {
        command.pre_exec(move || {
            let a = libc::fcntl(sfd, libc::F_DUPFD_CLOEXEC, 10);
            let b = libc::fcntl(wfd, libc::F_DUPFD_CLOEXEC, 10);
            let c = libc::fcntl(lfd, libc::F_DUPFD_CLOEXEC, 10);
            if a < 0
                || b < 0
                || c < 0
                || libc::dup2(a, 3) < 0
                || libc::dup2(b, 4) < 0
                || libc::dup2(c, 7) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    drop(store_lock);
    drop(checkout_lock);
    drop(lifecycle_lock);

    // All three leases (store, checkout writer, and checkout lifecycle) must release.
    let store_probe = fs::File::open(store.join("test-lifecycle.lock")).unwrap();
    while unsafe { libc::flock(store_probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        assert!(Instant::now() < deadline, "record leaked store lease");
        std::thread::sleep(Duration::from_millis(10));
    }
    while acquire_checkout(&root, None, LeaseClass::Exclusive).is_err() {
        assert!(
            Instant::now() < deadline,
            "record leaked checkout/lifecycle lease"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn lease_prepare_rejects_hard_linked_lock_file() {
    use plait_engine::lease::{checkout_lock_name, open_lease_dir, open_verified_dir};
    use std::os::unix::fs::PermissionsExt;

    let base = Scratch::new();
    let root = repository(&base.0);
    let lease_dir = base.0.join("leases");
    let (_, _, id) = open_verified_dir(&root).unwrap();
    let (canonical_lease_dir, _) = open_lease_dir(&lease_dir).unwrap();
    let outside = base.0.join("outside.lock");
    fs::write(&outside, "").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    let lock_path = canonical_lease_dir.join(checkout_lock_name(id));
    fs::hard_link(&outside, &lock_path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_plait-host"))
        .args(["lease-prepare", "checkout"])
        .arg(&root)
        .env("PLAIT_LEASE_DIR", &lease_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("single-link"), "{stderr}");
}

/// Runs `plait-host undo` with `lifecycle` as fd 7 and `writer` as fd 4, as the runtime passes them.
fn undo_with_inherited(request: &Value, lifecycle: &fs::File, writer: &fs::File) -> (bool, String) {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    let (l, w) = (lifecycle.as_raw_fd(), writer.as_raw_fd());
    let mut command = Command::new(env!("CARGO_BIN_EXE_plait-host"));
    command
        .arg("undo")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || {
            let a = libc::fcntl(l, libc::F_DUPFD_CLOEXEC, 10);
            let b = libc::fcntl(w, libc::F_DUPFD_CLOEXEC, 10);
            if a < 0 || b < 0 || libc::dup2(a, 7) < 0 || libc::dup2(b, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn undo_never_converts_an_inherited_shared_lifecycle_lock() {
    use plait_engine::lease::{LeaseClass, acquire_checkout, prepare_checkout_lock};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
    let base = Scratch::new();
    let root = repository(&base.0);
    let store = base.0.join("store");
    fs::create_dir(&store).unwrap();
    let (ok, before, error) = helper("record", &json!({"checkout":root,"store":store,"id":"b"}));
    assert!(ok, "{error}");
    fs::write(root.join("notes.txt"), "after run\n").unwrap();
    let (ok, after, error) = helper(
        "record",
        &json!({"checkout":root,"store":store,"id":"a","before":before["commit"]}),
    );
    assert!(ok, "{error}");
    let meta = fs::metadata(&root).unwrap();
    let request = |id: &str| {
        json!({
            "checkout": root, "store": store, "before": before["commit"], "after": after["commit"],
            "before_id": format!("ub-{id}"), "id": format!("ua-{id}"),
            "identity": {"dev": meta.dev().to_string(), "ino": meta.ino().to_string()}
        })
    };
    let prepared = prepare_checkout_lock(&root).unwrap();
    let open = || {
        let lifecycle = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&prepared.lock_path)
            .unwrap();
        (lifecycle, fs::File::open(&prepared.root).unwrap())
    };
    let flock = |file: &fs::File, operation| unsafe {
        libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) == 0
    };

    // A writer-class pair (lifecycle shared): undo is refused and the run's lock is left shared.
    let (lifecycle, writer) = open();
    assert!(flock(&lifecycle, libc::LOCK_SH) && flock(&writer, libc::LOCK_EX));
    let (ok, stderr) = undo_with_inherited(&request("shared"), &lifecycle, &writer);
    assert!(!ok && stderr.contains("busy"), "{stderr}");
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        "after run\n"
    );
    let reader = acquire_checkout(&root, None, LeaseClass::Reader)
        .expect("the inherited lifecycle lock must still be shared");
    drop(reader);
    drop((lifecycle, writer));

    // An exclusive pair, as turn undo passes it: the helper reuses it and restores the file.
    let (lifecycle, writer) = open();
    // Parallel tests can briefly inherit CLOEXEC files between fork and exec.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(flock(&lifecycle, libc::LOCK_EX) && flock(&writer, libc::LOCK_EX)) {
        assert!(
            Instant::now() < deadline,
            "the shared pair remained held after undo exited"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let (ok, stderr) = undo_with_inherited(&request("exclusive"), &lifecycle, &writer);
    assert!(ok, "{stderr}");
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        "before\n"
    );
    // The runtime still holds its lease after the helper exits.
    assert!(acquire_checkout(&root, None, LeaseClass::Reader).is_err());
}

#[test]
fn host_tests_keep_lock_files_out_of_the_account_lease_directory() {
    use plait_engine::lease::{
        LeaseClass, acquire_checkout, checkout_lock_name, open_verified_dir,
    };
    let base = Scratch::new();
    let root = repository(&base.0);
    drop(acquire_checkout(&root, None, LeaseClass::Writer).unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_plait-host"))
        .args(["lease-prepare", "checkout"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let prepared: Value = serde_json::from_slice(&output.stdout).unwrap();
    let private = fs::canonicalize(std::env::var_os("PLAIT_LEASE_DIR").unwrap()).unwrap();
    assert!(Path::new(prepared["lock_path"].as_str().unwrap()).starts_with(&private));
    let (_, _, id) = open_verified_dir(&root).unwrap();
    assert!(private.join(checkout_lock_name(id)).is_file());
    let account =
        PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share/plait-leases");
    assert!(!account.join(checkout_lock_name(id)).exists());
}

/// The host runs under a private umask for its store. A worktree is the
/// person's checkout, so its files must get ordinary modes, not owner-only.
#[test]
fn worktree_checkout_gets_ordinary_modes_under_the_private_host_umask() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    let base = Scratch::new();
    let root = repository(&base.0);
    let store = base.0.join("store");
    fs::create_dir(&store).unwrap();
    fs::set_permissions(&store, fs::Permissions::from_mode(0o700)).unwrap();
    let private = |request: &Value| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_plait-host"));
        command
            .arg("worktree")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: umask is async-signal-safe and changes only the child.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o077);
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(reply["error"].is_null(), "{reply}");
        reply["value"].clone()
    };
    let parent = private(&json!({"action":"parent","root":root}));
    let id = "12345678-1234-4abc-8abc-123456789abc";
    let tree = private(&json!({"action":"create","store":store,"thread_id":id,"parent":parent}));
    assert_eq!(tree["stage"], "ready", "{tree}");
    let path = PathBuf::from(tree["path"].as_str().unwrap());
    let mode = |path: &Path| fs::symlink_metadata(path).unwrap().mode() & 0o777;
    assert_eq!(mode(&path.join("notes.txt")), 0o644);
    assert_eq!(mode(&path), 0o755);
    assert_eq!(mode(path.parent().unwrap()), 0o755);
    // Journals keep their own private modes.
    assert_eq!(mode(&store.join("worktrees")), 0o700);
    assert_eq!(mode(&store.join(format!("worktrees/{id}.json"))), 0o600);
    git(&root, &["worktree", "remove", path.to_str().unwrap()]);
}

#[test]
fn host_admission_checks_git_records_only_while_holding_the_checkout_locks() {
    use plait_engine::lease::{git_cleanup, open_verified_dir, prepare_checkout_lock};
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    use std::time::{Duration, Instant};

    let base = Scratch::new();
    let root = repository(&base.0);
    let prepared = prepare_checkout_lock(&root).unwrap();
    let id = open_verified_dir(&root).unwrap().2;
    // A record checked before the locks is a check a dying desktop can pass:
    // its lease frees after the check while its Git child still runs.
    assert!(git_cleanup::unconfirmed(id).unwrap().is_none());
    let lease_dir = PathBuf::from(std::env::var_os("PLAIT_LEASE_DIR").unwrap());
    let record =
        fs::File::open(lease_dir.join(format!("git-cleanup-{:016x}-{:016x}.lock", id.dev, id.ino)))
            .unwrap();
    assert_eq!(unsafe { libc::flock(record.as_raw_fd(), libc::LOCK_EX) }, 0);
    let lifecycle = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&prepared.lock_path)
        .unwrap();
    let dir = fs::File::open(&prepared.root).unwrap();
    let (lfd, dfd) = (lifecycle.as_raw_fd(), dir.as_raw_fd());
    let mut helper = Command::new(env!("CARGO_BIN_EXE_plait-host"));
    helper.args(["lock-fd", "writer"]).arg(&prepared.root);
    unsafe {
        helper.pre_exec(move || {
            let a = libc::fcntl(lfd, libc::F_DUPFD_CLOEXEC, 10);
            let b = libc::fcntl(dfd, libc::F_DUPFD_CLOEXEC, 10);
            if a < 0 || b < 0 || libc::dup2(a, 3) < 0 || libc::dup2(b, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut helper = helper.spawn().unwrap();
    // While the helper waits on the record, the lifecycle lock must already be its.
    let probe = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&prepared.lock_path)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut locked_first = false;
    while Instant::now() < deadline {
        if unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            locked_first = true;
            break;
        }
        unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_UN) };
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe { libc::flock(record.as_raw_fd(), libc::LOCK_UN) };
    let status = helper.wait().unwrap();
    assert!(
        locked_first,
        "lock-fd read the Git record before taking the checkout locks"
    );
    assert!(status.success());
}
