//! Host release installation and shared artifact downloads.
pub mod install;
#[cfg(target_os = "linux")]
pub mod linux;

use plait_protocol::update::*;
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

const FEED_LIMIT: u64 = 8 << 20;
const MANIFEST_LIMIT: u64 = 64 << 10;

#[derive(Debug, PartialEq, Eq)]
struct Candidate {
    tag: String,
    version: Version,
}

/// The newest published release that carries a signed manifest. Releases
/// made before the updater existed have none and are skipped.
fn newest(feed: &[u8]) -> Result<Option<Candidate>, Error> {
    let releases: Vec<serde_json::Value> =
        serde_json::from_slice(feed).map_err(|error| Error::Network(error.to_string()))?;
    let mut best: Option<Candidate> = None;
    for release in releases {
        if release["draft"].as_bool() != Some(false) {
            continue;
        }
        let Some(tag) = release["tag_name"].as_str() else {
            continue;
        };
        let Some(Ok(version)) = tag.strip_prefix('v').map(Version::parse) else {
            continue;
        };
        let assets = release["assets"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let has = |name: &str| assets.iter().any(|asset| asset["name"] == name);
        if !(has(MANIFEST) && has(SIGNATURE)) {
            continue;
        }
        if best.as_ref().is_none_or(|best| version > best.version) {
            best = Some(Candidate {
                tag: tag.to_owned(),
                version,
            });
        }
    }
    Ok(best)
}

/// Finds a signed release newer than `current`, or the newest one for a source build.
pub fn check(current: Option<&Version>) -> Result<Option<Offer>, Error> {
    // The releases list includes previews, which `releases/latest` omits.
    let feed = fetch(
        &format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=30"),
        FEED_LIMIT,
    )?;
    let Some(candidate) = newest(&feed)? else {
        return Ok(None);
    };
    if current.is_some_and(|current| candidate.version <= *current) {
        return Ok(None);
    }
    let bytes = fetch(&asset_url(&candidate.tag, MANIFEST), MANIFEST_LIMIT)?;
    let signature = fetch(&asset_url(&candidate.tag, SIGNATURE), 1024)?;
    let manifest = verify(&bytes, &signature, &public_key()?)?;
    if manifest.tag != candidate.tag {
        return Err(invalid("release tag does not match its signed manifest"));
    }
    // Only the signed version decides whether this is newer.
    let version = Version::parse(&manifest.version)?;
    if current.is_some_and(|current| version <= *current) {
        return Ok(None);
    }
    Ok(Some(Offer { manifest, version }))
}

/// The newest release's host for `platform` (such as `linux-x86_64`), checked
/// against its signed manifest. Linux hosts have one manifest per platform.
pub fn host_release(platform: &str) -> Result<HostRelease, Error> {
    let feed = fetch(
        &format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=30"),
        FEED_LIMIT,
    )?;
    let candidate = newest(&feed)?.ok_or_else(|| invalid("no release is published yet"))?;
    let name = if platform.starts_with("macos-") {
        "update-host.json".to_owned()
    } else {
        format!("update-host-{platform}.json")
    };
    let bytes = fetch(&asset_url(&candidate.tag, &name), MANIFEST_LIMIT)
        .map_err(|_| invalid(format!("the newest release has no host for {platform} yet")))?;
    let signature = fetch(&asset_url(&candidate.tag, &format!("{name}.sig")), 1024)?;
    let manifest = verify_component(&bytes, &signature, &public_key()?, Component::Host)?;
    host_release_from(&manifest, platform)
}

fn host_release_from(manifest: &ComponentManifest, platform: &str) -> Result<HostRelease, Error> {
    let artifact = manifest
        .artifacts
        .get(platform)
        .ok_or_else(|| invalid(format!("the release has no host for {platform}")))?;
    let build_id = manifest
        .build_id
        .clone()
        .filter(|id| valid_build_id(id))
        .ok_or_else(|| invalid("invalid host build"))?;
    Ok(HostRelease {
        tag: manifest.tag.clone(),
        build_id,
        url: asset_url(&manifest.tag, &artifact.name),
        sha256: artifact.sha256.clone(),
        size: artifact.size,
    })
}

fn fetch(url: &str, limit: u64) -> Result<Vec<u8>, Error> {
    let output = curl()
        .args([
            "--max-time",
            "60",
            "--max-filesize",
            &limit.to_string(),
            url,
        ])
        .output()?;
    if !output.status.success() {
        return Err(Error::Network(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    if output.stdout.len() as u64 > limit {
        return Err(Error::Network("response is too large".into()));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod feed_tests {
    use super::*;
    #[test]
    fn feed_picks_the_newest_signed_release_including_previews() {
        let signed = serde_json::json!([{"name": MANIFEST}, {"name": SIGNATURE}]);
        let feed = serde_json::json!([
            {"tag_name": "v0.1.0-preview.2", "draft": false, "prerelease": true, "assets": [{"name": "Plait.dmg"}]},
            {"tag_name": "v0.1.0-preview.4", "draft": true, "prerelease": true, "assets": signed},
            {"tag_name": "v0.1.0-preview.3", "draft": false, "prerelease": true, "assets": signed},
            {"tag_name": "v0.1.0-preview.10", "draft": false, "prerelease": true, "assets": signed},
            {"tag_name": "nightly", "draft": false, "assets": signed},
        ]);
        let best = newest(&serde_json::to_vec(&feed).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(best.tag, "v0.1.0-preview.10");
        assert_eq!(newest(b"[]").unwrap(), None);
        assert!(newest(b"{\"message\":\"rate limited\"}").is_err());
    }
}
