//! A seed yields to checkout admission across host stores. A private FIFO wakes
//! its helper; no PID signaling, desktop polling, or clone-length wait is needed.
use super::*;
use std::os::unix::fs::FileTypeExt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub struct Capture {
    _lease: CheckoutLease,
    pipe: File,
    monitor: Option<thread::JoinHandle<()>>,
    _registration: File,
    cancelled: Arc<AtomicBool>,
}
impl Capture {
    pub fn cancelled(&self) -> &AtomicBool {
        &self.cancelled
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        // A normal return wakes the reader without cancelling the process.
        unsafe {
            libc::write(self.pipe.as_raw_fd(), b"D".as_ptr().cast(), 1);
        }
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }
}
fn name(identity: DirIdentity) -> CString {
    CString::new(format!("seed-{}-{}.pipe", identity.dev, identity.ino)).unwrap()
}
fn checked_pipe(dir: &File, name: &CStr, flags: i32) -> Result<Option<File>, LeaseError> {
    let raw = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENXIO)) {
            return Ok(None);
        }
        return Err(LeaseError::io(format!(
            "Cannot open build seed wakeup: {error}"
        )));
    }
    let file = unsafe { File::from_raw_fd(raw) };
    let metadata = file.metadata().map_err(|e| LeaseError::io(e.to_string()))?;
    let listed = crate::worktree_size::stat_at(dir.as_raw_fd(), name)
        .map_err(|e| LeaseError::io(e.to_string()))?;
    if !metadata.file_type().is_fifo()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || (metadata.dev(), metadata.ino()) != crate::worktree_size::id(&listed)
    {
        return Err(LeaseError::unsafe_store("Unsafe build seed wakeup."));
    }
    Ok(Some(file))
}
pub fn capture(
    root: &Path,
    expected: DirIdentity,
    exit_on_cancel: bool,
) -> Result<Capture, LeaseError> {
    capture_in(&default_lease_dir()?, root, expected, exit_on_cancel)
}
fn capture_in(
    lease_dir: &Path,
    root: &Path,
    expected: DirIdentity,
    exit_on_cancel: bool,
) -> Result<Capture, LeaseError> {
    let (_, _, identity) = open_verified_dir(root)?;
    if identity != expected {
        return Err(LeaseError::identity("Checkout identity changed."));
    }
    let (_, dir) = open_lease_dir(lease_dir)?;
    let (registration, _) = open_lock_file(
        &dir,
        &format!("seed-{}-{}.lock", identity.dev, identity.ino),
    )?;
    if !try_flock_fd(registration.as_raw_fd(), libc::LOCK_EX)? {
        return Err(LeaseError::checkout_busy(
            "Another seed capture is active for this checkout.",
        ));
    }
    let name = name(identity);
    if unsafe { libc::mkfifoat(dir.as_raw_fd(), name.as_ptr(), 0o600) } != 0
        && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
    {
        return Err(LeaseError::io(format!(
            "Cannot prepare build seed wakeup: {}",
            io::Error::last_os_error()
        )));
    }
    let pipe = checked_pipe(&dir, &name, libc::O_RDWR)?
        .ok_or_else(|| LeaseError::io("Build seed wakeup disappeared."))?;
    let lease = acquire_without_seed_yield(lease_dir, root, Some(expected), LeaseClass::Writer)?;
    let reader = pipe
        .try_clone()
        .map_err(|e| LeaseError::io(e.to_string()))?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let monitor = thread::Builder::new()
        .name("plait-seed-wakeup".into())
        .spawn(move || {
            let mut event = libc::pollfd {
                fd: reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                let ready = unsafe { libc::poll(&mut event, 1, -1) };
                if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                let mut byte = 0u8;
                if ready > 0
                    && unsafe { libc::read(reader.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) }
                        == 1
                    && byte == b'C'
                {
                    flag.store(true, Ordering::Relaxed);
                    if exit_on_cancel {
                        std::process::exit(75);
                    }
                }
                break;
            }
        })
        .map_err(|e| LeaseError::io(e.to_string()))?;
    Ok(Capture {
        _lease: lease,
        pipe,
        monitor: Some(monitor),
        _registration: registration,
        cancelled,
    })
}
/// Wake a live seed and wait only for the pipe's reader to close, bounded to
/// 20 ms. The ordinary lease check still decides whether a run may start.
pub fn yield_to_run(root: &Path) -> Result<(), LeaseError> {
    yield_in(&default_lease_dir()?, root)
}
pub(super) fn yield_in(lease_dir: &Path, root: &Path) -> Result<(), LeaseError> {
    let (_, _, identity) = open_verified_dir(root)?;
    let (_, dir) = open_lease_dir(lease_dir)?;
    let Some(pipe) = checked_pipe(&dir, &name(identity), libc::O_WRONLY)? else {
        return Ok(());
    };
    // The helper may have closed after open. Ignore EPIPE, never deliver SIGPIPE.
    #[cfg(target_os = "macos")]
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) } < 0 {
        return Err(LeaseError::io(io::Error::last_os_error().to_string()));
    }
    let written = unsafe { libc::write(pipe.as_raw_fd(), b"C".as_ptr().cast(), 1) };
    if written == 1 {
        let mut event = libc::pollfd {
            fd: pipe.as_raw_fd(),
            events: libc::POLLERR | libc::POLLHUP,
            revents: 0,
        };
        unsafe {
            libc::poll(&mut event, 1, 20);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::symlink,
        sync::{atomic::AtomicU64, mpsc},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn fixture() -> (PathBuf, PathBuf, PathBuf, DirIdentity) {
        let base = std::env::temp_dir().join(format!(
            "plait-seed-priority-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let root = base.join("root");
        fs::create_dir(&root).unwrap();
        let leases = base.join("leases");
        fs::DirBuilder::new().mode(0o700).create(&leases).unwrap();
        let (_, _, identity) = open_verified_dir(&root).unwrap();
        (base, root, leases, identity)
    }
    #[test]
    fn a_writer_preempts_a_seed_through_the_shared_registry() {
        let (base, root, leases, identity) = fixture();
        let source = root.clone();
        let registry = leases.clone();
        let (ready, wait) = mpsc::channel();
        let worker = thread::spawn(move || {
            let capture = capture_in(&registry, &source, identity, false).unwrap();
            ready.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !capture.cancelled().load(Ordering::Relaxed) {
                assert!(Instant::now() < deadline, "seed did not yield");
                thread::sleep(Duration::from_millis(1));
            }
        });
        wait.recv().unwrap();
        let start = Instant::now();
        let writer =
            acquire_checkout_in(&leases, &root, Some(identity), LeaseClass::Writer).unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        worker.join().unwrap();
        drop(writer);
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn wakeup_never_follows_links_or_writes_a_regular_file() {
        let (base, root, leases, identity) = fixture();
        let target = base.join("secret");
        fs::write(&target, "keep").unwrap();
        let pipe = leases.join(name(identity).to_str().unwrap());
        symlink(&target, &pipe).unwrap();
        assert!(yield_in(&leases, &root).is_err());
        fs::remove_file(&pipe).unwrap();
        fs::write(&pipe, "keep too").unwrap();
        assert!(yield_in(&leases, &root).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert_eq!(fs::read(&pipe).unwrap(), b"keep too");
        fs::remove_dir_all(base).unwrap();
    }
}
