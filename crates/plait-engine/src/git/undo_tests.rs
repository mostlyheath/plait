use super::*;
use crate::git::tests::Repo;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture {
    repo: Repo,
    store: Repo,
    located: Located,
}
impl Fixture {
    fn new() -> Self {
        let repo = Repo::new();
        let store = Repo::new();
        let located = record::locate(&repo.0).unwrap();
        Self {
            repo,
            store,
            located,
        }
    }
    fn take(&self, id: &str, before: Option<&Record>) -> Record {
        record::record(
            &self.located,
            &record::Request {
                checkout: self.located.root.clone(),
                store: self.store.0.clone(),
                id: id.into(),
                before: before.map(|b| b.commit.clone()),
            },
        )
        .unwrap()
    }
    fn request(&self, before: &Record, after: &Record, id: &str, leave: bool) -> Request {
        let meta = fs::metadata(&self.located.root).unwrap();
        Request {
            checkout: self.located.root.clone(),
            store: self.store.0.clone(),
            before: before.commit.clone(),
            after: after.commit.clone(),
            before_id: format!("{id}-before"),
            id: id.into(),
            paths: after.paths.clone(),
            not_covered: Some(after.not_covered.clone()),
            identity: Identity {
                dev: meta.dev().to_string(),
                ino: meta.ino().to_string(),
            },
            leave_changed: leave,
        }
    }
    fn undo(&self, before: &Record, after: &Record, id: &str, leave: bool) -> Outcome {
        let previous = files(&self.repo.0);
        let result = undo(&self.located, &self.request(before, after, id, leave)).unwrap();
        let now = files(&self.repo.0);
        let paths = after.paths.as_ref().unwrap();
        let changed: BTreeSet<_> = paths
            .iter()
            .filter(|p| result.state == "undone" && !result.changed_since.contains(&p.path))
            .map(|p| PathBuf::from(&p.path))
            .collect();
        for (path, meta) in &previous {
            if !changed.contains(path) {
                assert_eq!(
                    now.get(path),
                    Some(meta),
                    "Unrelated path changed: {}",
                    path.display()
                );
            }
        }
        for path in now.keys() {
            assert!(
                previous.contains_key(path) || changed.contains(path),
                "Unexpected path: {}",
                path.display()
            );
        }
        let reader = Reader::open(&self.located, &self.store.0).unwrap();
        for path in paths
            .iter()
            .filter(|p| changed.contains(Path::new(&p.path)))
        {
            assert_eq!(
                current(
                    &root(&self.request(before, after, id, leave)).unwrap(),
                    &path.path
                )
                .unwrap(),
                fingerprint_blob(
                    &reader,
                    path.before_mode.as_deref(),
                    path.before_oid.as_deref()
                )
                .unwrap(),
                "Before bytes/mode: {}",
                path.path
            );
        }
        result
    }
}

#[test]
fn exact_bytes_modes_links_and_undo_of_undo() {
    let f = Fixture::new();
    let names = [
        "space name",
        "line\nbreak",
        "日本語",
        "-leading",
        "binary",
        "mode",
        "deleted",
    ];
    for name in names {
        f.repo.write(name, b"old\0\xff\r\n");
    }
    f.repo.write(".gitignore", "local.env\n");
    f.repo.write("local.env", "old ignored");
    symlink("old-target", f.repo.0.join("link")).unwrap();
    f.repo.commit();
    f.repo.write("unrelated", "untouched");
    let before = f.take("before", None);
    for name in &names[..5] {
        f.repo.write(name, b"new\0\xfe");
    }
    fs::set_permissions(f.repo.0.join("mode"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_file(f.repo.0.join("deleted")).unwrap();
    f.repo.write("local.env", "new ignored");
    fs::remove_file(f.repo.0.join("link")).unwrap();
    symlink("new-target", f.repo.0.join("link")).unwrap();
    fs::create_dir_all(f.repo.0.join("created/deep")).unwrap();
    f.repo.write("created/deep/file", "created");
    let after = f.take("after", Some(&before));
    let reverted = f.undo(&before, &after, "undo", false);
    assert_eq!(reverted.state, "undone", "{:?}", reverted.error);
    for name in names {
        assert_eq!(fs::read(f.repo.0.join(name)).unwrap(), b"old\0\xff\r\n");
    }
    assert_eq!(
        fs::read_link(f.repo.0.join("link")).unwrap(),
        Path::new("old-target")
    );
    assert_eq!(
        fs::read(f.repo.0.join("local.env")).unwrap(),
        b"old ignored"
    );
    assert_eq!(
        fs::metadata(f.repo.0.join("mode")).unwrap().mode() & 0o111,
        0
    );
    assert!(!f.repo.0.join("created").exists());
    assert_eq!(fs::read(f.repo.0.join("unrelated")).unwrap(), b"untouched");
    let redone = f.undo(
        &reverted.before,
        reverted.after.as_ref().unwrap(),
        "redo",
        false,
    );
    assert_eq!(redone.state, "undone", "{:?}", redone.error);
    assert_eq!(redone.after.unwrap().tree, after.tree);
}

#[test]
fn changed_since_blocks_everything_or_leaves_changed() {
    let f = Fixture::new();
    f.repo.write("a", "original");
    f.repo.write("b", "original");
    let before = f.take("b", None);
    f.repo.write("a", "run");
    f.repo.write("b", "run");
    let after = f.take("a", Some(&before));
    f.repo.write("a", "person or later run");
    let blocked = f.undo(&before, &after, "blocked", false);
    assert_eq!(blocked.state, "blocked");
    assert_eq!(blocked.changed_since, ["a"]);
    assert_eq!(fs::read(f.repo.0.join("b")).unwrap(), b"run");
    let rest = f.undo(&before, &after, "rest", true);
    assert_eq!(rest.state, "undone");
    assert_eq!(rest.changed_since, ["a"]);
    assert_eq!(
        fs::read(f.repo.0.join("a")).unwrap(),
        b"person or later run"
    );
    assert_eq!(fs::read(f.repo.0.join("b")).unwrap(), b"original");
}

#[test]
fn structural_changes_refuse_nonempty_directories_and_file_ancestors() {
    for file_first in [true, false] {
        let f = Fixture::new();
        if file_first {
            f.repo.write("item", "before");
        } else {
            fs::create_dir(f.repo.0.join("item")).unwrap();
            f.repo.write("item/child", "before");
        }
        let before = f.take("b", None);
        if file_first {
            fs::remove_file(f.repo.0.join("item")).unwrap();
            fs::create_dir(f.repo.0.join("item")).unwrap();
            f.repo.write("item/child", "after");
        } else {
            fs::remove_dir_all(f.repo.0.join("item")).unwrap();
            f.repo.write("item", "after");
        }
        let after = f.take("a", Some(&before));
        let result = f.undo(&before, &after, "u", false);
        assert_eq!(result.state, "blocked");
        assert!(
            result
                .changed_since
                .contains(&if file_first { "item" } else { "item/child" }.into())
        );
        assert_eq!(f.take("unchanged", None).tree, after.tree);
    }
}

#[test]
fn symlink_parent_and_swap_before_rename_never_follow_the_link() {
    for swap in [false, true] {
        let f = Fixture::new();
        let outside = Repo::new();
        outside.write("file", "outside");
        fs::create_dir(f.repo.0.join("dir")).unwrap();
        f.repo.write("dir/file", "before");
        let before = f.take("b", None);
        f.repo.write("dir/file", "after");
        let after = f.take("a", Some(&before));
        let replace = || {
            fs::rename(f.repo.0.join("dir"), f.repo.0.join("moved")).unwrap();
            symlink(&outside.0, f.repo.0.join("dir")).unwrap();
        };
        if !swap {
            replace();
        }
        let result = undo_with(
            &f.located,
            &f.request(&before, &after, "u", false),
            Instant::now() + BUDGET,
            &mut |phase, _| {
                if swap && phase == "rename" {
                    replace();
                }
            },
        )
        .unwrap();
        assert_eq!(result.state, if swap { "incomplete" } else { "blocked" });
        assert_eq!(fs::read(outside.0.join("file")).unwrap(), b"outside");
        assert_eq!(fs::read(f.repo.0.join("moved/file")).unwrap(), b"after");
    }
}

#[test]
fn file_swapped_before_rename_is_not_overwritten() {
    let f = Fixture::new();
    f.repo.write("file", "before");
    let before = f.take("b", None);
    f.repo.write("file", "after");
    let after = f.take("a", Some(&before));
    let result = undo_with(
        &f.located,
        &f.request(&before, &after, "u", false),
        Instant::now() + BUDGET,
        &mut |_, _| f.repo.write("file", "concurrent"),
    )
    .unwrap();
    assert_eq!(result.state, "incomplete");
    assert_eq!(fs::read(f.repo.0.join("file")).unwrap(), b"concurrent");
}

#[test]
fn created_then_deleted_and_unborn_checkout() {
    let f = Fixture::new();
    let before = f.take("b", None);
    f.repo.write("transient", "gone");
    fs::remove_file(f.repo.0.join("transient")).unwrap();
    let after = f.take("a", Some(&before));
    assert!(after.paths.as_ref().unwrap().is_empty());
    assert_eq!(f.undo(&before, &after, "u", false).state, "undone");
}

#[test]
fn empty_directory_can_be_replaced_but_unrelated_content_cannot() {
    let f = Fixture::new();
    f.repo.write("file", "before");
    let before = f.take("b", None);
    fs::remove_file(f.repo.0.join("file")).unwrap();
    let after = f.take("a", Some(&before));
    fs::create_dir(f.repo.0.join("file")).unwrap();
    assert_eq!(f.undo(&before, &after, "u", false).state, "undone");
    assert_eq!(fs::read(f.repo.0.join("file")).unwrap(), b"before");
}

#[test]
fn worktree_undo_does_not_move_head_or_touch_main_checkout() {
    let f = Fixture::new();
    f.repo.write("file", "base");
    f.repo.commit();
    let worktree = f.store.0.join("worktree");
    f.repo.git(&[
        "worktree",
        "add",
        "-b",
        "thread",
        worktree.to_str().unwrap(),
    ]);
    let located = record::locate(&worktree).unwrap();
    let take = |id: &str, before: Option<String>| {
        record::record(
            &located,
            &record::Request {
                checkout: located.root.clone(),
                store: f.store.0.join("private"),
                id: id.into(),
                before,
            },
        )
        .unwrap()
    };
    let before = take("b", None);
    fs::write(worktree.join("file"), "run").unwrap();
    let after = take("a", Some(before.commit.clone()));
    let meta = fs::metadata(&worktree).unwrap();
    let result = undo(
        &located,
        &Request {
            checkout: located.root.clone(),
            store: f.store.0.join("private"),
            before: before.commit.clone(),
            after: after.commit.clone(),
            before_id: "ub".into(),
            id: "ua".into(),
            paths: after.paths.clone(),
            not_covered: Some(after.not_covered.clone()),
            identity: Identity {
                dev: meta.dev().to_string(),
                ino: meta.ino().to_string(),
            },
            leave_changed: false,
        },
    )
    .unwrap();
    assert_eq!(result.state, "undone");
    assert_eq!(result.after.unwrap().head, before.head);
    assert_eq!(fs::read(worktree.join("file")).unwrap(), b"base");
    assert_eq!(fs::read(f.repo.0.join("file")).unwrap(), b"base");
}

#[test]
fn case_only_rename_on_case_insensitive_disk_is_blocked() {
    let f = Fixture::new();
    f.repo.write("name", "before");
    let before = f.take("b", None);
    f.repo.write("name", "after");
    let after = f.take("a", Some(&before));
    fs::rename(f.repo.0.join("name"), f.repo.0.join("NAME")).unwrap();
    if !f.repo.0.join("name").exists() {
        return;
    } // A case-sensitive volume has no alias.
    let result = f.undo(&before, &after, "u", false);
    assert_eq!(result.state, "blocked");
    assert_eq!(fs::read(f.repo.0.join("NAME")).unwrap(), b"after");
}

#[test]
fn deadline_before_apply_leaves_files_untouched() {
    let f = Fixture::new();
    f.repo.write("file", "before");
    let before = f.take("b", None);
    f.repo.write("file", "after");
    let after = f.take("a", Some(&before));
    assert!(
        undo_with(
            &f.located,
            &f.request(&before, &after, "u", false),
            Instant::now(),
            &mut |_, _| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(f.repo.0.join("file")).unwrap(), b"after");
}

// All working files plus all Git metadata, including directory identities/times.
type FileState = (u32, u64, i64, i64, i64, i64, u64, Vec<u8>);
fn files(root: &Path) -> BTreeMap<PathBuf, FileState> {
    use std::os::unix::ffi::OsStrExt;
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = fs::symlink_metadata(&path).unwrap();
        let relative = path.strip_prefix(root).unwrap();
        if !meta.is_dir() || relative.starts_with(".git") {
            let content = if meta.is_file() {
                fs::read(&path).unwrap()
            } else if meta.file_type().is_symlink() {
                fs::read_link(&path)
                    .unwrap()
                    .as_os_str()
                    .as_bytes()
                    .to_vec()
            } else {
                Vec::new()
            };
            result.insert(
                relative.to_path_buf(),
                (
                    meta.mode(),
                    meta.len(),
                    meta.mtime(),
                    meta.mtime_nsec(),
                    meta.ctime(),
                    meta.ctime_nsec(),
                    meta.ino(),
                    content,
                ),
            );
        }
        if meta.is_dir() {
            pending.extend(fs::read_dir(&path).unwrap().map(|p| p.unwrap().path()));
        }
    }
    result
}

#[test]
fn mode_and_symlink_changes_since_the_run_are_blocked() {
    let f = Fixture::new();
    f.repo.write("mode", "old");
    symlink("old", f.repo.0.join("link")).unwrap();
    let before = f.take("b", None);
    f.repo.write("mode", "new");
    fs::remove_file(f.repo.0.join("link")).unwrap();
    symlink("new", f.repo.0.join("link")).unwrap();
    let after = f.take("a", Some(&before));
    fs::set_permissions(f.repo.0.join("mode"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_file(f.repo.0.join("link")).unwrap();
    symlink("person", f.repo.0.join("link")).unwrap();
    let result = f.undo(&before, &after, "u", false);
    assert_eq!(result.state, "blocked");
    assert_eq!(result.changed_since, ["link", "mode"]);
}

#[test]
fn a_later_record_does_not_authorize_overwriting_its_changes() {
    let f = Fixture::new();
    f.repo.write("file", "original");
    let before = f.take("b", None);
    f.repo.write("file", "first run");
    let after = f.take("a", Some(&before));
    let next = f.take("nb", None);
    f.repo.write("file", "later run");
    f.take("na", Some(&next));
    assert_eq!(f.undo(&before, &after, "u", false).state, "blocked");
    assert_eq!(fs::read(f.repo.0.join("file")).unwrap(), b"later run");
}

#[test]
fn created_file_is_rechecked_before_deletion() {
    let f = Fixture::new();
    let before = f.take("b", None);
    f.repo.write("created", "run");
    let after = f.take("a", Some(&before));
    let result = undo_with(
        &f.located,
        &f.request(&before, &after, "u", false),
        Instant::now() + BUDGET,
        &mut |phase, _| {
            assert_eq!(phase, "delete");
            f.repo.write("created", "person");
        },
    )
    .unwrap();
    assert_eq!(result.state, "incomplete");
    assert_eq!(fs::read(f.repo.0.join("created")).unwrap(), b"person");
}

#[test]
fn temporary_name_collision_is_never_deleted() {
    let f = Fixture::new();
    f.repo.write("file", "before");
    let before = f.take("b", None);
    f.repo.write("file", "after");
    let after = f.take("a", Some(&before));
    f.repo.write(".plait-undo-u-0", "person");
    let result = f.undo(&before, &after, "u", false);
    assert_eq!(result.state, "incomplete");
    assert_eq!(
        fs::read(f.repo.0.join(".plait-undo-u-0")).unwrap(),
        b"person"
    );
}

#[test]
fn invalid_checkout_identity_refuses_before_writing() {
    let f = Fixture::new();
    f.repo.write("file", "before");
    let before = f.take("b", None);
    f.repo.write("file", "after");
    let after = f.take("a", Some(&before));
    let mut request = f.request(&before, &after, "u", false);
    request.identity.ino = "0".into();
    let old = files(&f.repo.0);
    assert!(undo(&f.located, &request).is_err());
    assert_eq!(files(&f.repo.0), old);
}

#[test]
fn undo_records_only_its_own_completed_writes() {
    let f = Fixture::new();
    f.repo.write("file", "before");
    f.repo.write("other", "person before");
    let before = f.take("b", None);
    f.repo.write("file", "after");
    let after = f.take("a", Some(&before));
    let undone = undo_with(
        &f.located,
        &f.request(&before, &after, "u", false),
        Instant::now() + BUDGET,
        &mut |_, _| {
            f.repo.write("other", "person during undo");
        },
    )
    .unwrap();
    assert_eq!(undone.state, "undone");
    assert_eq!(
        undone
            .after
            .as_ref()
            .unwrap()
            .paths
            .as_ref()
            .unwrap()
            .iter()
            .map(|p| p.path.as_str())
            .collect::<Vec<_>>(),
        ["file"]
    );
    let redo = f.undo(
        &undone.before,
        undone.after.as_ref().unwrap(),
        "redo",
        false,
    );
    assert_eq!(redo.state, "undone");
    assert_eq!(
        fs::read(f.repo.0.join("other")).unwrap(),
        b"person during undo"
    );
    assert_eq!(fs::read(f.repo.0.join("file")).unwrap(), b"after");
}

#[test]
fn failed_revalidation_does_not_claim_the_persons_edit_for_redo() {
    let f = Fixture::new();
    f.repo.write("file", "before");
    let before = f.take("b", None);
    f.repo.write("file", "after");
    let after = f.take("a", Some(&before));
    let undone = undo_with(
        &f.located,
        &f.request(&before, &after, "u", false),
        Instant::now() + BUDGET,
        &mut |_, _| f.repo.write("file", "person"),
    )
    .unwrap();
    assert_eq!(undone.state, "incomplete");
    assert!(
        undone
            .after
            .as_ref()
            .unwrap()
            .paths
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.undo(
            &undone.before,
            undone.after.as_ref().unwrap(),
            "redo",
            false
        )
        .state,
        "undone"
    );
    assert_eq!(fs::read(f.repo.0.join("file")).unwrap(), b"person");
}

#[test]
fn case_only_rename_during_run_is_recorded_and_refused_on_apfs() {
    let f = Fixture::new();
    f.repo.write("name", "same bytes");
    f.repo.commit();
    let before = f.take("b", None);
    fs::rename(f.repo.0.join("name"), f.repo.0.join("NAME")).unwrap();
    let after = f.take("a", Some(&before));
    assert_eq!(
        after.paths.as_ref().unwrap().len(),
        2,
        "case-only rename must not disappear from the record"
    );
    let result = f.undo(&before, &after, "u", false);
    if f.repo.0.join("name").exists()
        && fs::read_dir(&f.repo.0)
            .unwrap()
            .any(|e| e.unwrap().file_name() == "NAME")
    {
        assert_eq!(result.state, "blocked");
        assert!(result.changed_since.contains(&"name".into()));
    } else {
        assert_eq!(result.state, "undone");
    }
}

#[test]
fn changed_files_restore_without_borrowed_unchanged_blobs() {
    let f = Fixture::new();
    fs::create_dir(f.repo.0.join("src")).unwrap();
    fs::create_dir(f.repo.0.join("other")).unwrap();
    f.repo.write("src/file", "before");
    f.repo.write("other/keep", "untouched borrowed blob");
    f.repo.commit();
    let before = f.take("b", None);
    f.repo.write("src/file", "after");
    let after = f.take("a", Some(&before));
    let entries = Reader::open(&f.located, &f.store.0)
        .unwrap()
        .entries(&before.commit)
        .unwrap();
    let oid = &entries["other/keep"].1;
    fs::remove_file(
        f.repo
            .0
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..]),
    )
    .unwrap();
    assert_eq!(f.undo(&before, &after, "u", false).state, "undone");
    assert_eq!(fs::read(f.repo.0.join("src/file")).unwrap(), b"before");
    assert_eq!(
        fs::read(f.repo.0.join("other/keep")).unwrap(),
        b"untouched borrowed blob"
    );
}

#[test]
fn undo_rest_never_deletes_objects_needed_by_a_later_nested_commit() {
    let f = Fixture::new();
    f.repo.git(&["init", "--bare", "-q", "nested.git"]);
    // A push drops the fixtures' environment settings for the repository that
    // receives it, so that repository's own config turns maintenance off.
    for (key, value) in [
        ("maintenance.auto", "false"),
        ("gc.auto", "0"),
        ("receive.autoGC", "false"),
    ] {
        f.repo.git(&["--git-dir=nested.git", "config", key, value]);
    }
    let nested = f.repo.0.join("nested.git");
    let producer = Repo::new();
    let before = f.take("before", None);
    producer.write("first", "valuable first\n");
    producer.commit();
    producer.git(&[
        "push",
        "-q",
        nested.to_str().unwrap(),
        "HEAD:refs/heads/main",
    ]);
    let after = f.take("after", Some(&before));
    producer.write("second", "later person work\n");
    producer.commit();
    producer.git(&[
        "push",
        "-q",
        nested.to_str().unwrap(),
        "HEAD:refs/heads/main",
    ]);
    let original = files(&nested);
    assert!(after.paths.as_ref().unwrap().is_empty());
    assert_eq!(f.undo(&before, &after, "undo", true).state, "undone");
    assert_eq!(files(&nested), original);
    f.repo.git(&["--git-dir=nested.git", "fsck", "--full"]);
}

#[test]
fn a_git_directory_appearing_after_capture_blocks_old_record_paths() {
    let f = Fixture::new();
    fs::create_dir(f.repo.0.join("metadata")).unwrap();
    f.repo.write("metadata/file", "before");
    let before = f.take("before", None);
    f.repo.write("metadata/file", "after");
    let after = f.take("after", Some(&before));
    f.repo.git(&["init", "--bare", "-q", "metadata"]);
    let outcome = f.undo(&before, &after, "undo", true);
    assert_eq!(outcome.changed_since, ["metadata/file"]);
    assert_eq!(outcome.applied, 0);
}

#[test]
fn making_an_unreadable_directory_readable_does_not_authorize_deleting_its_files() {
    let f = Fixture::new();
    let directory = f.repo.0.join("secret");
    fs::create_dir(&directory).unwrap();
    f.repo.write("secret/keep", "valuable untouched\n");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o000)).unwrap();
    let before = f.take("before", None);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let after = f.take("after", Some(&before));
    let outcome = f.undo(&before, &after, "undo", true);
    assert_eq!(outcome.applied, 0);
    assert_eq!(
        fs::read(directory.join("keep")).unwrap(),
        b"valuable untouched\n"
    );
}

#[test]
fn decomposed_names_have_one_entry_and_default_undo_restores_them() {
    let f = Fixture::new();
    let name = "cafe\u{301}.txt";
    f.repo.write(name, "before\n");
    f.repo.commit();
    let before = f.take("b", None);
    let reader = Reader::open(&f.located, &f.store.0).unwrap();
    let entries = reader.entries(&before.commit).unwrap();
    assert_eq!(entries.keys().collect::<Vec<_>>(), [&name.to_string()]);
    f.repo.write(name, "after\n");
    let after = f.take("a", Some(&before));
    assert_eq!(reader.entries(&after.commit).unwrap().len(), 1);
    assert_eq!(after.paths.as_ref().unwrap().len(), 1);
    let undone = f.undo(&before, &after, "u", false);
    assert_eq!(undone.state, "undone");
    assert_eq!(fs::read_to_string(f.repo.0.join(name)).unwrap(), "before\n");
}
