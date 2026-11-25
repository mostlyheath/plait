//! Desktop app-bundle updates and compatibility exports for host installation.
use super::{Error, Offer, invalid};
pub use plait_release::install::*;
use std::{
    fs,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
pub const BUNDLE_ID: &str = "dev.plait.desktop";
const MIGRATE: &str = "This Plait was installed system-wide by an Installer package. Download the latest release and open Move from system-wide install.command from its disk image once to move it to a per-user install; later updates install from here.";
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    format!(".{prefix}-{}-{nanos}", process::id())
}
fn run(command: &mut Command) -> Result<String, Error> {
    let output = command.stdin(Stdio::null()).output()?;
    if !output.status.success() {
        return Err(Error::Unsupported(format!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("plait-update-{name}{}", unique("")));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
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
}
