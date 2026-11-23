//! Bounded private records, written under the desktop journal's lock.
use super::Journal;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};

static SERIAL: AtomicU64 = AtomicU64::new(1);

impl Journal {
    pub fn remove_record(&self, name: &str) -> Result<(), String> {
        valid_name(name)?;
        match fs::remove_file(self.record_directory()?.join(name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    /// Blocking: caller runs this on the background executor.
    pub fn write_record<T: Serialize>(
        &self,
        name: &str,
        value: &T,
        limit: usize,
    ) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        self.write_record_bytes(name, &bytes, limit)
    }

    pub fn write_record_bytes(&self, name: &str, bytes: &[u8], limit: usize) -> Result<(), String> {
        valid_name(name)?;
        if bytes.len() > limit {
            return Err("Saved tab data exceeds its storage limit.".into());
        }
        let directory = self.record_directory()?;
        let temporary = directory.join(format!(
            ".{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, directory.join(name))?;
            File::open(&directory)?.sync_all()
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(format!("Cannot save tab data: {error}"));
        }
        Ok(())
    }

    pub fn read_record<T: DeserializeOwned>(
        &self,
        name: &str,
        limit: usize,
    ) -> Result<Option<T>, String> {
        self.read_record_bytes(name, limit)?
            .map(|bytes| {
                serde_json::from_slice(&bytes).map_err(|e| format!("Invalid tab data: {e}"))
            })
            .transpose()
    }

    pub fn read_record_bytes(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>, String> {
        valid_name(name)?;
        let directory = self.record_directory()?;
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join(name))
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Cannot read tab data: {e}")),
        };
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.len() > limit as u64
        {
            return Err("Tab data is not a bounded owned file.".into());
        }
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > limit {
            return Err("Tab data exceeds its storage limit.".into());
        }
        Ok(Some(bytes))
    }

    fn record_directory(&self) -> Result<std::path::PathBuf, String> {
        let directory = self.directory.join("tabs");
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        let metadata = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("Tab storage must be an owned directory, not a symlink.".into());
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        Ok(directory)
    }
}

fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err("Invalid tab record name.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_are_bounded_private_and_refuse_symlinks() {
        let directory =
            std::env::temp_dir().join(format!("plait-tab-records-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let (journal, _) = Journal::open(&directory, "/host".into()).unwrap();
        journal.write_record("editor", &vec!["kept"], 1024).unwrap();
        assert_eq!(
            journal
                .read_record::<Vec<String>>("editor", 1024)
                .unwrap()
                .unwrap(),
            ["kept"]
        );
        assert_eq!(
            fs::metadata(directory.join("tabs/editor")).unwrap().mode() & 0o777,
            0o600
        );
        assert!(journal.write_record("editor", &"oversize", 1).is_err());
        assert!(journal.read_record_bytes("../editor", 1024).is_err());
        std::os::unix::fs::symlink("editor", directory.join("tabs/link")).unwrap();
        assert!(journal.read_record_bytes("link", 1024).is_err());
        assert!(journal.read_record_bytes("editor", 1).is_err());
        drop(journal);
        fs::remove_dir_all(directory).unwrap();
    }
}
