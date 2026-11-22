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
