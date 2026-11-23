//! Bounded reference-only checkout information for the host.
use super::{Result, TIMEOUT, base_command, plain_folder, run};
use plait_protocol::host::git::{BranchSummary, CheckoutSummary, Fingerprint, Vcs};
use std::path::Path;

const LIMIT: usize = 64 * 1024;

fn query(root: &Path, args: &[&str]) -> Result<super::Raw> {
    let raw = run(base_command(root).args(args), None, LIMIT, TIMEOUT)?;
    if raw.out.truncated {
        return Err("Git summary exceeds the safe read limit.".into());
    }
    Ok(raw)
}
fn text(raw: &super::Raw) -> Result<&str> {
    std::str::from_utf8(&raw.out.bytes).map_err(|_| "Git returned non-UTF-8 summary data.".into())
}

pub fn checkout_summary(root: &Path) -> Result<CheckoutSummary> {
    if plain_folder(root) {
        if !root.is_dir() {
            return Err("The folder is missing.".into());
        }
        return Ok(CheckoutSummary {
            vcs: Vcs::Folder,
            fingerprint: None,
            branch: None,
            main_checkout: None,
        });
    }
    let head = query(root, &["symbolic-ref", "--quiet", "HEAD"])?;
    let head_ref = if head.status.success() {
        Some(text(&head)?.trim_end())
    } else {
        None
    };
    let mut branch = None;
    if let Some(head_ref) = head_ref {
        let refs = query(
            root,
            &[
                "for-each-ref",
                "--format=%(refname)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)",
                head_ref,
            ],
        )?;
        if !refs.status.success() {
            return Err(refs.failure());
        }
        if let Some(line) = text(&refs)?
            .lines()
            .find(|line| line.split('\0').next() == Some(head_ref))
        {
            let mut fields = line.split('\0');
            fields.next();
            let name = fields.next().ok_or("Invalid branch summary.")?;
            let upstream = fields.next().ok_or("Invalid branch summary.")?;
            let track = fields.next().ok_or("Invalid branch summary.")?;
            let header = super::parse_branch(&format!("{name}...{upstream} {track}"));
            branch = Some(BranchSummary {
                branch: name.into(),
                named: true,
                tracking: (!upstream.is_empty()).then_some(header.tracking).flatten(),
            });
        }
    }
    if let Some(tracking) = branch.as_mut().and_then(|branch| branch.tracking.as_mut())
        && !tracking.gone
    {
        let counts = query(
            root,
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        )?;
        if !counts.status.success() {
            return Err(counts.failure());
        }
        let mut values = text(&counts)?.split_whitespace();
        tracking.ahead = values
            .next()
            .ok_or("Missing ahead count.")?
            .parse()
            .map_err(|_| "Invalid ahead count.")?;
        tracking.behind = values
            .next()
            .ok_or("Missing behind count.")?
            .parse()
            .map_err(|_| "Invalid behind count.")?;
        if values.next().is_some() {
            return Err("Invalid tracking counts.".into());
        }
    }
    // Callers should cache roots: they change only after a history rewrite or first commit.
    let roots_raw = query(root, &["rev-list", "--max-parents=0", "HEAD"])?;
    let roots = if roots_raw.status.success() {
        let mut roots: Vec<String> = text(&roots_raw)?
            .lines()
            .map(str::to_ascii_lowercase)
            .collect();
        if roots.len() > 32 || roots.iter().any(|id| !super::log::is_object_id(id)) {
            return Err("Git returned too many or invalid root commits.".into());
        }
        roots.sort();
        roots
    } else if branch.is_none() {
        Vec::new()
    } else {
        return Err(roots_raw.failure());
    };
    if branch.is_none() {
        if let Some(head_ref) = head_ref {
            branch = Some(BranchSummary {
                branch: head_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(head_ref)
                    .into(),
                named: true,
                tracking: None,
            });
        } else {
            if !roots_raw.status.success() {
                return Err(roots_raw.failure());
            }
            let detached = query(root, &["rev-parse", "--short", "HEAD"])?;
            if !detached.status.success() {
                return Err(detached.failure());
            }
            branch = Some(BranchSummary {
                branch: text(&detached)?.trim_end().into(),
                named: false,
                tracking: None,
            });
        }
    }
    let config = query(root, &["config", "--get-regexp", "^remote\\..*\\.url$"])?;
    if !config.status.success() && config.status.code() != Some(1) {
        return Err(config.failure());
    }
    let mut remotes: Vec<_> = text(&config)?
        .lines()
        .filter_map(|line| line.split_once(' '))
        .collect();
    remotes.sort_by_key(|(name, _)| *name);
    let remote = remotes
        .iter()
        .find(|(name, _)| *name == "remote.origin.url")
        .or_else(|| remotes.first())
        .and_then(|(_, url)| normalize_remote(url));
    let worktrees = query(root, &["worktree", "list", "--porcelain", "-z"])?;
    if !worktrees.status.success() {
        return Err(worktrees.failure());
    }
    let first = worktrees
        .out
        .bytes
        .split(|b| *b == 0)
        .next()
        .and_then(|s| s.strip_prefix(b"worktree "))
        .ok_or("Invalid Git worktree list.")?;
    let main = super::path_from_bytes(first);
    if !main.is_absolute() {
        return Err("Invalid main checkout path.".into());
    }
    let main_checkout = if std::fs::canonicalize(&main).ok() == std::fs::canonicalize(root).ok() {
        None
    } else {
        Some(main)
    };
    Ok(CheckoutSummary {
        vcs: Vcs::Git,
        fingerprint: Some(Fingerprint { roots, remote }),
        branch,
        main_checkout,
    })
}

fn normalize_remote(input: &str) -> Option<String> {
    let input = input.trim().split(['?', '#']).next()?;
    let (authority, path) = if let Some((scheme, rest)) = input.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest);
        rest.split_once('/')?
    } else {
        if input.starts_with('/') || input.starts_with('.') || input.contains('\\') {
            return None;
        }
        let rest = input.rsplit_once('@').map_or(input, |(_, rest)| rest);
        rest.split_once(':')?
    };
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    };
    if host.is_empty() || path.is_empty() || host.contains('/') {
        return None;
    }
    let path = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'))
        .trim_end_matches('/');
    if path.is_empty() {
        return None;
    }
    let result = format!("{}/{}", host.to_ascii_lowercase(), path);
    (!result
        .chars()
        .any(|c| c == '@' || c.is_whitespace() || c.is_control()))
    .then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summary_normalizes_remotes() {
        for url in [
            "https://user:secret@EXAMPLE.com/org/repo.git/",
            "ssh://git@example.com/org/repo",
            "git@example.com:org/repo.git",
            "user:pass@word@example.com:org/repo.git",
            "https://u:p/ss@EXAMPLE.com/org/repo///",
            "https://u:p@ss:word@EXAMPLE.com/org/repo",
            "ssh://git@example.com:2222/org/repo",
            "git://EXAMPLE.com/org/repo",
        ] {
            assert!(!normalize_remote(url).unwrap().contains('@'));
            assert_eq!(
                normalize_remote(url).as_deref(),
                Some("example.com/org/repo")
            );
        }
        assert_eq!(
            normalize_remote("https://ghp_x@github.com/o/r").as_deref(),
            Some("github.com/o/r")
        );
        for url in [
            "/tmp/repo",
            "../repo",
            "file:///tmp/repo",
            "https://host/o/has space",
            "https://host/o/has\ncontrol",
        ] {
            assert_eq!(normalize_remote(url), None);
        }
    }
    #[test]
    fn summary_repository_states() {
        use crate::git::tests::Repo;
        let repo = Repo::new();
        let unborn = checkout_summary(&repo.0).unwrap();
        assert!(unborn.fingerprint.unwrap().roots.is_empty());
        assert_eq!(unborn.branch.unwrap().branch, "main");
        repo.write("file", "one");
        repo.commit();
        repo.git(&[
            "remote",
            "add",
            "origin",
            "https://user:secret@EXAMPLE.com/org/repo.git",
        ]);
        let initial = checkout_summary(&repo.0).unwrap();
        let fp = initial.fingerprint.unwrap();
        assert_eq!(fp.roots.len(), 1);
        assert_eq!(fp.remote.as_deref(), Some("example.com/org/repo"));
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["branch", "--set-upstream-to=origin/main"]);
        repo.write("file", "two");
        repo.commit();
        let tracking = checkout_summary(&repo.0)
            .unwrap()
            .branch
            .unwrap()
            .tracking
            .unwrap();
        assert_eq!(
            (tracking.ahead, tracking.behind, tracking.gone),
            (1, 0, false)
        );
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["reset", "--hard", "HEAD^"]);
        repo.write("another", "divergence");
        repo.commit();
        let tracking = checkout_summary(&repo.0)
            .unwrap()
            .branch
            .unwrap()
            .tracking
            .unwrap();
        assert_eq!((tracking.ahead, tracking.behind), (1, 1));
        repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
        assert!(
            checkout_summary(&repo.0)
                .unwrap()
                .branch
                .unwrap()
                .tracking
                .unwrap()
                .gone
        );
        repo.git(&["checkout", "--detach"]);
        let detached = checkout_summary(&repo.0).unwrap().branch.unwrap();
        assert!(!detached.named);
        assert_ne!(detached.branch, "HEAD");
        let copy = repo.0.join("differently-named-copy");
        repo.git(&[
            "clone",
            "--local",
            repo.0.to_str().unwrap(),
            copy.to_str().unwrap(),
        ]);
        let copy_repo = Repo(copy.clone());
        copy_repo.git(&[
            "remote",
            "set-url",
            "origin",
            "git@example.com:org/repo.git",
        ]);
        assert_eq!(checkout_summary(&copy).unwrap().fingerprint.unwrap(), fp);
        let worktree = repo.0.join("linked");
        repo.git(&["worktree", "add", "--detach", worktree.to_str().unwrap()]);
        assert_eq!(
            checkout_summary(&worktree)
                .unwrap()
                .main_checkout
                .as_deref(),
            Some(repo.0.as_path())
        );
        assert_eq!(
            checkout_summary(&worktree).unwrap().fingerprint.unwrap(),
            fp
        );
        let folder = repo.0.join("plain");
        std::fs::create_dir(&folder).unwrap();
        // A folder under a checkout belongs to that checkout.
        assert_eq!(checkout_summary(&folder).unwrap().vcs, Vcs::Git);
    }

    #[test]
    fn summary_many_branches() {
        use crate::git::tests::Repo;
        use std::io::Write;
        use std::process::{Command, Stdio};
        let repo = Repo::new();
        repo.write("file", "one");
        repo.commit();
        let mut child = Command::new("git")
            .current_dir(&repo.0)
            .args(["update-ref", "--stdin"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        for i in 0..3000 {
            writeln!(input, "create refs/heads/branch-{i:04} HEAD").unwrap();
        }
        drop(input);
        assert!(child.wait().unwrap().success());
        assert_eq!(
            checkout_summary(&repo.0).unwrap().branch.unwrap().branch,
            "main"
        );
    }

    #[test]
    fn summary_plain_folder_does_not_spawn_git() {
        let path = std::env::temp_dir().join(format!("plait-summary-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        crate::git::SPAWNED.with(|n| n.set(0));
        assert_eq!(checkout_summary(&path).unwrap().vcs, Vcs::Folder);
        crate::git::SPAWNED.with(|n| assert_eq!(n.get(), 0));
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn summary_multiple_roots_and_forks() {
        use crate::git::tests::Repo;
        let repo = Repo::new();
        repo.write("one", "one");
        repo.commit();
        repo.git(&["checkout", "--orphan", "other"]);
        repo.git(&["rm", "-rf", "."]);
        repo.write("two", "two");
        repo.commit();
        repo.git(&["checkout", "main"]);
        repo.git(&["merge", "--allow-unrelated-histories", "--no-edit", "other"]);
        repo.git(&[
            "remote",
            "add",
            "origin",
            "ssh://git@example.com/upstream/repo.git",
        ]);
        let first = checkout_summary(&repo.0).unwrap().fingerprint.unwrap();
        assert_eq!(first.roots.len(), 2);
        assert!(first.roots[0] < first.roots[1]);
        repo.git(&[
            "remote",
            "set-url",
            "origin",
            "git@example.com:fork/repo.git",
        ]);
        let fork = checkout_summary(&repo.0).unwrap().fingerprint.unwrap();
        assert_eq!(first.roots, fork.roots);
        assert_ne!(first.remote, fork.remote);
    }
}
