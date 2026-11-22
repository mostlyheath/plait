//! Immutable per-path record reads. Never opens the recorder's mutable index.
use super::{DIFF_LIMIT, Result, TIMEOUT, record, run};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Command, time::Instant};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub store: PathBuf,
    pub checkout: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub object_format: String,
    pub changed: record::ChangedPath,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    pub mode: String,
    pub fingerprint: String,
    pub bytes: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Hunk {
    pub first_line: usize,
    pub last_line: usize,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Read {
    pub before: Option<Entry>,
    pub after: Option<Entry>,
    pub status: String,
    pub patch: Vec<u8>,
    pub hunks: Vec<Hunk>,
    pub truncated: bool,
    pub elapsed_ms: u64,
    pub processes: usize,
}

pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.chars().any(char::is_control)
        && !path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.eq_ignore_ascii_case(".git"))
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        if !self.store.is_absolute()
            || !self.checkout.is_absolute()
            || !valid_path(&self.changed.path)
        {
            return Err("Invalid recorded path.".into());
        }
        let size = match self.object_format.as_str() {
            "sha1" => 40,
            "sha256" => 64,
            _ => return Err("Invalid object format.".into()),
        };
        for (mode, oid) in [
            (&self.changed.before_mode, &self.changed.before_oid),
            (&self.changed.after_mode, &self.changed.after_oid),
        ] {
            match (mode, oid) {
                (None, None) => (),
                (Some(mode), Some(oid))
                    if matches!(mode.as_str(), "100644" | "100755" | "120000")
                        && oid.len() == size
                        && oid.bytes().all(|c| c.is_ascii_hexdigit()) => {}
                _ => return Err("Invalid recorded entry.".into()),
            }
        }
        let root =
            std::fs::symlink_metadata(&self.checkout).map_err(|_| "Checkout is unavailable.")?;
        if !root.is_dir()
            || root.dev() != self.dev
            || root.ino() != self.ino
            || self.checkout.canonicalize().ok().as_ref() != Some(&self.checkout)
        {
            return Err("Checkout identity changed.".into());
        }
        // We read no working path or symlink target, only the saved blob IDs.
        Ok(())
    }
    fn git(&self) -> Command {
        let mut command = Command::new(record::git_binary());
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", "/nonexistent")
            .env("LC_ALL", "C")
            .env(
                "GIT_DIR",
                self.store.join("records").join(&self.object_format),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(&self.store)
            .args([
                "--no-pager",
                "--no-optional-locks",
                "--literal-pathspecs",
                "--no-replace-objects",
                "-c",
                "core.bare=true",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "gc.auto=0",
                "-c",
                "maintenance.auto=false",
            ]);
        command
    }
}

pub fn read(request: &Request) -> Result<Read> {
    request.validate()?;
    let started = Instant::now();
    let changed = &request.changed;
    let ids: Vec<_> = [&changed.before_oid, &changed.after_oid]
        .into_iter()
        .flatten()
        .collect();
    let input = ids.iter().map(|id| format!("{id}\n")).collect::<String>();
    // One batch, no alternates and no whole-tree walk: changed blobs are owned.
    let out = run(
        request.git().args(["cat-file", "--batch"]),
        Some(input.into_bytes()),
        200 * 1024 * 1024 + 1024,
        TIMEOUT,
    )?;
    if !out.status.success() || out.out.truncated {
        return Err("The saved content is unavailable.".into());
    }
    let mut bytes = out.out.bytes.as_slice();
    let mut lengths = Vec::new();
    // Use the recorder's Git numstat decision, including non-UTF-8 text.
    let binary = changed.added.is_none() || changed.removed.is_none();
    for id in &ids {
        let newline = bytes
            .iter()
            .position(|c| *c == b'\n')
            .ok_or("Invalid saved content.")?;
        let header =
            std::str::from_utf8(&bytes[..newline]).map_err(|_| "Invalid saved content.")?;
        let fields: Vec<_> = header.split(' ').collect();
        if fields.len() != 3 || fields[0] != id.as_str() || fields[1] != "blob" {
            return Err("The saved content is unavailable.".into());
        }
        let length: usize = fields[2].parse().map_err(|_| "Invalid saved content.")?;
        bytes = &bytes[newline + 1..];
        if length > 100 * 1024 * 1024 || bytes.len() <= length || bytes[length] != b'\n' {
            return Err("The saved content is unavailable.".into());
        }
        lengths.push(length);
        bytes = &bytes[length + 1..];
    }
    if !bytes.is_empty() {
        return Err("Invalid saved content.".into());
    }
    let mut lengths = lengths.into_iter();
    let mut entry = |mode: &Option<String>, oid: &Option<String>| {
        oid.as_ref().map(|id| Entry {
            mode: mode.clone().unwrap(),
            fingerprint: id.clone(),
            bytes: lengths.next().unwrap(),
        })
    };
    let before = entry(&changed.before_mode, &changed.before_oid);
    let after = entry(&changed.after_mode, &changed.after_oid);
    let mut processes = 1;
    let empty = if before.is_none() || after.is_none() {
        processes += 1;
        let out = run(
            request.git().args(["hash-object", "-w", "--stdin"]),
            Some(Vec::new()),
            100,
            TIMEOUT,
        )?;
        if !out.status.success() || out.out.truncated {
            return Err("The saved content is unavailable.".into());
        }
        String::from_utf8(out.out.bytes)
            .map_err(|_| "Invalid empty object.")?
            .trim()
            .to_owned()
    } else {
        String::new()
    };
    let a = changed.before_oid.as_deref().unwrap_or(&empty);
    let b = changed.after_oid.as_deref().unwrap_or(&empty);
    let mut patch = format!("diff --git a/{0} b/{0}\n", changed.path).into_bytes();
    match (&changed.before_mode, &changed.after_mode) {
        (None, Some(mode)) => patch.extend_from_slice(format!("new file mode {mode}\n").as_bytes()),
        (Some(mode), None) => {
            patch.extend_from_slice(format!("deleted file mode {mode}\n").as_bytes())
        }
        (Some(old), Some(new)) if old != new => {
            patch.extend_from_slice(format!("old mode {old}\nnew mode {new}\n").as_bytes())
        }
        _ => (),
    }
    let mut truncated = false;
    let status = if binary {
        patch.extend_from_slice(b"Binary files differ\n");
        "binary"
    } else if before.is_some() && after.is_some() && a == b {
        if changed.before_mode == changed.after_mode {
            patch.clear();
            "empty"
        } else {
            "mode"
        }
    } else {
        processes += 1;
        let out = run(
            request.git().args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--no-color",
                "--no-indent-heuristic",
                "--diff-algorithm=myers",
                "--minimal",
                "--unified=3",
                a,
                b,
                "--",
            ]),
            None,
            DIFF_LIMIT,
            TIMEOUT,
        )?;
        if !out.status.success() {
            return Err("The saved content is unavailable.".into());
        }
        truncated = out.out.truncated;
        let body = out
            .out
            .bytes
            .split_inclusive(|c| *c == b'\n')
            .skip_while(|line| !line.starts_with(b"@@ "))
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        patch.extend_from_slice(
            format!(
                "--- {}\n+++ {}\n",
                if before.is_some() {
                    format!("a/{}", changed.path)
                } else {
                    "/dev/null".into()
                },
                if after.is_some() {
                    format!("b/{}", changed.path)
                } else {
                    "/dev/null".into()
                }
            )
            .as_bytes(),
        );
        patch.extend_from_slice(&body);
        "ready"
    };
    truncated |=
        patch.len() >= DIFF_LIMIT || patch.iter().filter(|c| **c == b'\n').count() >= 10_000;
    let hunks = limit_patch(&mut patch, truncated)?;
    Ok(Read {
        before,
        after,
        status: status.into(),
        patch,
        hunks,
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
        processes,
    })
}
// A preview may stop between hunks, never inside a line or an incomplete hunk.
fn limit_patch(patch: &mut Vec<u8>, truncated: bool) -> Result<Vec<Hunk>> {
    if truncated {
        let end = patch[..patch.len().min(DIFF_LIMIT)]
            .iter()
            .rposition(|c| *c == b'\n')
            .map_or(0, |i| i + 1);
        patch.truncate(end);
        let parsed = hunks(patch)?;
        if let Some(last) = parsed.last() {
            let lines: Vec<_> = patch.split_inclusive(|c| *c == b'\n').collect();
            if lines[last.first_line].starts_with(b"@@ ") {
                let mut old = 0;
                let mut new = 0;
                for line in &lines[last.first_line + 1..] {
                    old += usize::from(matches!(line.first(), Some(b' ' | b'-')));
                    new += usize::from(matches!(line.first(), Some(b' ' | b'+')));
                }
                if old != last.old_count || new != last.new_count {
                    let end = lines[..last.first_line].iter().map(|line| line.len()).sum();
                    patch.truncate(end);
                }
            }
        }
    }
    hunks(patch)
}
fn hunks(patch: &[u8]) -> Result<Vec<Hunk>> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let lines: Vec<_> = patch.split_inclusive(|c| *c == b'\n').collect();
    for (index, line) in lines.iter().enumerate() {
        if !line.starts_with(b"@@ ") {
            continue;
        }
        if let Some(previous) = hunks.last_mut() {
            previous.last_line = index - 1;
        }
        let text = String::from_utf8_lossy(line);
        let mut words = text.split_whitespace().skip(1);
        let range = |text: Option<&str>, sign: char| -> Result<(usize, usize)> {
            let text = text
                .and_then(|s| s.strip_prefix(sign))
                .ok_or("Invalid patch range.")?;
            let (start, count) = text.split_once(',').unwrap_or((text, "1"));
            Ok((
                start.parse().map_err(|_| "Invalid patch range.")?,
                count.parse().map_err(|_| "Invalid patch range.")?,
            ))
        };
        let (old_start, old_count) = range(words.next(), '-')?;
        let (new_start, new_count) = range(words.next(), '+')?;
        hunks.push(Hunk {
            first_line: index,
            last_line: lines.len().saturating_sub(1),
            old_start,
            old_count,
            new_start,
            new_count,
        });
    }
    if hunks.is_empty() && !patch.is_empty() {
        hunks.push(Hunk {
            first_line: 0,
            last_line: lines.len().saturating_sub(1),
            old_start: 0,
            old_count: 0,
            new_start: 0,
            new_count: 0,
        });
    }
    Ok(hunks)
}
