use serde_json::Value;
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub struct RuntimePaths {
    pub node: OsString,
    pub runtime: PathBuf,
}

pub fn runtime_paths(executable: &Path) -> io::Result<RuntimePaths> {
    let root = executable
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("cannot locate host installation"))?;
    let manifest = match File::open(root.join("component.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RuntimePaths {
                node: "node".into(),
                runtime: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../runtime/src/main.ts"),
            });
        }
        Err(error) => return Err(error),
    };
    let manifest: Value = serde_json::from_reader(manifest.take(65537))?;
    // The manifest names the newest protocol the host speaks.
    if manifest["schema_version"] != 1
        || manifest["component"] != "host"
        || !(1..=plait_protocol::host::PROTOCOL_VERSION)
            .any(|version| manifest["protocol_version"] == version)
    {
        return Err(io::Error::other("incompatible host installation manifest"));
    }
    // A packaged host must use its own immutable runtime, never a shell's Node.
    Ok(RuntimePaths {
        node: root.join("bin/node").into_os_string(),
        runtime: root.join("runtime/src/main.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_host_uses_adjacent_runtime_and_rejects_unknown_contracts() {
        let root = std::env::temp_dir().join(format!(
            "plait-installation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let executable = root.join("bin/plait-host");
        let source = runtime_paths(&executable).unwrap();
        assert_eq!(source.node, "node");
        let manifest = root.join("component.json");
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"component":"host","protocol_version":1}"#,
        )
        .unwrap();
        let installed = runtime_paths(&executable).unwrap();
        assert_eq!(installed.node, root.join("bin/node").as_os_str());
        assert_eq!(installed.runtime, root.join("runtime/src/main.ts"));
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"component":"host","protocol_version":2}"#,
        )
        .unwrap();
        assert!(runtime_paths(&executable).is_ok());
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"component":"host","protocol_version":3}"#,
        )
        .unwrap();
        assert!(runtime_paths(&executable).is_ok());
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"component":"host","protocol_version":4}"#,
        )
        .unwrap();
        assert!(runtime_paths(&executable).is_ok());
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"component":"host","protocol_version":5}"#,
        )
        .unwrap();
        assert!(runtime_paths(&executable).is_ok());
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"component":"host","protocol_version":7}"#,
        )
        .unwrap();
        assert!(runtime_paths(&executable).is_ok());
        for invalid in [
            r#"{"schema_version":1,"component":"client","protocol_version":1}"#,
            r#"{"schema_version":1,"component":"host","protocol_version":8}"#,
            "not json",
        ] {
            std::fs::write(&manifest, invalid).unwrap();
            assert!(runtime_paths(&executable).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
