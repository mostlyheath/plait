use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Installed {
    root: PathBuf,
    root_identity: Identity,
    path: PathBuf,
    identity: Identity,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    checkout: PathBuf,
    identity: Identity,
    staging: PathBuf,
    staging_identity: Option<Identity>,
    installed: Vec<Installed>,
    outcome: Option<Outcome>,
}
fn journal_path(store: &Path, tree: &ManagedWorktree) -> io::Result<PathBuf> {
    uuid(&tree.thread_id)?;
    Ok(policy::folder(store, "build-output-operations")?.join(format!("{}.json", tree.thread_id)))
}
fn verify(tree: &ManagedWorktree, journal: &Journal) -> io::Result<()> {
    if journal.checkout != tree.path
        || tree.identity.as_ref() != Some(&journal.identity)
        || journal.staging != Path::new(&format!(".plait-build-{}", tree.thread_id))
    {
        return Err(io::Error::other(
            "Build output preparation identity changed.",
        ));
    }
    journal
        .identity
        .checked(&journal.checkout)
        .map_err(io::Error::other)
}
fn lease(tree: &ManagedWorktree) -> io::Result<lease::CheckoutLease> {
    lease::acquire_checkout(
        &tree.path,
        Some(
            tree.identity
                .as_ref()
                .ok_or_else(|| io::Error::other("Worktree identity is missing."))?
                .lease()
                .map_err(io::Error::other)?,
        ),
        LeaseClass::Exclusive,
    )
    .map_err(io::Error::other)
}
fn empty(tree: &ManagedWorktree) -> io::Result<Journal> {
    Ok(Journal {
        checkout: tree.path.clone(),
        identity: tree
            .identity
            .clone()
            .ok_or_else(|| io::Error::other("Worktree identity is missing."))?,
        staging: format!(".plait-build-{}", tree.thread_id).into(),
        staging_identity: None,
        installed: Vec::new(),
        outcome: None,
    })
}
pub(super) fn recover(store: &Path, tree: &ManagedWorktree) -> io::Result<Outcome> {
    let _lease = lease(tree)?;
    let path = journal_path(store, tree)?;
    let mut journal: Journal = policy::read(&path)?.unwrap_or(empty(tree)?);
    verify(tree, &journal)?;
    if let Some(result) = journal.outcome {
        return Ok(result);
    }
    rollback(&journal)?;
    let result = Outcome::cold(
        "Build output preparation was skipped or interrupted; the worktree starts cold.",
    );
    journal.outcome = Some(result.clone());
    policy::save(&path, &journal)?;
    Ok(result)
}
/// Returns the outcome, and whether it is cold only because no usable seed
/// exists. With `retry_seedless`, such an attempt records nothing.
pub(super) fn prepare(
    store: &Path,
    tree: &ManagedWorktree,
    private_target: bool,
    retry_seedless: bool,
) -> io::Result<(Outcome, bool)> {
    let _lease = lease(tree)?;
    let path = journal_path(store, tree)?;
    if let Some(mut journal) = policy::read::<Journal>(&path)? {
        verify(tree, &journal)?;
        if let Some(result) = journal.outcome {
            return Ok((result, false));
        }
        rollback(&journal)?;
        let result =
            Outcome::cold("Build output preparation was interrupted; the worktree starts cold.");
        journal.outcome = Some(result.clone());
        policy::save(&path, &journal)?;
        return Ok((result, false));
    }
    let mut journal = empty(tree)?;
    policy::save(&path, &journal)?;
    let start = std::time::Instant::now();
    let result = fill(store, tree, private_target, &path, &mut journal);
    let (mut outcome, seedless) = match result {
        Ok(result) => result,
        Err(error) => {
            rollback(&journal)?;
            (Outcome::cold(build_output::cold_reason(&error)), false)
        }
    };
    if seedless && retry_seedless {
        // Nothing was staged before the seed check: forget this attempt.
        fs::remove_file(&path)?;
        return Ok((outcome, true));
    }
    outcome.counts.elapsed_ms = start.elapsed().as_millis() as u64;
    journal.outcome = Some(outcome.clone());
    policy::save(&path, &journal)?;
    Ok((outcome, seedless))
}
fn fill(
    store: &Path,
    tree: &ManagedWorktree,
    private_target: bool,
    journal_path: &Path,
    journal: &mut Journal,
) -> io::Result<(Outcome, bool)> {
    let approved = policy::load(store, &tree.parent.main, &tree.parent.main_identity)?;
    if approved.paths.is_empty() {
        return Ok((
            Outcome::cold("No build directories are approved for this project."),
            false,
        ));
    }
    let Some((seed, _lock)) = published_seed(store, &tree.parent.main_identity)? else {
        return Ok((
            Outcome::cold(
                "The source checkout is busy or unavailable and there is no idle build seed.",
            ),
            true,
        ));
    };
    if seed.revision != approved.revision {
        return Ok((
            Outcome::cold("The build seed does not match the project's current approval."),
            true,
        ));
    }
    seed.identity
        .checked(&seed.path)
        .map_err(io::Error::other)?;
    if seed.directories.is_empty() {
        return Ok((
            Outcome::cold("Approved directories have no build output yet."),
            true,
        ));
    }
    let stage = tree.path.join(&journal.staging);
    fs::create_dir(&stage)?;
    journal.staging_identity = Some(Identity::read(&stage).map_err(io::Error::other)?);
    policy::save(journal_path, journal)?;
    let mut outcome = Outcome {
        warm: false,
        reason: "Build output was cloned copy-on-write from an idle seed.".into(),
        directories: Vec::new(),
        counts: Counts {
            skipped: seed.counts.skipped,
            notes: seed.counts.notes.clone(),
            ..Counts::default()
        },
    };
    for (index, directory) in approved.paths.iter().enumerate() {
        if !seed.directories.contains(directory) {
            outcome
                .counts
                .note(format!("{directory}: no build output in the seed"));
            continue;
        }
        // Git and path checks in the new checkout run again: a different HEAD
        // may no longer ignore this directory, or a hook may have created it.
        git::build_output::ignored(&tree.path, directory).map_err(io::Error::other)?;
        let root = if private_target && directory == "target" {
            private_cache(store, &tree.thread_id)?
        } else {
            tree.path.clone()
        };
        if fs::symlink_metadata(root.join(directory)).is_ok() {
            return Err(io::Error::other(format!(
                "{directory} already exists in the destination; it was preserved."
            )));
        }
        let source = build_output::directory_owned(
            &seed.path,
            Path::new(&index.to_string()),
            identity_pair(&seed.identity)?,
        )?;
        let local = stage.join(index.to_string());
        fs::create_dir(&local)?;
        let output = build_output::directory(&local, Path::new(""))?;
        let prior = outcome.counts.clone();
        let counts =
            build_output::clone_tree(&source.0, &output.0, &AtomicBool::new(false), &mut |next| {
                let mut total = prior.clone();
                merge(&mut total, next);
                progress(&total);
            })?;
        merge(&mut outcome.counts, &counts);
        journal.installed.push(Installed {
            root: root.clone(),
            root_identity: if root == tree.path {
                journal.identity.clone()
            } else {
                Identity::read(&root).map_err(io::Error::other)?
            },
            path: directory.into(),
            identity: Identity::read(&local).map_err(io::Error::other)?,
        });
        // A durable intention precedes the rename; recovery removes it on either side.
        policy::save(journal_path, journal)?;
        let intention = journal.installed.last().unwrap();
        build_output::install(
            &stage,
            identity_pair(journal.staging_identity.as_ref().unwrap())?,
            Path::new(&index.to_string()),
            &root,
            identity_pair(&intention.root_identity)?,
            Path::new(directory),
            identity_pair(&intention.identity)?,
        )?;
        outcome.directories.push(directory.clone());
    }
    if policy::load(store, &tree.parent.main, &tree.parent.main_identity)?.revision
        != approved.revision
    {
        return Err(io::Error::other(
            "Build output approval changed during cloning.",
        ));
    }
    build_output::remove_owned(
        &tree.path,
        &journal.staging,
        identity_pair(journal.staging_identity.as_ref().unwrap())?,
    )?;
    outcome.warm = !outcome.directories.is_empty();
    if !outcome.warm {
        outcome.reason = "Approved directories have no build output yet.".into();
    }
    Ok((outcome, false))
}
fn rollback(journal: &Journal) -> io::Result<()> {
    journal
        .identity
        .checked(&journal.checkout)
        .map_err(io::Error::other)?;
    for installed in journal.installed.iter().rev() {
        installed
            .root_identity
            .checked(&installed.root)
            .map_err(io::Error::other)?;
        // Before the rename this path is absent; after it the recorded inode owns it.
        if fs::symlink_metadata(installed.root.join(&installed.path)).is_ok() {
            build_output::remove_owned(
                &installed.root,
                &installed.path,
                identity_pair(&installed.identity)?,
            )?;
        }
    }
    if let Some(identity) = &journal.staging_identity {
        build_output::remove_owned(
            &journal.checkout,
            &journal.staging,
            identity_pair(identity)?,
        )?;
    } else {
        // A crash after mkdir but before recording its inode can leave only an
        // empty staging directory: filling starts after the identity is synced.
        match fs::remove_dir(journal.checkout.join(&journal.staging)) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
