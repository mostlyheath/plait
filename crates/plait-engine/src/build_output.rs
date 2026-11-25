//! Descriptor-relative copy-on-write trees. Callers own source and destination leases.
pub mod store;
use crate::worktree_size::{id, list, open_at, open_root, stat_at};
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString, OsStr, OsString},
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub use plait_protocol::build_output::{Counts, MAX_PATHS, Outcome, relative};
pub const ENTRY_LIMIT: usize = 1_000_000;
pub const DEADLINE: Duration = Duration::from_secs(120);
pub type DirectoryFd = OwnedFd;
/// Opens every component without following links, checking the listed identity.
pub fn directory(root: &Path, relative: &Path) -> io::Result<(OwnedFd, libc::stat)> {
    let listed = std::fs::symlink_metadata(root)?;
    directory_owned(root, relative, (listed.dev(), listed.ino()))
}
pub fn directory_owned(
    root: &Path,
    relative: &Path,
    expected: (u64, u64),
) -> io::Result<(OwnedFd, libc::stat)> {
    let mut current = open_root(root)?;
    if id(&current.1) != expected {
        return Err(moving());
    }
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let name = CString::new(name.as_bytes())?;
        let before = stat_at(current.0.as_raw_fd(), &name)?;
        let next = open_at(current.0.as_raw_fd(), &name)?;
        if id(&before) != id(&next.1) || id(&before).0 != id(&current.1).0 {
            return Err(io::Error::other(
                "Build directory changed or crosses a volume.",
            ));
        }
        reject_repository(&next.0)?;
        current = next;
    }
    Ok(current)
}

fn reject_repository(fd: &OwnedFd) -> io::Result<()> {
    let present = |name: &CStr| stat_at(fd.as_raw_fd(), name).is_ok();
    if present(c".git")
        || present(c"pyvenv.cfg")
        || present(c"HEAD") && present(c"objects") && present(c"refs")
    {
        return Err(io::Error::other(
            "Nested repositories and Python environments cannot be cloned.",
        ));
    }
    Ok(())
}

/// Approval checks the whole directory, including nested repositories that Git's
/// ignore listing intentionally collapses. It reads no file contents.
pub fn validate_tree(root: &OwnedFd) -> io::Result<()> {
    fn visit(fd: &OwnedFd, count: &mut usize, start: Instant, depth: usize) -> io::Result<()> {
        if depth > 128 {
            return Err(io::Error::other("Build directory is too deeply nested."));
        }
        reject_repository(fd)?;
        let metadata = stat(fd)?;
        let mut error = None;
        let complete = list(fd, |name| {
            *count += 1;
            if *count > ENTRY_LIMIT || start.elapsed() > DEADLINE {
                return false;
            }
            let result = (|| {
                if name.to_bytes().eq_ignore_ascii_case(b".git") {
                    return Err(io::Error::other("Nested repository in build output."));
                }
                let before = stat_at(fd.as_raw_fd(), name)?;
                if id(&before).0 != id(&metadata).0 {
                    return Err(io::Error::from_raw_os_error(libc::EXDEV));
                }
                if before.st_mode & libc::S_IFMT == libc::S_IFDIR {
                    let child = open_at(fd.as_raw_fd(), name)?;
                    if id(&child.1) != id(&before) {
                        return Err(moving());
                    }
                    visit(&child.0, count, start, depth + 1)?;
                    if id(&stat_at(fd.as_raw_fd(), name)?) != id(&before) {
                        return Err(moving());
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                error = Some(e);
                return false;
            }
            true
        });
        if let Some(error) = error {
            return Err(error);
        }
        if !complete {
            return Err(io::Error::other(
                "Build directory could not be checked completely.",
            ));
        }
        Ok(())
    }
    let scan = open_at(root.as_raw_fd(), c".")?;
    visit(&scan.0, &mut 0, Instant::now(), 0)
}

/// The destination must be a fresh, empty directory owned by the transaction.
pub fn clone_tree(
    source: &OwnedFd,
    destination: &OwnedFd,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(&Counts),
) -> io::Result<Counts> {
    clone_tree_digest(source, destination, cancelled, progress).map(|(counts, _)| counts)
}
/// Also returns the digest of every entry the clone saw, as `fingerprint`
/// would compute it for the same unchanged source.
pub fn clone_tree_digest(
    source: &OwnedFd,
    destination: &OwnedFd,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(&Counts),
) -> io::Result<(Counts, Digest)> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Build output cloning was skipped.",
        ));
    }
    if !cfg!(target_os = "macos") {
        return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
    }
    clone_with(source, destination, cancelled, progress, &mut clone_file)
}

type CloneFile<'a> = dyn FnMut(&OwnedFd, &OwnedFd, &CStr) -> io::Result<()> + 'a;
fn clone_with(
    source: &OwnedFd,
    destination: &OwnedFd,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(&Counts),
    clone: &mut CloneFile<'_>,
) -> io::Result<(Counts, Digest)> {
    let start = Instant::now();
    if id(&stat(source)?).0 != id(&stat(destination)?).0 {
        return Err(io::Error::from_raw_os_error(libc::EXDEV));
    }
    let mut walk = Walk {
        counts: Counts::default(),
        entries: 0,
        start,
        published: start,
        cancelled,
        progress,
        clone,
        members: BTreeMap::new(),
        links: Vec::new(),
        digest: Digest::default(),
    };
    walk.digest.add(Path::new(""), &stat(source)?, None);
    walk.visit(source, destination, Path::new(""), 0)?;
    set_mode(destination, &stat(source)?)?;
    for (path, target) in &walk.links {
        if resolves_inside(path, target, &walk.members) {
            let parent = path.parent().unwrap();
            let (fd, _) = below(destination, parent)?;
            let name = CString::new(path.file_name().unwrap().as_bytes())?;
            let target = CString::new(target.as_os_str().as_bytes())?;
            // SAFETY: checked directory and two valid NUL-terminated strings.
            if unsafe { libc::symlinkat(target.as_ptr(), fd.as_raw_fd(), name.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            walk.counts.links += 1;
        } else {
            walk.counts.skip(
                path,
                "link leaves the build directory, is absolute, or has no safe target",
            );
        }
    }
    walk.counts.elapsed_ms = start.elapsed().as_millis() as u64;
    (walk.progress)(&walk.counts);
    Ok((walk.counts, walk.digest))
}

fn below(root: &OwnedFd, path: &Path) -> io::Result<(OwnedFd, libc::stat)> {
    let mut current = open_at(root.as_raw_fd(), c".")?;
    for part in path.components() {
        let Component::Normal(name) = part else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let name = CString::new(name.as_bytes())?;
        let before = stat_at(current.0.as_raw_fd(), &name)?;
        let next = open_at(current.0.as_raw_fd(), &name)?;
        if id(&next.1) != id(&before) || id(&next.1).0 != id(&current.1).0 {
            return Err(moving());
        }
        current = next;
    }
    Ok(current)
}

fn make_at(root: &OwnedFd, path: &Path) -> io::Result<()> {
    let mut parent = open_at(root.as_raw_fd(), c".")?;
    for part in path.components() {
        let Component::Normal(name) = part else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let name = CString::new(name.as_bytes())?;
        if unsafe { libc::mkdirat(parent.0.as_raw_fd(), name.as_ptr(), 0o700) } != 0
            && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
        {
            return Err(io::Error::last_os_error());
        }
        let before = stat_at(parent.0.as_raw_fd(), &name)?;
        let next = open_at(parent.0.as_raw_fd(), &name)?;
        if id(&before) != id(&next.1) || id(&before).0 != id(&parent.1).0 {
            return Err(moving());
        }
        reject_repository(&next.0)?;
        parent = next;
    }
    Ok(())
}
/// Install between verified open roots, never through a substituted path.
pub fn install(
    source_root: &Path,
    source_identity: (u64, u64),
    slot: &Path,
    destination_root: &Path,
    destination_identity: (u64, u64),
    relative: &Path,
    expected: (u64, u64),
) -> io::Result<()> {
    let (source, _) = directory_owned(source_root, Path::new(""), source_identity)?;
    let (destination, _) = directory_owned(destination_root, Path::new(""), destination_identity)?;
    let parent = relative.parent().ok_or(io::ErrorKind::InvalidInput)?;
    make_at(&destination, parent)?;
    let (target, _) = below(&destination, parent)?;
    let slot = CString::new(slot.as_os_str().as_bytes())?;
    if id(&stat_at(source.as_raw_fd(), &slot)?) != expected {
        return Err(moving());
    }
    let name = CString::new(
        relative
            .file_name()
            .ok_or(io::ErrorKind::InvalidInput)?
            .as_bytes(),
    )?;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            source.as_raw_fd(),
            slot.as_ptr(),
            target.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(target_os = "macos"))]
    let result = unsafe {
        libc::renameat2(
            source.as_raw_fd(),
            slot.as_ptr(),
            target.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fsync(target.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Remove only an owned directory with the recorded identity. Every deletion is
/// relative to its open parent; replacements and links are never traversed.
pub fn remove_owned(root: &Path, relative: &Path, expected: (u64, u64)) -> io::Result<()> {
    let (parent, _) = directory(root, relative.parent().ok_or(io::ErrorKind::InvalidInput)?)?;
    let name = CString::new(
        relative
            .file_name()
            .ok_or(io::ErrorKind::InvalidInput)?
            .as_bytes(),
    )?;
    let listed = match stat_at(parent.as_raw_fd(), &name) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if id(&listed) != expected {
        return Err(io::Error::other(
            "Build output was replaced; it was preserved.",
        ));
    }
    erase(&parent, &name, &listed)
}
fn erase(parent: &OwnedFd, name: &CStr, before: &libc::stat) -> io::Result<()> {
    let flags = if before.st_mode & libc::S_IFMT == libc::S_IFDIR {
        let (fd, opened) = open_at(parent.as_raw_fd(), name)?;
        if id(before) != id(&opened) {
            return Err(moving());
        }
        let mut error = None;
        if !list(&fd, |child| {
            let result = stat_at(fd.as_raw_fd(), child).and_then(|stat| {
                if id(&stat).0 != id(&opened).0 {
                    return Err(io::Error::from_raw_os_error(libc::EXDEV));
                }
                erase(&fd, child, &stat)
            });
            if let Err(e) = result {
                error = Some(e);
                false
            } else {
                true
            }
        }) {
            return Err(
                error.unwrap_or_else(|| io::Error::other("Could not remove partial build output."))
            );
        }
        libc::AT_REMOVEDIR
    } else {
        0
    };
    if id(&stat_at(parent.as_raw_fd(), name)?) != id(before) {
        return Err(moving());
    }
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Walk<'a> {
    counts: Counts,
    entries: usize,
    start: Instant,
    published: Instant,
    cancelled: &'a AtomicBool,
    progress: &'a mut dyn FnMut(&Counts),
    clone: &'a mut CloneFile<'a>,
    members: BTreeMap<PathBuf, Option<PathBuf>>,
    links: Vec<(PathBuf, PathBuf)>,
    digest: Digest,
}
impl Walk<'_> {
    fn check(&mut self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Build output cloning was skipped.",
            ));
        }
        self.entries += 1;
        if self.entries > ENTRY_LIMIT || self.start.elapsed() > DEADLINE {
            return Err(io::Error::other(
                "Build output exceeds the clone preparation limit.",
            ));
        }
        if self.published.elapsed() >= Duration::from_millis(250) {
            self.counts.elapsed_ms = self.start.elapsed().as_millis() as u64;
            (self.progress)(&self.counts);
            self.published = Instant::now();
        }
        Ok(())
    }
    fn visit(&mut self, src: &OwnedFd, dst: &OwnedFd, path: &Path, depth: usize) -> io::Result<()> {
        if depth > 128 {
            return Err(io::Error::other("Build directory is too deeply nested."));
        }
        reject_repository(src)?;
        let before = stat(src)?;
        let mut error = None;
        let complete = list(src, |name| {
            match self.entry(
                src,
                dst,
                name,
                &path.join(OsString::from_vec(name.to_bytes().to_vec())),
                depth,
            ) {
                Ok(()) => true,
                Err(e) => {
                    error = Some(e);
                    false
                }
            }
        });
        if let Some(error) = error {
            return Err(error);
        }
        if !complete {
            return Err(io::Error::other(
                "Could not list the build directory completely.",
            ));
        }
        if !unchanged(&before, &stat(src)?) {
            return Err(moving());
        }
        Ok(())
    }
    fn entry(
        &mut self,
        src: &OwnedFd,
        dst: &OwnedFd,
        name: &CStr,
        path: &Path,
        depth: usize,
    ) -> io::Result<()> {
        self.check()?;
        if name.to_bytes().eq_ignore_ascii_case(b".git") {
            return Err(io::Error::other("Nested repository in build output."));
        }
        let before = stat_at(src.as_raw_fd(), name)?;
        if id(&before).0 != id(&stat(src)?).0 {
            return Err(io::Error::from_raw_os_error(libc::EXDEV));
        }
        if before.st_mode & libc::S_IFMT != libc::S_IFLNK {
            self.digest.add(path, &before, None);
        }
        match before.st_mode & libc::S_IFMT {
            libc::S_IFDIR => {
                let (child, opened) = open_at(src.as_raw_fd(), name)?;
                if id(&opened) != id(&before) {
                    return Err(moving());
                }
                // SAFETY: checked destination descriptor, one component, private staging tree.
                if unsafe { libc::mkdirat(dst.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                let (output, _) = open_at(dst.as_raw_fd(), name)?;
                self.members.insert(path.to_owned(), None);
                self.visit(&child, &output, path, depth + 1)?;
                if id(&stat_at(src.as_raw_fd(), name)?) != id(&before) {
                    return Err(moving());
                }
                set_mode(&output, &before)?;
            }
            libc::S_IFREG if locked(&before) => self.counts.skip(
                path,
                "immutable or append-only file, which no clone could delete",
            ),
            libc::S_IFREG => {
                // An open descriptor fixes the file's identity for fclonefileat.
                let raw = unsafe {
                    libc::openat(
                        src.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                    )
                };
                if raw < 0 {
                    return Err(unreadable(path, io::Error::last_os_error()));
                }
                let file = unsafe { OwnedFd::from_raw_fd(raw) };
                let opened = stat(&file)?;
                if opened.st_mode & libc::S_IFMT != libc::S_IFREG || !unchanged(&before, &opened) {
                    return Err(moving());
                }
                (self.clone)(&file, dst, name)?;
                if !unchanged(&before, &stat(&file)?)
                    || id(&stat_at(src.as_raw_fd(), name)?) != id(&before)
                {
                    return Err(moving());
                }
                self.counts.files += 1;
                self.counts.bytes += before.st_size.max(0) as u64;
                self.members.insert(path.to_owned(), None);
            }
            libc::S_IFLNK => {
                let buffer = read_link(src, name)?;
                self.digest.add(path, &before, Some(&buffer));
                let target = PathBuf::from(OsString::from_vec(buffer));
                if !unchanged(&before, &stat_at(src.as_raw_fd(), name)?) {
                    return Err(moving());
                }
                self.members.insert(path.to_owned(), Some(target.clone()));
                self.links.push((path.to_owned(), target));
            }
            _ => self
                .counts
                .skip(path, "not a regular file, directory or safe link"),
        }
        Ok(())
    }
}
/// Directories start owner-only while they fill; afterwards they get the
/// source's permissions, as a checkout would. The owner keeps full access so
/// a read-only source directory never makes its clone impossible to remove.
fn set_mode(directory: &OwnedFd, source: &libc::stat) -> io::Result<()> {
    let mode = (source.st_mode & 0o777) | 0o700;
    // SAFETY: an open directory descriptor this clone created.
    if unsafe { libc::fchmod(directory.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
/// A clone keeps the source's file flags. An immutable or append-only clone
/// could never be rolled back, refreshed or removed with its worktree.
fn locked(stat: &libc::stat) -> bool {
    #[cfg(target_os = "macos")]
    {
        const FLAGS: u32 =
            libc::UF_IMMUTABLE | libc::UF_APPEND | libc::SF_IMMUTABLE | libc::SF_APPEND;
        stat.st_flags & FLAGS != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = stat;
        false
    }
}
/// Skipping an unreadable file would leave an inconsistent cache, so the clone
/// fails; the reason names the file the person can fix.
fn unreadable(path: &Path, error: io::Error) -> io::Error {
    if matches!(error.raw_os_error(), Some(libc::EACCES | libc::EPERM)) {
        return io::Error::other(format!(
            "Cannot read {} in the build directory; it is not readable.",
            path.display()
        ));
    }
    error
}
fn read_link(parent: &OwnedFd, name: &CStr) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0u8; 4097];
    // SAFETY: an open directory, one NUL-terminated component and a sized buffer.
    let length = unsafe {
        libc::readlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize == buffer.len() {
        return Err(io::Error::other("Build link is too long."));
    }
    buffer.truncate(length as usize);
    Ok(buffer)
}
/// An order-independent digest of every entry a clone sees: its path, identity,
/// type, mode, size, flags, modification and change times, and a link's target.
/// Any write moves a file's change time, so an equal digest means a new clone
/// would copy the same tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Digest([u64; 4]);
impl Digest {
    // `dev_t`, `ino_t` and the time fields differ in width and sign by platform.
    #[allow(clippy::unnecessary_cast)]
    fn add(&mut self, path: &Path, stat: &libc::stat, link: Option<&[u8]>) {
        use sha2::Digest as _;
        let mut hash = sha2::Sha256::new();
        hash.update(path.as_os_str().as_bytes());
        hash.update([0]);
        for value in [
            stat.st_dev as u64,
            stat.st_ino as u64,
            stat.st_mode as u64,
            stat.st_size as u64,
            stat.st_mtime as u64,
            stat.st_mtime_nsec as u64,
            stat.st_ctime as u64,
            stat.st_ctime_nsec as u64,
            file_flags(stat),
        ] {
            hash.update(value.to_le_bytes());
        }
        if let Some(link) = link {
            hash.update(link);
        }
        for (slot, chunk) in self.0.iter_mut().zip(hash.finalize().chunks(8)) {
            *slot = slot.wrapping_add(u64::from_le_bytes(chunk.try_into().unwrap()));
        }
    }
    pub fn hex(&self) -> String {
        self.0.iter().map(|value| format!("{value:016x}")).collect()
    }
}
fn file_flags(stat: &libc::stat) -> u64 {
    #[cfg(target_os = "macos")]
    {
        u64::from(stat.st_flags)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = stat;
        0
    }
}
/// A stat-only walk with a clone's order, bounds and checks. It opens no file
/// and writes nothing, so an unchanged source costs listings, not a clone.
pub fn fingerprint(source: &OwnedFd, cancelled: &AtomicBool) -> io::Result<Digest> {
    struct Scan<'a> {
        digest: Digest,
        entries: usize,
        start: Instant,
        cancelled: &'a AtomicBool,
    }
    fn visit(scan: &mut Scan<'_>, fd: &OwnedFd, path: &Path, depth: usize) -> io::Result<()> {
        if depth > 128 {
            return Err(io::Error::other("Build directory is too deeply nested."));
        }
        reject_repository(fd)?;
        let mut error = None;
        let complete = list(fd, |name| {
            let result = (|| {
                if scan.cancelled.load(Ordering::Relaxed) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Build output cloning was skipped.",
                    ));
                }
                scan.entries += 1;
                if scan.entries > ENTRY_LIMIT || scan.start.elapsed() > DEADLINE {
                    return Err(io::Error::other(
                        "Build output exceeds the clone preparation limit.",
                    ));
                }
                if name.to_bytes().eq_ignore_ascii_case(b".git") {
                    return Err(io::Error::other("Nested repository in build output."));
                }
                let child = path.join(OsStr::from_bytes(name.to_bytes()));
                let before = stat_at(fd.as_raw_fd(), name)?;
                match before.st_mode & libc::S_IFMT {
                    libc::S_IFDIR => {
                        let (sub, opened) = open_at(fd.as_raw_fd(), name)?;
                        if id(&opened) != id(&before) {
                            return Err(moving());
                        }
                        scan.digest.add(&child, &before, None);
                        visit(scan, &sub, &child, depth + 1)
                    }
                    libc::S_IFLNK => {
                        let target = read_link(fd, name)?;
                        scan.digest.add(&child, &before, Some(&target));
                        Ok(())
                    }
                    _ => {
                        scan.digest.add(&child, &before, None);
                        Ok(())
                    }
                }
            })();
            match result {
                Ok(()) => true,
                Err(e) => {
                    error = Some(e);
                    false
                }
            }
        });
        if let Some(error) = error {
            return Err(error);
        }
        if !complete {
            return Err(io::Error::other(
                "Could not list the build directory completely.",
            ));
        }
        Ok(())
    }
    // A separate open has its own offset: the clone may list `source` next.
    let (root, root_stat) = open_at(source.as_raw_fd(), c".")?;
    let mut scan = Scan {
        digest: Digest::default(),
        entries: 0,
        start: Instant::now(),
        cancelled,
    };
    scan.digest.add(Path::new(""), &root_stat, None);
    visit(&mut scan, &root, Path::new(""), 0)?;
    Ok(scan.digest)
}
fn moving() -> io::Error {
    io::Error::other("Build output changed while it was being cloned.")
}
fn stat(fd: &OwnedFd) -> io::Result<libc::stat> {
    let mut value = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}
fn unchanged(a: &libc::stat, b: &libc::stat) -> bool {
    id(a) == id(b)
        && a.st_mode == b.st_mode
        && a.st_size == b.st_size
        && a.st_mtime == b.st_mtime
        && a.st_mtime_nsec == b.st_mtime_nsec
        && a.st_ctime == b.st_ctime
        && a.st_ctime_nsec == b.st_ctime_nsec
}
fn resolves_inside(
    path: &Path,
    target: &Path,
    members: &BTreeMap<PathBuf, Option<PathBuf>>,
) -> bool {
    use std::collections::VecDeque;
    if target.is_absolute() {
        return false;
    }
    let components = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_owned())
            .collect::<VecDeque<_>>()
    };
    let mut pending = components(target);
    let mut current = path.parent().unwrap().to_owned();
    let mut followed = 0;
    while let Some(part) = pending.pop_front() {
        if part == "." {
            continue;
        }
        if part == ".." {
            if !current.pop() {
                return false;
            }
            continue;
        }
        current.push(part);
        let Some(member) = members.get(&current) else {
            return false;
        };
        if let Some(link) = member {
            followed += 1;
            if followed > 40 || link.is_absolute() {
                return false;
            }
            current.pop();
            let mut next = components(link);
            next.append(&mut pending);
            pending = next;
        }
    }
    true
}
fn clone_file(src: &OwnedFd, dst: &OwnedFd, name: &CStr) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // libc 0.2.191 and the macOS SDK declare this descriptor-based CoW API.
        // CLONE_NOOWNERCOPY preserves modes without inheriting another owner.
        if unsafe { libc::fclonefileat(src.as_raw_fd(), dst.as_raw_fd(), name.as_ptr(), 2) } == 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (src, dst, name);
        Err(io::Error::from_raw_os_error(libc::ENOTSUP))
    }
}

pub fn cold_reason(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(libc::EXDEV) => "Build output is on another volume.".into(),
        Some(libc::ENOTSUP | libc::ENOSYS) => {
            "Copy-on-write cloning is unavailable on this filesystem or platform.".into()
        }
        Some(libc::ENOSPC | libc::EDQUOT) => {
            "There is not enough space to clone build output.".into()
        }
        _ => error.to_string().chars().take(2000).collect(),
    }
}

#[cfg(test)]
mod tests;
