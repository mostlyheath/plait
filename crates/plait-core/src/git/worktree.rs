//! Optional per-thread worktrees: a second checkout of the same repository on
//! its own branch, so a thread can work beside another without sharing files.
//!
//! Plait creates them beside the repository, in `<name>.worktrees/`, and removes
//! them only through `git worktree remove`, which refuses uncommitted work.

pub mod inventory;

use super::{
    Result, head_exists,
    write::{git, git_holding},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Checkout runs the repository's filters and post-checkout hook.
const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(120);
/// Fresh names to try before giving up on collisions.
const ATTEMPTS: u32 = 8;

/// A worktree Plait created: where it is checked out and the branch it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Check out HEAD of the repository at `root` on a new `plait/<name>` branch
/// in a new folder beside it. Blocking: call on a background thread.
pub fn add_worktree(root: &Path) -> Result<Worktree> {
    add_worktree_with_common_wait(root, crate::lease::COMMON_METADATA_WAIT)
}

pub(crate) fn add_worktree_with_common_wait(
    root: &Path,
    common_wait: Duration,
) -> Result<Worktree> {
    // Hold a shared lifecycle lease on the parent checkout (allowing an active
    // writer or reader there to continue) and serialize shared repository
    // metadata (refs and the worktree registry) exclusively on common_dir.
    let lease = crate::lease::acquire_git_checkout(
        root,
        None,
        crate::lease::LeaseClass::Reader,
        Some(common_wait),
    )
    .map_err(String::from)?;
    add_worktree_unlocked(lease.root(), &lease)
}

fn add_worktree_unlocked(root: &Path, lease: &crate::lease::CheckoutLease) -> Result<Worktree> {
    if !head_exists(root)? {
        return Err("Make a first commit before starting a thread in its own worktree.".into());
    }
    let (Some(parent), Some(name)) = (root.parent(), root.file_name()) else {
        return Err("This repository has no folder to put a worktree beside.".into());
    };
    let mut folder = name.to_owned();
    folder.push(".worktrees");
    let base = parent.join(folder);
    let seed = seed();
    for attempt in 0..ATTEMPTS {
        let name = format!("{:06x}", seed.wrapping_add(attempt) & 0xff_ffff);
        let path = base.join(&name);
        let branch = format!("plait/{name}");
        if path.symlink_metadata().is_ok() || branch_exists(root, &branch)? {
            continue;
        }
        let target = path
            .to_str()
            .ok_or("The worktree folder path is not UTF-8.")?;
        // Git checks the new tree out after this process may have died; it holds the lease too.
        git_holding(
            root,
            &["worktree", "add", "-q", "-b", &branch, target, "HEAD"],
            CHECKOUT_TIMEOUT,
            lease,
        )?;
        // The host names checkouts by their resolved path; match it.
        let path = path.canonicalize().map_err(|error| error.to_string())?;
        return Ok(Worktree { path, branch });
    }
    Err("Could not find an unused worktree name. Try again.".into())
}

/// Remove a worktree of the repository at `main`. Git refuses one with
/// modified or untracked files; its branch and commits are kept.
pub fn remove_worktree(main: &Path, path: &Path) -> Result<()> {
    remove_worktree_with_common_wait(main, path, crate::lease::COMMON_METADATA_WAIT)
}

pub(crate) fn remove_worktree_with_common_wait(
    main: &Path,
    path: &Path,
    common_wait: Duration,
) -> Result<()> {
    let main_paths = crate::lease::resolve_git_checkout(main).map_err(String::from)?;
    let target_paths = crate::lease::resolve_git_checkout(path).map_err(String::from)?;
    if main_paths.root == target_paths.root {
        return Err("Cannot remove the main repository checkout as a worktree.".into());
    }
    if main_paths.common_dir != target_paths.common_dir {
        return Err("This folder is not a worktree of the selected repository.".into());
    }
    // Hold exclusive lifecycle + writer leases on the target worktree before
    // taking the shared common-directory metadata lock.
    let lease = crate::lease::acquire_git_checkout(
        &target_paths.root,
        None,
        crate::lease::LeaseClass::Exclusive,
        Some(common_wait),
    )
    .map_err(String::from)?;
    remove_worktree_unlocked(&main_paths.root, &target_paths.root, &lease)
}

fn remove_worktree_unlocked(
    main: &Path,
    path: &Path,
    lease: &crate::lease::CheckoutLease,
) -> Result<()> {
    let target = path
        .to_str()
        .ok_or("The worktree folder path is not UTF-8.")?;
    // Git deletes the tree after this process may have died; it holds the lease too.
    git_holding(
        main,
        &["worktree", "remove", target],
        CHECKOUT_TIMEOUT,
        lease,
    )
    .map_err(|error| {
        if error.contains("modified or untracked files") {
            "This worktree has uncommitted changes. Commit or discard them first.".into()
        } else {
            error
        }
    })?;
    // Leave the shared folder once its last worktree is gone; a non-empty one stays.
    if let Some(parent) = path.parent()
        && parent.extension().is_some_and(|ext| ext == "worktrees")
    {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

pub(super) fn branch_exists(root: &Path, branch: &str) -> Result<bool> {
    let reference = format!("refs/heads/{branch}");
    let mut command = super::read_command(root)?;
    let raw = super::run(
        command.args(["rev-parse", "--verify", "-q", &reference]),
        None,
        super::STATUS_LIMIT,
        super::TIMEOUT,
    )?;
    match raw.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(raw.failure()),
    }
}

/// Short names only need to differ from the few worktrees beside them.
fn seed() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (nanos as u32) ^ (std::process::id() << 12)
}

// Host-managed operations have a private durable journal. Legacy helpers above
// remain for older callers; desktop associations alone cannot use this API.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub dev: String,
    pub ino: String,
}
impl Identity {
    pub fn read(path: &Path) -> Result<Self> {
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_dir() || path.canonicalize().map_err(|e| e.to_string())? != path {
            return Err("Worktree directory identity changed.".into());
        }
        Ok(Self {
            dev: meta.dev().to_string(),
            ino: meta.ino().to_string(),
        })
    }
    pub fn checked(&self, path: &Path) -> Result<()> {
        if Self::read(path)? != *self {
            return Err("Worktree directory identity changed.".into());
        }
        Ok(())
    }
    pub fn lease(&self) -> Result<crate::lease::DirIdentity> {
        Ok(crate::lease::DirIdentity {
            dev: self.dev.parse().map_err(|_| "Invalid device identity.")?,
            ino: self.ino.parse().map_err(|_| "Invalid inode identity.")?,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Parent {
    pub root: PathBuf,
    pub identity: Identity,
    pub main: PathBuf,
    pub main_identity: Identity,
    pub common: PathBuf,
    pub common_identity: Identity,
    pub base: String,
}

/// Capture the explicit base and resolve linked checkouts to their main root.
/// Reads never run checkout hooks, filters, external diff or textconv.
pub fn worktree_parent(root: &Path) -> Result<Parent> {
    if root.to_str().is_none() {
        return Err("The worktree folder path is not UTF-8.".into());
    }
    let paths = crate::lease::resolve_git_checkout(root).map_err(String::from)?;
    let list = read_worktree(&paths.root, &["worktree", "list", "--porcelain", "-z"])?;
    let first = list.split(|b| *b == 0).next().ok_or("No main worktree.")?;
    let main = first
        .strip_prefix(b"worktree ")
        .ok_or("No main worktree.")?;
    let main = PathBuf::from(
        std::str::from_utf8(main).map_err(|_| "The main repository path is not UTF-8.")?,
    );
    let main = main.canonicalize().map_err(|e| e.to_string())?;
    let main_paths = crate::lease::resolve_git_checkout(&main).map_err(String::from)?;
    if main_paths.common_dir != paths.common_dir {
        return Err("Main repository identity changed.".into());
    }
    for path in [&paths.root, &main, &paths.common_dir] {
        if path.to_str().is_none() {
            return Err("The worktree folder path is not UTF-8.".into());
        }
    }
    let base =
        text_read(&paths.root, &["rev-parse", "--verify", "HEAD^{commit}"]).map_err(|_| {
            "Make a first commit before starting a thread in its own worktree.".to_owned()
        })?;
    Ok(Parent {
        identity: Identity::read(&paths.root)?,
        main_identity: Identity::read(&main)?,
        common_identity: Identity::read(&paths.common_dir)?,
        root: paths.root,
        main,
        common: paths.common_dir,
        base,
    })
}

/// Resolve the main checkout without requiring a first commit.
pub fn main_checkout(root: &Path) -> Result<PathBuf> {
    let raw = read_worktree(root, &["worktree", "list", "--porcelain", "-z"])?;
    let first = raw
        .split(|b| *b == 0)
        .next()
        .ok_or("Git returned no checkouts.")?;
    let text = std::str::from_utf8(first).map_err(|_| "Worktree paths must be UTF-8.")?;
    let path = PathBuf::from(
        text.strip_prefix("worktree ")
            .ok_or("Invalid Git worktree list.")?,
    );
    if !path.is_absolute() {
        return Err("Invalid main checkout path.".into());
    }
    Ok(path)
}
fn read_worktree(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = super::read_command(root)?;
    super::run_readonly(command.args(args))
}
fn text_read(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(read_worktree(root, args)?)
        .map(|s| s.trim_end_matches('\n').to_owned())
        .map_err(|_| "Git returned a name that is not UTF-8.".into())
}
fn verify_parent(parent: &Parent) -> Result<()> {
    parent.identity.checked(&parent.root)?;
    parent.main_identity.checked(&parent.main)?;
    parent.common_identity.checked(&parent.common)?;
    let actual = worktree_parent(&parent.root)?;
    if actual.root != parent.root || actual.main != parent.main || actual.common != parent.common {
        return Err("The parent repository does not match its recorded identity.".into());
    }
    if !super::log::is_object_id(&parent.base)
        || text_read(
            &parent.root,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", parent.base),
            ],
        )? != parent.base
    {
        return Err("The selected base commit is unavailable.".into());
    }
    Ok(())
}

/// Removal needs the repository, not the checkout or commit the worktree
/// started from: either may be gone by then, as when it began in a worktree.
fn verify_repository(parent: &Parent) -> Result<()> {
    let changed = || {
        "The repository this worktree belongs to has moved or changed. Nothing was removed."
            .to_owned()
    };
    parent
        .main_identity
        .checked(&parent.main)
        .map_err(|_| changed())?;
    parent
        .common_identity
        .checked(&parent.common)
        .map_err(|_| changed())?;
    let paths = crate::lease::resolve_git_checkout(&parent.main).map_err(|_| changed())?;
    if paths.root != parent.main || paths.common_dir != parent.common {
        return Err(changed());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedStage {
    Reserved,
    Creating,
    Ready,
    Failed,
    Removing,
    Removed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedWorktree {
    pub thread_id: String,
    pub parent: Parent,
    pub path: PathBuf,
    pub branch: String,
    pub identity: Option<Identity>,
    pub stage: ManagedStage,
    pub error: Option<String>,
}

fn uuid_hex(id: &str) -> Result<String> {
    let bytes = id.as_bytes();
    if bytes.len() != 36
        || !bytes.iter().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
    {
        return Err("Invalid worktree thread UUID.".into());
    }
    Ok(id.replace('-', ""))
}
fn worktree_base(main: &Path) -> Result<PathBuf> {
    let mut name = main
        .file_name()
        .ok_or("The main repository has no folder name.")?
        .to_owned();
    name.push(".worktrees");
    Ok(main
        .parent()
        .ok_or("The main repository has no parent folder.")?
        .join(name))
}
fn owned_base(main: &Path) -> Result<PathBuf> {
    let base = worktree_base(main)?;
    // An explicit mode, so a group-writable umask in the calling process
    // (common on Linux) cannot produce a folder the check below refuses.
    // The host's helper already runs under 022; tests and other callers may not.
    match fs::DirBuilder::new().mode(0o755).create(&base) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.to_string()),
    }
    Identity::read(&base)?;
    let meta = fs::symlink_metadata(&base).map_err(|e| e.to_string())?;
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
        return Err(
            "The worktree parent folder must be owned and not writable by other users.".into(),
        );
    }
    Ok(base)
}

/// Only a path and branch derived from the thread's UUID, in the main
/// repository's worktree folder, can be one Plait made.
fn owned_shape(record: &ManagedWorktree, base: &Path, hex: &str) -> bool {
    let suffix = record.branch.strip_prefix("plait/");
    record.path.parent() == Some(base)
        && suffix.is_some_and(|suffix| suffix.len() >= 12 && hex.starts_with(suffix))
        && record.path.file_name().and_then(|s| s.to_str()) == suffix
}

/// `save` must commit and sync the journal before returning. A retry uses that
/// same journal, never an inferred association or an existing unrecognized path.
pub fn create_managed_worktree(
    parent: &Parent,
    thread_id: &str,
    previous: Option<ManagedWorktree>,
    mut save: impl FnMut(&ManagedWorktree) -> Result<()>,
) -> Result<ManagedWorktree> {
    create_managed_with_timeout(parent, thread_id, previous, &mut save, CHECKOUT_TIMEOUT)
}
/// The host lends the supplied leases to its supervised mutation and reclaims
/// descendants before returning, so both normal completion and death stay excluded.
pub type Mutation<'a> =
    dyn FnMut(&Path, &[&str], Duration, &[&crate::lease::CheckoutLease]) -> Result<()> + 'a;

pub fn create_managed_supervised(
    parent: &Parent,
    thread_id: &str,
    previous: Option<ManagedWorktree>,
    mut save: impl FnMut(&ManagedWorktree) -> Result<()>,
    mutate: &mut Mutation<'_>,
) -> Result<ManagedWorktree> {
    create_managed_inner(
        parent,
        thread_id,
        previous,
        &mut save,
        CHECKOUT_TIMEOUT,
        mutate,
    )
}
fn create_managed_with_timeout(
    parent: &Parent,
    thread_id: &str,
    previous: Option<ManagedWorktree>,
    save: &mut impl FnMut(&ManagedWorktree) -> Result<()>,
    timeout: Duration,
) -> Result<ManagedWorktree> {
    create_managed_inner(
        parent,
        thread_id,
        previous,
        save,
        timeout,
        &mut |root, args, timeout, leases| git_holding(root, args, timeout, leases[1]),
    )
}
fn create_managed_inner(
    parent: &Parent,
    thread_id: &str,
    previous: Option<ManagedWorktree>,
    save: &mut impl FnMut(&ManagedWorktree) -> Result<()>,
    timeout: Duration,
    mutate: &mut Mutation<'_>,
) -> Result<ManagedWorktree> {
    let hex = uuid_hex(thread_id)?;
    let parent_lease = crate::lease::acquire_git_checkout(
        &parent.root,
        Some(parent.identity.lease()?),
        crate::lease::LeaseClass::Reader,
        None,
    )
    .map_err(String::from)?;
    verify_parent(parent)?;
    let common =
        crate::lease::acquire_common_metadata(&parent.common, crate::lease::COMMON_METADATA_WAIT)
            .map_err(String::from)?;
    let base = owned_base(&parent.main)?;
    let mut record = match previous {
        Some(record) => {
            if record.thread_id != thread_id
                || &record.parent != parent
                || !owned_shape(&record, &base, &hex)
            {
                return Err(
                    "Worktree journal does not match the requested parent and UUID.".into(),
                );
            }
            record
        }
        None => {
            let suffix = (12..=32)
                .step_by(4)
                .map(|n| &hex[..n])
                .find_map(|suffix| {
                    let path = base.join(suffix);
                    match (
                        path.try_exists(),
                        branch_exists(&parent.root, &format!("plait/{suffix}")),
                    ) {
                        (Ok(false), Ok(false)) if fs::symlink_metadata(&path).is_err() => {
                            Some(Ok(suffix))
                        }
                        (Err(e), _) => Some(Err(e.to_string())),
                        (_, Err(e)) => Some(Err(e)),
                        _ => None,
                    }
                })
                .ok_or("All UUID-derived worktree names already exist.")??;
            let record = ManagedWorktree {
                thread_id: thread_id.into(),
                parent: parent.clone(),
                path: base.join(suffix),
                branch: format!("plait/{suffix}"),
                identity: None,
                stage: ManagedStage::Reserved,
                error: None,
            };
            save(&record)?;
            record
        }
    };
    if matches!(
        record.stage,
        ManagedStage::Removed | ManagedStage::Removing | ManagedStage::Failed
    ) {
        return Ok(record);
    }
    if record.stage == ManagedStage::Reserved && record.identity.is_none() {
        if branch_exists(&parent.root, &record.branch)? {
            return Err("The reserved branch already exists; it was left untouched.".into());
        }
        // Never take ownership of an existing directory, even when empty.
        fs::create_dir(&record.path)
            .map_err(|e| format!("Cannot reserve the worktree folder: {e}"))?;
        record.identity = Some(Identity::read(&record.path)?);
        save(&record)?;
    }
    // Release metadata before taking the new root's lower-level locks. The
    // persisted reservation keeps the selected name during this brief gap.
    drop(common);
    let target_lease = crate::lease::acquire_checkout(
        &record.path,
        Some(
            record
                .identity
                .as_ref()
                .ok_or("No reserved worktree identity.")?
                .lease()?,
        ),
        crate::lease::LeaseClass::Exclusive,
    )
    .map_err(String::from)?
    .with_common_metadata(&parent.common, crate::lease::COMMON_METADATA_WAIT)
    .map_err(String::from)?;
    verify_parent(parent)?;
    match record.stage {
        ManagedStage::Ready => {
            verify_managed(&record)?;
            return Ok(record);
        }
        ManagedStage::Removed | ManagedStage::Removing | ManagedStage::Failed => return Ok(record),
        ManagedStage::Creating => {
            // Git may have finished before its reply was lost. Never rerun hooks.
            record.stage = if verify_managed(&record).is_ok()
                && text_read(&record.path, &["rev-parse", "--verify", "HEAD"])
                    .ok()
                    .as_ref()
                    == Some(&parent.base)
                && read_worktree(
                    &record.path,
                    &[
                        "status",
                        "--porcelain=v1",
                        "-z",
                        "--untracked-files=all",
                        "--ignored=matching",
                    ],
                )
                .is_ok_and(|status| status.is_empty())
            {
                ManagedStage::Ready
            } else {
                ManagedStage::Failed
            };
            record.error = (record.stage == ManagedStage::Failed).then(|| "Worktree creation was interrupted. Existing files and branch were preserved; inspect them before making a new request.".into());
            save(&record)?;
            return Ok(record);
        }
        ManagedStage::Reserved => (),
    }
    if branch_exists(&parent.root, &record.branch)? {
        return Err("The reserved branch already exists; it was left untouched.".into());
    }
    if fs::read_dir(&record.path)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err(
            "The reserved worktree folder is no longer empty. Existing files were preserved."
                .into(),
        );
    }
    record.stage = ManagedStage::Creating;
    save(&record)?;
    let target = record
        .path
        .to_str()
        .ok_or("The worktree folder path is not UTF-8.")?;
    let result = mutate(
        &parent.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &record.branch,
            target,
            &parent.base,
        ],
        timeout,
        &[&parent_lease, &target_lease],
    );
    match result.and_then(|()| verify_managed(&record)) {
        Ok(()) => record.stage = ManagedStage::Ready,
        Err(error) => {
            record.stage = ManagedStage::Failed;
            record.error = Some(failed_creation(&record, &error));
        }
    }
    save(&record)?;
    Ok(record)
}
/// Git keeps a worktree it made when a later hook or filter fails, so say
/// that it exists: "not created" would invite a second one.
fn failed_creation(record: &ManagedWorktree, error: &str) -> String {
    if verify_managed(record).is_ok() {
        format!(
            "Git made the worktree at {} on branch {}, but a checkout hook or filter then failed or ran too long, so no thread was started. The worktree and branch were kept; remove them with Git if you do not need them. Git said: {error}",
            record.path.display(),
            record.branch
        )
    } else {
        format!(
            "Git could not create the worktree. Any files and branch it made were kept. Git said: {error}"
        )
    }
}
fn verify_managed(record: &ManagedWorktree) -> Result<()> {
    record
        .identity
        .as_ref()
        .ok_or("Worktree has no recorded directory identity.")?
        .checked(&record.path)?;
    let paths = crate::lease::resolve_git_checkout(&record.path).map_err(String::from)?;
    if paths.root != record.path || paths.common_dir != record.parent.common {
        return Err("Worktree is not attached to its recorded parent repository.".into());
    }
    record.parent.common_identity.checked(&paths.common_dir)?;
    if text_read(&record.path, &["symbolic-ref", "--quiet", "HEAD"])?
        != format!("refs/heads/{}", record.branch)
    {
        return Err("Worktree branch changed; removal is refused.".into());
    }
    Ok(())
}

/// Removal keeps the branch and requires an explicit publication exception when
/// the current commit is not reachable from the branch's known upstream.
pub fn remove_managed_worktree(
    record: ManagedWorktree,
    expected: &Identity,
    allow_unpushed: bool,
    save: impl FnMut(&ManagedWorktree) -> Result<()>,
) -> Result<ManagedWorktree> {
    remove_managed_supervised(
        record,
        expected,
        allow_unpushed,
        save,
        &mut |root, args, timeout, leases| git_holding(root, args, timeout, leases[0]),
    )
}
/// Holds checkout exclusion until the supervisor has reclaimed Git descendants.
pub fn remove_managed_supervised(
    record: ManagedWorktree,
    expected: &Identity,
    allow_unpushed: bool,
    save: impl FnMut(&ManagedWorktree) -> Result<()>,
    mutate: &mut Mutation<'_>,
) -> Result<ManagedWorktree> {
    remove_managed_with_output(record, expected, allow_unpushed, save, mutate, |_| {
        Ok(Vec::new())
    })
}
pub fn remove_managed_with_output(
    mut record: ManagedWorktree,
    expected: &Identity,
    allow_unpushed: bool,
    mut save: impl FnMut(&ManagedWorktree) -> Result<()>,
    mutate: &mut Mutation<'_>,
    policy: impl FnOnce(&Parent) -> Result<Vec<String>>,
) -> Result<ManagedWorktree> {
    if record.identity.as_ref() != Some(expected) {
        return Err("Removal identity does not match the host-owned worktree.".into());
    }
    // Recheck what creation checked: a journal naming any other folder or
    // branch was not written by a creation and grants no deletion.
    if !owned_shape(
        &record,
        &worktree_base(&record.parent.main)?,
        &uuid_hex(&record.thread_id)?,
    ) {
        return Err(
            "This record does not name a worktree Plait created. Nothing was removed.".into(),
        );
    }
    if record.stage == ManagedStage::Removed {
        return Ok(record);
    }
    verify_repository(&record.parent)?;
    if record.stage == ManagedStage::Removing && fs::symlink_metadata(&record.path).is_err() {
        let _lease = crate::lease::acquire_git_checkout(
            &record.parent.main,
            Some(record.parent.main_identity.lease()?),
            crate::lease::LeaseClass::Reader,
            Some(crate::lease::COMMON_METADATA_WAIT),
        )
        .map_err(String::from)?;
        // A missing path alone may be an unavailable volume, not successful removal.
        let list = read_worktree(
            &record.parent.main,
            &["worktree", "list", "--porcelain", "-z"],
        )?;
        let entry = format!("worktree {}", record.path.display());
        if list.split(|b| *b == 0).any(|part| part == entry.as_bytes()) {
            // Git may have deleted the folder before it stopped; say only what is known.
            return Err(format!(
                "Removal was interrupted: the worktree folder {} is gone, but Git still lists the worktree. Its branch {} was kept. Check `git worktree list` before trying again.",
                record.path.display(),
                record.branch
            ));
        }
        record.stage = ManagedStage::Removed;
        save(&record)?;
        return Ok(record);
    }
    if fs::symlink_metadata(&record.path).is_err() {
        return Err(format!(
            "The worktree folder {} is missing. Nothing was removed. If you moved it, move it back; if its disk is not connected, connect it.",
            record.path.display()
        ));
    }
    let lease = crate::lease::acquire_git_checkout(
        &record.path,
        Some(expected.lease()?),
        crate::lease::LeaseClass::Exclusive,
        Some(crate::lease::COMMON_METADATA_WAIT),
    )
    .map_err(String::from)?;
    verify_managed(&record)?;
    verify_repository(&record.parent)?;
    let approved = policy(&record.parent)?;
    inventory::preservation_with_output(
        &record.path,
        &record.parent.main,
        allow_unpushed,
        &approved,
    )?
    .require_safe()?;
    record.stage = ManagedStage::Removing;
    save(&record)?;
    mutate(
        &record.parent.main,
        &[
            "worktree",
            "remove",
            record.path.to_str().ok_or("Worktree path is not UTF-8.")?,
        ],
        CHECKOUT_TIMEOUT,
        &[&lease],
    )?;
    record.stage = ManagedStage::Removed;
    record.error = None;
    save(&record)?;
    Ok(record)
}

/// Status cannot see an edit behind an assume-unchanged or skip-worktree bit,
/// but `git worktree remove` deletes it with the folder.
fn refuse_hidden_edits(path: &Path) -> Result<()> {
    let listing = read_worktree(path, &["ls-files", "-v", "-z"])?;
    for entry in listing.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        let [tag, b' ', name @ ..] = entry else {
            return Err("Git listed an index entry Plait cannot read. Nothing was removed.".into());
        };
        // A sparse checkout's skipped files are absent; a present one may be edited.
        let present = || {
            use std::os::unix::ffi::OsStrExt;
            fs::symlink_metadata(path.join(std::ffi::OsStr::from_bytes(name))).is_ok()
        };
        if tag.is_ascii_lowercase() || (*tag == b'S' && present()) {
            return Err("This worktree has tracked files Git is told not to check (assume-unchanged or skip-worktree), so their edits cannot be seen. Clear those flags and commit or discard the edits before removing the worktree.".into());
        }
    }
    Ok(())
}

/// Commits kept only by this worktree's HEAD reflog or its own refs lose their
/// last reference when Git deletes the worktree's administrative folder.
fn worktree_only_commits(path: &Path, main: &Path) -> Result<u64> {
    let mut tips = read_worktree(path, &["rev-list", "--walk-reflogs", "HEAD"])?;
    tips.extend(read_worktree(
        path,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/worktree/",
            "refs/bisect/",
            "refs/rewritten/",
        ],
    )?);
    if tips.iter().all(u8::is_ascii_whitespace) {
        return Ok(0);
    }
    // From the main checkout, `--single-worktree` leaves out this worktree's
    // HEAD, reflog and own refs: what remains survives the removal. `--stdin`
    // comes before `--not` so every Git version reads the tips as positive.
    let mut command = super::read_command(main)?;
    let raw = super::run(
        command.args([
            "rev-list",
            "--count",
            "--single-worktree",
            "--stdin",
            "--not",
            "--all",
            "--reflog",
        ]),
        Some(tips),
        super::STATUS_LIMIT,
        super::TIMEOUT,
    )?;
    if !raw.status.success() {
        return Err(raw.failure());
    }
    String::from_utf8_lossy(&raw.out.bytes)
        .trim()
        .parse()
        .map_err(|_| "Invalid commit count.".into())
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperRequest {
    Inventory {
        root: PathBuf,
        #[serde(default)]
        store: Option<PathBuf>,
        records: Vec<ManagedWorktree>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<PathBuf>,
    },
    Parent {
        root: PathBuf,
    },
    Create {
        store: PathBuf,
        thread_id: String,
        parent: Parent,
    },
    Remove {
        store: PathBuf,
        thread_id: String,
        identity: Identity,
        allow_unpushed: bool,
    },
    Read {
        store: PathBuf,
        thread_id: String,
    },
}

/// Internal supervised Git worker; callers must hold the managed operation leases.
pub fn managed_git(root: &Path, args: &[&str]) -> Result<()> {
    if args.first() != Some(&"worktree") || !matches!(args.get(1), Some(&"add" | &"remove")) {
        return Err("Unsupported worktree Git operation.".into());
    }
    git(root, args, None, CHECKOUT_TIMEOUT)
}

#[cfg(test)]
mod managed_tests {
    use super::*;
    use crate::git::tests::Repo;
    const ID: &str = "12345678-1234-4abc-8abc-123456789abc";
    fn repository() -> Repo {
        let mut repo = Repo::new();
        repo.0 = repo.0.canonicalize().unwrap();
        fs::write(repo.0.join("file"), "committed\n").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }
    fn create(repo: &Repo) -> ManagedWorktree {
        create_managed_worktree(&worktree_parent(&repo.0).unwrap(), ID, None, |_| Ok(())).unwrap()
    }
    fn clean(tree: &ManagedWorktree) {
        if tree.path.exists() {
            let _ = remove_managed_worktree(
                tree.clone(),
                tree.identity.as_ref().unwrap(),
                true,
                |_| Ok(()),
            );
        }
        if let Ok(base) = worktree_base(&tree.parent.main) {
            let _ = fs::remove_dir_all(base);
        }
    }
    #[test]
    fn unborn_repository_and_forged_parent_have_no_effects() {
        let empty = Repo::new();
        assert!(
            worktree_parent(&empty.0)
                .unwrap_err()
                .contains("first commit")
        );
        let repo = repository();
        let mut parent = worktree_parent(&repo.0).unwrap();
        parent.common_identity.ino = "0".into();
        assert!(create_managed_worktree(&parent, ID, None, |_| Ok(())).is_err());
        assert!(!worktree_base(&repo.0).unwrap().exists());
    }
    #[test]
    fn uuid_collisions_extend_without_reusing_existing_branch_or_directory() {
        let repo = repository();
        let base = owned_base(&repo.0).unwrap();
        fs::create_dir(base.join("123456781234")).unwrap();
        repo.git(&["branch", "plait/1234567812344abc"]);
        let tree = create(&repo);
        assert_eq!(tree.branch, "plait/1234567812344abc8abc");
        assert!(base.join("123456781234").is_dir());
        clean(&tree);
    }
    #[test]
    fn linked_worktree_resolves_main_and_selected_base_ignores_dirty_files() {
        let repo = repository();
        fs::write(repo.0.join("file"), "dirty\n").unwrap();
        let tree = create(&repo);
        assert_eq!(
            fs::read_to_string(tree.path.join("file")).unwrap(),
            "committed\n"
        );
        let parent = worktree_parent(&tree.path).unwrap();
        assert_eq!(parent.main, repo.0);
        let second = create_managed_worktree(
            &parent,
            "22345678-1234-4abc-8abc-123456789abc",
            None,
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(second.path.parent(), tree.path.parent());
        clean(&second);
        clean(&tree);
    }
    #[test]
    fn selected_base_does_not_follow_a_moved_parent_head() {
        let repo = repository();
        let parent = worktree_parent(&repo.0).unwrap();
        fs::write(repo.0.join("file"), "new parent commit\n").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "advance parent"]);
        assert_ne!(worktree_parent(&repo.0).unwrap().base, parent.base);
        let tree = create_managed_worktree(&parent, ID, None, |_| Ok(())).unwrap();
        assert_eq!(
            text_read(&tree.path, &["rev-parse", "HEAD"]).unwrap(),
            parent.base
        );
        assert_eq!(
            fs::read_to_string(tree.path.join("file")).unwrap(),
            "committed\n"
        );
        clean(&tree);
    }
    #[test]
    fn spaces_work_and_non_utf8_main_paths_fail_explicitly() {
        use std::os::unix::ffi::OsStringExt;
        let mut repo = repository();
        let moved = repo.0.with_extension("with spaces");
        fs::rename(&repo.0, &moved).unwrap();
        repo.0 = moved;
        let tree = create(&repo);
        assert_eq!(tree.stage, ManagedStage::Ready);
        clean(&tree);
        let moved = repo
            .0
            .with_file_name(std::ffi::OsString::from_vec(vec![b'p', b'l', 0xff]));
        assert!(worktree_parent(&moved).unwrap_err().contains("UTF-8"));
    }
    #[test]
    fn restart_after_lost_reply_reuses_inode_branch_and_uuid() {
        let repo = repository();
        let parent = worktree_parent(&repo.0).unwrap();
        let mut journal = None;
        let tree = create_managed_worktree(&parent, ID, None, |record| {
            journal = Some(record.clone());
            Ok(())
        })
        .unwrap();
        let retry = create_managed_worktree(&parent, ID, journal.clone(), |_| Ok(())).unwrap();
        assert_eq!(retry, tree);
        let mut interrupted = tree.clone();
        interrupted.stage = ManagedStage::Creating;
        let recovered =
            create_managed_worktree(&parent, ID, Some(interrupted), |_| Ok(())).unwrap();
        assert_eq!(recovered.stage, ManagedStage::Ready);
        assert_eq!(recovered.identity, tree.identity);
        clean(&tree);
    }
    #[test]
    fn partial_creation_and_missing_filesystem_preserve_unknown_work() {
        let repo = repository();
        let parent = worktree_parent(&repo.0).unwrap();
        let mut journal = None;
        let error = create_managed_worktree(&parent, ID, None, |record| {
            journal = Some(record.clone());
            if record.stage == ManagedStage::Creating {
                Err("simulated crash".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error, "simulated crash");
        let saved = journal.unwrap();
        fs::write(saved.path.join("keep"), "user work").unwrap();
        let recovered =
            create_managed_worktree(&parent, ID, Some(saved.clone()), |_| Ok(())).unwrap();
        assert_eq!(recovered.stage, ManagedStage::Failed);
        assert_eq!(
            fs::read_to_string(saved.path.join("keep")).unwrap(),
            "user work"
        );
        fs::remove_dir_all(worktree_base(&repo.0).unwrap()).unwrap();
        let tree = create(&repo);
        fs::remove_dir_all(&tree.path).unwrap();
        let error =
            remove_managed_worktree(tree.clone(), tree.identity.as_ref().unwrap(), true, |_| {
                Ok(())
            })
            .unwrap_err();
        assert!(error.contains("is missing. Nothing was removed"), "{error}");
        // An interrupted removal never claims that nothing was removed.
        let mut interrupted = tree.clone();
        interrupted.stage = ManagedStage::Removing;
        let error =
            remove_managed_worktree(interrupted, tree.identity.as_ref().unwrap(), true, |_| {
                Ok(())
            })
            .unwrap_err();
        assert!(
            error.contains("Removal was interrupted") && !error.contains("Nothing was removed"),
            "{error}"
        );
    }
    #[test]
    fn hooks_and_filters_fail_or_timeout_without_replaying_effects() {
        use std::os::unix::fs::PermissionsExt;
        for (name, script) in [
            ("failure", "#!/bin/sh\nexit 1\n"),
            ("timeout", "#!/bin/sh\nsleep 5\n"),
        ] {
            let repo = repository();
            let hook = repo.0.join(".git/hooks/post-checkout");
            fs::write(&hook, script).unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let parent = worktree_parent(&repo.0).unwrap();
            let tree = create_managed_with_timeout(
                &parent,
                ID,
                None,
                &mut |_| Ok(()),
                Duration::from_millis(250),
            )
            .unwrap();
            assert_eq!(tree.stage, ManagedStage::Failed, "{name}");
            // Git kept the finished checkout; the person is told where it is.
            let error = tree.error.as_deref().unwrap();
            assert!(tree.path.join("file").is_file(), "{name}");
            assert!(
                error.contains(&tree.path.display().to_string())
                    && error.contains(&tree.branch)
                    && error.contains("were kept"),
                "{name}: {error}"
            );
            let retry =
                create_managed_worktree(&parent, ID, Some(tree.clone()), |_| Ok(())).unwrap();
            assert_eq!(retry, tree);
            clean(&tree);
        }
        for filter in ["false", "sleep 5"] {
            let repo = repository();
            fs::write(repo.0.join(".gitattributes"), "file filter=blocked\n").unwrap();
            repo.git(&["add", ".gitattributes"]);
            repo.git(&["commit", "-qm", "attributes"]);
            repo.git(&["config", "filter.blocked.smudge", filter]);
            repo.git(&["config", "filter.blocked.required", "true"]);
            let parent = worktree_parent(&repo.0).unwrap();
            let tree = create_managed_with_timeout(
                &parent,
                ID,
                None,
                &mut |_| Ok(()),
                Duration::from_millis(250),
            )
            .unwrap();
            assert_eq!(tree.stage, ManagedStage::Failed);
            clean(&tree);
        }
    }
    #[test]
    fn removal_refuses_dirty_ignored_unpushed_and_stale_busy_summary() {
        let repo = repository();
        let tree = create(&repo);
        let identity = tree.identity.as_ref().unwrap();
        assert!(
            remove_managed_worktree(tree.clone(), identity, false, |_| Ok(()))
                .unwrap_err()
                .contains("publication")
        );
        fs::write(tree.path.join("file"), "changed\n").unwrap();
        assert!(
            remove_managed_worktree(tree.clone(), identity, true, |_| Ok(()))
                .unwrap_err()
                .contains("uncommitted")
        );
        fs::write(tree.path.join("file"), "committed\n").unwrap();
        fs::write(tree.path.join("unknown"), "preserve\n").unwrap();
        assert!(remove_managed_worktree(tree.clone(), identity, true, |_| Ok(())).is_err());
        fs::remove_file(tree.path.join("unknown")).unwrap();
        let writer =
            crate::lease::acquire_checkout(&tree.path, None, crate::lease::LeaseClass::Writer)
                .unwrap();
        assert!(
            remove_managed_worktree(tree.clone(), identity, true, |_| Ok(()))
                .unwrap_err()
                .contains("in use")
        );
        drop(writer);
        let removed = remove_managed_worktree(tree.clone(), identity, true, |_| Ok(())).unwrap();
        assert_eq!(removed.stage, ManagedStage::Removed);
        assert!(branch_exists(&repo.0, &tree.branch).unwrap());
        assert_eq!(
            remove_managed_worktree(removed.clone(), identity, true, |_| Ok(())).unwrap(),
            removed
        );
    }
    #[test]
    fn publication_checks_refuse_ahead_commits_ignored_files_and_unfinished_git() {
        let repo = repository();
        fs::write(repo.0.join(".gitignore"), "target/\n").unwrap();
        repo.git(&["add", ".gitignore"]);
        repo.git(&["commit", "-qm", "ignore cache"]);
        let remote = repo.0.join(".git/test-remote.git");
        repo.git(&["init", "--bare", remote.to_str().unwrap()]);
        repo.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
        let tree = create(&repo);
        let identity = tree.identity.as_ref().unwrap();
        let command = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&tree.path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        command(&["push", "-u", "origin", &tree.branch]);
        fs::create_dir(tree.path.join("target")).unwrap();
        fs::write(tree.path.join("target/cache"), "ignored").unwrap();
        assert!(
            remove_managed_worktree(tree.clone(), identity, false, |_| Ok(()))
                .unwrap_err()
                .contains("ignored")
        );
        fs::remove_dir_all(tree.path.join("target")).unwrap();
        let git_dir = text_read(&tree.path, &["rev-parse", "--absolute-git-dir"]).unwrap();
        fs::write(Path::new(&git_dir).join("MERGE_HEAD"), &tree.parent.base).unwrap();
        assert!(
            remove_managed_worktree(tree.clone(), identity, false, |_| Ok(()))
                .unwrap_err()
                .contains("unfinished")
        );
        fs::remove_file(Path::new(&git_dir).join("MERGE_HEAD")).unwrap();
        fs::write(tree.path.join("file"), "new commit\n").unwrap();
        command(&["add", "file"]);
        command(&["commit", "-qm", "unpushed"]);
        assert!(
            remove_managed_worktree(tree.clone(), identity, false, |_| Ok(()))
                .unwrap_err()
                .contains("unpushed commits")
        );
        command(&["push"]);
        let removed = remove_managed_worktree(tree.clone(), identity, false, |_| Ok(())).unwrap();
        assert_eq!(removed.stage, ManagedStage::Removed);
        let mut lost = removed;
        lost.stage = ManagedStage::Removing;
        assert_eq!(
            remove_managed_worktree(lost, identity, false, |_| Ok(()))
                .unwrap()
                .stage,
            ManagedStage::Removed
        );
        clean(&tree);
    }
    #[test]
    fn reserved_identity_resumes_and_new_checkout_is_excluded_during_git() {
        let repo = repository();
        let parent = worktree_parent(&repo.0).unwrap();
        let mut journal = None;
        let stopped = create_managed_worktree(&parent, ID, None, |record| {
            journal = Some(record.clone());
            if record.stage == ManagedStage::Reserved && record.identity.is_some() {
                Err("crash after reservation".into())
            } else {
                Ok(())
            }
        });
        assert!(stopped.is_err());
        let saved = journal.unwrap();
        let tree = create_managed_supervised(
            &parent,
            ID,
            Some(saved),
            |_| Ok(()),
            &mut |root, args, timeout, leases| {
                let target = Path::new(args[5]);
                assert!(
                    crate::lease::acquire_checkout(target, None, crate::lease::LeaseClass::Writer)
                        .is_err()
                );
                assert!(
                    crate::lease::acquire_common_metadata(&parent.common, Duration::ZERO).is_err()
                );
                git_holding(root, args, timeout, leases[1])
            },
        )
        .unwrap();
        assert_eq!(tree.stage, ManagedStage::Ready);
        clean(&tree);
    }
    #[test]
    fn busy_parent_can_create_but_a_symlinked_destination_is_refused() {
        let repo = repository();
        let parent = worktree_parent(&repo.0).unwrap();
        let writer =
            crate::lease::acquire_checkout(&repo.0, None, crate::lease::LeaseClass::Writer)
                .unwrap();
        let tree = create_managed_worktree(&parent, ID, None, |_| Ok(())).unwrap();
        assert_eq!(tree.stage, ManagedStage::Ready);
        drop(writer);
        clean(&tree);
        let base = worktree_base(&repo.0).unwrap();
        let _ = fs::remove_dir(&base);
        std::os::unix::fs::symlink(&repo.0, &base).unwrap();
        assert!(create_managed_worktree(&parent, ID, None, |_| Ok(())).is_err());
        fs::remove_file(base).unwrap();
    }
    #[test]
    fn removal_refuses_edits_hidden_by_index_bits() {
        let repo = repository();
        let tree = create(&repo);
        let identity = tree.identity.as_ref().unwrap();
        for flag in ["--assume-unchanged", "--skip-worktree"] {
            git_in(&tree.path, &["update-index", flag, "file"]);
            fs::write(tree.path.join("file"), "only copy\n").unwrap();
            let error =
                remove_managed_worktree(tree.clone(), identity, true, |_| Ok(())).unwrap_err();
            assert!(error.contains("not to check"), "{flag}: {error}");
            assert_eq!(
                fs::read_to_string(tree.path.join("file")).unwrap(),
                "only copy\n"
            );
            let clear = flag.replace("--", "--no-");
            git_in(&tree.path, &["update-index", &clear, "file"]);
            git_in(&tree.path, &["checkout", "--", "file"]);
        }
        // A sparse-style skipped file that is absent holds no edit.
        git_in(&tree.path, &["update-index", "--skip-worktree", "file"]);
        fs::remove_file(tree.path.join("file")).unwrap();
        let removed = remove_managed_worktree(tree.clone(), identity, true, |_| Ok(())).unwrap();
        assert_eq!(removed.stage, ManagedStage::Removed);
        clean(&tree);
    }
    #[test]
    fn removal_refuses_commits_only_the_worktree_keeps() {
        let repo = repository();
        let tree = create(&repo);
        let identity = tree.identity.as_ref().unwrap();
        let commit = |name: &str| {
            fs::write(tree.path.join(name), "work\n").unwrap();
            git_in(&tree.path, &["add", name]);
            git_in(
                &tree.path,
                &[
                    "-c",
                    "user.name=T",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-qm",
                    name,
                ],
            );
        };
        // A commit made on a detached HEAD lives only in this worktree's HEAD reflog.
        git_in(&tree.path, &["checkout", "-q", "--detach"]);
        commit("detached");
        git_in(&tree.path, &["checkout", "-q", &tree.branch]);
        for allow in [false, true] {
            let error =
                remove_managed_worktree(tree.clone(), identity, allow, |_| Ok(())).unwrap_err();
            assert!(error.contains("on no branch"), "{error}");
        }
        git_in(&tree.path, &["branch", "kept-detached", "HEAD@{1}"]);
        // A per-worktree ref is deleted with the worktree too.
        commit("worktree-ref");
        git_in(&tree.path, &["update-ref", "refs/worktree/keep", "HEAD"]);
        git_in(&tree.path, &["reset", "-q", "--hard", "HEAD~1"]);
        git_in(&tree.path, &["reflog", "expire", "--expire=now", "--all"]);
        let error = remove_managed_worktree(tree.clone(), identity, true, |_| Ok(())).unwrap_err();
        assert!(error.contains("on no branch"), "{error}");
        git_in(&tree.path, &["branch", "kept-ref", "refs/worktree/keep"]);
        let removed = remove_managed_worktree(tree.clone(), identity, true, |_| Ok(())).unwrap();
        assert_eq!(removed.stage, ManagedStage::Removed);
        clean(&tree);
    }
    #[test]
    fn removal_refuses_a_record_naming_another_worktree() {
        let repo = repository();
        let tree = create(&repo);
        let base = worktree_base(&repo.0).unwrap();
        // A worktree the person made with Git, beside Plait's own.
        let mine = base.join("mine");
        repo.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "mine",
            mine.to_str().unwrap(),
        ]);
        let mine = mine.canonicalize().unwrap();
        let identity = Identity::read(&mine).unwrap();
        for (path, branch) in [
            (mine.clone(), "mine".to_owned()),
            (mine.clone(), "plait/mine".to_owned()),
        ] {
            git_in(&mine, &["checkout", "-q", "-B", &branch]);
            let mut forged = tree.clone();
            forged.path = path;
            forged.branch = branch.clone();
            forged.identity = Some(identity.clone());
            let error = remove_managed_worktree(forged, &identity, true, |_| Ok(())).unwrap_err();
            assert!(error.contains("Plait created"), "{branch}: {error}");
            assert!(mine.join("file").exists());
        }
        repo.git(&["worktree", "remove", mine.to_str().unwrap()]);
        clean(&tree);
    }
    #[test]
    fn a_worktree_started_from_a_removed_worktree_can_be_removed() {
        let repo = repository();
        let first = create(&repo);
        let second_id = "abcdef01-1234-4abc-8abc-123456789abc";
        let second = create_managed_worktree(
            &worktree_parent(&first.path).unwrap(),
            second_id,
            None,
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(second.parent.main, repo.0);
        for tree in [&first, &second] {
            let removed = remove_managed_worktree(
                tree.clone(),
                tree.identity.as_ref().unwrap(),
                true,
                |_| Ok(()),
            )
            .unwrap();
            assert_eq!(removed.stage, ManagedStage::Removed);
        }
        clean(&first);
    }
    fn git_in(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    fn substituted_path_and_forged_removal_identity_are_refused() {
        let repo = repository();
        let tree = create(&repo);
        let original = tree.identity.clone().unwrap();
        let forged = Identity {
            dev: original.dev.clone(),
            ino: "0".into(),
        };
        assert!(remove_managed_worktree(tree.clone(), &forged, true, |_| Ok(())).is_err());
        let saved = tree.path.with_extension("saved");
        fs::rename(&tree.path, &saved).unwrap();
        fs::create_dir(&tree.path).unwrap();
        fs::write(tree.path.join("keep"), "unrelated").unwrap();
        assert!(remove_managed_worktree(tree.clone(), &original, true, |_| Ok(())).is_err());
        assert_eq!(
            fs::read_to_string(tree.path.join("keep")).unwrap(),
            "unrelated"
        );
        fs::remove_dir_all(&tree.path).unwrap();
        fs::rename(saved, &tree.path).unwrap();
        clean(&tree);
    }
}
