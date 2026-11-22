//! One Git or file read, or one Git write, for a remote desktop: JSON on
//! stdin, the outcome on stdout. Git's own deadlines bound each command; the
//! runtime bounds the whole.
use plait_core::host::git::{HelperInput, WriteInput, execute, execute_write};
use std::io::{self, Read, Write};

/// Requests are small; the runtime already refused anything over its limit.
const REQUEST_LIMIT: u64 = 128 * 1024;

pub fn run() -> io::Result<()> {
    let input: HelperInput = serde_json::from_slice(&request()?).map_err(io::Error::other)?;
    reply(&execute(&input))
}

pub fn write() -> io::Result<()> {
    let input: WriteInput = serde_json::from_slice(&request()?).map_err(io::Error::other)?;
    reply(&execute_write(&input))
}

fn request() -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(REQUEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > REQUEST_LIMIT {
        return Err(io::Error::other("Git request too large."));
    }
    Ok(bytes)
}

fn reply(bytes: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()
}
