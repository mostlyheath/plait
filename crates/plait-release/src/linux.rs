//! Linux host release installation and service activation.
use super::{Error, HostRelease, Version, invalid, valid_build_id};
use std::{
    fs,
    io::Read,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn unique() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

pub fn running_build(home: &Path, executable: &Path) -> Result<String, Error> {
    let exe = executable.canonicalize()?;
    let root = home
        .join(".local/share/plait-host/releases")
        .canonicalize()
        .map_err(|_| source_error())?;
    let relative = exe.strip_prefix(root).map_err(|_| source_error())?;
    let parts: Vec<_> = relative.components().collect();
    if let [
        Component::Normal(id),
        Component::Normal(bin),
        Component::Normal(host),
    ] = parts.as_slice()
        && *bin == "bin"
        && *host == "plait-host"
    {
        let id = id.to_string_lossy();
        if valid_build_id(&id) {
            return Ok(id.into_owned());
        }
    }
    Err(source_error())
}
fn source_error() -> Error {
    Error::Unsupported(
        "This host was built from source; use scripts/install.sh --host or rebuild it.".into(),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Current,
    Offer(Version),
}
pub fn decide(
    release: &HostRelease,
    running: &str,
    current: Option<&Version>,
) -> Result<Decision, Error> {
    if release.build_id == running {
        return Ok(Decision::Current);
    }
    let version = Version::parse(
        release
            .tag
            .strip_prefix('v')
            .ok_or_else(|| invalid("invalid release tag"))?,
    )?;
    if current.is_some_and(|v| version < *v) {
        return Err(Error::Unsupported("Refusing to downgrade the host.".into()));
    }
    Ok(Decision::Offer(version))
}
fn units(home: &Path) -> Vec<(&'static str, bool)> {
    let mut units = vec![("plait-host.service", false)];
    if home
        .join(".config/systemd/user/plait-host-tailnet.service")
        .exists()
    {
        units.push(("plait-host-tailnet.service", true));
    }
    units
}
fn run(command: &mut Command) -> Result<(), Error> {
    let output = command.stdin(Stdio::null()).output()?;
    if !output.status.success() {
        return Err(Error::Unsupported(format!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
fn idle(home: &Path, host: &Path) -> Result<(), Error> {
    let count = super::install::active_runs_with_node(
        &home.join(".local/share/plait"),
        &host.with_file_name("node"),
    )?;
    if count > 0 {
        return Err(Error::Busy(count));
    }
    Ok(())
}
fn download(release: &HostRelease, path: &Path) -> Result<(), Error> {
    if !release.url.starts_with("https://") {
        return Err(invalid("host download must use HTTPS"));
    }
    let output = super::curl()
        .args([
            "--max-time",
            "1800",
            "--max-filesize",
            &release.size.to_string(),
            "--output",
        ])
        .arg(path)
        .arg(&release.url)
        .output()?;
    if !output.status.success() {
        return Err(Error::Network(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    Ok(())
}

fn check_links(directory: &Path, root: &Path) -> Result<(), Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            if !entry
                .path()
                .canonicalize()
                .map_err(|_| invalid("invalid archive symlink"))?
                .starts_with(root)
            {
                return Err(invalid("archive symlink escapes its release"));
            }
        } else if kind.is_dir() {
            check_links(&entry.path(), root)?;
        }
    }
    Ok(())
}
fn stage(home: &Path, release: &HostRelease, archive: &Path) -> Result<PathBuf, Error> {
    if !valid_build_id(&release.build_id) {
        return Err(invalid("invalid host build identity"));
    }
    if fs::metadata(archive)?.len() != release.size
        || super::sha256_file(archive)? != release.sha256
    {
        return Err(invalid("host archive size or checksum mismatch"));
    }
    let releases = home.join(".local/share/plait-host/releases");
    fs::create_dir_all(&releases)?;
    let destination = releases.join(&release.build_id);
    if destination.exists() {
        return Ok(destination.join("bin/plait-host"));
    }
    let staging = releases.join(format!(".stage.{}", unique()));
    fs::create_dir(&staging)?;
    let result = (|| {
        run(Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(&staging))?;
        // The archive matched the signed manifest; these checks catch a wrong or damaged build.
        let unpacked = staging.join(&release.build_id);
        if fs::read_dir(&staging)?.count() != 1 {
            return Err(invalid("host archive has more than its release folder"));
        }
        let component: serde_json::Value =
            serde_json::from_slice(&fs::read(unpacked.join("component.json"))?)
                .map_err(|error| invalid(error.to_string()))?;
        if component["component"] != "host" || component["build_id"] != release.build_id {
            return Err(invalid("host archive does not match its build identity"));
        }
        check_links(&unpacked, &unpacked.canonicalize()?)?;
        let executable = unpacked.join("bin/plait-host");
        let metadata = fs::symlink_metadata(&executable)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(invalid("host executable is missing or not executable"));
        }
        match fs::rename(&unpacked, &destination) {
            Ok(()) => (),
            Err(_) if destination.exists() => (),
            Err(error) => return Err(error.into()),
        }
        Ok(destination.join("bin/plait-host"))
    })();
    let _ = fs::remove_dir_all(staging);
    result
}
fn switch(program: &Path, units: &[(&str, bool)], systemctl: &Path) -> Result<(), Error> {
    for (_, tailnet) in units {
        let mut cmd = Command::new(program);
        cmd.arg("install-service");
        if *tailnet {
            cmd.arg("--tailnet");
        }
        run(&mut cmd)?;
    }
    for (unit, _) in units {
        run(Command::new(systemctl).args(["--user", "restart", unit]))?;
    }
    Ok(())
}
fn activate(
    new: &Path,
    old: &Path,
    units: &[(&str, bool)],
    systemctl: &Path,
    ready: impl Fn() -> bool,
) -> Result<(), Error> {
    let result = switch(new, units, systemctl).and_then(|()| {
        if ready() {
            Ok(())
        } else {
            Err(Error::Unsupported("The new host did not answer.".into()))
        }
    });
    if let Err(error) = result {
        return match switch(old, units, systemctl) {
            Ok(()) => Err(Error::Unsupported(format!(
                "{error} Previous release was restarted."
            ))),
            Err(rollback) => Err(Error::Unsupported(format!(
                "{error} Rollback failed: {rollback}"
            ))),
        };
    }
    Ok(())
}
/// Whether the host on `socket` answers `host_info` as `build` before `limit`.
/// The restart stopped the previous process, so an answer from the old build
/// means the service did not switch.
fn answers_as(socket: &Path, build: &str, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if host_build(socket).as_deref() == Some(build) {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    false
}

fn host_build(socket: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader, Write};
    let mut stream = crate::local_socket::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream.write_all(b"{\"op\":\"host_info\"}\n").ok()?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(64 * 1024)
        .read_line(&mut line)
        .ok()?;
    let reply: serde_json::Value = serde_json::from_str(&line).ok()?;
    reply["result"]["build_id"].as_str().map(str::to_owned)
}

fn prune_selected(name: &str, keep: &str) -> bool {
    name != keep && valid_build_id(name)
}

pub fn apply(home: &Path, old: &Path, release: &HostRelease) -> Result<PathBuf, Error> {
    idle(home, old)?;
    let base = home.join(".local/share/plait-host");
    fs::create_dir_all(&base)?;
    let temp = base.join(format!(".update.{}", unique()));
    fs::create_dir(&temp)?;
    let result = (|| {
        let archive = temp.join("host.tar.gz");
        download(release, &archive)?;
        let new = stage(home, release, &archive)?;
        idle(home, old)?;
        let selected = units(home);
        let socket = home.join(".local/share/plait/host.sock");
        activate(&new, old, &selected, Path::new("systemctl"), || {
            answers_as(&socket, &release.build_id, Duration::from_secs(20))
        })?;
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin)?;
        let link = bin.join(format!(".plait-host.{}", unique()));
        symlink(&new, &link)?;
        if let Err(error) = fs::rename(&link, bin.join("plait-host")) {
            let _ = fs::remove_file(link);
            return Err(error.into());
        }
        for entry in fs::read_dir(base.join("releases"))?.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir())
                && prune_selected(&entry.file_name().to_string_lossy(), &release.build_id)
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        Ok(new)
    })();
    let _ = fs::remove_dir_all(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scratch() -> PathBuf {
        let path = plait_test_support::temp_dir().join(format!("plait-linux-update-{}", unique()));
        fs::create_dir_all(&path).unwrap();
        path
    }
    const OLD: &str = "0.1.0-00000000000000000001";
    const NEW: &str = "0.2.0-00000000000000000002";
    fn release(archive: &Path) -> HostRelease {
        HostRelease {
            tag: "v0.2.0".into(),
            build_id: NEW.into(),
            url: "https://example.invalid/host".into(),
            sha256: super::super::sha256_file(archive).unwrap(),
            size: fs::metadata(archive).unwrap().len(),
        }
    }
    fn fixture(root: &Path, executable: bool) -> PathBuf {
        let source = root.join("source");
        fs::create_dir_all(source.join(NEW).join("bin")).unwrap();
        fs::write(
            source.join(NEW).join("component.json"),
            format!(r#"{{"component":"host","build_id":"{NEW}"}}"#),
        )
        .unwrap();
        if executable {
            let host = source.join(NEW).join("bin/plait-host");
            fs::write(&host, "host").unwrap();
            fs::set_permissions(host, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let archive = root.join("host.tar.gz");
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(source)
                .arg(NEW)
                .status()
                .unwrap()
                .success()
        );
        archive
    }
    #[test]
    fn running_build_paths_and_symlinks() {
        let root = scratch();
        let executable = root
            .join(".local/share/plait-host/releases")
            .join(OLD)
            .join("bin/plait-host");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "").unwrap();
        let link = root.join("link");
        symlink(&executable, &link).unwrap();
        assert_eq!(running_build(&root, &link).unwrap(), OLD);
        assert!(running_build(&root, &root.join("link").parent().unwrap().join("source")).is_err());
        fs::write(root.join("source"), "").unwrap();
        assert!(running_build(&root, &root.join("source")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn bundled_node_reads_active_threads_without_sqlite_cli() {
        let root = scratch();
        let store = root.join(".local/share/plait");
        fs::create_dir_all(&store).unwrap();
        let node = root.join("node");
        install_script(&node, "#!/bin/sh\nexec node \"$@\"\n");
        run(Command::new(&node).args(["-e", r#"const db = new (require('node:sqlite').DatabaseSync)(process.argv[1]); db.exec(`CREATE TABLE threads(snapshot TEXT); INSERT INTO threads VALUES ('{"status":"awaiting_approval"}');`);"#]).arg(store.join("host.sqlite"))).unwrap();
        assert!(matches!(
            idle(&root, &root.join("plait-host")),
            Err(Error::Busy(1))
        ));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn decisions_and_pruning() {
        let root = scratch();
        let archive = fixture(&root, true);
        let mut offer = release(&archive);
        assert_eq!(decide(&offer, NEW, None).unwrap(), Decision::Current);
        assert_eq!(
            decide(&offer, OLD, Some(&Version::parse("0.1.0").unwrap())).unwrap(),
            Decision::Offer(Version::parse("0.2.0").unwrap())
        );
        offer.tag = "v0.0.1".into();
        assert!(decide(&offer, OLD, Some(&Version::parse("0.1.0").unwrap())).is_err());
        assert!(prune_selected(OLD, NEW));
        assert!(!prune_selected(NEW, NEW));
        assert!(!prune_selected(".stage.123", NEW));
        assert!(!prune_selected("other", NEW));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stage_verifies_archive_and_keeps_existing_release() {
        let root = scratch();
        let archive = fixture(&root, true);
        let mut offer = release(&archive);
        offer.sha256 = "0".repeat(64);
        assert!(stage(&root, &offer, &archive).is_err());
        assert!(!root.join(".local/share/plait-host/releases").exists());
        offer = release(&archive);
        let program = stage(&root, &offer, &archive).unwrap();
        fs::write(&program, "kept").unwrap();
        stage(&root, &offer, &archive).unwrap();
        assert_eq!(fs::read_to_string(program).unwrap(), "kept");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stage_rejects_missing_host() {
        let root = scratch();
        let archive = fixture(&root, false);
        assert!(stage(&root, &release(&archive), &archive).is_err());
        assert!(
            !root
                .join(".local/share/plait-host/releases")
                .join(NEW)
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }
    /// Writes an executable script without this process ever holding it open
    /// for writing: a concurrent test's fork could inherit that descriptor, and
    /// running the script would then fail with ETXTBSY.
    fn install_script(path: &Path, body: &str) {
        let source = path.with_extension("source");
        fs::write(&source, body).unwrap();
        let copied = std::process::Command::new("/bin/cp")
            .arg(&source)
            .arg(path)
            .status()
            .unwrap();
        assert!(copied.success());
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[test]
    fn listener_switch_uses_the_installed_hosts_legacy_command() {
        let root = scratch();
        let host = root.join("host");
        install_script(
            &host,
            "#!/bin/sh\ntest \"$*\" = 'install-service --tailnet'\n",
        );
        switch(
            &host,
            &[("plait-host-tailnet.service", true)],
            Path::new("true"),
        )
        .unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn activation_rolls_back_after_failed_start() {
        let root = scratch();
        let old = root.join("old");
        let new = root.join("new");
        let systemctl = root.join("systemctl");
        let log = root.join("log");
        for (path, body) in [
            (
                &old,
                format!("#!/bin/sh\necho old >> '{}'\n", log.display()),
            ),
            (&new, "#!/bin/sh\nexit 0\n".into()),
            (&systemctl, "#!/bin/sh\nexit 0\n".into()),
        ] {
            install_script(path, &body);
        }
        assert!(
            activate(
                &new,
                &old,
                &[("plait-host.service", false)],
                &systemctl,
                || false
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(log).unwrap(), "old\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn readiness_waits_for_the_new_build_to_answer() {
        use std::io::{BufRead, BufReader, Write};
        let root = scratch();
        let socket = root.join("host.sock");
        assert!(!answers_as(&socket, NEW, Duration::from_millis(300)));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        // The client trusts only an owner-only socket, as the host makes it.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let server = thread::spawn(move || {
            for build in [OLD, NEW] {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                assert_eq!(line, "{\"op\":\"host_info\"}\n");
                writeln!(
                    &stream,
                    r#"{{"ok":true,"result":{{"build_id":"{build}"}}}}"#
                )
                .unwrap();
            }
        });
        // The first answer is the old build, still running; the second is the new one.
        assert!(answers_as(&socket, NEW, Duration::from_secs(5)));
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
