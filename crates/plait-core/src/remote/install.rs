//! Installing a Plait host on another machine over SSH, as that person,
//! without `sudo`. This Mac verifies the release's signed manifest; the
//! machine downloads the archive itself and checks its SHA-256 before
//! anything is unpacked.
use super::ssh::{SshDestination, SshOptions, base_arguments};
use crate::{host::Error, update::HostRelease};
use std::{
    io::Write,
    process::{Command, Stdio},
};

/// What this Mac learned about the machine before installing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The platform a host archive is published for, such as `linux-x86_64`.
    pub platform: Option<String>,
    /// `uname -sm`, to name an unsupported machine.
    pub system: String,
    /// A host already answers `plait-host version` there.
    pub installed: bool,
    pub downloader: bool,
}

const PROBE: &str = r#"printf 'system=%s\n' "$(uname -sm)"
if command -v plait-host >/dev/null 2>&1 || [ -x "$HOME/.local/bin/plait-host" ]; then echo installed=yes; else echo installed=no; fi
if command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1; then echo downloader=yes; else echo downloader=no; fi
"#;

/// The platform name host archives use for `uname -sm`.
pub fn platform(system: &str) -> Option<&'static str> {
    match system.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["Linux", "x86_64" | "amd64"] => Some("linux-x86_64"),
        ["Linux", "aarch64" | "arm64"] => Some("linux-arm64"),
        _ => None,
    }
}

fn parse_probe(output: &str) -> Probe {
    let field = |name: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let system = field("system");
    Probe {
        platform: platform(&system).map(str::to_owned),
        installed: field("installed") == "yes",
        downloader: field("downloader") == "yes",
        system,
    }
}

/// Blocking. Asks the machine what it is, over the person's own SSH.
pub fn probe(destination: &SshDestination, options: &SshOptions) -> Result<Probe, Error> {
    let output = run_script(destination, options, PROBE)?;
    Ok(parse_probe(&output))
}

/// Single-quotes a value for `sh`; release values are already restricted, this
/// keeps the script safe if that ever changes.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The script the machine runs. Each step names itself on failure, and
/// nothing replaces an installed host until the new one is unpacked and checked.
pub fn install_script(release: &HostRelease) -> String {
    format!(
        r#"set -eu
url={url}; sum={sum}; build={build}
step() {{ echo "plait-install: $1"; }}
fail() {{ echo "plait-install-error: $1" >&2; exit 1; }}
base="$HOME/.local/share/plait-host/releases"
mkdir -p "$base" "$HOME/.local/bin"
tmp=$(mktemp -d "$base/.install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
step download
if command -v curl >/dev/null 2>&1; then
  curl --proto '=https' --fail --silent --show-error --location -o "$tmp/host.tar.gz" "$url" || fail "Download failed."
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$tmp/host.tar.gz" "$url" || fail "Download failed."
else
  fail "This machine has neither curl nor wget."
fi
step verify
if command -v sha256sum >/dev/null 2>&1; then got=$(sha256sum "$tmp/host.tar.gz" | cut -d' ' -f1)
else got=$(shasum -a 256 "$tmp/host.tar.gz" | cut -d' ' -f1); fi
[ "$got" = "$sum" ] || fail "The download does not match the signed release."
step unpack
if [ ! -x "$base/$build/bin/plait-host" ]; then
  tar -xzf "$tmp/host.tar.gz" -C "$tmp" || fail "The archive could not be unpacked."
  [ -x "$tmp/$build/bin/plait-host" ] || fail "The archive is not the expected host."
  rm -rf "$base/$build"
  mv "$tmp/$build" "$base/$build"
fi
ln -sfn "$base/$build/bin/plait-host" "$HOME/.local/bin/plait-host"
step start
"$base/$build/bin/plait-host" install-service || fail "The host service did not start."
if loginctl show-user "$(id -un)" -p Linger 2>/dev/null | grep -q '=yes'; then echo linger=yes; else echo linger=no; fi
step done
"#,
        url = quote(&release.url),
        sum = quote(&release.sha256),
        build = quote(&release.build_id),
    )
}

/// How an installation went, step by step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The host keeps running after logout only with lingering on.
    pub linger: bool,
}

/// Blocking. Installs `release` on the machine and starts its host.
pub fn install(
    destination: &SshDestination,
    options: &SshOptions,
    release: &HostRelease,
) -> Result<Installed, Error> {
    let output = run_script(destination, options, &install_script(release))?;
    if !output.lines().any(|line| line == "plait-install: done") {
        return Err(Error::Transport("The installation did not finish.".into()));
    }
    Ok(Installed {
        linger: output.lines().any(|line| line == "linger=yes"),
    })
}

/// Runs `script` with `sh -s` on the machine and returns what it printed. A
/// failure reports the script's own reason, or SSH's.
fn run_script(
    destination: &SshDestination,
    options: &SshOptions,
    script: &str,
) -> Result<String, Error> {
    let mut child = Command::new(&options.program)
        .args(base_arguments(destination, options))
        .args(["sh", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Transport(format!("Could not start SSH: {e}")))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(script.as_bytes())
        .map_err(|e| Error::Transport(format!("SSH closed early: {e}")))?;
    let output = child
        .wait_with_output()
        .map_err(|e| Error::Transport(format!("SSH failed: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let reason = stderr
            .lines()
            .find_map(|line| line.strip_prefix("plait-install-error: "))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                stderr
                    .trim()
                    .lines()
                    .last()
                    .unwrap_or("SSH failed.")
                    .to_owned()
            });
        return Err(Error::Transport(reason));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn platforms_map_to_published_archives() {
        assert_eq!(platform("Linux x86_64"), Some("linux-x86_64"));
        assert_eq!(platform("Linux aarch64"), Some("linux-arm64"));
        assert_eq!(platform("Darwin arm64"), None);
        assert_eq!(platform("Linux riscv64"), None);
        let probe = parse_probe("system=Linux aarch64\ninstalled=no\ndownloader=yes\n");
        assert_eq!(probe.platform.as_deref(), Some("linux-arm64"));
        assert!(!probe.installed && probe.downloader);
    }

    /// A scripted `ssh` that runs the remote words locally, under a fake home.
    fn fake_ssh(home: &std::path::Path) -> SshOptions {
        let ssh = home.join("ssh");
        std::fs::write(
            &ssh,
            format!(
                "#!/bin/sh\n# Drop options up to the destination, then run the remote words.\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift 2\nHOME={0} PATH={0}/bin:/usr/bin:/bin exec \"$@\"\n",
                home.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        SshOptions {
            program: ssh.into(),
            ..SshOptions::default()
        }
    }

    #[test]
    fn a_mismatched_download_installs_nothing() {
        let home = std::env::temp_dir().join(format!("plait-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let options = fake_ssh(&home);
        let destination = SshDestination::new("machine").unwrap();
        let probe = probe(&destination, &options).unwrap();
        assert!(!probe.installed);
        // A file:// URL is refused by curl's https-only rule, so the download fails cleanly.
        let release = HostRelease {
            tag: "v0.1.0".into(),
            build_id: format!("0.1.0-{}", "a".repeat(20)),
            url: "file:///etc/hostname".into(),
            sha256: "0".repeat(64),
            size: 1,
        };
        let error = install(&destination, &options, &release)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Download failed") || error.contains("does not match"),
            "{error}"
        );
        assert!(
            !home.join(".local/bin/plait-host").exists(),
            "a failed install linked a host"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_verified_download_installs_links_and_starts_the_host() {
        use std::process::Command;
        let home = std::env::temp_dir().join(format!("plait-install-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let build = format!("0.1.0-{}", "b".repeat(20));
        // The archive: one folder named by the build, holding a host that records its arguments.
        let stage = home.join("stage");
        std::fs::create_dir_all(stage.join(&build).join("bin")).unwrap();
        let host = stage.join(&build).join("bin/plait-host");
        std::fs::write(
            &host,
            format!("#!/bin/sh\necho \"$@\" >> {}/called\n", home.display()),
        )
        .unwrap();
        std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
        let archive = home.join("host.tar.gz");
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&stage)
                .arg(&build)
                .status()
                .unwrap()
                .success()
        );
        let sum = crate::update::sha256_file(&archive).unwrap();
        // A curl that serves the archive for any URL.
        let curl = home.join("bin/curl");
        std::fs::write(
            &curl,
            format!(
                "#!/bin/sh\nwhile [ \"$1\" != \"-o\" ]; do shift; done\ncp {} \"$2\"\n",
                archive.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Lingering off, whatever the machine running the test has.
        let loginctl = home.join("bin/loginctl");
        std::fs::write(&loginctl, "#!/bin/sh\necho Linger=no\n").unwrap();
        std::fs::set_permissions(&loginctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let options = fake_ssh(&home);
        let destination = SshDestination::new("machine").unwrap();
        let release = HostRelease {
            tag: "v0.1.0".into(),
            build_id: build.clone(),
            url: "https://example.invalid/host.tar.gz".into(),
            sha256: sum,
            size: 1,
        };
        let installed = install(&destination, &options, &release).unwrap();
        assert!(!installed.linger);
        let link = std::fs::read_link(home.join(".local/bin/plait-host")).unwrap();
        assert_eq!(
            link,
            home.join(".local/share/plait-host/releases")
                .join(&build)
                .join("bin/plait-host")
        );
        assert_eq!(
            std::fs::read_to_string(home.join("called")).unwrap(),
            "install-service\n"
        );
        assert!(probe(&destination, &options).unwrap().installed);
        // Installing the same build again keeps it and starts it again.
        install(&destination, &options, &release).unwrap();
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn the_script_quotes_release_values() {
        let release = HostRelease {
            tag: "v1".into(),
            build_id: "1.0.0-abc".into(),
            url: "https://example.invalid/a'b".into(),
            sha256: "f".repeat(64),
            size: 1,
        };
        assert!(install_script(&release).contains("url='https://example.invalid/a'\\''b'"));
    }
}
