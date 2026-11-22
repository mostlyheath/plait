use super::*;
use crate::git::tests::Repo;
use std::{
    collections::BTreeMap,
    os::unix::fs::symlink,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

/// A store directory apart from any checkout.
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "plait-records-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(root: &Path, store: &Path, id: &str, before: Option<&Record>) -> Request {
    Request {
        checkout: root.to_path_buf(),
        store: store.to_path_buf(),
        id: id.into(),
        before: before.map(|r| r.commit.clone()),
    }
}

fn take_at(root: &Path, store: &Path, id: &str, before: Option<&Record>) -> Record {
    let located = locate(root).unwrap();
    record(&located, &request(root, store, id, before)).unwrap()
}

fn take(repo: &Repo, store: &Scratch, id: &str, before: Option<&Record>) -> Record {
    take_at(&repo.0, &store.0, id, before)
}

/// A run's changes in a short, readable form.
fn summary(record: &Record) -> Vec<String> {
    record
        .paths
        .as_ref()
        .expect("a later record lists paths")
        .iter()
        .map(|p| {
            let kind = match (&p.before_mode, &p.after_mode) {
                (None, Some(_)) => "A".to_string(),
                (Some(_), None) => "D".to_string(),
                (Some(a), Some(b)) if a != b && p.before_oid == p.after_oid => {
                    format!("mode {a}->{b}")
                }
                _ => "M".to_string(),
            };
            let count = |n: Option<u64>| n.map_or("-".to_string(), |n| n.to_string());
            format!(
                "{kind} {} +{} -{}",
                p.path,
                count(p.added),
                count(p.removed)
            )
        })
        .collect()
}

fn uncovered_list(record: &Record) -> Vec<(String, Reason)> {
    record
        .not_covered
        .iter()
        .map(|u| (u.path.clone(), u.reason))
        .collect()
}

/// A command against the store's own Git directory, optionally reading the
/// person's objects too.
fn store_git(store: &Path, format: &str, objects: Option<&Path>, args: &[&str]) -> (bool, Vec<u8>) {
    let mut command = Command::new(git_binary());
    command
        .env_clear()
        .env("PATH", SAFE_PATH)
        .env("GIT_DIR", records_dir(store).join(format))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", "/nonexistent")
        .args(["--literal-pathspecs"])
        .args(args);
    if let Some(objects) = objects {
        command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects);
    }
    let out = command.output().unwrap();
    (out.status.success(), out.stdout)
}

/// The mode and content a record holds for a path.
fn held(store: &Path, located: &Located, record: &Record, path: &str) -> Option<(String, Vec<u8>)> {
    let objects = located.common_dir.join("objects");
    let (_, out) = store_git(
        store,
        &located.object_format,
        Some(&objects),
        &["ls-tree", "-z", &record.commit, "--", path],
    );
    let line = out.split(|b| *b == 0).next().filter(|l| !l.is_empty())?;
    let text = String::from_utf8_lossy(line).into_owned();
    let (meta, _) = text.split_once('\t').unwrap();
    let parts: Vec<&str> = meta.split(' ').collect();
    let (ok, content) = store_git(
        store,
        &located.object_format,
        Some(&objects),
        &["cat-file", "blob", parts[2]],
    );
    assert!(ok);
    Some((parts[0].to_owned(), content))
}

/// Whether the store holds an object itself, without the person's objects.
fn owned(store: &Path, format: &str, oid: &str) -> bool {
    store_git(store, format, None, &["cat-file", "-e", oid]).0
}

type Stat = (bool, u64, u32, i64, i64, i64, i64, u64, Option<Vec<u8>>);

/// Every path under `root`, Git directory included, with the metadata a write
/// of any kind would change.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Stat> {
    let mut all = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = fs::symlink_metadata(&path).unwrap();
        all.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
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
                    Some(Sha256::digest(fs::read(&path).unwrap()).to_vec())
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
            for entry in fs::read_dir(&path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        }
    }
    all
}

/// Runs `work` and asserts nothing under any of `roots` changed, not even a time.
fn untouched<T>(roots: &[&Path], work: impl FnOnce() -> T) -> T {
    let before: Vec<_> = roots.iter().map(|root| snapshot(root)).collect();
    let result = work();
    for (root, earlier) in roots.iter().zip(before) {
        let later = snapshot(root);
        for (path, stat) in &earlier {
            assert_eq!(
                later.get(path),
                Some(stat),
                "{} changed under {}",
                path.display(),
                root.display()
            );
        }
        assert_eq!(
            earlier.len(),
            later.len(),
            "paths appeared under {}",
            root.display()
        );
    }
    result
}

fn committed() -> Repo {
    let repo = Repo::new();
    repo.write("a.txt", "one\ntwo\n");
    repo.write("keep.txt", "keep\n");
    repo.write("old.txt", "old\n");
    repo.write("run.sh", "echo hi\n");
    repo.commit();
    repo
}

#[test]
fn a_run_lists_what_it_changed_and_nothing_else() {
    let repo = committed();
    let store = Scratch::new();
    let before = untouched(&[&repo.0], || take(&repo, &store, "b1", None));
    assert!(before.paths.is_none());
    assert_eq!(before.head_ref.as_deref(), Some("refs/heads/main"));
    assert!(before.head.is_some());
    repo.write("a.txt", "one\nthree\nfour\n");
    repo.write("new.txt", "new\n");
    fs::remove_file(repo.0.join("old.txt")).unwrap();
    // Created and deleted within the run: no trace.
    repo.write("brief.txt", "brief\n");
    fs::remove_file(repo.0.join("brief.txt")).unwrap();
    let mode = fs::metadata(repo.0.join("run.sh")).unwrap().permissions();
    fs::set_permissions(
        repo.0.join("run.sh"),
        fs::Permissions::from_mode(mode.mode() | 0o111),
    )
    .unwrap();
    let after = untouched(&[&repo.0], || take(&repo, &store, "a1", Some(&before)));
    assert_eq!(
        summary(&after),
        [
            "M a.txt +2 -1",
            "A new.txt +1 -0",
            "D old.txt +0 -1",
            "mode 100644->100755 run.sh +0 -0",
        ]
    );
    assert!(after.not_covered.is_empty());
    // The next run's earlier record sees no change since this one.
    let next = take(&repo, &store, "b2", None);
    let next_after = take(&repo, &store, "a2", Some(&next));
    assert!(summary(&next_after).is_empty());
    assert_eq!(next.tree, after.tree);
}

#[test]
fn records_hold_exact_bytes_of_binary_files() {
    let repo = committed();
    let store = Scratch::new();
    let bytes: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    repo.write("blob.bin", &bytes);
    let before = take(&repo, &store, "b", None);
    let changed: Vec<u8> = bytes.iter().rev().copied().collect();
    repo.write("blob.bin", &changed);
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["M blob.bin +- --"]);
    let located = locate(&repo.0).unwrap();
    assert_eq!(
        held(&store.0, &located, &before, "blob.bin").unwrap().1,
        bytes
    );
    assert_eq!(
        held(&store.0, &located, &after, "blob.bin").unwrap().1,
        changed
    );
}

#[test]
fn symlinks_are_recorded_as_links_never_followed() {
    let repo = committed();
    let store = Scratch::new();
    let outside = Scratch::new();
    fs::write(outside.0.join("secret"), "outside the checkout\n").unwrap();
    let before = take(&repo, &store, "b", None);
    symlink("a.txt", repo.0.join("inside")).unwrap();
    symlink(outside.0.join("secret"), repo.0.join("outside")).unwrap();
    symlink("/nonexistent/target", repo.0.join("dangling")).unwrap();
    let after = untouched(&[&repo.0, &outside.0], || {
        take(&repo, &store, "a", Some(&before))
    });
    assert_eq!(
        summary(&after),
        ["A dangling +1 -0", "A inside +1 -0", "A outside +1 -0"]
    );
    let located = locate(&repo.0).unwrap();
    let (mode, target) = held(&store.0, &located, &after, "outside").unwrap();
    assert_eq!(mode, "120000");
    assert_eq!(target, outside.0.join("secret").as_os_str().as_bytes());
    assert_eq!(
        held(&store.0, &located, &after, "inside").unwrap(),
        ("120000".into(), b"a.txt".to_vec())
    );
}

#[test]
fn crlf_and_filtered_files_keep_their_bytes_and_filters_never_run() {
    let repo = Repo::new();
    let marker = repo.0.join("filter-ran");
    repo.write(
        ".gitattributes",
        "*.txt text eol=crlf\n*.dat filter=upper\n",
    );
    repo.git(&[
        "config",
        "filter.upper.clean",
        &format!("sh -c 'touch {}; tr a-z A-Z'", marker.display()),
    ]);
    repo.git(&["config", "filter.upper.smudge", "cat"]);
    repo.write("lines.txt", "one\r\ntwo\r\n");
    repo.write("data.dat", "lower case\n");
    // Older than the index, so Git trusts the stat data and would not hash them again.
    thread::sleep(Duration::from_millis(1100));
    repo.commit();
    fs::remove_file(&marker).unwrap();
    // The person's index holds converted content: LF endings and upper case.
    let store = Scratch::new();
    let located = locate(&repo.0).unwrap();
    let before = untouched(&[&repo.0], || take(&repo, &store, "b", None));
    assert!(!marker.exists(), "a record ran the clean filter");
    assert_eq!(
        held(&store.0, &located, &before, "lines.txt").unwrap().1,
        b"one\r\ntwo\r\n"
    );
    assert_eq!(
        held(&store.0, &located, &before, "data.dat").unwrap().1,
        b"lower case\n"
    );
    // Untracked files under the same attributes are taken as they are too.
    repo.write("new.txt", "a\r\nb\n");
    repo.write("new.dat", "mixed Case\n");
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["A new.dat +1 -0", "A new.txt +2 -0"]);
    assert_eq!(
        held(&store.0, &located, &after, "new.txt").unwrap().1,
        b"a\r\nb\n"
    );
    assert_eq!(
        held(&store.0, &located, &after, "new.dat").unwrap().1,
        b"mixed Case\n"
    );
    assert!(!marker.exists(), "a record ran the clean filter");
}

#[test]
fn autocrlf_checkouts_are_hashed_whole_with_exact_bytes() {
    let repo = Repo::new();
    repo.git(&["config", "core.autocrlf", "true"]);
    repo.write("dos.txt", "one\r\ntwo\r\n");
    repo.commit();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    let located = locate(&repo.0).unwrap();
    assert_eq!(
        held(&store.0, &located, &before, "dos.txt").unwrap().1,
        b"one\r\ntwo\r\n"
    );
}

#[test]
fn ignored_files_are_kept_within_their_limits() {
    let repo = committed();
    repo.write(".gitignore", "*.env\n*.log\ntarget/\n");
    repo.commit();
    repo.write("local.env", "TOKEN=1\n");
    repo.write("big.log", vec![b'x'; IGNORED_FILE_LIMIT as usize + 1]);
    fs::create_dir_all(repo.0.join("target/debug")).unwrap();
    repo.write("target/debug/out", "build output\n");
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    assert_eq!(
        uncovered_list(&before),
        [
            ("big.log".to_string(), Reason::IgnoredLarge),
            ("target/".to_string(), Reason::IgnoredDirectory),
        ]
    );
    let located = locate(&repo.0).unwrap();
    assert_eq!(
        held(&store.0, &located, &before, "local.env").unwrap().1,
        b"TOKEN=1\n"
    );
    // A change to a kept ignored file is a change like any other; one that
    // grows past the limit leaves the record and is listed, not shown as deleted.
    repo.write("local.env", "TOKEN=2\n");
    repo.write("second.env", "B=1\n");
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["M local.env +1 -1", "A second.env +1 -0"]);
    let grown = take(&repo, &store, "b2", None);
    repo.write("local.env", vec![b'y'; IGNORED_FILE_LIMIT as usize + 1]);
    let after = take(&repo, &store, "a2", Some(&grown));
    assert!(summary(&after).is_empty(), "{:?}", summary(&after));
    assert!(uncovered_list(&after).contains(&("local.env".into(), Reason::IgnoredLarge)));
    assert!(held(&store.0, &located, &after, "local.env").is_none());
}

#[test]
fn ignored_files_stop_at_the_count_limit_and_a_later_file_is_not_shown_as_created() {
    let repo = committed();
    repo.write(".gitignore", "*.o\n");
    repo.commit();
    fs::create_dir(repo.0.join("objs")).unwrap();
    for n in 0..=IGNORED_COUNT_LIMIT {
        repo.write(&format!("objs/{n:05}.o"), "o");
    }
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    assert_eq!(before.not_covered_total, 1);
    let last = format!("objs/{IGNORED_COUNT_LIMIT:05}.o");
    assert_eq!(
        uncovered_list(&before),
        [(last.clone(), Reason::IgnoredLimit)]
    );
    // One kept file goes; the one left out now fits, but the earlier record
    // could not see it, so the run did not create it.
    fs::remove_file(repo.0.join("objs/00000.o")).unwrap();
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["D objs/00000.o +0 -1"]);
    let located = locate(&repo.0).unwrap();
    assert!(held(&store.0, &located, &after, &last).is_some());
}

#[test]
fn ignored_files_stop_at_the_total_size_limit() {
    let repo = committed();
    repo.write(".gitignore", "*.bin\n");
    repo.commit();
    let count = (IGNORED_TOTAL_LIMIT / IGNORED_FILE_LIMIT) as usize + 1;
    for n in 0..count {
        repo.write(
            &format!("{n:03}.bin"),
            vec![n as u8; IGNORED_FILE_LIMIT as usize],
        );
    }
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    assert_eq!(
        uncovered_list(&before),
        [(format!("{:03}.bin", count - 1), Reason::IgnoredLimit)]
    );
}

#[test]
fn a_file_over_the_size_limit_is_listed_not_recorded() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    // Sparse, so the test does not write 100 MB.
    let file = fs::File::create(repo.0.join("huge.iso")).unwrap();
    file.set_len(FILE_LIMIT + 1).unwrap();
    let after = take(&repo, &store, "a", Some(&before));
    assert!(summary(&after).is_empty());
    assert_eq!(
        uncovered_list(&after),
        [("huge.iso".into(), Reason::TooLarge)]
    );
    // A tracked file that grows past the limit is not shown as deleted.
    let next = take(&repo, &store, "b2", None);
    fs::File::options()
        .write(true)
        .open(repo.0.join("a.txt"))
        .unwrap()
        .set_len(FILE_LIMIT + 1)
        .unwrap();
    let grown = take(&repo, &store, "a2", Some(&next));
    assert!(summary(&grown).is_empty(), "{:?}", summary(&grown));
    assert!(uncovered_list(&grown).contains(&("a.txt".into(), Reason::TooLarge)));
}

#[test]
fn a_large_first_record_is_packed_and_later_records_stay_loose() {
    let repo = Repo::new();
    fs::create_dir(repo.0.join("many")).unwrap();
    for n in 0..BULK_PATHS {
        repo.write(&format!("many/{n}.txt"), format!("{n}\n"));
    }
    let store = Scratch::new();
    let objects = records_dir(&store.0).join("sha1/objects");
    let packs = || {
        fs::read_dir(objects.join("pack"))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "pack"))
                    .count()
            })
            .unwrap_or(0)
    };
    let before = take(&repo, &store, "b", None);
    assert_eq!(packs(), 1);
    let located = locate(&repo.0).unwrap();
    assert_eq!(
        held(&store.0, &located, &before, "many/7.txt").unwrap().1,
        b"7\n"
    );
    for n in 0..BULK_PATHS {
        repo.write(&format!("many/{n}.txt"), format!("{n} changed\n"));
    }
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(after.paths.as_ref().unwrap().len(), BULK_PATHS);
    assert_eq!(
        packs(),
        1,
        "a later record made a pack that pruning cannot shrink"
    );
}

#[test]
fn untracked_directories_are_recorded_file_by_file() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    fs::create_dir_all(repo.0.join("gen/deep/er")).unwrap();
    repo.write("gen/one.rs", "1\n");
    repo.write("gen/deep/two.rs", "2\n");
    repo.write("gen/deep/er/three.rs", "3\n3\n");
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(
        summary(&after),
        [
            "A gen/deep/er/three.rs +2 -0",
            "A gen/deep/two.rs +1 -0",
            "A gen/one.rs +1 -0",
        ]
    );
}

#[test]
fn nested_repositories_and_submodules_are_not_covered() {
    let upstream = Repo::new();
    upstream.write("lib.rs", "lib\n");
    upstream.commit();
    let repo = committed();
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        upstream.0.to_str().unwrap(),
        "vendor/lib",
    ]);
    repo.git(&["commit", "-qm", "submodule"]);
    let nested = repo.0.join("tools");
    fs::create_dir(&nested).unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(&nested)
        .status()
        .unwrap();
    fs::write(nested.join("tool.rs"), "tool\n").unwrap();
    let store = Scratch::new();
    let before = untouched(&[&repo.0, &upstream.0], || take(&repo, &store, "b", None));
    assert_eq!(
        uncovered_list(&before),
        [
            ("tools/".into(), Reason::NestedRepository),
            ("vendor/lib/".into(), Reason::Submodule),
        ]
    );
    // Work inside either never shows as the run's change.
    fs::write(nested.join("tool.rs"), "changed\n").unwrap();
    fs::write(repo.0.join("vendor/lib/lib.rs"), "changed\n").unwrap();
    let after = take(&repo, &store, "a", Some(&before));
    assert!(summary(&after).is_empty(), "{:?}", summary(&after));
}

#[test]
fn bare_and_separate_git_directories_are_excluded_across_creation_and_deletion() {
    for name in ["mirror.git", "metadata"] {
        let repo = committed();
        let store = Scratch::new();
        let before = take(&repo, &store, "before", None);
        repo.git(&["init", "--bare", "-q", name]);
        let created = take(&repo, &store, "created", Some(&before));
        assert!(created.paths.as_ref().unwrap().is_empty());
        assert!(uncovered_list(&created).contains(&(format!("{name}/"), Reason::NestedRepository)));
        let existing = take(&repo, &store, "existing", None);
        let (ok, tree) = store_git(
            &store.0,
            "sha1",
            None,
            &["ls-tree", "-r", "--name-only", &existing.commit],
        );
        assert!(ok);
        assert!(!String::from_utf8(tree).unwrap().contains(name));
        fs::remove_dir_all(repo.0.join(name)).unwrap();
        let deleted = take(&repo, &store, "deleted", Some(&existing));
        assert!(deleted.paths.as_ref().unwrap().is_empty());
        assert!(uncovered_list(&deleted).contains(&(format!("{name}/"), Reason::NestedRepository)));
    }
}

#[test]
fn a_checkout_with_no_commits_is_recorded() {
    let repo = Repo::new();
    repo.write("first.txt", "first\n");
    let store = Scratch::new();
    let before = untouched(&[&repo.0], || take(&repo, &store, "b", None));
    assert_eq!(before.head, None);
    assert_eq!(before.head_ref.as_deref(), Some("refs/heads/main"));
    repo.write("first.txt", "first\nsecond\n");
    repo.git(&["add", "first.txt"]);
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["M first.txt +1 -0"]);
}

#[test]
fn a_held_index_lock_changes_nothing() {
    let repo = committed();
    repo.write("a.txt", "dirty\n");
    let lock = repo.0.join(".git/index.lock");
    fs::write(&lock, "held by another Git").unwrap();
    let store = Scratch::new();
    let before = untouched(&[&repo.0], || take(&repo, &store, "b", None));
    repo.write("a.txt", "dirtier\n");
    let after = untouched(&[&repo.0], || take(&repo, &store, "a", Some(&before)));
    assert_eq!(summary(&after), ["M a.txt +1 -1"]);
    assert_eq!(fs::read(&lock).unwrap(), b"held by another Git");
}

#[test]
fn a_moved_head_is_reported() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    repo.write("a.txt", "committed by the run\n");
    repo.commit();
    repo.git(&["checkout", "-q", "-b", "topic"]);
    let after = take(&repo, &store, "a", Some(&before));
    assert_ne!(after.head, before.head);
    assert_eq!(after.head_ref.as_deref(), Some("refs/heads/topic"));
    assert_eq!(summary(&after), ["M a.txt +1 -2"]);
}

#[test]
fn a_per_thread_worktree_reads_the_main_repository_objects() {
    let repo = committed();
    let tree = Scratch::new();
    let path = tree.0.join("thread");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "thread",
        path.to_str().unwrap(),
    ]);
    let store = Scratch::new();
    let located = locate(&path).unwrap();
    assert_eq!(
        located.common_dir,
        fs::canonicalize(repo.0.join(".git")).unwrap()
    );
    assert_ne!(located.git_dir, located.common_dir);
    let before = untouched(&[&repo.0, &path], || take_at(&path, &store.0, "b", None));
    fs::write(path.join("a.txt"), "worktree change\n").unwrap();
    let after = untouched(&[&repo.0, &path], || {
        take_at(&path, &store.0, "a", Some(&before))
    });
    assert_eq!(summary(&after), ["M a.txt +1 -2"]);
    let earlier = &after.paths.as_ref().unwrap()[0];
    assert!(owned(
        &store.0,
        "sha1",
        earlier.before_oid.as_ref().unwrap()
    ));
    assert_eq!(
        held(&store.0, &located, &before, "keep.txt").unwrap().1,
        b"keep\n"
    );
}

#[test]
fn two_checkouts_record_into_one_store() {
    let first = committed();
    let second = committed();
    second.write("a.txt", "second checkout\n");
    let store = Scratch::new();
    let one = take(&first, &store, "one-b", None);
    let two = take(&second, &store, "two-b", None);
    first.write("x.txt", "x\n");
    second.write("y.txt", "y\n");
    let one = take(&first, &store, "one-a", Some(&one));
    let two = take(&second, &store, "two-a", Some(&two));
    assert_eq!(summary(&one), ["A x.txt +1 -0"]);
    assert_eq!(summary(&two), ["A y.txt +1 -0"]);
    let worktrees = fs::read_dir(records_dir(&store.0).join("sha1/worktrees")).unwrap();
    assert_eq!(worktrees.count(), 2);
}

#[test]
fn a_sha256_repository_has_its_own_store() {
    let repo = Repo::new();
    let path = repo.0.join("sha256");
    let made = Command::new("git")
        .args(["init", "-q", "--object-format=sha256"])
        .arg(&path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    if !made.success() {
        eprintln!("This Git cannot make a SHA-256 repository; skipped.");
        return;
    }
    fs::write(path.join("f.txt"), "f\n").unwrap();
    for args in [
        &["add", "."][..],
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e.invalid",
            "commit",
            "-qm",
            "x",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .envs(crate::git::tests::NO_MAINTENANCE)
                .status()
                .unwrap()
                .success()
        );
    }
    let store = Scratch::new();
    let before = untouched(&[&path], || take_at(&path, &store.0, "b", None));
    assert_eq!(before.object_format, "sha256");
    assert_eq!(before.commit.len(), 64);
    fs::write(path.join("f.txt"), "g\n").unwrap();
    let after = take_at(&path, &store.0, "a", Some(&before));
    assert_eq!(summary(&after), ["M f.txt +1 -1"]);
    use std::os::unix::fs::MetadataExt;
    let root = path.canonicalize().unwrap();
    let metadata = fs::metadata(&root).unwrap();
    let diff = crate::git::review::read(&crate::git::review::Request {
        store: store.0.clone(),
        checkout: root,
        dev: metadata.dev(),
        ino: metadata.ino(),
        object_format: "sha256".into(),
        changed: after.paths.as_ref().unwrap()[0].clone(),
    })
    .unwrap();
    assert!(String::from_utf8(diff.patch).unwrap().contains("-f\n+g\n"));

    assert!(records_dir(&store.0).join("sha256/HEAD").is_file());
    assert!(owned(
        &store.0,
        "sha256",
        after.paths.as_ref().unwrap()[0]
            .before_oid
            .as_ref()
            .unwrap()
    ));
}

#[test]
fn file_names_with_spaces_newlines_accents_and_dashes_survive() {
    let repo = committed();
    let names = [
        "with space.txt",
        "new\nline.txt",
        "café.txt",
        "-dash.txt",
        "--all",
    ];
    for name in &names[..2] {
        repo.write(name, "tracked\n");
    }
    repo.commit();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    for name in names {
        repo.write(name, "changed\n");
    }
    let after = take(&repo, &store, "a", Some(&before));
    let mut paths: Vec<String> = after.paths.unwrap().into_iter().map(|p| p.path).collect();
    paths.sort();
    let mut expected: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(paths, expected);
}

#[test]
fn a_file_changing_while_recorded_is_caught_by_the_next_record() {
    let repo = committed();
    let store = Scratch::new();
    let path = repo.0.join("busy.log");
    fs::write(&path, "start\n").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (stop, path) = (stop.clone(), path.clone());
        thread::spawn(move || {
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                n += 1;
                fs::write(&path, format!("write {n}\n").repeat((n % 50) as usize + 1)).unwrap();
            }
            n
        })
    };
    let mut records = Vec::new();
    for round in 0..5 {
        records.push(take(&repo, &store, &format!("r{round}"), None));
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    let last = fs::read(&path).unwrap();
    let settled = take(&repo, &store, "settled", None);
    let located = locate(&repo.0).unwrap();
    assert_eq!(
        held(&store.0, &located, &settled, "busy.log").unwrap().1,
        last
    );
    // Same size, same second: Git's racy-clean check must still see it.
    let before = take(&repo, &store, "b", None);
    let same: Vec<u8> = last
        .iter()
        .map(|b| if *b == b'w' { b'W' } else { *b })
        .collect();
    fs::write(&path, &same).unwrap();
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after).len(), 1, "{:?}", summary(&after));
    assert_eq!(
        held(&store.0, &located, &after, "busy.log").unwrap().1,
        same
    );
}

#[test]
fn a_path_marked_changing_is_left_out_of_the_runs_changes() {
    let earlier = Note::default();
    let later = vec![Uncovered {
        path: "busy.log".into(),
        reason: Reason::Changing,
    }];
    assert!(blind_in("busy.log", &later, true));
    assert!(!blind_in("busy.log", &earlier.not_covered, true));
    // A tracked file inside an ignored directory is seen by both records.
    let ignored = vec![Uncovered {
        path: "target/".into(),
        reason: Reason::IgnoredDirectory,
    }];
    assert!(!blind_in("target/kept", &ignored, true));
    assert!(blind_in("target/kept", &ignored, false));
    assert!(!blind_in("targets", &ignored, false));
}

#[test]
fn a_tracked_file_inside_an_ignored_directory_still_counts() {
    let repo = committed();
    repo.write(".gitignore", "build/\n");
    fs::create_dir(repo.0.join("build")).unwrap();
    repo.write("build/kept.cfg", "kept\n");
    repo.git(&["add", "-f", "build/kept.cfg", ".gitignore"]);
    repo.git(&["commit", "-qm", "kept"]);
    fs::create_dir(repo.0.join("build/cache")).unwrap();
    repo.write("build/cache/blob", "untracked output\n");
    repo.write("build/out.o", "untracked output\n");
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    // A tracked exception does not expose the rest of an ignored directory.
    assert_eq!(
        uncovered_list(&before),
        [
            ("build/".into(), Reason::IgnoredDirectory),
            ("build/cache/".into(), Reason::IgnoredDirectory)
        ]
    );
    let located = locate(&repo.0).unwrap();
    assert!(held(&store.0, &located, &before, "build/out.o").is_none());
    repo.write("build/kept.cfg", "changed\n");
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["M build/kept.cfg +1 -1"]);
}

#[test]
fn an_unreadable_file_is_listed_and_the_rest_is_recorded() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    repo.write("secret.txt", "no access\n");
    fs::set_permissions(repo.0.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    repo.write("a.txt", "readable change\n");
    repo.write("b.txt", "another\n");
    let after = take(&repo, &store, "a", Some(&before));
    fs::set_permissions(repo.0.join("secret.txt"), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(summary(&after), ["M a.txt +1 -2", "A b.txt +1 -0"]);
    assert_eq!(
        uncovered_list(&after),
        [("secret.txt".into(), Reason::Unreadable)]
    );
}

#[test]
fn untracked_unreadable_directories_stay_excluded_on_either_side() {
    for unreadable_first in [true, false] {
        let repo = committed();
        let store = Scratch::new();
        let directory = repo.0.join("secret");
        fs::create_dir(&directory).unwrap();
        repo.write("secret/keep", "pre-existing valuable bytes\n");
        let set_mode =
            |mode| fs::set_permissions(&directory, fs::Permissions::from_mode(mode)).unwrap();
        if unreadable_first {
            set_mode(0o000);
        }
        let before = take(&repo, &store, "before", None);
        set_mode(if unreadable_first { 0o755 } else { 0o000 });
        let after = take(&repo, &store, "after", Some(&before));
        set_mode(0o755);
        assert!(after.paths.as_ref().unwrap().is_empty());
        assert!(uncovered_list(&after).contains(&("secret/".into(), Reason::Unreadable)));
    }
}

#[test]
fn untracked_fifo_and_socket_are_reported_as_unsupported() {
    let repo = committed();
    let store = Scratch::new();
    let name = std::ffi::CString::new(repo.0.join("pipe").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let _socket = std::os::unix::net::UnixListener::bind(repo.0.join("socket")).unwrap();
    let before = take(&repo, &store, "before", None);
    assert!(uncovered_list(&before).contains(&("pipe".into(), Reason::Special)));
    assert!(uncovered_list(&before).contains(&("socket".into(), Reason::Special)));
    fs::remove_file(repo.0.join("pipe")).unwrap();
    repo.write("pipe", "replaced unsupported entry");
    let after = take(&repo, &store, "after", Some(&before));
    assert!(after.paths.as_ref().unwrap().is_empty());
}

#[test]
fn files_and_directories_can_swap_places() {
    let repo = committed();
    fs::create_dir(repo.0.join("dir")).unwrap();
    repo.write("dir/inner.txt", "inner\n");
    repo.commit();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    fs::remove_file(repo.0.join("a.txt")).unwrap();
    fs::create_dir(repo.0.join("a.txt")).unwrap();
    repo.write("a.txt/now.txt", "now a directory\n");
    fs::remove_dir_all(repo.0.join("dir")).unwrap();
    repo.write("dir", "now a file\n");
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(
        summary(&after),
        [
            "D a.txt +0 -2",
            "A a.txt/now.txt +1 -0",
            "A dir +1 -0",
            "D dir/inner.txt +0 -1",
        ]
    );
}

#[test]
fn a_directory_replaced_by_a_symlink_is_recorded() {
    let repo = committed();
    fs::create_dir(repo.0.join("dir")).unwrap();
    repo.write("dir/inner.txt", "inner\n");
    repo.commit();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    fs::remove_dir_all(repo.0.join("dir")).unwrap();
    symlink("/tmp", repo.0.join("dir")).unwrap();
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["A dir +1 -0", "D dir/inner.txt +0 -1"]);
}

#[test]
fn earlier_content_is_copied_into_the_store() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    let located = locate(&repo.0).unwrap();
    let (_, out) = store_git(
        &store.0,
        "sha1",
        Some(&located.common_dir.join("objects")),
        &["rev-parse", &format!("{}:keep.txt", before.commit)],
    );
    let oid = String::from_utf8(out).unwrap().trim().to_owned();
    // Seeded from the person's index, the store has not copied it yet.
    assert!(!owned(&store.0, "sha1", &oid));
    repo.write("keep.txt", "changed\n");
    let after = untouched(&[&repo.0], || take(&repo, &store, "a", Some(&before)));
    assert_eq!(
        after.paths.as_ref().unwrap()[0].before_oid.as_deref(),
        Some(oid.as_str())
    );
    assert!(owned(&store.0, "sha1", &oid));
}

#[test]
fn records_are_commits_linked_only_within_a_run() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    let after = take(&repo, &store, "a", Some(&before));
    let next = take(&repo, &store, "b2", None);
    let parents = |commit: &str| {
        let located = locate(&repo.0).unwrap();
        let (_, out) = store_git(
            &store.0,
            "sha1",
            Some(&located.common_dir.join("objects")),
            &["rev-list", "--parents", "-n", "1", commit],
        );
        String::from_utf8(out).unwrap().split_whitespace().count() - 1
    };
    assert_eq!(parents(&before.commit), 0);
    assert_eq!(parents(&after.commit), 1);
    assert_eq!(parents(&next.commit), 0);
    let (_, refs) = store_git(
        &store.0,
        "sha1",
        None,
        &["for-each-ref", "--format=%(refname)"],
    );
    assert_eq!(
        String::from_utf8(refs).unwrap(),
        "refs/records/a\nrefs/records/b\nrefs/records/b2\n"
    );
}

#[test]
fn prune_forgets_records_and_the_next_record_still_works() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    repo.write("gone.txt", "only in an old record\n");
    let after = take(&repo, &store, "a", Some(&before));
    let oid = after.paths.as_ref().unwrap()[0].after_oid.clone().unwrap();
    fs::remove_file(repo.0.join("gone.txt")).unwrap();
    let latest = take(&repo, &store, "b2", None);
    let pruned = untouched(&[&repo.0], || {
        prune(&PruneRequest {
            store: store.0.clone(),
            remove: vec!["b".into(), "a".into()],
        })
        .unwrap()
    });
    assert_eq!(
        pruned,
        Pruned {
            refs_removed: 2,
            checkouts_removed: 0,
            pruned: true
        }
    );
    assert!(
        !owned(&store.0, "sha1", &oid),
        "an unreachable object was kept"
    );
    assert!(owned(&store.0, "sha1", &latest.commit));
    repo.write("a.txt", "after the prune\n");
    let next = take(&repo, &store, "a2", Some(&latest));
    assert_eq!(summary(&next), ["M a.txt +1 -2"]);
    // Nothing to forget: nothing runs.
    assert_eq!(
        prune(&PruneRequest {
            store: store.0.clone(),
            remove: vec![]
        })
        .unwrap(),
        Pruned::default()
    );
}

#[test]
fn prune_drops_checkouts_that_no_longer_exist() {
    let store = Scratch::new();
    let kept = committed();
    take(&kept, &store, "kept", None);
    {
        let gone = committed();
        take(&gone, &store, "gone", None);
    }
    assert_eq!(sources(&store.0).len(), 2);
    let pruned = prune(&PruneRequest {
        store: store.0.clone(),
        remove: vec!["gone".into()],
    })
    .unwrap();
    assert_eq!(pruned.checkouts_removed, 1);
    assert_eq!(sources(&store.0).len(), 1);
}

#[test]
fn bad_requests_are_refused_before_any_work() {
    let repo = committed();
    let store = Scratch::new();
    let located = locate(&repo.0).unwrap();
    for id in ["", "a/b", "../x", &"x".repeat(129)] {
        assert!(record(&located, &request(&repo.0, &store.0, id, None)).is_err());
    }
    let mut bad = request(&repo.0, &store.0, "x", None);
    bad.before = Some("not-a-commit".into());
    assert!(record(&located, &bad).is_err());
    assert!(!records_dir(&store.0).exists());
    let plain = Scratch::new();
    assert!(locate(&plain.0).is_err());
    fs::create_dir(repo.0.join("sub")).unwrap();
    assert!(locate(&repo.0.join("sub")).is_err());
}

#[test]
fn a_record_does_not_extend_its_helpers_deadline() {
    let repo = committed();
    let store = Scratch::new();
    let located = locate(&repo.0).unwrap();
    let result = within(Instant::now(), || {
        record(&located, &request(&repo.0, &store.0, "late", None))
    });
    assert!(result.unwrap_err().contains("time budget"));
}

#[test]
fn the_sandbox_profile_names_canonical_paths_and_refuses_a_checkout_in_the_store() {
    let profile = sandbox_profile(
        &[Path::new("/a/\"quoted\""), Path::new("/b")],
        Path::new("/s/records"),
    )
    .unwrap();
    assert!(
        profile.contains("(deny file-write* (subpath \"/a/\\\"quoted\\\"\") (subpath \"/b\"))")
    );
    assert!(
        profile.ends_with("(allow file-write* (literal \"/dev/null\") (subpath \"/s/records\"))")
    );
    assert!(sandbox_profile(&[Path::new("/s/records/x")], Path::new("/s/records")).is_err());
    assert!(sandbox_profile(&[Path::new("/a\nb")], Path::new("/s")).is_err());
}

#[test]
fn status_records_parse_with_odd_names() {
    let bytes = b"1 .M N... 100644 100644 100644 aa bb with space\0\
1 A. N... 000000 100644 100644 00 cc staged\0\
1 .D S... 160000 160000 160000 dd dd sub\0\
u UU N... 100644 100644 100644 100644 a b c conflict\0\
? new\nline\0? nested/\0! target/\0! .env\0# header\0";
    assert_eq!(
        parse_status(bytes).unwrap(),
        [
            Entry::Tracked {
                path: b"with space".to_vec(),
                worktree: b'M',
                submodule: false
            },
            Entry::Tracked {
                path: b"staged".to_vec(),
                worktree: b'.',
                submodule: false
            },
            Entry::Tracked {
                path: b"sub".to_vec(),
                worktree: b'D',
                submodule: true
            },
            Entry::Unmerged(b"conflict".to_vec()),
            Entry::Untracked(b"new\nline".to_vec()),
            Entry::Untracked(b"nested/".to_vec()),
            Entry::Ignored(b"target/".to_vec()),
            Entry::Ignored(b".env".to_vec()),
        ]
    );
    for bad in [
        &b"? ../escape\0"[..],
        b"? /abs\0",
        b"9 nonsense\0",
        b"1 .M\0",
    ] {
        assert!(
            parse_status(bad).is_err(),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn diff_records_pair_raw_entries_with_counts() {
    let bytes = b":100644 100755 aa aa M\0run.sh\0:000000 100644 00 bb A\0new file\0\
:100644 000000 cc 00 D\0gone\0:100644 100644 dd ee M\0bin\0\
0\t0\trun.sh\x001\t0\tnew file\x000\t3\tgone\0-\t-\tbin\0";
    let entries = parse_diff(bytes).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].before_mode.as_deref(), Some("100644"));
    assert_eq!(entries[0].after_mode.as_deref(), Some("100755"));
    assert_eq!((entries[0].added, entries[0].removed), (Some(0), Some(0)));
    assert_eq!(entries[1].path, b"new file");
    assert_eq!(entries[1].before_mode, None);
    assert_eq!(entries[2].after_oid, None);
    assert_eq!((entries[2].added, entries[2].removed), (Some(0), Some(3)));
    assert_eq!((entries[3].added, entries[3].removed), (None, None));
    assert!(parse_diff(b":100644 100644 aa bb M\0").is_err());
    assert!(parse_diff(b":100644 bb M\0x\0").is_err());
}

/// The cost of a record of a large checkout. Run by `scripts/bench-records.sh`.
#[test]
#[ignore = "timing run"]
fn record_timing_on_a_large_checkout() {
    let files: usize = std::env::var("PLAIT_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let repo = Repo::new();
    for n in 0..files {
        let dir = repo.0.join(format!("src/d{}/m{}", n / 5000, n / 100));
        if n % 100 == 0 {
            fs::create_dir_all(&dir).unwrap();
        }
        fs::write(
            dir.join(format!("f{n}.rs")),
            format!("// file {n}\nfn f{n}() {{}}\n"),
        )
        .unwrap();
    }
    repo.commit();
    let store = Scratch::new();
    let time = |id: &str, before: Option<&Record>| {
        let start = Instant::now();
        let record = take(&repo, &store, id, before);
        (record, start.elapsed())
    };
    let (_, cold) = time("cold", None);
    let mut warm = Vec::new();
    for n in 0..7 {
        warm.push(time(&format!("warm{n}"), None).1);
    }
    warm.sort();
    let median = warm[warm.len() / 2];
    // A run that changes one file: both records and the changed-path list.
    // The first change after seeding also writes the checkout's trees into the store.
    let run = |n: usize| {
        let (before, first) = time(&format!("run{n}-b"), None);
        repo.write("src/d0/m0/f0.rs", format!("// changed {n}\n"));
        let (after, second) = time(&format!("run{n}-a"), Some(&before));
        assert_eq!(after.paths.as_ref().unwrap().len(), 1);
        (first, second)
    };
    let (first_before, first_after) = run(1);
    let (next_before, next_after) = run(2);
    // A checkout whose index cannot seed the store's is hashed whole.
    repo.git(&["config", "core.autocrlf", "input"]);
    let full = Scratch::new();
    let start = Instant::now();
    take(&repo, &full, "full", None);
    let full_hash = start.elapsed();
    println!(
        "files={files} cold_seeded_ms={} cold_full_hash_ms={} warm_median_ms={} warm_min_ms={} warm_max_ms={} \
         first_run_ms={}+{} later_run_ms={}+{}",
        cold.as_millis(),
        full_hash.as_millis(),
        median.as_millis(),
        warm[0].as_millis(),
        warm[warm.len() - 1].as_millis(),
        first_before.as_millis(),
        first_after.as_millis(),
        next_before.as_millis(),
        next_after.as_millis(),
    );
    assert!(
        median < Duration::from_millis(300),
        "a warm record took {median:?}"
    );
}

#[test]
fn a_clean_oversized_index_entry_is_excluded_from_the_first_record() {
    let repo = Repo::new();
    let file = fs::File::create(repo.0.join("huge.bin")).unwrap();
    file.set_len(FILE_LIMIT + 1).unwrap();
    repo.commit();
    let store = Scratch::new();
    let before = untouched(&[&repo.0], || take(&repo, &store, "b", None));
    assert_eq!(
        uncovered_list(&before),
        [("huge.bin".into(), Reason::TooLarge)]
    );
    assert!(held(&store.0, &locate(&repo.0).unwrap(), &before, "huge.bin").is_none());
}

#[test]
fn ignored_files_that_grow_still_obey_the_total_budget() {
    let repo = Repo::new();
    repo.write(".gitignore", "*.bin\n");
    repo.commit();
    for n in 0..65 {
        repo.write(&format!("{n:03}.bin"), b"small");
    }
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    for n in 0..65 {
        fs::File::options()
            .write(true)
            .open(repo.0.join(format!("{n:03}.bin")))
            .unwrap()
            .set_len(IGNORED_FILE_LIMIT)
            .unwrap();
    }
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(
        uncovered_list(&after),
        [("064.bin".into(), Reason::IgnoredLimit)]
    );
    assert_eq!(after.paths.as_ref().unwrap().len(), 64);
    assert!(held(&store.0, &locate(&repo.0).unwrap(), &after, "064.bin").is_none());
}

#[test]
fn a_full_hash_keeps_force_tracked_ignored_files_and_excludes_uninitialized_submodules() {
    let repo = committed();
    let head = person_head(&repo.0).unwrap().0.unwrap();
    repo.write(".gitignore", "*.cfg\n");
    repo.write("kept.cfg", "kept\r\n");
    repo.git(&["add", "-f", "kept.cfg", ".gitignore"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{},module", head.trim()),
    ]);
    repo.git(&["commit", "-qm", "gitlink"]);
    repo.git(&["config", "core.autocrlf", "true"]);
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    assert_eq!(
        held(&store.0, &locate(&repo.0).unwrap(), &before, "kept.cfg")
            .unwrap()
            .1,
        b"kept\r\n"
    );
    assert!(uncovered_list(&before).contains(&("module/".into(), Reason::Submodule)));
}

#[test]
fn pruning_the_latest_pair_releases_the_checkout_cache_and_initial_pack() {
    let repo = Repo::new();
    for n in 0..BULK_PATHS {
        repo.write(&format!("f{n}"), format!("content {n}"));
    }
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    let after = take(&repo, &store, "a", Some(&before));
    let blob = held(&store.0, &locate(&repo.0).unwrap(), &after, "f0")
        .unwrap()
        .1;
    assert_eq!(blob, b"content 0");
    prune(&PruneRequest {
        store: store.0.clone(),
        remove: vec!["b".into(), "a".into()],
    })
    .unwrap();
    assert!(!owned(&store.0, "sha1", &after.commit));
    assert!(sources(&store.0).is_empty());
    let packs = records_dir(&store.0).join("sha1/objects/pack");
    assert_eq!(fs::read_dir(packs).unwrap().count(), 0);
    assert_eq!(take(&repo, &store, "next", None).tree, after.tree);
}

#[test]
fn a_new_ignore_rule_rechecks_files_already_in_the_private_index() {
    let repo = committed();
    fs::create_dir(repo.0.join("cache")).unwrap();
    repo.write("cache/secret", "private\n");
    repo.write("large.env", vec![b'x'; IGNORED_FILE_LIMIT as usize + 1]);
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    repo.write(".gitignore", "cache/\n*.env\n");
    let after = take(&repo, &store, "a", Some(&before));
    assert!(uncovered_list(&after).contains(&("cache/".into(), Reason::IgnoredDirectory)));
    assert!(uncovered_list(&after).contains(&("large.env".into(), Reason::IgnoredLarge)));
    assert_eq!(summary(&after), ["A .gitignore +2 -0"]);
    let located = locate(&repo.0).unwrap();
    assert!(held(&store.0, &located, &after, "cache/secret").is_none());
    assert!(held(&store.0, &located, &after, "large.env").is_none());
}

#[test]
fn a_directory_that_becomes_a_nested_repository_is_no_longer_covered() {
    let repo = committed();
    fs::create_dir(repo.0.join("vendor")).unwrap();
    repo.write("vendor/lib", "old\n");
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    repo.git(&["init", "-q", "vendor"]);
    let after = take(&repo, &store, "a", Some(&before));
    assert!(uncovered_list(&after).contains(&("vendor/".into(), Reason::NestedRepository)));
    assert!(summary(&after).is_empty());
    assert!(held(&store.0, &locate(&repo.0).unwrap(), &after, "vendor/lib").is_none());
}

#[test]
fn private_attributes_override_checkout_info_attributes_and_lfs() {
    let repo = Repo::new();
    repo.write(
        ".gitattributes",
        "*.bin filter=lfs diff=lfs -text\n*.txt text eol=lf\n",
    );
    repo.git(&[
        "config",
        "filter.lfs.clean",
        "printf 'version https://git-lfs.github.com/spec/v1\\noid sha256:fixture\\nsize 7\\n'",
    ]);
    repo.git(&["config", "filter.lfs.required", "true"]);
    repo.write("actual.bin", b"\0actual bytes\xff");
    repo.write("lines.txt", b"a\r\nb\r\n");
    fs::write(repo.0.join(".git/info/attributes"), "*.txt text eol=crlf\n").unwrap();
    repo.commit();
    let store = Scratch::new();
    let before = untouched(&[&repo.0], || take(&repo, &store, "b", None));
    let located = locate(&repo.0).unwrap();
    assert_eq!(
        held(&store.0, &located, &before, "actual.bin").unwrap().1,
        b"\0actual bytes\xff"
    );
    assert_eq!(
        held(&store.0, &located, &before, "lines.txt").unwrap().1,
        b"a\r\nb\r\n"
    );
}

#[test]
fn split_and_assume_unchanged_indexes_do_not_hide_working_bytes() {
    for split in [false, true] {
        let repo = committed();
        repo.write(".gitignore", "*.cfg\n");
        repo.write("kept.cfg", "first\n");
        repo.git(&["add", "-f", "kept.cfg", ".gitignore"]);
        repo.git(&["commit", "-qm", "ignored exception"]);
        if split {
            repo.git(&["update-index", "--split-index"]);
        } else {
            repo.git(&["update-index", "--assume-unchanged", "kept.cfg"]);
        }
        repo.write("kept.cfg", "changed\n");
        let store = Scratch::new();
        let before = untouched(&[&repo.0], || take(&repo, &store, "b", None));
        assert_eq!(
            held(&store.0, &locate(&repo.0).unwrap(), &before, "kept.cfg")
                .unwrap()
                .1,
            b"changed\n"
        );
    }
}

#[test]
fn a_directory_replaced_by_an_uninitialized_gitlink_loses_its_old_entries() {
    let repo = committed();
    fs::create_dir(repo.0.join("module")).unwrap();
    repo.write("module/old", "before\n");
    repo.commit();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    let head = person_head(&repo.0).unwrap().0.unwrap();
    repo.git(&["rm", "--cached", "module/old"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},module"),
    ]);
    let after = take(&repo, &store, "a", Some(&before));
    assert!(uncovered_list(&after).contains(&("module/".into(), Reason::Submodule)));
    assert!(summary(&after).is_empty());
    assert!(held(&store.0, &locate(&repo.0).unwrap(), &after, "module/old").is_none());
}

#[test]
fn coverage_lists_every_omitted_file_even_past_ten_thousand() {
    let repo = Repo::new();
    repo.write(".gitignore", "*.ignored\n");
    repo.commit();
    let total = IGNORED_COUNT_LIMIT + 10_002;
    for n in 0..total {
        repo.write(&format!("{n:05}.ignored"), b"x");
    }
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    assert_eq!(before.not_covered_total, 10_002);
    assert_eq!(before.not_covered.len(), before.not_covered_total);
    assert_eq!(
        before.not_covered.last().unwrap().path,
        format!("{:05}.ignored", total - 1)
    );
    fs::remove_file(repo.0.join("00000.ignored")).unwrap();
    let after = take(&repo, &store, "a", Some(&before));
    assert_eq!(summary(&after), ["D 00000.ignored +0 -1"]);
    assert_eq!(after.not_covered.len(), 10_002);
}

#[test]
fn configured_excludes_and_info_exclude_are_copied_with_their_precedence() {
    let repo = committed();
    repo.write("personal-ignore", "*.local\n*.keep\n");
    repo.git(&["config", "core.excludesFile", "personal-ignore"]);
    fs::write(repo.0.join(".git/info/exclude"), "!*.keep\n*.private\n").unwrap();
    repo.write(
        "settings.local",
        vec![b'x'; IGNORED_FILE_LIMIT as usize + 1],
    );
    repo.write("settings.keep", vec![b'x'; IGNORED_FILE_LIMIT as usize + 1]);
    repo.write(
        "settings.private",
        vec![b'x'; IGNORED_FILE_LIMIT as usize + 1],
    );
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    assert_eq!(
        uncovered_list(&before),
        [
            ("settings.local".into(), Reason::IgnoredLarge),
            ("settings.private".into(), Reason::IgnoredLarge),
        ]
    );
    assert!(
        held(
            &store.0,
            &locate(&repo.0).unwrap(),
            &before,
            "settings.keep"
        )
        .is_some()
    );
}

#[test]
fn retention_never_drops_private_copies_of_borrowed_before_blobs() {
    let repo = committed();
    let store = Scratch::new();
    let before = take(&repo, &store, "b", None);
    repo.write("keep.txt", "changed\n");
    let after = take(&repo, &store, "a", Some(&before));
    let oid = after.paths.as_ref().unwrap()[0]
        .before_oid
        .as_ref()
        .unwrap();
    assert!(owned(&store.0, "sha1", oid));
    take(&repo, &store, "expired", None);
    take(&repo, &store, "latest", None);
    prune(&PruneRequest {
        store: store.0.clone(),
        remove: vec!["expired".into()],
    })
    .unwrap();
    assert!(
        owned(&store.0, "sha1", oid),
        "retention dropped the host's copy of earlier content"
    );
    fs::remove_dir_all(repo.0.join(".git/objects")).unwrap();
    let (ok, bytes) = store_git(&store.0, "sha1", None, &["cat-file", "blob", oid]);
    assert!(ok);
    assert_eq!(bytes, b"keep\n");
}

#[test]
fn equivalent_names_are_rejected() {
    let repo = Repo::new();
    repo.write("cafe\u{301}", "before");
    let aliases = vec![
        "cafe\u{301}".as_bytes().to_vec(),
        "caf\u{e9}".as_bytes().to_vec(),
    ];
    if repo.0.join("caf\u{e9}").exists() {
        assert!(reject_aliases(&repo.0, &aliases, &mut HashMap::new()).is_err());
    }
}

#[test]
fn recorded_diffs_read_owned_blobs_without_trees_or_current_files() {
    use crate::git::review;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let repo = Repo::new();
    let store = Scratch::new();
    repo.write("text", b"old\r\nlast");
    repo.write("deleted", "gone\n");
    repo.write("mode", "same\n");
    repo.write("binary", b"old\0");
    symlink("old-target", repo.0.join("link")).unwrap();
    repo.commit();
    let before = take(&repo, &store, "before", None);
    repo.write("text", b"new\r\nlast");
    repo.write("created", "new file\n");
    repo.write("binary", b"new\0");
    fs::remove_file(repo.0.join("deleted")).unwrap();
    fs::remove_file(repo.0.join("link")).unwrap();
    symlink("new-target", repo.0.join("link")).unwrap();
    fs::set_permissions(repo.0.join("mode"), fs::Permissions::from_mode(0o755)).unwrap();
    let after = take(&repo, &store, "after", Some(&before));
    let root = repo.0.canonicalize().unwrap();
    let metadata = fs::metadata(&root).unwrap();
    let read = |changed: &ChangedPath| {
        review::read(&review::Request {
            store: store.0.clone(),
            checkout: root.clone(),
            dev: metadata.dev(),
            ino: metadata.ino(),
            object_format: "sha1".into(),
            changed: changed.clone(),
        })
    };
    // Remove the source objects entirely. Unchanged borrowed trees cannot be read now.
    fs::rename(repo.0.join(".git/objects"), repo.0.join("removed-objects")).unwrap();
    for changed in after.paths.as_ref().unwrap() {
        let result = untouched(&[&repo.0], || read(changed).unwrap());
        let patch = String::from_utf8(result.patch).unwrap();
        match changed.path.as_str() {
            "text" => {
                assert!(patch.contains("-old\r\n+new\r\n"), "{patch}");
                assert!(patch.contains("\\ No newline"));
                assert_eq!(result.processes, 2);
            }
            "created" => {
                assert!(result.before.is_none());
                assert!(patch.contains("new file mode"));
            }
            "deleted" => {
                assert!(result.after.is_none());
                assert!(patch.contains("deleted file mode"));
            }
            "mode" => {
                assert_eq!(result.status, "mode");
                assert!(patch.contains("old mode 100644\nnew mode 100755"));
            }
            "binary" => assert_eq!(result.status, "binary"),
            "link" => assert!(patch.contains("+new-target")),
            other => panic!("unexpected path {other}"),
        }
        assert!(!result.hunks.is_empty());
    }
    let mut changed = after.paths.unwrap().remove(0);
    changed.before_oid = Some("f".repeat(40));
    changed.before_mode = Some("100644".into());
    assert!(read(&changed).is_err());
}

#[test]
fn recorded_review_paths_reject_metadata_and_traversal() {
    for path in [
        "",
        "/a",
        "a//b",
        "a/../b",
        ".git/config",
        "a/.GIT/objects",
        "a\0b",
        "./x",
        "a\nb",
    ] {
        assert!(!crate::git::review::valid_path(path), "{path}");
    }
    assert!(crate::git::review::valid_path("src/a b.rs"));
}

fn read_recorded_path(
    repo: &Repo,
    store: &Scratch,
    changed: &ChangedPath,
) -> crate::git::review::Read {
    use std::os::unix::fs::MetadataExt;
    let checkout = repo.0.canonicalize().unwrap();
    let metadata = fs::metadata(&checkout).unwrap();
    crate::git::review::read(&crate::git::review::Request {
        store: store.0.clone(),
        checkout,
        dev: metadata.dev(),
        ino: metadata.ino(),
        object_format: "sha1".into(),
        changed: changed.clone(),
    })
    .unwrap()
}

#[test]
fn recorded_diff_limit_drops_partial_headers_and_hunks() {
    let repo = Repo::new();
    let store = Scratch::new();
    let path = "x".repeat(212);
    repo.write(&path, "old\na\nb\nc\nd\ne\nf\ng\ntail\n");
    let before = take(&repo, &store, "before", None);
    // Put the second hunk header across the synthesized patch's byte limit.
    let prefix = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,4 +1,4 @@\n-old\n+"
    );
    let length = crate::git::DIFF_LIMIT - 8 - prefix.len() - "\n a\n b\n c\n".len();
    for (index, extra) in [0, -10, 100].into_iter().enumerate() {
        let text = "x".repeat(length.checked_add_signed(extra).unwrap());
        repo.write(&path, format!("{text}\na\nb\nc\nd\ne\nf\ng\nend\n"));
        let after = take(&repo, &store, &format!("after-{index}"), Some(&before));
        let read = read_recorded_path(&repo, &store, &after.paths.unwrap()[0]);
        assert_eq!(read.status, "ready");
        assert!(read.truncated);
        assert!(read.patch.len() <= crate::git::DIFF_LIMIT);
        assert_eq!(read.patch.last(), Some(&b'\n'));
        let patch = String::from_utf8(read.patch).unwrap();
        assert!(
            !patch.contains("@@ -6"),
            "partial second hunk must be omitted"
        );
        if extra == 0 {
            assert_eq!(read.hunks.len(), 1);
            assert_eq!(read.hunks[0].old_count, 4);
            assert!(patch.ends_with(" c\n"));
        }
    }
}

#[test]
fn recorded_empty_file_creation_and_deletion_are_not_mode_changes() {
    let repo = Repo::new();
    let store = Scratch::new();
    repo.write("deleted", "");
    repo.write("mode", "");
    let before = take(&repo, &store, "before", None);
    repo.write("created", "");
    fs::remove_file(repo.0.join("deleted")).unwrap();
    fs::set_permissions(repo.0.join("mode"), fs::Permissions::from_mode(0o755)).unwrap();
    let after = take(&repo, &store, "after", Some(&before));
    for changed in after.paths.unwrap() {
        let read = read_recorded_path(&repo, &store, &changed);
        let patch = String::from_utf8(read.patch).unwrap();
        match changed.path.as_str() {
            "created" => {
                assert_eq!(read.status, "ready");
                assert!(read.before.is_none());
                assert_eq!(read.after.unwrap().bytes, 0);
                assert!(patch.contains("new file mode 100644\n--- /dev/null\n+++ b/created\n"));
            }
            "deleted" => {
                assert_eq!(read.status, "ready");
                assert!(read.after.is_none());
                assert_eq!(read.before.unwrap().bytes, 0);
                assert!(patch.contains("deleted file mode 100644\n--- a/deleted\n+++ /dev/null\n"));
            }
            "mode" => {
                assert_eq!(read.status, "mode");
                assert!(patch.contains("old mode 100644\nnew mode 100755\n"));
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn recorded_latin1_text_uses_the_recorders_line_counts() {
    let repo = Repo::new();
    let store = Scratch::new();
    repo.write("latin1", b"caf\xe9\n");
    repo.write("binary", b"old\0\n");
    let before = take(&repo, &store, "before", None);
    repo.write("latin1", b"caf\xe8\n");
    repo.write("binary", b"new\0\n");
    let after = take(&repo, &store, "after", Some(&before));
    for changed in after.paths.unwrap() {
        let read = read_recorded_path(&repo, &store, &changed);
        if changed.path == "latin1" {
            assert_eq!((changed.added, changed.removed), (Some(1), Some(1)));
            assert_eq!(read.status, "ready");
            assert!(read.patch.windows(12).any(|s| s == b"-caf\xe9\n+caf\xe8\n"));
            let diff = crate::git::recorded_diff(&read.patch, false, std::time::Duration::ZERO);
            assert!(diff.lines.iter().any(|l| l.text.contains("+caf\u{fffd}")));
        } else {
            assert_eq!((changed.added, changed.removed), (None, None));
            assert_eq!(read.status, "binary");
        }
    }
}
