//! Bounded logical storage accounting. No Git, symlink following, or reclamation promise.
pub use plait_protocol::worktree_size::{BUILD_DIRS, DirId, Key, Size};
use std::{
    collections::HashSet,
    ffi::{CStr, CString},
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
pub const ENTRY_LIMIT: usize = 50_000;
pub const DEADLINE: Duration = Duration::from_millis(250);
pub fn measure(
    root: &Path,
    cache: Option<&Path>,
    skip: &HashSet<DirId>,
    cancelled: &AtomicBool,
) -> Result<Size, String> {
    measure_bounded(root, cache, skip, ENTRY_LIMIT, DEADLINE, cancelled)
}
/// Entries are reached from their parent's open descriptor and never through a
/// path, so a directory swapped for a symlink mid-walk cannot lead it outside.
/// The walk stays on each root's device and does not enter the `skip` folders
/// (the project's other checkouts), which are measured on their own.
pub fn measure_bounded(
    root: &Path,
    cache: Option<&Path>,
    skip: &HashSet<DirId>,
    limit: usize,
    duration: Duration,
    cancelled: &AtomicBool,
) -> Result<Size, String> {
    measure_output(root, cache, skip, (limit, duration), cancelled, &[])
}
fn measure_output(
    root: &Path,
    cache: Option<&Path>,
    skip: &HashSet<DirId>,
    bounds: (usize, Duration),
    cancelled: &AtomicBool,
    approved: &[String],
) -> Result<Size, String> {
    let (limit, duration) = bounds;
    let mut walk = Walk {
        result: Size::default(),
        seen: HashSet::new(),
        stack: Vec::new(),
        skip,
        approved,
        limit,
        deadline: Instant::now() + duration,
    };
    let checkout =
        open_root(root).map_err(|_| "The checkout folder is missing or replaced.".to_owned())?;
    walk.enter(checkout, false, Path::new(""));
    if let Some(cache) = cache
        && cache != root
        && fs::symlink_metadata(cache).is_ok()
    {
        match open_root(cache) {
            Ok(dir) => walk.enter(dir, true, Path::new("")),
            Err(_) => walk.result.incomplete = true,
        }
    }
    while let Some(entry) = walk.stack.pop() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Measurement cancelled.".into());
        }
        if walk.result.entries >= limit || Instant::now() >= walk.deadline {
            walk.result.incomplete = true;
            break;
        }
        walk.result.entries += 1;
        walk.visit(entry);
    }
    Ok(walk.result)
}
struct Entry {
    parent: Rc<OwnedFd>,
    name: CString,
    build: bool,
    relative: PathBuf,
    device: u64,
}
struct Walk<'a> {
    result: Size,
    seen: HashSet<DirId>,
    stack: Vec<Entry>,
    skip: &'a HashSet<DirId>,
    approved: &'a [String],
    limit: usize,
    deadline: Instant,
}
impl Walk<'_> {
    /// Queue a directory's entries. `top` marks the checkout's own top level,
    /// where conventional output folders count as build output.
    fn enter(&mut self, dir: (OwnedFd, libc::stat), build: bool, relative: &Path) {
        let (fd, stat) = dir;
        if !self.seen.insert(id(&stat)) {
            return;
        }
        let parent = Rc::new(fd);
        let device = id(&stat).0;
        let complete = list(&parent, |name| {
            if self.stack.len() + self.result.entries >= self.limit
                || Instant::now() >= self.deadline
            {
                return false;
            }
            if name.to_bytes() != b".git" {
                let path = relative.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
                let output = build
                    || self.approved.iter().any(|p| path == Path::new(p))
                    || relative.as_os_str().is_empty()
                        && std::str::from_utf8(name.to_bytes())
                            .is_ok_and(|s| BUILD_DIRS.contains(&s));
                self.stack.push(Entry {
                    parent: parent.clone(),
                    name: name.to_owned(),
                    build: output,
                    relative: path,
                    device,
                });
            }
            true
        });
        if !complete {
            self.result.incomplete = true;
        }
    }
    fn visit(&mut self, entry: Entry) {
        let Ok(stat) = stat_at(entry.parent.as_raw_fd(), &entry.name) else {
            self.result.incomplete = true;
            return;
        };
        let kind = stat.st_mode & libc::S_IFMT;
        if kind == libc::S_IFLNK || id(&stat).0 != entry.device || self.skip.contains(&id(&stat)) {
            return;
        }
        if kind == libc::S_IFREG {
            if self.seen.insert(id(&stat)) {
                let size = stat.st_size.max(0) as u64;
                let total = if entry.build {
                    &mut self.result.build
                } else {
                    &mut self.result.source
                };
                *total = total.saturating_add(size);
            }
        } else if kind == libc::S_IFDIR {
            // The name may have been replaced since it was read: open it without
            // following a link and require the directory that was listed.
            match open_at(entry.parent.as_raw_fd(), &entry.name) {
                Ok(dir) if id(&dir.1) == id(&stat) => self.enter(dir, entry.build, &entry.relative),
                _ => self.result.incomplete = true,
            }
        }
    }
}
#[cfg(target_vendor = "apple")]
unsafe fn errno() -> *mut libc::c_int {
    unsafe { libc::__error() }
}
#[cfg(not(target_vendor = "apple"))]
unsafe fn errno() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}
// `dev_t` and `ino_t` differ in width and sign between macOS and Linux.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn id(stat: &libc::stat) -> DirId {
    (stat.st_dev as u64, stat.st_ino as u64)
}
fn checked(fd: libc::c_int) -> io::Result<(OwnedFd, libc::stat)> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned open by the kernel and is owned here alone.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: `stat` is plain data the kernel fills.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok((fd, stat))
}
const DIR_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
pub(crate) fn open_root(path: &Path) -> io::Result<(OwnedFd, libc::stat)> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: a valid NUL-terminated path.
    checked(unsafe { libc::open(path.as_ptr(), DIR_FLAGS) })
}
pub(crate) fn open_at(parent: libc::c_int, name: &CStr) -> io::Result<(OwnedFd, libc::stat)> {
    // SAFETY: `parent` is an open directory and `name` one NUL-terminated component.
    checked(unsafe { libc::openat(parent, name.as_ptr(), DIR_FLAGS) })
}
pub(crate) fn stat_at(parent: libc::c_int, name: &CStr) -> io::Result<libc::stat> {
    // SAFETY: as in `open_at`; `stat` is plain data the kernel fills.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    let status =
        unsafe { libc::fstatat(parent, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
    if status == 0 {
        Ok(stat)
    } else {
        Err(io::Error::last_os_error())
    }
}
/// Calls `each` with every name in the directory except `.` and `..` until it
/// returns false. Returns false when the listing stopped early or failed.
pub(crate) fn list(dir: &OwnedFd, mut each: impl FnMut(&CStr) -> bool) -> bool {
    // SAFETY: `dup` of an open descriptor; `fdopendir` takes ownership of the
    // duplicate and `closedir` releases both.
    let copy = unsafe { libc::dup(dir.as_raw_fd()) };
    if copy < 0 {
        return false;
    }
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        unsafe { libc::close(copy) };
        return false;
    }
    let mut complete = true;
    loop {
        // `readdir` returns null both at the end and on an error; only errno differs.
        // SAFETY: `stream` is open; the entry is read before the next call.
        unsafe { *errno() = 0 };
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            complete &= unsafe { *errno() } == 0;
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        if !each(name) {
            complete = false;
            break;
        }
    }
    unsafe { libc::closedir(stream) };
    complete
}
pub fn measure_retained_cache(
    cache: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<Size, String> {
    let Some(cache) = cache else {
        return Ok(Size::default());
    };
    match fs::symlink_metadata(cache) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Size::default()),
        _ => {}
    }
    let mut result = measure(cache, None, &HashSet::new(), cancelled)?;
    result.build = result.total();
    result.source = 0;
    Ok(result)
}
pub fn measure_verified(
    key: &Key,
    cache: Option<&Path>,
    skip: &HashSet<DirId>,
    cancelled: &AtomicBool,
) -> Result<Size, String> {
    if let Some(expected) = &key.identity {
        let meta = fs::symlink_metadata(&key.root)
            .map_err(|e| format!("Cannot measure this folder: {e}"))?;
        if expected.dev != meta.dev().to_string() || expected.ino != meta.ino().to_string() {
            return Err(
                "The checkout folder changed identity. Refresh before measuring it.".into(),
            );
        }
    }
    measure_output(
        &key.root,
        cache,
        skip,
        (ENTRY_LIMIT, DEADLINE),
        cancelled,
        &key.build_directories,
    )
}
#[derive(Default)]
pub struct Cache(Vec<(Key, Size)>);
impl Cache {
    pub fn get(&self, key: &Key) -> Option<&Size> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, s)| s)
    }
    pub fn put(&mut self, key: Key, size: Size) {
        self.0.retain(|(k, _)| k.root != key.root);
        self.0.push((key, size));
        if self.0.len() > 256 {
            self.0.remove(0);
        }
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approved_nested_custom_directories_count_as_build_output() {
        let root = std::env::temp_dir().join(format!("plait-custom-size-{}", std::process::id()));
        fs::create_dir_all(root.join("runtime/artifacts")).unwrap();
        fs::write(root.join("runtime/artifacts/library"), [0u8; 19]).unwrap();
        fs::write(root.join("runtime/source"), [0u8; 7]).unwrap();
        let size = measure_output(
            &root,
            None,
            &HashSet::new(),
            (100, Duration::from_secs(2)),
            &AtomicBool::new(false),
            &["runtime/artifacts".into()],
        )
        .unwrap();
        assert_eq!((size.source, size.build), (7, 19));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sizes_bound_entries_deadline_links_and_cache_without_claiming_reclaimed_space() {
        let root = std::env::temp_dir().join(format!("plait-size-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("source"), vec![0; 100]).unwrap();
        fs::write(root.join("target/output"), vec![0; 200]).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        // The host sandbox denies link creation; the ordinary test still covers deduplication.
        if let Err(error) = fs::hard_link(root.join("source"), root.join("copy")) {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        }
        let cancel = AtomicBool::new(false);
        let none = HashSet::new();
        let full =
            measure_bounded(&root, None, &none, 100, Duration::from_secs(1), &cancel).unwrap();
        assert_eq!((full.source, full.build), (100, 200));
        assert!(!full.incomplete);
        assert!(
            measure_bounded(&root, None, &none, 1, Duration::from_secs(1), &cancel)
                .unwrap()
                .incomplete
        );
        assert!(
            measure_bounded(&root, None, &none, 100, Duration::ZERO, &cancel)
                .unwrap()
                .incomplete
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(measure(&root, None, &none, &cancel).is_err());
        let key = Key {
            build_directories: Vec::new(),
            root: root.clone(),
            identity: None,
            head: Some("a".into()),
            last_used: None,
            changed: Some(0),
            removed: false,
        };
        let mut cache = Cache::default();
        cache.put(key.clone(), full.clone());
        assert_eq!(cache.get(&key), Some(&full));
        let changed = Key {
            head: Some("b".into()),
            ..key.clone()
        };
        assert!(cache.get(&changed).is_none());
        let removed = Key {
            removed: true,
            ..key.clone()
        };
        assert!(cache.get(&removed).is_none());
        cancel.store(false, Ordering::Relaxed);
        let retained = measure_retained_cache(Some(&root.join("target")), &cancel).unwrap();
        assert_eq!((retained.source, retained.build), (0, 200));
        let replaced = Key {
            identity: Some(crate::git::worktree::Identity {
                dev: "0".into(),
                ino: "0".into(),
            }),
            ..key.clone()
        };
        assert!(measure_verified(&replaced, None, &none, &cancel).is_err());
        cache.clear();
        assert!(cache.get(&key).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("plait-size-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }
    fn identity(path: &Path) -> DirId {
        let meta = fs::symlink_metadata(path).unwrap();
        (meta.dev(), meta.ino())
    }
    #[test]
    fn links_to_the_root_and_other_checkouts_inside_are_not_walked() {
        let root = scratch("inside");
        fs::write(root.join("own"), vec![0; 10]).unwrap();
        std::os::unix::fs::symlink("/", root.join("everything")).unwrap();
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/.git"), "gitdir: /elsewhere\n").unwrap();
        fs::write(root.join("nested/big"), vec![0; 1000]).unwrap();
        let cancel = AtomicBool::new(false);
        let skip = HashSet::from([identity(&root.join("nested"))]);
        let size = measure(&root, None, &skip, &cancel).unwrap();
        assert_eq!((size.source, size.incomplete), (10, false));
        // Without the other checkout named, it is ordinary content.
        assert_eq!(
            measure(&root, None, &HashSet::new(), &cancel)
                .unwrap()
                .source,
            1010
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unreadable_folders_make_the_size_a_lower_bound() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("locked");
        fs::create_dir_all(root.join("closed")).unwrap();
        fs::write(root.join("closed/file"), vec![0; 5]).unwrap();
        fs::set_permissions(root.join("closed"), fs::Permissions::from_mode(0o000)).unwrap();
        let size = measure(&root, None, &HashSet::new(), &AtomicBool::new(false)).unwrap();
        fs::set_permissions(root.join("closed"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(size.incomplete);
        assert_eq!(size.source, 0);
        fs::remove_dir_all(root).unwrap();
    }
    /// A folder swapped for a symlink while the walk runs must not lead it
    /// outside: path-based walking counted outside files in a fifth of runs.
    #[cfg(target_vendor = "apple")]
    #[test]
    fn a_folder_swapped_for_a_symlink_mid_walk_never_leads_outside() {
        let root = scratch("swap");
        let outside = scratch("swap-outside");
        fs::create_dir_all(outside.join("deep")).unwrap();
        for i in 0..200 {
            fs::write(outside.join(format!("deep/o{i}")), vec![0; 1000]).unwrap();
        }
        let sub = root.join("sub");
        fs::create_dir_all(sub.join("deep")).unwrap();
        for i in 0..300 {
            fs::write(sub.join(format!("s{i}")), [0u8]).unwrap();
        }
        let link = root.with_extension("link");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let (a, b) = (
            CString::new(sub.as_os_str().as_bytes()).unwrap(),
            CString::new(link.as_os_str().as_bytes()).unwrap(),
        );
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let swapper = std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                // SAFETY: two valid paths; swapping them is the point of the test.
                unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
            }
        });
        let cancel = AtomicBool::new(false);
        let worst = (0..3000)
            .map(|_| {
                measure(&root, None, &HashSet::new(), &cancel)
                    .unwrap()
                    .source
            })
            .max()
            .unwrap();
        stop.store(true, Ordering::Relaxed);
        swapper.join().unwrap();
        assert!(
            worst <= 300,
            "counted {worst} bytes, more than the checkout holds"
        );
        let _ = fs::remove_file(&link);
        let _ = fs::remove_dir_all(&outside);
        let _ = fs::remove_dir_all(&root);
    }
}
