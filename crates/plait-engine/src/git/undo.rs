//! Restore a run's changed working files under the checkout lease. Git only
//! writes the private record store; descriptor-relative operations write files.
use super::record::{self, ChangedPath, Located, Reader, Record, Uncovered};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
pub const BUDGET: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub checkout: PathBuf,
    pub store: PathBuf,
    pub before: String,
    pub after: String,
    pub before_id: String,
    pub id: String,
    pub identity: Identity,
    #[serde(default)]
    pub paths: Option<Vec<ChangedPath>>,
    #[serde(default)]
    pub not_covered: Option<Vec<Uncovered>>,
    #[serde(default)]
    pub leave_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub dev: String,
    pub ino: String,
}

/// Also persisted before applying, then after the final capture. On interruption
/// the runtime can recover the pair without replaying any working-file writes.
#[derive(Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub state: String,
    pub changed_since: Vec<String>,
    pub not_covered: Vec<Uncovered>,
    pub before: Record,
    pub after: Option<Record>,
    pub applied: usize,
    pub applied_paths: Vec<String>,
    pub error: Option<String>,
}

pub fn journal(store: &Path, id: &str) -> Result<PathBuf> {
    record::valid_id(id)?;
    Ok(record::records_dir(store).join(format!("undo-{id}.json")))
}

fn save(request: &Request, outcome: &Outcome) -> Result<()> {
    let path = journal(&request.store, &request.id)?;
    let temporary = path.with_extension("tmp");
    let bytes = serde_json::to_vec(outcome).map_err(|e| e.to_string())?;
    let mut file = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&temporary)
        .map_err(err)?;
    file.write_all(&bytes).map_err(err)?;
    file.sync_all().map_err(err)?;
    fs::rename(temporary, &path).map_err(err)?;
    File::open(path.parent().ok_or("Journal has no parent")?)
        .map_err(err)?
        .sync_all()
        .map_err(err)
}

/// Discovery belongs to the supervised undo process group as well.
pub fn locate(checkout: &Path) -> Result<Located> {
    record::within(Instant::now() + BUDGET, || record::locate(checkout))
}

pub fn undo(located: &Located, request: &Request) -> Result<Outcome> {
    undo_with(located, request, Instant::now() + BUDGET, &mut |_, _| {})
}

fn undo_with(
    located: &Located,
    request: &Request,
    deadline: Instant,
    hook: &mut dyn FnMut(&str, &str),
) -> Result<Outcome> {
    record::within(deadline, || {
        record::valid_id(&request.id)?;
        record::valid_id(&request.before_id)?;
        let root = root(request)?;
        let reader = Reader::open(located, &request.store)?;
        let (paths, mut not_covered) = match &request.paths {
            Some(paths) => (
                paths.clone(),
                match &request.not_covered {
                    Some(list) => list.clone(),
                    None => reader.coverage(&request.before, &request.after)?,
                },
            ),
            None => reader.changes(&request.before, &request.after)?,
        };
        let capture = |id: &str, before: Option<String>| {
            record::record(
                located,
                &record::Request {
                    checkout: request.checkout.clone(),
                    store: request.store.clone(),
                    id: id.into(),
                    before,
                },
            )
        };
        // A fresh record is always taken before checking or applying anything.
        let before = capture(&request.before_id, None)?;
        // Only changed entries need to be readable. Unchanged borrowed blobs
        // are not a backup and may already have been pruned in the source repo.
        let earlier = reader.entries(&request.before).unwrap_or_else(|_| {
            paths
                .iter()
                .filter_map(|p| Path::new(&p.path).parent())
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| {
                    (
                        format!("{}/unknown", p.display()),
                        (String::new(), String::new()),
                    )
                })
                .collect()
        });
        not_covered.extend(before.not_covered.clone());
        not_covered.sort();
        not_covered.dedup();
        let mut outcome = Outcome {
            state: "blocked".into(),
            changed_since: Vec::new(),
            not_covered,
            before,
            after: None,
            applied: 0,
            applied_paths: Vec::new(),
            error: None,
        };
        let mut safe = Vec::new();
        // Validate every before-object before the first write, including deleted files.
        for path in paths {
            record::remaining()?;
            validate_path(&path.path)?;
            let expected = path.after_mode.clone().zip(path.after_oid.clone());
            let covered = !outcome.before.not_covered.iter().any(|item| {
                let base = item.path.trim_end_matches('/');
                (path.path == base || path.path.starts_with(&format!("{base}/")))
                    && (expected.is_none() || item.reason == record::Reason::Changing)
            });
            let after = fingerprint_blob(
                &reader,
                path.after_mode.as_deref(),
                path.after_oid.as_deref(),
            )?;
            // Preflight object availability even when another path is blocked.
            if let Some(oid) = &path.before_oid {
                reader.blob(oid)?;
            }
            if covered
                && reader.entry(&outcome.before.commit, &path.path)? == expected
                && current(&root, &path.path).is_ok_and(|now| now == after)
            {
                safe.push((path, after));
            } else {
                outcome.changed_since.push(path.path);
            }
        }
        if !outcome.changed_since.is_empty() && !request.leave_changed {
            save(request, &outcome)?;
            return Ok(outcome);
        }
        // Empty/safe-only requests are still records, and can be deduplicated.
        outcome.state = "applying".into();
        save(request, &outcome)?;
        let applied = apply(request, &root, &reader, &earlier, &safe, &mut outcome, hook);
        if let Err(error) = applied {
            outcome.error = Some(error);
        }
        match record::record_undo(
            located,
            &record::Request {
                checkout: request.checkout.clone(),
                store: request.store.clone(),
                id: request.id.clone(),
                before: Some(outcome.before.commit.clone()),
            },
            &outcome.applied_paths,
        ) {
            Ok(after) => {
                outcome.after = Some(after);
            }
            Err(error) => {
                outcome.error.get_or_insert(error);
            }
        }
        outcome.state = if outcome.error.is_some() {
            "incomplete"
        } else {
            "undone"
        }
        .into();
        save(request, &outcome)?;
        Ok(outcome)
    })
}

#[derive(Debug, PartialEq, Eq, Clone)]
struct Fingerprint {
    mode: String,
    hash: Vec<u8>,
}
fn fingerprint_blob(
    reader: &Reader,
    mode: Option<&str>,
    oid: Option<&str>,
) -> Result<Option<Fingerprint>> {
    match (mode, oid) {
        (None, None) => Ok(None),
        (Some(mode @ ("100644" | "100755" | "120000")), Some(oid)) => Ok(Some(Fingerprint {
            mode: mode.into(),
            hash: Sha256::digest(reader.blob(oid)?).to_vec(),
        })),
        _ => Err("Unsupported record entry.".into()),
    }
}
fn err(error: io::Error) -> String {
    error.to_string()
}
fn cstr(value: &[u8]) -> Result<CString> {
    CString::new(value).map_err(|e| e.to_string())
}
fn check(code: i32) -> Result<()> {
    if code == -1 {
        Err(err(io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

fn open_at(parent: &File, name: &str, flags: i32, mode: u32) -> Result<File> {
    let name = cstr(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            mode as libc::c_uint,
        )
    };
    check(fd)?;
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn root(request: &Request) -> Result<File> {
    if !request.checkout.is_absolute()
        || fs::canonicalize(&request.checkout).map_err(err)? != request.checkout
    {
        return Err("Checkout root changed.".into());
    }
    let root = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(&request.checkout)
        .map_err(err)?;
    let meta = root.metadata().map_err(err)?;
    if meta.dev().to_string() != request.identity.dev
        || meta.ino().to_string() != request.identity.ino
    {
        return Err("Checkout root identity changed.".into());
    }
    Ok(root)
}
fn validate_path(path: &str) -> Result<()> {
    super::validate_relative(Path::new(path))?;
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part.eq_ignore_ascii_case(".git"))
        || path.contains('\0')
    {
        return Err("Unsafe undo path.".into());
    }
    Ok(())
}
fn stat(parent: &File, name: &str) -> Result<Option<libc::stat>> {
    let name = cstr(name.as_bytes())?;
    let mut meta = std::mem::MaybeUninit::uninit();
    let code = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            meta.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if code == -1 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(err(error));
    }
    Ok(Some(unsafe { meta.assume_init() }))
}
fn names(directory: &File) -> Result<Vec<Vec<u8>>> {
    // A new open description avoids sharing readdir's offset with the held parent.
    let copy = open_at(directory, ".", libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    use std::os::fd::IntoRawFd;
    let fd = copy.into_raw_fd();
    let dir = unsafe { libc::fdopendir(fd) };
    if dir.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(err(io::Error::last_os_error()));
    }
    let mut result = Vec::new();
    loop {
        let item = unsafe { libc::readdir(dir) };
        if item.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            result.push(name.to_vec());
        }
    }
    unsafe {
        libc::closedir(dir);
    }
    Ok(result)
}
/// Let the filesystem resolve Unicode and case aliases, rather than guessing
/// its folding rules. A different spelling resolving to this entry is blocked.
fn no_alias(parent: &File, name: &str, meta: Option<&libc::stat>) -> Result<()> {
    let Some(meta) = meta else {
        return Ok(());
    };
    let names = names(parent)?;
    if names.iter().any(|entry| entry == name.as_bytes()) {
        return Ok(());
    }
    Err(format!(
        "Path spelling changed: {name} (inode {}).",
        meta.st_ino
    ))
}
fn parent(root: &File, path: &str, create: bool) -> Result<Option<(File, String)>> {
    let mut parts: Vec<_> = path.split('/').collect();
    let name = parts.pop().ok_or("Empty path")?.to_owned();
    let mut directory = root.try_clone().map_err(err)?;
    for part in parts {
        let meta = stat(&directory, part)?;
        no_alias(&directory, part, meta.as_ref())?;
        if meta.is_none() {
            if !create {
                return Ok(None);
            }
            let name = cstr(part.as_bytes())?;
            check(unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) })?;
        }
        directory = open_at(&directory, part, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        if record::repository_entries(|name| stat(&directory, name).is_ok_and(|m| m.is_some())) {
            return Err("Nested repositories cannot be restored.".into());
        }
    }
    Ok(Some((directory, name)))
}
fn current(root: &File, path: &str) -> Result<Option<Fingerprint>> {
    let Some((directory, name)) = parent(root, path, false)? else {
        return Ok(None);
    };
    current_at(&directory, &name)
}
fn current_at(directory: &File, name: &str) -> Result<Option<Fingerprint>> {
    let meta = stat(directory, name)?;
    no_alias(directory, name, meta.as_ref())?;
    let Some(meta) = meta else {
        return Ok(None);
    };
    let kind = meta.st_mode & libc::S_IFMT;
    if kind == libc::S_IFDIR {
        if names(&open_at(
            directory,
            name,
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?)?
        .is_empty()
        {
            return Ok(None);
        }
        return Err("Directory contains files.".into());
    }
    // Replacing one hard link changes metadata of its siblings, which can
    // include the person's index or object store. Refuse rather than touch it.
    if meta.st_nlink != 1 {
        return Err("Hard-linked paths cannot be restored safely.".into());
    }
    let (mode, bytes) = if kind == libc::S_IFLNK {
        let mut bytes = vec![0u8; 65536];
        let name = cstr(name.as_bytes())?;
        let count = unsafe {
            libc::readlinkat(
                directory.as_raw_fd(),
                name.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        if count < 0 {
            return Err(err(io::Error::last_os_error()));
        }
        if count as usize == bytes.len() {
            return Err("Symlink too large.".into());
        }
        bytes.truncate(count as usize);
        ("120000", bytes)
    } else if kind == libc::S_IFREG {
        let mut file = open_at(directory, name, libc::O_RDONLY, 0)?;
        let held = file.metadata().map_err(err)?;
        // `dev_t` is 32 bits on macOS and already 64 on Linux.
        #[allow(clippy::unnecessary_cast)]
        let device = meta.st_dev as u64;
        if held.ino() != meta.st_ino || held.dev() != device || held.len() > record::FILE_LIMIT {
            return Err("File changed during undo.".into());
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(record::FILE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(err)?;
        let later = file.metadata().map_err(err)?;
        if bytes.len() as u64 > record::FILE_LIMIT
            || held.mtime() != later.mtime()
            || held.mtime_nsec() != later.mtime_nsec()
            || held.ctime() != later.ctime()
            || held.ctime_nsec() != later.ctime_nsec()
            || held.len() != later.len()
        {
            return Err("File changed while reading.".into());
        }
        (
            if held.mode() & 0o100 == 0 {
                "100644"
            } else {
                "100755"
            },
            bytes,
        )
    } else {
        return Err("Unsupported working file.".into());
    };
    Ok(Some(Fingerprint {
        mode: mode.into(),
        hash: Sha256::digest(bytes).to_vec(),
    }))
}
fn unlink(directory: &File, name: &str, folder: bool) -> Result<()> {
    let name = cstr(name.as_bytes())?;
    check(unsafe {
        libc::unlinkat(
            directory.as_raw_fd(),
            name.as_ptr(),
            if folder { libc::AT_REMOVEDIR } else { 0 },
        )
    })
}
fn recheck(
    request: &Request,
    held: &File,
    path: &str,
    expected: &Option<Fingerprint>,
) -> Result<()> {
    // Resolve anew as well as through the held parent. A swapped symlink cannot
    // redirect writes, and a renamed/replaced parent is refused before mutation.
    let fresh = root(request)?;
    let (parent, _) = parent(&fresh, path, false)?.ok_or("Parent disappeared")?;
    let a = held.metadata().map_err(err)?;
    let b = parent.metadata().map_err(err)?;
    if a.dev() != b.dev() || a.ino() != b.ino() || current(&fresh, path)? != *expected {
        return Err("Path changed immediately before undo.".into());
    }
    record::remaining()?;
    Ok(())
}
fn rename(directory: &File, source: &CStr, target: &CStr, exclusive: bool) -> Result<()> {
    let fd = directory.as_raw_fd();
    if !exclusive {
        return check(unsafe { libc::renameat(fd, source.as_ptr(), fd, target.as_ptr()) });
    }
    // A path recorded absent must still be absent at the atomic creation step.
    #[cfg(target_os = "macos")]
    {
        check(unsafe {
            libc::renameatx_np(fd, source.as_ptr(), fd, target.as_ptr(), libc::RENAME_EXCL)
        })
    }
    #[cfg(target_os = "linux")]
    {
        check(unsafe {
            libc::renameat2(
                fd,
                source.as_ptr(),
                fd,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        })
    }
}

fn apply(
    request: &Request,
    root: &File,
    reader: &Reader,
    earlier: &BTreeMap<String, (String, String)>,
    safe: &[(ChangedPath, Option<Fingerprint>)],
    outcome: &mut Outcome,
    hook: &mut dyn FnMut(&str, &str),
) -> Result<()> {
    let mut deletions: Vec<_> = safe
        .iter()
        .filter(|(path, _)| path.before_mode.is_none())
        .collect();
    deletions.sort_by_key(|(path, _)| std::cmp::Reverse(path.path.split('/').count()));
    let mut directories = BTreeSet::new();
    for (path, expected) in deletions {
        record::remaining()?;
        let (directory, name) = parent(root, &path.path, false)?.ok_or("Parent disappeared")?;
        hook("delete", &path.path);
        recheck(request, &directory, &path.path, expected)?;
        unlink(&directory, &name, false)?;
        outcome.applied += 1;
        outcome.applied_paths.push(path.path.clone());
        save(request, outcome)?;
        let mut ancestor = Path::new(&path.path).parent();
        while let Some(path) = ancestor.filter(|p| !p.as_os_str().is_empty()) {
            directories.insert(path.to_string_lossy().into_owned());
            ancestor = path.parent();
        }
    }
    let mut directories: Vec<_> = directories.into_iter().collect();
    directories.sort_by_key(|p| std::cmp::Reverse(p.split('/').count()));
    for path in directories {
        if earlier.keys().any(|p| p.starts_with(&format!("{path}/"))) {
            continue;
        }
        if let Some((directory, name)) = parent(root, &path, false)? {
            // rmdir is atomic and refuses nonempty directories, symlinks and files.
            let _ = unlink(&directory, &name, true);
        }
    }
    let mut restores: Vec<_> = safe
        .iter()
        .filter(|(path, _)| path.before_mode.is_some())
        .collect();
    restores.sort_by_key(|(path, _)| path.path.split('/').count());
    for (path, expected) in restores {
        record::remaining()?;
        let (directory, name) = parent(root, &path.path, true)?.ok_or("Parent disappeared")?;
        let bytes = reader.blob(path.before_oid.as_deref().ok_or("Missing before object")?)?;
        let temporary = format!(".plait-undo-{}-{}", request.id, outcome.applied);
        let temp = cstr(temporary.as_bytes())?;
        let target = cstr(name.as_bytes())?;
        let mut created = false;
        let result: Result<()> = (|| {
            if path.before_mode.as_deref() == Some("120000") {
                let link = cstr(&bytes)?;
                check(unsafe {
                    libc::symlinkat(link.as_ptr(), directory.as_raw_fd(), temp.as_ptr())
                })?;
                created = true;
            } else {
                let mode = if path.before_mode.as_deref() == Some("100755") {
                    0o755
                } else {
                    0o644
                };
                let mut file = open_at(
                    &directory,
                    &temporary,
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    mode,
                )?;
                created = true;
                file.write_all(&bytes).map_err(err)?;
                file.sync_all().map_err(err)?;
            }
            hook("rename", &path.path);
            recheck(request, &directory, &path.path, expected)?;
            if stat(&directory, &name)?.is_some_and(|m| m.st_mode & libc::S_IFMT == libc::S_IFDIR) {
                unlink(&directory, &name, true)?;
            }
            rename(&directory, &temp, &target, expected.is_none())?;
            // The rename has effects even if syncing the directory fails.
            outcome.applied += 1;
            outcome.applied_paths.push(path.path.clone());
            save(request, outcome)?;
            directory.sync_all().map_err(err)?;
            Ok(())
        })();
        if created && result.is_err() {
            let _ = unlink(&directory, &temporary, false);
        }
        result?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "undo_tests.rs"]
mod tests;
