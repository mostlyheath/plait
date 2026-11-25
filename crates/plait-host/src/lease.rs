use plait_engine::lease::{
    CHECKOUT_BUSY, COMMON_METADATA_BUSY, DirIdentity, LIFECYCLE_BUSY, LeaseError, LeaseErrorKind,
    MAX_COMMON_METADATA_WAIT, WRITER_BUSY, checkout_lock_name, common_lock_name, default_lease_dir,
    flock_fd_wait, open_lease_dir, open_lock_file, open_verified_dir, prepare_checkout_lock,
    prepare_common_lock, try_flock_fd, verify_dir_still,
};
use plait_engine::lease::{acquire_for_cleanup, cleanup, git_cleanup};
use serde_json::json;
use std::{
    fs::File,
    io::{self, Read, Write},
    mem::MaybeUninit,
    os::fd::AsRawFd,
    path::Path,
    time::Duration,
};

/// Prepare and verify a lock file path for the runtime sidecar (`checkout` or `common`).
pub fn prepare(args: Vec<String>) -> io::Result<()> {
    if args.len() != 2 {
        return Err(io::Error::other(
            "lease-prepare expects <checkout|common> <path>",
        ));
    }
    let prepared = match args[0].as_str() {
        "checkout" => prepare_checkout_lock(Path::new(&args[1])),
        "common" => prepare_common_lock(Path::new(&args[1])),
        _ => return Err(io::Error::other("unknown lease-prepare kind")),
    }
    .map_err(|error| io::Error::other(error.to_string()))?;
    let mut out = io::stdout().lock();
    serde_json::to_writer(
        &mut out,
        &json!({
            "root": prepared.root,
            "dev": prepared.identity.dev.to_string(),
            "ino": prepared.identity.ino.to_string(),
            "lock_path": prepared.lock_path,
            "lock_dev": prepared.lock_identity.dev.to_string(),
            "lock_ino": prepared.lock_identity.ino.to_string(),
        }),
    )?;
    out.write_all(b"\n")
}

/// Acquire `flock` on open file descriptions inherited from the runtime sidecar.
///
/// With no arguments, preserves the legacy single-fd writer lock on fd 3.
/// With `<reader|writer|exclusive|common> <canonical_path> [timeout_ms]`,
/// verifies every inherited descriptor and acquires locks in strict Level 1 -> Level 2 order.
/// A writer or exclusive lease may pass `owner` third to lock runtime-only fd 5.
pub fn acquire(args: Vec<String>) -> io::Result<()> {
    if args.is_empty() {
        return acquire_legacy();
    }
    if args.len() < 2 || args.len() > 3 {
        return Err(io::Error::other(
            "lock-fd expects <reader|writer|exclusive|common> <path> [timeout_ms]",
        ));
    }
    let mode = args[0].as_str();
    let target = Path::new(&args[1]);
    let owner_fd =
        matches!(mode, "writer" | "exclusive") && args.get(2).is_some_and(|arg| arg == "owner");
    let timeout_ms = args
        .get(2)
        .filter(|_| !owner_fd)
        .map(|raw| {
            raw.parse::<u64>()
                .map_err(|_| io::Error::other("invalid lease timeout"))
        })
        .transpose()?
        .unwrap_or(0);

    let (canonical, _dir, id) =
        open_verified_dir(target).map_err(|error| io::Error::other(error.to_string()))?;
    let lease_dir = default_lease_dir().map_err(|error| io::Error::other(error.to_string()))?;
    let (_lease_dir_path, lease_dir_fd) =
        open_lease_dir(&lease_dir).map_err(|error| io::Error::other(error.to_string()))?;
    let expected_name = if mode == "common" {
        common_lock_name(id)
    } else {
        checkout_lock_name(id)
    };
    let (_expected_file, expected_lock_id) = open_lock_file(&lease_dir_fd, &expected_name)
        .map_err(|error| io::Error::other(error.to_string()))?;
    verify_inherited_lock_fd(3, expected_lock_id)?;
    if matches!(mode, "writer" | "exclusive") {
        verify_inherited_dir_fd(4, id)?;
        plait_engine::lease::seed::yield_to_run(&canonical).map_err(io::Error::other)?;
    }
    if let Err(error) = lock_mode(mode, id, timeout_ms) {
        // A lent Git child may hold these locks: explain its saved state. A
        // record that cannot be read explains nothing: keep the refusal.
        if mode != "common"
            && let Err(held) = git_cleanup::require_clear(id)
            && held.kind == LeaseErrorKind::CleanupUncertain
        {
            return Err(admission_error(held));
        }
        return Err(error);
    }

    if let Err(error) = verify_dir_still(&canonical, id) {
        if matches!(mode, "writer" | "exclusive") {
            unsafe {
                libc::flock(4, libc::LOCK_UN);
            }
        }
        unsafe {
            libc::flock(3, libc::LOCK_UN);
        }
        return Err(io::Error::other(format!("identity: {error}")));
    }
    let clear = match mode {
        "common" => Ok(()),
        // Only with the locks held: a desktop that dies after this check can
        // no longer free its lease beside a surviving Git child.
        "reader" => git_cleanup::require_clear(id)
            .and_then(|()| cleanup::require_clear_for_reader(&lease_dir, id)),
        _ => git_cleanup::require_clear(id).and_then(|()| cleanup::require_clear(id)),
    };
    if let Err(error) = clear {
        // These descriptors belong to Node. Close in this helper alone would
        // leave its copies locked after a rejected admission.
        if mode != "reader" {
            unsafe {
                libc::flock(4, libc::LOCK_UN);
            }
        }
        unsafe {
            libc::flock(3, libc::LOCK_UN);
        }
        return Err(admission_error(error));
    }
    if owner_fd {
        let (_, owner_id) = open_lock_file(&lease_dir_fd, &cleanup::owner_lock_name(id))
            .map_err(io::Error::other)?;
        verify_inherited_lock_fd(5, owner_id)?;
        if !try_flock_fd(5, libc::LOCK_EX).map_err(io::Error::other)? {
            return Err(io::Error::other("busy: Cleanup ownership is still held."));
        }
    }
    Ok(())
}

/// Takes the mode's locks in Level 1 -> Level 2 order, releasing any on refusal.
fn lock_mode(mode: &str, id: DirIdentity, timeout_ms: u64) -> io::Result<()> {
    match mode {
        "reader" => {
            if !try_flock_fd(3, libc::LOCK_SH).map_err(|e| io::Error::other(e.to_string()))? {
                return Err(io::Error::other(format!("busy: {LIFECYCLE_BUSY}")));
            }
        }
        "writer" => {
            verify_inherited_dir_fd(4, id)?;
            if !try_flock_fd(3, libc::LOCK_SH).map_err(|e| io::Error::other(e.to_string()))? {
                return Err(io::Error::other(format!("busy: {LIFECYCLE_BUSY}")));
            }
            if !try_flock_fd(4, libc::LOCK_EX).map_err(|e| io::Error::other(e.to_string()))? {
                unsafe {
                    libc::flock(3, libc::LOCK_UN);
                }
                return Err(io::Error::other(format!("busy: {WRITER_BUSY}")));
            }
        }
        "exclusive" => {
            verify_inherited_dir_fd(4, id)?;
            if !try_flock_fd(3, libc::LOCK_EX).map_err(|e| io::Error::other(e.to_string()))? {
                return Err(io::Error::other(format!("busy: {CHECKOUT_BUSY}")));
            }
            if !try_flock_fd(4, libc::LOCK_EX).map_err(|e| io::Error::other(e.to_string()))? {
                unsafe {
                    libc::flock(3, libc::LOCK_UN);
                }
                return Err(io::Error::other(format!("busy: {CHECKOUT_BUSY}")));
            }
        }
        "common" => {
            let wait = Duration::from_millis(timeout_ms).min(MAX_COMMON_METADATA_WAIT);
            if !flock_fd_wait(3, libc::LOCK_EX, wait)
                .map_err(|e| io::Error::other(e.to_string()))?
            {
                return Err(io::Error::other(format!("busy: {COMMON_METADATA_BUSY}")));
            }
        }
        _ => return Err(io::Error::other("unknown lock-fd mode")),
    }
    Ok(())
}

/// Private sidecar helper; mutations either inherit the run's writer lease or
/// acquire exclusive recovery ownership. No caller may bypass OS contention.
pub fn cleanup_operation(args: Vec<String>) -> io::Result<()> {
    if args.len() != 2 {
        return Err(io::Error::other("cleanup expects operation checkout"));
    }
    let root = Path::new(&args[1]);
    let (canonical, _, id) = open_verified_dir(root).map_err(io::Error::other)?;
    let action = args[0].as_str();
    let mut bytes = Vec::new();
    io::stdin().take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(io::Error::other("cleanup input exceeds limit"));
    }
    let input: serde_json::Value = serde_json::from_slice(&bytes)?;
    let fields = input
        .as_object()
        .ok_or_else(|| io::Error::other("invalid cleanup input"))?;
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "token" | "reason"))
    {
        return Err(io::Error::other("unknown cleanup field"));
    }
    let token = input.get("token").and_then(serde_json::Value::as_str);
    let reason = input.get("reason").and_then(serde_json::Value::as_str);
    let recovery = if action == "resolve" {
        Some(acquire_for_cleanup(&canonical).map_err(io::Error::other)?)
    } else {
        None
    };
    if matches!(action, "begin" | "mark" | "finish") {
        verify_inherited_dir_fd(4, id)?;
        let (_, dir) = open_lease_dir(&default_lease_dir().map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        let (_, lock_id) =
            open_lock_file(&dir, &checkout_lock_name(id)).map_err(io::Error::other)?;
        verify_inherited_lock_fd(7, lock_id)?;
        if !try_flock_fd(4, libc::LOCK_EX).map_err(io::Error::other)? {
            return Err(io::Error::other(WRITER_BUSY));
        }
    }
    match action {
        "read" => {}
        "begin" | "mark" => {
            let token = token.ok_or_else(|| io::Error::other("cleanup token missing"))?;
            let next = cleanup::Cleanup {
                process: None,
                token: token.into(),
                reason: reason
                    .ok_or_else(|| io::Error::other("cleanup reason missing"))?
                    .into(),
            };
            cleanup::replace(id, (action == "mark").then_some(token), Some(&next))
                .map_err(io::Error::other)?;
        }
        "finish" | "resolve" => {
            let token = token.ok_or_else(|| io::Error::other("cleanup token missing"))?;
            if action == "resolve" && token.starts_with("git-") {
                git_cleanup::resolve(id, token).map_err(io::Error::other)?;
            } else {
                cleanup::replace(id, Some(token), None).map_err(io::Error::other)?;
            }
        }
        "stop" => git_cleanup::stop(
            id,
            token.ok_or_else(|| io::Error::other("cleanup token missing"))?,
        )
        .map_err(io::Error::other)?,
        _ => return Err(io::Error::other("unknown cleanup operation")),
    }
    let state = cleanup::unconfirmed(id).map_err(io::Error::other)?;
    serde_json::to_writer(
        io::stdout(),
        &json!({"checkout": canonical, "cleanup": state}),
    )?;
    drop(recovery);
    Ok(())
}

fn acquire_legacy() -> io::Result<()> {
    // flock follows the shared open description, which Node retains after we exit.
    let stat = fstat_fd(3)?;
    #[allow(clippy::unnecessary_cast)]
    let file_type = (stat.st_mode as u32) & (libc::S_IFMT as u32);
    #[allow(clippy::unnecessary_cast)]
    if file_type != (libc::S_IFDIR as u32) {
        return Err(io::Error::other("invalid checkout lease"));
    }
    if !try_flock_fd(3, libc::LOCK_EX).map_err(|e| io::Error::other(e.to_string()))? {
        return Err(io::Error::other("checkout already has a writer"));
    }
    #[allow(clippy::unnecessary_cast)]
    let id = DirIdentity {
        dev: stat.st_dev as u64,
        ino: stat.st_ino as u64,
    };
    if let Err(error) = cleanup::require_clear(id).and_then(|()| git_cleanup::require_clear(id)) {
        unsafe {
            libc::flock(3, libc::LOCK_UN);
        }
        return Err(admission_error(error));
    }
    Ok(())
}

/// The runtime holds a checkout for `cleanup_uncertain`, so only a saved
/// cleanup record that is unconfirmed is reported that way. Any other failure,
/// such as a record's lock file that cannot be opened, refuses this lease with
/// its own message.
fn admission_error(error: LeaseError) -> io::Error {
    if error.kind == LeaseErrorKind::CleanupUncertain {
        io::Error::other(format!("cleanup_uncertain: {error}"))
    } else {
        io::Error::other(error.message)
    }
}

/// Guard held by `plait-host undo` across working-tree restoration.
/// Releases only descriptors it opened itself; inherited runtime leases stay held by the runtime.
pub struct UndoLeaseGuard {
    owned_writer: Option<File>,
    owned_lifecycle: Option<File>,
}

impl Drop for UndoLeaseGuard {
    fn drop(&mut self) {
        if let Some(writer) = self.owned_writer.take() {
            unsafe {
                libc::flock(writer.as_raw_fd(), libc::LOCK_UN);
            }
        }
        if let Some(lifecycle) = self.owned_lifecycle.take() {
            unsafe {
                libc::flock(lifecycle.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// Enforce Level 1 (`lifecycle EX`) and Level 2 (`writer EX`) for `plait-host undo`.
/// Reuses inherited fd 7 (lifecycle) and fd 4 (writer) when provided by the runtime,
/// and acquires any missing lease standalone.
pub fn acquire_or_verify_exclusive(
    root: &Path,
    expected: DirIdentity,
) -> io::Result<UndoLeaseGuard> {
    let (canonical, dir_file, actual) =
        open_verified_dir(root).map_err(|_| io::Error::other("Checkout identity changed"))?;
    if actual != expected {
        return Err(io::Error::other("Checkout identity changed"));
    }
    let lease_dir = default_lease_dir().map_err(|e| io::Error::other(e.to_string()))?;
    let (_lease_dir_path, lease_dir_fd) =
        open_lease_dir(&lease_dir).map_err(|e| io::Error::other(e.to_string()))?;
    let (lifecycle_file, expected_lock_id) =
        open_lock_file(&lease_dir_fd, &checkout_lock_name(expected))
            .map_err(|e| io::Error::other(e.to_string()))?;

    let mut guard = UndoLeaseGuard {
        owned_writer: None,
        owned_lifecycle: None,
    };

    // Level 1: lifecycle exclusive (reuse inherited fd 7 if it matches the lock file).
    if verify_inherited_lock_fd(7, expected_lock_id).is_ok() {
        if !inherited_exclusive(7, &lifecycle_file)? {
            return Err(io::Error::other(format!("busy: {CHECKOUT_BUSY}")));
        }
    } else {
        if !try_flock_fd(lifecycle_file.as_raw_fd(), libc::LOCK_EX)
            .map_err(|e| io::Error::other(e.to_string()))?
        {
            return Err(io::Error::other(format!("busy: {CHECKOUT_BUSY}")));
        }
        guard.owned_lifecycle = Some(lifecycle_file);
    }

    // Level 2: checkout directory inode writer exclusive (reuse inherited fd 4 if it matches).
    if verify_inherited_dir_fd(4, expected).is_ok() {
        if !try_flock_fd(4, libc::LOCK_EX).map_err(|e| io::Error::other(e.to_string()))? {
            return Err(io::Error::other("busy: checkout already has a writer"));
        }
    } else {
        if !try_flock_fd(dir_file.as_raw_fd(), libc::LOCK_EX)
            .map_err(|e| io::Error::other(e.to_string()))?
        {
            return Err(io::Error::other("busy: checkout already has a writer"));
        }
        guard.owned_writer = Some(dir_file);
    }

    verify_dir_still(&canonical, expected)
        .map_err(|_| io::Error::other("Checkout identity changed"))?;
    if guard.owned_writer.is_some() {
        cleanup::require_clear(expected).map_err(io::Error::other)?;
    }
    Ok(guard)
}

/// Whether the inherited lifecycle descriptor `fd` already holds `LOCK_EX`.
///
/// The inherited description belongs to the runtime, so it is never converted:
/// a shared lock upgraded here would stay exclusive after this helper exits, and
/// on Linux a refused conversion removes the shared lock first. `probe` is a
/// fresh description of the same lock file. It can take a shared lock only when
/// nobody holds an exclusive one; when it cannot, re-asserting `LOCK_EX` on `fd`
/// is a no-op if `fd` is that holder and a refusal otherwise.
fn inherited_exclusive(fd: i32, probe: &File) -> io::Result<bool> {
    let lock =
        |fd, operation| try_flock_fd(fd, operation).map_err(|e| io::Error::other(e.to_string()));
    if lock(probe.as_raw_fd(), libc::LOCK_SH)? {
        unsafe {
            libc::flock(probe.as_raw_fd(), libc::LOCK_UN);
        }
        return Ok(false);
    }
    lock(fd, libc::LOCK_EX)
}

fn fstat_fd(fd: i32) -> io::Result<libc::stat> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { stat.assume_init() })
}

fn verify_inherited_lock_fd(fd: i32, expected: DirIdentity) -> io::Result<()> {
    let stat = fstat_fd(fd)?;
    let uid = unsafe { libc::geteuid() };
    #[allow(clippy::unnecessary_cast)]
    let file_type = (stat.st_mode as u32) & (libc::S_IFMT as u32);
    #[allow(clippy::unnecessary_cast)]
    let perms = (stat.st_mode as u32) & 0o077;
    #[allow(clippy::unnecessary_cast)]
    let dev = stat.st_dev as u64;
    #[allow(clippy::unnecessary_cast)]
    let ino = stat.st_ino as u64;
    #[allow(clippy::unnecessary_cast)]
    let nlink = stat.st_nlink as u64;
    #[allow(clippy::unnecessary_cast)]
    if file_type != (libc::S_IFREG as u32)
        || nlink != 1
        || stat.st_uid != uid
        || perms != 0
        || dev != expected.dev
        || ino != expected.ino
    {
        return Err(io::Error::other("invalid checkout lifecycle lease"));
    }
    Ok(())
}

fn verify_inherited_dir_fd(fd: i32, expected: DirIdentity) -> io::Result<()> {
    let stat = fstat_fd(fd)?;
    #[allow(clippy::unnecessary_cast)]
    let file_type = (stat.st_mode as u32) & (libc::S_IFMT as u32);
    #[allow(clippy::unnecessary_cast)]
    let dev = stat.st_dev as u64;
    #[allow(clippy::unnecessary_cast)]
    let ino = stat.st_ino as u64;
    #[allow(clippy::unnecessary_cast)]
    if file_type != (libc::S_IFDIR as u32) || dev != expected.dev || ino != expected.ino {
        return Err(io::Error::other("invalid checkout writer lease"));
    }
    Ok(())
}

/// Lock an owner-only regular runner lock inherited on fd 3.
pub fn runner_lock() -> io::Result<()> {
    let stat = fstat_fd(3)?;
    #[allow(clippy::unnecessary_cast)]
    if (stat.st_mode as u32 & libc::S_IFMT as u32) != libc::S_IFREG as u32
        || stat.st_mode as u32 & 0o077 != 0
        || stat.st_uid != unsafe { libc::geteuid() }
        || stat.st_nlink != 1
    {
        return Err(io::Error::other("invalid runner lock"));
    }
    if !try_flock_fd(3, libc::LOCK_EX).map_err(io::Error::other)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "runner lock busy",
        ));
    }
    Ok(())
}
