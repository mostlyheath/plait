use std::{
    ffi::OsStr,
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use crate::model::DiffKind;
use crate::model::{BranchHeader, Change, Diff, DiffMode, Snapshot, Tracking};
use crate::patch::FileHeaders;

const STATUS_LIMIT: usize = 8 * 1024 * 1024;
const DIFF_LIMIT: usize = 512 * 1024;
const ERROR_LIMIT: usize = 8192;
const TIMEOUT: Duration = Duration::from_secs(10);
/// How long to keep reading output after Git exits. A hook or filter can leave a
/// background process holding the pipes; Git's own output is complete by then.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

type Result<T> = std::result::Result<T, String>;

struct Output {
    bytes: Vec<u8>,
    truncated: bool,
}

fn drain(mut reader: impl Read, limit: usize) -> std::io::Result<Output> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let keep = n.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < n;
    }
    Ok(Output { bytes, truncated })
}

pub(crate) fn base_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
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
            "color.ui=false",
        ])
        .arg("-C")
        .arg(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    // The selected folder, not the launching shell, owns Git's repository context.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_EXTERNAL_DIFF",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(key);
    }
    command
}

/// A read command with every configured filter driver neutralized.
fn read_command(root: &Path) -> Result<Command> {
    let overrides = SHARED_OVERRIDES.with_borrow_mut(|shared| match shared {
        Some(cache) => match cache.get(root) {
            Some(overrides) => Ok(overrides.clone()),
            None => {
                let overrides = filter_overrides(root)?;
                cache.insert(root.to_owned(), overrides.clone());
                Ok(overrides)
            }
        },
        None => filter_overrides(root),
    })?;
    Ok(read_command_with(root, &overrides))
}

thread_local! {
    static SHARED_OVERRIDES: std::cell::RefCell<Option<std::collections::HashMap<PathBuf, Vec<String>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `read` with each checkout's filter configuration read once and reused,
/// instead of once per Git command. Keyed by checkout because a worktree can
/// carry its own configuration. Only for a read-only pass that decides nothing
/// by itself: removal reads configuration afresh under its lease.
pub(crate) fn sharing_config<T>(read: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            if self.0 {
                SHARED_OVERRIDES.set(None);
            }
        }
    }
    let outer = SHARED_OVERRIDES.with_borrow_mut(|shared| {
        let outer = shared.is_some();
        shared.get_or_insert_default();
        outer
    });
    let _reset = Reset(!outer);
    read()
}

/// The base command plus `-c` overrides from `filter_overrides`, so several
/// reads can share one configuration read.
fn read_command_with(root: &Path, overrides: &[String]) -> Command {
    let mut command = base_command(root);
    for setting in overrides {
        command.arg("-c").arg(setting);
    }
    command
}

/// Settings that disable every configured filter driver. Even read-only Git
/// commands can execute clean/process filters while comparing the worktree.
fn filter_overrides(root: &Path) -> Result<Vec<String>> {
    let keys = execute(
        base_command(root).args(["config", "--null", "--name-only", "--list"]),
        STATUS_LIMIT,
        TIMEOUT,
    )?;
    if keys.truncated {
        return Err("Git configuration exceeds the safe read limit".into());
    }
    let keys = String::from_utf8(keys.bytes).map_err(|_| "Git configuration keys are not UTF-8")?;
    let filters: std::collections::BTreeSet<_> = keys
        .split('\0')
        .filter_map(|key| {
            let (driver, _) = key.rsplit_once('.')?;
            driver.starts_with("filter.").then_some(driver)
        })
        .collect();
    Ok(filters
        .into_iter()
        .flat_map(|driver| {
            ["clean=", "process=", "required=false"].map(|setting| format!("{driver}.{setting}"))
        })
        .collect())
}

fn command(root: &Path, args: &[&OsStr], limit: usize) -> Result<Output> {
    execute(read_command(root)?.args(args), limit, TIMEOUT)
}

/// Whether HEAD names a commit. Exit status 1 from a quiet verify means an unborn branch.
fn head_exists(root: &Path) -> Result<bool> {
    let mut command = read_command(root)?;
    let raw = run(
        command.args(["rev-parse", "--verify", "-q", "HEAD"]),
        None,
        STATUS_LIMIT,
        TIMEOUT,
    )?;
    match raw.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(raw.failure()),
    }
}

/// A finished process: exit status plus bounded output.
struct Raw {
    status: ExitStatus,
    out: Output,
    err: Output,
}

impl Raw {
    fn failure(&self) -> String {
        format!("Git: {}", String::from_utf8_lossy(&self.err.bytes).trim())
    }
}

fn execute(command: &mut Command, limit: usize, timeout: Duration) -> Result<Output> {
    let raw = run(command, None, limit, timeout)?;
    if !raw.status.success() {
        return Err(raw.failure());
    }
    Ok(raw.out)
}

/// Run a short hardened read-only Git query (used by checkout lease resolution).
pub(crate) fn run_readonly(command: &mut Command) -> Result<Vec<u8>> {
    let output = execute(command, STATUS_LIMIT, TIMEOUT)?;
    if output.truncated {
        return Err("Git query output exceeds the safe read limit.".into());
    }
    Ok(output.bytes)
}

#[cfg(test)]
thread_local! {
    /// Git processes this thread has started, so tests can hold a path to its cost.
    pub(crate) static SPAWNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Run to completion under a deadline. `input` goes to stdin from its own
/// thread so a full pipe cannot stall the output readers or the deadline.
fn run(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
) -> Result<Raw> {
    run_recorded(command, input, limit, timeout, None)
}

fn run_recorded(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
    recording: Option<crate::lease::git_cleanup::Running>,
) -> Result<Raw> {
    run_with_reader(command, input, timeout, recording, move |pipe| {
        drain(pipe, limit)
    })
}

fn run_with_reader(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    recording: Option<crate::lease::git_cleanup::Running>,
    stdout_reader: impl FnOnce(std::process::ChildStdout) -> std::io::Result<Output> + Send + 'static,
) -> Result<Raw> {
    #[cfg(test)]
    SPAWNED.with(|count| count.set(count.get() + 1));
    let stdin = if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    let shared_group = record::has_deadline();
    if shared_group && recording.is_some() {
        // A recorded group must be Git's own, never the helper's.
        return Err("Git cleanup recording needs its own process group.".into());
    }
    let mut child = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Undo's helper owns the whole process group so its outer deadline
        // also kills Git. Ordinary reads/writes retain independent groups.
        .process_group(if shared_group {
            unsafe { libc::getpgrp() }
        } else {
            0
        })
        .spawn()
        .map_err(|e| {
            if let Some(recording) = &recording {
                let _ = recording.finish(true);
            }
            format!("Cannot start Git: {e}")
        })?;
    if let Some(recording) = &recording {
        recording.started(child.id());
    }
    let stdout = child.stdout.take().ok_or("Git stdout is unavailable")?;
    let stderr = child.stderr.take().ok_or("Git stderr is unavailable")?;
    let feed = child.stdin.take().zip(input).map(|(mut pipe, bytes)| {
        // Git may exit before reading everything; its exit status says why.
        thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        })
    });
    let out = reader(move || stdout_reader(stdout));
    let err = reader(move || drain(stderr, ERROR_LIMIT));
    let group = i32::try_from(child.id()).ok();
    let (done, finished) = mpsc::channel();
    // A waiting thread wakes the moment Git exits; polling added up to a
    // tick to every command, most of which take a few milliseconds.
    thread::spawn(move || {
        let _ = done.send(child.wait());
    });
    let status = match finished.recv_timeout(timeout) {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(e)) => {
            stop_group(group, shared_group);
            Err(format!("Cannot wait for Git: {e}"))
        }
        Err(_) => {
            stop_group(group, shared_group);
            // The kill ends the wait, which reaps Git.
            let _ = finished.recv_timeout(DRAIN_GRACE);
            Err(format!(
                "Git did not finish within {} seconds and was stopped.",
                timeout.as_secs()
            ))
        }
    };
    if let Some(recording) = &recording {
        recording.finish(status.as_ref().is_ok_and(|status| status.code().is_some()))?;
    }
    let status = status?;
    // A writer still blocked on a full pipe ends once Git's pipes close; it is
    // never waited on, so a stray descendant cannot hold this thread.
    drop(feed);
    let out = collect(out, "output")?;
    let err = collect(err, "error output")?;
    Ok(Raw { status, out, err })
}

/// Drain a pipe on its own thread, reporting through a channel so the wait can be bounded.
fn reader(
    read: impl FnOnce() -> std::io::Result<Output> + Send + 'static,
) -> mpsc::Receiver<std::io::Result<Output>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = send.send(read());
    });
    receive
}

/// Git has exited. Wait briefly for its output, then give up on a pipe that a
/// leftover process is holding open.
fn collect(reader: mpsc::Receiver<std::io::Result<Output>>, name: &str) -> Result<Output> {
    match reader.recv_timeout(DRAIN_GRACE) {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "Git finished, but a program it started still holds its {name}. Check your hooks and filters."
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!("Git {name} reader failed")),
    }
}

/// Kill Git and everything it started (hooks, filters). The thread waiting
/// on Git reaps it.
fn stop_group(group: Option<i32>, shared: bool) {
    // The child leads its own group (see `process_group(0)`), so its PID is the group ID.
    if let Some(group) = group {
        // SAFETY: killpg only sends a signal; a stale group ID fails with ESRCH.
        unsafe {
            if shared {
                libc::kill(group, libc::SIGKILL);
            } else {
                libc::killpg(group, libc::SIGKILL);
            }
        }
    }
}

/// The top of the work tree containing `path`, as Git reports it.
pub fn toplevel(path: &Path) -> Result<PathBuf> {
    let root = command(
        path,
        &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        STATUS_LIMIT,
    )?;
    let root = root.bytes.strip_suffix(b"\n").unwrap_or(&root.bytes);
    Ok(path_from_bytes(root))
}

/// A plain folder: no repository marker at `path` or any folder above it. A
/// marker is a `.git` file (a linked worktree) or a `.git` folder with `HEAD`,
/// as Git itself requires; a broken repository with one is not plain, and
/// Git's own error reports it. An empty stray `.git` folder is no marker.
pub fn plain_folder(path: &Path) -> bool {
    !path.ancestors().any(repository_marker)
}

fn repository_marker(folder: &Path) -> bool {
    let git = folder.join(".git");
    match std::fs::symlink_metadata(&git) {
        Ok(meta) if meta.is_dir() => std::fs::symlink_metadata(git.join("HEAD")).is_ok(),
        Ok(_) => true,
        Err(_) => false,
    }
}

pub fn status(path: &Path) -> Result<Snapshot> {
    let start = Instant::now();
    if plain_folder(path) {
        let root = path
            .canonicalize()
            .map_err(|_| "The folder is missing.".to_string())?;
        if !root.is_dir() {
            return Err("Not a folder.".into());
        }
        return Ok(Snapshot {
            root,
            branch: String::new(),
            named: false,
            tracking: None,
            changes: Vec::new(),
            elapsed: start.elapsed(),
            plain: true,
        });
    }
    let root = command(
        path,
        &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        STATUS_LIMIT,
    )?;
    let root = root.bytes.strip_suffix(b"\n").unwrap_or(&root.bytes);
    let root = path_from_bytes(root);
    let output = command(
        &root,
        &[
            OsStr::new("status"),
            OsStr::new("--porcelain=v1"),
            OsStr::new("-z"),
            OsStr::new("--branch"),
            OsStr::new("--untracked-files=all"),
            OsStr::new("--ignore-submodules=dirty"),
        ],
        STATUS_LIMIT,
    )?;
    if output.truncated {
        return Err("Status exceeds 8 MiB. Reduce the untracked-file set and Refresh.".into());
    }
    let (header, changes) = parse_status(&output.bytes)?;
    Ok(Snapshot {
        root,
        branch: header.branch,
        named: header.named,
        tracking: header.tracking,
        changes,
        elapsed: start.elapsed(),
        plain: false,
    })
}

pub fn parse_status(bytes: &[u8]) -> Result<(BranchHeader, Vec<Change>)> {
    let mut records = bytes.split(|b| *b == 0).filter(|s| !s.is_empty());
    let header = records.next().ok_or("Git returned an empty status")?;
    let branch = header
        .strip_prefix(b"## ")
        .ok_or("Missing Git branch header")?;
    let mut changes = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            return Err("Malformed Git status record".into());
        }
        let index = record[0];
        let worktree = record[1];
        let path = path_from_bytes(&record[3..]);
        validate_relative(&path)?;
        let original = if [index, worktree].iter().any(|s| matches!(s, b'R' | b'C')) {
            let old = path_from_bytes(records.next().ok_or("Missing rename source")?);
            validate_relative(&old)?;
            Some(old)
        } else {
            None
        };
        changes.push(Change {
            path,
            original,
            index,
            worktree,
        });
    }
    Ok((parse_branch(&String::from_utf8_lossy(branch)), changes))
}

/// Read Git's porcelain v1 branch line, without the leading `## `:
/// `main`, `main...origin/main [ahead 2, behind 1]`, `main...origin/main [gone]`,
/// `No commits yet on main`, or `HEAD (no branch)`. Ref names cannot contain
/// spaces or `..`, so the first space and the first `...` are delimiters.
pub fn parse_branch(line: &str) -> BranchHeader {
    for unborn in ["No commits yet on ", "Initial commit on "] {
        if let Some(name) = line.strip_prefix(unborn) {
            return BranchHeader {
                branch: name.to_owned(),
                ..BranchHeader::default()
            };
        }
    }
    if line.starts_with("HEAD (no branch)") {
        return BranchHeader {
            branch: "Detached HEAD".into(),
            ..BranchHeader::default()
        };
    }
    let (refs, counts) = line.split_once(' ').unwrap_or((line, ""));
    let Some((branch, upstream)) = refs.split_once("...") else {
        return BranchHeader {
            branch: refs.to_owned(),
            named: true,
            tracking: None,
        };
    };
    let mut tracking = Tracking {
        upstream: upstream.to_owned(),
        ..Tracking::default()
    };
    let counts = counts.trim_start_matches('[').trim_end_matches(']');
    for part in counts.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            tracking.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            tracking.behind = n.parse().unwrap_or(0);
        } else if part == "gone" {
            tracking.gone = true;
        }
    }
    BranchHeader {
        branch: branch.to_owned(),
        named: true,
        tracking: Some(tracking),
    }
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

pub(super) fn validate_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Git returned an unsafe relative path".into());
    }
    Ok(())
}

pub fn diff(root: &Path, change: &Change, mode: DiffMode) -> Result<Diff> {
    let start = Instant::now();
    validate_relative(&change.path)?;
    if let Some(original) = &change.original {
        validate_relative(original)?;
    }
    if change.index == b'?' {
        if mode == DiffMode::Staged {
            return Ok(Diff::message(
                "Untracked file. There is no staged version.",
                start.elapsed(),
            ));
        }
        return untracked(root, change, start);
    }
    let mut args: Vec<&OsStr> = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--unified=3",
        "--ignore-submodules=dirty",
        "--submodule=short",
    ]
    .into_iter()
    .map(OsStr::new)
    .collect();
    if mode == DiffMode::Staged {
        args.push(OsStr::new("--cached"));
    }
    args.push(OsStr::new("--"));
    args.push(change.path.as_os_str());
    if let Some(original) = &change.original {
        args.push(original.as_os_str());
    }
    let output = command(root, &args, DIFF_LIMIT)?;
    // One file, named in the bar above its diff.
    let mut diff = format_diff(
        &output.bytes,
        output.truncated,
        start.elapsed(),
        FileHeaders::Omit,
    );
    diff.elapsed = start.elapsed();
    Ok(diff)
}

fn untracked(root: &Path, change: &Change, start: Instant) -> Result<Diff> {
    let path = root.join(&change.path);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|e| format!("Cannot inspect file: {e}"))?;
    if !metadata.is_file() {
        return Ok(Diff::message(
            "Untracked symlink or special file. Content is not followed.",
            start.elapsed(),
        ));
    }
    let mut bytes = Vec::new();
    open_regular_file(root, &change.path)?
        .take(DIFF_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let truncated = bytes.len() > DIFF_LIMIT;
    bytes.truncate(DIFF_LIMIT);
    if let Err(error) = std::str::from_utf8(&bytes)
        && truncated
        && error.error_len().is_none()
    {
        bytes.truncate(error.valid_up_to());
    }
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return Ok(Diff::message(
            "Binary or non-UTF-8 file. Text preview is unavailable.",
            start.elapsed(),
        ));
    }
    if bytes.is_empty() {
        return Ok(Diff::message(
            "Empty untracked file. Nothing to preview.",
            start.elapsed(),
        ));
    }
    let mut patch = Vec::new();
    writeln!(patch, "Untracked file — preview only").map_err(|e| e.to_string())?;
    writeln!(
        patch,
        "@@ -0,0 +1,{} @@",
        bytes.split_inclusive(|b| *b == b'\n').count()
    )
    .map_err(|e| e.to_string())?;
    for line in bytes
        .strip_suffix(b"\n")
        .unwrap_or(&bytes)
        .split(|b| *b == b'\n')
    {
        patch.push(b'+');
        patch.extend_from_slice(line);
        patch.push(b'\n');
    }
    let mut diff = format_diff(&patch, truncated, start.elapsed(), FileHeaders::Omit);
    diff.elapsed = start.elapsed();
    Ok(diff)
}

pub use plait_protocol::git::{FILE_LIST_LIMIT, FileList};

/// List the files in the checkout at `root` without reading their contents.
///
/// Files deleted from the worktree, sparse-checkout entries, and nested
/// repositories are left out. Submodules appear as their gitlink path.
pub fn files(root: &Path) -> Result<FileList> {
    let start = Instant::now();
    if plain_folder(root) {
        let mut list = plain_files(root, FILE_LIST_LIMIT, Duration::from_secs(2));
        list.elapsed = start.elapsed();
        return Ok(list);
    }
    let args = [
        "ls-files",
        "-z",
        "-t",
        "--cached",
        "--others",
        "--deleted",
        "--exclude-standard",
        "--full-name",
    ]
    .map(OsStr::new);
    let output = command(root, &args, STATUS_LIMIT)?;
    let mut list = parse_files(&output.bytes, output.truncated, FILE_LIST_LIMIT)?;
    list.elapsed = start.elapsed();
    Ok(list)
}

/// Files in a plain folder, without Git: hidden entries are skipped, links are
/// not followed, and the walk stops at `limit` files or after `budget`.
fn plain_files(root: &Path, limit: usize, budget: Duration) -> FileList {
    let deadline = Instant::now() + budget;
    let mut list = FileList::default();
    let mut folders = vec![PathBuf::new()];
    while let Some(relative) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&relative)) else {
            continue;
        };
        for entry in entries.flatten() {
            if list.paths.len() >= limit || Instant::now() > deadline {
                list.truncated = true;
                folders.clear();
                break;
            }
            let name = entry.file_name();
            if name.as_encoded_bytes().first() == Some(&b'.') {
                continue;
            }
            let path = relative.join(&name);
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file() {
                match path.to_str() {
                    Some(text) if !text.chars().any(char::is_control) => list
                        .paths
                        .push(text.replace(std::path::MAIN_SEPARATOR, "/")),
                    _ => list.skipped += 1,
                }
            }
        }
    }
    list.paths.sort();
    list
}

/// Parse `git ls-files -z -t` records: a status tag, a space, and a path.
fn parse_files(bytes: &[u8], clipped: bool, limit: usize) -> Result<FileList> {
    let mut records: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    // The final piece is empty after a complete record and partial after a clipped one.
    records.pop();
    let mut list = FileList {
        truncated: clipped,
        ..FileList::default()
    };
    let mut deleted = std::collections::HashSet::new();
    for record in records {
        let [tag, b' ', path @ ..] = record else {
            return Err("Malformed Git file record".into());
        };
        validate_relative(&path_from_bytes(path))?;
        // An untracked nested repository is reported as a directory.
        if *tag == b'S' || path.ends_with(b"/") {
            continue;
        }
        let Some(text) = prompt_path(path) else {
            list.skipped += 1;
            continue;
        };
        if *tag == b'R' {
            deleted.insert(text);
        } else {
            list.paths.push(text.to_owned());
        }
    }
    list.paths.retain(|path| !deleted.contains(path.as_str()));
    list.paths.sort_unstable();
    // An unmerged path has one record per stage.
    list.paths.dedup();
    if list.paths.len() > limit {
        list.paths.truncate(limit);
        list.truncated = true;
    }
    Ok(list)
}

/// A path is offered only if it survives being typed into a message unchanged.
fn prompt_path(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.chars().any(char::is_control))
}

fn open_regular_file(root: &Path, relative: &Path) -> Result<std::fs::File> {
    let file = open_beneath(root, relative, libc::O_RDONLY | libc::O_NONBLOCK, 0)
        .map_err(|e| format!("Cannot safely open preview: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Preview is not a regular file".into());
    }
    Ok(file)
}

/// Open `relative` under `root` without following a symlink anywhere on the
/// way, so a link swapped in by another process cannot point outside the
/// checkout. `flags` and `mode` apply to the last component only.
pub(crate) fn open_beneath(
    root: &Path,
    relative: &Path,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> Result<std::fs::File> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    validate_relative(relative)?;
    let mut file = std::fs::File::open(root).map_err(|e| e.to_string())?;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let name = CString::new(component.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let last = components.peek().is_none();
        let flags = libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if last {
                flags
            } else {
                libc::O_RDONLY | libc::O_DIRECTORY
            };
        // Resolve each component from its open parent so a concurrent symlink swap cannot escape.
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags, mode) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // openat returned an owned descriptor; File closes it on every return path.
        file = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(file)
}

/// A diff line as shown: clipped, with tabs expanded. Staging a hunk compares
/// Git's raw header through this, so it matches what the person saw.
use plait_protocol::git::{display_line, format_diff};

mod branch;
pub mod build_output;
mod counts;
pub mod log;
pub mod record;
pub mod review;
pub mod summary;
pub mod undo;
pub mod worktree;
pub use summary::checkout_summary;
mod write;
#[cfg(test)]
use branch::parse_branches;
pub use branch::{BRANCH_LIMIT, Branch, Branches, branches};
#[cfg(test)]
use counts::parse_numstat;
pub use counts::{LineCount, line_counts};
pub use log::{Commit, History, log, show};
pub use worktree::{Worktree, add_worktree, remove_worktree};
use write::BUSY;
pub use write::{Backup, WriteOp, WriteOutcome, write, write_checked};
#[cfg(test)]
use write::{hunk_patch, network_hint};

#[cfg(test)]
mod tests;

pub use plait_protocol::git::recorded_diff;
