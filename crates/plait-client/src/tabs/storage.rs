//! Blocking, bounded tab-preference I/O. Call only on a background executor.
use super::persistence::FILE_LIMIT;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(1);

pub fn default_path() -> Option<PathBuf> {
    crate::preferences::default_path()
        .and_then(|p| p.parent().map(|p| p.join(super::persistence::FILE_NAME)))
}

/// None is first use. Invalid bytes remain for the total decoder to classify.
pub fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot read saved tabs: {error}")),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err("Saved tabs must be an owned regular file.".into());
    }
    if metadata.len() > FILE_LIMIT as u64 {
        return Ok(Some(vec![0; FILE_LIMIT + 1]));
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(Some(bytes))
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > FILE_LIMIT {
        return Err("Tab preferences exceed their limit; the last saved tabs are intact.".into());
    }
    let parent = path.parent().ok_or("Tab preferences have no directory.")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let metadata = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Tab preferences need an owned directory.".into());
    }
    let temporary = parent.join(format!(
        ".tabs-{}-{}",
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
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("Cannot save tabs: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn isolated(name: &str) -> bool {
        if std::env::var("PLAIT_TAB_STORAGE_TEST").ok().as_deref() == Some(name) {
            return true;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("PLAIT_TAB_STORAGE_TEST", name)
            .status()
            .unwrap();
        assert!(status.success(), "isolated tab storage test failed");
        false
    }
    #[test]
    fn atomic_private_tabs_preserve_last_good_data_and_reject_symlinks() {
        if !isolated(
            "tabs::storage::tests::atomic_private_tabs_preserve_last_good_data_and_reject_symlinks",
        ) {
            return;
        }
        let directory =
            std::env::temp_dir().join(format!("plait-tabs-storage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("tabs.json");
        assert_eq!(read(&path).unwrap(), None);
        write(&path, b"good").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write(&path, &vec![0; FILE_LIMIT + 1]).is_err());
        assert_eq!(read(&path).unwrap().unwrap(), b"good");
        let link = directory.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read(&link).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
