//! Git decides ignored status; a directory name or manifest never grants approval.
use super::{Result, STATUS_LIMIT, TIMEOUT, read_command, run, run_readonly};
use crate::build_output::{directory, relative, validate_tree};
use std::path::{Path, PathBuf};

pub fn validate(root: &Path, value: &str) -> Result<PathBuf> {
    let path = relative(value)?;
    let (fd, _) = directory(root, &path).map_err(|e| format!("Cannot approve {value}: {e}"))?;
    ignored(root, value)?;
    validate_tree(&fd).map_err(|e| format!("Cannot approve {value}: {e}"))?;
    Ok(path)
}
pub fn ignored(root: &Path, value: &str) -> Result<()> {
    relative(value)?;
    let mut command = read_command(root)?;
    // check-ignore accepts filenames, not pathspecs, and rejects Git's global
    // literal-pathspec flag. Keep every other read hardening flag intact.
    let ignored = run(
        command.args([
            "--no-literal-pathspecs",
            "check-ignore",
            "--no-index",
            "-z",
            "--stdin",
        ]),
        Some(format!("{value}/\0").into_bytes()),
        STATUS_LIMIT,
        TIMEOUT,
    )?;
    if ignored.status.code() != Some(0) || ignored.out.bytes != format!("{value}/\0").as_bytes() {
        return Err(format!(
            "{value} must be ignored by Git before it can be approved."
        ));
    }
    let mut command = read_command(root)?;
    let tracked = run_readonly(command.args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        value,
    ]))?;
    if !tracked.is_empty() {
        return Err(format!("{value} contains tracked or non-ignored files."));
    }
    Ok(())
}

pub fn suggestions(root: &Path) -> Result<Vec<String>> {
    let mut command = read_command(root)?;
    let files = run_readonly(command.args(["ls-files", "--cached", "-z"]))?;
    let mut result = Vec::new();
    for file in files
        .split(|b| *b == 0)
        .filter_map(|p| std::str::from_utf8(p).ok())
    {
        let path = Path::new(file);
        let Some(parent) = path.parent() else {
            continue;
        };
        let Ok((parent_fd, _)) = directory(root, parent) else {
            continue;
        };
        let Some(name) = path.file_name().and_then(|p| p.to_str()) else {
            continue;
        };
        if !["Cargo.toml", "package.json"].contains(&name) {
            continue;
        }
        let Some(data) = manifest(&parent_fd, name) else {
            continue;
        };
        // These are suggestions only, so Cargo section names suffice; no build
        // tool, manifest extension or package script is ever evaluated.
        let candidate = if name == "Cargo.toml"
            && data.lines().any(|line| {
                matches!(
                    line.split('#').next().unwrap_or("").trim(),
                    "[package]" | "[workspace]"
                )
            }) {
            "target"
        } else if name == "package.json"
            && serde_json::from_str::<serde_json::Value>(&data).is_ok_and(|value| value.is_object())
            && [
                "package-lock.json",
                "npm-shrinkwrap.json",
                "yarn.lock",
                "pnpm-lock.yaml",
                "bun.lock",
                "bun.lockb",
            ]
            .iter()
            .any(|name| {
                use std::os::fd::AsRawFd;
                let name = std::ffi::CString::new(*name).unwrap();
                crate::worktree_size::stat_at(parent_fd.as_raw_fd(), &name)
                    .is_ok_and(|s| s.st_mode & libc::S_IFMT == libc::S_IFREG)
            })
        {
            "node_modules"
        } else {
            continue;
        };
        let value = parent.join(candidate).to_string_lossy().into_owned();
        if result.len() < crate::build_output::MAX_PATHS
            && !result.contains(&value)
            && validate(root, &value).is_ok()
        {
            result.push(value);
        }
    }
    Ok(result)
}

fn manifest(parent: &std::os::fd::OwnedFd, name: &str) -> Option<String> {
    use std::{
        io::Read,
        os::fd::{AsRawFd, FromRawFd},
    };
    let name = std::ffi::CString::new(name).ok()?;
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return None;
    }
    let file = unsafe { std::fs::File::from_raw_fd(raw) };
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return None;
    }
    let mut data = String::new();
    file.take(1024 * 1024 + 1).read_to_string(&mut data).ok()?;
    (data.len() <= 1024 * 1024).then_some(data)
}
