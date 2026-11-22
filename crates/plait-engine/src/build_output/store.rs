//! Host-private approvals, immutable seeds and recoverable worktree preparation.
pub mod policy;
mod transaction;
use crate::{
    build_output::{self, Counts, Outcome},
    git::{
        self,
        worktree::{Identity, ManagedWorktree},
    },
    lease::{self, LeaseClass},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Info {
        store: PathBuf,
        root: PathBuf,
    },
    Set {
        store: PathBuf,
        root: PathBuf,
        identity: Identity,
        revision: u64,
        paths: Vec<String>,
    },
    Seed {
        store: PathBuf,
        root: PathBuf,
        identity: Identity,
        generation: String,
        source_cache: Option<String>,
        /// Ignore the seed's age; an unchanged source is still not cloned.
        #[serde(default)]
        force: bool,
    },
    Clone {
        store: PathBuf,
        worktree: ManagedWorktree,
        private_target: bool,
        /// With no usable seed, record nothing so the host can capture one
        /// and ask again; otherwise the cold outcome is final.
        #[serde(default)]
        retry_seedless: bool,
    },
    Recover {
        store: PathBuf,
        worktree: ManagedWorktree,
    },
}
#[derive(Clone, Serialize, Deserialize)]
struct Seed {
    revision: u64,
    path: PathBuf,
    identity: Identity,
    directories: Vec<String>,
    counts: Counts,
    /// The digest of what was cloned; an unchanged source is not cloned again.
    #[serde(default)]
    digest: Option<String>,
    /// When this seed was taken or last found unchanged (Unix seconds).
    #[serde(default)]
    verified_at: u64,
}
/// What a published seed holds, as recorded when it was taken: logical bytes,
/// not a promise of reclaimable space (its blocks are shared copy-on-write).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedSummary {
    pub bytes: u64,
    pub files: u64,
    pub verified_at: u64,
}
/// Reads the pointer only: no walk, no Git.
pub fn seed_summary(store: &Path, main_identity: &Identity) -> io::Result<Option<SeedSummary>> {
    Ok(
        policy::read::<Seed>(&seed_pointer(store, main_identity)?)?.map(|seed| SeedSummary {
            bytes: seed.counts.bytes,
            files: seed.counts.files,
            verified_at: seed.verified_at,
        }),
    )
}
pub fn run() -> io::Result<()> {
    // This helper starts no descendants. EOF means its host died: stop promptly
    // and let the next host recover its durable transaction before admission.
    unsafe {
        libc::umask(0o077);
    }
    if std::env::var("PLAIT_BUILD_LIFETIME_FD").as_deref() == Ok("4") {
        std::thread::spawn(|| {
            use std::os::fd::FromRawFd;
            let mut lifetime = unsafe { fs::File::from_raw_fd(4) };
            let mut byte = [0];
            let _ = lifetime.read(&mut byte);
            std::process::exit(75);
        });
    }
    let mut bytes = Vec::new();
    io::stdin().take(65537).read_to_end(&mut bytes)?;
    let result = if bytes.len() > 65536 {
        Err(io::Error::other("Build output request is too large."))
    } else {
        serde_json::from_slice::<Request>(&bytes)
            .map_err(io::Error::other)
            .and_then(execute)
    };
    emit(&match result {
        Ok(value) => json!({"value":value}),
        Err(e) => json!({"error":build_output::cold_reason(&e)}),
    })
}
fn emit(value: &Value) -> io::Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()
}
fn progress(counts: &Counts) {
    let _ = emit(&json!({"progress":counts}));
}
fn execute(request: Request) -> io::Result<Value> {
    let value = match request {
        Request::Info { store, root } => {
            let (main, identity) = policy::project(&root)?;
            let policy = policy::load(&store, &main, &identity)?;
            let suggestions = git::build_output::suggestions(&main).map_err(io::Error::other)?;
            let seed = seed_summary(&store, &identity)?;
            json!({"schema":1,"worktree_build_output_v1":true,"policy":policy,"suggestions":suggestions,"seed":seed})
        }
        Request::Set {
            store,
            root,
            identity,
            revision,
            paths,
        } => {
            let policy = policy::set(&store, &root, &identity, revision, paths)?;
            let seed = seed_summary(&store, &policy.identity)?;
            json!({"schema":1,"worktree_build_output_v1":true,"policy":policy,"suggestions":[],"seed":seed})
        }
        Request::Seed {
            store,
            root,
            identity,
            generation,
            source_cache,
            force,
        } => serde_json::to_value(seed(
            &store,
            &root,
            &identity,
            &generation,
            source_cache.as_deref(),
            force,
        )?)?,
        Request::Clone {
            store,
            worktree,
            private_target,
            retry_seedless,
        } => {
            let (outcome, seedless) =
                transaction::prepare(&store, &worktree, private_target, retry_seedless)?;
            json!({"outcome":outcome,"seedless":seedless})
        }
        Request::Recover { store, worktree } => {
            serde_json::to_value(transaction::recover(&store, &worktree)?)?
        }
    };
    Ok(value)
}
fn uuid(id: &str) -> io::Result<()> {
    if id.len() != 36
        || !id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
    {
        return Err(io::Error::other("Invalid build output identity."));
    }
    Ok(())
}
/// Opens an owner-only lock file. Without `create`, a missing file is `None`.
fn lock_file(path: &Path, create: bool) -> io::Result<Option<fs::File>> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if !create && e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
    {
        return Err(io::Error::other("Unsafe build seed lock."));
    }
    Ok(Some(file))
}
fn flock(file: &fs::File, operation: libc::c_int) -> io::Result<bool> {
    loop {
        // SAFETY: an open lock file descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EWOULDBLOCK) => return Ok(false),
            _ => return Err(error),
        }
    }
}
/// One seed refresh per project at a time. Creation never takes this lock.
fn seeder_lock(store: &Path, identity: &Identity) -> io::Result<fs::File> {
    let folder = policy::folder(store, "build-output-seeds")?;
    let file = lock_file(
        &folder.join(format!("{}.seeding.lock", policy::key(identity)?)),
        true,
    )?
    .unwrap();
    if !flock(&file, libc::LOCK_EX | libc::LOCK_NB)? {
        return Err(io::Error::other(
            "Another build seed refresh is active for this project.",
        ));
    }
    Ok(file)
}
/// Each seed has its own lock: a creation holds it shared while cloning, and
/// deleting a retired seed waits for it exclusively. A refresh never blocks a
/// creation from the published seed, and a deletion never removes one in use.
fn generation_lock(seed: &Path) -> PathBuf {
    let mut name = seed.file_name().unwrap_or_default().to_owned();
    name.push(".lock");
    seed.with_file_name(name)
}
/// The published seed, held shared. A pointer that moves while it is being
/// locked is read again, so a concurrent refresh or deletion cannot make
/// creation start cold while a seed is published.
fn published_seed(store: &Path, identity: &Identity) -> io::Result<Option<(Seed, fs::File)>> {
    let pointer = seed_pointer(store, identity)?;
    for _ in 0..8 {
        let Some(seed) = policy::read::<Seed>(&pointer)? else {
            return Ok(None);
        };
        let Some(lock) = lock_file(&generation_lock(&seed.path), false)? else {
            if policy::read::<Seed>(&pointer)?.is_some_and(|now| now.path == seed.path) {
                return Ok(None);
            }
            continue;
        };
        if flock(&lock, libc::LOCK_SH | libc::LOCK_NB)?
            && policy::read::<Seed>(&pointer)?.is_some_and(|now| now.path == seed.path)
        {
            return Ok(Some((seed, lock)));
        }
    }
    Ok(None)
}
fn seed_pointer(store: &Path, identity: &Identity) -> io::Result<PathBuf> {
    Ok(policy::folder(store, "build-output-seeds")?
        .join(format!("{}.json", policy::key(identity)?)))
}
fn private_cache(store: &Path, thread: &str) -> io::Result<PathBuf> {
    uuid(thread)?;
    let parent = policy::folder(store, "build-cache")?;
    let root = parent.join(thread);
    policy::private_directory(&root, true)?;
    Ok(root)
}
const MAIN_ONLY: &str = "Build seeds are taken only from the project's main checkout.";
/// A run-end refresh is skipped while the seed matches the approval and was
/// taken, or found unchanged, within this bound. What makes a seed worth having
/// is dependency output, which changes when a lockfile does, not on every run;
/// a new worktree rebuilds the workspace's own artifacts anyway because their
/// fingerprints carry the checkout path. An hour-old seed is nearly as good as
/// a fresh one, and a full clone after every run is disk churn for nothing.
pub const SEED_FRESH: std::time::Duration = std::time::Duration::from_secs(60 * 60);
/// `checked_at` (Unix seconds) is when this seed was last taken or verified, so
/// the host can skip starting this helper until it is older than `SEED_FRESH`.
#[derive(Serialize)]
struct SeedReply {
    outcome: Outcome,
    checked_at: u64,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn fresh(seed: &Seed, now: u64) -> bool {
    // A clock moved backwards counts as stale, never as fresh forever.
    seed.verified_at <= now && now - seed.verified_at < SEED_FRESH.as_secs()
}
fn seed_outcome(seed: &Seed, reason: &str) -> Outcome {
    Outcome {
        warm: !seed.directories.is_empty(),
        reason: reason.into(),
        directories: seed.directories.clone(),
        counts: seed.counts.clone(),
    }
}
/// One approved directory's source: absent, or its verified open directory.
struct Source {
    directory: String,
    found: Option<(build_output::DirectoryFd, PathBuf, Identity)>,
}
fn sources(
    store: &Path,
    root: &Path,
    identity: &Identity,
    approved: &policy::Policy,
    source_cache: Option<&str>,
) -> io::Result<Vec<Source>> {
    let mut sources = Vec::new();
    for directory in &approved.paths {
        git::build_output::ignored(root, directory).map_err(io::Error::other)?;
        let source_root = if directory == "target" {
            source_cache
                .map(|id| private_cache(store, id))
                .transpose()?
                .unwrap_or(root.into())
        } else {
            root.into()
        };
        let source_identity = if source_root == root {
            identity.clone()
        } else {
            Identity::read(&source_root).map_err(io::Error::other)?
        };
        let found = match build_output::directory_owned(
            &source_root,
            Path::new(directory),
            identity_pair(&source_identity)?,
        ) {
            Ok((fd, _)) => Some((fd, source_root, source_identity)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        sources.push(Source {
            directory: directory.clone(),
            found,
        });
    }
    Ok(sources)
}
/// The whole seed's digest: approval revision, and for each directory its
/// source root and identity (a different private cache is a different seed)
/// and its tree digest, or that it was absent.
fn combine(revision: u64, parts: &[(&Source, Option<build_output::Digest>)]) -> String {
    use sha2::Digest as _;
    let mut hash = sha2::Sha256::new();
    hash.update(revision.to_le_bytes());
    for (source, digest) in parts {
        hash.update(source.directory.as_bytes());
        hash.update([0]);
        if let (Some((_, root, identity)), Some(digest)) = (&source.found, digest) {
            hash.update(root.as_os_str().as_encoded_bytes());
            hash.update([0]);
            hash.update(format!("{}:{}", identity.dev, identity.ino));
            hash.update(digest.hex());
        } else {
            hash.update(b"absent");
        }
        hash.update([0]);
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn unchanged_digest(
    sources: &[Source],
    revision: u64,
    cancelled: &AtomicBool,
) -> io::Result<String> {
    let mut parts = Vec::new();
    for source in sources {
        let digest = match &source.found {
            Some((fd, _, _)) => Some(build_output::fingerprint(fd, cancelled)?),
            None => None,
        };
        parts.push((source, digest));
    }
    Ok(combine(revision, &parts))
}
fn seed(
    store: &Path,
    root: &Path,
    identity: &Identity,
    generation: &str,
    source_cache: Option<&str>,
    force: bool,
) -> io::Result<SeedReply> {
    uuid(generation)?;
    let now = now();
    let (main, main_identity) = policy::project(root)?;
    // A linked worktree's output is one thread's: it must not replace a fuller
    // seed, nor become every later thread's starting cache.
    if main != root {
        return Ok(SeedReply {
            outcome: Outcome::cold(MAIN_ONLY),
            checked_at: now,
        });
    }
    let _seeder = seeder_lock(store, &main_identity)?;
    let approved = policy::load(store, &main, &main_identity)?;
    let pointer = seed_pointer(store, &main_identity)?;
    if approved.paths.is_empty() {
        // A seed pins the old blocks once the source is rebuilt: with nothing
        // approved it is only the person's disk, so it goes.
        remove_project_seed(&pointer, &main_identity)?;
        return Ok(SeedReply {
            outcome: Outcome::cold("No build directories are approved for this project."),
            checked_at: now,
        });
    }
    let previous: Option<Seed> = policy::read(&pointer)?;
    let current = previous
        .as_ref()
        .filter(|seed| seed.revision == approved.revision);
    // A fresh seed needs no lease, no Git and no walk.
    if let Some(seed) = current
        && !force
        && fresh(seed, now)
    {
        return Ok(SeedReply {
            outcome: seed_outcome(seed, "The build seed is less than an hour old."),
            checked_at: seed.verified_at,
        });
    }
    let folder = pointer.parent().unwrap();
    let key = policy::key(&main_identity)?;
    let pending = folder.join(format!("{key}.pending.json"));
    let retired = folder.join(format!("{key}.retired.json"));
    // An abandoned capture joins the retired list; it is deleted after this
    // refresh, so a creation waiting for a seed never waits for it.
    if let Some(old) = policy::read::<Seed>(&pending)? {
        if previous
            .as_ref()
            .is_none_or(|committed| committed.path != old.path)
        {
            retire(&retired, old)?;
        }
        fs::remove_file(&pending)?;
    }
    let capture = lease::seed::capture(
        root,
        identity.lease().map_err(io::Error::other)?,
        std::env::var("PLAIT_BUILD_LIFETIME_FD").as_deref() == Ok("4"),
    )
    .map_err(io::Error::other)?;
    let result = (|| {
        let sources = sources(store, root, identity, &approved, source_cache)?;
        if let Some(seed) = current
            && seed.digest.is_some()
            && unchanged_digest(&sources, approved.revision, capture.cancelled())
                .is_ok_and(|digest| seed.digest.as_deref() == Some(digest.as_str()))
        {
            let mut seed = seed.clone();
            seed.verified_at = now;
            policy::save(&pointer, &seed)?;
            return Ok(seed_outcome(
                &seed,
                "Build output has not changed since the seed was taken.",
            ));
        }
        capture_seed(
            store,
            root,
            identity,
            &approved,
            &sources,
            (folder, &key, generation, now),
            (&pointer, &pending, &retired),
            previous.as_ref(),
            capture.cancelled(),
        )
    })();
    drop(capture);
    // Deletion holds no source lease and no lock a creation needs; it waits
    // only for a creation still cloning from the seed being retired.
    let checked_at = now;
    match (result, delete_retired(folder, &retired)) {
        (Ok(mut outcome), Err(error)) => {
            outcome
                .counts
                .note(format!("An earlier build seed is not deleted yet: {error}"));
            Ok(SeedReply {
                outcome,
                checked_at,
            })
        }
        (result, _) => result.map(|outcome| SeedReply {
            outcome,
            checked_at,
        }),
    }
}
#[allow(clippy::too_many_arguments)]
fn capture_seed(
    store: &Path,
    root: &Path,
    identity: &Identity,
    approved: &policy::Policy,
    sources: &[Source],
    (folder, key, generation, now): (&Path, &str, &str, u64),
    (pointer, pending, retired): (&Path, &Path, &Path),
    previous: Option<&Seed>,
    cancelled: &AtomicBool,
) -> io::Result<Outcome> {
    let path = folder.join(format!("{key}-{generation}"));
    fs::create_dir(&path)?;
    lock_file(&generation_lock(&path), true)?;
    let mut seed = Seed {
        revision: approved.revision,
        identity: Identity::read(&path).map_err(io::Error::other)?,
        path,
        directories: Vec::new(),
        counts: Counts::default(),
        digest: None,
        verified_at: now,
    };
    policy::save(pending, &seed)?;
    let mut published = false;
    let result = (|| {
        let mut parts = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            let Some((fd, _, _)) = &source.found else {
                seed.counts
                    .note(format!("{}: no build output yet", source.directory));
                parts.push((source, None));
                continue;
            };
            let destination = seed.path.join(index.to_string());
            fs::create_dir(&destination)?;
            let output = build_output::directory(&destination, Path::new(""))?;
            let (counts, digest) =
                build_output::clone_tree_digest(fd, &output.0, cancelled, &mut progress)?;
            merge(&mut seed.counts, &counts);
            seed.directories.push(source.directory.clone());
            parts.push((source, Some(digest)));
        }
        seed.digest = Some(combine(approved.revision, &parts));
        identity.checked(root).map_err(io::Error::other)?;
        let (main, main_identity) = (&approved.project, &approved.identity);
        // A policy edit during the scan cannot publish a widened or stale seed.
        if policy::load(store, main, main_identity)?.revision != approved.revision {
            return Err(io::Error::other(
                "Build output approval changed during seeding.",
            ));
        }
        if let Some(previous) = previous {
            retire(retired, previous.clone())?;
        }
        policy::save(pointer, &seed)?;
        published = true;
        fs::remove_file(pending)?;
        Ok(seed_outcome(
            &seed,
            "An idle checkout supplied an immutable build seed.",
        ))
    })();
    if result.is_err() && !published {
        remove_seed(folder, &seed)?;
        let _ = fs::remove_file(pending);
    }
    result
}
fn remove_project_seed(pointer: &Path, main_identity: &Identity) -> io::Result<()> {
    let folder = pointer.parent().unwrap();
    let key = policy::key(main_identity)?;
    let retired = folder.join(format!("{key}.retired.json"));
    let pending = folder.join(format!("{key}.pending.json"));
    for journal in [pointer, pending.as_path()] {
        if let Some(seed) = policy::read::<Seed>(journal)? {
            retire(&retired, seed)?;
            fs::remove_file(journal)?;
        }
    }
    delete_retired(folder, &retired)
}
/// Seeds waiting to be deleted. A list, so an interrupted deletion is resumed
/// by the next refresh and no abandoned seed is forgotten.
fn retire(list: &Path, seed: Seed) -> io::Result<()> {
    let mut seeds: Vec<Seed> = policy::read(list)?.unwrap_or_default();
    if !seeds.iter().any(|s| s.path == seed.path) {
        seeds.push(seed);
    }
    policy::save(list, &seeds)
}
fn delete_retired(folder: &Path, list: &Path) -> io::Result<()> {
    let mut seeds: Vec<Seed> = policy::read(list)?.unwrap_or_default();
    while let Some(seed) = seeds.first() {
        remove_seed(folder, seed)?;
        seeds.remove(0);
        if seeds.is_empty() {
            fs::remove_file(list)?;
        } else {
            policy::save(list, &seeds)?;
        }
    }
    Ok(())
}
fn remove_seed(folder: &Path, seed: &Seed) -> io::Result<()> {
    if seed.path.parent() != Some(folder) {
        return Err(io::Error::other("Invalid build seed path."));
    }
    let lock_path = generation_lock(&seed.path);
    let lock = lock_file(&lock_path, false)?;
    if let Some(lock) = &lock {
        flock(lock, libc::LOCK_EX)?;
    }
    build_output::remove_owned(
        folder,
        Path::new(seed.path.file_name().unwrap()),
        identity_pair(&seed.identity)?,
    )?;
    if lock.is_some() {
        fs::remove_file(lock_path)?;
    }
    Ok(())
}
fn identity_pair(identity: &Identity) -> io::Result<(u64, u64)> {
    let id = identity.lease().map_err(io::Error::other)?;
    Ok((id.dev, id.ino))
}
fn merge(total: &mut Counts, next: &Counts) {
    total.files += next.files;
    total.links += next.links;
    total.skipped += next.skipped;
    total.bytes += next.bytes;
    total.elapsed_ms += next.elapsed_ms;
    total.notes.extend(
        next.notes
            .iter()
            .take(20usize.saturating_sub(total.notes.len()))
            .cloned(),
    );
}

#[cfg(test)]
mod tests;
