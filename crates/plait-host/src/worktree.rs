//! Supervised host worktree helper. Journals outlive both the socket and runtime.
use plait_engine::git::worktree::{self, HelperRequest as Request, ManagedWorktree};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

/// The checkout's usual file modes. The host's private umask protects its
/// store, but a worktree is the person's checkout: owner-only files would
/// break shared trees and build contexts. Journals set their modes explicitly.
const CHECKOUT_UMASK: libc::mode_t = 0o022;

pub fn run() -> io::Result<()> {
    // SAFETY: umask only changes this process and what it starts.
    unsafe {
        libc::umask(CHECKOUT_UMASK);
    }
    super::record::supervise("worktree-worker", Duration::from_secs(150))
}
pub fn worker(input: Value) -> io::Result<()> {
    let result = execute(input);
    let value = match result {
        Ok(value) => json!({"value":value,"error":null}),
        Err(error) => json!({"value":null,"error":error.to_string()}),
    };
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &value)?;
    out.write_all(b"\n")
}
fn execute(input: Value) -> io::Result<Value> {
    let request: Request = serde_json::from_value(input).map_err(io::Error::other)?;
    let result = match request {
        Request::Inventory {
            root,
            store,
            records,
            only,
        } => {
            use plait_engine::build_output::store::{policy, seed_summary};
            let Some(store) = store else {
                return worktree::inventory::inventory_of(&root, &records, only.as_deref())
                    .and_then(|value| serde_json::to_value(value).map_err(|e| e.to_string()))
                    .map_err(io::Error::other);
            };
            let (main, identity) = policy::project(&root)?;
            let approved = policy::load(&store, &main, &identity)?.paths;
            let checkouts = worktree::inventory::inventory_with_output(
                &root,
                &records,
                only.as_deref(),
                &approved,
            )
            .map_err(io::Error::other)?;
            // The seed counts toward the project's storage; read from its pointer.
            let seed = seed_summary(&store, &identity)?.map(|seed| seed.bytes);
            return Ok(json!({"checkouts":checkouts,"seed_bytes":seed}));
        }
        Request::Parent { root } => {
            return worktree::worktree_parent(&root)
                .and_then(|value| serde_json::to_value(value).map_err(|e| e.to_string()))
                .map_err(io::Error::other);
        }
        Request::Create {
            store,
            thread_id,
            parent,
        } => {
            let path = journal_path(&store, &thread_id)?;
            worktree::create_managed_supervised(
                &parent,
                &thread_id,
                read(&path)?,
                |record| save(&path, record).map_err(|e| e.to_string()),
                &mut supervised_git,
            )
        }
        Request::Remove {
            store,
            thread_id,
            identity,
            allow_unpushed,
        } => {
            let path = journal_path(&store, &thread_id)?;
            let record = read(&path)?.ok_or_else(|| {
                io::Error::other(
                    "No host-owned worktree record. Imported worktrees cannot be removed here.",
                )
            })?;
            worktree::remove_managed_with_output(
                record,
                &identity,
                allow_unpushed,
                |record| save(&path, record).map_err(|e| e.to_string()),
                &mut supervised_git,
                |parent| {
                    plait_engine::build_output::store::policy::load(
                        &store,
                        &parent.main,
                        &parent.main_identity,
                    )
                    .map(|policy| policy.paths)
                    .map_err(|e| e.to_string())
                },
            )
        }
        Request::Read { store, thread_id } => {
            return serde_json::to_value(read(&journal_path(&store, &thread_id)?)?)
                .map_err(io::Error::other);
        }
    };
    serde_json::to_value(result.map_err(io::Error::other)?).map_err(io::Error::other)
}
fn journal_path(store: &Path, id: &str) -> io::Result<PathBuf> {
    if id.len() != 36
        || !id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
    {
        return Err(io::Error::other("Invalid thread UUID."));
    }
    if !store.is_absolute() || store.canonicalize()? != store {
        return Err(io::Error::other("Invalid host store."));
    }
    let base = store.join("worktrees");
    match fs::DirBuilder::new().mode(0o700).create(&base) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    for path in [store, base.as_path()] {
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "Worktree journals require a private host store.",
            ));
        }
    }
    Ok(base.join(format!("{id}.json")))
}
fn read(path: &Path) -> io::Result<Option<ManagedWorktree>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.len() > 65536
    {
        return Err(io::Error::other("Unsafe worktree journal."));
    }
    let mut data = Vec::new();
    file.take(65537).read_to_end(&mut data)?;
    if data.len() > 65536 {
        return Err(io::Error::other(
            "Worktree journal exceeds the safe read limit.",
        ));
    }
    serde_json::from_slice(&data)
        .map(Some)
        .map_err(io::Error::other)
}
fn save(path: &Path, record: &ManagedWorktree) -> io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, record)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().unwrap())?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

// Nested supervision keeps the outer worker's leases alive until hooks and
// filters are reclaimed. Outer-helper death closes both lifetime pipes.
fn supervised_git(
    root: &Path,
    args: &[&str],
    _timeout: Duration,
    leases: &[&plait_engine::lease::CheckoutLease],
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("worktree-git")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    // A killed worker cannot unlock the checkout while its Git group still lives.
    for lease in leases {
        lease.lend_to(&mut command);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let mut input = child.stdin.take().ok_or("Missing Git helper input.")?;
    serde_json::to_writer(&mut input, &json!({"root":root,"args":args}))
        .map_err(|e| e.to_string())?;
    drop(input);
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(
            "Worktree Git helper stopped; existing files and branch were preserved.".into(),
        );
    }
    let reply: Value = serde_json::from_slice(&result.stdout).map_err(|e| e.to_string())?;
    match reply["error"].as_str() {
        Some(error) => Err(error.into()),
        None if reply["ok"] == true => Ok(()),
        _ => Err("Invalid worktree Git result.".into()),
    }
}
pub fn git_run() -> io::Result<()> {
    super::record::supervise("worktree-git-worker", Duration::from_secs(130))
}
pub fn git_worker(input: Value) -> io::Result<()> {
    let root = input["root"]
        .as_str()
        .ok_or_else(|| io::Error::other("Missing Git root."))?;
    let args = input["args"]
        .as_array()
        .ok_or_else(|| io::Error::other("Missing Git arguments."))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| io::Error::other("Invalid Git argument."))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let result = worktree::managed_git(Path::new(root), &args);
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &json!({"ok":result.is_ok(),"error":result.err()}))?;
    out.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    const ID: &str = "12345678-1234-4abc-8abc-123456789abc";
    struct Store(PathBuf);
    impl Store {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("plait-worktree-{name}-{}", std::process::id()));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn journals_refuse_shared_stores_and_symlinked_directories() {
        let store = Store::new("directory");
        assert!(journal_path(&store.0, "------------------------------------").is_err());
        assert!(!store.0.join("worktrees").exists());
        fs::set_permissions(&store.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(journal_path(&store.0, ID).is_err());
        fs::set_permissions(&store.0, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(store.0.join("worktrees")).unwrap();
        let unrelated = store.0.join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        symlink(&unrelated, store.0.join("worktrees")).unwrap();
        assert!(journal_path(&store.0, ID).is_err());
        assert_eq!(fs::read_dir(unrelated).unwrap().count(), 0);
    }
    #[test]
    fn journals_refuse_symlinks_hardlinks_permissions_and_oversize() {
        let store = Store::new("file");
        let path = journal_path(&store.0, ID).unwrap();
        assert!(read(&path).unwrap().is_none());
        let unrelated = store.0.join("unrelated");
        fs::write(&unrelated, "preserve").unwrap();
        fs::set_permissions(&unrelated, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&unrelated, &path).unwrap();
        assert!(read(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&unrelated, &path).unwrap();
        assert!(read(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&path).unwrap_err().to_string().contains("Unsafe"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, vec![b' '; 65537]).unwrap();
        assert!(read(&path).is_err());
        fs::write(&path, "{bad").unwrap();
        assert!(read(&path).is_err());
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "preserve");
    }
}
