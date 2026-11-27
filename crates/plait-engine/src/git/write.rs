//! Explicit Git writes the user asks for: staging, unstaging, committing,
//! exchanging commits with the branch's remote (push, publish, fetch), and
//! creating or switching local branches (see `branch`).
//!
//! Reads elsewhere in `git` neutralize filters and never run hooks. A write the
//! user chose must produce what their own `git add` or `git commit` would, so
//! it runs with the repository's configuration: clean filters (such as LFS)
//! run while staging, and commit hooks run while committing.

use super::{
    DIFF_LIMIT, Result, STATUS_LIMIT, TIMEOUT, base_command, command, display_line, head_exists,
    log::{is_object_id, log},
    open_beneath, read_command, run, validate_relative,
};
use crate::model::DiffMode;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

/// Filters (LFS) can be slow on large files.
const STAGE_TIMEOUT: Duration = Duration::from_secs(60);
/// Pre-commit hooks may run formatters or tests.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(120);
/// Push and fetch may run hooks and cross a slow network.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_PATHS: usize = 100_000;
/// Hook output shown to the user is clipped to this many characters.
const ERROR_TEXT_LIMIT: usize = 2000;
pub(super) const BUSY: &str =
    "Another Git process is using this repository. Try again when it finishes.";

pub use plait_protocol::git::write::{Backup, WriteOp, WriteOutcome};

/// Whether this write updates shared repository metadata (refs, upstream config,
/// or branch checkout state) stored under `--git-common-dir`.
pub fn needs_common_metadata_lock(op: &WriteOp) -> bool {
    matches!(
        op,
        WriteOp::Commit { .. }
            | WriteOp::Push
            | WriteOp::Publish
            | WriteOp::Fetch
            | WriteOp::Pull
            | WriteOp::CreateBranch(_)
            | WriteOp::SwitchBranch(_)
    )
}

/// Run one write in the repository at `root` under authoritative checkout
/// lifecycle and writer leases. Blocking: call on a background thread.
pub fn write(root: &Path, op: &WriteOp) -> Result<WriteOutcome> {
    write_checked(root, None, op)
}

/// Run one write after verifying `expected` directory identity under the
/// acquired checkout leases so root swaps cannot race admission.
pub fn write_checked(
    root: &Path,
    expected: Option<crate::lease::DirIdentity>,
    op: &WriteOp,
) -> Result<WriteOutcome> {
    write_with_common_wait(root, expected, op, crate::lease::COMMON_METADATA_WAIT)
}

pub(crate) fn write_with_common_wait(
    root: &Path,
    expected: Option<crate::lease::DirIdentity>,
    op: &WriteOp,
    common_wait: Duration,
) -> Result<WriteOutcome> {
    let common = needs_common_metadata_lock(op).then_some(common_wait);
    let lease = crate::lease::acquire_git_checkout(
        root,
        expected,
        crate::lease::LeaseClass::Exclusive,
        common,
    )
    .map_err(String::from)?;
    crate::lease::git_cleanup::with_lease(&lease, || write_unlocked(lease.root(), op))
}

fn write_unlocked(root: &Path, op: &WriteOp) -> Result<WriteOutcome> {
    match op {
        WriteOp::Stage(paths) => {
            let input = pathspec_input(paths)?;
            let summary = format!("Staged {}", files(paths_count(paths)));
            git(
                root,
                &["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"],
                Some(input),
                STAGE_TIMEOUT,
            )?;
            Ok(done(summary))
        }
        WriteOp::StageAll => {
            git(root, &["add", "-A"], None, STAGE_TIMEOUT)?;
            Ok(done("Staged all changes".into()))
        }
        WriteOp::Unstage(paths) => {
            let input = pathspec_input(paths)?;
            let summary = format!("Unstaged {}", files(paths_count(paths)));
            unstage(root, Some(input))?;
            Ok(done(summary))
        }
        WriteOp::UnstageAll => {
            unstage(root, None)?;
            Ok(done("Unstaged all changes".into()))
        }
        WriteOp::Commit { message } => commit(root, message),
        WriteOp::Push => {
            remote_git(root, &["push", "-q"])?;
            Ok(done(match destination(root) {
                Some(to) => format!("Pushed to {to}"),
                None => "Pushed".into(),
            }))
        }
        WriteOp::Publish => {
            let remote = publish_remote(root)?;
            remote_git(
                root,
                &["push", "-q", "-u", "--end-of-options", &remote, "HEAD"],
            )?;
            Ok(done(format!("Published to {remote}")))
        }
        WriteOp::Fetch => {
            remote_git(root, &["fetch", "-q"])?;
            Ok(done("Fetched".into()))
        }
        WriteOp::Pull => pull(root),
        WriteOp::Hunk {
            path,
            from,
            index,
            header,
        } => hunk(root, path, *from, *index, header),
        WriteOp::Discard {
            path,
            untracked,
            hunk,
            guard,
        } => discard(
            root,
            path,
            *untracked,
            hunk.as_ref(),
            guard.as_ref().and_then(|g| g.digest.as_deref()),
        ),
        WriteOp::Restore(backup) => restore(root, backup),
        WriteOp::CreateBranch(name) => super::branch::create(root, name).map(done),
        WriteOp::SwitchBranch(name) => super::branch::switch(root, name).map(done),
    }
}

/// Larger files are not copied before a discard, so the discard is refused.
const BACKUP_LIMIT: usize = 64 * 1024 * 1024;

fn discard(
    root: &Path,
    path: &Path,
    untracked: bool,
    hunk: Option<&(usize, String)>,
    reviewed_digest: Option<&str>,
) -> Result<WriteOutcome> {
    validate_relative(path)?;
    if hunk.is_some() && has_filter(root, path)? {
        return Err(
            "This file passes through a Git filter (such as LFS). Discard the whole file.".into(),
        );
    }
    // Save first: if this fails, nothing has been thrown away.
    let backup = save(root, path, untracked)?;
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let summary = match (untracked, hunk) {
        (true, _) => {
            git_path(root, &["clean", "-f", "-q"], path)?;
            format!("Deleted {name}")
        }
        (false, None) => {
            git_path(root, &["restore", "--worktree"], path)?;
            format!("Discarded changes to {name}")
        }
        (false, Some((index, header))) => {
            use std::ffi::OsStr;
            let args: Vec<&OsStr> = [
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                "--ignore-submodules=dirty",
                "--submodule=short",
                "--",
            ]
            .into_iter()
            .map(OsStr::new)
            .chain([path.as_os_str()])
            .collect();
            let diff = command(root, &args, DIFF_LIMIT)?;
            if diff.truncated {
                return Err(
                    "This diff is too large to discard by hunk. Discard the whole file.".into(),
                );
            }
            use sha2::{Digest, Sha256};
            if reviewed_digest
                .is_some_and(|digest| format!("{:x}", Sha256::digest(&diff.bytes)) != digest)
            {
                return Err(STALE.into());
            }
            let patch = hunk_patch(&diff.bytes, *index, header)?;
            git(
                root,
                &["apply", "--reverse", "--whitespace=nowarn", "-"],
                Some(patch),
                STAGE_TIMEOUT,
            )
            .map_err(|error| {
                if error.contains("does not apply") {
                    STALE.to_owned()
                } else {
                    error
                }
            })?;
            format!("Discarded a hunk of {name}")
        }
    };
    Ok(WriteOutcome {
        summary,
        commit: None,
        backup,
    })
}

/// Copy the file's bytes into the object store, unfiltered. A missing file or
/// a symlink has nothing to save.
fn save(root: &Path, path: &Path, untracked: bool) -> Result<Option<Backup>> {
    use std::{io::Read, os::unix::fs::PermissionsExt};
    let file = match open_beneath(root, path, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
        Ok(file) => file,
        Err(e) if untracked => return Err(format!("Cannot save a copy before discarding: {e}")),
        Err(_) => return Ok(None),
    };
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        if untracked {
            return Err("Plait can only save and discard a regular file.".into());
        }
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(BACKUP_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > BACKUP_LIMIT {
        return Err(
            "This file is too large to keep a copy of, so Plait will not discard it.".into(),
        );
    }
    let mut command = write_command(root);
    command.args(["hash-object", "-w", "--no-filters", "--stdin"]);
    let raw = run_write(
        &mut command,
        Some(bytes),
        STATUS_LIMIT,
        STAGE_TIMEOUT,
        "git hash-object",
    )?;
    let blob = String::from_utf8_lossy(&raw.out.bytes).trim().to_owned();
    if !raw.status.success() || !is_object_id(&blob) {
        return Err(failure_text(&raw));
    }
    Ok(Some(Backup {
        path: path.to_owned(),
        blob,
        executable: meta.permissions().mode() & 0o111 != 0,
    }))
}

fn restore(root: &Path, backup: &Backup) -> Result<WriteOutcome> {
    use std::io::Write;
    validate_relative(&backup.path)?;
    if !is_object_id(&backup.blob) {
        return Err("The saved copy has an invalid ID.".into());
    }
    let mut command = write_command(root);
    command.args(["cat-file", "blob", &backup.blob]);
    let raw = run(&mut command, None, BACKUP_LIMIT, STAGE_TIMEOUT)?;
    if !raw.status.success() || raw.out.truncated {
        return Err(failure_text(&raw));
    }
    let mode = if backup.executable { 0o755 } else { 0o644 };
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
    let mut file = open_beneath(root, &backup.path, flags, mode)
        .map_err(|e| format!("Cannot restore the file: {e}"))?;
    file.write_all(&raw.out.bytes).map_err(|e| e.to_string())?;
    let name = backup
        .path
        .file_name()
        .unwrap_or(backup.path.as_os_str())
        .to_string_lossy();
    Ok(done(format!("Restored {name}")))
}

/// A write whose last argument is one literal path, which may not be UTF-8.
fn git_path(root: &Path, args: &[&str], path: &Path) -> Result<()> {
    let mut command = write_command(root);
    command.args(args).arg("--").arg(path);
    let raw = run_write(
        &mut command,
        None,
        STATUS_LIMIT,
        STAGE_TIMEOUT,
        &format!("git {}", args[0]),
    )?;
    if raw.status.success() {
        Ok(())
    } else {
        Err(failure_text(&raw))
    }
}

const STALE: &str = "The file changed since its diff was shown. Plait reread it; try again.";

fn hunk(
    root: &Path,
    path: &Path,
    from: DiffMode,
    index: usize,
    header: &str,
) -> Result<WriteOutcome> {
    use std::ffi::OsStr;
    validate_relative(path)?;
    if has_filter(root, path)? {
        return Err(
            "This file passes through a Git filter (such as LFS). Stage the whole file.".into(),
        );
    }
    let staged = from == DiffMode::Staged;
    // The same diff the view reads, so hunk numbering and headers agree.
    let mut args: Vec<&OsStr> = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--unified=3",
    ]
    .into_iter()
    .map(OsStr::new)
    .collect();
    if staged {
        args.push(OsStr::new("--cached"));
    }
    args.extend([OsStr::new("--"), path.as_os_str()]);
    let diff = command(root, &args, DIFF_LIMIT)?;
    if diff.truncated {
        return Err("This diff is too large to stage by hunk. Stage the whole file.".into());
    }
    let patch = hunk_patch(&diff.bytes, index, header)?;
    let mut apply = vec!["apply", "--cached", "--whitespace=nowarn"];
    if staged {
        apply.push("--reverse");
    }
    apply.push("-");
    git(root, &apply, Some(patch), STAGE_TIMEOUT).map_err(|error| {
        if error.contains("does not apply") || error.contains("does not match index") {
            STALE.to_owned()
        } else {
            error
        }
    })?;
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    Ok(done(format!(
        "{} a hunk of {name}",
        if staged { "Unstaged" } else { "Staged" }
    )))
}

/// A patch of the file header plus hunk `index`, if that hunk's header still
/// reads as `header`. Splits on newlines only, keeping them, so bytes pass
/// through unchanged.
pub(super) fn hunk_patch(diff: &[u8], index: usize, header: &str) -> Result<Vec<u8>> {
    let lines: Vec<&[u8]> = diff.split_inclusive(|b| *b == b'\n').collect();
    if lines
        .iter()
        .filter(|line| line.starts_with(b"diff "))
        .count()
        != 1
    {
        return Err("Stage this file whole; its diff is not a single text change.".into());
    }
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].starts_with(b"@@ "))
        .collect();
    let first = *starts.first().ok_or("This diff has no hunks to stage.")?;
    let start = *starts.get(index).ok_or(STALE)?;
    let end = starts.get(index + 1).copied().unwrap_or(lines.len());
    let raw = String::from_utf8_lossy(lines[start]);
    let raw = raw.trim_end_matches('\n').trim_end_matches('\r');
    if display_line(raw) != header {
        return Err(STALE.into());
    }
    Ok([&lines[..first], &lines[start..end]].concat().concat())
}

/// A clean or smudge filter means the index holds something other than the
/// file's bytes, so a patch of those bytes would be wrong.
fn has_filter(root: &Path, path: &Path) -> Result<bool> {
    let mut command = read_command(root)?;
    command.args(["check-attr", "-z", "filter", "--"]).arg(path);
    let raw = run(&mut command, None, STATUS_LIMIT, TIMEOUT)?;
    if !raw.status.success() {
        return Err(raw.failure());
    }
    // -z output is path NUL attribute NUL value NUL.
    let value = raw.out.bytes.split(|b| *b == 0).nth(2).unwrap_or_default();
    Ok(!matches!(value, b"unspecified" | b"unset" | b""))
}

/// Two explicit steps instead of `git pull`, whose configuration may rebase.
fn pull(root: &Path) -> Result<WriteOutcome> {
    remote_git(root, &["fetch", "-q"])?;
    let count = |range: &str| -> Result<usize> {
        let mut command = write_command(root);
        command.args(["rev-list", "--count", range]);
        let raw = run(&mut command, None, STATUS_LIMIT, TIMEOUT)?;
        if !raw.status.success() {
            return Err(failure_text(&raw));
        }
        String::from_utf8_lossy(&raw.out.bytes)
            .trim()
            .parse()
            .map_err(|_| "Git returned an unreadable commit count.".to_owned())
    };
    let behind = count("HEAD..@{upstream}")?;
    if behind == 0 {
        return Ok(done("Already up to date".into()));
    }
    if count("@{upstream}..HEAD")? > 0 {
        return Err(DIVERGED.into());
    }
    let mut command = write_command(root);
    command.args(["merge", "--ff-only", "-q", "@{upstream}"]);
    let raw = run_write(
        &mut command,
        None,
        STATUS_LIMIT,
        NETWORK_TIMEOUT,
        "git merge --ff-only",
    )?;
    if !raw.status.success() {
        let text = failure_text(&raw);
        return Err(if text.contains("would be overwritten") {
            format!(
                "Pulling would overwrite uncommitted changes. Commit, stash, or discard them first.\n{text}"
            )
        } else {
            text
        });
    }
    Ok(done(format!(
        "Pulled {behind} {}",
        if behind == 1 { "commit" } else { "commits" }
    )))
}

const DIVERGED: &str =
    "This branch and its upstream have each moved on. Merge or rebase them in Terminal.";

/// The ref a plain push updates, such as `origin/main`, for the summary.
fn destination(root: &Path) -> Option<String> {
    let mut command = write_command(root);
    command.args([
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{push}",
    ]);
    let raw = run(&mut command, None, STATUS_LIMIT, TIMEOUT).ok()?;
    let text = String::from_utf8_lossy(&raw.out.bytes).trim().to_owned();
    (raw.status.success() && !text.is_empty()).then_some(text)
}

/// `remote.pushDefault`, else `origin`, else the only remote.
fn publish_remote(root: &Path) -> Result<String> {
    let read = |args: &[&str]| -> Result<String> {
        let mut command = write_command(root);
        let raw = run(command.args(args), None, STATUS_LIMIT, TIMEOUT)?;
        Ok(String::from_utf8_lossy(&raw.out.bytes).into_owned())
    };
    let preferred = read(&["config", "--get", "remote.pushDefault"])?;
    let remotes = read(&["remote"])?;
    let remotes: Vec<&str> = remotes
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect();
    let chosen = match preferred.trim() {
        name if !name.is_empty() => name,
        _ if remotes.contains(&"origin") => "origin",
        _ if remotes.len() == 1 => remotes[0],
        _ if remotes.is_empty() => {
            return Err("This repository has no remote to publish to.".into());
        }
        _ => {
            return Err(
                "There are several remotes and none is named origin. Publish this branch once from Terminal."
                    .into(),
            );
        }
    };
    Ok(chosen.to_owned())
}

/// A push or fetch. The user's credential helpers are asked first; the GitHub
/// CLI's helper, when installed, is tried after them, so a `gh auth login` is
/// enough for HTTPS remotes. Git hands the token to itself; Plait never sees it.
fn remote_git(root: &Path, args: &[&str]) -> Result<()> {
    let mut command = write_command(root);
    if let Some(helper) = gh_helper() {
        command.args(["-c", helper]);
    }
    let raw = run_write(
        command.args(args),
        None,
        STATUS_LIMIT,
        NETWORK_TIMEOUT,
        &format!("git {}", args[0]),
    )?;
    if raw.status.success() {
        return Ok(());
    }
    let text = failure_text(&raw);
    Err(network_hint(&text).map_or(text.clone(), |hint| format!("{hint}\n{text}")))
}

/// `credential.helper=!'/path/to/gh' auth git-credential`, found once.
fn gh_helper() -> Option<&'static str> {
    static HELPER: OnceLock<Option<String>> = OnceLock::new();
    HELPER
        .get_or_init(|| {
            let gh = find_program("gh")?;
            let gh = gh.to_str()?;
            // Quoted for the shell Git runs `!` helpers in.
            let quoted = format!("'{}'", gh.replace('\'', r"'\''"));
            Some(format!("credential.helper=!{quoted} auth git-credential"))
        })
        .as_deref()
}

/// An app started from Finder has a minimal PATH, so also look where
/// package managers install command-line tools.
fn find_program(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/run/current-system/sw/bin",
            ]
            .map(PathBuf::from),
        )
        .chain(
            home.iter()
                .flat_map(|home| [home.join(".nix-profile/bin"), home.join(".local/bin")]),
        )
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// What to do about the common push and fetch failures, in words.
pub(super) fn network_hint(text: &str) -> Option<&'static str> {
    let has = |needle: &str| text.contains(needle);
    if has("could not read Username")
        || has("terminal prompts disabled")
        || has("Authentication failed")
    {
        Some(
            "The remote needs a sign-in. For GitHub over HTTPS, run `gh auth login`; otherwise set up a credential helper or an SSH key.",
        )
    } else if has("Permission denied (publickey)") {
        Some(
            "The remote refused your SSH key. Check that ssh-agent holds a key the remote accepts.",
        )
    } else if has("Host key verification failed") {
        Some(
            "SSH does not know this remote's host key yet. Connect once from Terminal to confirm it.",
        )
    } else if has("[rejected]") || has("non-fast-forward") || has("fetch first") {
        Some("The remote has commits this branch does not. Pull them first, then push again.")
    } else if has("Could not resolve host")
        || has("Network is unreachable")
        || has("Connection timed out")
    {
        Some("Cannot reach the remote. Check the network connection.")
    } else {
        None
    }
}

fn done(summary: String) -> WriteOutcome {
    WriteOutcome {
        summary,
        commit: None,
        backup: None,
    }
}

fn files(count: usize) -> String {
    match count {
        1 => "1 file".into(),
        n => format!("{n} files"),
    }
}

fn unique(paths: &[PathBuf]) -> BTreeSet<&Path> {
    paths.iter().map(PathBuf::as_path).collect()
}

fn paths_count(paths: &[PathBuf]) -> usize {
    unique(paths).len()
}

/// NUL-terminated pathspecs for `--pathspec-from-file=-`, which has no argv length limit.
fn pathspec_input(paths: &[PathBuf]) -> Result<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let paths = unique(paths);
    if paths.is_empty() {
        return Err("Choose at least one file.".into());
    }
    if paths.len() > MAX_PATHS {
        return Err(format!("Too many files at once (limit {MAX_PATHS})."));
    }
    let mut input = Vec::new();
    for path in paths {
        validate_relative(path)?;
        let bytes = path.as_os_str().as_bytes();
        if bytes.contains(&0) {
            return Err("Git returned an unsafe relative path".into());
        }
        input.extend_from_slice(bytes);
        input.push(0);
    }
    Ok(input)
}

/// Return paths (or everything) from the index to their HEAD state.
fn unstage(root: &Path, input: Option<Vec<u8>>) -> Result<()> {
    let nul = ["--pathspec-from-file=-", "--pathspec-file-nul"];
    let timeout = STAGE_TIMEOUT;
    if head_exists(root)? {
        return match input {
            Some(input) => {
                let args = ["restore", "--staged", nul[0], nul[1]];
                git(root, &args, Some(input), timeout).map(drop)
            }
            None => git(root, &["reset", "-q", "HEAD", "--", "."], None, timeout).map(drop),
        };
    }
    // No HEAD to restore from: dropping index entries returns new files to untracked.
    // -f because the index entry may differ from both the file and the (absent) HEAD.
    match input {
        Some(input) => {
            let args = ["rm", "--cached", "-r", "-q", "-f", nul[0], nul[1]];
            git(root, &args, Some(input), timeout).map(drop)
        }
        None => {
            let args = [
                "rm",
                "--cached",
                "-r",
                "-q",
                "-f",
                "--ignore-unmatch",
                "--",
                ".",
            ];
            git(root, &args, None, timeout).map(drop)
        }
    }
}

fn commit(root: &Path, message: &str) -> Result<WriteOutcome> {
    let message = message.trim_end();
    if message.trim().is_empty() {
        return Err("Write a commit message first.".into());
    }
    if message.contains('\0') {
        return Err("The commit message cannot contain NUL characters.".into());
    }
    if !has_staged_changes(root)? {
        return Err("Nothing is staged. Stage changes before committing.".into());
    }
    let mut input = message.as_bytes().to_vec();
    input.push(b'\n');
    // whitespace cleanup keeps lines that start with '#', unlike Git's default.
    let args = ["commit", "-q", "--cleanup=whitespace", "-F", "-"];
    git(root, &args, Some(input), COMMIT_TIMEOUT)?;
    // Read back what Git recorded, after hooks and cleanup.
    let head = log(root, None, 0, 1)?
        .commits
        .into_iter()
        .next()
        .ok_or("Git committed but HEAD is unreadable")?;
    Ok(WriteOutcome {
        summary: format!("Committed {}: {}", head.short, head.subject),
        commit: Some(head.id),
        backup: None,
    })
}

/// `diff --cached --quiet` exits 1 when the index differs from HEAD (or from empty).
fn has_staged_changes(root: &Path) -> Result<bool> {
    let mut command = write_command(root);
    let raw = run(
        command.args(["diff", "--cached", "--quiet", "--no-ext-diff"]),
        None,
        STATUS_LIMIT,
        TIMEOUT,
    )?;
    match raw.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(failure_text(&raw)),
    }
}

/// Repository configuration applies as for the user's own command; only the
/// terminal-facing and ambient behavior is pinned.
fn write_command(root: &Path) -> Command {
    let mut command = base_command(root);
    // A commit must never wait on an editor nobody can see.
    command
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true");
    command
}

pub(super) fn git(
    root: &Path,
    args: &[&str],
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<()> {
    finish(write_command(root), args, input, timeout)
}

/// As `git`, with `lease` lent to the Git process so a crash of this one does
/// not free the checkout while Git still writes it. Only for a command that
/// starts no long-lived helper (see `CheckoutLease::lend_to`).
pub(super) fn git_holding(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    lease: &crate::lease::CheckoutLease,
) -> Result<()> {
    let mut command = write_command(root);
    lease.lend_to(&mut command);
    crate::lease::git_cleanup::with_lease(lease, || finish(command, args, None, timeout))
}

fn finish(
    mut command: std::process::Command,
    args: &[&str],
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<()> {
    // A stopped write can leave Git's lock behind; say how to recover.
    let label = format!(
        "git {}",
        args.iter()
            .take(if args.first() == Some(&"worktree") {
                2
            } else {
                1
            })
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    );
    let raw =
        run_write(command.args(args), input, STATUS_LIMIT, timeout, &label).map_err(|error| {
            if error.contains("was stopped") {
                format!(
                    "{error} If .git/index.lock remains, remove it once no Git process is running."
                )
            } else {
                error
            }
        })?;
    if raw.status.success() {
        return Ok(());
    }
    Err(failure_text(&raw))
}

fn run_write(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
    label: &str,
) -> Result<super::Raw> {
    let recording = crate::lease::git_cleanup::prepare(label)?;
    super::run_recorded(command, input, limit, timeout, recording)
}

/// A user-facing reason: Git's stderr plus any stdout, so hook output shows.
fn failure_text(raw: &super::Raw) -> String {
    let mut text = String::from_utf8_lossy(&raw.err.bytes).trim().to_owned();
    let out = String::from_utf8_lossy(&raw.out.bytes);
    if !out.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(out.trim());
    }
    explain(&text, raw.status.code())
}

fn explain(text: &str, code: Option<i32>) -> String {
    if text.contains("index.lock") {
        return BUSY.into();
    }
    if text.is_empty() {
        return format!("Git failed with status {}.", code.unwrap_or(-1));
    }
    let mut shown: String = text.chars().take(ERROR_TEXT_LIMIT).collect();
    if text.chars().count() > ERROR_TEXT_LIMIT {
        shown.push_str(" … [output clipped]");
    }
    format!("Git: {shown}")
}
