//! Signed release updates from GitHub, shared by the desktop and headless hosts.
//!
//! Each release publishes `update.json` and a detached Ed25519 signature. The
//! signature covers the version and every artifact digest, so an altered feed
//! cannot pair an older signed build with a newer version number. Blocking
//! network and file work must run on the caller's background executor.

pub mod install;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod setup;

use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub const REPOSITORY: &str = "heath0xFF/plait";
pub const MANIFEST: &str = "update.json";
pub const SIGNATURE: &str = "update.json.sig";
/// Release signing key. Its private half is the `PLAIT_UPDATE_SIGNING_KEY`
/// Actions secret; see docs/RELEASING.md before replacing it.
const PUBLIC_KEY: &str = "6dc3a8ed96fa5c2f4f7b1ef2fd006abf6f376981a2f48b4a3c837b51812b6312";
const FEED_LIMIT: u64 = 8 << 20;
const MANIFEST_LIMIT: u64 = 64 << 10;
const ARTIFACT_LIMIT: u64 = 2 << 30;

#[derive(Debug)]
pub enum Error {
    Network(String),
    Signature,
    Invalid(String),
    Unsupported(String),
    /// Runs that a host restart would interrupt.
    Busy(u64),
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(detail) => write!(f, "Could not reach GitHub Releases: {detail}"),
            Self::Signature => f.write_str("The update is not signed by the Plait release key."),
            Self::Invalid(detail) => write!(f, "The update is invalid: {detail}"),
            Self::Unsupported(detail) => f.write_str(detail),
            Self::Busy(1) => {
                f.write_str("1 run is active or needs approval. Finish or stop it, then update.")
            }
            Self::Busy(runs) => write!(
                f,
                "{runs} runs are active or need approval. Finish or stop them, then update."
            ),
            Self::Io(error) => write!(f, "Update failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn invalid(detail: impl Into<String>) -> Error {
    Error::Invalid(detail.into())
}

/// A release version: `MAJOR.MINOR.PATCH`, optionally `-preview.N`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    preview: Option<u64>,
}

impl Version {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let (core, preview) = match text.split_once("-preview.") {
            Some((core, preview)) => (core, Some(number(preview)?)),
            None => (text, None),
        };
        let parts = core.split('.').map(number).collect::<Result<Vec<_>, _>>()?;
        let [major, minor, patch] = parts[..] else {
            return Err(invalid(format!("unrecognized version {text:?}")));
        };
        Ok(Self {
            core: [major, minor, patch],
            preview,
        })
    }
}

fn number(text: &str) -> Result<u64, Error> {
    let canonical = !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if !canonical {
        return Err(invalid(format!("unrecognized version number {text:?}")));
    }
    text.parse()
        .map_err(|_| invalid("version number is too large"))
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // A final release follows every preview of the same version.
        self.core
            .cmp(&other.core)
            .then(match (self.preview, other.preview) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(&b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.core;
        write!(f, "{major}.{minor}.{patch}")?;
        if let Some(preview) = self.preview {
            write!(f, "-preview.{preview}")?;
        }
        Ok(())
    }
}

/// The release this binary was built as. Source builds have none and are
/// offered the newest release.
pub fn current_version() -> Option<Version> {
    option_env!("PLAIT_RELEASE_VERSION").and_then(|text| Version::parse(text).ok())
}

/// The artifact suffix for this machine, such as `macos-arm64`.
pub fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-arm64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-arm64",
        _ => "unsupported",
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub version: String,
    pub tag: String,
    pub revision: String,
    pub host_build_id: String,
    /// Keyed by `client-<platform>` or `host-<platform>`.
    pub artifacts: BTreeMap<String, Artifact>,
}

/// A signed release for one independently updated component.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentManifest {
    pub schema_version: u32,
    pub component: Component,
    pub version: String,
    pub tag: String,
    pub revision: String,
    pub protocol: ProtocolRange,
    pub build_id: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub storage_schema: Option<u32>,
    pub artifacts: BTreeMap<String, Artifact>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    Client,
    Host,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

impl Manifest {
    pub fn client(&self) -> Option<&Artifact> {
        self.artifacts.get(&format!("client-{}", platform()))
    }

    pub fn host(&self) -> Option<&Artifact> {
        self.artifacts.get(&format!("host-{}", platform()))
    }
}

/// A verified release newer than the running build.
#[derive(Clone, Debug)]
pub struct Offer {
    pub manifest: Manifest,
    pub version: Version,
}

impl Offer {
    pub fn notes_url(&self) -> String {
        format!(
            "https://github.com/{REPOSITORY}/releases/tag/{}",
            self.manifest.tag
        )
    }
}

pub fn public_key() -> Result<VerifyingKey, Error> {
    let bytes = decode_hex::<32>(PUBLIC_KEY).ok_or(Error::Signature)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| Error::Signature)
}

fn verify_signature(bytes: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<(), Error> {
    let text = std::str::from_utf8(signature).map_err(|_| Error::Signature)?;
    let signature = decode_hex::<64>(text.trim()).ok_or(Error::Signature)?;
    key.verify_strict(bytes, &Signature::from_bytes(&signature))
        .map_err(|_| Error::Signature)
}

/// Checks the signature before parsing, then the manifest's own invariants.
pub fn verify(bytes: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<Manifest, Error> {
    verify_signature(bytes, signature, key)?;
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?;
    if manifest.schema_version != 1 {
        return Err(invalid("unsupported manifest schema"));
    }
    Version::parse(&manifest.version)?;
    if manifest.tag != format!("v{}", manifest.version) {
        return Err(invalid("release tag does not match its version"));
    }
    if !valid_build_id(&manifest.host_build_id) {
        return Err(invalid("invalid host build identity"));
    }
    for artifact in manifest.artifacts.values() {
        let digest = artifact.sha256.len() == 64
            && artifact
                .sha256
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if !safe_name(&artifact.name)
            || !digest
            || artifact.size == 0
            || artifact.size > ARTIFACT_LIMIT
        {
            return Err(invalid(format!("invalid artifact {:?}", artifact.name)));
        }
    }
    Ok(manifest)
}

/// Verify the detached signature and the invariants for one component.
pub fn verify_component(
    bytes: &[u8],
    signature: &[u8],
    key: &VerifyingKey,
    expected: Component,
) -> Result<ComponentManifest, Error> {
    verify_signature(bytes, signature, key)?;
    let manifest: ComponentManifest =
        serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?;
    if manifest.schema_version != 2 || manifest.component != expected {
        return Err(invalid("unsupported manifest schema or component"));
    }
    Version::parse(&manifest.version)?;
    if manifest.tag != format!("v{}", manifest.version) {
        return Err(invalid("release tag does not match its version"));
    }
    if manifest.protocol.min > manifest.protocol.max {
        return Err(invalid("invalid protocol range"));
    }
    if manifest.artifacts.is_empty() {
        return Err(invalid("the manifest names no artifact"));
    }
    match manifest.component {
        Component::Client
            if manifest.build_id.is_some()
                || manifest.capabilities.is_some()
                || manifest.storage_schema.is_some() =>
        {
            return Err(invalid("unexpected host fields"));
        }
        Component::Host
            if !manifest.build_id.as_deref().is_some_and(valid_build_id)
                || manifest.storage_schema.is_none()
                || !manifest.capabilities.as_ref().is_some_and(|items| {
                    items.len() <= 64
                        && items.iter().all(|item| item.len() <= 64 && item.is_ascii())
                }) =>
        {
            return Err(invalid("invalid host metadata"));
        }
        _ => {}
    }
    for (platform, artifact) in &manifest.artifacts {
        let digest = artifact.sha256.len() == 64
            && artifact
                .sha256
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if !safe_name(platform)
            || !safe_name(&artifact.name)
            || !digest
            || artifact.size == 0
            || artifact.size > ARTIFACT_LIMIT
        {
            return Err(invalid(format!("invalid artifact {:?}", artifact.name)));
        }
    }
    Ok(manifest)
}

/// Host release directories are named `<version>-<20 hex digits>`.
pub fn valid_build_id(id: &str) -> bool {
    let Some((version, digest)) = id.rsplit_once('-') else {
        return false;
    };
    Version::parse(version).is_ok_and(|version| version.preview.is_none())
        && digest.len() == 20
        && digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn decode_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return None;
    }
    let mut output = [0; N];
    for (index, pair) in bytes.chunks(2).enumerate() {
        let pair = std::str::from_utf8(pair).ok()?;
        output[index] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(output)
}

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

fn asset_url(tag: &str, name: &str) -> String {
    format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}")
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

/// A host archive for another machine, from a release whose signed manifest
/// this Mac has verified. The machine downloads it and checks `sha256`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRelease {
    pub tag: String,
    pub build_id: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_order_previews_before_the_final_release() {
        let ordered = [
            "0.1.0-preview.2",
            "0.1.0-preview.10",
            "0.1.0",
            "0.1.1-preview.1",
            "0.2.0",
        ];
        for pair in ordered.windows(2) {
            assert!(version(pair[0]) < version(pair[1]), "{pair:?}");
        }
        assert_eq!(version("0.1.0-preview.3").to_string(), "0.1.0-preview.3");
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-beta.1",
            "1.2.3-preview.",
            "1.2.3-preview.01",
            "v1.2.3",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
    }

    fn manifest(version: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1, "version": version, "tag": format!("v{version}"),
            "revision": "1eb61f1259dd", "host_build_id": "0.1.0-014483b69f94cba19406",
            "artifacts": {"client-macos-arm64": {"name": "Plait-0.1.0-1eb61f1259dd-macos-arm64.zip", "sha256": "a".repeat(64), "size": 10}},
        }))
        .unwrap()
    }

    #[test]
    fn manifests_need_the_release_signature_and_safe_contents() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let other = SigningKey::from_bytes(&[8; 32]);
        let sign = |key: &SigningKey, bytes: &[u8]| {
            key.sign(bytes)
                .to_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let bytes = manifest("0.1.0-preview.3");
        let verified = verify(&bytes, sign(&key, &bytes).as_bytes(), &key.verifying_key()).unwrap();
        assert_eq!(verified.tag, "v0.1.0-preview.3");

        // A valid signature from another key, or over other bytes, is rejected.
        assert!(matches!(
            verify(
                &bytes,
                sign(&other, &bytes).as_bytes(),
                &key.verifying_key()
            ),
            Err(Error::Signature)
        ));
        let newer = manifest("0.1.0-preview.9");
        assert!(matches!(
            verify(&newer, sign(&key, &bytes).as_bytes(), &key.verifying_key()),
            Err(Error::Signature)
        ));
        assert!(matches!(
            verify(&bytes, b"not hex", &key.verifying_key()),
            Err(Error::Signature)
        ));

        // Signed but malformed manifests are still refused.
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["artifacts"]["client-macos-arm64"]["name"] = "../Plait.zip".into();
        let traversal = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            verify(
                &traversal,
                sign(&key, &traversal).as_bytes(),
                &key.verifying_key()
            ),
            Err(Error::Invalid(_))
        ));
        value["artifacts"]["client-macos-arm64"]["name"] = "Plait.zip".into();
        value["tag"] = "v9.9.9".into();
        let mismatch = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            verify(
                &mismatch,
                sign(&key, &mismatch).as_bytes(),
                &key.verifying_key()
            ),
            Err(Error::Invalid(_))
        ));
    }

    /// Manifests written by scripts/update_manifest.py, read from PLAIT_MANIFEST_DIR.
    #[test]
    #[ignore = "set PLAIT_MANIFEST_DIR to manifests the release script wrote"]
    fn script_manifests_decode_and_verify() {
        let dir = std::path::PathBuf::from(std::env::var_os("PLAIT_MANIFEST_DIR").unwrap());
        let key = SigningKey::from_bytes(&[9; 32]);
        for (name, component) in [
            ("update-client.json", Component::Client),
            ("update-host.json", Component::Host),
        ] {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            let signature: String = key
                .sign(&bytes)
                .to_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            verify_component(
                &bytes,
                signature.as_bytes(),
                &key.verifying_key(),
                component,
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    #[test]
    fn component_manifests_verify_and_reject_invalid_inputs() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let sign = |bytes: &[u8]| {
            key.sign(bytes)
                .to_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let mut value = serde_json::json!({
            "schema_version": 2, "component": "host", "version": "0.1.0",
            "tag": "v0.1.0", "revision": "abc", "protocol": {"min": 1, "max": 6},
            "build_id": "0.1.0-014483b69f94cba19406", "capabilities": [],
            "storage_schema": 1, "artifacts": {"macos-arm64": {
                "name": "host.tar.gz", "sha256": "a".repeat(64), "size": 10}}
        });
        let bytes = serde_json::to_vec(&value).unwrap();
        let signature = sign(&bytes);
        assert!(
            verify_component(
                &bytes,
                signature.as_bytes(),
                &key.verifying_key(),
                Component::Host
            )
            .is_ok()
        );
        assert!(matches!(
            verify_component(
                b"tampered",
                signature.as_bytes(),
                &key.verifying_key(),
                Component::Host
            ),
            Err(Error::Signature)
        ));
        assert!(
            verify_component(
                &bytes,
                signature.as_bytes(),
                &key.verifying_key(),
                Component::Client
            )
            .is_err()
        );
        for (field, bad) in [
            ("protocol", serde_json::json!({"min": 7, "max": 6})),
            ("unknown", serde_json::json!(true)),
        ] {
            let old = value.get(field).cloned();
            value[field] = bad;
            let invalid_bytes = serde_json::to_vec(&value).unwrap();
            assert!(
                verify_component(
                    &invalid_bytes,
                    sign(&invalid_bytes).as_bytes(),
                    &key.verifying_key(),
                    Component::Host
                )
                .is_err()
            );
            if let Some(old) = old {
                value[field] = old;
            } else {
                value.as_object_mut().unwrap().remove(field);
            }
        }
        value["artifacts"] = serde_json::json!({"linux-x86_64": {
            "name": "host.tar.gz", "sha256": "a".repeat(64), "size": 10
        }});
        let linux_bytes = serde_json::to_vec(&value).unwrap();
        assert!(
            verify_component(
                &linux_bytes,
                sign(&linux_bytes).as_bytes(),
                &key.verifying_key(),
                Component::Host,
            )
            .is_ok()
        );
        let legacy = manifest("0.1.0");
        assert!(
            verify_component(
                &legacy,
                sign(&legacy).as_bytes(),
                &key.verifying_key(),
                Component::Host
            )
            .is_err()
        );
        assert!(verify(&bytes, signature.as_bytes(), &key.verifying_key()).is_err());
    }

    #[test]
    fn the_built_in_release_key_is_valid() {
        public_key().unwrap();
    }

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

    #[test]
    fn host_build_ids_match_the_packaged_form() {
        assert!(valid_build_id("0.1.0-014483b69f94cba19406"));
        for bad in [
            "0.1.0",
            "0.1.0-014483b69f94cba1940",
            "0.1.0-014483B69F94CBA19406",
            "../0.1.0-014483b69f94cba19406",
        ] {
            assert!(!valid_build_id(bad), "{bad}");
        }
    }
}
