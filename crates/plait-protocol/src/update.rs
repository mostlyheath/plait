//! Signed update manifest data and verification.
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::{cmp::Ordering, collections::BTreeMap, fmt, io};

pub const REPOSITORY: &str = "heath0xFF/plait";
pub const MANIFEST: &str = "update.json";
pub const SIGNATURE: &str = "update.json.sig";
/// Release signing key. Its private half is the `PLAIT_UPDATE_SIGNING_KEY`
/// Actions secret; see docs/RELEASING.md before replacing it.
const PUBLIC_KEY: &str = "6dc3a8ed96fa5c2f4f7b1ef2fd006abf6f376981a2f48b4a3c837b51812b6312";
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
