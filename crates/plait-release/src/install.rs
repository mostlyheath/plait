//! Per-user installation: immutable host releases, the LaunchAgent that runs
//! the active host, and replacement of the running app bundle.
//!
//! Everything lives in locations the user owns, so updates need no
//! administrator access. Earlier previews installed root-owned files with
//! Installer packages; those need a one-time migration first.

use super::{Error, Offer, invalid, valid_build_id};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const LABEL: &str = "dev.plait.host";
const LEGACY_AGENT: &str = "/Library/LaunchAgents/dev.plait.host.plist";
const MIGRATE: &str = "This Plait was installed system-wide by an Installer package. \
    Run the Terminal install command from the Plait README once to move it to a per-user install; \
    it asks for your administrator password once, and later updates install from here.";

/// Where one user's host releases, service definition, and store live.
pub struct Layout {
    pub host_base: PathBuf,
    pub agent: PathBuf,
    pub store: PathBuf,
    pub cache: PathBuf,
}

impl Layout {
    pub fn for_user(home: &Path) -> Self {
        Self {
            host_base: home.join("Library/Application Support/Plait/Host"),
            agent: home.join("Library/LaunchAgents/dev.plait.host.plist"),
            store: home.join(".local/share/plait"),
            cache: home.join("Library/Caches/dev.plait.desktop/updates"),
        }
    }

    pub fn executable(&self, build_id: &str) -> PathBuf {
        self.host_base
            .join("releases")
            .join(build_id)
            .join("bin/plait-host")
    }
}

pub fn legacy_install() -> bool {
    Path::new(LEGACY_AGENT).exists()
}

fn run(command: &mut Command) -> Result<String, Error> {
    let output = command.stdin(Stdio::null()).output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Unsupported(format!(
            "{:?} failed: {}",
            command.get_program(),
            detail.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    format!(".{prefix}-{}-{nanos}", process::id())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().ok_or_else(|| invalid("path has no parent"))?;
    fs::create_dir_all(parent)?;
    let next = parent.join(unique("write"));
    fs::write(&next, bytes)?;
    fs::rename(&next, path).inspect_err(|_| {
        let _ = fs::remove_file(&next);
    })?;
    Ok(())
}

/// Unpacks a host archive as an immutable release and records it as installed.
/// A release that already exists is reused: a running host keeps its files.
pub fn install_host(layout: &Layout, archive: &Path, build_id: &str) -> Result<PathBuf, Error> {
    if !valid_build_id(build_id) {
        return Err(invalid("invalid host build identity"));
    }
    let releases = layout.host_base.join("releases");
    fs::create_dir_all(&releases)?;
    let release = releases.join(build_id);
    if !release.exists() {
        let staging = releases.join(unique("staging"));
        fs::create_dir(&staging)?;
        let result = unpack_host(archive, &staging, build_id, &release);
        let _ = fs::remove_dir_all(&staging);
        result?;
    }
    write_atomic(
        &layout.host_base.join("installed.json"),
        &fs::read(release.join("component.json"))?,
    )?;
    Ok(layout.executable(build_id))
}

fn unpack_host(
    archive: &Path,
    staging: &Path,
    build_id: &str,
    release: &Path,
) -> Result<(), Error> {
    run(Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(staging))?;
    let unpacked = staging.join(build_id);
    let component: serde_json::Value =
        serde_json::from_slice(&fs::read(unpacked.join("component.json"))?)
            .map_err(|error| invalid(error.to_string()))?;
    if component["component"] != "host" || component["build_id"] != build_id {
        return Err(invalid("host archive does not match its build identity"));
    }
    fs::rename(&unpacked, release)?;
    // Read-only after the move: renaming a directory needs write access to it.
    run(Command::new("/bin/chmod").args(["-R", "a-w"]).arg(release))?;
    Ok(())
}

fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn service() -> String {
    format!("gui/{}/{LABEL}", uid())
}

fn loaded() -> bool {
    Command::new("/bin/launchctl")
        .args(["print", &service()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The executable launchd runs for the host service, if it is loaded.
pub fn running_program() -> Option<PathBuf> {
    let output = Command::new("/bin/launchctl")
        .args(["print", &service()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("\tprogram = "))
        .map(PathBuf::from)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn agent_plist(program: &Path) -> String {
    let program = escape(&program.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>{LABEL}</string>
	<key>ProgramArguments</key><array><string>{program}</string><string>run-user</string></array>
	<key>RunAtLoad</key><true/>
	<key>AbandonProcessGroup</key><true/>
	<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
	<key>ThrottleInterval</key><integer>10</integer>
	<key>ProcessType</key><string>Standard</string>
	<key>EnvironmentVariables</key><dict><key>PATH</key><string>/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
</dict>
</plist>
"#
    )
}

/// Restarts the host service on `program`. Callers check `active_runs` first:
/// a run admitted during the restart is marked interrupted, never replayed.
pub fn activate(layout: &Layout, program: &Path) -> Result<(), Error> {
    if legacy_install() {
        return Err(Error::Unsupported(MIGRATE.into()));
    }
    write_atomic(&layout.agent, agent_plist(program).as_bytes())?;
    if running_program().as_deref() == Some(program) {
        return Ok(());
    }
    if loaded() {
        let _ = Command::new("/bin/launchctl")
            .args(["bootout", &service()])
            .stdin(Stdio::null())
            .output();
        wait(50, || !loaded())
            .ok_or_else(|| Error::Unsupported("The running host did not stop.".into()))?;
    }
    bootstrap(layout, program)
}

/// Registers the host service on first launch. Unlike `activate`, it never
/// stops a loaded service: if one appeared meanwhile, it is left running.
pub fn register(layout: &Layout, program: &Path) -> Result<(), Error> {
    if legacy_install() {
        return Err(Error::Unsupported(MIGRATE.into()));
    }
    if loaded() {
        return Ok(());
    }
    write_atomic(&layout.agent, agent_plist(program).as_bytes())?;
    bootstrap(layout, program)
}

/// Loads the written service definition and waits until its host answers.
fn bootstrap(layout: &Layout, program: &Path) -> Result<(), Error> {
    run(Command::new("/bin/launchctl")
        .args(["bootstrap", &format!("gui/{}", uid())])
        .arg(&layout.agent))?;
    let answers = || {
        Command::new(program)
            .arg("status")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    wait(100, answers)
        .ok_or_else(|| Error::Unsupported("The new host did not answer after starting.".into()))
}

fn wait(tries: u32, mut done: impl FnMut() -> bool) -> Option<()> {
    for _ in 0..tries {
        if done() {
            return Some(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Removes releases other than those containing `keep`; each release is 300+ MB.
pub fn prune(layout: &Layout, keep: &[&Path]) {
    let Ok(entries) = fs::read_dir(layout.host_base.join("releases")) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if !path.is_dir() || hidden || keep.iter().any(|kept| kept.starts_with(&path)) {
            continue;
        }
        let _ = Command::new("/bin/chmod")
            .args(["-R", "u+w"])
            .arg(&path)
            .status();
        let _ = fs::remove_dir_all(&path);
    }
}

/// Count persisted active threads and conservatively guard live runner locks.
/// A runner can still hold a checkout after an event gap marked its thread interrupted.
pub fn active_runs(store: &Path) -> Result<u64, Error> {
    active_runs_with(store, |database| {
        run(Command::new("/usr/bin/sqlite3")
            .args(["-cmd", "PRAGMA query_only=1"])
            .arg(database)
            .arg(ACTIVE_RUNS_QUERY))
    })
}

const ACTIVE_RUNS_QUERY: &str = "SELECT count(*) AS active FROM threads WHERE json_extract(snapshot,'$.status') IN ('running','awaiting_approval')";

#[cfg(target_os = "linux")]
pub(super) fn active_runs_with_node(store: &Path, node: &Path) -> Result<u64, Error> {
    const COUNT: &str = "const { DatabaseSync } = require('node:sqlite');
        const db = new DatabaseSync(process.argv[1], { readOnly: true });
        console.log(db.prepare(process.argv[2]).get().active);";
    active_runs_with(store, |database| {
        run(Command::new(node)
            .env_remove("NODE_OPTIONS")
            .args(["--input-type=commonjs", "-e", COUNT, "--"])
            .arg(database)
            .arg(ACTIVE_RUNS_QUERY))
    })
}

fn active_runs_with(
    store: &Path,
    query: impl FnOnce(&Path) -> Result<String, Error>,
) -> Result<u64, Error> {
    let database = store.join("host.sqlite");
    let count = if database.try_exists()? {
        query(&database)?
            .trim()
            .parse()
            .map_err(|_| invalid("could not read active runs"))?
    } else {
        0
    };
    if count > 0 {
        return Ok(count);
    }
    live_runner_locks(store)
}

#[cfg(unix)]
fn live_runner_locks(store: &Path) -> Result<u64, Error> {
    use std::os::fd::AsRawFd;
    let entries = match fs::read_dir(store.join("runners")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut count = 0;
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "lock") {
            continue;
        }
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::WouldBlock {
                return Err(error.into());
            }
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(not(unix))]
fn live_runner_locks(_store: &Path) -> Result<u64, Error> {
    Ok(0)
}

/// Installs an offered release's host for this machine. Used by `plait-host update`.
pub fn apply_host_update(offer: &Offer, home: &Path) -> Result<PathBuf, Error> {
    let layout = Layout::for_user(home);
    let artifact = offer.manifest.host().ok_or_else(|| {
        Error::Unsupported(format!(
            "This release has no host for {}.",
            super::platform()
        ))
    })?;
    if legacy_install() {
        return Err(Error::Unsupported(MIGRATE.into()));
    }
    let runs = active_runs(&layout.store)?;
    if runs > 0 {
        return Err(Error::Busy(runs));
    }
    let cache = layout.cache.join(format!("host-{}", offer.version));
    let archive = super::download(offer, artifact, &cache)?;
    let program = install_host(&layout, &archive, &offer.manifest.host_build_id)?;
    let runs = active_runs(&layout.store)?;
    if runs > 0 {
        return Err(Error::Busy(runs));
    }
    activate(&layout, &program)?;
    prune(&layout, &[&program]);
    let _ = fs::remove_dir_all(&cache);
    Ok(program)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("plait-update-{name}{}", unique("")));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn host_archive(root: &Path, build_id: &str, component_id: &str) -> PathBuf {
        let source = root.join("source");
        fs::create_dir_all(source.join(build_id).join("bin")).unwrap();
        fs::write(
            source.join(build_id).join("component.json"),
            serde_json::to_vec(&serde_json::json!({"component": "host", "build_id": component_id}))
                .unwrap(),
        )
        .unwrap();
        let archive = root.join(format!("{component_id}.tar.gz"));
        run(Command::new("/usr/bin/tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg(build_id))
        .unwrap();
        fs::remove_dir_all(&source).unwrap();
        archive
    }

    fn cleanup(root: &Path) {
        let _ = Command::new("/bin/chmod")
            .args(["-R", "u+w"])
            .arg(root)
            .status();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_releases_install_once_read_only_and_prune_unused_ones() {
        let root = scratch("host");
        let layout = Layout::for_user(&root.join("home"));
        let first = "0.1.0-0000000000000000000a";
        let second = "0.1.0-0000000000000000000b";
        let program = install_host(&layout, &host_archive(&root, first, first), first).unwrap();
        assert_eq!(program, layout.executable(first));
        let release = layout.host_base.join("releases").join(first);
        assert!(fs::metadata(&release).unwrap().permissions().readonly());
        // Reinstalling keeps the existing, possibly running, release.
        install_host(&layout, &root.join("missing.tar.gz"), first).unwrap();

        let next = install_host(&layout, &host_archive(&root, second, second), second).unwrap();
        let installed: serde_json::Value =
            serde_json::from_slice(&fs::read(layout.host_base.join("installed.json")).unwrap())
                .unwrap();
        assert_eq!(installed["build_id"], second);
        prune(&layout, &[&next]);
        assert!(!release.exists());
        assert!(layout.host_base.join("releases").join(second).exists());
        cleanup(&root);
    }

    #[test]
    fn host_archives_must_match_their_signed_build() {
        let root = scratch("mismatch");
        let layout = Layout::for_user(&root.join("home"));
        let claimed = "0.1.0-0000000000000000000c";
        let archive = host_archive(&root, claimed, "0.1.0-0000000000000000000d");
        assert!(matches!(
            install_host(&layout, &archive, claimed),
            Err(Error::Invalid(_))
        ));
        assert!(!layout.host_base.join("releases").join(claimed).exists());
        assert!(matches!(
            install_host(&layout, &archive, "../escape"),
            Err(Error::Invalid(_))
        ));
        cleanup(&root);
    }

    #[test]
    fn agent_definition_escapes_the_program_path() {
        let plist = agent_plist(Path::new("/Users/a&b/<Host>/bin/plait-host"));
        assert!(plist.contains("<string>/Users/a&amp;b/&lt;Host&gt;/bin/plait-host</string>"));
        assert!(plist.contains("<string>run-user</string>"));
        assert!(plist.contains("<key>AbandonProcessGroup</key><true/>"));
    }

    #[test]
    fn active_runs_counts_running_and_waiting_threads() {
        let root = scratch("runs");
        assert_eq!(active_runs(&root).unwrap(), 0);
        let database = root.join("host.sqlite");
        run(Command::new("/usr/bin/sqlite3").arg(&database).arg(
            "PRAGMA journal_mode=WAL; CREATE TABLE threads (id TEXT, snapshot TEXT); INSERT INTO threads VALUES \
             ('a','{\"status\":\"running\"}'),('b','{\"status\":\"awaiting_approval\"}'),('c','{\"status\":\"idle\"}');",
        ))
        .unwrap();
        assert_eq!(active_runs(&root).unwrap(), 2);
        cleanup(&root);
    }
}

#[cfg(all(test, unix))]
mod runner_lock_tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn active_runs_waits_for_a_live_runner_after_interruption() {
        let root = std::env::temp_dir().join(unique("runner-lock-test"));
        fs::create_dir_all(root.join("runners")).unwrap();
        run(Command::new("/usr/bin/sqlite3").arg(root.join("host.sqlite")).arg(
            "CREATE TABLE threads (id TEXT, snapshot TEXT); INSERT INTO threads VALUES ('a', '{\"status\":\"interrupted\"}');",
        )).unwrap();
        let lock = fs::File::create(root.join("runners/run.lock")).unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(active_runs(&root).unwrap(), 1);
        drop(lock);
        assert_eq!(active_runs(&root).unwrap(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
