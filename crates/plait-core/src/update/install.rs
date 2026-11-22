//! Per-user installation: immutable host releases, the LaunchAgent that runs
//! the active host, and replacement of the running app bundle.
//!
//! Everything lives in locations the user owns, so updates need no
//! administrator access. Earlier previews installed root-owned files with
//! Installer packages; those need a one-time migration first.

use super::{Error, Offer, invalid, valid_build_id};
use std::{
    fs,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const LABEL: &str = "dev.plait.host";
pub const BUNDLE_ID: &str = "dev.plait.desktop";
const LEGACY_AGENT: &str = "/Library/LaunchAgents/dev.plait.host.plist";
const MIGRATE: &str = "This Plait was installed system-wide by an Installer package. \
    Download the latest release and open Move from system-wide install.command from its disk image once \
    to move it to a per-user install; later updates install from here.";

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

/// Runs a host restart would interrupt. Mirrors the runtime store's recovery
/// query. Query-only rather than read-only: a read-only open of a WAL store
/// fails while no host is running, because it cannot create the shared memory.
pub fn active_runs(store: &Path) -> Result<u64, Error> {
    let database = store.join("host.sqlite");
    if !database.exists() {
        return Ok(0);
    }
    let output = run(Command::new("/usr/bin/sqlite3")
        .args(["-cmd", "PRAGMA query_only=1"])
        .arg(&database)
        .arg(
        "SELECT count(*) FROM threads WHERE json_extract(snapshot,'$.status') IN ('running','awaiting_approval')",
    ))?;
    output
        .trim()
        .parse()
        .map_err(|_| invalid("could not read active runs"))
}

/// The bundle this app runs from, if updates can replace it in place.
pub fn running_bundle() -> Result<PathBuf, Error> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let bundle = executable
        .ancestors()
        .nth(3)
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .ok_or_else(|| {
            Error::Unsupported(
                "Updates apply to an installed Plait.app, not a source build.".into(),
            )
        })?;
    let parent = bundle
        .parent()
        .ok_or_else(|| invalid("bundle has no parent"))?;
    // Package installs are root-owned; replacing them would need administrator access.
    let owned = fs::metadata(bundle)?.uid() == uid() && writable(parent);
    if !owned || legacy_install() {
        return Err(Error::Unsupported(MIGRATE.into()));
    }
    Ok(bundle.to_owned())
}

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// Unpacks a downloaded app archive and checks it is an intact Plait bundle.
pub fn unpack_app(archive: &Path, directory: &Path) -> Result<PathBuf, Error> {
    if directory.exists() {
        let _ = Command::new("/bin/chmod")
            .args(["-R", "u+w"])
            .arg(directory)
            .status();
        fs::remove_dir_all(directory)?;
    }
    fs::create_dir_all(directory)?;
    run(Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(directory))?;
    let bundle = directory.join("Plait.app");
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&bundle))?;
    let identifier = run(Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist")))?;
    if identifier.trim() != BUNDLE_ID {
        return Err(invalid("archive does not contain Plait"));
    }
    Ok(bundle)
}

/// Swaps `next` into place. The running process keeps its already-open files.
pub fn replace_app(current: &Path, next: &Path) -> Result<(), Error> {
    let parent = current
        .parent()
        .ok_or_else(|| invalid("bundle has no parent"))?;
    // Copy beside the target first, so the swap is two renames on one volume.
    let incoming = parent.join(unique("Plait.app.next"));
    let outgoing = parent.join(unique("Plait.app.previous"));
    run(Command::new("/usr/bin/ditto").arg(next).arg(&incoming))?;
    if let Err(error) = fs::rename(current, &outgoing) {
        let _ = fs::remove_dir_all(&incoming);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&incoming, current) {
        let _ = fs::rename(&outgoing, current);
        let _ = fs::remove_dir_all(&incoming);
        return Err(error.into());
    }
    let _ = fs::remove_dir_all(&outgoing);
    Ok(())
}

/// Reopens `bundle` after this process exits.
pub fn relaunch(bundle: &Path) -> Result<(), Error> {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", r#"while /bin/kill -0 "$1" 2>/dev/null; do /bin/sleep 0.2; done; exec /usr/bin/open "$2""#, "plait-relaunch"])
        .arg(process::id().to_string())
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Outlive the app's process group.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()?;
    Ok(())
}

/// Installs an offered release over the running app and its host. Returns the
/// bundle to relaunch. Refuses while runs are active, before changing anything.
pub fn apply_app_update(offer: &Offer, home: &Path) -> Result<PathBuf, Error> {
    let bundle = running_bundle()?;
    let layout = Layout::for_user(home);
    let runs = active_runs(&layout.store)?;
    if runs > 0 {
        return Err(Error::Busy(runs));
    }
    let client = offer.manifest.client().ok_or_else(|| {
        Error::Unsupported(format!(
            "This release has no app for {}.",
            super::platform()
        ))
    })?;
    let cache = layout.cache.join(offer.version.to_string());
    let archive = super::download(offer, client, &cache)?;
    let next = unpack_app(&archive, &cache.join("unpacked"))?;
    let resources = next.join("Contents/Resources");
    let host: serde_json::Value = serde_json::from_slice(&fs::read(resources.join("host.json"))?)
        .map_err(|error| invalid(error.to_string()))?;
    if host["build_id"] != offer.manifest.host_build_id.as_str() {
        return Err(invalid("the app's host does not match the signed release"));
    }
    let program = install_host(
        &layout,
        &resources.join("Host.tar.gz"),
        &offer.manifest.host_build_id,
    )?;
    // Check again just before the restart; downloading can take a while.
    let runs = active_runs(&layout.store)?;
    if runs > 0 {
        return Err(Error::Busy(runs));
    }
    activate(&layout, &program)?;
    prune(&layout, &[&program]);
    replace_app(&bundle, &next)?;
    let _ = fs::remove_dir_all(&cache);
    Ok(bundle)
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
    fn app_swap_replaces_the_bundle_and_leaves_no_staging() {
        let root = scratch("swap");
        let current = root.join("Applications/Plait.app");
        let next = root.join("next/Plait.app");
        for (bundle, marker) in [(&current, "old"), (&next, "new")] {
            fs::create_dir_all(bundle.join("Contents")).unwrap();
            fs::write(bundle.join("Contents/marker"), marker).unwrap();
        }
        replace_app(&current, &next).unwrap();
        assert_eq!(
            fs::read_to_string(current.join("Contents/marker")).unwrap(),
            "new"
        );
        assert_eq!(fs::read_dir(root.join("Applications")).unwrap().count(), 1);
        cleanup(&root);
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
