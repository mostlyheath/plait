use super::*;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

/// Git starts detached maintenance after a commit or a push. It briefly holds
/// a lock under `.git/objects`, which a before-and-after comparison of a
/// repository can catch. Fixtures turn it off, as the recorder's own commands do.
pub(super) const NO_MAINTENANCE: [(&str, &str); 7] = [
    ("GIT_CONFIG_COUNT", "3"),
    ("GIT_CONFIG_KEY_0", "maintenance.auto"),
    ("GIT_CONFIG_VALUE_0", "false"),
    ("GIT_CONFIG_KEY_1", "gc.auto"),
    ("GIT_CONFIG_VALUE_1", "0"),
    ("GIT_CONFIG_KEY_2", "receive.autoGC"),
    ("GIT_CONFIG_VALUE_2", "false"),
];

pub(super) struct Repo(pub(super) PathBuf);
impl Repo {
    pub(super) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "plait-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let repo = Self(path);
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "Plait Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }
    pub(super) fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .envs(NO_MAINTENANCE)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    pub(super) fn write(&self, path: &str, text: impl AsRef<[u8]>) {
        fs::write(self.0.join(path), text).unwrap();
    }
    pub(super) fn commit(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "-m", "fixture"]);
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(diff: &Diff) -> String {
    diff.lines
        .iter()
        .map(|line| &*line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn branch_lines_report_upstream_and_counts() {
    let tracking = |upstream: &str, ahead, behind, gone| {
        Some(Tracking {
            upstream: upstream.into(),
            ahead,
            behind,
            gone,
        })
    };
    let line = parse_branch("main...origin/main [ahead 2, behind 13]");
    assert_eq!(line.branch, "main");
    assert!(line.named);
    assert_eq!(line.tracking, tracking("origin/main", 2, 13, false));
    assert_eq!(
        parse_branch("fix/a.b...up/fix/a.b [behind 1]").tracking,
        tracking("up/fix/a.b", 0, 1, false)
    );
    assert_eq!(
        parse_branch("main...origin/main").tracking,
        tracking("origin/main", 0, 0, false)
    );
    assert_eq!(
        parse_branch("main...origin/main [gone]").tracking,
        tracking("origin/main", 0, 0, true)
    );
    let local = parse_branch("topic");
    assert!(local.named && local.tracking.is_none());
    let unborn = parse_branch("No commits yet on main");
    assert_eq!(unborn.branch, "main");
    assert!(!unborn.named && unborn.tracking.is_none());
    let detached = parse_branch("HEAD (no branch)");
    assert!(!detached.named && detached.tracking.is_none());
}

#[test]
fn porcelain_preserves_nul_delimited_names_and_rename_order() {
    let (_, changes) = parse_status(
        b"## main\0 M spaces and\nnewlines.rs\0R  destination\0source name\0?? -odd\0",
    )
    .unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].path, PathBuf::from("spaces and\nnewlines.rs"));
    assert_eq!(changes[1].path, PathBuf::from("destination"));
    assert_eq!(changes[1].original, Some(PathBuf::from("source name")));
    assert_eq!(changes[0].display_path(), "spaces and\\nnewlines.rs");
    assert!(parse_status(b"## main\0R  incomplete\0").is_err());
    assert!(parse_status(b"## main\0?? ../outside\0").is_err());
    assert!(parse_status(b"## main\0?? /outside\0").is_err());
}

#[test]
fn unborn_clean_and_non_repository_states() {
    let repo = Repo::new();
    let initial = status(&repo.0).unwrap();
    assert!(initial.branch.contains("main"));
    assert!(initial.changes.is_empty());
    repo.write("hello.txt", "hello\n");
    let snapshot = status(&repo.0).unwrap();
    assert_eq!(snapshot.changes[0].label(), "New");
    assert!(
        text(&diff(&snapshot.root, &snapshot.changes[0], DiffMode::Unstaged).unwrap())
            .contains("+hello")
    );
    repo.commit();
    assert!(status(&repo.0).unwrap().changes.is_empty());
    let outside = repo.0.join("not-a-repository");
    fs::create_dir(&outside).unwrap();
    assert!(status(&repo.0.join("missing")).is_err());
    assert!(status(Path::new("/dev/null")).is_err());
}

#[test]
fn staged_and_unstaged_diffs_are_separate_and_reads_preserve_index() {
    let repo = Repo::new();
    repo.write("file.txt", "base\n");
    repo.commit();
    repo.write("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write("file.txt", "working\n");
    let index_before = fs::read(repo.0.join(".git/index")).unwrap();
    let snapshot = status(&repo.0).unwrap();
    let change = &snapshot.changes[0];
    assert!(change.staged() && change.unstaged());
    let staged = text(&diff(&snapshot.root, change, DiffMode::Staged).unwrap());
    let working = text(&diff(&snapshot.root, change, DiffMode::Unstaged).unwrap());
    assert!(staged.contains("-base") && staged.contains("+staged"));
    assert!(working.contains("-staged") && working.contains("+working"));
    assert!(!staged.contains("+working"));
    assert_eq!(index_before, fs::read(repo.0.join(".git/index")).unwrap());
    assert_eq!(
        fs::read_to_string(repo.0.join("file.txt")).unwrap(),
        "working\n"
    );
    assert!(!repo.0.join(".git/index.lock").exists());
}

#[test]
fn rename_deletion_binary_and_literal_pathspecs() {
    let repo = Repo::new();
    for path in ["old name", "deleted", ":(glob)*", "other"] {
        repo.write(path, "base\n");
    }
    repo.commit();
    repo.git(&["mv", "old name", "new name"]);
    fs::remove_file(repo.0.join("deleted")).unwrap();
    repo.write(":(glob)*", "literal\n");
    repo.write("other", "not selected\n");
    repo.write("binary", [0, 1, 2, 3]);
    let snapshot = status(&repo.0).unwrap();
    let rename = snapshot
        .changes
        .iter()
        .find(|c| c.path == Path::new("new name"))
        .unwrap();
    assert_eq!(rename.label(), "Renamed");
    assert!(text(&diff(&snapshot.root, rename, DiffMode::Staged).unwrap()).contains("rename from"));
    let deleted = snapshot
        .changes
        .iter()
        .find(|c| c.path == Path::new("deleted"))
        .unwrap();
    assert!(text(&diff(&snapshot.root, deleted, DiffMode::Unstaged).unwrap()).contains("-base"));
    let literal = snapshot
        .changes
        .iter()
        .find(|c| c.path == Path::new(":(glob)*"))
        .unwrap();
    let patch = text(&diff(&snapshot.root, literal, DiffMode::Unstaged).unwrap());
    assert!(patch.contains("+literal") && !patch.contains("not selected"));
    let binary = snapshot
        .changes
        .iter()
        .find(|c| c.path == Path::new("binary"))
        .unwrap();
    assert!(text(&diff(&snapshot.root, binary, DiffMode::Unstaged).unwrap()).contains("Binary"));
}

#[test]
fn symlink_and_non_utf8_names_do_not_escape_or_change_identity() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let repo = Repo::new();
    symlink("/etc/passwd", repo.0.join("link")).unwrap();
    let raw = OsStr::from_bytes(b"non-utf8-\xff");
    let (_, parsed) = parse_status(b"## main\0?? non-utf8-\xff\0").unwrap();
    assert_eq!(parsed[0].path.as_os_str(), raw);
    let snapshot = status(&repo.0).unwrap();
    let link = snapshot
        .changes
        .iter()
        .find(|c| c.path == Path::new("link"))
        .unwrap();
    let preview = text(&diff(&snapshot.root, link, DiffMode::Unstaged).unwrap());
    assert!(preview.contains("not followed") && !preview.contains("root:"));
}

// Linux permits filename bytes that the macOS filesystem rejects.
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_paths_survive_real_status_and_diff_reads() {
    use std::os::unix::ffi::OsStrExt;
    let repo = Repo::new();
    let name = OsStr::from_bytes(b"non-utf8-\xff");
    let path = repo.0.join(name);
    fs::write(&path, "base\n").unwrap();
    let snapshot = status(&repo.0).unwrap();
    assert_eq!(snapshot.changes.len(), 1);
    let change = &snapshot.changes[0];
    assert_eq!(change.path.as_os_str(), name);
    assert!(text(&diff(&snapshot.root, change, DiffMode::Unstaged).unwrap()).contains("+base"));

    repo.commit();
    fs::write(&path, "changed\n").unwrap();
    let index = fs::read(repo.0.join(".git/index")).unwrap();
    let snapshot = status(&repo.0).unwrap();
    assert_eq!(snapshot.changes.len(), 1);
    let change = &snapshot.changes[0];
    assert_eq!(change.path.as_os_str(), name);
    let patch = text(&diff(&snapshot.root, change, DiffMode::Unstaged).unwrap());
    assert!(patch.contains("-base") && patch.contains("+changed"));
    assert_eq!(fs::read(repo.0.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read_to_string(path).unwrap(), "changed\n");
    assert!(!repo.0.join(".git/index.lock").exists());
}

#[test]
fn external_diff_and_textconv_are_never_executed() {
    let repo = Repo::new();
    repo.write("file.txt", "base\n");
    repo.write(".gitattributes", "*.txt diff=unsafe\n");
    repo.commit();
    repo.write("file.txt", "changed\n");
    repo.git(&["config", "diff.external", "false"]);
    repo.git(&["config", "diff.unsafe.textconv", "false"]);
    let snapshot = status(&repo.0).unwrap();
    assert!(
        text(&diff(&snapshot.root, &snapshot.changes[0], DiffMode::Unstaged).unwrap())
            .contains("+changed")
    );
}

#[test]
fn configured_clean_filters_are_not_executed() {
    let repo = Repo::new();
    repo.write("file.txt", "base\n");
    repo.write(".gitattributes", "*.txt filter=unsafe\n");
    repo.commit();
    repo.git(&["config", "filter.unsafe.clean", "false"]);
    repo.git(&["config", "filter.unsafe.process", "false"]);
    repo.git(&["config", "filter.unsafe.required", "true"]);
    repo.write("file.txt", "changed\n");
    let snapshot = status(&repo.0).unwrap();
    let preview = diff(&snapshot.root, &snapshot.changes[0], DiffMode::Unstaged).unwrap();
    assert!(text(&preview).contains("+changed"));
}

#[test]
fn patch_line_numbers_and_empty_untracked_files() {
    let patch = format_diff(
        b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -5,2 +8,2 @@\n old\n-gone\n+++literal pluses\n",
        false,
        Duration::ZERO,
        FileHeaders::Omit,
    );
    assert_eq!(patch.lines[4].old_line, Some(5));
    assert_eq!(patch.lines[4].new_line, Some(8));
    assert_eq!(patch.lines[5].old_line, Some(6));
    assert_eq!(patch.lines[5].new_line, None);
    assert_eq!(patch.lines[6].new_line, Some(9));
    assert_eq!(patch.lines[6].kind, DiffKind::Added);
    // An index into the drawn rows, not a length; the path-only header lines
    // are not drawn, so the widest drawn line is the literal pluses.
    let widest = patch.rows[patch.widest_row].line();
    assert_eq!(&*patch.lines[widest].text, "+++literal pluses");
    assert_eq!(patch.lines.len(), 7);
    assert_eq!(patch.rows.len(), 4);
    let repo = Repo::new();
    repo.write("empty", "");
    let snapshot = status(&repo.0).unwrap();
    let change = &snapshot.changes[0];
    assert!(
        text(&diff(&snapshot.root, change, DiffMode::Unstaged).unwrap())
            .contains("Empty untracked")
    );
    assert!(
        text(&diff(&snapshot.root, change, DiffMode::Staged).unwrap())
            .contains("no staged version")
    );
}

#[test]
fn preview_rejects_parent_symlinks_and_special_files() {
    use std::os::unix::fs::symlink;
    let repo = Repo::new();
    let outside = Repo::new();
    outside.write("secret", "must not read");
    symlink(&outside.0, repo.0.join("outside")).unwrap();
    assert!(open_regular_file(&repo.0, Path::new("outside/secret")).is_err());
    assert!(open_regular_file(&repo.0, Path::new("../secret")).is_err());
    assert!(open_regular_file(&repo.0, Path::new(".git")).is_err());
    repo.write("ordinary", "safe");
    assert!(open_regular_file(&repo.0, Path::new("ordinary")).is_ok());
}

#[test]
fn detached_head_and_merge_conflict_remain_readable() {
    let repo = Repo::new();
    repo.write("file", "base\n");
    repo.commit();
    repo.git(&["checkout", "-b", "other"]);
    repo.write("file", "other\n");
    repo.commit();
    repo.git(&["checkout", "main"]);
    repo.write("file", "main\n");
    repo.commit();
    let merge = Command::new("git")
        .arg("-C")
        .arg(&repo.0)
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let snapshot = status(&repo.0).unwrap();
    assert_eq!(snapshot.changes[0].label(), "Conflict");
    assert!(
        text(&diff(&snapshot.root, &snapshot.changes[0], DiffMode::Unstaged).unwrap())
            .contains("<<<<<<<")
    );
    repo.git(&["merge", "--abort"]);
    repo.git(&["checkout", "--detach"]);
    assert!(status(&repo.0).unwrap().branch.contains("HEAD"));
}

#[test]
fn subprocess_deadline_reaps_the_child() {
    let start = Instant::now();
    let error = execute(
        Command::new("sleep").arg("5"),
        100,
        Duration::from_millis(30),
    )
    .err()
    .unwrap();
    assert!(error.contains("was stopped"), "{error}");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn output_reader_drains_but_does_not_retain_unbounded_data() {
    let out = drain(&b"0123456789"[..], 4).unwrap();
    assert_eq!(out.bytes, b"0123");
    assert!(out.truncated);
}

#[test]
fn large_previews_and_long_lines_are_bounded() {
    let repo = Repo::new();
    repo.write("large", "a\n".repeat(DIFF_LIMIT));
    let snapshot = status(&repo.0).unwrap();
    let preview = diff(&snapshot.root, &snapshot.changes[0], DiffMode::Unstaged).unwrap();
    assert!(preview.truncated);
    assert_eq!(preview.lines.len(), 10_000);
    let preview = format_diff(
        "x".repeat(9000).as_bytes(),
        false,
        Duration::ZERO,
        FileHeaders::Omit,
    );
    assert!(preview.lines[0].text.ends_with("[line clipped]"));
    assert!(preview.lines[0].text.len() < 2100);
}

fn listed(repo: &Repo) -> Vec<String> {
    files(&repo.0).unwrap().paths
}

#[test]
fn file_list_has_tracked_and_unignored_untracked_files_only() {
    let repo = Repo::new();
    fs::create_dir_all(repo.0.join("src/deep dir")).unwrap();
    fs::create_dir(repo.0.join("build")).unwrap();
    repo.write(".gitignore", "*.log\n/build/\n");
    repo.write("src/main.rs", "fn main() {}\n");
    repo.write("src/deep dir/mod.rs", "\n");
    repo.write("removed.txt", "gone\n");
    repo.commit();
    fs::remove_file(repo.0.join("removed.txt")).unwrap();
    repo.write("src/main.rs", "fn main() { changed() }\n");
    repo.write("untracked.md", "new\n");
    repo.write("debug.log", "ignored\n");
    repo.write("build/output", "ignored\n");
    let index = fs::read(repo.0.join(".git/index")).unwrap();
    let list = files(&repo.0).unwrap();
    assert_eq!(
        list.paths,
        [
            ".gitignore",
            "src/deep dir/mod.rs",
            "src/main.rs",
            "untracked.md"
        ]
    );
    assert!(!list.truncated);
    assert_eq!(list.skipped, 0);
    assert_eq!(fs::read(repo.0.join(".git/index")).unwrap(), index);
    assert!(!repo.0.join(".git/index.lock").exists());
}

#[test]
fn file_list_is_relative_to_the_checkout_from_a_subdirectory() {
    let repo = Repo::new();
    fs::create_dir(repo.0.join("nested")).unwrap();
    repo.write("top.txt", "\n");
    repo.write("nested/inner.txt", "\n");
    repo.commit();
    assert_eq!(listed(&repo), ["nested/inner.txt", "top.txt"]);
    let snapshot = status(&repo.0.join("nested")).unwrap();
    assert_eq!(files(&snapshot.root).unwrap().paths, listed(&repo));
}

#[test]
fn file_list_covers_empty_staged_conflicted_and_nested_repositories() {
    let repo = Repo::new();
    assert!(listed(&repo).is_empty());
    repo.write("file", "base\n");
    repo.git(&["add", "file"]);
    assert_eq!(listed(&repo), ["file"]);
    repo.commit();
    repo.git(&["checkout", "-b", "other"]);
    repo.write("file", "other\n");
    repo.commit();
    repo.git(&["checkout", "main"]);
    repo.write("file", "main\n");
    repo.commit();
    let merge = Command::new("git")
        .arg("-C")
        .arg(&repo.0)
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    // Three index stages are still one file.
    assert_eq!(listed(&repo), ["file"]);
    repo.git(&["merge", "--abort"]);
    // The outer fixture removes this nested repository with its own directory.
    let nested = std::mem::ManuallyDrop::new(Repo(repo.0.join("vendor")));
    fs::create_dir(&nested.0).unwrap();
    nested.git(&["init", "-b", "main"]);
    nested.write("inside.txt", "\n");
    assert_eq!(listed(&repo), ["file"]);
    assert!(files(&repo.0.join("missing")).is_err());
}

#[test]
fn file_list_does_not_run_configured_filters_or_fsmonitor() {
    let repo = Repo::new();
    repo.write("file.txt", "base\n");
    repo.write(".gitattributes", "*.txt filter=unsafe\n");
    repo.commit();
    let marker = repo.0.join("executed");
    let touch = format!("touch '{}'", marker.display());
    repo.git(&["config", "filter.unsafe.clean", &touch]);
    repo.git(&["config", "filter.unsafe.process", &touch]);
    repo.git(&["config", "filter.unsafe.required", "true"]);
    repo.git(&["config", "core.fsmonitor", &touch]);
    repo.write("file.txt", "changed\n");
    assert_eq!(listed(&repo), [".gitattributes", "file.txt"]);
    assert!(!marker.exists());
}

#[test]
fn file_records_skip_unusable_names_and_bound_the_list() {
    let list = parse_files(
        b"? new file.rs\0H src/b.rs\0H src/a.rs\0M both\0M both\0R src/b.rs\0S sparse.rs\0? repo/\0H bad-\xff\0H line\nbreak\0",
        false,
        usize::MAX,
    )
    .unwrap();
    assert_eq!(list.paths, ["both", "new file.rs", "src/a.rs"]);
    assert_eq!(list.skipped, 2);
    assert!(!list.truncated);

    let clipped = parse_files(b"H a\0H b\0H partial-na", true, usize::MAX).unwrap();
    assert_eq!(clipped.paths, ["a", "b"]);
    assert!(clipped.truncated);

    let bounded = parse_files(b"H c\0H a\0H b\0", false, 2).unwrap();
    assert_eq!(bounded.paths, ["a", "b"]);
    assert!(bounded.truncated);

    assert!(parse_files(b"", false, 9).unwrap().paths.is_empty());
    assert!(parse_files(b"H ../outside\0", false, 9).is_err());
    assert!(parse_files(b"H /absolute\0", false, 9).is_err());
    assert!(parse_files(b"Hmissing-space\0", false, 9).is_err());
    assert!(parse_files(b"H \0", false, 9).is_err());
}

// ---- writes and history ----

impl Repo {
    fn out(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .envs(NO_MAINTENANCE)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
    fn short_status(&self) -> String {
        self.out(&["status", "--porcelain=v1", "--untracked-files=all"])
    }
    fn hook(&self, name: &str, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(".git/hooks").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

fn stage(repo: &Repo, names: &[&str]) -> Result<WriteOutcome> {
    write(&repo.0, &WriteOp::Stage(paths(names)))
}

fn unstage(repo: &Repo, names: &[&str]) -> Result<WriteOutcome> {
    write(&repo.0, &WriteOp::Unstage(paths(names)))
}

fn commit_op(repo: &Repo, message: &str) -> Result<WriteOutcome> {
    write(
        &repo.0,
        &WriteOp::Commit {
            message: message.into(),
        },
    )
}

#[test]
fn staging_covers_modified_new_and_deleted_files() {
    let repo = Repo::new();
    repo.write("mod.txt", "a\n");
    repo.write("gone.txt", "b\n");
    repo.commit();
    repo.write("mod.txt", "changed\n");
    repo.write("new.txt", "n\n");
    fs::remove_file(repo.0.join("gone.txt")).unwrap();
    let outcome = stage(&repo, &["mod.txt", "new.txt", "gone.txt"]).unwrap();
    assert_eq!(outcome.summary, "Staged 3 files");
    assert_eq!(outcome.commit, None);
    assert_eq!(repo.short_status(), "D  gone.txt\nM  mod.txt\nA  new.txt\n");
    assert_eq!(stage(&repo, &["mod.txt"]).unwrap().summary, "Staged 1 file");
}

#[test]
fn staging_paths_are_literal_pathspecs() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("*.txt", "star\n");
    stage(&repo, &["*.txt"]).unwrap();
    assert_eq!(repo.out(&["diff", "--cached", "--name-only"]), "*.txt\n");
}

#[test]
fn unstage_with_head_restores_the_committed_state() {
    let repo = Repo::new();
    repo.write("mod.txt", "a\n");
    repo.write("gone.txt", "b\n");
    repo.commit();
    repo.write("mod.txt", "changed\n");
    repo.write("new.txt", "n\n");
    fs::remove_file(repo.0.join("gone.txt")).unwrap();
    write(&repo.0, &WriteOp::StageAll).unwrap();
    let outcome = unstage(&repo, &["mod.txt", "new.txt", "gone.txt"]).unwrap();
    assert_eq!(outcome.summary, "Unstaged 3 files");
    assert_eq!(repo.short_status(), " D gone.txt\n M mod.txt\n?? new.txt\n");
    // The worktree is untouched.
    assert_eq!(
        fs::read_to_string(repo.0.join("mod.txt")).unwrap(),
        "changed\n"
    );
}

#[test]
fn unstage_on_an_unborn_branch_returns_files_to_untracked() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    fs::create_dir(repo.0.join("dir")).unwrap();
    repo.write("dir/b.txt", "b\n");
    stage(&repo, &["a.txt", "dir/b.txt"]).unwrap();
    // Differs from both the index and (absent) HEAD: still unstages.
    repo.write("a.txt", "edited after staging\n");
    unstage(&repo, &["a.txt"]).unwrap();
    assert_eq!(repo.short_status(), "A  dir/b.txt\n?? a.txt\n");
    let outcome = write(&repo.0, &WriteOp::UnstageAll).unwrap();
    assert_eq!(outcome.summary, "Unstaged all changes");
    assert_eq!(repo.short_status(), "?? a.txt\n?? dir/b.txt\n");
    // Nothing staged on an unborn branch is not an error.
    write(&repo.0, &WriteOp::UnstageAll).unwrap();
}

#[test]
fn rename_stages_and_unstages_with_both_paths() {
    let repo = Repo::new();
    repo.write("old.txt", "content that is long enough to match\n");
    repo.commit();
    fs::rename(repo.0.join("old.txt"), repo.0.join("new.txt")).unwrap();
    stage(&repo, &["new.txt", "old.txt"]).unwrap();
    assert_eq!(repo.short_status(), "R  old.txt -> new.txt\n");
    unstage(&repo, &["new.txt", "old.txt"]).unwrap();
    assert_eq!(repo.short_status(), " D old.txt\n?? new.txt\n");
}

#[test]
fn stage_all_and_unstage_all() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    repo.write("a.txt", "b\n");
    repo.write("n.txt", "n\n");
    assert_eq!(
        write(&repo.0, &WriteOp::StageAll).unwrap().summary,
        "Staged all changes"
    );
    assert_eq!(repo.short_status(), "M  a.txt\nA  n.txt\n");
    write(&repo.0, &WriteOp::UnstageAll).unwrap();
    assert_eq!(repo.short_status(), " M a.txt\n?? n.txt\n");
}

#[test]
fn write_paths_are_validated() {
    let repo = Repo::new();
    for bad in ["../outside", "/etc/passwd", "a/../b", ""] {
        assert!(stage(&repo, &[bad]).is_err(), "{bad}");
        assert!(unstage(&repo, &[bad]).is_err(), "{bad}");
    }
    assert!(stage(&repo, &[]).is_err());
    assert!(unstage(&repo, &[]).is_err());
    let many: Vec<PathBuf> = (0..100_001).map(|n| PathBuf::from(n.to_string())).collect();
    assert!(
        write(&repo.0, &WriteOp::Stage(many))
            .unwrap_err()
            .contains("Too many")
    );
}

#[test]
fn many_paths_travel_over_stdin_without_deadlock() {
    let repo = Repo::new();
    let names: Vec<String> = (0..3000).map(|n| format!("f{n:04}.txt")).collect();
    for name in &names {
        repo.write(name, "x\n");
    }
    let list: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
    let outcome = write(&repo.0, &WriteOp::Stage(list)).unwrap();
    assert_eq!(outcome.summary, "Staged 3000 files");
}

#[test]
fn commit_returns_the_new_id_and_summary() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    stage(&repo, &["a.txt"]).unwrap();
    let outcome = commit_op(&repo, "First change\n\nBody text.\n\n").unwrap();
    let head = repo.out(&["rev-parse", "HEAD"]).trim().to_owned();
    assert_eq!(outcome.commit.as_deref(), Some(head.as_str()));
    let short = repo
        .out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_owned();
    assert_eq!(outcome.summary, format!("Committed {short}: First change"));
    assert_eq!(
        repo.out(&["log", "-1", "--format=%B"]),
        "First change\n\nBody text.\n\n"
    );
    assert_eq!(repo.short_status(), "");
}

#[test]
fn commit_takes_only_staged_changes() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.commit();
    repo.write("a.txt", "a2\n");
    repo.write("b.txt", "b2\n");
    stage(&repo, &["a.txt"]).unwrap();
    commit_op(&repo, "Only a").unwrap();
    assert_eq!(repo.short_status(), " M b.txt\n");
}

#[test]
fn blank_message_and_empty_index_are_rejected() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    stage(&repo, &["a.txt"]).unwrap();
    assert!(commit_op(&repo, " \n\t\n").unwrap_err().contains("message"));
    assert!(commit_op(&repo, "").is_err());
    commit_op(&repo, "Add a").unwrap();
    let error = commit_op(&repo, "Nothing").unwrap_err();
    assert!(error.contains("Nothing is staged"), "{error}");
}

#[test]
fn failing_pre_commit_hook_blocks_the_commit_and_explains_why() {
    let repo = Repo::new();
    repo.hook(
        "pre-commit",
        "echo 'lint: trailing whitespace in a.txt' >&2\nexit 1",
    );
    repo.write("a.txt", "a\n");
    stage(&repo, &["a.txt"]).unwrap();
    let error = commit_op(&repo, "Blocked").unwrap_err();
    assert!(error.contains("lint: trailing whitespace"), "{error}");
    assert_eq!(log(&repo.0, None, 0, 5).unwrap(), History::default());
    assert_eq!(repo.short_status(), "A  a.txt\n");
}

#[test]
fn passing_hooks_run_and_may_see_a_non_interactive_environment() {
    let repo = Repo::new();
    repo.hook("pre-commit", "touch hook-ran");
    repo.write("a.txt", "a\n");
    stage(&repo, &["a.txt"]).unwrap();
    commit_op(&repo, "Hooked").unwrap();
    assert!(repo.0.join("hook-ran").exists());
}

#[test]
fn held_index_lock_gives_a_friendly_message() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write(".git/index.lock", "");
    let error = stage(&repo, &["a.txt"]).unwrap_err();
    assert_eq!(
        error,
        "Another Git process is using this repository. Try again when it finishes."
    );
    assert_eq!(write(&repo.0, &WriteOp::StageAll).unwrap_err(), error);
}

#[test]
fn missing_identity_surfaces_gits_message() {
    let repo = Repo::new();
    repo.git(&["config", "user.useConfigOnly", "true"]);
    // Empty local values override any global identity on the machine.
    repo.git(&["config", "user.name", ""]);
    repo.git(&["config", "user.email", ""]);
    repo.write("a.txt", "a\n");
    stage(&repo, &["a.txt"]).unwrap();
    let error = commit_op(&repo, "No identity").unwrap_err();
    assert!(error.starts_with("Git: "), "{error}");
}

#[test]
fn hash_lines_in_commit_messages_are_kept() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    stage(&repo, &["a.txt"]).unwrap();
    commit_op(&repo, "# Heading style subject\n\n# kept line\nbody").unwrap();
    assert_eq!(
        repo.out(&["log", "-1", "--format=%B"]),
        "# Heading style subject\n\n# kept line\nbody\n\n"
    );
}

#[test]
fn staging_runs_clean_filters_but_history_and_diff_reads_do_not() {
    let repo = Repo::new();
    repo.write(".gitattributes", "*.txt filter=upper\n");
    repo.git(&["config", "filter.upper.clean", "tr a-z A-Z"]);
    repo.write("a.txt", "hello\n");
    stage(&repo, &["a.txt"]).unwrap();
    assert_eq!(repo.out(&["show", ":a.txt"]), "HELLO\n");
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["commit", "-m", "base"]);
    // Reads stay hardened even though a failing filter is configured.
    repo.git(&["config", "filter.upper.clean", "false"]);
    repo.git(&["config", "filter.upper.process", "false"]);
    repo.git(&["config", "filter.upper.required", "true"]);
    let id = log(&repo.0, None, 0, 10).unwrap().commits[0].id.clone();
    assert!(text(&show(&repo.0, &id).unwrap()).contains("+HELLO"));
    repo.write("a.txt", "again\n");
    assert_eq!(log(&repo.0, None, 0, 10).unwrap().commits.len(), 1);
}

#[test]
fn worktree_graph_read_keeps_merge_parents_and_never_runs_diff_helpers() {
    let repo = Repo::new();
    assert!(super::log::graph(&repo.0, &[]).unwrap().0.is_empty());
    repo.write("a", "base");
    repo.commit();
    let base = repo.out(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.git(&["checkout", "-b", "topic"]);
    repo.write("b", "topic");
    repo.commit();
    let topic = repo.out(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.git(&["checkout", "main"]);
    repo.write("c", "main");
    repo.commit();
    repo.git(&["merge", "--no-ff", "topic", "-m", "merge"]);
    let head = repo.out(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.git(&["config", "diff.external", "false"]);
    let (commits, more) = super::log::graph(&repo.0, &[head.clone(), topic.clone()]).unwrap();
    assert!(!more);
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].commit.id, head);
    assert_eq!(commits[0].parents.len(), 2);
    assert_eq!(commits[0].parents[1], topic);
    assert_eq!(commits.last().unwrap().commit.id, base);
    assert!(super::log::graph(&repo.0, &["--all".into()]).is_err());
}

#[test]
fn log_on_an_unborn_branch_is_empty() {
    let repo = Repo::new();
    assert_eq!(log(&repo.0, None, 0, 10).unwrap(), History::default());
}

#[test]
fn log_pages_with_skip_and_more() {
    let repo = Repo::new();
    for n in 0..5 {
        repo.write("a.txt", format!("{n}\n"));
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", &format!("commit {n}")]);
    }
    let first = log(&repo.0, None, 0, 2).unwrap();
    assert!(first.more);
    assert_eq!(first.commits.len(), 2);
    assert_eq!(first.commits[0].subject, "commit 4");
    assert_eq!(first.commits[0].author, "Plait Test");
    assert_eq!(first.commits[0].id.len(), 40);
    assert!(first.commits[0].id.starts_with(&first.commits[0].short));
    assert!(first.commits[0].time > 1_000_000_000);
    let last = log(&repo.0, None, 4, 2).unwrap();
    assert!(!last.more);
    assert_eq!(last.commits.len(), 1);
    assert_eq!(last.commits[0].subject, "commit 0");
    let exact = log(&repo.0, None, 3, 2).unwrap();
    assert!(!exact.more);
    assert_eq!(log(&repo.0, None, 9, 2).unwrap(), History::default());
    assert_eq!(log(&repo.0, None, 0, 0).unwrap(), History::default());
    assert_eq!(log(&repo.0, None, 0, 10_000).unwrap().commits.len(), 5);
}

#[test]
fn later_pages_follow_the_first_pages_tip_when_head_moves() {
    let repo = Repo::new();
    for n in 0..4 {
        repo.write("a.txt", format!("{n}\n"));
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", &format!("commit {n}")]);
    }
    let first = log(&repo.0, None, 0, 2).unwrap();
    let tip = first.commits[0].id.clone();
    // A commit lands between pages.
    repo.write("a.txt", "later\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "later"]);
    let second = log(&repo.0, Some(&tip), 2, 2).unwrap();
    let subjects: Vec<_> = second.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["commit 1", "commit 0"]);
    assert!(log(&repo.0, Some("HEAD"), 0, 2).is_err());
    assert!(log(&repo.0, Some("--all"), 0, 2).is_err());
}

#[test]
fn a_hook_that_leaves_a_process_holding_output_does_not_hang_the_commit() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.git(&["add", "."]);
    let hooks = repo.0.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    // The background sleep inherits stdout and outlives the hook.
    std::fs::write(&hook, "#!/bin/sh\nsleep 30 &\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&hook, permissions).unwrap();
    let start = std::time::Instant::now();
    let result = write(
        &repo.0,
        &WriteOp::Commit {
            message: "with a stray hook process".into(),
        },
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(15),
        "{:?}",
        start.elapsed()
    );
    // Either the commit is reported, or the stray process is named; never a hang.
    match result {
        Ok(outcome) => assert!(outcome.commit.is_some()),
        Err(error) => assert!(error.contains("still holds"), "{error}"),
    }
}

#[test]
fn log_fields_are_sanitized_and_records_validated() {
    let records = parse_log_for_test(b"");
    assert!(records.is_empty());
    let id = "a".repeat(40);
    let raw = format!("{id}\0aaaaaaa\0Evil\x1b[31m Name\0100\0Sub\tject\r\n");
    let commits = parse_log_for_test(raw.as_bytes());
    assert_eq!(commits[0].author, "Evil [31m Name");
    assert_eq!(commits[0].subject, "Sub ject");
    assert!(log::parse_log(b"short\0a\0b\0c\0d\0").is_err());
    assert!(log::parse_log(b"a\0b").is_err());
}

fn parse_log_for_test(bytes: &[u8]) -> Vec<Commit> {
    log::parse_log(bytes).unwrap()
}

#[test]
fn show_returns_message_then_patch() {
    let repo = Repo::new();
    repo.write("a.txt", "one\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "Root commit"]);
    repo.write("a.txt", "one\ntwo\n");
    repo.git(&["add", "."]);
    repo.git(&[
        "commit",
        "-m",
        "Add two",
        "-m",
        "diff --git fake\n@@ -1 +1 @@ in body",
    ]);
    let id = log(&repo.0, None, 0, 1).unwrap().commits[0].id.clone();
    let diff = show(&repo.0, &id).unwrap();
    let body = text(&diff);
    assert!(body.contains("Author: Plait Test <test@example.invalid>"));
    assert!(body.contains("    Add two"));
    assert!(body.contains("+two"));
    let first_diff = diff
        .lines
        .iter()
        .position(|l| l.text.starts_with("diff --git a/a.txt"))
        .unwrap();
    // Everything before the patch, message included, is a header line.
    assert!(
        diff.lines[..first_diff]
            .iter()
            .all(|l| l.kind == DiffKind::Header)
    );
    assert!(
        diff.lines
            .iter()
            .any(|l| l.kind == DiffKind::Added && &*l.text == "+two")
    );
    // The root commit shows its files as additions.
    let root_id = log(&repo.0, None, 1, 1).unwrap().commits[0].id.clone();
    assert!(text(&show(&repo.0, &root_id).unwrap()).contains("+one"));
}

/// The rows each patch draws, as Git really writes the headers.
fn drawn(diff: &Diff) -> Vec<String> {
    diff.rows
        .iter()
        .map(|row| match row {
            crate::patch::PatchRow::Line(line) => diff.lines[*line].text.to_string(),
            crate::patch::PatchRow::File {
                name, directory, ..
            } => format!("# {directory} | {name}"),
        })
        .collect()
}

#[test]
fn diffs_start_at_the_change_and_commits_head_each_file() {
    let repo = Repo::new();
    fs::create_dir(repo.0.join("src")).unwrap();
    repo.write("src/a.rs", "fn a() {}\n");
    repo.write("old.txt", "kept\nkept\nkept\nkept\n");
    repo.write("run.sh", "echo\n");
    repo.write("gone.txt", "bye\n");
    repo.commit();
    repo.write("src/a.rs", "fn a() { b() }\n");
    repo.git(&["mv", "old.txt", "new.txt"]);
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(repo.0.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&["rm", "-q", "gone.txt"]);
    repo.write("pic.bin", [0u8, 1, 2, 255]);
    repo.write("café.md", "hi\n");
    repo.commit();

    // One file in Changes: the first drawn row is its hunk.
    repo.write("src/a.rs", "fn a() { c() }\n");
    let snapshot = status(&repo.0).unwrap();
    let change = snapshot.changes.iter().find(|c| c.path.ends_with("a.rs"));
    let unstaged = diff(&snapshot.root, change.unwrap(), DiffMode::Unstaged).unwrap();
    assert!(
        drawn(&unstaged)[0].starts_with("@@ -1 +1 @@"),
        "{unstaged:?}"
    );
    assert_eq!(unstaged.lines.len(), unstaged.rows.len() + 4);

    let id = log(&repo.0, None, 0, 1).unwrap().commits[0].id.clone();
    let rows = drawn(&show(&repo.0, &id).unwrap());
    for heading in [
        "#  | café.md",
        "#  | gone.txt",
        "#  | new.txt",
        "#  | pic.bin",
        "#  | run.sh",
        "# src | a.rs",
    ] {
        assert!(
            rows.iter().any(|row| row == heading),
            "{heading}: {rows:#?}"
        );
    }
    for kept in [
        "deleted file mode 100644",
        "rename from old.txt",
        "rename to new.txt",
        "old mode 100644",
        "new mode 100755",
    ] {
        assert!(rows.iter().any(|row| row == kept), "{kept}: {rows:#?}");
    }
    assert!(rows.iter().any(|row| row.starts_with("Binary files")));
    assert!(!rows.iter().any(|row| row.starts_with("diff --git")
        || row.starts_with("index ")
        || row.starts_with("--- ")
        || row.starts_with("+++ ")));
}

#[test]
fn show_rejects_ids_that_are_not_full_hex() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    for bad in [
        "HEAD",
        "abc1234",
        "--output=/tmp/x",
        &"A".repeat(40),
        &"g".repeat(40),
        &"a".repeat(41),
        "",
    ] {
        assert!(show(&repo.0, bad).is_err(), "{bad}");
    }
    // A well-formed ID that names nothing is an error, not output.
    assert!(show(&repo.0, &"a".repeat(40)).is_err());
}

#[test]
fn show_of_a_merge_uses_the_first_parent_and_truncates_large_commits() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("side.txt", "s\n");
    repo.commit();
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "--no-ff", "-q", "side", "-m", "Merge side"]);
    let id = log(&repo.0, None, 0, 1).unwrap().commits[0].id.clone();
    assert!(text(&show(&repo.0, &id).unwrap()).contains("+s"));
    repo.write("big.txt", "line\n".repeat(200_000));
    repo.commit();
    let id = log(&repo.0, None, 0, 1).unwrap().commits[0].id.clone();
    assert!(show(&repo.0, &id).unwrap().truncated);
}

/// Run Git in `dir` with the test's isolated configuration.
fn git_in(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Other",
            "-c",
            "user.email=o@example.invalid",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn tracking(repo: &Repo) -> Option<Tracking> {
    status(&repo.0).unwrap().tracking
}

#[test]
fn publish_push_and_fetch_move_commits_through_a_remote() {
    let work = Repo::new();
    work.write("a.txt", "a\n");
    work.commit();
    // A bare repository on disk stands in for the network remote.
    let holder = Repo::new();
    let hub = holder.0.join("hub.git");
    git_in(&holder.0, &["init", "--bare", "-b", "main", "hub.git"]);
    let url = hub.to_str().unwrap();

    // No remote: publishing says so instead of guessing.
    assert!(
        write(&work.0, &WriteOp::Publish)
            .unwrap_err()
            .contains("no remote")
    );
    work.git(&["remote", "add", "origin", url]);
    let before = status(&work.0).unwrap();
    assert!(before.named && before.tracking.is_none());

    let published = write(&work.0, &WriteOp::Publish).unwrap();
    assert_eq!(published.summary, "Published to origin");
    let mut expected = Tracking {
        upstream: "origin/main".into(),
        ..Tracking::default()
    };
    assert_eq!(tracking(&work), Some(expected.clone()));

    work.write("a.txt", "b\n");
    work.commit();
    expected.ahead = 1;
    assert_eq!(tracking(&work), Some(expected.clone()));
    assert_eq!(
        write(&work.0, &WriteOp::Push).unwrap().summary,
        "Pushed to origin/main"
    );
    expected.ahead = 0;
    assert_eq!(tracking(&work), Some(expected.clone()));

    // Someone else pushes; a fetch reveals it.
    git_in(&holder.0, &["clone", "-q", url, "other"]);
    let other = holder.0.join("other");
    fs::write(other.join("b.txt"), "theirs\n").unwrap();
    git_in(&other, &["add", "."]);
    git_in(&other, &["commit", "-q", "-m", "theirs"]);
    git_in(&other, &["push", "-q"]);
    assert_eq!(tracking(&work), Some(expected.clone()));
    assert_eq!(write(&work.0, &WriteOp::Fetch).unwrap().summary, "Fetched");
    expected.behind = 1;
    assert_eq!(tracking(&work), Some(expected.clone()));

    // A pull moves the branch forward to what was fetched.
    assert_eq!(
        write(&work.0, &WriteOp::Pull).unwrap().summary,
        "Pulled 1 commit"
    );
    expected.behind = 0;
    assert_eq!(tracking(&work), Some(expected.clone()));
    assert_eq!(
        fs::read_to_string(work.0.join("b.txt")).unwrap(),
        "theirs\n"
    );
    assert_eq!(
        write(&work.0, &WriteOp::Pull).unwrap().summary,
        "Already up to date"
    );

    // A pull that would overwrite an uncommitted edit is refused, and the edit stays.
    fs::write(other.join("b.txt"), "theirs again\n").unwrap();
    git_in(&other, &["commit", "-q", "-am", "again"]);
    git_in(&other, &["push", "-q"]);
    work.write("b.txt", "mine, uncommitted\n");
    let error = write(&work.0, &WriteOp::Pull).unwrap_err();
    assert!(error.contains("overwrite uncommitted"), "{error}");
    assert_eq!(
        fs::read_to_string(work.0.join("b.txt")).unwrap(),
        "mine, uncommitted\n"
    );
    work.git(&["checkout", "--", "b.txt"]);

    // Diverged: push and pull are both refused, and the reasons say what to do.
    work.write("a.txt", "c\n");
    work.commit();
    let error = write(&work.0, &WriteOp::Push).unwrap_err();
    assert!(error.contains("Pull them"), "{error}");
    let error = write(&work.0, &WriteOp::Pull).unwrap_err();
    assert!(error.contains("Merge or rebase"), "{error}");
}

#[test]
fn network_failures_are_explained() {
    assert!(
        network_hint("fatal: could not read Username for 'https://github.com'")
            .unwrap()
            .contains("gh auth login")
    );
    assert!(
        network_hint("git@github.com: Permission denied (publickey).")
            .unwrap()
            .contains("SSH key")
    );
    assert!(
        network_hint(" ! [rejected]        main -> main (fetch first)")
            .unwrap()
            .contains("Pull")
    );
    assert!(
        network_hint("ssh: Could not resolve host: github.com")
            .unwrap()
            .contains("network")
    );
    assert!(network_hint("something else").is_none());
}

fn numbered(lines: usize, changed: &[usize]) -> String {
    (1..=lines)
        .map(|n| {
            if changed.contains(&n) {
                format!("line {n} changed\n")
            } else {
                format!("line {n}\n")
            }
        })
        .collect()
}

fn tracked(path: &str) -> Change {
    Change {
        path: PathBuf::from(path),
        original: None,
        index: b' ',
        worktree: b'M',
    }
}

fn hunk_op(diff: &Diff, from: DiffMode, index: usize) -> WriteOp {
    WriteOp::Hunk {
        path: PathBuf::from("a.txt"),
        from,
        index,
        header: diff.lines[diff.hunks[index]].text.to_string(),
    }
}

#[test]
fn one_hunk_stages_and_unstages_without_touching_the_other() {
    let repo = Repo::new();
    repo.write("a.txt", numbered(30, &[]));
    repo.commit();
    repo.write("a.txt", numbered(30, &[1, 25]));
    let unstaged = diff(&repo.0, &tracked("a.txt"), DiffMode::Unstaged).unwrap();
    assert_eq!(unstaged.hunks.len(), 2);

    let outcome = write(&repo.0, &hunk_op(&unstaged, DiffMode::Unstaged, 1)).unwrap();
    assert_eq!(outcome.summary, "Staged a hunk of a.txt");
    let staged = diff(&repo.0, &tracked("a.txt"), DiffMode::Staged).unwrap();
    assert_eq!(staged.hunks.len(), 1);
    assert!(text(&staged).contains("+line 25 changed"));
    assert!(!text(&staged).contains("line 1 changed"));
    let left = diff(&repo.0, &tracked("a.txt"), DiffMode::Unstaged).unwrap();
    assert!(text(&left).contains("+line 1 changed"));
    assert!(!text(&left).contains("line 25 changed"));

    write(&repo.0, &hunk_op(&staged, DiffMode::Staged, 0)).unwrap();
    let staged = diff(&repo.0, &tracked("a.txt"), DiffMode::Staged).unwrap();
    assert!(staged.hunks.is_empty());
    // The worktree never changes.
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        numbered(30, &[1, 25])
    );
}

#[test]
fn a_hunk_that_moved_since_it_was_shown_is_refused() {
    let repo = Repo::new();
    repo.write("a.txt", numbered(30, &[]));
    repo.commit();
    repo.write("a.txt", numbered(30, &[25]));
    let shown = diff(&repo.0, &tracked("a.txt"), DiffMode::Unstaged).unwrap();
    // An agent edits above it; the hunk's header shifts.
    let mut edited = String::from("new first line\n");
    edited.push_str(&numbered(30, &[25]));
    repo.write("a.txt", edited);
    let error = write(&repo.0, &hunk_op(&shown, DiffMode::Unstaged, 0)).unwrap_err();
    assert!(error.contains("changed since"), "{error}");
    let missing = write(&repo.0, &hunk_op(&shown, DiffMode::Unstaged, 0));
    assert!(missing.is_err());
    let staged = diff(&repo.0, &tracked("a.txt"), DiffMode::Staged).unwrap();
    assert!(staged.hunks.is_empty());
}

#[test]
fn a_filtered_file_is_staged_whole_not_by_hunk() {
    let repo = Repo::new();
    repo.write(".gitattributes", "a.txt filter=custom\n");
    repo.write("a.txt", numbered(10, &[]));
    repo.commit();
    repo.write("a.txt", numbered(10, &[2]));
    let shown = diff(&repo.0, &tracked("a.txt"), DiffMode::Unstaged).unwrap();
    let error = write(&repo.0, &hunk_op(&shown, DiffMode::Unstaged, 0)).unwrap_err();
    assert!(error.contains("filter"), "{error}");
}

#[test]
fn hunk_patches_keep_bytes_and_match_shown_headers() {
    let diff = b"diff --git a/a b/a\nindex 1..2 100644\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@ fn\tx\n-old\r\n+new\r\n same\r\n@@ -9 +9 @@\n-9\n+nine\n\\ No newline at end of file\n";
    let first = hunk_patch(diff, 0, "@@ -1,2 +1,2 @@ fn    x").unwrap();
    assert_eq!(
        first,
        b"diff --git a/a b/a\nindex 1..2 100644\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@ fn\tx\n-old\r\n+new\r\n same\r\n"
    );
    let second = hunk_patch(diff, 1, "@@ -9 +9 @@").unwrap();
    assert!(second.ends_with(b"+nine\n\\ No newline at end of file\n"));
    assert!(hunk_patch(diff, 1, "@@ -8 +8 @@").is_err());
    assert!(hunk_patch(diff, 2, "@@ -9 +9 @@").is_err());
    assert!(hunk_patch(b"diff --cc a\n@@@ -1 -1 +1 @@@\n", 0, "@@@ -1 -1 +1 @@@").is_err());
}

fn discard_op(path: &str, untracked: bool, hunk: Option<(usize, String)>) -> WriteOp {
    WriteOp::Discard {
        path: PathBuf::from(path),
        untracked,
        hunk,
    }
}

#[test]
fn discarding_a_file_keeps_its_staged_side_and_undo_restores_it() {
    let repo = Repo::new();
    repo.write("a.txt", numbered(10, &[]));
    repo.commit();
    repo.write("a.txt", numbered(10, &[2]));
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", numbered(10, &[2, 8]));
    let outcome = write(&repo.0, &discard_op("a.txt", false, None)).unwrap();
    assert_eq!(outcome.summary, "Discarded changes to a.txt");
    // The worktree returns to the index, which still holds the staged change.
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        numbered(10, &[2])
    );
    let backup = outcome.backup.unwrap();
    assert_eq!(
        write(&repo.0, &WriteOp::Restore(backup)).unwrap().summary,
        "Restored a.txt"
    );
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        numbered(10, &[2, 8])
    );
}

#[test]
fn discarding_one_hunk_leaves_the_others() {
    let repo = Repo::new();
    repo.write("a.txt", numbered(30, &[]));
    repo.commit();
    repo.write("a.txt", numbered(30, &[1, 25]));
    let shown = diff(&repo.0, &tracked("a.txt"), DiffMode::Unstaged).unwrap();
    let header = shown.lines[shown.hunks[0]].text.to_string();
    let outcome = write(
        &repo.0,
        &discard_op("a.txt", false, Some((0, header.clone()))),
    )
    .unwrap();
    assert_eq!(outcome.summary, "Discarded a hunk of a.txt");
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        numbered(30, &[25])
    );
    // The same hunk again no longer matches.
    let error = write(&repo.0, &discard_op("a.txt", false, Some((0, header)))).unwrap_err();
    assert!(error.contains("changed since"), "{error}");
    write(&repo.0, &WriteOp::Restore(outcome.backup.unwrap())).unwrap();
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        numbered(30, &[1, 25])
    );
}

#[test]
fn deleting_an_untracked_file_can_be_undone_with_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    fs::create_dir(repo.0.join("tools")).unwrap();
    repo.write("tools/run.sh", "#!/bin/sh\n");
    let script = repo.0.join("tools/run.sh");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let outcome = write(&repo.0, &discard_op("tools/run.sh", true, None)).unwrap();
    assert_eq!(outcome.summary, "Deleted run.sh");
    assert!(!script.exists());
    write(&repo.0, &WriteOp::Restore(outcome.backup.unwrap())).unwrap();
    assert_eq!(fs::read_to_string(&script).unwrap(), "#!/bin/sh\n");
    assert_ne!(
        fs::metadata(&script).unwrap().permissions().mode() & 0o111,
        0
    );
}

#[test]
fn restore_never_writes_through_a_symlink() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    let outside = Repo::new();
    std::os::unix::fs::symlink(&outside.0, repo.0.join("link")).unwrap();
    let backup = Backup {
        path: PathBuf::from("link/escaped.txt"),
        // The committed "a\n" blob exists, so only the symlink can stop this.
        blob: "78981922613b2afb6025042ff6bd878ac1994e85".into(),
        executable: false,
    };
    let mut inside = backup.clone();
    inside.path = PathBuf::from("restored.txt");
    write(&repo.0, &WriteOp::Restore(inside)).unwrap();
    assert_eq!(
        fs::read_to_string(repo.0.join("restored.txt")).unwrap(),
        "a\n"
    );
    let error = write(&repo.0, &WriteOp::Restore(backup)).unwrap_err();
    assert!(error.contains("Cannot restore"), "{error}");
    assert!(!outside.0.join("escaped.txt").exists());
}

#[test]
fn a_worktree_needs_a_first_commit() {
    let repo = Repo::new();
    let error = add_worktree(&repo.0).unwrap_err();
    assert!(error.contains("first commit"), "{error}");
}

#[test]
fn a_worktree_is_created_beside_the_repository_and_removed_only_when_clean() {
    let repo = Repo::new();
    repo.write("hello.txt", "hello\n");
    repo.git(&["add", "hello.txt"]);
    repo.git(&["commit", "-q", "-m", "First"]);
    let tree = add_worktree(&repo.0).unwrap();
    let base = tree.path.parent().unwrap().to_owned();
    let mut folder = repo.0.file_name().unwrap().to_owned();
    folder.push(".worktrees");
    assert_eq!(
        base,
        repo.0
            .canonicalize()
            .unwrap()
            .parent()
            .unwrap()
            .join(folder)
    );
    assert!(tree.branch.starts_with("plait/"));
    let snapshot = status(&tree.path).unwrap();
    assert_eq!(snapshot.root, tree.path);
    assert_eq!(snapshot.branch, tree.branch);
    assert_eq!(
        fs::read_to_string(tree.path.join("hello.txt")).unwrap(),
        "hello\n"
    );

    // A second worktree gets its own folder and branch.
    let other = add_worktree(&repo.0).unwrap();
    assert_ne!(other.path, tree.path);
    assert_ne!(other.branch, tree.branch);

    fs::write(tree.path.join("draft.txt"), "unsaved\n").unwrap();
    let error = remove_worktree(&repo.0, &tree.path).unwrap_err();
    assert!(error.contains("uncommitted changes"), "{error}");
    assert!(tree.path.exists());

    fs::remove_file(tree.path.join("draft.txt")).unwrap();
    remove_worktree(&repo.0, &tree.path).unwrap();
    remove_worktree(&repo.0, &other.path).unwrap();
    assert!(!tree.path.exists());
    assert!(!base.exists(), "the empty shared folder is removed");
    // The branch and its commits stay.
    repo.git(&[
        "rev-parse",
        "--verify",
        "-q",
        &format!("refs/heads/{}", tree.branch),
    ]);
}

#[test]
fn numstat_records_parse_counts_binary_and_odd_names() {
    let counts =
        parse_numstat(b"3\t1\tsrc/a.rs\0-\t-\timage.png\x000\t2\twith\ttab.txt\0").unwrap();
    assert_eq!(counts.len(), 3);
    assert_eq!((counts[0].added, counts[0].removed), (Some(3), Some(1)));
    assert_eq!((counts[1].added, counts[1].removed), (None, None));
    assert_eq!(counts[2].path, PathBuf::from("with\ttab.txt"));
    assert!(parse_numstat(b"3\t1\0").is_err());
    assert!(parse_numstat(b"1\t1\t../escape\0").is_err());
}

#[test]
fn line_counts_cover_staged_unstaged_and_new_files_and_skip_clean_ones() {
    let repo = Repo::new();
    repo.write("a.txt", "one\ntwo\n");
    repo.write("clean.txt", "same\n");
    repo.write("gone.txt", "x\ny\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-q", "-m", "First"]);
    repo.write("a.txt", "one\nTWO\nthree\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    fs::remove_file(repo.0.join("gone.txt")).unwrap();
    repo.write("new.txt", "a\nb\nc");
    repo.write("bin.dat", [0u8, 1, 2]);
    repo.write(".gitignore", "ignored.txt\n");
    repo.write("ignored.txt", "x\n");
    let paths: Vec<PathBuf> = [
        "new.txt",
        "a.txt",
        "clean.txt",
        "gone.txt",
        "bin.dat",
        "ignored.txt",
        "missing.txt",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    let counts = line_counts(&repo.0, &paths).unwrap();
    let shown: Vec<_> = counts
        .iter()
        .map(|c| (c.path.to_str().unwrap(), c.added, c.removed, c.untracked))
        .collect();
    assert_eq!(
        shown,
        [
            ("new.txt", Some(3), Some(0), true),
            ("a.txt", Some(3), Some(1), false),
            ("gone.txt", Some(0), Some(2), false),
            ("bin.dat", None, None, true),
        ]
    );
    assert!(line_counts(&repo.0, &[PathBuf::from("../x")]).is_err());
    assert!(line_counts(&repo.0, &[]).unwrap().is_empty());
}

#[test]
fn line_counts_before_the_first_commit_count_staged_files_as_new() {
    let repo = Repo::new();
    repo.write("staged.txt", "a\nb\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("loose.txt", "c\n");
    let counts = line_counts(
        &repo.0,
        &[PathBuf::from("staged.txt"), PathBuf::from("loose.txt")],
    )
    .unwrap();
    let shown: Vec<_> = counts
        .iter()
        .map(|c| (c.path.to_str().unwrap(), c.added, c.untracked))
        .collect();
    assert_eq!(
        shown,
        [("staged.txt", Some(2), false), ("loose.txt", Some(1), true)]
    );
}

// ---- branches ----

impl Repo {
    /// Commit everything with a fixed committer time, so branch order is known.
    fn commit_at(&self, time: i64) {
        self.git(&["add", "."]);
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(["commit", "-q", "--allow-empty", "-m", "fixture"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .envs(NO_MAINTENANCE)
            .env("GIT_COMMITTER_DATE", format!("@{time} +0000"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn head_branch(&self) -> String {
        self.out(&["branch", "--show-current"]).trim().to_owned()
    }
    fn names(&self) -> Vec<String> {
        let list = branches(&self.0).unwrap().list;
        list.into_iter().map(|b| b.name).collect()
    }
}

fn create(repo: &Repo, name: &str) -> Result<WriteOutcome> {
    write(&repo.0, &WriteOp::CreateBranch(name.into()))
}

fn switch(repo: &Repo, name: &str) -> Result<WriteOutcome> {
    write(&repo.0, &WriteOp::SwitchBranch(name.into()))
}

#[test]
fn create_switches_to_a_new_branch_and_switch_goes_back() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    let head = repo.out(&["rev-parse", "HEAD"]);
    let outcome = create(&repo, "feature/login").unwrap();
    assert_eq!(outcome.summary, "Created feature/login and switched to it");
    assert_eq!(repo.head_branch(), "feature/login");
    // From the current commit.
    assert_eq!(repo.out(&["rev-parse", "HEAD"]), head);
    assert_eq!(status(&repo.0).unwrap().branch, "feature/login");
    assert_eq!(switch(&repo, "main").unwrap().summary, "Switched to main");
    assert_eq!(repo.head_branch(), "main");
    switch(&repo, "feature/login").unwrap();
    assert_eq!(repo.head_branch(), "feature/login");
}

#[test]
fn a_switch_that_would_overwrite_changes_is_refused_and_changes_nothing() {
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.commit();
    create(&repo, "side").unwrap();
    repo.write("a.txt", "side\n");
    repo.commit();
    switch(&repo, "main").unwrap();
    repo.write("a.txt", "dirty\n");
    let index = fs::read(repo.0.join(".git/index")).unwrap();
    let error = switch(&repo, "side").unwrap_err();
    // Git's own words, shown as they are.
    assert!(error.starts_with("Git: "), "{error}");
    assert!(error.contains("would be overwritten"), "{error}");
    assert_eq!(repo.head_branch(), "main");
    assert_eq!(fs::read_to_string(repo.0.join("a.txt")).unwrap(), "dirty\n");
    assert_eq!(fs::read(repo.0.join(".git/index")).unwrap(), index);
}

#[test]
fn a_change_that_does_not_conflict_is_carried_across_a_switch() {
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.write("b.txt", "base\n");
    repo.commit();
    create(&repo, "side").unwrap();
    repo.write("a.txt", "side\n");
    repo.commit();
    switch(&repo, "main").unwrap();
    repo.write("b.txt", "mine\n");
    repo.write("new.txt", "untracked\n");
    switch(&repo, "side").unwrap();
    assert_eq!(repo.head_branch(), "side");
    assert_eq!(fs::read_to_string(repo.0.join("a.txt")).unwrap(), "side\n");
    assert_eq!(fs::read_to_string(repo.0.join("b.txt")).unwrap(), "mine\n");
    assert!(repo.0.join("new.txt").exists());
    // A new branch carries them too.
    create(&repo, "carried").unwrap();
    assert_eq!(fs::read_to_string(repo.0.join("b.txt")).unwrap(), "mine\n");
}

#[test]
fn invalid_and_existing_names_are_refused_before_anything_changes() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    create(&repo, "other").unwrap();
    switch(&repo, "main").unwrap();
    let before = repo.names();
    for name in [
        "",
        "  ",
        "two words",
        "a..b",
        "-x",
        "--force",
        "@{-1}",
        "main@{1}",
        "@",
        "HEAD",
        "x.lock",
        "trailing/",
        "a\nb",
    ] {
        let error = create(&repo, name).unwrap_err();
        assert!(
            error.contains("not a valid branch name") || error.contains("Type a name"),
            "{name:?}: {error}"
        );
    }
    let error = create(&repo, "other").unwrap_err();
    assert_eq!(
        error,
        "A branch named other already exists. Choose it in the list to switch to it."
    );
    assert_eq!(repo.names(), before);
    assert_eq!(repo.head_branch(), "main");
    // `@{-1}` names the branch before this one; a switch never follows it.
    for name in ["@{-1}", "-", "missing", "HEAD", "origin/main"] {
        let error = switch(&repo, name).unwrap_err();
        assert_eq!(error, format!("There is no local branch named {name}."));
    }
    assert_eq!(repo.head_branch(), "main");
}

#[test]
fn names_with_slashes_are_branches() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    create(&repo, "team/feature/one").unwrap();
    assert_eq!(repo.head_branch(), "team/feature/one");
    // `team` cannot also be a branch: Git's own refusal is shown.
    assert!(create(&repo, "team").unwrap_err().starts_with("Git: "));
    switch(&repo, "main").unwrap();
    switch(&repo, "team/feature/one").unwrap();
    assert_eq!(repo.head_branch(), "team/feature/one");
}

#[test]
fn branches_can_be_made_and_listed_from_a_detached_head() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    repo.git(&["switch", "--detach", "HEAD"]);
    let list = branches(&repo.0).unwrap().list;
    assert_eq!(list.len(), 1);
    assert!(!list[0].current, "no branch is current on a detached HEAD");
    create(&repo, "rescued").unwrap();
    assert_eq!(repo.head_branch(), "rescued");
    repo.git(&["switch", "--detach", "HEAD"]);
    switch(&repo, "main").unwrap();
    assert_eq!(repo.head_branch(), "main");
}

#[test]
fn a_repository_with_no_commits_lists_nothing_and_renames_its_unborn_branch() {
    let repo = Repo::new();
    assert_eq!(branches(&repo.0).unwrap(), Branches::default());
    // Git moves the unborn HEAD to the new name; there is still no commit.
    create(&repo, "start").unwrap();
    assert_eq!(repo.head_branch(), "start");
    assert!(branches(&repo.0).unwrap().list.is_empty());
    assert!(
        switch(&repo, "main")
            .unwrap_err()
            .contains("no local branch")
    );
}

#[test]
fn a_held_index_lock_refuses_both_writes_and_changes_nothing() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    create(&repo, "other").unwrap();
    repo.write(".git/index.lock", "");
    let busy = "Another Git process is using this repository. Try again when it finishes.";
    assert_eq!(create(&repo, "new").unwrap_err(), busy);
    assert_eq!(switch(&repo, "main").unwrap_err(), busy);
    assert_eq!(repo.head_branch(), "other");
    assert!(!repo.names().contains(&"new".to_owned()));
}

#[test]
fn the_list_is_newest_first_marks_the_current_branch_and_states_its_cap() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit_at(1_700_000_000);
    repo.git(&["switch", "-q", "-c", "older"]);
    repo.commit_at(1_700_000_100);
    repo.git(&["switch", "-q", "-c", "newest"]);
    repo.commit_at(1_700_000_300);
    repo.git(&["switch", "-q", "-c", "middle", "older"]);
    repo.commit_at(1_700_000_200);
    let list = branches(&repo.0).unwrap();
    let shown: Vec<_> = list
        .list
        .iter()
        .map(|b| (b.name.as_str(), b.current, b.time))
        .collect();
    assert_eq!(
        shown,
        [
            ("newest", false, 1_700_000_300),
            ("middle", true, 1_700_000_200),
            ("older", false, 1_700_000_100),
            ("main", false, 1_700_000_000),
        ]
    );
    assert!(!list.more);
    // One past the cap: the list stops there and says more exist.
    let head = repo.out(&["rev-parse", "HEAD"]);
    let refs: String = (0..BRANCH_LIMIT)
        .map(|n| format!("create refs/heads/bulk/{n:04} {}\n", head.trim()))
        .collect();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&repo.0)
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(refs.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    let list = branches(&repo.0).unwrap();
    assert_eq!(list.list.len(), BRANCH_LIMIT);
    assert!(list.more);
}

#[test]
fn a_branch_in_another_worktree_is_marked_and_git_refuses_to_switch_to_it() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit();
    let tree = add_worktree(&repo.0).unwrap();
    let list = branches(&repo.0).unwrap().list;
    let named = |list: &[Branch], name: &str| list.iter().find(|b| b.name == name).unwrap().clone();
    let other = named(&list, &tree.branch);
    assert!(!other.current && other.elsewhere.is_some());
    assert!(named(&list, "main").elsewhere.is_none());
    let error = switch(&repo, &tree.branch).unwrap_err();
    assert!(
        error.starts_with("Git: ") && error.contains("worktree"),
        "{error}"
    );
    assert_eq!(repo.head_branch(), "main");
    // The worktree's own view: its branch is current, main is elsewhere.
    let inside = branches(&tree.path).unwrap().list;
    assert!(named(&inside, &tree.branch).current);
    assert!(named(&inside, "main").elsewhere.is_some());
    remove_worktree(&repo.0, &tree.path).unwrap();
}

#[test]
fn branch_records_are_validated() {
    let list =
        parse_branches(b"refs/heads/main\0*\x001700000000\0/repo\nrefs/heads/x\0 \x0012\0\n")
            .unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].current && list[0].elsewhere.is_none());
    assert_eq!((list[1].name.as_str(), list[1].time), ("x", 12));
    assert!(parse_branches(b"refs/tags/v1\0 \x001\0\n").is_err());
    assert!(parse_branches(b"refs/heads/a\0 \x00soon\0\n").is_err());
    assert!(parse_branches(b"refs/heads/a\0 \n").is_err());
    // A name that is not UTF-8 is left out rather than changed.
    assert!(
        parse_branches(b"refs/heads/\xff\0 \x001\0\n")
            .unwrap()
            .is_empty()
    );
    assert!(parse_branches(b"").unwrap().is_empty());
}

#[test]
fn writer_lease_blocks_every_desktop_git_write_including_via_alias() {
    use crate::lease::{CHECKOUT_BUSY, LeaseClass, acquire_checkout};
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.commit();
    repo.git(&["branch", "other"]);
    repo.write("a.txt", "staged\n");
    stage(&repo, &["a.txt"]).unwrap();
    repo.write("a.txt", "working\n");
    let unstaged = diff(
        &repo.0,
        &status(&repo.0).unwrap().changes[0],
        DiffMode::Unstaged,
    )
    .unwrap();
    let backup = write(&repo.0, &discard_op("a.txt", false, None))
        .unwrap()
        .backup
        .unwrap();
    write(&repo.0, &WriteOp::Restore(backup.clone())).unwrap();
    let alias = repo.0.parent().unwrap().join(format!(
        "plait-alias-{}",
        repo.0.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_file(&alias);
    std::os::unix::fs::symlink(&repo.0, &alias).unwrap();

    let writer = acquire_checkout(&repo.0, None, LeaseClass::Writer).unwrap();
    let ops = [
        WriteOp::Stage(paths(&["a.txt"])),
        WriteOp::Unstage(paths(&["a.txt"])),
        WriteOp::StageAll,
        WriteOp::UnstageAll,
        WriteOp::Commit {
            message: "should not commit".into(),
        },
        WriteOp::Push,
        WriteOp::Publish,
        WriteOp::Fetch,
        WriteOp::Pull,
        hunk_op(&unstaged, DiffMode::Unstaged, 0),
        discard_op("a.txt", false, None),
        WriteOp::Restore(backup),
        WriteOp::CreateBranch("blocked".into()),
        WriteOp::SwitchBranch("other".into()),
    ];
    for op in &ops {
        assert_eq!(write(&repo.0, op).unwrap_err(), CHECKOUT_BUSY, "{op:?}");
        assert_eq!(
            write(&alias, op).unwrap_err(),
            CHECKOUT_BUSY,
            "alias {op:?}"
        );
    }
    // Working tree, branch, and commit history were untouched.
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        "working\n"
    );
    assert_eq!(repo.head_branch(), "main");
    assert_eq!(log(&repo.0, None, 0, 10).unwrap().commits.len(), 1);
    drop(writer);

    // Once released, desktop Git mutations proceed normally.
    write(
        &repo.0,
        &WriteOp::Commit {
            message: "after release".into(),
        },
    )
    .unwrap();
    assert_eq!(log(&repo.0, None, 0, 10).unwrap().commits.len(), 2);
    let _ = fs::remove_file(&alias);
}

#[test]
fn fake_reader_lease_blocks_desktop_git_writes_and_worktree_removal_while_allowing_worktree_add() {
    use crate::lease::{CHECKOUT_BUSY, LeaseClass, acquire_checkout};
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.commit();
    repo.git(&["branch", "side"]);
    repo.write("a.txt", "edited\n");

    let reader_main = acquire_checkout(&repo.0, None, LeaseClass::Reader).unwrap();
    // Desktop mutations on main are refused while a reader holds lifecycle-shared.
    assert_eq!(
        write(&repo.0, &WriteOp::StageAll).unwrap_err(),
        CHECKOUT_BUSY
    );
    assert_eq!(
        write(&repo.0, &WriteOp::SwitchBranch("side".into())).unwrap_err(),
        CHECKOUT_BUSY
    );
    assert_eq!(
        write(&repo.0, &discard_op("a.txt", false, None)).unwrap_err(),
        CHECKOUT_BUSY
    );
    // Creating a new worktree from main only takes shared lifecycle + common_dir,
    // so it succeeds even while a reader (or writer) is active in main.
    let tree = add_worktree(&repo.0).unwrap();
    drop(reader_main);

    // Holding a reader lease on the worktree itself refuses removing that worktree.
    let reader_wt = acquire_checkout(&tree.path, None, LeaseClass::Reader).unwrap();
    assert_eq!(
        remove_worktree(&repo.0, &tree.path).unwrap_err(),
        CHECKOUT_BUSY
    );
    assert!(tree.path.is_dir());
    drop(reader_wt);
    remove_worktree(&repo.0, &tree.path).unwrap();
    assert!(!tree.path.exists());
}

#[test]
fn writer_lease_blocks_worktree_removal_while_unrelated_checkout_progresses() {
    use crate::lease::{CHECKOUT_BUSY, LeaseClass, acquire_checkout};
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.commit();
    let other = Repo::new();
    other.write("b.txt", "base\n");
    other.commit();

    let tree = add_worktree(&repo.0).unwrap();
    let alias = repo.0.parent().unwrap().join(format!(
        "plait-wt-alias-{}",
        repo.0.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_file(&alias);
    std::os::unix::fs::symlink(&tree.path, &alias).unwrap();

    let writer = acquire_checkout(&tree.path, None, LeaseClass::Writer).unwrap();
    assert_eq!(
        remove_worktree(&repo.0, &tree.path).unwrap_err(),
        CHECKOUT_BUSY
    );
    assert_eq!(remove_worktree(&repo.0, &alias).unwrap_err(), CHECKOUT_BUSY);
    assert!(tree.path.is_dir());

    // An unrelated checkout continues committing and switching branches freely.
    other.write("b.txt", "updated\n");
    write(&other.0, &WriteOp::StageAll).unwrap();
    write(
        &other.0,
        &WriteOp::Commit {
            message: "unrelated commit".into(),
        },
    )
    .unwrap();
    write(&other.0, &WriteOp::CreateBranch("unrelated-branch".into())).unwrap();
    assert_eq!(other.head_branch(), "unrelated-branch");

    drop(writer);
    let _ = fs::remove_file(&alias);
    remove_worktree(&repo.0, &tree.path).unwrap();
    assert!(!tree.path.exists());
}

#[test]
fn shared_metadata_lock_serializes_linked_worktree_ops_and_times_out_truthfully() {
    use crate::lease::{COMMON_METADATA_BUSY, acquire_common_metadata};
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.commit();
    let tree = add_worktree(&repo.0).unwrap();

    // While common_dir metadata is held, short-wait ops refuse truthfully.
    let held = acquire_common_metadata(&repo.0, Duration::ZERO).unwrap();
    let short = Duration::from_millis(20);
    assert_eq!(
        super::write::write_with_common_wait(
            &tree.path,
            None,
            &WriteOp::CreateBranch("delayed".into()),
            short
        )
        .unwrap_err(),
        COMMON_METADATA_BUSY
    );
    assert_eq!(
        super::worktree::add_worktree_with_common_wait(&repo.0, short).unwrap_err(),
        COMMON_METADATA_BUSY
    );
    assert_eq!(
        super::worktree::remove_worktree_with_common_wait(&repo.0, &tree.path, short).unwrap_err(),
        COMMON_METADATA_BUSY
    );
    // Non-metadata per-worktree staging in the sibling worktree does not need common_dir.
    fs::write(tree.path.join("local.txt"), "hi\n").unwrap();
    write(&tree.path, &WriteOp::Stage(paths(&["local.txt"]))).unwrap();
    write(&tree.path, &WriteOp::UnstageAll).unwrap();
    fs::remove_file(tree.path.join("local.txt")).unwrap();

    // Bounded wait serializes a sibling worktree branch creation behind the holder.
    let started = Instant::now();
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(120));
        drop(held);
    });
    write(&tree.path, &WriteOp::CreateBranch("serialized".into())).unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(90),
        "sibling worktree metadata write did not wait for shared metadata lock"
    );
    releaser.join().unwrap();

    remove_worktree(&repo.0, &tree.path).unwrap();
}

/// The process `a_killed_worktree_removal_keeps_the_lease_until_git_exits` kills
/// while its `git worktree remove` is still running. Does nothing otherwise.
#[test]
fn worktree_removal_whose_process_is_killed() {
    let Some(spec) = std::env::var_os("PLAIT_TEST_KILLED_REMOVAL") else {
        return;
    };
    let spec = spec.into_string().unwrap();
    let (main, target) = spec.split_once('\n').unwrap();
    let _ = remove_worktree(Path::new(main), Path::new(target));
}

#[test]
fn a_killed_worktree_removal_keeps_the_lease_until_git_exits() {
    use crate::lease::{LeaseClass, LeaseErrorKind, acquire_checkout, acquire_common_metadata};
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.commit();
    let tree = add_worktree(&repo.0).unwrap();
    let scratch = repo.0.parent().unwrap().join(format!(
        "plait-killed-removal-{}",
        repo.0.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir(&scratch).unwrap();
    let real = Command::new("/bin/sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
    let marker = scratch.join("git-started");
    // Git for the removal waits a moment first, so the process can die while it runs.
    let wrapper = format!(
        "#!/bin/sh\ncase \"$*\" in *'worktree remove'*) echo $$ > '{}'; sleep 2;; esac\nexec '{real}' \"$@\"\n",
        marker.display()
    );
    fs::write(scratch.join("git"), wrapper).unwrap();
    let mut perms = fs::metadata(scratch.join("git")).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    fs::set_permissions(scratch.join("git"), perms).unwrap();
    let path = format!(
        "{}:{}",
        scratch.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args([
            "git::tests::worktree_removal_whose_process_is_killed",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PATH", path)
        .env(
            "PLAIT_TEST_KILLED_REMOVAL",
            format!("{}\n{}", repo.0.display(), tree.path.display()),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the removal never started Git");
        thread::sleep(Duration::from_millis(10));
    }
    parent.kill().unwrap();
    parent.wait().unwrap();

    // Git is still running: the worktree and the repository's shared metadata stay held.
    assert_eq!(
        acquire_checkout(&tree.path, None, LeaseClass::Exclusive)
            .unwrap_err()
            .kind,
        LeaseErrorKind::CleanupUncertain
    );
    assert_eq!(
        acquire_common_metadata(&repo.0, Duration::ZERO)
            .unwrap_err()
            .kind,
        LeaseErrorKind::CommonMetadataBusy
    );
    // Once Git has finished, it has removed the worktree and the lease is free.
    let deadline = Instant::now() + Duration::from_secs(20);
    while acquire_common_metadata(&repo.0, Duration::ZERO).is_err() {
        assert!(Instant::now() < deadline, "Git never released the lease");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!tree.path.exists());
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn a_plain_folder_reads_as_plain_with_its_files_but_a_repository_never_does() {
    let root = std::env::temp_dir().join(format!("plait-plain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("notes/deep")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::fs::write(root.join("notes/a.md"), "a").unwrap();
    std::fs::write(root.join("notes/deep/b.md"), "b").unwrap();
    std::fs::write(root.join(".hidden/c"), "c").unwrap();
    std::fs::write(root.join("top.txt"), "t").unwrap();
    assert!(super::plain_folder(&root));
    let snapshot = super::status(&root).unwrap();
    assert!(snapshot.plain && snapshot.changes.is_empty());
    assert_eq!(snapshot.root, root.canonicalize().unwrap());
    let files = super::files(&root).unwrap();
    assert_eq!(files.paths, ["notes/a.md", "notes/deep/b.md", "top.txt"]);
    assert!(
        super::line_counts(&root, &["top.txt".into()])
            .unwrap()
            .is_empty()
    );
    // A limit stops the walk and says so.
    let clipped = super::plain_files(&root, 2, std::time::Duration::from_secs(2));
    assert!(clipped.truncated && clipped.paths.len() == 2);
    // An empty stray `.git` folder is no repository, as Git itself decides.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    assert!(super::plain_folder(&root.join("notes")));
    // With HEAD it marks a repository, even a broken one: never plain.
    std::fs::write(root.join(".git/HEAD"), "garbage").unwrap();
    assert!(!super::plain_folder(&root.join("notes")));
    assert!(
        super::status(&root.join("notes")).is_err(),
        "a broken repository read as plain"
    );
    std::fs::remove_dir_all(root).unwrap();
}
