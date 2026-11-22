//! Local branches: listing them, and the two branch writes the user may ask
//! for, creating one from the current commit and switching to an existing one.
//!
//! Both writes run `git switch`, so uncommitted changes are carried as Git
//! carries them and a switch that would overwrite them is refused by Git.
//! Nothing here forces, merges, detaches, deletes, or renames.

use super::{
    BUSY, Result, STATUS_LIMIT, TIMEOUT, base_command, command, run, worktree::branch_exists,
    write::git,
};
use std::{ffi::OsStr, path::Path, time::Duration};

/// More branches than this are not listed; the list says so.
pub const BRANCH_LIMIT: usize = 500;
/// A switch rewrites files, running filters and the post-checkout hook.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(120);

/// One local branch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Branch {
    /// The short name, such as `main` or `feature/login`.
    pub name: String,
    /// HEAD is on this branch.
    pub current: bool,
    /// Committer time of its tip, seconds since the Unix epoch.
    pub time: i64,
    /// Another worktree has this branch checked out; Git refuses to switch to it.
    pub elsewhere: Option<String>,
}

/// Local branches, most recently committed first.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Branches {
    pub list: Vec<Branch>,
    /// More branches exist than `BRANCH_LIMIT`.
    pub more: bool,
}

/// List local branches. An unborn branch has no tip yet, so it is not listed;
/// on a detached HEAD no branch is current. Blocking: call on a background thread.
pub fn branches(root: &Path) -> Result<Branches> {
    let count = format!("--count={}", BRANCH_LIMIT + 1);
    let args = [
        "for-each-ref",
        "--sort=-committerdate",
        &count,
        // Ref names cannot hold NUL or newline, so these delimit fields and records.
        "--format=%(refname)%00%(HEAD)%00%(committerdate:unix)%00%(worktreepath)",
        "refs/heads/",
    ]
    .map(OsStr::new);
    let output = command(root, &args, STATUS_LIMIT)?;
    if output.truncated {
        return Err("The branch list exceeds the safe read limit.".into());
    }
    let mut list = parse_branches(&output.bytes)?;
    let more = list.len() > BRANCH_LIMIT;
    list.truncate(BRANCH_LIMIT);
    Ok(Branches { list, more })
}

/// Records of `refname NUL HEAD NUL time NUL worktreepath`, one per line.
pub(super) fn parse_branches(bytes: &[u8]) -> Result<Vec<Branch>> {
    let malformed = || "Malformed Git branch record".to_owned();
    let mut list = Vec::new();
    for record in bytes.split(|b| *b == b'\n').filter(|r| !r.is_empty()) {
        let fields: Vec<&[u8]> = record.split(|b| *b == 0).collect();
        let [reference, head, time, worktree] = fields[..] else {
            return Err(malformed());
        };
        let name = reference
            .strip_prefix(b"refs/heads/")
            .ok_or_else(malformed)?;
        // A name that is not UTF-8 could not be passed back to Git unchanged.
        let Ok(name) = std::str::from_utf8(name) else {
            continue;
        };
        let current = head == b"*";
        // The current branch's worktree is this one; any other is elsewhere.
        let elsewhere = (!current && !worktree.is_empty())
            .then(|| String::from_utf8_lossy(worktree).into_owned());
        list.push(Branch {
            name: name.to_owned(),
            current,
            time: std::str::from_utf8(time)
                .ok()
                .and_then(|time| time.parse().ok())
                .ok_or_else(malformed)?,
            elsewhere,
        });
    }
    Ok(list)
}

/// `git switch -c <name>`: a new branch at the current commit, checked out.
pub(super) fn create(root: &Path, name: &str) -> Result<String> {
    check_name(root, name)?;
    if branch_exists(root, name)? {
        return Err(format!(
            "A branch named {name} already exists. Choose it in the list to switch to it."
        ));
    }
    refuse_if_locked(root)?;
    // `-c` takes the next argument as its value, even one that looks like an
    // option; `check_name` has already refused a leading dash.
    git(root, &["switch", "-q", "-c", name], None, SWITCH_TIMEOUT)?;
    Ok(format!("Created {name} and switched to it"))
}

/// `git switch <name>` to an existing local branch, never a remote one.
pub(super) fn switch(root: &Path, name: &str) -> Result<String> {
    // Only a local branch: not `-`, `@{-1}`, a reflog entry, a commit, or a remote name.
    if name.is_empty()
        || name.starts_with('-')
        || name.contains("@{")
        || !branch_exists(root, name)?
    {
        return Err(format!("There is no local branch named {name}."));
    }
    refuse_if_locked(root)?;
    git(
        root,
        &["switch", "-q", "--no-guess", "--end-of-options", name],
        None,
        SWITCH_TIMEOUT,
    )?;
    Ok(format!("Switched to {name}"))
}

/// Refuse names Git would read as something else, then ask Git itself.
fn check_name(root: &Path, name: &str) -> Result<()> {
    let invalid = || format!("{name:?} is not a valid branch name.");
    if name.trim().is_empty() {
        return Err("Type a name for the new branch.".into());
    }
    // `@{-1}` and `@` name other commits; a leading dash reads as an option.
    if name.starts_with('-') || name.contains("@{") || name == "@" {
        return Err(invalid());
    }
    let mut check = base_command(root);
    check.args(["check-ref-format", "--branch", name]);
    let raw = run(&mut check, None, STATUS_LIMIT, TIMEOUT)?;
    let normalized = String::from_utf8_lossy(&raw.out.bytes);
    // Git prints the name it would use; anything else was expanded or refused.
    if !raw.status.success() || normalized.trim_end_matches('\n') != name {
        return Err(invalid());
    }
    Ok(())
}

/// A branch write must not move HEAD under another Git process. Creating a
/// branch does not touch the index, so Git itself would not notice the lock.
fn refuse_if_locked(root: &Path) -> Result<()> {
    let mut find = base_command(root);
    find.args([
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "index.lock",
    ]);
    let raw = run(&mut find, None, STATUS_LIMIT, TIMEOUT)?;
    if !raw.status.success() {
        return Err(raw.failure());
    }
    let lock = super::path_from_bytes(raw.out.bytes.strip_suffix(b"\n").unwrap_or(&raw.out.bytes));
    if lock.symlink_metadata().is_ok() {
        return Err(BUSY.into());
    }
    Ok(())
}
