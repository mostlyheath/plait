use super::*;
#[cfg(target_os = "macos")]
use std::os::unix::fs::MetadataExt;
use std::{fs, os::unix::fs::symlink, sync::atomic::AtomicU64};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "plait-clone-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn dir(&self, name: &str) -> OwnedFd {
        fs::create_dir_all(self.0.join(name)).unwrap();
        open_root(&self.0.join(name)).unwrap().0
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn paths_never_authorize_git_traversal_or_virtual_environments() {
    for path in [
        "",
        "/target",
        "target/",
        "a//b",
        "a/../b",
        ".git",
        "a/.GIT/objects",
        "venv",
        ".venv/bin",
        "a\\b",
        "a\nb",
    ] {
        assert!(relative(path).is_err(), "{path}");
    }
    for path in ["target", "runtime/node_modules", "custom-artifacts"] {
        assert!(relative(path).is_ok());
    }
}
#[test]
fn link_resolution_expands_links_before_parent_components() {
    let members = [
        ("a", None),
        ("a/up", Some("..")),
        ("file", None),
        ("a/loop", Some("loop")),
    ]
    .into_iter()
    .map(|(p, l)| (p.into(), l.map(PathBuf::from)))
    .collect();
    assert!(resolves_inside(
        Path::new("bin"),
        Path::new("a/up/file"),
        &members
    ));
    for target in ["/file", "../file", "a/up/../file", "missing", "a/loop"] {
        assert!(
            !resolves_inside(Path::new("bin"), Path::new(target), &members),
            "{target}"
        );
    }
}
#[test]
fn unsupported_space_and_cancellation_have_no_copy_fallback() {
    let tree = Tree::new();
    let src = tree.dir("src");
    fs::write(tree.0.join("src/file"), "original").unwrap();
    for (index, code) in [libc::ENOTSUP, libc::ENOSPC, libc::EXDEV]
        .into_iter()
        .enumerate()
    {
        let dst = tree.dir(&format!("dst{index}"));
        let mut calls = 0;
        let error = clone_with(
            &src,
            &dst,
            &AtomicBool::new(false),
            &mut |_| {},
            &mut |_, _, _| {
                calls += 1;
                Err(io::Error::from_raw_os_error(code))
            },
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.raw_os_error(), Some(code));
        assert_eq!(
            fs::read_dir(tree.0.join(format!("dst{index}")))
                .unwrap()
                .count(),
            0
        );
        assert!(!cold_reason(&error).is_empty());
        // readdir consumes the descriptor's offset; production opens each source once.
        unsafe {
            libc::lseek(src.as_raw_fd(), 0, libc::SEEK_SET);
        }
    }
    let dst = tree.dir("cancelled");
    assert_eq!(
        clone_tree(&src, &dst, &AtomicBool::new(true), &mut |_| {})
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
}
#[cfg(target_os = "macos")]
#[test]
fn clones_are_independent_keep_bin_links_and_skip_escaping_links_and_sockets() {
    let tree = Tree::new();
    let src = tree.dir("src");
    let dst = tree.dir("dst");
    fs::create_dir_all(tree.0.join("src/node_modules/.bin")).unwrap();
    fs::write(tree.0.join("src/node_modules/tool"), "source").unwrap();
    fs::hard_link(
        tree.0.join("src/node_modules/tool"),
        tree.0.join("src/hard"),
    )
    .unwrap();
    symlink("../tool", tree.0.join("src/node_modules/.bin/tool")).unwrap();
    symlink(
        "../../../outside",
        tree.0.join("src/node_modules/.bin/escape"),
    )
    .unwrap();
    symlink("/etc/passwd", tree.0.join("src/absolute")).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(tree.0.join("src/socket")).unwrap();
    let counts = clone_tree(&src, &dst, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!((counts.files, counts.links, counts.skipped), (2, 1, 3));
    assert_eq!(
        fs::read(tree.0.join("dst/node_modules/.bin/tool")).unwrap(),
        b"source"
    );
    assert!(fs::symlink_metadata(tree.0.join("dst/node_modules/.bin/escape")).is_err());
    let original = tree.0.join("src/node_modules/tool");
    let cloned = tree.0.join("dst/node_modules/tool");
    assert_ne!(
        fs::metadata(&original).unwrap().ino(),
        fs::metadata(&cloned).unwrap().ino()
    );
    assert_ne!(
        fs::metadata(tree.0.join("dst/hard")).unwrap().ino(),
        fs::metadata(&cloned).unwrap().ino()
    );
    fs::write(&cloned, "clone edit").unwrap();
    assert_eq!(fs::read(&original).unwrap(), b"source");
    fs::write(&original, "source edit").unwrap();
    assert_eq!(fs::read(&cloned).unwrap(), b"clone edit");
    assert_eq!(fs::read(tree.0.join("dst/hard")).unwrap(), b"source");
}
#[cfg(target_os = "macos")]
#[test]
fn immutable_files_are_skipped_so_the_clone_can_always_be_removed() {
    let tree = Tree::new();
    let src = tree.dir("src");
    let dst = tree.dir("dst");
    fs::write(tree.0.join("src/locked"), "keep").unwrap();
    fs::write(tree.0.join("src/plain"), "copy").unwrap();
    let flag = |path: &Path, flags: u32| {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::chflags(path.as_ptr(), flags) }, 0);
    };
    flag(&tree.0.join("src/locked"), libc::UF_IMMUTABLE);
    let result = clone_tree(&src, &dst, &AtomicBool::new(false), &mut |_| {});
    flag(&tree.0.join("src/locked"), 0);
    // Before the fix the clone carried the flag; clear it so the fixture can go.
    if fs::symlink_metadata(tree.0.join("dst/locked")).is_ok() {
        flag(&tree.0.join("dst/locked"), 0);
    }
    let counts = result.unwrap();
    assert_eq!((counts.files, counts.skipped), (1, 1));
    assert!(counts.notes[0].starts_with("locked: immutable"));
    assert!(fs::symlink_metadata(tree.0.join("dst/locked")).is_err());
    let expected = id(&stat(&dst).unwrap());
    remove_owned(&tree.0, Path::new("dst"), expected).unwrap();
}
#[cfg(target_os = "macos")]
#[test]
fn cloned_directories_keep_the_source_permissions_and_stay_removable() {
    use std::os::unix::fs::PermissionsExt;
    let tree = Tree::new();
    let src = tree.dir("src");
    let dst = tree.dir("dst");
    for (name, mode) in [("open", 0o755), ("group", 0o750), ("sealed", 0o555)] {
        fs::create_dir(tree.0.join("src").join(name)).unwrap();
        fs::write(tree.0.join("src").join(name).join("file"), "x").unwrap();
        fs::set_permissions(
            tree.0.join("src").join(name),
            fs::Permissions::from_mode(mode),
        )
        .unwrap();
    }
    fs::set_permissions(tree.0.join("src"), fs::Permissions::from_mode(0o755)).unwrap();
    clone_tree(&src, &dst, &AtomicBool::new(false), &mut |_| {}).unwrap();
    let mode = |name: &str| {
        fs::metadata(tree.0.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode("dst"), 0o755);
    assert_eq!(mode("dst/open"), 0o755);
    assert_eq!(mode("dst/group"), 0o750);
    assert_eq!(mode("dst/sealed"), 0o755);
    let expected = id(&stat(&dst).unwrap());
    remove_owned(&tree.0, Path::new("dst"), expected).unwrap();
    fs::set_permissions(tree.0.join("src/sealed"), fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(target_os = "macos")]
#[test]
fn an_unreadable_file_fails_the_clone_and_names_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let tree = Tree::new();
    let src = tree.dir("src");
    let dst = tree.dir("dst");
    fs::create_dir(tree.0.join("src/deps")).unwrap();
    fs::write(tree.0.join("src/deps/private"), "x").unwrap();
    fs::set_permissions(
        tree.0.join("src/deps/private"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let error = clone_tree(&src, &dst, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert_eq!(
        cold_reason(&error),
        "Cannot read deps/private in the build directory; it is not readable."
    );
}
#[test]
fn replaced_directory_is_not_followed_and_partial_output_can_be_erased() {
    let tree = Tree::new();
    let src = tree.dir("src");
    let dst = tree.dir("dst");
    fs::create_dir(tree.0.join("src/sub")).unwrap();
    fs::write(tree.0.join("src/sub/file"), "a").unwrap();
    fs::create_dir(tree.0.join("outside")).unwrap();
    fs::write(tree.0.join("outside/secret"), "keep").unwrap();
    let result = clone_with(
        &src,
        &dst,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |_, _, _| {
            fs::rename(tree.0.join("src/sub"), tree.0.join("saved")).unwrap();
            symlink(tree.0.join("outside"), tree.0.join("src/sub")).unwrap();
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(!tree.0.join("dst/sub/secret").exists());
    let expected = id(&stat(&dst).unwrap());
    remove_owned(&tree.0, Path::new("dst"), expected).unwrap();
    assert!(!tree.0.join("dst").exists());
    assert_eq!(fs::read(tree.0.join("outside/secret")).unwrap(), b"keep");
}
#[test]
fn replaced_output_is_preserved_even_when_original_folder_was_removed() {
    let tree = Tree::new();
    let dst = tree.dir("dst");
    let identity = id(&stat(&dst).unwrap());
    fs::rename(tree.0.join("dst"), tree.0.join("old")).unwrap();
    fs::create_dir(tree.0.join("dst")).unwrap();
    fs::write(tree.0.join("dst/user"), "keep").unwrap();
    assert!(remove_owned(&tree.0, Path::new("dst"), identity).is_err());
    assert_eq!(fs::read(tree.0.join("dst/user")).unwrap(), b"keep");
}
#[cfg(target_os = "macos")]
#[test]
#[ignore = "disposable APFS measurement; run explicitly"]
fn measure_real_shaped_cow_tree() {
    let tree = Tree::new();
    let src = tree.dir("src");
    let dst = tree.dir("dst");
    let bytes = vec![0x5a; 128 * 1024];
    for bucket in 0..200 {
        let directory = tree.0.join(format!("src/deps-{bucket}"));
        fs::create_dir(&directory).unwrap();
        for file in 0..100 {
            fs::write(directory.join(format!("lib-{file}.rlib")), &bytes).unwrap();
        }
    }
    let available = || {
        let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        let path = CString::new(tree.0.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::statvfs(path.as_ptr(), s.as_mut_ptr()) }, 0);
        let s = unsafe { s.assume_init() };
        u64::from(s.f_bavail) * s.f_frsize
    };
    let before = available();
    let start = Instant::now();
    let counts = clone_tree(&src, &dst, &AtomicBool::new(false), &mut |_| {}).unwrap();
    eprintln!(
        "APFS clone: files={} logical_bytes={} elapsed_ms={} available_before={} available_after={}",
        counts.files,
        counts.bytes,
        start.elapsed().as_millis(),
        before,
        available()
    );
    assert_eq!(counts.files, 20000);
    assert_eq!(counts.bytes, 2_621_440_000);
}
#[cfg(target_os = "macos")]
#[test]
fn a_clone_and_a_stat_walk_agree_until_anything_is_written() {
    let tree = Tree::new();
    let src = tree.dir("src");
    fs::create_dir_all(tree.0.join("src/deps/.bin")).unwrap();
    fs::write(tree.0.join("src/deps/lib"), "library").unwrap();
    symlink("../lib", tree.0.join("src/deps/.bin/tool")).unwrap();
    symlink("/outside", tree.0.join("src/escape")).unwrap();
    let dst = tree.dir("dst");
    let (_, cloned) = clone_tree_digest(&src, &dst, &AtomicBool::new(false), &mut |_| {}).unwrap();
    let walked = fingerprint(&src, &AtomicBool::new(false)).unwrap();
    assert_eq!(walked, cloned);
    assert_eq!(fingerprint(&src, &AtomicBool::new(false)).unwrap(), walked);
    // Same size, same name: only the change time tells.
    fs::write(tree.0.join("src/deps/lib"), "librarz").unwrap();
    assert_ne!(fingerprint(&src, &AtomicBool::new(false)).unwrap(), walked);
}
