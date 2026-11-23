//! Signed release updates from GitHub, shared by the desktop and headless hosts.
//!
//! Each release publishes `update.json` and a detached Ed25519 signature. The
//! signature covers the version and every artifact digest, so an altered feed
//! cannot pair an older signed build with a newer version number. Blocking
//! network and file work must run on the caller's background executor.

pub mod install;
#[cfg(target_os = "linux")]
pub use plait_engine::update::linux;
pub mod setup;

pub use plait_protocol::update::*;

fn invalid(detail: impl Into<String>) -> Error {
    Error::Invalid(detail.into())
}

pub use plait_release::{HostRelease, check, host_release};

pub use plait_release::{download, sha256_file};

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
