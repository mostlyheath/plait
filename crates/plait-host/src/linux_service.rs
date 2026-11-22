use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const UNIT: &str = "plait-host.service";
const DESCRIPTION: &str = "Description=Plait host";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

// Values are quoted for systemd's syntax, not a shell; percent signs are specifiers.
fn unit_value(path: &Path, exec: bool) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::other("service path is not UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(io::Error::other("service path contains control characters"));
    }
    let mut escaped = String::new();
    for ch in text.chars() {
        match ch {
            '%' => escaped.push_str("%%"),
            '$' if exec => escaped.push_str("$$"),
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            _ => escaped.push(ch),
        }
    }
    Ok(format!("\"{escaped}\""))
}

pub fn unit_text(executable: &Path, node_dir: Option<&Path>) -> io::Result<String> {
    if !executable.is_absolute() || node_dir.is_some_and(|dir| !dir.is_absolute()) {
        return Err(io::Error::other("service paths must be absolute"));
    }
    let executable = unit_value(executable, true)?;
    let path = match node_dir {
        Some(dir) => {
            let dir = dir
                .to_str()
                .ok_or_else(|| io::Error::other("node path is not UTF-8"))?;
            if dir.contains(':') {
                return Err(io::Error::other("node directory contains a PATH separator"));
            }
            format!("{dir}:{SYSTEM_PATH}")
        }
        None => SYSTEM_PATH.to_owned(),
    };
    let path = unit_value(Path::new(&format!("PATH={path}")), false)?;
    Ok(format!(
        "[Unit]\n{DESCRIPTION}\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={executable} run-user\nRestart=on-failure\nRestartSec=5\nEnvironment={path}\n\n[Install]\nWantedBy=default.target\n"
    ))
}

fn unit_path_for(unit: &str) -> io::Result<PathBuf> {
    let base = match env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => {
            PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?)
                .join(".config")
        }
    };
    if !base.is_absolute() {
        return Err(io::Error::other("configuration directory must be absolute"));
    }
    Ok(base.join("systemd/user").join(unit))
}

/// Refuses a symlink at the unit file or the two folders this command
/// creates under the configuration directory. Folders above those, such as
/// a `/home` that links to `/var/home`, are the system's own layout.
fn reject_symlinks(path: &Path) -> io::Result<()> {
    let ours = path.ancestors().take(3).collect::<Vec<_>>();
    for current in ours.into_iter().rev() {
        match fs::symlink_metadata(current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "refusing symlink: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn systemctl(args: &[&str]) -> io::Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "systemctl {} failed: {status}",
            args.join(" ")
        )));
    }
    Ok(())
}

fn node_directory() -> io::Result<PathBuf> {
    let path =
        env::var_os("PATH").ok_or_else(|| io::Error::other("PATH is not set; cannot find node"))?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join("node");
        if let Ok(metadata) = fs::metadata(&candidate)
            && metadata.is_file()
            && metadata.permissions().mode() & 0o111 != 0
        {
            let absolute = if dir.is_absolute() {
                dir
            } else {
                env::current_dir()?.join(dir)
            };
            return fs::canonicalize(absolute);
        }
    }
    Err(io::Error::other("node was not found on PATH"))
}

pub fn install(tailnet: bool) -> io::Result<()> {
    let unit = if tailnet {
        "plait-host-tailnet.service"
    } else {
        UNIT
    };
    let executable = env::current_exe()?;
    let runtime = crate::installation::runtime_paths(&executable)?;
    let node_dir = if runtime.node == "node" {
        Some(node_directory()?)
    } else {
        None
    };
    let mut text = unit_text(&executable, node_dir.as_deref())?;
    if tailnet {
        text = text
            .replace(
                "Description=Plait host",
                "Description=Plait tailnet listener",
            )
            .replace(" run-user", " serve --tailnet");
    }
    let path = unit_path_for(unit)?;
    reject_symlinks(&path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("unit has no directory"))?;
    fs::create_dir_all(parent)?;
    reject_symlinks(&path)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temp = parent.join(format!(".{unit}.{}.{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temp)?;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        file.write_all(text.as_bytes())?;
        reject_symlinks(&path)?;
        fs::rename(&temp, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    println!("Wrote {}", path.display());
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", unit])?;
    println!("Enabled and started {unit}");
    if let Some(user) = env::var_os("USER")
        && let Ok(output) = Command::new("loginctl")
            .arg("show-user")
            .arg(&user)
            .args(["-p", "Linger"])
            .output()
        && output.status.success()
        && String::from_utf8_lossy(&output.stdout).trim() == "Linger=no"
    {
        println!(
            "The host stops when you log out. Run loginctl enable-linger $USER to keep it running."
        );
    }
    Ok(())
}

pub fn uninstall() -> io::Result<()> {
    systemctl(&["disable", "--now", UNIT])?;
    let path = unit_path_for(UNIT)?;
    reject_symlinks(&path)?;
    match fs::read_to_string(&path) {
        Ok(text) if text.lines().any(|line| line == DESCRIPTION) => {
            fs::remove_file(&path)?;
            println!("Removed {}", path.display());
        }
        Ok(_) => println!("Left {}: description does not match", path.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    systemctl(&["daemon-reload"])?;
    println!("Disabled {UNIT} and reloaded systemd");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_and_packaged_paths() {
        let source = unit_text(Path::new("/host"), Some(Path::new("/home/me/node/bin"))).unwrap();
        assert!(
            source.contains("Environment=\"PATH=/home/me/node/bin:/usr/local/bin:/usr/bin:/bin\"")
        );
        let packaged = unit_text(Path::new("/host"), None).unwrap();
        assert!(packaged.contains("Environment=\"PATH=/usr/local/bin:/usr/bin:/bin\""));
    }
    #[test]
    fn rejects_controls_and_quotes_spaces() {
        assert!(unit_text(Path::new("/bad\npath"), None).is_err());
        assert!(unit_text(Path::new("/host"), Some(Path::new("/bad\tpath"))).is_err());
        let text = unit_text(Path::new("/some dir/host"), Some(Path::new("/node dir"))).unwrap();
        assert!(text.contains("ExecStart=\"/some dir/host\" run-user"));
        assert!(text.contains("Environment=\"PATH=/node dir:/usr/local/bin:/usr/bin:/bin\""));
    }

    #[test]
    fn a_linked_home_is_fine_but_a_linked_unit_is_refused() {
        let root = std::env::temp_dir().join(format!("plait-unit-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("real/.config/systemd/user")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("home")).unwrap();
        let unit = root.join("home/.config/systemd/user/plait-host.service");
        assert!(
            reject_symlinks(&unit).is_ok(),
            "a linked home folder is the system's layout"
        );
        fs::write(root.join("elsewhere"), "").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), &unit).unwrap();
        assert!(
            reject_symlinks(&unit).is_err(),
            "a linked unit file is refused"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
