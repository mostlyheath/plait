//! Host-side dispatch for Git and file operations.
use crate::git::WriteOutcome;
use crate::{file_view, git, lease::DirIdentity};
use base64::{Engine, engine::general_purpose::STANDARD};
use plait_protocol::host::git::{FileBytes, HelperInput, Outcome, Read, WriteInput};
use serde::Serialize;
use std::{os::unix::fs::MetadataExt, path::Path};

/// Blocking, on the host. Runs one write under the checkout's leases, as the
/// desktop does for this Mac's checkouts, and serializes its outcome.
pub fn execute_write(input: &WriteInput) -> Vec<u8> {
    let outcome = match write_checked(input) {
        Ok((identity, result)) => Outcome::Ok {
            identity: identity.into(),
            result,
        },
        Err(message) => Outcome::Error { message },
    };
    serde_json::to_vec(&outcome).unwrap_or_default()
}

fn write_checked(input: &WriteInput) -> Result<(DirIdentity, WriteOutcome), String> {
    if !input.checkout.is_absolute() {
        return Err("The checkout path must be absolute.".into());
    }
    let expected = input.identity.parse()?;
    if identity(&input.checkout)? != expected {
        return Err("Checkout identity changed.".into());
    }
    if git::toplevel(&input.checkout).and_then(|top| identity(&top))? != expected {
        return Err("Open the top folder of the checkout.".into());
    }
    let outcome = git::write_checked(&input.checkout, Some(expected), &input.write)?;
    Ok((expected, outcome))
}

/// Blocking, on the host. Runs one read and serializes its outcome.
pub fn execute(input: &HelperInput) -> Vec<u8> {
    let outcome = checked(input);
    serde_json::to_vec(&outcome).unwrap_or_else(|_| {
        serde_json::to_vec(&Outcome::<()>::Error {
            message: "The host could not encode this read.".into(),
        })
        .unwrap_or_default()
    })
}

fn checked(input: &HelperInput) -> Outcome<serde_json::Value> {
    match run_checked(input) {
        Ok((identity, result)) => Outcome::Ok {
            identity: identity.into(),
            result,
        },
        Err(message) => Outcome::Error { message },
    }
}

/// The checkout is the top of a work tree with the expected identity, before
/// and after the read, so a replaced folder never answers for the old one.
fn run_checked(input: &HelperInput) -> Result<(DirIdentity, serde_json::Value), String> {
    let root = &input.checkout;
    if !root.is_absolute() {
        return Err("The checkout path must be absolute.".into());
    }
    let before = identity(root)?;
    if let Some(expected) = &input.identity
        && expected.parse()? != before
    {
        return Err("Checkout identity changed.".into());
    }
    // A plain folder has no work tree; its identity checks still apply.
    if !git::plain_folder(root) {
        let top = git::toplevel(root)?;
        if identity(&top)? != before {
            return Err("Open the top folder of the checkout.".into());
        }
    }
    let result = match &input.read {
        Read::WorktreeSize { .. } => value(measure_size(input)),
        read => dispatch(root, read),
    }?;
    if identity(root)? != before {
        return Err("Checkout identity changed.".into());
    }
    Ok((before, result))
}

fn identity(path: &Path) -> Result<DirIdentity, String> {
    let meta =
        std::fs::symlink_metadata(path).map_err(|_| "The checkout is missing.".to_string())?;
    if !meta.is_dir() {
        return Err("The checkout is not a folder.".into());
    }
    Ok(DirIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

fn measure_size(input: &HelperInput) -> Result<crate::worktree_size::Size, String> {
    use crate::worktree_size::{self as size, Key};
    let Read::WorktreeSize {
        worktree: root,
        worktree_identity: identity,
        thread,
        removed,
        others,
        build_directories,
    } = &input.read
    else {
        return Err("Not a size read.".into());
    };
    let store = input
        .store
        .as_ref()
        .ok_or("The host did not name its store.")?;
    // The thread names a folder in the store, so only an id's characters pass.
    let cache = match thread {
        Some(id) if valid_thread(id) => Some(store.join("build-cache").join(id)),
        Some(_) => return Err("Invalid thread.".into()),
        None => None,
    };
    let never = std::sync::atomic::AtomicBool::new(false);
    if *removed {
        return size::measure_retained_cache(cache.as_deref(), &never);
    }
    if !root.is_absolute() {
        return Err("The worktree path must be absolute.".into());
    }
    let key = Key {
        root: root.clone(),
        identity: identity.as_ref().map(|id| git::worktree::Identity {
            dev: id.dev.clone(),
            ino: id.ino.clone(),
        }),
        head: None,
        last_used: None,
        changed: None,
        removed: false,
        build_directories: build_directories.clone(),
    };
    let skip = others
        .iter()
        .map(|id| id.parse().map(|id| (id.dev, id.ino)))
        .collect::<Result<_, _>>()?;
    size::measure_verified(&key, cache.as_deref(), &skip, &never)
}

fn valid_thread(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn value<T: Serialize>(result: Result<T, String>) -> Result<serde_json::Value, String> {
    serde_json::to_value(result?).map_err(|_| "The host could not encode this read.".into())
}

fn dispatch(root: &Path, read: &Read) -> Result<serde_json::Value, String> {
    match read {
        Read::Status => value(git::status(root)),
        Read::Diff { change, mode } => value(git::diff(root, &change.clone().into(), *mode)),
        Read::Files => value(git::files(root)),
        Read::Log { tip, skip, limit } => value(git::log::log(root, tip.as_deref(), *skip, *limit)),
        Read::Show { id } => value(git::log::show(root, id)),
        Read::Branches => value(git::branches(root)),
        Read::LineCounts { paths } => value(git::line_counts(root, paths)),
        Read::Graph { heads } => value(git::log::graph(root, heads)),
        Read::File { path } => {
            value(
                file_view::read_bytes(root, path).map(|(stamp, bytes)| FileBytes {
                    stamp,
                    base64: bytes.map(|b| STANDARD.encode(b)),
                }),
            )
        }
        Read::FileStamp { path } => value(file_view::stamp(root, path)),
        Read::WorktreeSize { .. } => Err("Not a plain read.".into()),
    }
}

#[cfg(test)]
#[path = "host_git_tests.rs"]
mod tests;
