//! Bounded startup diagnostics. Record phases, not provider errors or credentials.

use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct Startup {
    path: PathBuf,
    started: Instant,
    record: Value,
}

impl Startup {
    pub fn new(store: &Path) -> Self {
        Self {
            path: store.join("startup.json"),
            started: Instant::now(),
            record: json!({
                "schema_version": 1,
                "pid": std::process::id(),
                "started_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
                "events": [],
            }),
        }
    }

    pub fn phase(&mut self, phase: &'static str) -> io::Result<()> {
        let events = self.record["events"].as_array_mut().unwrap();
        if events.len() == 8 {
            events.remove(0);
        }
        events.push(json!({ "phase": phase, "elapsed_ms": self.started.elapsed().as_millis() }));
        self.record["phase"] = phase.into();
        self.save()
    }

    pub fn child(&mut self, pid: u32) {
        self.record["sidecar_pid"] = pid.into();
    }

    pub fn failed(&mut self, error: &io::Error) {
        // Raw sidecar stderr can contain credentials. An OS error kind is safe.
        self.record["error_kind"] = format!("{:?}", error.kind()).into();
        let _ = self.phase("failed");
    }

    fn save(&self) -> io::Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = self
            .path
            .with_extension(format!("{}.{nonce}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        let result = (|| {
            serde_json::to_writer(&mut file, &self.record)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn phases_are_private_bounded_and_never_include_raw_errors() {
        let root =
            plait_test_support::temp_dir().join(format!("plait-startup-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut startup = Startup::new(&root);
        startup.phase("starting_sidecar").unwrap();
        startup.child(123);
        startup.phase("waiting_for_runtime").unwrap();
        startup.failed(&io::Error::new(
            io::ErrorKind::TimedOut,
            "PRIVATE_CREDENTIAL",
        ));
        let path = root.join("startup.json");
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("PRIVATE_CREDENTIAL"));
        assert!(text.len() < 2048);
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["phase"], "failed");
        assert_eq!(value["error_kind"], "TimedOut");
        assert_eq!(value["sidecar_pid"], 123);
        assert_eq!(value["events"].as_array().unwrap().len(), 3);
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }
}
