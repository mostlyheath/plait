//! Additive turn-review reads. No protocol version is sent or negotiated.
use super::{Client, Error};
use std::time::Duration;

pub const OLDER_HOST: &str = "Turn review needs a newer host.";
pub use plait_protocol::host::review::*;

#[derive(Debug)]
pub struct Prepared {
    pub reply: DiffReply,
    pub diff: crate::model::Diff,
}
fn invalid() -> Error {
    Error::Protocol("The host returned inconsistent turn-review data. Refresh to try again.".into())
}
pub fn needs_host_update(error: &Error) -> bool {
    matches!(error, Error::Rejected(error) if error.code == "invalid_request")
}
pub fn info(client: &Client, thread: &str) -> Result<Info, Error> {
    let reply: Info = client.send(&Request::TurnReviewInfo {
        thread_id: thread.into(),
    })?;
    if reply.schema != 1 {
        return Err(Error::Protocol(
            "This turn-review schema is unavailable.".into(),
        ));
    }
    Ok(reply)
}
fn document<T: serde::de::DeserializeOwned + Clone>(
    client: &Client,
    first: &Page<T>,
) -> Result<Vec<T>, Error> {
    if first.offset != 0 || first.total > 100_000 || first.token.is_empty() {
        return Err(invalid());
    }
    let mut page = first.clone();
    let mut items = Vec::new();
    loop {
        if page.token != first.token
            || page.total != first.total
            || page.offset != items.len()
            || page.items.len() > 500
            || (page.items.is_empty() && page.next_offset.is_some())
        {
            return Err(invalid());
        }
        items.extend(page.items);
        if items.len() > first.total {
            return Err(invalid());
        }
        match page.next_offset {
            None if items.len() == first.total => return Ok(items),
            Some(next) if next == items.len() && next < first.total => {
                page = client.send(&Request::TurnReviewPage {
                    token: first.token.clone(),
                    offset: next,
                })?
            }
            _ => return Err(invalid()),
        }
    }
}
/// Complete and prepare off the UI thread; never install a partial document.
pub fn read(
    client: &Client,
    thread: &str,
    record: &str,
    path: &str,
    checkout: &Checkout,
) -> Result<Prepared, Error> {
    let reply: DiffReply = client.send(&Request::TurnDiff {
        thread_id: thread.into(),
        record_id: record.into(),
        path: path.into(),
    })?;
    if reply.schema != 1
        || reply.checkout != *checkout
        || reply.path != path
        || reply.source_record_id.as_deref() != Some(record)
        || reply.basis != Basis::Record
        || reply.side.is_some()
        || reply.patch_bytes > 512 * 1024
    {
        return Err(invalid());
    }
    let chunks = document(client, &reply.patch)?;
    let hunks = document(client, &reply.hunks)?;
    let mut bytes = Vec::with_capacity(reply.patch_bytes);
    for chunk in chunks {
        if chunk.byte_offset != bytes.len() {
            return Err(invalid());
        }
        let decoded = base64(&chunk.bytes_base64)?;
        if decoded.is_empty()
            || decoded.len() > 65_536
            || bytes.len() + decoded.len() > reply.patch_bytes
        {
            return Err(invalid());
        }
        bytes.extend(decoded);
    }
    if bytes.len() != reply.patch_bytes {
        return Err(invalid());
    }
    let lines = bytes.split_inclusive(|b| *b == b'\n').count();
    let mut previous = None;
    for hunk in hunks {
        if hunk.first_line > hunk.last_line
            || hunk.last_line >= lines
            || previous.is_some_and(|end| hunk.first_line <= end)
        {
            return Err(invalid());
        }
        previous = Some(hunk.last_line);
    }
    let diff = crate::git::recorded_diff(
        &bytes,
        reply.truncated,
        Duration::from_millis(reply.elapsed_ms),
    );
    Ok(Prepared { reply, diff })
}
fn base64(text: &str) -> Result<Vec<u8>, Error> {
    if !text.len().is_multiple_of(4) || text.len() > 87_384 {
        return Err(invalid());
    }
    let value = |b: u8| -> Result<u8, Error> {
        match b {
            b'A'..=b'Z' => Ok(b - b'A'),
            b'a'..=b'z' => Ok(b - b'a' + 26),
            b'0'..=b'9' => Ok(b - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(invalid()),
        }
    };
    let mut out = Vec::new();
    for (i, part) in text.as_bytes().as_chunks::<4>().0.iter().enumerate() {
        let a = value(part[0])?;
        let b = value(part[1])?;
        let end = (i + 1) * 4 == text.len();
        out.push(a << 2 | b >> 4);
        if part[2] == b'=' {
            if !end || part[3] != b'=' || b & 15 != 0 {
                return Err(invalid());
            }
            continue;
        }
        let c = value(part[2])?;
        out.push(b << 4 | c >> 2);
        if part[3] == b'=' {
            if !end || c & 3 != 0 {
                return Err(invalid());
            }
            continue;
        }
        out.push(c << 6 | value(part[3])?);
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_rejects_unknown_schema_and_identifies_older_host() {
        use std::{
            io::{BufRead, BufReader, Write},
            os::unix::{fs::PermissionsExt, net::UnixListener},
        };
        for (name, reply, old) in [
            (
                "old",
                r#"{"ok":false,"error":{"code":"invalid_request","message":"Unknown operation or field."}}"#,
                true,
            ),
            (
                "future",
                r#"{"ok":true,"result":{"schema":2,"checkout":{"host_id":"h","id":"c","catalog_revision":0,"review_revision":0},"features":{"record_diff":true,"attribution":false,"stage_turn":false,"since_review":false},"retention_days":30}}"#,
                false,
            ),
        ] {
            let directory =
                std::env::temp_dir().join(format!("plait-review-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            let path = directory.join("host.sock");
            let listener = UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut input = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut input)
                    .unwrap();
                assert_eq!(input, "{\"op\":\"turn_review_info\",\"thread_id\":\"t\"}\n");
                writeln!(socket, "{reply}").unwrap();
            });
            let error = info(
                &Client {
                    transport: super::super::Transport::Local(path),
                },
                "t",
            )
            .unwrap_err();
            assert_eq!(needs_host_update(&error), old);
            server.join().unwrap();
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn additive_request_has_no_version() {
        assert_eq!(
            serde_json::to_string(&Request::TurnReviewInfo {
                thread_id: "t".into()
            })
            .unwrap(),
            r#"{"op":"turn_review_info","thread_id":"t"}"#
        );
        let reply: Info = serde_json::from_str(r#"{"schema":1,"checkout":{"host_id":"h","id":"c","catalog_revision":0,"review_revision":0,"new":true},"features":{"record_diff":true,"attribution":false,"stage_turn":false,"since_review":false,"new":true},"retention_days":30,"new":true}"#).unwrap();
        assert!(reply.features.record_diff);
        assert!(serde_json::from_str::<Status>("\"new\"").is_err());
        assert!(serde_json::from_str::<Mode>("\"160000\"").is_err());
    }
    #[test]
    fn base64_preserves_exact_bytes_and_refuses_bad_padding() {
        assert_eq!(base64("YWJjDQpm/w==").unwrap(), b"abc\r\nf\xff");
        for text in ["A", "====", "AA=A", "AB==", "AA==AA==", "AAA-"] {
            assert!(base64(text).is_err(), "{text}");
        }
    }
}
