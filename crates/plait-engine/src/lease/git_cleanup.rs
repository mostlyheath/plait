//! Crash records for desktop Git and lent worktree children. A process crash
//! preserves writes already made here; this path deliberately does not fsync.
use super::{
    cleanup::{Cleanup, CleanupProcess},
    process, *,
};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    io::{Read, Seek},
    sync::Arc,
};

const LEN: usize = 168;
const MAGIC: &[u8; 8] = b"PLAITG01";
struct Scope {
    file: File,
    owner: process::Process,
    boot: u64,
}
thread_local! { static CURRENT: RefCell<Option<Arc<Scope>>> = const { RefCell::new(None) }; }

fn file(id: DirIdentity) -> Result<File, LeaseError> {
    let (_, dir) = open_lease_dir(&default_lease_dir()?)?;
    open_lock_file(
        &dir,
        &format!("git-cleanup-{:016x}-{:016x}.lock", id.dev, id.ino),
    )
    .map(|(file, _)| file)
}
fn lock(file: &File) -> Result<(), LeaseError> {
    if flock_fd_wait(file.as_raw_fd(), libc::LOCK_EX, Duration::from_secs(5))? {
        Ok(())
    } else {
        Err(LeaseError::io(
            "Git cleanup state is being checked. Try again.",
        ))
    }
}
fn unlock(file: &File) {
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}
fn bytes(file: &mut File) -> Result<Vec<u8>, LeaseError> {
    file.rewind().map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take((LEN + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    Ok(bytes)
}
fn io_error(error: io::Error) -> LeaseError {
    LeaseError::io(error.to_string())
}
fn token(bytes: &[u8]) -> String {
    format!("git-{:x}", Sha256::digest(bytes))
}
fn put(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn get(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// The enclosing operation keeps its existing lease. Read-only Git commands
/// inside it are not recorded; only write::finish installs the child hook.
pub fn with_lease<T>(
    lease: &CheckoutLease,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let scope = Arc::new(Scope {
        file: file(lease.identity).map_err(String::from)?,
        owner: process::inspect(std::process::id() as i32).map_err(|e| e.to_string())?,
        boot: process::boot().map_err(|e| e.to_string())?,
    });
    struct Restore(Option<Arc<Scope>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with(|slot| slot.replace(Some(scope))));
    work()
}

/// Dropped without `finish` (an early return or a panic while Git may run),
/// the record is marked unconfirmed, never left hidden behind a live desktop.
pub(crate) struct Running {
    scope: Arc<Scope>,
    finished: std::cell::Cell<bool>,
    group: std::cell::Cell<i32>,
}
fn save(file: &File, bytes: &[u8], offset: i64) -> Result<(), String> {
    lock(file).map_err(String::from)?;
    let wrote =
        unsafe { libc::pwrite(file.as_raw_fd(), bytes.as_ptr().cast(), bytes.len(), offset) };
    let error = io::Error::last_os_error();
    unlock(file);
    if wrote == bytes.len() as isize {
        Ok(())
    } else {
        Err(format!("Git cleanup could not be saved: {error}"))
    }
}
/// Saved before spawn, so a crash before the child is known still holds the
/// checkout. No `pre_exec`: that would fork the whole desktop for every write.
pub(crate) fn prepare(label: &str) -> Result<Option<Running>, String> {
    CURRENT.with(|slot| {
        let Some(scope) = slot.borrow().clone() else {
            return Ok(None);
        };
        let mut record = [0u8; LEN];
        record[..8].copy_from_slice(MAGIC);
        put(&mut record, 8, scope.boot);
        put(&mut record, 16, scope.owner.pid as u64);
        put(&mut record, 24, scope.owner.birth[0]);
        put(&mut record, 32, scope.owner.birth[1]);
        let label = label.as_bytes();
        let size = label.len().min(95);
        record[64..64 + size].copy_from_slice(&label[..size]);
        save(&scope.file, &record, 0)?;
        Ok(Some(Running {
            scope,
            finished: Default::default(),
            group: Default::default(),
        }))
    })
}
impl Running {
    /// The child is not reaped yet, so its PID cannot have been reused. A
    /// failed save leaves the starting record, which also holds the checkout.
    /// Git leads its own group (`process_group(0)`). A child that already
    /// exited may be unreadable (a zombie on macOS): its group is still known,
    /// but with no birth identity it is never offered a signal.
    pub(crate) fn started(&self, pid: u32) {
        let pid = pid as i32;
        let birth = match process::inspect(pid) {
            Ok(child) if child.group != pid => return,
            Ok(child) => child.birth,
            Err(_) => [0, 0],
        };
        self.group.set(pid);
        let mut fields = [0u8; 24];
        put(&mut fields, 0, pid as u64);
        put(&mut fields, 8, birth[0]);
        put(&mut fields, 16, birth[1]);
        let _ = save(&self.scope.file, &fields, 40);
    }
    /// Git's exit acknowledges its own work, not that of a hook job or helper
    /// still in its group; those keep the record until the group is gone.
    fn quiet(&self) -> bool {
        let group = self.group.get();
        group == 0 || wait_gone_within(group, Duration::from_millis(100))
    }
    pub(crate) fn finish(&self, acknowledged: bool) -> Result<(), String> {
        let acknowledged = acknowledged && self.quiet();
        lock(&self.scope.file).map_err(String::from)?;
        let fd = self.scope.file.as_raw_fd();
        let result = unsafe {
            if acknowledged {
                libc::ftruncate(fd, 0)
            } else {
                // Exceptional failure: do not hide the marker behind a live desktop.
                let uncertain = 1u64.to_le_bytes();
                if libc::pwrite(fd, uncertain.as_ptr().cast(), 8, 160) == 8 {
                    0
                } else {
                    -1
                }
            }
        };
        let error = io::Error::last_os_error();
        unlock(&self.scope.file);
        if result == 0 {
            self.finished.set(true);
            Ok(())
        } else {
            Err(format!("Git cleanup could not be saved: {error}"))
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.finished.get() {
            let _ = self.finish(false);
        }
    }
}

struct Record {
    owner: process::Process,
    /// None while starting: the desktop ended before the child was known.
    child: Option<process::Process>,
    boot: u64,
    command: String,
    uncertain: bool,
}
fn decode(bytes: &[u8]) -> Option<Record> {
    if bytes.len() != LEN || bytes.get(..8)? != MAGIC {
        return None;
    }
    let owner = i32::try_from(get(bytes, 16)).ok()?;
    let child = i32::try_from(get(bytes, 40)).ok()?;
    if owner <= 0 || child < 0 {
        return None;
    }
    let label = &bytes[64..160];
    let end = label.iter().position(|byte| *byte == 0)?;
    let command = std::str::from_utf8(&label[..end]).ok()?.to_owned();
    if command.is_empty() || command.chars().any(char::is_control) {
        return None;
    }
    Some(Record {
        owner: process::Process {
            pid: owner,
            group: 0,
            birth: [get(bytes, 24), get(bytes, 32)],
        },
        child: (child > 0).then(|| process::Process {
            pid: child,
            group: child,
            birth: [get(bytes, 48), get(bytes, 56)],
        }),
        boot: get(bytes, 8),
        command,
        uncertain: get(bytes, 160) != 0,
    })
}
fn same(expected: process::Process) -> bool {
    process::inspect(expected.pid).is_ok_and(|now| now.birth == expected.birth)
}
fn leader(child: process::Process) -> bool {
    process::inspect(child.pid).is_ok_and(|now| now.birth == child.birth && now.group == child.pid)
}
fn live_owner(record: &Record) -> bool {
    !record.uncertain && same(record.owner)
}
fn describe(bytes: &[u8], record: Option<&Record>) -> Cleanup {
    match record {
        Some(Record {
            child: Some(child),
            command,
            ..
        }) => Cleanup {
            token: token(bytes),
            reason: format!(
                "Plait could not confirm {command} stopped. Process group {} may still use this checkout.",
                child.pid
            ),
            process: Some(CleanupProcess {
                group: child.pid,
                command: command.clone(),
                can_stop: leader(*child),
            }),
        },
        Some(Record { command, .. }) => Cleanup {
            token: token(bytes),
            reason: format!(
                "Plait ended while starting {command}, so its process group is unknown. It may still use this checkout."
            ),
            process: None,
        },
        None => Cleanup {
            token: token(bytes),
            reason: "Plait could not read its saved Git cleanup state.".into(),
            process: None,
        },
    }
}

pub fn unconfirmed(id: DirIdentity) -> Result<Option<Cleanup>, LeaseError> {
    let mut file = file(id)?;
    lock(&file)?;
    let bytes = bytes(&mut file)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let record = decode(&bytes);
    if let Some(record) = &record {
        let same_boot = !process::other_boot(record.boot).map_err(io_error)?;
        if same_boot && live_owner(record) {
            return Ok(None);
        }
        if !same_boot
            || record
                .child
                .is_some_and(|child| process::group_gone(child.pid))
        {
            file.set_len(0).map_err(io_error)?;
            return Ok(None);
        }
    }
    Ok(Some(describe(&bytes, record.as_ref())))
}

pub fn resolve(id: DirIdentity, expected: &str) -> Result<(), LeaseError> {
    let mut file = file(id)?;
    lock(&file)?;
    let bytes = bytes(&mut file)?;
    if bytes.is_empty() || token(&bytes) != expected {
        return Err(LeaseError::io(
            "Git cleanup state changed. Refresh before confirming.",
        ));
    }
    file.set_len(0).map_err(io_error)
}

fn wait_gone(group: i32) -> bool {
    wait_gone_within(group, Duration::from_secs(2))
}
fn wait_gone_within(group: i32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if process::group_gone(group) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
/// Never needs the checkout lease: a surviving lent child can still hold it.
/// The marker token, boot and leader birth identity gate each group signal.
pub fn stop(id: DirIdentity, expected: &str) -> Result<(), LeaseError> {
    let mut file = file(id)?;
    lock(&file)?;
    let bytes = bytes(&mut file)?;
    if bytes.is_empty() || token(&bytes) != expected {
        return Err(LeaseError::io(
            "Git cleanup state changed. Refresh before stopping.",
        ));
    }
    let unverified =
        || LeaseError::io("The Git process identity could not be verified. No signal was sent.");
    let record = decode(&bytes).ok_or_else(unverified)?;
    if process::other_boot(record.boot).map_err(io_error)? {
        return file.set_len(0).map_err(io_error);
    }
    let child = record.child.ok_or_else(unverified)?;
    if process::group_gone(child.pid) {
        return file.set_len(0).map_err(io_error);
    }
    if live_owner(&record) {
        return Err(LeaseError::io(
            "The desktop still owns this Git operation. Wait for it to finish.",
        ));
    }
    let mut verified = false;
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        // After a verified SIGTERM the group was watched without a gap, and a
        // live group's ID is never reallocated: a leader that exited since
        // still names the same group. Any process now at its PID does not.
        if !leader(child) && !(verified && process::pid_free(child.pid)) {
            return Err(LeaseError::io(
                "The recorded Git leader no longer has the same identity. No further signal was sent; inspect the remaining processes before confirming.",
            ));
        }
        if unsafe { libc::kill(-child.pid, signal) } != 0 && !process::group_gone(child.pid) {
            return Err(io_error(io::Error::last_os_error()));
        }
        verified = true;
        if wait_gone(child.pid) {
            return file.set_len(0).map_err(io_error);
        }
    }
    Err(LeaseError::io(
        "Stop unconfirmed. The recorded Git process group still exists; the checkout remains blocked.",
    ))
}

pub fn require_clear(id: DirIdentity) -> Result<(), LeaseError> {
    if let Some(state) = unconfirmed(id)? {
        return Err(LeaseError::new(
            LeaseErrorKind::CleanupUncertain,
            state.message(),
        ));
    }
    Ok(())
}

pub(super) fn check_lease(
    root: &Path,
    result: Result<CheckoutLease, LeaseError>,
) -> Result<CheckoutLease, LeaseError> {
    match result {
        Ok(lease) => {
            require_clear(lease.identity())?;
            Ok(lease)
        }
        Err(error) => {
            // A lent child may own the very lock needed for admission. Explain
            // its saved state even though that lock could not be acquired. A
            // record that cannot be read explains nothing: keep the refusal.
            if let Ok((_, _, id)) = open_verified_dir(root)
                && let Err(held) = require_clear(id)
                && held.kind == LeaseErrorKind::CleanupUncertain
            {
                return Err(held);
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, Write},
        process::{Child, Stdio},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "plait-git-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn pending(root: &Path, script: &str) -> (Child, DirIdentity) {
        let lease = acquire_checkout(root, None, LeaseClass::Exclusive).unwrap();
        let id = lease.identity();
        let (child, running) = with_lease(&lease, || {
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", script])
                .process_group(0)
                .stdout(Stdio::piped());
            let running = prepare("git commit")?.unwrap();
            let child = command.spawn().map_err(|e| e.to_string())?;
            running.started(child.id());
            Ok((child, running))
        })
        .unwrap();
        running.finish(false).unwrap();
        drop(lease);
        (child, id)
    }
    #[test]
    fn verified_git_stop_escalates_and_clears_only_after_group_exit() {
        let root = root();
        let (mut child, id) = pending(&root, "trap '' TERM; echo ready; exec sleep 30");
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let state = unconfirmed(id).unwrap().unwrap();
        assert!(state.process.as_ref().unwrap().can_stop);
        assert!(stop(id, "stale").is_err());
        assert!(child.try_wait().unwrap().is_none());
        let wait = thread::spawn(move || child.wait().unwrap());
        stop(id, &state.token).unwrap();
        assert!(!wait.join().unwrap().success());
        assert!(unconfirmed(id).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mismatched_git_birth_identity_sends_nothing_and_late_absence_resolves() {
        let root = root();
        let (mut child, id) = pending(&root, "exec sleep 30");
        let mut saved = file(id).unwrap();
        let mut raw = bytes(&mut saved).unwrap();
        let birth = get(&raw, 48);
        put(&mut raw, 48, birth.wrapping_add(1));
        saved.rewind().unwrap();
        saved.write_all(&raw).unwrap();
        drop(saved);
        let state = unconfirmed(id).unwrap().unwrap();
        assert!(!state.process.as_ref().unwrap().can_stop);
        assert!(stop(id, &state.token).is_err());
        assert!(child.try_wait().unwrap().is_none());
        assert_eq!(
            acquire_checkout(&root, None, LeaseClass::Writer)
                .unwrap_err()
                .kind,
            LeaseErrorKind::CleanupUncertain
        );
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        drop(acquire_checkout(&other, None, LeaseClass::Writer).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(unconfirmed(id).unwrap().is_none());
        drop(acquire_checkout(&root, None, LeaseClass::Writer).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn normal_git_acknowledgement_clears_without_an_extra_process() {
        let root = root();
        let lease = acquire_checkout(&root, None, LeaseClass::Exclusive).unwrap();
        with_lease(&lease, || {
            let mut command = Command::new("/usr/bin/true");
            command.process_group(0);
            let running = prepare("git add")?.unwrap();
            let mut child = command.spawn().unwrap();
            running.started(child.id());
            assert!(unconfirmed(lease.identity()).unwrap().is_none());
            assert!(child.wait().unwrap().success());
            running.finish(true)?;
            Ok(())
        })
        .unwrap();
        assert!(unconfirmed(lease.identity()).unwrap().is_none());
        drop(lease);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unreadable_git_record_requires_current_explicit_confirmation() {
        let root = root();
        let lease = acquire_for_cleanup(&root).unwrap();
        let id = lease.identity();
        file(id).unwrap().write_all(b"torn record").unwrap();
        let state = unconfirmed(id).unwrap().unwrap();
        assert!(state.process.is_none());
        assert!(stop(id, &state.token).is_err());
        assert!(resolve(id, "stale").is_err());
        assert!(unconfirmed(id).unwrap().is_some());
        resolve(id, &state.token).unwrap();
        assert!(unconfirmed(id).unwrap().is_none());
        drop(lease);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn crashed_desktop_fixture() {
        let Some(root) = std::env::var_os("PLAIT_GIT_CRASH_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root);
        if std::env::var_os("PLAIT_GIT_LENT_FIXTURE").is_some() {
            let lease = acquire_checkout(&root, None, LeaseClass::Exclusive).unwrap();
            with_lease(&lease, || {
                let mut command = Command::new("/bin/sh");
                command
                    .args(["-c", "echo ready > ready; exec sleep 30"])
                    .current_dir(&root)
                    .process_group(0);
                lease.lend_to(&mut command);
                let running = prepare("git worktree remove")?.unwrap();
                let mut child = command.spawn().unwrap();
                running.started(child.id());
                let status = child.wait().unwrap();
                running.finish(true)?;
                assert!(status.success());
                Ok(())
            })
            .unwrap();
        } else {
            crate::git::write(
                &root,
                &crate::git::WriteOp::Commit {
                    message: "fixture".into(),
                },
            )
            .unwrap();
        }
    }
    #[test]
    fn dying_before_spawn_fixture() {
        let Some(root) = std::env::var_os("PLAIT_GIT_START_FIXTURE") else {
            return;
        };
        let lease = acquire_checkout(Path::new(&root), None, LeaseClass::Exclusive).unwrap();
        with_lease(&lease, || -> Result<(), String> {
            let _running = prepare("git commit")?;
            // The desktop dies after saving the record, before Git is known.
            std::process::abort()
        })
        .unwrap();
    }
    #[test]
    fn a_desktop_that_dies_before_its_git_child_is_known_still_holds_the_checkout() {
        let root = root();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lease::git_cleanup::tests::dying_before_spawn_fixture",
            ])
            .env("PLAIT_GIT_START_FIXTURE", &root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        let id = open_verified_dir(&root).unwrap().2;
        let state = unconfirmed(id)
            .unwrap()
            .expect("the start gap must hold the checkout");
        assert!(
            state.process.is_none(),
            "an unknown group is never offered a signal"
        );
        assert!(
            state.reason.contains("process group is unknown"),
            "{}",
            state.reason
        );
        assert!(stop(id, &state.token).is_err());
        assert_eq!(
            acquire_checkout(&root, None, LeaseClass::Writer)
                .unwrap_err()
                .kind,
            LeaseErrorKind::CleanupUncertain
        );
        resolve(id, &state.token).unwrap();
        drop(acquire_checkout(&root, None, LeaseClass::Writer).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn an_unfinished_git_write_is_not_hidden_behind_a_live_desktop() {
        let root = root();
        let lease = acquire_checkout(&root, None, LeaseClass::Exclusive).unwrap();
        let id = lease.identity();
        let mut child = with_lease(&lease, || {
            let mut command = Command::new("/bin/sleep");
            command.arg("30").process_group(0);
            let running = prepare("git commit")?.unwrap();
            let child = command.spawn().map_err(|e| e.to_string())?;
            running.started(child.id());
            // An early return or unwind skips finish while Git still runs.
            drop(running);
            Ok(child)
        })
        .unwrap();
        drop(lease);
        let state = unconfirmed(id).unwrap();
        let admission = acquire_checkout(&root, None, LeaseClass::Writer).map(drop);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            state.is_some(),
            "a live desktop hid its unfinished Git write"
        );
        assert_eq!(
            admission.unwrap_err().kind,
            LeaseErrorKind::CleanupUncertain
        );
        assert!(unconfirmed(id).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_hook_job_left_in_the_git_group_holds_the_checkout_until_it_exits() {
        use std::os::unix::fs::PermissionsExt;
        let root = root();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        fs::write(root.join("a.txt"), "fixture").unwrap();
        git(&["add", "a.txt"]);
        let hook = root.join(".git/hooks/post-commit");
        // A background job with its output redirected: Git exits normally.
        fs::write(
            &hook,
            "#!/bin/sh\n(sleep 5; echo late > late.txt) >/dev/null 2>&1 &\n",
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        crate::git::write(
            &root,
            &crate::git::WriteOp::Commit {
                message: "m".into(),
            },
        )
        .unwrap();
        let id = open_verified_dir(&root).unwrap().2;
        let state = unconfirmed(id).unwrap();
        let admission = acquire_checkout(&root, None, LeaseClass::Writer).map(drop);
        // The record names the group; without one the job ends on its own.
        if let Some(group) = state.as_ref().and_then(|state| state.process.as_ref()) {
            unsafe {
                libc::kill(-group.group, libc::SIGKILL);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while unconfirmed(id).unwrap().is_some() {
            assert!(
                Instant::now() < deadline,
                "group exit never cleared the record"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let state = state.expect("a live member of the Git group was released");
        assert!(state.reason.contains("git commit"), "{}", state.reason);
        assert_eq!(
            admission.unwrap_err().kind,
            LeaseErrorKind::CleanupUncertain
        );
        drop(acquire_checkout(&root, None, LeaseClass::Writer).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_escalates_when_sigterm_ends_git_but_its_group_survives() {
        let root = root();
        // The leader dies on SIGTERM; its child ignores TERM and stays in the group.
        let (mut child, id) = pending(&root, "(trap '' TERM; echo ready; exec sleep 30) & wait");
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let state = unconfirmed(id).unwrap().unwrap();
        let group = state.process.as_ref().unwrap().group;
        let wait = thread::spawn(move || child.wait().unwrap());
        let stopped = stop(id, &state.token);
        let alive = !process::group_gone(group);
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        wait.join().unwrap();
        assert!(stopped.is_ok(), "{stopped:?}");
        assert!(!alive, "Stop left the recorded group running");
        assert!(unconfirmed(id).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
    fn crash_case(lent: bool) {
        use std::os::unix::fs::PermissionsExt;
        let root = root();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .output()
                    .unwrap()
                    .status
                    .success()
            )
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        fs::write(root.join("a.txt"), "fixture").unwrap();
        git(&["add", "a.txt"]);
        fs::write(
            root.join(".git/hooks/pre-commit"),
            "#!/bin/sh\necho ready > ready\nsleep 30\n",
        )
        .unwrap();
        fs::set_permissions(
            root.join(".git/hooks/pre-commit"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "lease::git_cleanup::tests::crashed_desktop_fixture",
                "--nocapture",
            ])
            .env("PLAIT_GIT_CRASH_FIXTURE", &root)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if lent {
            command.env("PLAIT_GIT_LENT_FIXTURE", "1");
        }
        let mut desktop = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("ready").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        desktop.kill().unwrap();
        desktop.wait().unwrap();
        let id = open_verified_dir(&root).unwrap().2;
        let state = unconfirmed(id).unwrap().unwrap();
        assert!(state.process.as_ref().unwrap().can_stop);
        assert_eq!(
            acquire_checkout(&root, None, LeaseClass::Writer)
                .unwrap_err()
                .kind,
            LeaseErrorKind::CleanupUncertain
        );
        assert!(
            crate::git::write(&root, &crate::git::WriteOp::StageAll)
                .unwrap_err()
                .contains("Process group")
        );
        stop(id, &state.token).unwrap();
        assert!(unconfirmed(id).unwrap().is_none());
        drop(acquire_checkout(&root, None, LeaseClass::Writer).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn desktop_crash_does_not_admit_a_run_beside_its_surviving_git_writer() {
        crash_case(false);
    }
    #[test]
    fn a_lent_child_can_be_stopped_without_acquiring_the_lease_it_still_holds() {
        crash_case(true);
    }
}
