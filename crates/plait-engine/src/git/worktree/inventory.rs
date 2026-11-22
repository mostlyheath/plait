//! Host-side live inventory. Removal and preview consume the same preservation facts.
use super::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Preservation {
    pub changed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_output: Option<u64>,
    pub unpushed: Option<u64>,
    pub exclusive: Option<u64>,
    pub refusal: Option<String>,
    /// The same checks with the person's explicit publication exception.
    #[serde(default)]
    pub can_keep_local: bool,
}
impl Preservation {
    pub fn require_safe(&self) -> Result<()> {
        self.refusal.clone().map_or(Ok(()), Err)
    }
}
const DIRTY: &str = "This worktree has uncommitted or ignored files. Preserve or remove them before removing the worktree.";
const EXCLUSIVE: &str = "This worktree has commits that are on no branch (made on a detached HEAD or kept only by its own refs). Removing it would lose them. Put them on a branch, then remove the worktree.";

pub(super) fn preservation_with_output(
    path: &Path,
    main: &Path,
    allow_unpushed: bool,
    approved: &[String],
) -> Result<Preservation> {
    let approved: Vec<PathBuf> = approved
        .iter()
        .filter_map(|directory| crate::git::build_output::validate(path, directory).ok())
        .collect();
    let known = |name: &[u8]| {
        use std::os::unix::ffi::OsStrExt;
        let candidate = Path::new(std::ffi::OsStr::from_bytes(name));
        covered_ignored(path, candidate, &approved)
    };
    let dirty = read_worktree(
        path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )?;
    let mut entries = dirty.split(|b| *b == 0).filter(|v| !v.is_empty());
    let mut changed = 0;
    let mut build_output = 0;
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            return Err("Invalid Git status entry.".into());
        }
        if &entry[..2] == b"!!" && known(&entry[3..]) {
            build_output += 1;
        } else {
            changed += 1;
        }
        if entry[..2].iter().any(|c| matches!(c, b'R' | b'C')) {
            entries.next().ok_or("Incomplete Git rename.")?;
        }
    }
    let hidden = refuse_hidden_edits(path).err();
    let exclusive = worktree_only_commits(path, main);
    let git_dir = text_read(path, &["rev-parse", "--absolute-git-dir"])?;
    let unfinished = [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ]
    .iter()
    .any(|name| fs::symlink_metadata(Path::new(&git_dir).join(name)).is_ok());
    let publication = publication(path);
    let local_refusal = if changed > 0 {
        Some(DIRTY.into())
    } else if hidden.is_some() {
        hidden
    } else if let Err(error) = &exclusive {
        Some(error.clone())
    } else if exclusive.as_ref().is_ok_and(|n| *n > 0) {
        Some(EXCLUSIVE.into())
    } else if unfinished {
        Some("This worktree has an unfinished Git operation.".into())
    } else {
        None
    };
    let can_keep_local = local_refusal.is_none();
    let refusal = local_refusal.or_else(|| if !allow_unpushed {
        match &publication {
            Err(error) => Some(error.clone()),
            Ok(n) if *n > 0 => Some("This worktree has unpushed commits. Push them or explicitly confirm preserving only the local branch.".into()),
            _ => None,
        }
    } else {
        None
    });
    Ok(Preservation {
        changed: Some(changed),
        build_output: (!approved.is_empty()).then_some(build_output),
        unpushed: publication.ok(),
        exclusive: exclusive.ok(),
        refusal,
        can_keep_local,
    })
}
/// Git can collapse an ignored parent above an approved directory. Expand only
/// those ancestors; any sibling outside the approved list still preserves it.
fn covered_ignored(root: &Path, path: &Path, approved: &[PathBuf]) -> bool {
    use crate::worktree_size::{id, list, open_at, stat_at};
    use std::os::{
        fd::{AsRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    };
    fn visit(fd: &OwnedFd, path: &Path, approved: &[PathBuf], depth: usize) -> bool {
        if depth > 128 {
            return false;
        }
        let mut entries = 0;
        list(fd, |name| {
            entries += 1;
            let next = path.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
            if approved.iter().any(|p| next.starts_with(p)) {
                return true;
            }
            if !approved.iter().any(|p| p.starts_with(&next)) {
                return false;
            }
            let Ok(before) = stat_at(fd.as_raw_fd(), name) else {
                return false;
            };
            let Ok(child) = open_at(fd.as_raw_fd(), name) else {
                return false;
            };
            id(&before) == id(&child.1) && visit(&child.0, &next, approved, depth + 1)
        }) && entries > 0
    }
    if approved.iter().any(|p| path.starts_with(p)) {
        return true;
    }
    if !approved.iter().any(|p| p.starts_with(path)) {
        return false;
    }
    crate::build_output::directory(root, path).is_ok_and(|(fd, _)| visit(&fd, path, approved, 0))
}

fn publication(path: &Path) -> Result<u64> {
    let remote = text_read(path, &["rev-parse", "--symbolic-full-name", "@{upstream}"])
        .map_err(|_| "This worktree has unpushed or unknown publication status. Explicitly confirm preserving only the local branch.".to_owned())?;
    if !remote.starts_with("refs/remotes/") {
        return Err("This worktree tracks a local branch; publication is unknown. Explicitly confirm preserving only the local branch.".into());
    }
    let upstream = text_read(path, &["rev-parse", "--verify", "@{upstream}^{commit}"])
        .map_err(|_| "This worktree has unpushed or unknown publication status. Explicitly confirm keeping its work only on the local branch.".to_owned())?;
    text_read(path, &["rev-list", "--count", &format!("{upstream}..HEAD")])?
        .parse()
        .map_err(|_| "Invalid commit count.".into())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    pub path: PathBuf,
    pub main: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub base: Option<String>,
    pub thread_id: Option<String>,
    pub identity: Option<Identity>,
    pub missing: bool,
    pub removed: bool,
    pub preservation: Preservation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_directories: Vec<String>,
}
/// All registered checkouts, including ones created outside Plait. Never infer ownership.
/// One configuration read per checkout serves the whole pass.
pub fn inventory(root: &Path, records: &[ManagedWorktree]) -> Result<Vec<Checkout>> {
    inventory_of(root, records, None)
}
/// With `only`, just that checkout's row, or none when it is not one of this
/// repository's: a run ending re-reads its own checkout, not every one.
pub fn inventory_of(
    root: &Path,
    records: &[ManagedWorktree],
    only: Option<&Path>,
) -> Result<Vec<Checkout>> {
    inventory_with_output(root, records, only, &[])
}
pub fn inventory_with_output(
    root: &Path,
    records: &[ManagedWorktree],
    only: Option<&Path>,
    approved: &[String],
) -> Result<Vec<Checkout>> {
    crate::git::sharing_config(|| read_inventory(root, records, only, approved))
}
fn read_inventory(
    root: &Path,
    records: &[ManagedWorktree],
    only: Option<&Path>,
    approved: &[String],
) -> Result<Vec<Checkout>> {
    let paths = crate::lease::resolve_git_checkout(root).map_err(String::from)?;
    let raw = read_worktree(&paths.root, &["worktree", "list", "--porcelain", "-z"])?;
    let text = std::str::from_utf8(&raw).map_err(|_| "Worktree paths must be UTF-8.")?;
    let mut rows = Vec::new();
    for block in text.split("\0\0").filter(|s| !s.is_empty()) {
        if rows.len() == 256 {
            return Err(
                "This repository has more than 256 checkouts. Use Git to inspect the full list."
                    .into(),
            );
        }
        let mut fields = block.split('\0');
        let path = PathBuf::from(
            fields
                .next()
                .and_then(|s| s.strip_prefix("worktree "))
                .ok_or("Invalid Git worktree list.")?,
        );
        let fields: Vec<_> = fields.collect();
        let main = rows.is_empty();
        let managed = records.iter().find(|record| {
            record.path == path
                && record.parent.common == paths.common_dir
                && record.stage != ManagedStage::Removed
        });
        let branch = fields
            .iter()
            .find_map(|s| s.strip_prefix("branch refs/heads/"))
            .map(str::to_owned);
        let head = fields
            .iter()
            .find_map(|s| s.strip_prefix("HEAD "))
            .filter(|s| !s.bytes().all(|b| b == b'0'))
            .map(str::to_owned);
        let missing = fs::symlink_metadata(&path).is_err();
        rows.push(Checkout {
            path: path.clone(),
            main,
            branch,
            head,
            base: managed.map(|r| r.parent.base.clone()),
            thread_id: managed.map(|r| r.thread_id.clone()),
            identity: managed.and_then(|r| r.identity.clone()).or_else(|| {
                fs::symlink_metadata(&path)
                    .ok()
                    .filter(|m| m.is_dir() && !m.is_symlink())
                    .map(|m| Identity {
                        dev: m.dev().to_string(),
                        ino: m.ino().to_string(),
                    })
            }),
            missing,
            removed: false,
            preservation: Preservation::default(),
            build_directories: approved.to_vec(),
        });
    }
    let main = rows
        .first()
        .ok_or("Git returned no checkouts.")?
        .path
        .clone();
    if let Some(only) = only {
        rows.retain(|row| row.path == only);
    }
    // Removal keeps private build output. Keep its attribution visible afterwards.
    for record in records.iter().filter(|r| {
        r.stage == ManagedStage::Removed
            && r.parent.common == paths.common_dir
            && only.is_none_or(|only| only == r.path)
    }) {
        if rows.iter().any(|row| row.path == record.path) {
            continue;
        }
        if rows.len() == 256 {
            return Err("This repository has more than 256 checkout records. Use Git to inspect the full list.".into());
        }
        rows.push(Checkout {build_directories:approved.to_vec(),path:record.path.clone(),main:false,branch:Some(record.branch.clone()),head:text_read(&main,&["rev-parse","--verify",&format!("refs/heads/{}^{{commit}}",record.branch)]).ok(),base:Some(record.parent.base.clone()),thread_id:Some(record.thread_id.clone()),identity:record.identity.clone(),missing:true,removed:true,preservation:Preservation {refusal:Some("This worktree was removed. Its branch, thread history and private build cache remain.".into()),..Default::default()}});
    }
    for row in &mut rows {
        if row.removed {
            continue;
        }
        let managed = records
            .iter()
            .find(|record| Some(&record.thread_id) == row.thread_id.as_ref());
        let result = if row.missing {
            Err(format!(
                "The worktree folder {} is missing. Nothing was removed. If you moved it, move it back; if its disk is not connected, connect it.",
                row.path.display()
            ))
        } else {
            preservation_with_output(&row.path, &main, false, approved)
        };
        row.preservation = result.unwrap_or_else(|error| Preservation {
            refusal: Some(error),
            ..Default::default()
        });
        let ownership = if row.main {
            Some("The main checkout cannot be removed here.".into())
        } else if let Some(record) = managed {
            removal_identity(record).err()
        } else {
            Some("Plait did not create this worktree. Remove it with your Git tools.".into())
        };
        if ownership.is_some() && !row.missing {
            row.preservation.refusal = ownership;
            row.preservation.can_keep_local = false;
        }
    }
    Ok(rows)
}
fn removal_identity(record: &ManagedWorktree) -> Result<()> {
    if !owned_shape(
        record,
        &worktree_base(&record.parent.main)?,
        &uuid_hex(&record.thread_id)?,
    ) {
        return Err(
            "This record does not name a worktree Plait created. Nothing was removed.".into(),
        );
    }
    verify_repository(&record.parent)?;
    verify_managed(record)?;
    let expected = record
        .identity
        .as_ref()
        .ok_or("Worktree has no recorded directory identity.")?;
    // A preview takes no lease: runs take theirs without waiting, so even a
    // brief hold here refuses one. Removal rechecks everything under its own.
    let id = expected.lease()?;
    crate::lease::git_cleanup::require_clear(id).map_err(String::from)?;
    crate::lease::cleanup::require_clear(id).map_err(String::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::Repo;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    const ID: &str = "12345678-1234-4abc-8abc-123456789abc";
    fn git_in(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn commit(root: &Path, name: &str) {
        fs::write(root.join(name), "work\n").unwrap();
        git_in(root, &["add", name]);
        git_in(
            root,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                name,
            ],
        );
    }
    fn repository() -> (Repo, ManagedWorktree) {
        let mut repo = Repo::new();
        repo.0 = repo.0.canonicalize().unwrap();
        fs::write(repo.0.join("file"), "committed\n").unwrap();
        fs::write(repo.0.join(".gitignore"), "*.log\n.env\n").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        let tree =
            create_managed_worktree(&worktree_parent(&repo.0).unwrap(), ID, None, |_| Ok(()))
                .unwrap();
        (repo, tree)
    }
    fn clean(tree: &ManagedWorktree) {
        if let Ok(base) = worktree_base(&tree.parent.main) {
            let _ = fs::remove_dir_all(base);
        }
    }
    /// The Git commands a pass started, one line each, from a logging `git`
    /// placed first on PATH. The test re-runs itself in a child process so the
    /// changed PATH never reaches tests running beside it.
    pub(crate) fn counted_git(test: &str, child: impl FnOnce(&Path)) -> Option<Vec<String>> {
        if let Some(log) = std::env::var_os("PLAIT_GIT_COUNT_LOG") {
            child(Path::new(&log));
            return None;
        }
        let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("git"))
            .find(|git| git.is_file())
            .unwrap();
        let dir = std::env::temp_dir().join(format!(
            "plait-git-count-{}-{}",
            std::process::id(),
            test.rsplit("::").next().unwrap()
        ));
        fs::create_dir_all(&dir).unwrap();
        let (shim, log) = (dir.join("git"), dir.join("log"));
        fs::write(
            &shim,
            format!("#!/bin/sh\nprintf 'GIT %s\\n' \"$*\" >> \"$PLAIT_GIT_COUNT_LOG\"\nexec '{}' \"$@\"\n", real.display()),
        )
        .unwrap();
        fs::set_permissions(&shim, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut path = std::ffi::OsString::from(&dir);
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap());
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env("PATH", path)
            .env("PLAIT_GIT_COUNT_LOG", &log)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let text = fs::read_to_string(&log).unwrap();
        let _ = fs::remove_dir_all(&dir);
        Some(text.lines().map(str::to_owned).collect())
    }
    /// The Git commands between the child's `PASS` and `END` notes: what the
    /// measured pass started, not the repository's setup or later checks.
    pub(crate) fn pass(lines: &[String]) -> Vec<String> {
        let start = lines.iter().position(|l| l == "PASS").expect("pass note");
        let end = lines.iter().position(|l| l == "END").expect("end note");
        lines[start + 1..end].to_vec()
    }
    /// Writes `line` into the log a counted child shares with its Git.
    pub(crate) fn note(log: &Path, line: &str) {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(log)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }
    /// Each checkout's filter configuration is read once per inventory, not
    /// once per Git command; every read still carries the hardened flags.
    #[test]
    fn inventory_reads_each_checkouts_configuration_once() {
        let test =
            "git::worktree::inventory::tests::inventory_reads_each_checkouts_configuration_once";
        let Some(lines) = counted_git(test, |log| {
            let (repo, tree) = repository();
            repo.git(&["config", "filter.probe.clean", "false"]);
            for name in ["outside-a", "outside-b"] {
                let path = repo.0.with_file_name(format!(
                    "{}-{name}",
                    repo.0.file_name().unwrap().to_string_lossy()
                ));
                repo.git(&["worktree", "add", "-q", "-b", name, path.to_str().unwrap()]);
            }
            note(log, "PASS");
            let rows = inventory(&repo.0, std::slice::from_ref(&tree)).unwrap();
            note(log, "END");
            assert_eq!(rows.len(), 4);
            clean(&tree);
        }) else {
            return;
        };
        let lines = pass(&lines);
        let config: Vec<_> = lines
            .iter()
            .filter(|l| l.ends_with("config --null --name-only --list"))
            .collect();
        let mut roots: Vec<_> = config
            .iter()
            .map(|l| l.split(" -C ").nth(1).unwrap().split(' ').next().unwrap())
            .collect();
        roots.sort();
        roots.dedup();
        assert_eq!(
            (config.len(), roots.len()),
            (4, 4),
            "one configuration read per checkout:\n{}",
            lines.join("\n")
        );
        let hardened = "GIT --no-pager --no-optional-locks --literal-pathspecs --no-replace-objects -c core.fsmonitor=false -c core.untrackedCache=false -c color.ui=false -C ";
        for line in &lines {
            assert!(line.starts_with(hardened), "unhardened read: {line}");
            assert!(
                config.contains(&line) || line.contains(" -c filter.probe.clean= "),
                "a read kept the filter driver: {line}"
            );
        }
        eprintln!(
            "inventory, main + 3 worktrees: {} Git processes",
            lines.len()
        );
    }
    /// A run ending re-reads its own checkout: one row, and no Git command in
    /// any other worktree. The main checkout lists worktrees and holds the
    /// refs that decide which commits are kept only here.
    #[test]
    fn a_one_checkout_pass_reads_only_that_checkout() {
        let test = "git::worktree::inventory::tests::a_one_checkout_pass_reads_only_that_checkout";
        let Some(log) = counted_git(test, |log| {
            let (repo, tree) = repository();
            let other = repo.0.with_file_name(format!(
                "{}-other",
                repo.0.file_name().unwrap().to_string_lossy()
            ));
            repo.git(&[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                other.to_str().unwrap(),
            ]);
            note(log, &format!("ROOT {}", repo.0.display()));
            note(log, &format!("ROOT {}", tree.path.display()));
            let heads: Vec<_> = inventory(&repo.0, std::slice::from_ref(&tree))
                .unwrap()
                .into_iter()
                .filter_map(|row| row.head)
                .collect();
            note(log, "PASS");
            let rows =
                inventory_of(&repo.0, std::slice::from_ref(&tree), Some(&tree.path)).unwrap();
            // The desktop then draws the graph again from every row's head.
            crate::git::log::graph(&repo.0, &heads).unwrap();
            note(log, "END");
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].path, tree.path);
            assert_eq!(rows[0].thread_id.as_deref(), Some(ID));
            assert_eq!(rows[0].preservation.changed, Some(0));
            let main = inventory_of(&repo.0, &[], Some(&repo.0)).unwrap();
            assert!(main.len() == 1 && main[0].main);
            let stranger = repo.0.with_file_name("not-a-checkout");
            assert!(
                inventory_of(&repo.0, &[], Some(&stranger))
                    .unwrap()
                    .is_empty()
            );
            clean(&tree);
        }) else {
            return;
        };
        let roots: Vec<_> = log.iter().filter_map(|l| l.strip_prefix("ROOT ")).collect();
        let lines = pass(&log);
        for line in &lines {
            let root = line
                .split(" -C ")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap();
            assert!(roots.contains(&root), "a read outside the checkout: {line}");
        }
        // Worktree list and main's configuration, this checkout's nine checks
        // (two more when it has an upstream), and the graph's two.
        assert!(
            lines.len() <= 13,
            "a run's end costs more than its own checkout:\n{}",
            lines.join("\n")
        );
        eprintln!(
            "one checkout's row and the graph: {} Git processes",
            lines.len()
        );
    }
    /// Runs take their writer lease without waiting; a preview must never be the
    /// reason one is refused.
    #[test]
    fn inventory_never_refuses_a_run_starting_beside_it() {
        let (repo, tree) = repository();
        let stop = Arc::new(AtomicBool::new(false));
        let (path, done) = (tree.path.clone(), stop.clone());
        let runner = std::thread::spawn(move || {
            let mut refused = Vec::new();
            while !done.load(Ordering::Relaxed) {
                match crate::lease::acquire_checkout(&path, None, crate::lease::LeaseClass::Writer)
                {
                    Ok(lease) => drop(lease),
                    Err(error) => refused.push(error.message),
                }
            }
            refused
        });
        for _ in 0..8 {
            let rows = inventory(&repo.0, std::slice::from_ref(&tree)).unwrap();
            assert_eq!(rows.len(), 2);
        }
        stop.store(true, Ordering::Relaxed);
        let refused = runner.join().unwrap();
        assert!(
            refused.is_empty(),
            "runs refused beside a preview: {refused:?}"
        );
        clean(&tree);
    }
    /// The preview's answer and removal's answer come from one function. Feed
    /// both the same work and require the same decision and the same words.
    #[test]
    fn preview_and_removal_agree_on_every_kind_of_work() {
        type Setup<'a> = &'a dyn Fn(&ManagedWorktree);
        let cases: &[(&str, Setup)] = &[
            ("no upstream", &|_| {}),
            ("published", &|_| {}),
            ("local upstream", &|t| {
                git_in(&t.path, &["branch", "--set-upstream-to=main"]);
            }),
            ("untracked", &|t| {
                fs::write(t.path.join("new"), "x").unwrap()
            }),
            ("ignored", &|t| {
                fs::write(t.path.join("out.log"), "x").unwrap()
            }),
            ("approved output only", &|t| {
                fs::create_dir(t.path.join("artifacts.log")).unwrap();
                fs::write(t.path.join("artifacts.log/library"), "output").unwrap();
            }),
            ("approved output plus env", &|t| {
                fs::create_dir(t.path.join("artifacts.log")).unwrap();
                fs::write(t.path.join("artifacts.log/library"), "output").unwrap();
                fs::write(t.path.join(".env"), "secret").unwrap();
            }),
            ("nested approved output", &|t| {
                fs::create_dir_all(t.path.join("artifacts.log/sub")).unwrap();
                fs::write(t.path.join("artifacts.log/sub/library"), "output").unwrap();
            }),
            ("nested output plus sibling", &|t| {
                fs::create_dir_all(t.path.join("artifacts.log/sub")).unwrap();
                fs::write(t.path.join("artifacts.log/sub/library"), "output").unwrap();
                fs::write(t.path.join("artifacts.log/secret"), "preserve").unwrap();
            }),
            ("unapproved output", &|t| {
                fs::create_dir(t.path.join("unapproved.log")).unwrap();
                fs::write(t.path.join("unapproved.log/library"), "output").unwrap();
            }),
            ("revoked output", &|t| {
                fs::create_dir(t.path.join("artifacts.log")).unwrap();
                fs::write(t.path.join("artifacts.log/library"), "output").unwrap();
            }),
            ("staged", &|t| {
                fs::write(t.path.join("file"), "staged\n").unwrap();
                git_in(&t.path, &["add", "file"]);
            }),
            ("assume-unchanged", &|t| {
                git_in(&t.path, &["update-index", "--assume-unchanged", "file"]);
                fs::write(t.path.join("file"), "only copy\n").unwrap();
            }),
            ("skip-worktree", &|t| {
                git_in(&t.path, &["update-index", "--skip-worktree", "file"]);
                fs::write(t.path.join("file"), "only copy\n").unwrap();
            }),
            ("worktree ref", &|t| {
                commit(&t.path, "kept");
                git_in(&t.path, &["update-ref", "refs/worktree/keep", "HEAD"]);
                git_in(&t.path, &["reset", "-q", "--hard", "HEAD~1"]);
                git_in(&t.path, &["reflog", "expire", "--expire=now", "--all"]);
            }),
            ("reflog only", &|t| {
                git_in(&t.path, &["checkout", "-q", "--detach"]);
                commit(&t.path, "detached");
                git_in(&t.path, &["checkout", "-q", &t.branch]);
            }),
            ("unpushed", &|t| commit(&t.path, "ahead")),
            ("unpushed and untracked", &|t| {
                commit(&t.path, "ahead");
                fs::write(t.path.join("new"), "x").unwrap()
            }),
            ("unfinished merge", &|t| {
                let head = text_read(&t.path, &["rev-parse", "HEAD"]).unwrap();
                let git_dir = text_read(&t.path, &["rev-parse", "--absolute-git-dir"]).unwrap();
                fs::write(Path::new(&git_dir).join("MERGE_HEAD"), head + "\n").unwrap();
            }),
            ("unfinished rebase", &|t| {
                let git_dir = text_read(&t.path, &["rev-parse", "--absolute-git-dir"]).unwrap();
                fs::create_dir(Path::new(&git_dir).join("rebase-merge")).unwrap();
            }),
            ("branch changed", &|t| {
                git_in(&t.path, &["checkout", "-qb", "different"])
            }),
            ("detached head", &|t| {
                git_in(&t.path, &["checkout", "-q", "--detach"])
            }),
            ("folder changed", &|t| {
                fs::rename(&t.path, t.path.with_extension("moved")).unwrap();
                fs::create_dir(&t.path).unwrap();
            }),
            ("missing", &|t| fs::remove_dir_all(&t.path).unwrap()),
        ];
        for allow_unpushed in [false, true] {
            for (name, setup) in cases {
                let (repo, tree) = repository();
                if *name != "no upstream" {
                    repo.git(&["remote", "add", "origin", "/nonexistent"]);
                    let remote = format!("refs/remotes/origin/{}", tree.branch);
                    git_in(&repo.0, &["update-ref", &remote, "HEAD"]);
                    let upstream = format!("--set-upstream-to=origin/{}", tree.branch);
                    git_in(&tree.path, &["branch", &upstream]);
                }
                setup(&tree);
                let approved =
                    if matches!(*name, "approved output only" | "approved output plus env") {
                        vec!["artifacts.log".into()]
                    } else if matches!(
                        *name,
                        "nested approved output" | "nested output plus sibling"
                    ) {
                        vec!["artifacts.log/sub".into()]
                    } else {
                        Vec::new()
                    };
                let facts =
                    inventory_with_output(&repo.0, std::slice::from_ref(&tree), None, &approved)
                        .unwrap()[1]
                        .preservation
                        .clone();
                let preview = if allow_unpushed && facts.can_keep_local {
                    None
                } else {
                    facts.refusal.clone()
                };
                let identity = tree.identity.clone().unwrap();
                let removal = remove_managed_with_output(
                    tree.clone(),
                    &identity,
                    allow_unpushed,
                    |_| Ok(()),
                    &mut |root, args, timeout, leases| git_holding(root, args, timeout, leases[0]),
                    |_| Ok(approved),
                );
                match (&preview, &removal) {
                    (None, Ok(removed)) => assert_eq!(removed.stage, ManagedStage::Removed),
                    (Some(preview), Err(removal)) => {
                        // A replaced or missing folder can fail Git discovery before
                        // removal reaches its ownership check; both must refuse.
                        if !matches!(*name, "folder changed" | "missing") {
                            assert_eq!(preview, removal, "{name}");
                        }
                    }
                    _ => panic!("{name}: preview {preview:?}, removal {removal:?}"),
                }
                assert_eq!(
                    preview.is_none(),
                    matches!(
                        *name,
                        "published" | "approved output only" | "nested approved output"
                    ) || (allow_unpushed
                        && matches!(*name, "no upstream" | "local upstream" | "unpushed")),
                    "{name}, keep local: {allow_unpushed}"
                );
                clean(&tree);
            }
        }
    }
}
