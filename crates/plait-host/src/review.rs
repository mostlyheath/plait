//! Read helpers share the recorder's lifetime supervisor, but never its index.
use plait_engine::git::{record, review};
use serde_json::Value;
use std::{
    io::{self, Read},
    time::Duration,
};
// Only the sandboxed path on macOS starts a contained copy of this program.
#[cfg(target_os = "macos")]
use std::{io::Write, process::Stdio};

pub fn run() -> io::Result<()> {
    super::record::supervise("review-worker", Duration::from_secs(10))
}
pub fn worker(value: Value) -> io::Result<()> {
    let request: review::Request = serde_json::from_value(value.clone())?;
    request.validate().map_err(io::Error::other)?;
    let store = request.store.canonicalize()?;
    let records = store.join("records");
    record::sandbox_profile(&[&request.checkout], &records).map_err(io::Error::other)?;
    #[cfg(target_os = "macos")]
    {
        // The only possible write is the empty blob for an absent comparison side.
        // The recorder's index, refs, locks, and every checkout remain read-only.
        let objects = records.join(&request.object_format).join("objects");
        let quoted = objects
            .to_str()
            .filter(|p| !p.chars().any(char::is_control))
            .ok_or_else(|| io::Error::other("Invalid store path."))?
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let profile = format!(
            "(version 1)(allow default)(deny network*)(deny file-write*)(allow file-write* (literal \"/dev/null\") (subpath \"{quoted}\"))"
        );
        let mut child = crate::command::with_profile(&profile)
            .arg(std::env::current_exe()?)
            .arg("review-contained")
            .env("PLAIT_GIT", record::git_binary())
            .stdin(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Missing helper input."))?
            .write_all(&serde_json::to_vec(&value)?)?;
        if !child.wait()?.success() {
            return Err(io::Error::other("Recorded content is unavailable."));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    output(request)
}
fn output(request: review::Request) -> io::Result<()> {
    let result = review::read(&request).map_err(io::Error::other)?;
    serde_json::to_writer(io::stdout().lock(), &result)?;
    Ok(())
}
pub fn contained() -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().take(65_001).read_to_end(&mut input)?;
    if input.len() > 65_000 {
        return Err(io::Error::other("Review request is too large."));
    }
    output(serde_json::from_slice(&input)?)
}
