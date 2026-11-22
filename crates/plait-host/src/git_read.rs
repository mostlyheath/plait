//! One Git or file read for a remote desktop: JSON on stdin, the outcome on
//! stdout. Git's own deadlines bound each command; the runtime bounds the whole.
use plait_core::host::git::{HelperInput, execute};
use std::io::{self, Read, Write};

/// Requests are small; the runtime already refused anything over its limit.
const REQUEST_LIMIT: u64 = 128 * 1024;

pub fn run() -> io::Result<()> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(REQUEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > REQUEST_LIMIT {
        return Err(io::Error::other("Git read request too large."));
    }
    let input: HelperInput = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let mut out = io::stdout().lock();
    out.write_all(&execute(&input))?;
    out.flush()
}
