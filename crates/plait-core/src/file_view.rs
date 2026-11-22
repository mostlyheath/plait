//! Reading one checkout file for the read-only viewer, finding text in it, and
//! telling whether it changed on disk. Blocking: call on a background thread.
//!
//! The bounded read and its grew-while-reading guard follow Sidelit's
//! (same maintainer, MIT); opening uses the checkout-relative, no-symlink
//! `open_beneath` shared with Git previews.

use crate::{git::open_beneath, highlight};
use std::{
    hash::{Hash, Hasher},
    io::Read,
    ops::Range,
    os::unix::fs::MetadataExt,
    path::Path,
    time::SystemTime,
};

/// Larger files are described, not drawn.
pub const READ_LIMIT: u64 = 8 * 1024 * 1024;
/// A NUL byte in this much of the start marks a binary file, as Git decides.
const SNIFF: usize = 8000;
/// Find stops counting here, so a one-letter query stays instant.
pub const FIND_LIMIT: usize = 1000;
/// Lines longer than this are clipped when drawn; the text itself is kept.
pub const LINE_CLIP: usize = 2000;

/// Size and modification time: enough to skip rereading an unchanged file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    pub len: u64,
    pub device: u64,
    pub inode: u64,
    pub modified: Option<SystemTime>,
}

/// What the viewer can show for a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Contents {
    Text(Text),
    Binary {
        len: u64,
    },
    TooLarge {
        len: u64,
    },
    /// Not valid UTF-8; shown as a note rather than guessed at.
    NotUtf8 {
        len: u64,
    },
}

/// A text file, split into lines and highlighted once, off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub text: String,
    /// Byte range of each line, without its line ending.
    pub lines: Vec<Range<usize>>,
    /// Highlight spans, sorted; empty when the language is unknown or the file
    /// is past the highlighter's limit.
    pub spans: Vec<highlight::Span>,
    /// Characters in the longest line, for horizontal scrolling.
    pub widest: usize,
    /// First line with the widest clipped width.
    pub widest_index: usize,
    pub hash: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub contents: Contents,
    pub stamp: Stamp,
}

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

/// Read `relative` under `root` for display.
pub fn read(root: &Path, relative: &Path) -> Result<Opened, String> {
    let (stamp, bytes) = read_bytes(root, relative)?;
    Ok(classify(stamp, bytes, relative))
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

/// What the viewer shows for bytes read by `read_bytes`.
pub fn classify(stamp: Stamp, bytes: Option<Vec<u8>>, relative: &Path) -> Opened {
    let Some(bytes) = bytes else {
        return Opened {
            contents: Contents::TooLarge { len: stamp.len },
            stamp,
        };
    };
    let len = bytes.len() as u64;
    let contents = if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        Contents::Binary { len }
    } else {
        match String::from_utf8(bytes) {
            Ok(text) => Contents::Text(prepare(text, &relative.to_string_lossy())),
            Err(_) => Contents::NotUtf8 { len },
        }
    };
    Opened { contents, stamp }
}

fn open_regular(root: &Path, relative: &Path) -> Result<std::fs::File, String> {
    // Non-blocking, so a FIFO swapped in cannot hang the read.
    let file = open_beneath(root, relative, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Not a regular file.".into());
    }
    Ok(file)
}

/// Split into lines and highlight by the path's language.
pub fn prepare(text: String, path: &str) -> Text {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            let end = if index > start && text.as_bytes()[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            lines.push(start..end);
            start = index + 1;
        }
    }
    // A final line without a newline; an empty file still has one line.
    if start < text.len() || lines.is_empty() {
        lines.push(start..text.len());
    }
    let (widest_index, widest) = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (index, text[line.clone()].chars().count().min(LINE_CLIP)))
        .fold(
            (0, 0),
            |best, item| if item.1 > best.1 { item } else { best },
        );
    let spans = highlight::highlight(highlight::language_for_path(path), &text);
    Text {
        hash: hash(&text),
        text,
        lines,
        spans,
        widest,
        widest_index,
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Byte ranges of `query` in `text`, at most `FIND_LIMIT`, ignoring letter
/// case as find in the conversation does, outside ASCII too.
pub fn find(text: &str, query: &str) -> Vec<Range<usize>> {
    crate::conversation_find::find(text, query, FIND_LIMIT)
}

/// The line holding byte `offset`.
pub fn line_of(lines: &[Range<usize>], offset: usize) -> usize {
    lines
        .partition_point(|line| line.end < offset)
        .min(lines.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("plait-file-view-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn text_is_split_into_lines_and_highlighted() {
        let text = prepare("fn main() {\r\n    1\n}".into(), "src/main.rs");
        assert_eq!(text.lines, vec![0..11, 13..18, 19..20]);
        assert_eq!(&text.text[text.lines[1].clone()], "    1");
        assert!(
            text.spans
                .iter()
                .any(|(_, token)| *token == highlight::Token::Keyword)
        );
        assert_eq!(text.widest, 11);
        assert_eq!(text.widest_index, 0);
        assert_eq!(prepare("a\nlonger\nx".into(), "a").widest_index, 1);
        assert_eq!(prepare("longer\na".into(), "a").widest_index, 0);
        assert_eq!(prepare(String::new(), "a").lines, vec![0..0]);
        assert_eq!(prepare("a\n".into(), "a").lines, vec![0..1]);
    }

    #[test]
    fn reads_text_and_describes_what_it_cannot_show() {
        let dir = scratch("read");
        std::fs::write(dir.join("a.md"), "# Title\n").unwrap();
        std::fs::write(dir.join("image.png"), [0x89, b'P', 0, 1]).unwrap();
        std::fs::write(dir.join("latin1.txt"), [b'c', 0xe9]).unwrap();
        let read = read(&dir, Path::new("a.md")).unwrap();
        let Contents::Text(text) = read.contents else {
            panic!("text")
        };
        assert_eq!(text.text, "# Title\n");
        assert_eq!(read.stamp.len, 8);
        assert_eq!(
            super::read(&dir, Path::new("image.png")).unwrap().contents,
            Contents::Binary { len: 4 }
        );
        assert_eq!(
            super::read(&dir, Path::new("latin1.txt")).unwrap().contents,
            Contents::NotUtf8 { len: 2 }
        );
        let big = std::fs::File::create(dir.join("big.log")).unwrap();
        big.set_len(READ_LIMIT + 1).unwrap();
        assert_eq!(
            super::read(&dir, Path::new("big.log")).unwrap().contents,
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

    #[test]
    fn find_ignores_case_and_stops_at_the_limit() {
        assert_eq!(find("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
        assert_eq!(
            find("aaaa", "aa"),
            vec![0..2, 2..4],
            "matches do not overlap"
        );
        assert_eq!(
            find("Ébé ébé ÉBÉ", "ébé"),
            vec![0..5, 6..11, 12..17],
            "case is ignored outside ASCII too"
        );
        assert_eq!(find("let café = CAFÉ;", "Café"), vec![4..9, 12..17]);
        assert!(find("abc", "").is_empty());
        assert_eq!(find(&"x".repeat(5000), "x").len(), FIND_LIMIT);
    }

    #[test]
    fn offsets_map_to_lines() {
        let lines = vec![0..3, 4..4, 5..9];
        assert_eq!(line_of(&lines, 0), 0);
        assert_eq!(line_of(&lines, 3), 0);
        assert_eq!(line_of(&lines, 4), 1);
        assert_eq!(line_of(&lines, 6), 2);
        assert_eq!(line_of(&lines, 99), 2);
    }
}
