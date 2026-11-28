//! Durable exclusion when process cleanup cannot be confirmed. All changes
//! require the checkout writer lease; a separate short lock makes reads atomic.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, Write};

pub use plait_protocol::lease::cleanup::{
    Cleanup, CleanupProcess, RESTART_REASON, STOP_UNCONFIRMED,
};

fn locked_file(lease_dir: &Path, id: DirIdentity) -> Result<File, LeaseError> {
    let (_, directory) = open_lease_dir(lease_dir)?;
    let (file, _) = open_lock_file(
        &directory,
        &format!("cleanup-{:016x}-{:016x}.lock", id.dev, id.ino),
    )?;
    if !flock_fd_wait(file.as_raw_fd(), libc::LOCK_EX, Duration::from_secs(2))? {
        return Err(LeaseError::io("Cleanup state is being saved. Try again."));
    }
    Ok(file)
}

fn read(file: &mut File) -> Result<Option<Cleanup>, LeaseError> {
    let mut bytes = Vec::new();
    file.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|e| LeaseError::io(e.to_string()))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() <= 8192
        && let Ok(value) = serde_json::from_slice::<Cleanup>(&bytes)
        && valid(&value)
    {
        return Ok(Some(value));
    }
    // A torn save is uncertainty too. Its digest lets a person acknowledge that
    // exact damaged state without letting an old confirmation clear a new run.
    Ok(Some(Cleanup {
        process: None,
        token: format!("{:x}", Sha256::digest(&bytes)),
        reason: "The host could not read its saved cleanup confirmation.".into(),
    }))
}

fn valid(value: &Cleanup) -> bool {
    value.process.is_none()
        && !value.token.is_empty()
        && value.token.len() <= 128
        && value
            .token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        && !value.reason.is_empty()
        && value.reason.len() <= 1024
        && !value.reason.chars().any(char::is_control)
}

pub fn state(id: DirIdentity) -> Result<Option<Cleanup>, LeaseError> {
    state_in(&default_lease_dir()?, id)
}

pub(super) fn state_in(dir: &Path, id: DirIdentity) -> Result<Option<Cleanup>, LeaseError> {
    read(&mut locked_file(dir, id)?)
}

pub fn owner_lock_name(id: DirIdentity) -> String {
    format!("cleanup-owner-{:016x}-{:016x}.lock", id.dev, id.ino)
}

/// Worktree creation already holds a lifecycle reader in its busy parent. Only
/// a live runtime's crash marker may coexist with that operation, never an
/// uncertain cleanup marker or a marker abandoned by a crashed runtime.
pub fn require_clear_for_reader(dir: &Path, id: DirIdentity) -> Result<(), LeaseError> {
    if let Some(state) = unconfirmed_in(dir, id)? {
        return Err(LeaseError::new(
            LeaseErrorKind::CleanupUncertain,
            state.message(),
        ));
    }
    Ok(())
}

/// A live run's crash marker is not an unconfirmed stop, even in another store.
pub fn unconfirmed(id: DirIdentity) -> Result<Option<Cleanup>, LeaseError> {
    if let Some(state) = super::git_cleanup::unconfirmed(id)? {
        return Ok(Some(state));
    }
    unconfirmed_in(&default_lease_dir()?, id)
}

fn unconfirmed_in(dir: &Path, id: DirIdentity) -> Result<Option<Cleanup>, LeaseError> {
    let state = state_in(dir, id)?;
    if state
        .as_ref()
        .is_some_and(|state| state.reason == RESTART_REASON)
    {
        let (_, directory) = open_lease_dir(dir)?;
        let (owner, _) = open_lock_file(&directory, &owner_lock_name(id))?;
        if !try_flock_fd(owner.as_raw_fd(), libc::LOCK_EX)? {
            return Ok(None);
        }
    }
    Ok(state)
}

pub fn require_clear(id: DirIdentity) -> Result<(), LeaseError> {
    require_clear_in(&default_lease_dir()?, id)
}

pub(super) fn require_clear_in(dir: &Path, id: DirIdentity) -> Result<(), LeaseError> {
    if let Some(state) = state_in(dir, id)? {
        return Err(LeaseError::new(
            LeaseErrorKind::CleanupUncertain,
            state.message(),
        ));
    }
    Ok(())
}

/// Compare and save under the already-held checkout lease. A stale callback or
/// confirmation must never erase another run's marker.
pub fn replace(
    id: DirIdentity,
    expected: Option<&str>,
    next: Option<&Cleanup>,
) -> Result<(), LeaseError> {
    replace_in(&default_lease_dir()?, id, expected, next)
}

fn replace_in(
    dir: &Path,
    id: DirIdentity,
    expected: Option<&str>,
    next: Option<&Cleanup>,
) -> Result<(), LeaseError> {
    if next.is_some_and(|value| !valid(value)) {
        return Err(LeaseError::io("Invalid cleanup state."));
    }
    let mut file = locked_file(dir, id)?;
    // Only run-marker mutations need power-loss durability, never lease reads.
    open_lease_dir(dir)?
        .1
        .sync_all()
        .map_err(|e| LeaseError::io(e.to_string()))?;
    if read(&mut file)?.as_ref().map(|value| value.token.as_str()) != expected {
        return Err(LeaseError::io(
            "Cleanup state changed. Refresh before confirming.",
        ));
    }
    file.rewind().map_err(|e| LeaseError::io(e.to_string()))?;
    if let Some(next) = next {
        let bytes = serde_json::to_vec(next).map_err(|e| LeaseError::io(e.to_string()))?;
        file.write_all(&bytes)
            .map_err(|e| LeaseError::io(e.to_string()))?;
        file.set_len(bytes.len() as u64)
            .map_err(|e| LeaseError::io(e.to_string()))?;
    } else {
        file.set_len(0).map_err(|e| LeaseError::io(e.to_string()))?;
    }
    file.sync_all().map_err(|e| LeaseError::io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn quarantine_blocks_runs_and_desktop_writes_until_exact_confirmation() {
        let base = plait_test_support::temp_dir().join(format!(
            "plait-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(base.join("repo")).unwrap();
        fs::create_dir_all(base.join("other")).unwrap();
        let dir = base.join("leases");
        let root = base.join("repo");
        let run = acquire_checkout_in(&dir, &root, None, LeaseClass::Writer).unwrap();
        let id = run.identity();
        let marker = Cleanup {
            process: None,
            token: "run-one".into(),
            reason: RESTART_REASON.into(),
        };
        replace_in(&dir, id, None, Some(&marker)).unwrap();
        drop(run); // Kernel lock loss alone must not erase uncertain cleanup.
        for class in [
            LeaseClass::Writer,
            LeaseClass::Reader,
            LeaseClass::Exclusive,
        ] {
            let error = acquire_checkout_in(&dir, &root, None, class).unwrap_err();
            assert_eq!(error.kind, LeaseErrorKind::CleanupUncertain);
            assert!(error.message.contains("Stop unconfirmed"));
        }
        let other =
            acquire_checkout_in(&dir, &base.join("other"), None, LeaseClass::Writer).unwrap();
        let recovery = acquire_for_cleanup_in(&dir, &root, None, LeaseClass::Exclusive).unwrap();
        assert!(replace_in(&dir, id, Some("stale"), None).is_err());
        assert_eq!(state_in(&dir, id).unwrap(), Some(marker));
        replace_in(&dir, id, Some("run-one"), None).unwrap();
        drop(recovery);
        let next = acquire_checkout_in(&dir, &root, None, LeaseClass::Writer).unwrap();
        replace_in(
            &dir,
            id,
            None,
            Some(&Cleanup {
                process: None,
                token: "run-two".into(),
                reason: "A helper did not confirm exit.".into(),
            }),
        )
        .unwrap();
        assert!(replace_in(&dir, id, Some("run-one"), None).is_err());
        replace_in(&dir, id, Some("run-two"), None).unwrap();
        drop((next, other));
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn quarantine_refuses_desktop_git_changes_with_reason_and_resolution_restores_them() {
        use crate::git::{WriteOp, write};
        let base = plait_test_support::temp_dir().join(format!(
            "plait-quarantined-git-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("repo");
        let other = base.join("other");
        for checkout in [&root, &other] {
            fs::create_dir_all(checkout).unwrap();
            assert!(
                std::process::Command::new("git")
                    .args(["init", "-q"])
                    .arg(checkout)
                    .status()
                    .unwrap()
                    .success()
            );
            fs::write(checkout.join("a.txt"), "change").unwrap();
        }
        let lease = acquire_checkout(&root, None, LeaseClass::Writer).unwrap();
        let id = lease.identity();
        replace(
            id,
            None,
            Some(&Cleanup {
                process: None,
                token: "uncertain-command".into(),
                reason: "The command group did not stop.".into(),
            }),
        )
        .unwrap();
        drop(lease);
        let refused: Vec<_> = [
            WriteOp::StageAll,
            WriteOp::Commit {
                message: "blocked".into(),
            },
            WriteOp::CreateBranch("blocked".into()),
            WriteOp::Discard {
                guard: None,
                path: "a.txt".into(),
                untracked: true,
                hunk: None,
            },
        ]
        .iter()
        .map(|op| write(&root, op))
        .collect();
        let unrelated = write(&other, &WriteOp::StageAll);
        let recovery = acquire_for_cleanup(&root).unwrap();
        replace(id, Some("uncertain-command"), None).unwrap();
        drop(recovery);
        let resolved = write(&root, &WriteOp::StageAll);
        fs::remove_dir_all(base).unwrap();
        for result in refused {
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("Stop unconfirmed") && error.contains("command group did not stop"),
                "{error}"
            );
        }
        assert!(unrelated.is_ok());
        assert!(resolved.is_ok());
    }

    #[test]
    fn torn_cleanup_state_fails_closed_and_can_be_confirmed() {
        let base = plait_test_support::temp_dir().join(format!(
            "plait-cleanup-torn-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let id = DirIdentity { dev: 1, ino: 2 };
        let mut file = locked_file(&base, id).unwrap();
        file.write_all(b"{torn").unwrap();
        drop(file);
        assert!(require_clear_in(&base, id).is_err());
        let saved = state_in(&base, id).unwrap().unwrap();
        assert_eq!(saved.token.len(), 64);
        replace_in(&base, id, Some(&saved.token), None).unwrap();
        require_clear_in(&base, id).unwrap();
        fs::remove_dir_all(base).unwrap();
    }
}
