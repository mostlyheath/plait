//! First-launch setup of a missing local server.
//!
//! An app dragged into Applications carries its host as a bootstrap copy. On
//! first launch, with no server installed, that copy is installed into the
//! per-user release folder and registered with launchd. Setup never stops,
//! replaces, or downgrades a server that already exists: replacement belongs to
//! the update path, which checks for active work first.
//!
//! Probing and deciding are separate so the decision can be tested without
//! launchd. Everything here blocks; run it on a background executor.

use super::{
    Error,
    install::{self, BUNDLE_ID, Layout},
    invalid, valid_build_id,
};
use std::{
    fs::{self, File},
    os::unix::io::AsRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// What setup found, gathered before anything is changed.
#[derive(Debug, Default)]
pub struct Facts {
    /// The `.app` this process runs from; `None` for a source build.
    pub bundle: Option<PathBuf>,
    /// The bundled host archive and its build identity, if the bundle has one.
    pub bundled: Option<(PathBuf, String)>,
    /// An explicit development store (`PLAIT_HOST_STORE`) is selected.
    pub store_override: bool,
    /// An earlier system-wide package install is present.
    pub legacy: bool,
    /// launchd already has the host service loaded.
    pub service_loaded: bool,
    /// Something answers on the default host socket, such as a manual host.
    pub answering: bool,
    /// The executable of the release recorded in `installed.json`, if present.
    pub installed: Option<PathBuf>,
}

/// Why setup does nothing on this launch.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not an app bundle, or an explicit development store: never provision.
    Development,
    /// Running from the disk image or a quarantined download location.
    NotInApplications,
    /// A system-wide install needs its explicit one-time migration.
    Legacy,
    /// The bundle has no valid embedded host.
    Malformed,
}

/// What setup will do.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// A server is already running or registered. Leave it alone.
    Existing,
    /// The installed release exists but is not registered: register it,
    /// never the bundle's copy, so an installed newer server is not downgraded.
    Register(PathBuf),
    /// No server is installed: install the bundled copy, then register it.
    Install {
        archive: PathBuf,
        build_id: String,
    },
    Refused(Refusal),
}

/// What happened, for the desktop to present.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing was changed because a server already exists.
    Existing,
    /// A server was installed or registered and now answers.
    Started,
    Refused(Refusal),
}

impl Refusal {
    /// What the person can do, or `None` when nothing needs saying.
    pub fn guidance(&self) -> Option<&'static str> {
        match self {
            Self::Development => None,
            Self::NotInApplications => Some(
                "Plait is running from the disk image or Downloads. Drag Plait to Applications, \
                 then open it from there to finish setup.",
            ),
            Self::Legacy => Some(
                "This Mac has an earlier system-wide Plait install. Open Move from system-wide \
                 install.command from the latest disk image once to move it to your user account.",
            ),
            Self::Malformed => Some(
                "This copy of Plait is missing its server. Download it again from the releases page.",
            ),
        }
    }
}

/// Decides what to do. Order matters: development and placement checks come
/// before anything that could register a service, and an existing server
/// always wins over the bundled copy.
pub fn plan(facts: &Facts) -> Plan {
    let Some(bundle) = &facts.bundle else {
        return Plan::Refused(Refusal::Development);
    };
    if facts.store_override {
        return Plan::Refused(Refusal::Development);
    }
    if !installed_location(bundle) {
        return Plan::Refused(Refusal::NotInApplications);
    }
    if facts.legacy {
        return Plan::Refused(Refusal::Legacy);
    }
    if facts.service_loaded || facts.answering {
        return Plan::Existing;
    }
    if let Some(program) = &facts.installed {
        return Plan::Register(program.clone());
    }
    match &facts.bundled {
        Some((archive, build_id)) if valid_build_id(build_id) => Plan::Install {
            archive: archive.clone(),
            build_id: build_id.clone(),
        },
        _ => Plan::Refused(Refusal::Malformed),
    }
}

/// A mounted disk image or Gatekeeper's randomized read-only copy of a
/// quarantined download. A service registered from there would point at a
/// location that disappears, and updates could not replace the app.
fn installed_location(bundle: &Path) -> bool {
    let text = bundle.to_string_lossy();
    !(bundle.starts_with("/Volumes") || text.contains("/AppTranslocation/"))
}

/// Probes, decides, and carries out setup, serialized across processes.
pub fn ensure(home: &Path) -> Result<Outcome, Error> {
    let layout = Layout::for_user(home);
    if std::env::var_os("PLAIT_HOST_STORE").is_some() {
        return Ok(Outcome::Refused(Refusal::Development));
    }
    let Some(bundle) = running_bundle() else {
        return Ok(Outcome::Refused(Refusal::Development));
    };
    if !installed_location(&bundle) {
        return Ok(Outcome::Refused(Refusal::NotInApplications));
    }
    refuse_symlinks(&layout)?;
    let _lock = lock(&layout)?;
    // Probe under the lock: another launch may have finished setup meanwhile.
    let facts = Facts {
        bundled: bundled_host(&bundle)?,
        bundle: Some(bundle),
        store_override: false,
        legacy: install::legacy_install(),
        service_loaded: install::running_program().is_some(),
        answering: answering(&layout),
        installed: installed_program(&layout),
    };
    match plan(&facts) {
        Plan::Existing => Ok(Outcome::Existing),
        Plan::Refused(refusal) => Ok(Outcome::Refused(refusal)),
        Plan::Register(program) => {
            install::register(&layout, &program)?;
            Ok(Outcome::Started)
        }
        Plan::Install { archive, build_id } => {
            let program = install::install_host(&layout, &archive, &build_id)?;
            install::register(&layout, &program)?;
            Ok(Outcome::Started)
        }
    }
}

/// The `.app` this executable runs from, without the ownership checks updates
/// need: first setup never writes the bundle.
fn running_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    executable
        .ancestors()
        .nth(3)
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .map(Path::to_owned)
}

/// The bundle's host archive, after checking the bundle is an intact Plait.
fn bundled_host(bundle: &Path) -> Result<Option<(PathBuf, String)>, Error> {
    let resources = bundle.join("Contents/Resources");
    let archive = resources.join("Host.tar.gz");
    let Ok(metadata) = fs::read(resources.join("host.json")) else {
        return Ok(None);
    };
    if !archive.is_file() {
        return Ok(None);
    }
    let host: serde_json::Value =
        serde_json::from_slice(&metadata).map_err(|error| invalid(error.to_string()))?;
    let Some(build_id) = host["build_id"]
        .as_str()
        .filter(|_| host["component"] == "host")
    else {
        return Ok(None);
    };
    verify_bundle(bundle)?;
    Ok(Some((archive, build_id.to_owned())))
}

fn verify_bundle(bundle: &Path) -> Result<(), Error> {
    let signed = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !signed.success() {
        return Err(invalid("the app failed its signature check"));
    }
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .stdin(Stdio::null())
        .output()?;
    if String::from_utf8_lossy(&output.stdout).trim() != BUNDLE_ID {
        return Err(invalid("the app is not Plait"));
    }
    Ok(())
}

/// The recorded release's executable, only if its files are actually there.
fn installed_program(layout: &Layout) -> Option<PathBuf> {
    let bytes = fs::read(layout.host_base.join("installed.json")).ok()?;
    let installed: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let build_id = installed["build_id"]
        .as_str()
        .filter(|id| valid_build_id(id))?;
    let program = layout.executable(build_id);
    program.is_file().then_some(program)
}

fn answering(layout: &Layout) -> bool {
    crate::local_socket::connect(&layout.store.join("host.sock")).is_ok()
}

/// Setup writes under these folders; a symlink there could redirect the
/// install or the service definition somewhere the person did not choose.
fn refuse_symlinks(layout: &Layout) -> Result<(), Error> {
    let folders = [
        layout.host_base.clone(),
        layout.host_base.join("releases"),
        layout
            .agent
            .parent()
            .map(Path::to_owned)
            .unwrap_or_default(),
    ];
    for folder in folders {
        if fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(Error::Unsupported(format!(
                "{} is a symbolic link. Setup does not write through links; remove it and retry.",
                folder.display()
            )));
        }
    }
    Ok(())
}

/// Serializes setup across app instances. Released when the file closes.
fn lock(layout: &Layout) -> Result<File, Error> {
    fs::create_dir_all(&layout.host_base)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(layout.host_base.join(".setup.lock"))?;
    // Blocks until another launch's setup finishes, then probes afresh.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "0.1.0-0000000000000000000a";

    fn fresh() -> Facts {
        Facts {
            bundle: Some("/Applications/Plait.app".into()),
            bundled: Some((
                "/Applications/Plait.app/Contents/Resources/Host.tar.gz".into(),
                BUILD.into(),
            )),
            ..Facts::default()
        }
    }

    #[test]
    fn a_fresh_mac_installs_the_bundled_server() {
        assert_eq!(
            plan(&fresh()),
            Plan::Install {
                archive: "/Applications/Plait.app/Contents/Resources/Host.tar.gz".into(),
                build_id: BUILD.into(),
            }
        );
    }

    #[test]
    fn a_running_or_manual_server_is_never_touched() {
        let loaded = Facts {
            service_loaded: true,
            ..fresh()
        };
        assert_eq!(plan(&loaded), Plan::Existing);
        let manual = Facts {
            answering: true,
            ..fresh()
        };
        assert_eq!(plan(&manual), Plan::Existing);
        // Even with an installed release that differs from the bundle's.
        let both = Facts {
            service_loaded: true,
            installed: Some("/x/bin/plait-host".into()),
            ..fresh()
        };
        assert_eq!(plan(&both), Plan::Existing);
    }

    #[test]
    fn an_installed_server_is_registered_instead_of_the_bundled_copy() {
        // The installed release may be newer than the bundle: never downgrade.
        let program = PathBuf::from(
            "/Users/a/Library/Application Support/Plait/Host/releases/0.2.0-0000000000000000000b/bin/plait-host",
        );
        let facts = Facts {
            installed: Some(program.clone()),
            ..fresh()
        };
        assert_eq!(plan(&facts), Plan::Register(program));
    }

    #[test]
    fn source_builds_and_development_stores_never_provision() {
        let source = Facts {
            bundle: None,
            ..fresh()
        };
        assert_eq!(plan(&source), Plan::Refused(Refusal::Development));
        let store = Facts {
            store_override: true,
            ..fresh()
        };
        assert_eq!(plan(&store), Plan::Refused(Refusal::Development));
        assert_eq!(Refusal::Development.guidance(), None);
    }

    #[test]
    fn the_disk_image_and_translocated_downloads_ask_to_move_first() {
        for bundle in [
            "/Volumes/Plait/Plait.app",
            "/private/var/folders/x/AppTranslocation/1234/d/Plait.app",
        ] {
            let facts = Facts {
                bundle: Some(bundle.into()),
                ..fresh()
            };
            assert_eq!(
                plan(&facts),
                Plan::Refused(Refusal::NotInApplications),
                "{bundle}"
            );
        }
        // Outside Applications but a stable, user-chosen place is fine.
        let home = Facts {
            bundle: Some("/Users/a/Applications/Plait.app".into()),
            ..fresh()
        };
        assert!(matches!(plan(&home), Plan::Install { .. }));
    }

    #[test]
    fn a_legacy_install_needs_explicit_migration() {
        let facts = Facts {
            legacy: true,
            ..fresh()
        };
        assert_eq!(plan(&facts), Plan::Refused(Refusal::Legacy));
        // Checked before an existing service: a loaded legacy host still needs migration.
        let loaded = Facts {
            legacy: true,
            service_loaded: true,
            ..fresh()
        };
        assert_eq!(plan(&loaded), Plan::Refused(Refusal::Legacy));
    }

    #[test]
    fn a_bundle_without_a_valid_host_is_refused() {
        let missing = Facts {
            bundled: None,
            ..fresh()
        };
        assert_eq!(plan(&missing), Plan::Refused(Refusal::Malformed));
        let unsafe_id = Facts {
            bundled: Some(("/a".into(), "../escape".into())),
            ..fresh()
        };
        assert_eq!(plan(&unsafe_id), Plan::Refused(Refusal::Malformed));
    }

    #[test]
    fn installed_program_requires_the_release_files() {
        let root = std::env::temp_dir().join(format!("plait-setup-{}", std::process::id()));
        let layout = Layout::for_user(&root);
        fs::create_dir_all(&layout.host_base).unwrap();
        fs::write(
            layout.host_base.join("installed.json"),
            format!(r#"{{"component":"host","build_id":"{BUILD}"}}"#),
        )
        .unwrap();
        assert_eq!(installed_program(&layout), None);
        let program = layout.executable(BUILD);
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "").unwrap();
        assert_eq!(installed_program(&layout), Some(program));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn setup_refuses_to_write_through_symbolic_links() {
        let root = std::env::temp_dir().join(format!("plait-setup-link-{}", std::process::id()));
        let layout = Layout::for_user(&root);
        fs::create_dir_all(layout.host_base.parent().unwrap()).unwrap();
        fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), &layout.host_base).unwrap();
        assert!(matches!(
            refuse_symlinks(&layout),
            Err(Error::Unsupported(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
