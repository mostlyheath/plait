//! SSH process transport for a remote host bridge.
use super::link::{Link, Settings};
use crate::host::Error;
use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct SshDestination(String);
impl SshDestination {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 253
            || value.starts_with('-')
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_@:[]".contains(&b))
        {
            return Err(Error::Transport("Invalid SSH destination.".into()));
        }
        Ok(Self(value))
    }
}

impl<'de> serde::Deserialize<'de> for SshDestination {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug)]
pub struct SshOptions {
    pub program: OsString,
    pub store: Option<PathBuf>,
    pub port: Option<u16>,
}
impl Default for SshOptions {
    fn default() -> Self {
        Self {
            program: "ssh".into(),
            store: None,
            port: None,
        }
    }
}

fn arguments(destination: &SshDestination, options: &SshOptions) -> Result<Vec<OsString>, Error> {
    let mut args: Vec<OsString> = [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if let Some(port) = options.port {
        args.extend([OsString::from("-p"), port.to_string().into()]);
    }
    args.extend([
        OsString::from("--"),
        destination.0.clone().into(),
        "plait-host".into(),
        "bridge".into(),
    ]);
    if let Some(path) = &options.store {
        use std::os::unix::ffi::OsStrExt;
        if !path.is_absolute()
            || path
                .as_os_str()
                .as_bytes()
                .iter()
                .any(|b| b.is_ascii_control())
        {
            return Err(Error::Transport("Invalid SSH store path.".into()));
        }
        // OpenSSH joins remote command arguments into a shell command. Quote the
        // path so spaces and shell metacharacters remain literal on the host.
        let bytes = path.as_os_str().as_bytes();
        let mut quoted = Vec::from(&b"'"[..]);
        for &b in bytes {
            if b == b'\'' {
                quoted.extend_from_slice(b"'\\''");
            } else {
                quoted.push(b);
            }
        }
        quoted.push(b'\'');
        use std::os::unix::ffi::OsStringExt;
        args.extend(["--store".into(), OsString::from_vec(quoted)]);
    }
    Ok(args)
}

pub fn connect(destination: &SshDestination, options: SshOptions) -> Result<Arc<Link>, Error> {
    let args = arguments(destination, &options)?;
    let mut child = Command::new(&options.program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Transport(format!("Could not start SSH: {e}")))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let captured = Arc::new(Mutex::new(VecDeque::new()));
    let buffer = Arc::clone(&captured);
    let stderr_thread = thread::spawn(move || {
        let mut bytes = [0; 1024];
        while let Ok(n) = stderr.read(&mut bytes) {
            if n == 0 {
                break;
            }
            let mut buffer = buffer.lock().expect("stderr lock poisoned");
            for &byte in &bytes[..n] {
                if buffer.len() == 4096 {
                    buffer.pop_front();
                }
                buffer.push_back(byte);
            }
        }
    });
    let child = Arc::new(Mutex::new(Some(child)));
    let link = Arc::new(Link::new(stdout, stdin, Settings::default()));
    let cleanup = Arc::clone(&child);
    link.on_dead(move || {
        if let Some(mut child) = cleanup.lock().expect("child lock poisoned").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    });
    let result = (|| -> std::io::Result<()> {
        let mut stream = link.open()?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(15)))?;
        stream.write_all(b"{\"op\":\"status\"}\n")?;
        let mut line = Vec::new();
        let count = BufReader::new(stream)
            .take(1024 * 1024 + 1)
            .read_until(b'\n', &mut line)?;
        if count == 0 || line.last() != Some(&b'\n') {
            return Err(std::io::Error::other("no startup reply"));
        }
        Ok(())
    })();
    if result.is_err() {
        drop(link);
        let _ = stderr_thread.join();
        let bytes: Vec<_> = captured
            .lock()
            .expect("stderr lock poisoned")
            .iter()
            .copied()
            .collect();
        let message = String::from_utf8_lossy(&bytes);
        let line: String = message
            .lines()
            .last()
            .unwrap_or("")
            .trim()
            .chars()
            .take(200)
            .collect();
        return Err(Error::Transport(if line.is_empty() {
            "SSH connection failed.".into()
        } else {
            format!("SSH connection failed: {line}")
        }));
    }
    Ok(link)
}

#[derive(Default, Debug)]
pub struct Reconnect {
    failures: u32,
}
impl Reconnect {
    pub fn next_delay(&mut self) -> Duration {
        let seconds = if self.failures >= 5 {
            30
        } else {
            1 << self.failures
        };
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(seconds)
    }
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_destination_and_arguments() {
        for bad in [
            "",
            "-oProxyCommand=x",
            "a b",
            "a\n",
            "a;echo",
            &"a".repeat(254),
        ] {
            assert!(SshDestination::new(bad).is_err());
        }
        for good in ["host", "user@host", "alias_1", "[::1]"] {
            assert!(SshDestination::new(good).is_ok());
        }
        let args = arguments(
            &SshDestination::new("host").unwrap(),
            &SshOptions::default(),
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "--",
                "host",
                "plait-host",
                "bridge"
            ]
            .map(OsString::from)
        );
        assert!(
            arguments(
                &SshDestination::new("host").unwrap(),
                &SshOptions {
                    store: Some("relative".into()),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn remote_reconnect_backoff() {
        let mut retry = Reconnect::default();
        assert_eq!(
            (0..8)
                .map(|_| retry.next_delay().as_secs())
                .collect::<Vec<_>>(),
            [1, 2, 4, 8, 16, 30, 30, 30]
        );
        retry.reset();
        assert_eq!(retry.next_delay().as_secs(), 1);
    }
}
