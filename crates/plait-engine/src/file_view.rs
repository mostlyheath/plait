use crate::git::open_beneath;
use plait_protocol::file_view::{READ_LIMIT, Stamp};
use std::{io::Read, os::unix::fs::MetadataExt, path::Path};

/// Size and time of `relative` under `root`, without following symlinks.
pub fn stamp(root: &Path, relative: &Path) -> Result<Stamp, String> {
    let file = open_regular(root, relative)?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    Ok(Stamp {
        len: metadata.len(),
        device: metadata.dev(),
        inode: metadata.ino(),
        modified: metadata.modified().ok(),
    })
}

/// The file's stamp and bytes, or no bytes when it is over `READ_LIMIT`.
/// A remote host sends these, and the desktop classifies and highlights them.
pub fn read_bytes(root: &Path, relative: &Path) -> Result<(Stamp, Option<Vec<u8>>), String> {
    let file = open_regular(root, relative)?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let stamp = Stamp {
        len: metadata.len(),
        device: metadata.dev(),
        inode: metadata.ino(),
        modified: metadata.modified().ok(),
    };
    if stamp.len > READ_LIMIT {
        return Ok((stamp, None));
    }
    let mut bytes = Vec::with_capacity(stamp.len as usize);
    file.take(READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    // It grew past the limit between the size check and the read.
    if bytes.len() as u64 > READ_LIMIT {
        return Ok((stamp, None));
    }
    if self::stamp(root, relative)? != stamp {
        return Err("File changed while it was being read. Open it again.".into());
    }
    Ok((stamp, Some(bytes)))
}

fn open_regular(root: &Path, relative: &Path) -> Result<std::fs::File, String> {
    // Non-blocking, so a FIFO swapped in cannot hang the read.
    let file = open_beneath(root, relative, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Not a regular file.".into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plait_core::file_view::{self, Contents, Opened};
    use std::path::PathBuf;
    fn read(root: &Path, relative: &Path) -> Result<Opened, String> {
        let (stamp, bytes) = read_bytes(root, relative)?;
        Ok(file_view::classify(stamp, bytes, relative))
    }
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("plait-file-view-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_text_and_describes_what_it_cannot_show() {
        let dir = scratch("read");
        std::fs::write(dir.join("a.md"), "# Title\n").unwrap();
        std::fs::write(dir.join("image.png"), [0x89, b'P', 0, 1]).unwrap();
        std::fs::write(dir.join("latin1.txt"), [b'c', 0xe9]).unwrap();
        let opened = read(&dir, Path::new("a.md")).unwrap();
        let Contents::Text(text) = opened.contents else {
            panic!("text")
        };
        assert_eq!(text.text, "# Title\n");
        assert_eq!(opened.stamp.len, 8);
        assert_eq!(
            read(&dir, Path::new("image.png")).unwrap().contents,
            Contents::Binary { len: 4 }
        );
        assert_eq!(
            read(&dir, Path::new("latin1.txt")).unwrap().contents,
            Contents::NotUtf8 { len: 2 }
        );
        let big = std::fs::File::create(dir.join("big.log")).unwrap();
        big.set_len(READ_LIMIT + 1).unwrap();
        assert_eq!(
            read(&dir, Path::new("big.log")).unwrap().contents,
            Contents::TooLarge {
                len: READ_LIMIT + 1
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_escapes_symlinks_and_special_files() {
        let dir = scratch("refuse");
        let outside = scratch("refuse-outside");
        std::fs::write(outside.join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), dir.join("link")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("folder")).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        for path in [
            "link",
            "folder/secret",
            "../refuse-outside/secret",
            "sub",
            "missing",
        ] {
            assert!(read(&dir, Path::new(path)).is_err(), "{path}");
        }
        let fifo = std::ffi::CString::new(dir.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(
            read(&dir, Path::new("pipe")).is_err(),
            "a FIFO neither blocks nor reads"
        );
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn a_stamp_changes_with_the_file() {
        let dir = scratch("stamp");
        std::fs::write(dir.join("a"), "one").unwrap();
        let first = stamp(&dir, Path::new("a")).unwrap();
        std::fs::write(dir.join("a"), "three").unwrap();
        assert_ne!(stamp(&dir, Path::new("a")).unwrap(), first);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
