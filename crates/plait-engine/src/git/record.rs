//! Records of a checkout, kept by the host in its own store.
//!
//! A record is a commit in a private Git directory under `STORE/records`. Its
//! tree is the working tree as Git would store it, with filters and line-ending
//! conversion off, so it holds the exact bytes on disk. The person's repository
//! is only read: its objects are reached through `GIT_ALTERNATE_OBJECT_DIRECTORIES`
//! by commands that read, and every command that writes an object runs without
//! them, so Git never refreshes the modification time of an object it finds
//! there. The host also runs the recorder under a sandbox that denies writes
//! to the checkout and its Git directories (see `sandbox_profile`).
//!
//! Each checkout has its own linked-worktree directory in the store, with its
//! own `HEAD` and index. `HEAD` follows the checkout's latest record, so the
//! next status compares against it and lists only what changed, and `git prune`
//! keeps the objects that index and `HEAD` name.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::run;

type Result<T> = std::result::Result<T, String>;

/// An individually ignored file is recorded up to this size.
pub const IGNORED_FILE_LIMIT: u64 = 1 << 20;
/// At most this many ignored files are kept in one checkout's records.
pub const IGNORED_COUNT_LIMIT: usize = 2_000;
/// The ignored files kept in one checkout's records total at most this.
pub const IGNORED_TOTAL_LIMIT: u64 = 64 << 20;
/// Any single file larger than this is left out of a record.
pub const FILE_LIMIT: u64 = 100 << 20;

/// A checkout's first record packs its files when it must hash at least this many.
const BULK_PATHS: usize = 1_000;
/// A record of a large checkout can take a while the first time; later ones are quick.
const TIMEOUT: Duration = Duration::from_secs(120);
/// Includes discovery and capture in the host; below the runtime's five minutes.
pub const BUDGET: Duration = Duration::from_secs(240);
const LIMIT: usize = 8 * 1024 * 1024;
const LISTING_LIMIT: usize = 512 * 1024 * 1024;
/// The search path for Git in every recording command; nothing from the host's shell.
const SAFE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// Every recording command, against the private Git directory.
const PRIVATE_FLAGS: &[&str] = &[
    "core.autocrlf=false",
    "core.safecrlf=false",
    "core.fileMode=true",
    "core.symlinks=true",
    "core.fsmonitor=false",
    "core.untrackedCache=false",
    "core.splitIndex=false",
    "core.hooksPath=/dev/null",
    "core.logAllRefUpdates=false",
    "gc.auto=0",
    "maintenance.auto=false",
    "index.sparse=false",
];
/// Attributes in the private Git directory win over the checkout's own, so
/// nothing is converted on the way into a record.
const ATTRIBUTES: &str = "* -text -eol -crlf -filter -ident -working-tree-encoding\n";
/// Attributes that mean the person's index holds converted content, not the bytes on disk.
const CONVERSIONS: &[&str] = &[
    "text",
    "eol",
    "crlf",
    "filter",
    "ident",
    "working-tree-encoding",
];
const AUTHOR: &str = "Plait";
const EMAIL: &str = "records@plait.invalid";

/// Where a checkout's Git data lives. Every path is canonical, so a sandbox
/// profile built from them matches what the kernel sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub index: PathBuf,
    pub object_format: String,
    /// Object directories the repository borrows from; also only read.
    #[serde(default)]
    pub alternates: Vec<PathBuf>,
}

/// One record to make. `before` is the commit of the run's earlier record; it
/// makes this the run's later record and asks for the paths that changed.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    pub checkout: PathBuf,
    pub store: PathBuf,
    pub id: String,
    #[serde(default)]
    pub before: Option<String>,
}

/// Why a path is not in a record.
pub use plait_protocol::git::record::{Reason, Uncovered};

/// A path whose entry differs between a run's two records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedPath {
    pub path: String,
    /// Git mode and object of the earlier entry, absent when the run created the path.
    pub before_mode: Option<String>,
    pub before_oid: Option<String>,
    pub after_mode: Option<String>,
    pub after_oid: Option<String>,
    /// Lines added and removed; absent for a binary file.
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub commit: String,
    pub tree: String,
    pub object_format: String,
    /// The person's `HEAD` commit, absent before their first commit.
    pub head: Option<String>,
    /// The branch `HEAD` names, absent when detached.
    pub head_ref: Option<String>,
    pub not_covered: Vec<Uncovered>,
    pub not_covered_total: usize,
    /// For a run's later record: the paths the run changed, sorted.
    pub paths: Option<Vec<ChangedPath>>,
    pub elapsed_ms: u64,
}

/// What a record says about itself, in its commit message, so the run's later
/// record knows which paths the earlier one could not see.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Note {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    undo_paths: Option<Vec<String>>,
    not_covered: Vec<Uncovered>,
    total: usize,
}

/// Finds the checkout's Git directories without writing anything.
pub fn locate(checkout: &Path) -> Result<Located> {
    let checkout = canonical(checkout)?;
    let out = ok(
        person(&checkout).args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
            "--git-path",
            "index",
            "--show-object-format",
        ]),
        None,
        LIMIT,
    )?;
    let text = String::from_utf8(out).map_err(|_| "Checkout paths must be UTF-8.")?;
    let lines: Vec<&str> = text.lines().collect();
    let [root, git_dir, common_dir, index, format] = lines[..] else {
        return Err("Git did not describe the checkout.".into());
    };
    if canonical(Path::new(root))? != checkout {
        return Err("The checkout must be the top of its working tree.".into());
    }
    if !matches!(format, "sha1" | "sha256") {
        return Err(format!("Unsupported object format {format}."));
    }
    let index = Path::new(index);
    let index_dir = canonical(index.parent().ok_or("Git index path has no parent")?)?;
    let common_dir = canonical(Path::new(common_dir))?;
    Ok(Located {
        alternates: borrowed_objects(&common_dir.join("objects")),
        root: checkout,
        git_dir: canonical(Path::new(git_dir))?,
        index: index_dir.join(index.file_name().ok_or("Git index path has no name")?),
        common_dir,
        object_format: format.to_owned(),
    })
}

/// The object directories a repository's `objects/info/alternates` names.
fn borrowed_objects(objects: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(objects.join("info/alternates")) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| fs::canonicalize(objects.join(line)).ok())
        .collect()
}

fn canonical(path: &Path) -> Result<PathBuf> {
    let path =
        fs::canonicalize(path).map_err(|e| format!("Cannot resolve {}: {e}", path.display()))?;
    // Profiles and line-based Git output cannot carry these safely.
    if path
        .as_os_str()
        .as_bytes()
        .iter()
        .any(|b| b.is_ascii_control())
    {
        return Err("Checkout paths must not contain control characters.".into());
    }
    Ok(path)
}

/// The directory that holds every record in a store.
pub fn records_dir(store: &Path) -> PathBuf {
    store.join("records")
}

/// A macOS sandbox profile for the recorder: everything is allowed except
/// writing outside the record store. Overlapping stores and checkouts are
/// refused, so the writable exception cannot expose the person's repository.
pub fn sandbox_profile(protected: &[&Path], records: &Path) -> Result<String> {
    for path in protected {
        if path.starts_with(records) || records.starts_with(path) {
            return Err("The record store and checkout must not overlap.".into());
        }
    }
    let quote = |path: &Path| -> Result<String> {
        let text = path.to_str().ok_or("Sandbox paths must be UTF-8.")?;
        if text.chars().any(char::is_control) {
            return Err("Sandbox paths must not contain control characters.".into());
        }
        Ok(format!(
            "\"{}\"",
            text.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    };
    let mut denied = String::new();
    for path in protected {
        denied.push_str(&format!(" (subpath {})", quote(path)?));
    }
    // A rule with no paths would deny every write, `/dev/null` included.
    let deny = if denied.is_empty() {
        String::new()
    } else {
        format!(" (deny file-write*{denied})")
    };
    Ok(format!(
        "(version 1) (allow default) (deny network*) (deny file-write*){deny} (allow file-write* (literal \"/dev/null\") (subpath {}))",
        quote(records)?
    ))
}

/// The paths a record must never write: the checkout and its Git directories.
pub fn protected_paths(located: &Located) -> Vec<&Path> {
    let mut paths = vec![
        located.root.as_path(),
        located.git_dir.as_path(),
        located.common_dir.as_path(),
    ];
    paths.extend(located.alternates.iter().map(PathBuf::as_path));
    paths
}

/// Records the checkout. See the module comment for what a record holds.
pub fn record(located: &Located, request: &Request) -> Result<Record> {
    within(Instant::now() + BUDGET, || {
        record_with_paths(located, request, None)
    })
}

/// Undo only claims writes it completed, even if another process edited the checkout.
pub fn record_undo(located: &Located, request: &Request, paths: &[String]) -> Result<Record> {
    within(Instant::now() + BUDGET, || {
        record_with_paths(located, request, Some(paths))
    })
}

fn record_with_paths(
    located: &Located,
    request: &Request,
    undo_paths: Option<&[String]>,
) -> Result<Record> {
    let start = Instant::now();
    valid_id(&request.id)?;
    let before = match &request.before {
        Some(commit) => Some(valid_oid(commit, &located.object_format)?),
        None => None,
    };
    let config = PersonConfig::read(&located.root)?;
    let private = Private::open(&records_dir(&request.store), located, &config)?;
    let Indexed {
        files: tracked,
        submodules,
    } = indexed(located, &private)?;
    let mut rehash = if private.fresh {
        private.seed(located, &config, &tracked)?
    } else {
        Vec::new()
    };
    let (head, head_ref) = person_head(&located.root)?;
    let mut listing = private.status()?;
    let private_paths = private.index_paths()?;
    let mut tracked: HashSet<Vec<u8>> = tracked.into_iter().collect();
    let untracked: Vec<Vec<u8>> = private_paths
        .iter()
        .filter(|path| !tracked.contains(*path))
        .cloned()
        .collect();
    let mut ignored: BTreeSet<Vec<u8>> = private.ignored(&untracked)?.into_iter().collect();
    let ignored_directories = private.ignored_directories(&listing, &ignored)?;
    let skipped: HashSet<_> = ignored_directories
        .iter()
        .cloned()
        .chain(submodules.iter().map(|p| {
            let mut p = p.clone();
            p.push(b'/');
            p
        }))
        .collect();
    let mut inventory = inventory(&private.root, &skipped)?;
    // Reuse the directory inventory to reconcile filesystem aliases with Git.
    let reconcile = (!rehash.is_empty()).then(|| {
        private_paths
            .iter()
            .chain(&rehash)
            .cloned()
            .collect::<Vec<_>>()
    });
    for (old, new) in renamed_spellings(
        &located.root,
        reconcile.as_deref().unwrap_or(&private_paths),
        &mut inventory.names,
    ) {
        for path in &mut rehash {
            if *path == old {
                *path = new.clone();
            }
        }
        if tracked.contains(&old) {
            tracked.insert(new.clone());
        }
        if ignored.remove(&old) {
            ignored.insert(new.clone());
        }
        listing.retain(|entry| !matches!(entry, Entry::Tracked { path, .. } if path == &old));
        listing.push(Entry::Tracked {
            path: old,
            worktree: b'D',
            submodule: false,
        });
        listing.push(Entry::Untracked(new));
    }
    inventory.excluded.extend(submodules.iter().map(|path| {
        let mut directory = path.clone();
        directory.push(b'/');
        (directory, Reason::Submodule)
    }));
    let mut plan = plan(&located.root, &listing, &rehash, &mut ignored, &submodules);
    for (directory, reason) in ignored_directories
        .iter()
        .map(|p| (p, Reason::IgnoredDirectory))
        .chain(
            inventory
                .excluded
                .iter()
                .map(|(path, reason)| (path, *reason)),
        )
    {
        plan.not_covered.push(uncovered(directory, reason));
        let excluded = |path: &[u8]| {
            (path == directory.strip_suffix(b"/").unwrap_or(directory)
                || (directory.ends_with(b"/") && path.starts_with(directory)))
                && (reason != Reason::IgnoredDirectory || !tracked.contains(path))
        };
        plan.add.retain(|(path, _)| !excluded(path));
        ignored.retain(|path| !excluded(path));
        plan.remove
            .extend(private_paths.iter().filter(|path| excluded(path)).cloned());
    }
    let failed = private.apply(&plan)?;
    if !failed.is_empty() {
        // Whatever the index held for them before is no longer true.
        ok(
            private
                .git(false)
                .args(["update-index", "--force-remove", "-z", "--stdin"]),
            Some(nul_joined(&failed)),
            LIMIT,
        )?;
        let failed: HashSet<&Vec<u8>> = failed.iter().collect();
        plan.add.retain(|(path, _)| !failed.contains(path));
        for path in failed {
            ignored.remove(path);
            plan.not_covered.push(uncovered(path, Reason::Unreadable));
        }
    }
    let mut not_covered = plan.not_covered;
    not_covered.extend(changing(&located.root, &plan.add));
    not_covered.sort();
    not_covered.dedup();
    if private.fresh || !plan.add.is_empty() || !plan.remove.is_empty() {
        reject_aliases(&located.root, &private.index_paths()?, &mut inventory.names)?;
    }
    let tree = private.write_tree()?;
    if private.fresh {
        // A copied index can return its cached root tree without writing it.
        private.own_objects(&[tree.as_str()])?;
    }
    let note = Note {
        undo_paths: undo_paths.map(<[String]>::to_vec),
        total: not_covered.len(),
        not_covered: not_covered.clone(),
    };
    let commit = private.commit(&tree, before.as_deref(), &request.id, &note)?;
    private.update_refs(&request.id, &commit)?;
    fs::write(private.dir.join("plait-ready"), b"").map_err(|e| e.to_string())?;
    let paths = match before {
        Some(before) => {
            let earlier = private.note(&before)?;
            let (mut paths, extra) = private.changes(&before, &commit, &earlier, &not_covered)?;
            if let Some(scope) = undo_paths {
                paths.retain(|p| scope.contains(&p.path));
            }
            private.own_copies(&paths)?;
            not_covered.extend(earlier.not_covered);
            not_covered.extend(extra);
            not_covered.sort();
            not_covered.dedup();
            Some(paths)
        }
        None => None,
    };
    let total = not_covered.len();
    Ok(Record {
        commit,
        tree,
        object_format: located.object_format.clone(),
        head,
        head_ref,
        not_covered,
        not_covered_total: total,
        paths,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

pub(super) fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid record ID.".into());
    }
    Ok(())
}

fn valid_oid(oid: &str, format: &str) -> Result<String> {
    let length = if format == "sha256" { 64 } else { 40 };
    if oid.len() != length
        || !oid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Invalid record commit.".into());
    }
    Ok(oid.to_owned())
}

/// The Git every recording command runs. On macOS `/usr/bin/git` is a shim
/// that finds the developer tools on every call, which costs more than many
/// of the commands a record runs; it is resolved once per process instead.
/// `PLAIT_GIT` passes the result from the host to the contained recorder.
pub fn git_binary() -> &'static Path {
    static GIT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    GIT.get_or_init(|| {
        if let Some(path) = std::env::var_os("PLAIT_GIT")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_file())
        {
            return path;
        }
        resolved_shim().unwrap_or_else(|| {
            let developer_git =
                PathBuf::from("/Applications/Xcode.app/Contents/Developer/usr/bin/git");
            if developer_git.is_file() {
                developer_git
            } else {
                PathBuf::from("git")
            }
        })
    })
}

#[cfg(target_os = "macos")]
fn resolved_shim() -> Option<PathBuf> {
    let out = Command::new("/usr/bin/xcrun")
        .args(["--find", "git"])
        .env_clear()
        .env("PATH", SAFE_PATH)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    (out.status.success() && path.is_absolute() && path.is_file()).then_some(path)
}

#[cfg(not(target_os = "macos"))]
fn resolved_shim() -> Option<PathBuf> {
    None
}

/// A command against the person's repository. It only reads: optional locks are
/// off and no command here writes. `HOME` stays so their global settings apply.
fn person(root: &Path) -> Command {
    let mut command = Command::new(git_binary());
    command
        .env_clear()
        .env("PATH", SAFE_PATH)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "--no-replace-objects",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .arg("-C")
        .arg(root)
        .current_dir(root);
    for key in ["HOME", "XDG_CONFIG_HOME"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}

/// Runs a command that must succeed, with its whole output.
fn ok(command: &mut Command, input: Option<Vec<u8>>, limit: usize) -> Result<Vec<u8>> {
    let raw = run(command, input, limit, remaining()?)?;
    if !raw.status.success() {
        return Err(raw.failure());
    }
    if raw.out.truncated {
        return Err("Git output exceeds the record limit.".into());
    }
    Ok(raw.out.bytes)
}

/// Runs a command whose exit status 1 means "none".
fn maybe(command: &mut Command) -> Result<Option<Vec<u8>>> {
    let raw = run(command, None, LIMIT, remaining()?)?;
    match raw.status.code() {
        Some(0) => Ok(Some(raw.out.bytes)),
        Some(1) => Ok(None),
        _ => Err(raw.failure()),
    }
}

fn person_head(root: &Path) -> Result<(Option<String>, Option<String>)> {
    let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).trim().to_owned();
    let branch = maybe(person(root).args(["symbolic-ref", "-q", "HEAD"]))?.map(text);
    let commit =
        maybe(person(root).args(["rev-parse", "-q", "--verify", "HEAD^{commit}"]))?.map(text);
    Ok((commit, branch))
}

/// The person's settings a record needs to match their view of the checkout.
#[derive(Debug, Default)]
struct PersonConfig {
    autocrlf: bool,
    split_index: bool,
    sparse: bool,
    ignore_case: Option<bool>,
    precompose: Option<bool>,
    excludes: Option<PathBuf>,
    attributes: Option<PathBuf>,
}

impl PersonConfig {
    fn read(root: &Path) -> Result<Self> {
        let pattern = "^(core\\.(autocrlf|splitindex|sparsecheckout|ignorecase|precomposeunicode|excludesfile|attributesfile)|index\\.sparse)$";
        let bytes = maybe(person(root).args(["config", "-z", "--get-regexp", pattern]))?
            .unwrap_or_default();
        let mut config = Self::default();
        let mut excludes = None;
        let mut attributes = None;
        for entry in bytes.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('\n').unwrap_or((&entry, ""));
            let truth = value.is_empty() || boolean(value);
            match key {
                "core.autocrlf" => config.autocrlf = truth,
                "core.splitindex" => config.split_index = truth,
                "core.sparsecheckout" | "index.sparse" => config.sparse |= truth,
                "core.ignorecase" => config.ignore_case = Some(truth),
                "core.precomposeunicode" => config.precompose = Some(truth),
                "core.excludesfile" => excludes = Some(value.to_owned()),
                "core.attributesfile" => attributes = Some(value.to_owned()),
                _ => {}
            }
        }
        let at_checkout = |path: PathBuf| {
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        };
        config.excludes = user_file(excludes, "ignore").map(at_checkout);
        config.attributes = user_file(attributes, "attributes").map(at_checkout);
        Ok(config)
    }

    /// `-c` settings copied from the person's repository, so names compare as theirs do.
    fn overrides(&self) -> Vec<String> {
        let mut settings = Vec::new();
        if let Some(value) = self.ignore_case {
            settings.push(format!("core.ignoreCase={value}"));
        }
        if let Some(value) = self.precompose {
            settings.push(format!("core.precomposeUnicode={value}"));
        }
        settings
    }
}

fn boolean(value: &str) -> bool {
    !matches!(
        value.to_ascii_lowercase().as_str(),
        "false" | "no" | "off" | "0" | ""
    )
}

/// A configured per-user file, or Git's default under the XDG directory.
fn user_file(configured: Option<String>, name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(value) = configured {
        return match value.strip_prefix("~/") {
            Some(rest) => home.map(|home| home.join(rest)),
            None => Some(PathBuf::from(value)),
        };
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".config")))?;
    Some(xdg.join("git").join(name))
}

/// Where a checkout's records are made.
struct Private {
    root: PathBuf,
    /// This checkout's linked-worktree directory: `HEAD`, index, excludes.
    dir: PathBuf,
    /// The person's object directory, read through alternates.
    objects: PathBuf,
    overrides: Vec<String>,
    /// No record has been made here yet.
    fresh: bool,
}

/// What a checkout's directory in the store says about where it came from;
/// pruning uses it to find the checkouts still in use.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

impl Private {
    fn open(records: &Path, located: &Located, config: &PersonConfig) -> Result<Self> {
        private_dir(records)?;
        let common = records.join(&located.object_format);
        ensure_common(records, &common, &located.object_format)?;
        let dir = common.join("worktrees").join(checkout_key(&located.root)?);
        let fresh = !dir.join("plait-ready").is_file();
        let store = |e: io::Error| format!("Cannot prepare the record store: {e}");
        if fresh {
            fs::create_dir_all(&dir).map_err(store)?;
            fs::write(dir.join("commondir"), "../..\n").map_err(store)?;
            let gitdir = located.root.join(".git");
            let mut line = gitdir.as_os_str().as_bytes().to_vec();
            line.push(b'\n');
            fs::write(dir.join("gitdir"), line).map_err(store)?;
            let _ = fs::remove_file(dir.join("index"));
        }
        let source = Source {
            root: located.root.clone(),
            git_dir: located.git_dir.clone(),
            common_dir: located.common_dir.clone(),
        };
        let source = serde_json::to_vec(&source).map_err(|e| e.to_string())?;
        write_if_changed(&dir.join("plait-source.json"), &source).map_err(store)?;
        // Records in one checkout never overlap, so a lock left here is from a stopped recorder.
        for name in ["index.lock", "HEAD.lock"] {
            let _ = fs::remove_file(dir.join(name));
        }
        let exclude = dir.join("exclude");
        write_if_changed(&exclude, &excludes(located, config)).map_err(store)?;
        if fresh {
            // Written last: a directory with a HEAD is complete.
            fs::write(dir.join("HEAD"), "ref: refs/heads/none\n").map_err(store)?;
        }
        let mut overrides = config.overrides();
        // Records use the directory entry spelling, including decomposed names.
        overrides.push("core.precomposeUnicode=false".into());
        overrides.push(format!("core.excludesFile={}", exclude.display()));
        Ok(Self {
            root: located.root.clone(),
            objects: located.common_dir.join("objects"),
            dir,
            overrides,
            fresh,
        })
    }

    /// A command against the private Git directory. Only commands that read
    /// pass `alternates`, so no write reaches the person's objects.
    fn git(&self, alternates: bool) -> Command {
        let mut command = Command::new(git_binary());
        command
            .env_clear()
            .env("PATH", SAFE_PATH)
            .env("GIT_DIR", &self.dir)
            .env("GIT_WORK_TREE", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("HOME", "/nonexistent")
            .env("LC_ALL", "C")
            .current_dir(&self.root)
            .args(["--no-pager", "--literal-pathspecs", "--no-replace-objects"]);
        if alternates {
            command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", alternate(&self.objects));
        }
        for setting in PRIVATE_FLAGS
            .iter()
            .map(|s| s.to_string())
            .chain(self.overrides.iter().cloned())
        {
            command.arg("-c").arg(setting);
        }
        command
    }

    /// Starts this checkout's index from the person's, so the first record
    /// hashes only what differs from it. Returns paths whose index entry is
    /// converted content rather than the bytes on disk, which must be hashed again.
    fn seed(
        &self,
        located: &Located,
        config: &PersonConfig,
        tracked: &[Vec<u8>],
    ) -> Result<Vec<Vec<u8>>> {
        if config.autocrlf || config.split_index || config.sparse || shared_index(located) {
            return Ok(tracked.to_vec());
        }
        let index = self.dir.join("index");
        let temporary = self.dir.join("index.seed");
        match fs::copy(&located.index, &temporary) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(tracked.to_vec()),
            Err(e) => return Err(format!("Cannot read the checkout's index: {e}")),
        }
        fs::rename(&temporary, &index).map_err(|e| e.to_string())?;
        let listing = ok(
            self.git(false).args(["ls-files", "-z", "-v", "-s"]),
            None,
            LISTING_LIMIT,
        );
        let listing = match listing {
            Ok(listing) => listing,
            // An index this Git cannot read is no use as a start; hash everything.
            Err(_) => {
                let _ = fs::remove_file(&index);
                return Ok(tracked.to_vec());
            }
        };
        let mut paths = Vec::new();
        let mut gitlinks = Vec::new();
        for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let Some(tab) = entry.iter().position(|b| *b == b'\t') else {
                return Err("Malformed Git index listing.".into());
            };
            let (head, path) = (&entry[..tab], &entry[tab + 1..]);
            // Skip-worktree and assume-unchanged entries hide changes from status.
            if head[0] == b'S' || head[0].is_ascii_lowercase() {
                let _ = fs::remove_file(&index);
                return Ok(tracked.to_vec());
            }
            if head[2..].starts_with(b"160000 ") {
                gitlinks.push(path.to_vec());
            } else {
                paths.push(path.to_vec());
            }
        }
        if !gitlinks.is_empty() {
            // Submodules are other repositories; they show as nested ones from now on.
            ok(
                self.git(false)
                    .args(["update-index", "--force-remove", "-z", "--stdin"]),
                Some(nul_joined(&gitlinks)),
                LIMIT,
            )?;
        }
        paths.dedup();
        let converted = converted(located, self, config, &paths)?;
        if !converted.is_empty() {
            // `update-index --add` keeps an entry whose file metadata still
            // matches, so these must leave the index to be hashed from disk.
            ok(
                self.git(false)
                    .args(["update-index", "--force-remove", "-z", "--stdin"]),
                Some(nul_joined(&converted)),
                LIMIT,
            )?;
        }
        Ok(paths)
    }

    fn status(&self) -> Result<Vec<Entry>> {
        let bytes = ok(
            self.git(true).args([
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
                "--ignored=matching",
                "--no-renames",
                "--ignore-submodules=all",
            ]),
            None,
            LISTING_LIMIT,
        )?;
        parse_status(&bytes)
    }

    fn index_paths(&self) -> Result<Vec<Vec<u8>>> {
        let bytes = ok(
            self.git(false).args(["ls-files", "-z"]),
            None,
            LISTING_LIMIT,
        )?;
        Ok(bytes
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect())
    }

    fn ignored(&self, paths: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        let paths: Vec<_> = paths
            .iter()
            .filter(|path| {
                let relative = Path::new(OsStr::from_bytes(path));
                !relative
                    .ancestors()
                    .skip(1)
                    .filter(|p| !p.as_os_str().is_empty())
                    .any(|p| fs::symlink_metadata(self.root.join(p)).is_ok_and(|m| !m.is_dir()))
            })
            .cloned()
            .collect();
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let raw = run(
            self.git(false).args([
                "--no-literal-pathspecs",
                "check-ignore",
                "--no-index",
                "-z",
                "--stdin",
            ]),
            Some(nul_joined(&paths)),
            LISTING_LIMIT,
            remaining()?,
        )?;
        if raw.out.truncated {
            return Err("Git ignore output exceeds the record limit.".into());
        }
        match raw.status.code() {
            Some(0) | Some(1) => Ok(raw
                .out
                .bytes
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(<[u8]>::to_vec)
                .collect()),
            _ => Err(raw.failure()),
        }
    }

    fn ignored_directories(
        &self,
        listing: &[Entry],
        kept: &BTreeSet<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>> {
        let mut paths: Vec<Vec<u8>> = kept.iter().cloned().collect();
        paths.extend(listing.iter().filter_map(|entry| match entry {
            Entry::Ignored(path) => Some(path.clone()),
            _ => None,
        }));
        self.ignored(&ancestors(&paths).into_iter().collect::<Vec<_>>())
    }

    /// Updates the private index. Returns the paths Git could not take, such
    /// as a file cut short while it was read; the rest are taken.
    fn apply(&self, plan: &Plan) -> Result<Vec<Vec<u8>>> {
        if !plan.remove.is_empty() {
            ok(
                self.git(false)
                    .args(["update-index", "--force-remove", "-z", "--stdin"]),
                Some(nul_joined(&plan.remove)),
                LIMIT,
            )?;
        }
        let paths: Vec<Vec<u8>> = plan.add.iter().map(|(path, _)| path.clone()).collect();
        // Writing one loose object per file costs most of a large first
        // record; Git's bulk check-in streams them into one pack instead.
        // Only the first record does this; later small changes stay loose,
        // and retention repacks private objects to reclaim expired content.
        let bulk = self.fresh && paths.len() >= BULK_PATHS;
        let mut failed = Vec::new();
        self.add(&paths, bulk, &mut failed);
        Ok(failed)
    }

    /// Git stops at the first path it cannot read and writes nothing, so a
    /// failed batch is split until the paths that fail are alone.
    fn add(&self, paths: &[Vec<u8>], bulk: bool, failed: &mut Vec<Vec<u8>>) {
        if paths.is_empty() {
            return;
        }
        let mut command = self.git(false);
        if bulk {
            command.args(["-c", "core.bigFileThreshold=1"]);
        }
        let taken = ok(
            command.args([
                "update-index",
                "--add",
                "--remove",
                "--replace",
                "-z",
                "--stdin",
            ]),
            Some(nul_joined(paths)),
            LIMIT,
        );
        if taken.is_ok() {
            return;
        }
        if paths.len() == 1 {
            failed.push(paths[0].clone());
            return;
        }
        let (left, right) = paths.split_at(paths.len() / 2);
        self.add(left, bulk, failed);
        self.add(right, bulk, failed);
    }

    fn write_tree(&self) -> Result<String> {
        // Entries from the person's index name their objects; those are not copied here.
        let out = ok(
            self.git(false).args(["write-tree", "--missing-ok"]),
            None,
            LIMIT,
        )?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn commit(&self, tree: &str, parent: Option<&str>, id: &str, note: &Note) -> Result<String> {
        let mut message = format!("Plait record {id}\n\n").into_bytes();
        message.extend(serde_json::to_vec(note).map_err(|e| e.to_string())?);
        message.push(b'\n');
        let mut command = self.git(false);
        // A fixed author and date: the time of a record lives in the host's database.
        for (key, value) in [
            ("GIT_AUTHOR_NAME", AUTHOR),
            ("GIT_AUTHOR_EMAIL", EMAIL),
            ("GIT_AUTHOR_DATE", "@0 +0000"),
            ("GIT_COMMITTER_NAME", AUTHOR),
            ("GIT_COMMITTER_EMAIL", EMAIL),
            ("GIT_COMMITTER_DATE", "@0 +0000"),
        ] {
            command.env(key, value);
        }
        command.args(["commit-tree", tree]);
        if let Some(parent) = parent {
            command.args(["-p", parent]);
        }
        let out = ok(command.args(["-F", "-"]), Some(message), LIMIT)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn update_refs(&self, id: &str, commit: &str) -> Result<()> {
        let input = format!("update refs/records/{id} {commit}\nupdate HEAD {commit}\n");
        ok(
            self.git(false)
                .args(["update-ref", "--no-deref", "--stdin"]),
            Some(input.into_bytes()),
            LIMIT,
        )?;
        Ok(())
    }

    fn note(&self, commit: &str) -> Result<Note> {
        let out = ok(
            self.git(false).args(["cat-file", "commit", commit]),
            None,
            LISTING_LIMIT,
        )?;
        let text = String::from_utf8_lossy(&out);
        let body = text.split_once("\n\n").map(|(_, body)| body).unwrap_or("");
        let json = body.split_once("\n\n").map(|(_, json)| json).unwrap_or("");
        serde_json::from_str(json.trim()).map_err(|e| format!("Cannot read record coverage: {e}"))
    }

    /// The paths that differ between two records, and the paths left out of that list.
    fn changes(
        &self,
        before: &str,
        after: &str,
        earlier: &Note,
        later: &[Uncovered],
    ) -> Result<(Vec<ChangedPath>, Vec<Uncovered>)> {
        let out = ok(
            self.git(true).args([
                "diff-tree",
                "-r",
                "-z",
                "--raw",
                "--numstat",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                before,
                after,
            ]),
            None,
            LISTING_LIMIT,
        )?;
        let diff = parse_diff(&out)?;
        // A path absent from a record that could not see it may only look
        // created or deleted; one that changed while recorded may be torn.
        let blind = |change: &DiffEntry, path: &str| {
            blind_in(path, &earlier.not_covered, change.before_mode.is_some())
                || blind_in(path, later, change.after_mode.is_some())
        };
        let mut paths = Vec::new();
        let mut extra = Vec::new();
        for change in diff {
            let Ok(path) = String::from_utf8(change.path.clone()) else {
                extra.push(Uncovered {
                    path: String::from_utf8_lossy(&change.path).into_owned(),
                    reason: Reason::Name,
                });
                continue;
            };
            if change.before_mode.as_deref() == Some("160000")
                || change.after_mode.as_deref() == Some("160000")
            {
                extra.push(Uncovered {
                    path,
                    reason: Reason::Submodule,
                });
                continue;
            }
            if blind(&change, &path) {
                continue;
            }
            paths.push(ChangedPath {
                path,
                before_mode: change.before_mode,
                before_oid: change.before_oid,
                after_mode: change.after_mode,
                after_oid: change.after_oid,
                added: change.added,
                removed: change.removed,
            });
        }
        paths.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((paths, extra))
    }

    /// Makes sure the store holds the earlier content of every changed path
    /// itself, so pruning or rewriting the person's history cannot lose it.
    fn own_copies(&self, paths: &[ChangedPath]) -> Result<()> {
        let mut wanted: Vec<&str> = paths
            .iter()
            .filter(|p| {
                matches!(
                    p.before_mode.as_deref(),
                    Some("100644" | "100755" | "120000")
                )
            })
            .filter_map(|p| p.before_oid.as_deref())
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        self.own_objects(&wanted)
    }

    fn own_objects(&self, wanted: &[&str]) -> Result<()> {
        if wanted.is_empty() {
            return Ok(());
        }
        let missing = self.missing(wanted)?;
        if missing.is_empty() {
            return Ok(());
        }
        let list = missing.join("\n") + "\n";
        let mut pack = self.git(true);
        pack.args(["pack-objects", "--stdout", "-q"]);
        let mut unpack = self.git(false);
        unpack.args(["unpack-objects", "-q"]);
        pipe(&mut pack, list.into_bytes(), &mut unpack)?;
        let left = self.missing(wanted)?;
        if !left.is_empty() {
            return Err(format!(
                "The store could not keep {} earlier file versions.",
                left.len()
            ));
        }
        Ok(())
    }

    /// Objects the store does not hold itself.
    fn missing(&self, oids: &[&str]) -> Result<Vec<String>> {
        let input = oids.join("\n") + "\n";
        let out = ok(
            self.git(false)
                .args(["cat-file", "--batch-check=%(objectname) %(objecttype)"]),
            Some(input.into_bytes()),
            LISTING_LIMIT,
        )?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| line.strip_suffix(" missing"))
            .map(str::to_owned)
            .collect())
    }
}

/// The checkout's attributes that would convert content, path by path.
fn renamed_spellings(
    root: &Path,
    paths: &[Vec<u8>],
    directories: &mut HashMap<PathBuf, BTreeSet<OsString>>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut parents: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    parents.insert(PathBuf::new(), Some(root.to_path_buf()));
    fn actual(
        root: &Path,
        path: &Path,
        directories: &mut HashMap<PathBuf, BTreeSet<OsString>>,
        parents: &mut HashMap<PathBuf, Option<PathBuf>>,
    ) -> Option<PathBuf> {
        if let Some(known) = parents.get(path) {
            return known.clone();
        }
        let parent_path = path.parent()?;
        let parent = actual(root, parent_path, directories, parents)?;
        let names = directories.entry(parent.clone()).or_insert_with(|| {
            if !fs::symlink_metadata(&parent).is_ok_and(|m| m.is_dir()) {
                return BTreeSet::new();
            }
            fs::read_dir(&parent)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.ok().map(|e| e.file_name()))
                .collect()
        });
        let name = path.file_name()?;
        let resolved = if names.contains(name) {
            Some(parent.join(name))
        } else {
            let expected = fs::symlink_metadata(parent.join(name)).ok()?;
            names
                .iter()
                .find(|candidate| {
                    fs::symlink_metadata(parent.join(candidate))
                        .is_ok_and(|m| m.dev() == expected.dev() && m.ino() == expected.ino())
                })
                .map(|name| parent.join(name))
        };
        // Cache shared ancestors, not every leaf in a 50,000-file checkout.
        if resolved
            .as_ref()
            .is_some_and(|p| p != root && directories.contains_key(p))
        {
            parents.insert(path.to_path_buf(), resolved.clone());
        }
        resolved
    }
    let mut result = Vec::new();
    for path in paths {
        let relative = Path::new(OsStr::from_bytes(path));
        // Resolve and cache each parent once. A symlink directory supplies no names.
        if let Some(parent) = relative.parent()
            && !parents.contains_key(parent)
        {
            let resolved = actual(root, parent, directories, &mut parents);
            parents.insert(parent.to_path_buf(), resolved);
        }
        if let Some(resolved) = actual(root, relative, directories, &mut parents)
            && let Ok(relative) = resolved.strip_prefix(root)
            && relative.as_os_str().as_bytes() != path
        {
            result.push((path.clone(), relative.as_os_str().as_bytes().to_vec()));
        }
    }
    result
}

/// Distinct names may be hard links; only different spellings of one directory
/// entry are aliases. Never publish a tree that can address the same entry twice.
fn reject_aliases(
    root: &Path,
    paths: &[Vec<u8>],
    directories: &mut HashMap<PathBuf, BTreeSet<OsString>>,
) -> Result<()> {
    let entries: HashSet<_> = paths.iter().collect();
    for (_, actual) in renamed_spellings(root, paths, directories) {
        if entries.contains(&actual) {
            return Err("Record contains equivalent file names.".into());
        }
    }
    Ok(())
}

fn converted(
    located: &Located,
    private: &Private,
    config: &PersonConfig,
    paths: &[Vec<u8>],
) -> Result<Vec<Vec<u8>>> {
    let has_rules = paths
        .iter()
        .any(|p| p == b".gitattributes" || p.ends_with(b"/.gitattributes"))
        || [
            Some(located.common_dir.join("info/attributes")),
            config.attributes.clone(),
        ]
        .into_iter()
        .flatten()
        .any(|path| fs::metadata(path).is_ok_and(|m| m.len() > 0));
    if !has_rules || paths.is_empty() {
        return Ok(Vec::new());
    }
    let out = ok(
        index_reader(located, private)?
            .args(["check-attr", "-z", "--stdin"])
            .args(CONVERSIONS),
        Some(nul_joined(paths)),
        LISTING_LIMIT,
    )?;
    let fields: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    let mut found = Vec::new();
    for triple in fields.as_chunks::<3>().0 {
        if triple[2] != b"unspecified" && triple[2] != b"unset" {
            found.push(triple[0].to_vec());
        }
    }
    found.dedup();
    Ok(found)
}

/// The person's index defines which files remain tracked even under ignore rules.
struct Indexed {
    files: Vec<Vec<u8>>,
    submodules: HashSet<Vec<u8>>,
}

fn indexed(located: &Located, private: &Private) -> Result<Indexed> {
    let bytes = ok(
        index_reader(located, private)?.args(["ls-files", "--stage", "-z"]),
        None,
        LISTING_LIMIT,
    )?;
    let mut files = BTreeSet::new();
    let mut submodules = HashSet::new();
    for entry in bytes.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let tab = entry
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("Malformed Git index listing.")?;
        let path = &entry[tab + 1..];
        check_relative(path)?;
        if entry.starts_with(b"160000 ") {
            submodules.insert(path.to_vec());
        } else {
            files.insert(path.to_vec());
        }
    }
    Ok(Indexed {
        files: files.into_iter().collect(),
        submodules,
    })
}

/// Reading a split index refreshes sharedindex timestamps even with optional
/// locks disabled. Point all index readers at private copies instead.
fn index_reader(located: &Located, private: &Private) -> Result<Command> {
    let mut command = person(&located.root);
    if shared_index(located) {
        let index = private.dir.join("source-index");
        fs::copy(&located.index, &index).map_err(|e| e.to_string())?;
        for directory in [&located.git_dir, &located.common_dir] {
            for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_name().as_bytes().starts_with(b"sharedindex.") {
                    fs::copy(entry.path(), private.dir.join(entry.file_name()))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        command = private.git(true);
        command.env("GIT_INDEX_FILE", index);
    }
    Ok(command)
}

/// A split index keeps entries in another file, which the private copy cannot follow.
fn shared_index(located: &Located) -> bool {
    [&located.git_dir, &located.common_dir].iter().any(|dir| {
        fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_name().as_bytes().starts_with(b"sharedindex."))
        })
    })
}

/// The person's per-user excludes, then their repository's `info/exclude`,
/// which Git gives the higher precedence. Read from a private directory, Git
/// would otherwise see neither.
fn excludes(located: &Located, config: &PersonConfig) -> Vec<u8> {
    let mut bytes = Vec::new();
    for path in [
        config.excludes.clone(),
        Some(located.common_dir.join("info/exclude")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(content) = fs::read(path) {
            bytes.extend(content);
            bytes.push(b'\n');
        }
    }
    bytes
}

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| format!("Cannot create the record store: {e}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("Cannot protect the record store: {e}"))
}

/// Creates the store's Git directory for one object format, once.
fn ensure_common(records: &Path, common: &Path, format: &str) -> Result<()> {
    if common.join("HEAD").is_file() {
        return Ok(());
    }
    let temporary = records.join(format!(".init-{}-{format}", std::process::id()));
    let _ = fs::remove_dir_all(&temporary);
    let init = || -> Result<()> {
        ok(
            bare(&temporary).args([
                "-c",
                "init.defaultBranch=none",
                "init",
                "--quiet",
                "--bare",
                "--template=",
                &format!("--object-format={format}"),
            ]),
            None,
            LIMIT,
        )?;
        for (key, value) in [("core.bare", "false"), ("core.logAllRefUpdates", "false")] {
            ok(bare(&temporary).args(["config", key, value]), None, LIMIT)?;
        }
        fs::create_dir_all(temporary.join("info")).map_err(|e| e.to_string())?;
        fs::write(temporary.join("info/attributes"), ATTRIBUTES).map_err(|e| e.to_string())?;
        fs::write(temporary.join("info/exclude"), "").map_err(|e| e.to_string())?;
        Ok(())
    };
    if let Err(error) = init() {
        let _ = fs::remove_dir_all(&temporary);
        return Err(format!("Cannot create the record store: {error}"));
    }
    if fs::rename(&temporary, common).is_err() {
        // Another recorder finished first.
        let _ = fs::remove_dir_all(&temporary);
        if !common.join("HEAD").is_file() {
            return Err("Cannot create the record store.".into());
        }
    }
    Ok(())
}

/// A command against a store Git directory itself, with nothing from the person.
fn bare(dir: &Path) -> Command {
    let mut command = Command::new(git_binary());
    command
        .env_clear()
        .env("PATH", SAFE_PATH)
        .env("GIT_DIR", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("HOME", "/nonexistent")
        .env("LC_ALL", "C")
        .args([
            "--no-pager",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "gc.auto=0",
        ]);
    command
}

/// One store directory per checkout, named by its path and identity, so a
/// checkout replaced at the same path starts afresh.
fn checkout_key(root: &Path) -> Result<String> {
    let meta = fs::metadata(root).map_err(|e| format!("Cannot inspect the checkout: {e}"))?;
    let mut hash = Sha256::new();
    hash.update(root.as_os_str().as_bytes());
    hash.update(format!("\0{}\0{}", meta.dev(), meta.ino()));
    Ok(hash
        .finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// An alternates entry, quoted so a path with a colon is one entry.
fn alternate(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

fn nul_joined(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(paths.iter().map(|p| p.len() + 1).sum());
    for path in paths {
        bytes.extend_from_slice(path);
        bytes.push(0);
    }
    bytes
}

/// One entry of `git status --porcelain=v2 -z`.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    /// A path in the private index; `worktree` is Git's `Y` code against the work tree.
    Tracked {
        path: Vec<u8>,
        worktree: u8,
        submodule: bool,
    },
    Unmerged(Vec<u8>),
    Untracked(Vec<u8>),
    Ignored(Vec<u8>),
}

fn parse_status(bytes: &[u8]) -> Result<Vec<Entry>> {
    let malformed = || "Malformed Git status record.".to_string();
    let mut entries = Vec::new();
    let mut records = bytes.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.is_empty() || record[0] == b'#' {
            continue;
        }
        // Fields before the path, by record kind.
        let (fields, kind) = match record[0] {
            b'1' => (8, b'1'),
            b'2' => (9, b'2'),
            b'u' => (10, b'u'),
            b'?' | b'!' => (1, record[0]),
            _ => return Err(malformed()),
        };
        let mut parts = record.splitn(fields + 1, |b| *b == b' ');
        let head: Vec<&[u8]> = parts.by_ref().take(fields).collect();
        let path = parts.next().ok_or_else(malformed)?.to_vec();
        check_relative(&path)?;
        match kind {
            b'?' => entries.push(Entry::Untracked(path)),
            b'!' => entries.push(Entry::Ignored(path)),
            b'u' => entries.push(Entry::Unmerged(path)),
            _ => {
                let xy = head
                    .get(1)
                    .filter(|xy| xy.len() == 2)
                    .ok_or_else(malformed)?;
                let submodule = head.get(2).is_some_and(|s| s.first() == Some(&b'S'));
                if kind == b'2' {
                    // The source of a rename, which `--no-renames` should never produce.
                    let source = records.next().ok_or_else(malformed)?.to_vec();
                    check_relative(&source)?;
                    entries.push(Entry::Tracked {
                        path: source,
                        worktree: b'D',
                        submodule,
                    });
                }
                entries.push(Entry::Tracked {
                    path,
                    worktree: xy[1],
                    submodule,
                });
            }
        }
    }
    Ok(entries)
}

fn check_relative(path: &[u8]) -> Result<()> {
    let trimmed = path.strip_suffix(b"/").unwrap_or(path);
    super::validate_relative(Path::new(OsStr::from_bytes(trimmed)))
}

/// What one record changes in the private index.
#[derive(Debug, Default)]
struct Plan {
    /// Paths to take as they are now, with the metadata seen before taking them.
    add: Vec<(Vec<u8>, Stamp)>,
    /// Paths to drop: gone from the checkout, or no longer covered.
    remove: Vec<Vec<u8>>,
    not_covered: Vec<Uncovered>,
}

/// File metadata that changes whenever the content can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    mode: u32,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            mode: meta.mode(),
            size: meta.size(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

fn uncovered(path: &[u8], reason: Reason) -> Uncovered {
    Uncovered {
        path: String::from_utf8_lossy(path).into_owned(),
        reason,
    }
}

/// Decides, from a status listing, what goes into the record and what is left out.
fn plan(
    root: &Path,
    listing: &[Entry],
    rehash: &[Vec<u8>],
    ignored: &mut BTreeSet<Vec<u8>>,
    submodules: &HashSet<Vec<u8>>,
) -> Plan {
    let mut plan = Plan::default();
    for path in submodules {
        let mut directory = path.clone();
        directory.push(b'/');
        plan.not_covered
            .push(uncovered(&directory, Reason::Submodule));
        plan.remove.push(path.clone());
    }
    let lstat = |path: &[u8]| fs::symlink_metadata(root.join(OsStr::from_bytes(path)));
    let mut candidates: Vec<&[u8]> = rehash.iter().map(Vec::as_slice).collect();
    let mut fresh_ignored = Vec::new();
    for entry in listing {
        match entry {
            Entry::Tracked {
                path, submodule, ..
            } if *submodule => plan.not_covered.push(uncovered(path, Reason::Submodule)),
            Entry::Tracked { path, worktree, .. } => match worktree {
                b'.' => {}
                b'D' => {
                    ignored.remove(path);
                    plan.remove.push(path.clone());
                }
                _ => candidates.push(path),
            },
            Entry::Unmerged(path) => candidates.push(path),
            Entry::Untracked(path) if path.ends_with(b"/") => {
                let name = &path[..path.len() - 1];
                let reason = if submodules.contains(name) {
                    Reason::Submodule
                } else {
                    Reason::NestedRepository
                };
                plan.not_covered.push(uncovered(path, reason));
            }
            Entry::Untracked(path) => candidates.push(path),
            Entry::Ignored(path) if path.ends_with(b"/") => {
                plan.not_covered
                    .push(uncovered(path, Reason::IgnoredDirectory));
            }
            Entry::Ignored(path) => fresh_ignored.push(path),
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    for path in candidates {
        if submodules.iter().any(|base| {
            path == base
                || path
                    .strip_prefix(base.as_slice())
                    .is_some_and(|rest| rest.starts_with(b"/"))
        }) {
            plan.remove.push(path.to_vec());
            continue;
        }
        if std::str::from_utf8(path).is_err() {
            plan.remove.push(path.to_vec());
            plan.not_covered.push(uncovered(path, Reason::Name));
            continue;
        }
        match lstat(path) {
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    plan.not_covered.push(uncovered(path, Reason::Unreadable));
                }
                ignored.remove(path);
                plan.remove.push(path.to_vec());
            }
            Ok(meta) if meta.is_dir() => plan.remove.push(path.to_vec()),
            Ok(meta) if !(meta.is_file() || meta.file_type().is_symlink()) => {
                ignored.remove(path);
                plan.remove.push(path.to_vec());
                plan.not_covered.push(uncovered(path, Reason::Special));
            }
            Ok(meta) if meta.len() > FILE_LIMIT => {
                ignored.remove(path);
                plan.remove.push(path.to_vec());
                plan.not_covered.push(uncovered(path, Reason::TooLarge));
            }
            // An ignored file kept earlier that has since grown past its limit.
            Ok(meta) if meta.len() > IGNORED_FILE_LIMIT && ignored.contains(path) => {
                ignored.remove(path);
                plan.remove.push(path.to_vec());
                plan.not_covered.push(uncovered(path, Reason::IgnoredLarge));
            }
            Ok(meta) => plan.add.push((path.to_vec(), Stamp::of(&meta))),
        }
    }
    let mut total = 0;
    let mut count = 0;
    let mut dropped = HashSet::new();
    ignored.retain(|path| {
        let Ok(meta) = lstat(path) else {
            plan.remove.push(path.clone());
            dropped.insert(path.clone());
            return false;
        };
        let reason = if meta.len() > IGNORED_FILE_LIMIT {
            Some(Reason::IgnoredLarge)
        } else if count >= IGNORED_COUNT_LIMIT || total + meta.len() > IGNORED_TOTAL_LIMIT {
            Some(Reason::IgnoredLimit)
        } else {
            None
        };
        if let Some(reason) = reason {
            plan.remove.push(path.clone());
            plan.not_covered.push(uncovered(path, reason));
            dropped.insert(path.clone());
            false
        } else {
            total += meta.len();
            count += 1;
            true
        }
    });
    plan.add.retain(|(path, _)| !dropped.contains(path));
    let taken: HashSet<Vec<u8>> = plan.add.iter().map(|(path, _)| path.clone()).collect();
    for path in fresh_ignored {
        if taken.contains(path) {
            continue;
        }
        let Ok(meta) = lstat(path) else { continue };
        if !(meta.is_file() || meta.file_type().is_symlink()) {
            plan.not_covered.push(uncovered(path, Reason::Special));
        } else if meta.len() > IGNORED_FILE_LIMIT {
            plan.not_covered.push(uncovered(path, Reason::IgnoredLarge));
        } else if ignored.len() >= IGNORED_COUNT_LIMIT || total + meta.len() > IGNORED_TOTAL_LIMIT {
            plan.not_covered.push(uncovered(path, Reason::IgnoredLimit));
        } else {
            total += meta.len();
            ignored.insert(path.to_vec());
            plan.add.push((path.to_vec(), Stamp::of(&meta)));
        }
    }
    plan.remove.sort_unstable();
    plan.remove.dedup();
    plan
}

/// Directory names include the slash, so prefix checks cannot match a sibling.
fn ancestors(paths: &[Vec<u8>]) -> BTreeSet<Vec<u8>> {
    let mut directories = BTreeSet::new();
    for path in paths {
        for (index, byte) in path.iter().enumerate() {
            if *byte == b'/' {
                directories.insert(path[..=index].to_vec());
            }
        }
    }
    directories
}

#[derive(Default)]
struct Inventory {
    names: HashMap<PathBuf, BTreeSet<OsString>>,
    excluded: Vec<(Vec<u8>, Reason)>,
}

/// Git omits unreadable untracked folders and special files from successful
/// status output. Enumerate directories ourselves; file types come from readdir.
fn inventory(root: &Path, skipped: &HashSet<Vec<u8>>) -> Result<Inventory> {
    let mut result = Inventory::default();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        remaining()?;
        let directory = root.join(&relative);
        let mut prefix = relative.as_os_str().as_bytes().to_vec();
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        if skipped.contains(&prefix) {
            continue;
        }
        // Do not descend through a parent that changed to a symlink.
        if !fs::symlink_metadata(&directory).is_ok_and(|m| m.is_dir()) {
            result.excluded.push((prefix, Reason::Changing));
            continue;
        }
        let entries =
            fs::read_dir(&directory).and_then(|entries| entries.collect::<io::Result<Vec<_>>>());
        let Ok(entries) = entries else {
            if relative.as_os_str().is_empty() {
                return Err("Cannot read the checkout directory.".into());
            }
            result.names.insert(directory, BTreeSet::new());
            result.excluded.push((prefix, Reason::Unreadable));
            continue;
        };
        let names: BTreeSet<_> = entries.iter().map(|e| e.file_name()).collect();
        let repository = !relative.as_os_str().is_empty()
            && repository_entries(|name| {
                names
                    .iter()
                    .any(|n| n.as_bytes().eq_ignore_ascii_case(name.as_bytes()))
            });
        result.names.insert(directory, names);
        if repository {
            result.excluded.push((prefix, Reason::NestedRepository));
            continue;
        }
        for entry in entries {
            let name = entry.file_name();
            if name.as_bytes().eq_ignore_ascii_case(b".git") {
                continue;
            }
            let path = relative.join(&name);
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(kind) if kind.is_file() || kind.is_symlink() => {}
                kind => result.excluded.push((
                    path.as_os_str().as_bytes().to_vec(),
                    if kind.is_ok() {
                        Reason::Special
                    } else {
                        Reason::Unreadable
                    },
                )),
            }
        }
    }
    Ok(result)
}

/// Recognize worktrees and Git directories without running Git in each folder.
/// Ambiguous Git-shaped directories are excluded rather than risk their history.
pub(super) fn repository_entries(mut exists: impl FnMut(&str) -> bool) -> bool {
    exists(".git")
        || (exists("HEAD") && (exists("commondir") || (exists("objects") && exists("refs"))))
}

/// Paths that changed between being planned and being taken; what the
/// record holds for them may be torn.
fn changing(root: &Path, added: &[(Vec<u8>, Stamp)]) -> Vec<Uncovered> {
    added
        .iter()
        .filter(|(path, stamp)| {
            fs::symlink_metadata(root.join(OsStr::from_bytes(path)))
                .map(|meta| Stamp::of(&meta) != *stamp)
                .unwrap_or(true)
        })
        .map(|(path, _)| uncovered(path, Reason::Changing))
        .collect()
}

/// Whether a record that left out `list` cannot be trusted about `path`: the
/// path is, or is inside, an entry of the list, and the record either lacks it
/// or holds it as it was while changing.
fn blind_in(path: &str, list: &[Uncovered], present: bool) -> bool {
    list.iter().any(|entry| {
        let base = entry.path.trim_end_matches('/');
        let inside = path == base
            || path
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('/'));
        inside && (!present || entry.reason == Reason::Changing)
    })
}

#[derive(Debug, PartialEq, Eq)]
struct DiffEntry {
    path: Vec<u8>,
    before_mode: Option<String>,
    before_oid: Option<String>,
    after_mode: Option<String>,
    after_oid: Option<String>,
    added: Option<u64>,
    removed: Option<u64>,
}

/// Parses `diff-tree -r -z --raw --numstat`: every raw record, then every count.
fn parse_diff(bytes: &[u8]) -> Result<Vec<DiffEntry>> {
    let malformed = || "Malformed Git diff record.".to_string();
    let mut fields = bytes.split(|b| *b == 0).peekable();
    let mut entries = Vec::new();
    while let Some(field) = fields.next_if(|f| f.first() == Some(&b':')) {
        let text = std::str::from_utf8(&field[1..]).map_err(|_| malformed())?;
        let parts: Vec<&str> = text.split(' ').collect();
        let [before_mode, after_mode, before_oid, after_oid, _status] = parts[..] else {
            return Err(malformed());
        };
        let path = fields.next().ok_or_else(malformed)?.to_vec();
        check_relative(&path)?;
        let present =
            |mode: &str, oid: &str| (mode != "000000").then(|| (mode.to_owned(), oid.to_owned()));
        let before = present(before_mode, before_oid);
        let after = present(after_mode, after_oid);
        entries.push(DiffEntry {
            path,
            before_mode: before.as_ref().map(|b| b.0.clone()),
            before_oid: before.map(|b| b.1),
            after_mode: after.as_ref().map(|a| a.0.clone()),
            after_oid: after.map(|a| a.1),
            added: None,
            removed: None,
        });
    }
    let mut counts = std::collections::HashMap::new();
    for field in fields.filter(|f| !f.is_empty()) {
        let mut parts = field.splitn(3, |b| *b == b'\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(malformed());
        };
        let number = |value: &[u8]| std::str::from_utf8(value).ok()?.parse::<u64>().ok();
        counts.insert(path.to_vec(), (number(added), number(removed)));
    }
    for entry in &mut entries {
        if let Some((added, removed)) = counts.get(&entry.path) {
            entry.added = *added;
            entry.removed = *removed;
        }
    }
    Ok(entries)
}

/// Runs `first | second`, feeding `input` to the first, under the record deadline.
fn pipe(first: &mut Command, input: Vec<u8>, second: &mut Command) -> Result<()> {
    let spawn = |command: &mut Command, stdin: Stdio, stdout: Stdio| {
        command
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::null())
            .process_group(if has_deadline() {
                unsafe { libc::getpgrp() }
            } else {
                0
            })
            .spawn()
            .map_err(|e| format!("Cannot start Git: {e}"))
    };
    let mut producer = spawn(first, Stdio::piped(), Stdio::piped())?;
    let output = producer.stdout.take().ok_or("Git stdout is unavailable")?;
    let mut consumer = match spawn(second, Stdio::from(output), Stdio::null()) {
        Ok(child) => child,
        Err(error) => {
            let _ = producer.kill();
            let _ = producer.wait();
            return Err(error);
        }
    };
    if let Some(mut stdin) = producer.stdin.take() {
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let start = Instant::now();
    let timeout = remaining()?;
    let mut statuses = [None, None];
    while statuses.iter().any(Option::is_none) {
        for (slot, child) in statuses.iter_mut().zip([&mut producer, &mut consumer]) {
            if slot.is_none() {
                *slot = child.try_wait().map_err(|e| e.to_string())?;
            }
        }
        if start.elapsed() > timeout {
            for child in [&mut producer, &mut consumer] {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err("Copying earlier file versions took too long.".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if statuses.iter().all(|s| s.is_some_and(|s| s.success())) {
        Ok(())
    } else {
        Err("Git could not copy earlier file versions into the store.".into())
    }
}

/// A prune request: records to forget. Objects only they reached are deleted.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PruneRequest {
    pub store: PathBuf,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pruned {
    pub refs_removed: usize,
    pub checkouts_removed: usize,
    pub pruned: bool,
}

/// The checkouts the store's records belong to, for a prune's sandbox profile.
pub fn sources(store: &Path) -> Vec<Source> {
    let mut sources = Vec::new();
    for format in ["sha1", "sha256"] {
        let worktrees = records_dir(store).join(format).join("worktrees");
        let Ok(entries) = fs::read_dir(worktrees) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(source) = fs::read(entry.path().join("plait-source.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Source>(&bytes).ok())
            {
                sources.push(source);
            }
        }
    }
    sources
}

/// Forgets records and deletes the objects nothing else reaches. A checkout
/// that no longer exists loses its private index too.
pub fn prune(request: &PruneRequest) -> Result<Pruned> {
    within(Instant::now() + BUDGET, || prune_within(request))
}

fn prune_within(request: &PruneRequest) -> Result<Pruned> {
    for id in &request.remove {
        valid_id(id)?;
    }
    for id in &request.remove {
        for extension in ["json", "tmp", "paths"] {
            let path = records_dir(&request.store).join(format!("undo-{id}.{extension}"));
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    let mut result = Pruned::default();
    for format in ["sha1", "sha256"] {
        let common = records_dir(&request.store).join(format);
        if !common.join("HEAD").is_file() {
            continue;
        }
        let mut objects = Vec::new();
        let mut changed = false;
        if let Ok(entries) = fs::read_dir(common.join("worktrees")) {
            for entry in entries.flatten() {
                let source = fs::read(entry.path().join("plait-source.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Source>(&bytes).ok());
                let live = source.as_ref().filter(|source| {
                    checkout_key(&source.root).is_ok_and(|key| entry.file_name() == key.as_str())
                        && source.common_dir.join("objects").is_dir()
                });
                match live {
                    Some(source) => objects.push(source.common_dir.join("objects")),
                    None => {
                        fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
                        result.checkouts_removed += 1;
                        changed = true;
                    }
                }
            }
        }
        if !request.remove.is_empty() {
            let expired: HashSet<String> = request
                .remove
                .iter()
                .filter_map(|id| {
                    maybe(bare(&common).args([
                        "rev-parse",
                        "-q",
                        "--verify",
                        &format!("refs/records/{id}"),
                    ]))
                    .ok()
                    .flatten()
                    .map(|out| String::from_utf8_lossy(&out).trim().to_owned())
                })
                .collect();
            if let Ok(entries) = fs::read_dir(common.join("worktrees")) {
                for entry in entries.flatten() {
                    let head = fs::read_to_string(entry.path().join("HEAD")).unwrap_or_default();
                    if expired.contains(head.trim()) {
                        fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
                    }
                }
            }
            let input: String = request
                .remove
                .iter()
                .map(|id| format!("delete refs/records/{id}\n"))
                .collect();
            ok(
                bare(&common).args(["update-ref", "--stdin"]),
                Some(input.into_bytes()),
                LIMIT,
            )?;
            result.refs_removed += expired.len();
            changed = true;
        }
        if changed {
            let mut command = bare(&common);
            if !objects.is_empty() {
                let list: Vec<String> = objects.iter().map(|p| alternate(p)).collect();
                command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", list.join(":"));
            }
            // Repack private objects only; alternates remain read-only. Prune alone
            // never releases unreachable objects from the initial bulk pack.
            let mut repack = bare(&common);
            if !objects.is_empty() {
                repack.env(
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                    objects
                        .iter()
                        .map(|p| alternate(p))
                        .collect::<Vec<_>>()
                        .join(":"),
                );
            }
            ok(repack.args(["repack", "-a", "-d", "-l"]), None, LIMIT)?;
            ok(command.args(["prune", "--expire=now"]), None, LIMIT)?;
            result.pruned = true;
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;

// Undo has one budget across all of its Git commands, including both captures.
thread_local! {
    static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Share a helper's budget and supervised process group across all Git work.
pub fn within<T>(deadline: Instant, work: impl FnOnce() -> T) -> T {
    struct Reset(Option<Instant>);
    impl Drop for Reset {
        fn drop(&mut self) {
            DEADLINE.set(self.0);
        }
    }
    let deadline = DEADLINE.get().map_or(deadline, |outer| outer.min(deadline));
    let _reset = Reset(DEADLINE.replace(Some(deadline)));
    work()
}

pub(super) fn has_deadline() -> bool {
    DEADLINE.get().is_some()
}

pub(super) fn remaining() -> Result<Duration> {
    DEADLINE.get().map_or(Ok(TIMEOUT), |end| {
        end.checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .map(|d| d.min(TIMEOUT))
            .ok_or_else(|| "Checkout operation exceeded its time budget.".into())
    })
}

/// Reads only the private store. Undo needs changed blobs, not a copy of all
/// borrowed, unchanged objects. Reusing this command builder keeps filters off.
pub(super) struct Reader(Private);
impl Reader {
    pub(super) fn open(located: &Located, store: &Path) -> Result<Self> {
        Ok(Self(Private::open(
            &records_dir(store),
            located,
            &PersonConfig::read(&located.root)?,
        )?))
    }
    pub(super) fn changes(
        &self,
        before: &str,
        after: &str,
    ) -> Result<(Vec<ChangedPath>, Vec<Uncovered>)> {
        valid_oid(before, if before.len() == 64 { "sha256" } else { "sha1" })?;
        valid_oid(after, if before.len() == 64 { "sha256" } else { "sha1" })?;
        let earlier = self.0.note(before)?;
        let later = self.0.note(after)?;
        let (mut paths, mut uncovered) =
            self.0
                .changes(before, after, &earlier, &later.not_covered)?;
        if let Some(scope) = later.undo_paths {
            paths.retain(|p| scope.contains(&p.path));
        }
        uncovered.extend(earlier.not_covered);
        uncovered.extend(later.not_covered);
        uncovered.sort();
        uncovered.dedup();
        Ok((paths, uncovered))
    }
    pub(super) fn coverage(&self, before: &str, after: &str) -> Result<Vec<Uncovered>> {
        let mut result = self.0.note(before)?.not_covered;
        result.extend(self.0.note(after)?.not_covered);
        result.sort();
        result.dedup();
        Ok(result)
    }
    pub(super) fn entry(&self, commit: &str, path: &str) -> Result<Option<(String, String)>> {
        let bytes = ok(
            self.0.git(true).args(["ls-tree", "-z", commit, "--", path]),
            None,
            LIMIT,
        )?;
        let Some(entry) = bytes.split(|b| *b == 0).find(|e| !e.is_empty()) else {
            return Ok(None);
        };
        let tab = entry
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("Invalid record entry")?;
        let header = std::str::from_utf8(&entry[..tab]).map_err(|e| e.to_string())?;
        let fields: Vec<_> = header.split(' ').collect();
        if fields.len() != 3 {
            return Err("Invalid record entry".into());
        }
        if fields[1] == "tree" {
            return Ok(None);
        }
        Ok(Some((fields[0].into(), fields[2].into())))
    }
    pub(super) fn entries(
        &self,
        commit: &str,
    ) -> Result<std::collections::BTreeMap<String, (String, String)>> {
        let bytes = ok(
            self.0.git(true).args(["ls-tree", "-r", "-z", commit]),
            None,
            LISTING_LIMIT,
        )?;
        let mut entries = std::collections::BTreeMap::new();
        for entry in bytes.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let tab = entry
                .iter()
                .position(|b| *b == b'\t')
                .ok_or("Invalid record tree")?;
            let header = std::str::from_utf8(&entry[..tab]).map_err(|e| e.to_string())?;
            let parts: Vec<_> = header.split(' ').collect();
            if parts.len() != 3 {
                return Err("Invalid record tree".into());
            }
            if let Ok(path) = std::str::from_utf8(&entry[tab + 1..]) {
                entries.insert(path.to_owned(), (parts[0].to_owned(), parts[2].to_owned()));
            }
        }
        Ok(entries)
    }
    pub(super) fn blob(&self, oid: &str) -> Result<Vec<u8>> {
        ok(
            self.0.git(false).args(["cat-file", "blob", oid]),
            None,
            FILE_LIMIT as usize + 1,
        )
    }
}
