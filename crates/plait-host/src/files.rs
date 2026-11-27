use crate::fingerprint::{postimage, stamp};
use serde_json::{Value, json};
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};

fn open_at(parent: &File, name: &str, flags: i32) -> io::Result<File> {
    let name = CString::new(name).map_err(io::Error::other)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn components(path: &str) -> io::Result<Vec<&str>> {
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == ".." || part.eq_ignore_ascii_case(".git"))
    {
        return Err(io::Error::other("unsafe relative path"));
    }
    Path::new(path)
        .components()
        .map(|part| match part {
            Component::Normal(name) => name
                .to_str()
                .ok_or_else(|| io::Error::other("non-UTF8 path")),
            _ => Err(io::Error::other("unsafe path component")),
        })
        .collect()
}

fn check_protected(file: &File, protected: &Value) -> io::Result<()> {
    let meta = file.metadata()?;
    if protected.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["dev"].as_str() == Some(meta.dev().to_string().as_str())
                && item["ino"].as_str() == Some(meta.ino().to_string().as_str())
        })
    }) {
        return Err(io::Error::other(
            "pre-existing dirty file identity is protected",
        ));
    }
    Ok(())
}

fn parent(
    root: &str,
    path: &str,
    protected: &Value,
    identity: &[String],
) -> io::Result<(File, String)> {
    if !root.starts_with('/') {
        return Err(io::Error::other("checkout must be absolute"));
    }
    let mut directory = File::open("/")?;
    for part in Path::new(root).components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = open_at(
                    &directory,
                    name.to_str()
                        .ok_or_else(|| io::Error::other("invalid checkout"))?,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?
            }
            _ => return Err(io::Error::other("invalid checkout")),
        }
    }
    let meta = directory.metadata()?;
    if identity.len() == 2
        && (meta.dev().to_string() != identity[0] || meta.ino().to_string() != identity[1])
    {
        return Err(io::Error::other("checkout root identity changed"));
    }
    let parts = components(path)?;
    let (name, parents) = parts
        .split_last()
        .ok_or_else(|| io::Error::other("empty path"))?;
    check_protected(&directory, protected)?;
    for part in parents {
        directory = open_at(&directory, part, libc::O_RDONLY | libc::O_DIRECTORY)?;
        check_protected(&directory, protected)?;
    }
    Ok((directory, (*name).to_owned()))
}

fn text(file: &mut File, limit: u64) -> io::Result<String> {
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > limit {
        return Err(io::Error::other(
            "expected a regular, single-link file within the size limit",
        ));
    }
    let mut value = String::new();
    (&mut *file).take(limit + 1).read_to_string(&mut value)?;
    if value.len() as u64 > limit {
        return Err(io::Error::other("file too large"));
    }
    if stamp(&meta) != stamp(&file.metadata()?) {
        return Err(io::Error::other("file changed while reading"));
    }
    Ok(value)
}

// Range reads never allocate the whole file and keep the same descriptor/path checks.
fn read_range(file: &mut File, request: &Value) -> io::Result<Value> {
    let offset = match request.get("offset") {
        None => 0,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| io::Error::other("invalid read offset"))?,
    };
    let limit = match request.get("limit") {
        None => 8192,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| io::Error::other("invalid read limit"))?,
    };
    if !(4..=12000).contains(&limit) {
        return Err(io::Error::other("invalid read limit"));
    }
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(io::Error::other("invalid file range"));
    }
    if offset > meta.len() {
        return Err(io::Error::other("read offset beyond EOF"));
    }
    // A continuation byte alone could be corrupt data, not a split character.
    let start = offset.saturating_sub(3);
    file.seek(SeekFrom::Start(start))?;
    let mut context = Vec::new();
    (&mut *file)
        .take(offset - start + 4)
        .read_to_end(&mut context)?;
    let boundary = (offset - start) as usize;
    for index in 0..boundary {
        // Only the crossing character needs to be valid, not the following bytes.
        for end in boundary + 1..=context.len() {
            if let Ok(value) = std::str::from_utf8(&context[index..end])
                && value.chars().count() == 1
            {
                return Err(io::Error::other("read offset inside UTF-8 character"));
            }
        }
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    (&mut *file).take(limit).read_to_end(&mut bytes)?;
    let end = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() && offset + (bytes.len() as u64) < meta.len() => {
            error.valid_up_to()
        }
        Err(_) => {
            return Err(io::Error::other("file contents are not valid UTF-8"));
        }
    };
    let next = offset + end as u64;
    let value = std::str::from_utf8(&bytes[..end]).map_err(io::Error::other)?;
    if stamp(&meta) != stamp(&file.metadata()?) {
        return Err(io::Error::other("file changed while reading"));
    }
    Ok(
        json!({"text":value,"offset":offset,"next_offset":(next < meta.len()).then_some(next),"total_bytes":meta.len()}),
    )
}

fn replace(
    directory: &File,
    name: &str,
    file: &File,
    before: &str,
    after: &str,
    protected: &Value,
) -> io::Result<File> {
    let meta = file.metadata()?;
    let temporary = format!(
        ".plait-edit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos()
    );
    let temp_name = CString::new(temporary.as_str()).map_err(io::Error::other)?;
    let target_name = CString::new(name).map_err(io::Error::other)?;
    let result = (|| {
        let mut replacement = open_at(
            directory,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        replacement.write_all(after.as_bytes())?;
        if unsafe {
            libc::fchmod(
                replacement.as_raw_fd(),
                (meta.mode() & 0o777) as libc::mode_t,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        replacement.sync_all()?;
        let mut current = open_at(directory, name, libc::O_RDONLY)?;
        check_protected(&current, protected)?;
        let now = current.metadata()?;
        if now.dev() != meta.dev()
            || now.ino() != meta.ino()
            || text(&mut current, 8 * 1024 * 1024)? != before
        {
            return Err(io::Error::other("file changed during approval or edit"));
        }
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                temp_name.as_ptr(),
                directory.as_raw_fd(),
                target_name.as_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        directory.sync_all()?;
        Ok(replacement)
    })();
    if result.is_err() {
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temp_name.as_ptr(), 0);
        }
    }
    result
}

pub fn run(args: Vec<String>) -> io::Result<()> {
    if args.len() != 3 && args.len() != 5 {
        return Err(io::Error::other("file expects action checkout path"));
    }
    let request: Value = if matches!(args[0].as_str(), "edit" | "write" | "read_range") {
        serde_json::from_reader(io::stdin().take(393217))?
    } else {
        Value::Null
    };
    let (directory, name) = parent(&args[1], &args[2], &request["protected"], &args[3..])?;
    let result = match args[0].as_str() {
        "read_range" => {
            let mut file = open_at(&directory, &name, libc::O_RDONLY)?;
            read_range(&mut file, &request)?
        }
        "read" | "inspect" => {
            let mut file = open_at(&directory, &name, libc::O_RDONLY)?;
            let before = stamp(&file.metadata()?);
            let value = text(
                &mut file,
                if args[0] == "inspect" {
                    8 * 1024 * 1024
                } else {
                    262144
                },
            )?;
            if before != stamp(&file.metadata()?) {
                return Err(io::Error::other("file changed while reading"));
            }
            json!({"text":if args[0] == "inspect" { "" } else { &value },"postimage":postimage(&file, &args[1], value.as_bytes())?})
        }
        "edit" | "write" => {
            let value = request["content"]
                .as_str()
                .ok_or_else(|| io::Error::other("content missing"))?;
            if value.len() > 16384 {
                return Err(io::Error::other("edit too large"));
            }
            let (file, bytes) = if args[0] == "write" {
                let mut file = open_at(
                    &directory,
                    &name,
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                )?;
                file.write_all(value.as_bytes())?;
                file.sync_all()?;
                (file, value.to_owned())
            } else {
                let expected = request["expected"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("expected text missing"))?;
                // Opening a provenance-marked original for write can itself change ctime.
                let mut file = open_at(&directory, &name, libc::O_RDONLY)?;
                check_protected(&file, &request["protected"])?;
                let before = text(&mut file, 8 * 1024 * 1024)?;
                if expected.is_empty() || before.matches(expected).count() != 1 {
                    return Err(io::Error::other("edit must match exactly once"));
                }
                if let Some(expected_file) = request["expected_file"].as_str()
                    && expected_file != before
                {
                    return Err(io::Error::other(
                        "file changed after approval was requested",
                    ));
                }
                if !request["expected_image"].is_null()
                    && request["expected_image"] != postimage(&file, &args[1], before.as_bytes())?
                {
                    return Err(io::Error::other(
                        "file identity or metadata changed after approval",
                    ));
                }
                let after = before.replacen(expected, value, 1);
                if after.len() > 8 * 1024 * 1024 {
                    return Err(io::Error::other("result exceeds file bound"));
                }
                let replacement = replace(
                    &directory,
                    &name,
                    &file,
                    &before,
                    &after,
                    &request["protected"],
                )?;
                (replacement, after)
            };
            directory.sync_all()?;
            json!({"text":"File updated.","postimage":postimage(&file, &args[1], bytes.as_bytes())?})
        }
        _ => return Err(io::Error::other("unknown file action")),
    };
    serde_json::to_writer(io::stdout(), &result)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn range(bytes: &[u8], request: Value) -> io::Result<Value> {
        // Parallel tests can observe the same clock tick on CI runners.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = plait_test_support::temp_dir().join(format!(
            "plait-range-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        let result = read_range(&mut file, &request);
        std::fs::remove_file(path)?;
        result
    }

    #[test]
    fn range_arguments_are_strict() {
        for value in [
            json!(null),
            json!(-1),
            json!(1.5),
            json!("4"),
            json!(true),
            json!([]),
            json!({}),
        ] {
            for field in ["offset", "limit"] {
                let error = range(b"hello", json!({field: value})).unwrap_err();
                assert_eq!(error.to_string(), format!("invalid read {field}"));
            }
        }
        for limit in [0, 3, 12001, u64::MAX] {
            assert_eq!(
                range(b"hello", json!({"limit": limit}))
                    .unwrap_err()
                    .to_string(),
                "invalid read limit"
            );
        }
        assert_eq!(range(b"hello", json!({})).unwrap()["text"], "hello");
    }

    #[test]
    fn range_offsets_and_unicode_boundaries() {
        let bytes = "abc🚀z".as_bytes();
        let page = range(bytes, json!({"offset": 0, "limit": 4})).unwrap();
        assert_eq!(
            page,
            json!({"text":"abc", "offset":0, "next_offset":3, "total_bytes":8})
        );
        assert_eq!(
            range(bytes, json!({"offset":3, "limit":4})).unwrap()["text"],
            "🚀"
        );
        for offset in 4..7 {
            assert_eq!(
                range(bytes, json!({"offset":offset}))
                    .unwrap_err()
                    .to_string(),
                "read offset inside UTF-8 character"
            );
        }
        for bytes in [b"".as_slice(), bytes] {
            assert_eq!(
                range(bytes, json!({"offset":bytes.len()})).unwrap()["text"],
                ""
            );
            assert_eq!(
                range(bytes, json!({"offset":bytes.len() + 1}))
                    .unwrap_err()
                    .to_string(),
                "read offset beyond EOF"
            );
        }
        for bytes in [b"\xff".as_slice(), b"\x80abc", b"abc\xf0\x9f"] {
            assert_eq!(
                range(bytes, json!({})).unwrap_err().to_string(),
                "file contents are not valid UTF-8"
            );
        }
    }

    #[test]
    fn rejects_escapes_and_git_metadata() {
        for path in [
            "../secret",
            "/etc/passwd",
            "a/../../b",
            "a/.git/config",
            ".GIT/config",
            ".Git/config",
            "a//b",
            "",
        ] {
            assert!(components(path).is_err(), "{path}");
        }
        assert_eq!(components("src/main.rs").unwrap(), ["src", "main.rs"]);
    }
}
