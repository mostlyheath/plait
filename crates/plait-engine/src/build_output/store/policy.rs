use crate::{
    build_output,
    git::{self, worktree::Identity},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema: u32,
    pub project: PathBuf,
    pub identity: Identity,
    pub revision: u64,
    pub paths: Vec<String>,
}
pub fn private_directory(path: &Path, create: bool) -> io::Result<()> {
    if create {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || path.canonicalize()? != path
    {
        return Err(io::Error::other(
            "Build output needs a canonical private host directory.",
        ));
    }
    Ok(())
}
pub fn folder(store: &Path, name: &str) -> io::Result<PathBuf> {
    private_directory(store, false)?;
    let parent = store.join(name);
    private_directory(&parent, true)?;
    Ok(parent)
}
pub fn project(root: &Path) -> io::Result<(PathBuf, Identity)> {
    let root = git::worktree::main_checkout(root).map_err(io::Error::other)?;
    let identity = Identity::read(&root).map_err(io::Error::other)?;
    Ok((root, identity))
}
pub fn key(identity: &Identity) -> io::Result<String> {
    identity.lease().map_err(io::Error::other)?;
    Ok(format!("{}-{}", identity.dev, identity.ino))
}
pub fn load(store: &Path, root: &Path, identity: &Identity) -> io::Result<Policy> {
    identity.checked(root).map_err(io::Error::other)?;
    let path = folder(store, "build-output-policy")?.join(format!("{}.json", key(identity)?));
    let value: Policy = read(&path)?.unwrap_or(Policy {
        schema: 1,
        project: root.into(),
        identity: identity.clone(),
        revision: 0,
        paths: Vec::new(),
    });
    if value.schema != 1
        || value.project != root
        || &value.identity != identity
        || value.paths.len() > build_output::MAX_PATHS
        || value
            .paths
            .iter()
            .any(|p| build_output::relative(p).is_err())
    {
        return Err(io::Error::other("Invalid build output policy."));
    }
    Ok(value)
}
pub fn set(
    store: &Path,
    root: &Path,
    expected: &Identity,
    revision: u64,
    paths: Vec<String>,
) -> io::Result<Policy> {
    let (main, identity) = project(root)?;
    if &identity != expected {
        return Err(io::Error::other(
            "Project identity changed. Refresh before approving build output.",
        ));
    }
    let resolved = crate::lease::resolve_git_checkout(&main).map_err(io::Error::other)?;
    // Removal holds this same lock while reading policy and checking ignored files.
    let _lock = crate::lease::acquire_common_metadata(
        &resolved.common_dir,
        crate::lease::COMMON_METADATA_WAIT,
    )
    .map_err(io::Error::other)?;
    let mut policy = load(store, &main, &identity)?;
    if policy.revision != revision {
        return Err(io::Error::other(
            "Build output choices changed. Refresh before saving.",
        ));
    }
    if paths.len() > build_output::MAX_PATHS {
        return Err(io::Error::other("Approve at most 32 build directories."));
    }
    let mut approved = Vec::<PathBuf>::new();
    for path in &paths {
        // Revocation must work even if another previously approved cache is gone.
        // Clone and removal revalidate retained entries at their use site.
        let checked = if policy.paths.contains(path) {
            build_output::relative(path).map_err(io::Error::other)?
        } else {
            git::build_output::validate(&main, path).map_err(io::Error::other)?
        };
        if approved
            .iter()
            .any(|p| checked.starts_with(p) || p.starts_with(&checked))
        {
            return Err(io::Error::other(
                "Approved build directories must not overlap.",
            ));
        }
        approved.push(checked);
    }
    policy.revision = policy
        .revision
        .checked_add(1)
        .ok_or_else(|| io::Error::other("Build output revision exhausted."))?;
    policy.paths = paths;
    save(
        &folder(store, "build-output-policy")?.join(format!("{}.json", key(&identity)?)),
        &policy,
    )?;
    Ok(policy)
}
const STATE_LIMIT: u64 = 1024 * 1024;

pub fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
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
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
        || meta.len() > STATE_LIMIT
    {
        return Err(io::Error::other("Unsafe build output state."));
    }
    let mut bytes = Vec::new();
    file.take(STATE_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > STATE_LIMIT {
        return Err(io::Error::other(
            "Build output state exceeds its read limit.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::other)
}
pub fn save(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() as u64 > STATE_LIMIT {
            return Err(io::Error::other(
                "Build output state exceeds its write limit.",
            ));
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().unwrap())?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
