//! Host release installation and shared artifact downloads.
pub mod install;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
use plait_protocol::update::Version;
use plait_protocol::update::{Artifact, Error, Offer, REPOSITORY, platform, valid_build_id};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRelease {
    pub tag: String,
    pub build_id: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}
fn invalid(detail: impl Into<String>) -> Error {
    Error::Invalid(detail.into())
}
fn asset_url(tag: &str, name: &str) -> String {
    format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}")
}
fn curl() -> Command {
    let mut command = Command::new(if cfg!(target_os = "macos") {
        "/usr/bin/curl"
    } else {
        "curl"
    });
    command
        .args(["--proto", "=https", "--proto-redir", "=https", "--tlsv1.2"])
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--user-agent", "plait-updater"])
        .stdin(Stdio::null());
    command
}
/// Downloads one artifact into `directory` and checks its signed size and digest.
pub fn download(offer: &Offer, artifact: &Artifact, directory: &Path) -> Result<PathBuf, Error> {
    fs::create_dir_all(directory)?;
    let path = directory.join(&artifact.name);
    if path.is_file() && sha256_file(&path)? == artifact.sha256 {
        return Ok(path);
    }
    let partial = directory.join(format!(".{}.partial", artifact.name));
    let output = curl()
        .args([
            "--max-time",
            "1800",
            "--max-filesize",
            &artifact.size.to_string(),
        ])
        .arg("--output")
        .arg(&partial)
        .arg(asset_url(&offer.manifest.tag, &artifact.name))
        .stdout(Stdio::null())
        .output()?;
    if !output.status.success() {
        let _ = fs::remove_file(&partial);
        return Err(Error::Network(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    if fs::metadata(&partial)?.len() != artifact.size || sha256_file(&partial)? != artifact.sha256 {
        let _ = fs::remove_file(&partial);
        return Err(invalid(format!(
            "{} does not match its signed digest",
            artifact.name
        )));
    }
    fs::rename(&partial, &path)?;
    Ok(path)
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
