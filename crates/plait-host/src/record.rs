//! `plait-host record`, `undo` and `prune`: the runtime's helpers for the
//! host's own records of a checkout. Each reads one JSON request on stdin and
//! writes one JSON reply on stdout.
//!
//! On macOS the work runs in a second copy of this program under
//! `sandbox-exec`. Recording denies writes to the checkout; undo permits working
//! files. Both deny Git-directory writes, including timestamp changes.
//! Elsewhere the helpers run directly.

use plait_engine::git::{
    record::{self, Located, PruneRequest, Request},
    undo,
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

/// Requests are small; anything larger is not from the runtime.
const REQUEST_LIMIT: u64 = 1 << 20;

fn read_request() -> io::Result<Value> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(REQUEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > REQUEST_LIMIT {
        return Err(io::Error::other("record request too large"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn reply(value: Value) -> io::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &value)?;
    out.write_all(b"\n")
}

fn failure(message: String) -> io::Error {
    io::Error::other(message)
}

pub fn run() -> io::Result<()> {
    supervise("record-worker", record::BUDGET)
}

/// The outer helper keeps stdin open as the worker's lifetime lease. The
/// worker and every descendant share a group, including discovery commands.
pub(crate) fn supervise(operation: &str, budget: Duration) -> io::Result<()> {
    let value = read_request().inspect_err(|_| crate::command::cleanup_receipt("clean\n"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command.arg(operation);
    supervise_command(command, value, budget)
}

fn supervise_command(mut command: Command, value: Value, budget: Duration) -> io::Result<()> {
    command.env_remove("PLAIT_CLEANUP_RECEIPT_FD");
    let mut child = command
        .process_group(0)
        .stdin(Stdio::piped())
        .spawn()
        .inspect_err(|_| crate::command::cleanup_receipt("clean\n"))?;
    crate::command::cleanup_receipt(&format!("group:{}\n", child.id()));
    let mut lifetime = child
        .stdin
        .take()
        .ok_or_else(|| failure("Worker stdin missing".into()))?;
    let input = serde_json::to_vec(&value)?;
    let written = lifetime
        .write_all(&input)
        .and_then(|()| lifetime.write_all(b"\n"));
    if let Err(error) = written {
        kill_group(child.id());
        let _ = wait_worker(child, Duration::ZERO);
        return Err(error);
    }
    let status = wait_worker(child, budget)?;
    drop(lifetime);
    if status.success() {
        Ok(())
    } else {
        Err(failure("Checkout helper failed.".into()))
    }
}

fn kill_group(pid: u32) {
    unsafe {
        libc::killpg(pid as i32, libc::SIGKILL);
    }
}

fn wait_worker(mut child: Child, budget: Duration) -> io::Result<ExitStatus> {
    let group = child.id();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait());
    });
    let result = receive.recv_timeout(budget);
    // Also reclaim descendants if their worker exited first.
    kill_group(group);
    if crate::command::wait_group_gone(group as i32, Duration::from_secs(2)) {
        crate::command::cleanup_receipt("clean\n");
    }
    match result {
        Ok(status) => status,
        Err(_) => {
            let _ = receive.recv_timeout(Duration::from_secs(1));
            Err(failure("Checkout helper exceeded its time budget.".into()))
        }
    }
}

pub fn worker(operation: &str) -> io::Result<()> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(REQUEST_LIMIT + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() as u64 > REQUEST_LIMIT {
        return Err(failure("record request too large".into()));
    }
    let value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let group = unsafe { libc::getpgrp() };
    std::thread::spawn(move || {
        loop {
            match io::stdin().read(&mut [0u8; 1]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                _ => {
                    unsafe {
                        libc::killpg(group, libc::SIGKILL);
                    }
                    break;
                }
            }
        }
    });
    let budget = if operation == "undo-worker" || operation == "review-worker" {
        undo::BUDGET
    } else {
        record::BUDGET
    };
    record::within(Instant::now() + budget, || match operation {
        "record-worker" => record_value(value),
        "prune-worker" => prune_value(value),
        "undo-worker" => undo_request(value),
        "review-worker" => super::review::worker(value),
        "worktree-worker" => super::worktree::worker(value),
        "worktree-git-worker" => super::worktree::git_worker(value),
        _ => Err(failure("Unknown checkout worker.".into())),
    })
}

fn record_value(value: Value) -> io::Result<()> {
    let request: Request = serde_json::from_value(value.clone()).map_err(io::Error::other)?;
    if !request.store.is_absolute() {
        return Err(failure("store must be absolute".into()));
    }
    let located = record::locate(&request.checkout).map_err(failure)?;
    let protected = record::protected_paths(&located);
    // Validate before creating anything: even mkdir or chmod would break the guarantee.
    let store = resolve_store(&request.store)?;
    record::sandbox_profile(&protected, &store).map_err(failure)?;
    let records = prepare_store(&store)?;
    contain(
        "record-contained",
        &json!({"request": request, "located": located, "undo": value["undo"] == true}),
        &protected,
        &records,
    )
}

pub fn run_contained() -> io::Result<()> {
    let value = read_request()?;
    let request: Request =
        serde_json::from_value(value["request"].clone()).map_err(io::Error::other)?;
    let located: Located =
        serde_json::from_value(value["located"].clone()).map_err(io::Error::other)?;
    let made = capture(&located, &request, value["undo"] == true)?;
    reply(serde_json::to_value(made)?)
}

fn capture(located: &Located, request: &Request, is_undo: bool) -> io::Result<record::Record> {
    if !is_undo {
        return record::record(located, request).map_err(failure);
    }
    // Recovery reads the last durable list of completed writes, never broadens
    // an interrupted undo to edits made by another process while it was down.
    let path = undo::journal(&request.store, &request.id).map_err(failure)?;
    let saved: undo::Outcome =
        serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
    record::record_undo(located, request, &saved.applied_paths).map_err(failure)
}

pub fn prune() -> io::Result<()> {
    supervise("prune-worker", record::BUDGET)
}

fn prune_value(value: Value) -> io::Result<()> {
    let request: PruneRequest = serde_json::from_value(value).map_err(io::Error::other)?;
    if !request.store.is_absolute() {
        return Err(failure("store must be absolute".into()));
    }
    let records = record::records_dir(&request.store);
    if !records.is_dir() {
        return reply(serde_json::to_value(record::Pruned::default())?);
    }
    let records = records.canonicalize()?;
    // Every checkout the store reads from: pruning reads their objects too.
    let sources = record::sources(&request.store);
    let mut protected = Vec::new();
    for source in &sources {
        for path in [&source.root, &source.git_dir, &source.common_dir] {
            if let Ok(path) = path.canonicalize()
                && !path.starts_with(&records)
            {
                protected.push(path);
            }
        }
    }
    let protected: Vec<&Path> = protected.iter().map(|p| p.as_path()).collect();
    contain(
        "prune-contained",
        &serde_json::to_value(&request).map_err(io::Error::other)?,
        &protected,
        &records,
    )
}

pub fn prune_contained() -> io::Result<()> {
    let request: PruneRequest =
        serde_json::from_value(read_request()?).map_err(io::Error::other)?;
    reply(serde_json::to_value(
        record::prune(&request).map_err(failure)?,
    )?)
}

/// Resolve missing final directories without creating them inside a protected tree.
fn resolve_store(path: &Path) -> io::Result<std::path::PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(error)?;
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("invalid store path"))?;
            Ok(resolve_store(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
}

/// The records directory, created private and resolved, so the sandbox
/// profile names the path the kernel sees.
fn prepare_store(store: &Path) -> io::Result<std::path::PathBuf> {
    let records = record::records_dir(store);
    std::fs::create_dir_all(&records)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&records, std::fs::Permissions::from_mode(0o700))?;
    records.canonicalize()
}

/// Runs `operation` in a copy of this program that cannot write `protected`.
#[cfg(target_os = "macos")]
fn contain(operation: &str, input: &Value, protected: &[&Path], records: &Path) -> io::Result<()> {
    use std::process::Stdio;
    let mut profile = record::sandbox_profile(protected, records).map_err(failure)?;
    if operation == "undo-contained" {
        let located: Located =
            serde_json::from_value(input["located"].clone()).map_err(io::Error::other)?;
        profile = undo_profile(&located, records)?;
    }
    let mut child = crate::command::with_profile(&profile)
        .arg(std::env::current_exe()?)
        .arg(operation)
        // Resolved here once, so the contained copy does not look it up again.
        .env("PLAIT_GIT", record::git_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("recorder stdin missing"))?;
    stdin.write_all(&serde_json::to_vec(input)?)?;
    drop(stdin);
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        // The contained copy already said why on stderr.
        std::process::exit(status.code().unwrap_or(1))
    }
}

/// Without a sandbox, the recorder's own design is the only guard: commands
/// that write objects never see the person's object directory.
#[cfg(not(target_os = "macos"))]
fn contain(operation: &str, input: &Value, _: &[&Path], _: &Path) -> io::Result<()> {
    match operation {
        "record-contained" => {
            let request: Request =
                serde_json::from_value(input["request"].clone()).map_err(io::Error::other)?;
            let located: Located =
                serde_json::from_value(input["located"].clone()).map_err(io::Error::other)?;
            reply(serde_json::to_value(capture(
                &located,
                &request,
                input["undo"] == true,
            )?)?)
        }
        "undo-contained" => undo_value(input.clone()),
        _ => {
            let request: PruneRequest =
                serde_json::from_value(input.clone()).map_err(io::Error::other)?;
            reply(serde_json::to_value(
                record::prune(&request).map_err(failure)?,
            )?)
        }
    }
}

/// Undo has the same Git protection as recording, but can replace working files.
#[cfg(target_os = "macos")]
fn undo_profile(located: &Located, records: &Path) -> io::Result<String> {
    let quote =
        |path: &Path| serde_json::to_string(&path.to_string_lossy()).map_err(io::Error::other);
    let mut profile = format!(
        "(version 1) (allow default) (deny network*) (deny file-write*) (allow file-write* (literal \"/dev/null\") (subpath {}) (subpath {}))",
        quote(records)?,
        quote(&located.root)?
    );
    // An explicit deny wins over the working-tree exception, including worktrees.
    for path in [&located.git_dir, &located.common_dir]
        .into_iter()
        .chain(located.alternates.iter())
    {
        profile.push_str(&format!(" (deny file-write* (subpath {}))", quote(path)?));
    }
    profile.push_str(&format!(
        " (deny file-write* (subpath {}))",
        quote(&located.root.join(".git"))?
    ));
    Ok(profile)
}

pub fn undo() -> io::Result<()> {
    supervise("undo-worker", undo::BUDGET)
}

fn undo_request(value: Value) -> io::Result<()> {
    let request: undo::Request = serde_json::from_value(value.clone()).map_err(io::Error::other)?;
    if !request.store.is_absolute() {
        return Err(failure("store must be absolute".into()));
    }
    let located = undo::locate(&request.checkout).map_err(failure)?;
    let expected = plait_engine::lease::DirIdentity {
        dev: request
            .identity
            .dev
            .parse()
            .map_err(|_| failure("Checkout identity changed".into()))?,
        ino: request
            .identity
            .ino
            .parse()
            .map_err(|_| failure("Checkout identity changed".into()))?,
    };
    let _lease = crate::lease::acquire_or_verify_exclusive(&located.root, expected)?;
    let store = resolve_store(&request.store)?;
    record::sandbox_profile(&record::protected_paths(&located), &store).map_err(failure)?;
    let records = prepare_store(&store)?;
    contain(
        "undo-contained",
        &json!({"request": request, "located": located, "paths_file": value["paths_file"] == true}),
        &record::protected_paths(&located),
        &records,
    )
}
fn undo_value(value: Value) -> io::Result<()> {
    let mut request: undo::Request =
        serde_json::from_value(value["request"].clone()).map_err(io::Error::other)?;
    if value["paths_file"] == true {
        let journal = undo::journal(&request.store, &request.id).map_err(failure)?;
        let path = journal.with_extension("paths");
        if std::fs::metadata(&path)?.len() > 256 * 1024 * 1024 {
            return Err(failure("Undo path list too large".into()));
        }
        let source: Value =
            serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
        request.paths =
            Some(serde_json::from_value(source["paths"].clone()).map_err(io::Error::other)?);
        request.not_covered =
            Some(serde_json::from_value(source["not_covered"].clone()).map_err(io::Error::other)?);
    }

    let located: Located =
        serde_json::from_value(value["located"].clone()).map_err(io::Error::other)?;
    reply(serde_json::to_value(
        undo::undo(&located, &request).map_err(failure)?,
    )?)
}
pub fn undo_contained() -> io::Result<()> {
    undo_value(read_request()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helper_deadline_stops_the_entire_git_group() {
        use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};
        let scratch =
            std::env::temp_dir().join(format!("plait-helper-budget-{}", std::process::id()));
        std::fs::create_dir(&scratch).unwrap();
        let lock = std::fs::File::open(&scratch).unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let wrapper = scratch.join("git");
        let marker = scratch.join("pid");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nsleep 30 &\nwait\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            // The shell reads the script rather than executing the file just
            // written: a concurrent test's fork can still hold it open for
            // writing, and exec would then fail with ETXTBSY.
            .args(["-c", "/bin/sh \"$PLAIT_GIT\" --version"])
            .env("PLAIT_GIT", &wrapper)
            .stdout(Stdio::null());
        let fd = lock.as_raw_fd();
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(fd, 4) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::fcntl(4, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let start = Instant::now();
        // Long enough for the injected Git to start on a busy machine; the
        // wrapper itself would run for thirty seconds.
        let result = supervise_command(command, json!({}), Duration::from_millis(1500));
        assert!(
            matches!(&result, Err(error) if error.to_string().contains("time budget")),
            "unexpected result: {result:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(marker.exists(), "injected Git was not started");
        drop(lock);
        let fresh = std::fs::File::open(&scratch).unwrap();
        while unsafe { libc::flock(fresh.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "deadline leaked a descendant lock"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
