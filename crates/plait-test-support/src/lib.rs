//! Temporary directory ownership for Rust tests and their child processes.
use std::{fs::Permissions, os::unix::fs::PermissionsExt, path::PathBuf, sync::OnceLock};
use tempfile::TempDir;

pub fn directory() -> TempDir {
    directory_in(std::env::temp_dir())
}

pub fn socket_directory() -> TempDir {
    // The longest fixture names must still fit in a macOS Unix socket address.
    let root = if cfg!(target_os = "macos") {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    directory_in(root)
}

fn directory_in(root: PathBuf) -> TempDir {
    tempfile::Builder::new()
        .prefix("plait-")
        .permissions(Permissions::from_mode(0o700))
        .tempdir_in(root)
        .expect("create test directory")
}

thread_local! {
    static FIXTURES: TempDir = directory();
    static SOCKET_FIXTURES: TempDir = socket_directory();
}

/// Libtest drops the test thread's directory even when the test unwinds.
pub fn temp_dir() -> PathBuf {
    FIXTURES.with(canonical_root)
}

pub fn socket_temp_dir() -> PathBuf {
    SOCKET_FIXTURES.with(canonical_root)
}

fn canonical_root(root: &TempDir) -> PathBuf {
    root.path()
        .canonicalize()
        .expect("canonical test directory")
}

// Concurrent lease holders and their subprocesses must all use the same directory.
static LEASES: OnceLock<(u32, PathBuf)> = OnceLock::new();

extern "C" fn remove_leases() {
    if let Some((pid, directory)) = LEASES.get()
        && *pid == std::process::id()
    {
        let _ = std::fs::remove_dir_all(directory);
    }
}

pub fn lease_dir() -> PathBuf {
    let (_, path) = LEASES.get_or_init(|| {
        let mut root = directory();
        let path = root
            .path()
            .canonicalize()
            .expect("canonical lease directory");
        // SAFETY: the callback has static lifetime and the required C signature.
        assert_eq!(unsafe { libc::atexit(remove_leases) }, 0);
        root.disable_cleanup(true);
        (std::process::id(), path)
    });
    path.clone()
}

#[test]
fn failed_fixture_setup_releases_its_directory() {
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let root = temp_dir();
        std::fs::create_dir(root.join("unfinished")).unwrap();
        tx.send(root).unwrap();
        panic!("fixture setup failed");
    });
    assert!(worker.join().is_err());
    assert!(!rx.recv().unwrap().exists());
}

#[test]
fn ordinary_fixture_uses_private_tmpdir() {
    const TEST: &str = "ordinary_fixture_uses_private_tmpdir";
    if std::env::var_os("PLAIT_TEST_PRIVATE_TMPDIR").is_some() {
        assert_eq!(temp_dir().parent(), Some(std::env::temp_dir().as_path()));
        return;
    }
    let scratch = directory();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST])
        .env("TMPDIR", canonical_root(&scratch))
        .env("PLAIT_TEST_PRIVATE_TMPDIR", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
