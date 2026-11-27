//! Blocking local-host transport. Call these operations on the background executor.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{BufRead, BufReader, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::PathBuf,
    time::{Duration, Instant},
};

pub use plait_protocol::host::*;
const REPLY_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Error {
    Transport(String),
    Protocol(String),
    Rejected(HostError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(message) | Self::Protocol(message) => f.write_str(message),
            Self::Rejected(error) => write!(f, "{}: {}", error.code, error.message),
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Transport(format!(
            "Local host connection failed: {error}. Start the host, then Reconnect."
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    error: Option<HostError>,
}

fn decode<T: DeserializeOwned>(line: &[u8]) -> Result<T, Error> {
    let envelope: Envelope<T> = serde_json::from_slice(line).map_err(|_| {
        Error::Protocol("Invalid host reply. Update the host and desktop together.".into())
    })?;
    match (envelope.ok, envelope.result, envelope.error) {
        (true, Some(value), None) => Ok(value),
        (false, None, Some(error)) => Err(Error::Rejected(error)),
        _ => Err(Error::Protocol(
            "Inconsistent host reply; no action was confirmed.".into(),
        )),
    }
}

fn validate_created_checkout(line: &[u8], expected: &std::path::Path) -> Result<(), Error> {
    #[derive(Deserialize)]
    struct Created {
        thread: Checkout,
    }
    #[derive(Deserialize)]
    struct Checkout {
        checkout: PathBuf,
    }
    let reply: Created = decode(line)?;
    if reply.thread.checkout != expected {
        return Err(Error::Protocol(
            "Host returned another checkout's thread.".into(),
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct Client {
    transport: Transport,
}
#[derive(Clone)]
enum Transport {
    #[cfg(feature = "test-host")]
    TestReads(std::sync::Arc<dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync>),
    #[cfg(feature = "local-host")]
    Local(PathBuf),
    Link(std::sync::Arc<crate::remote::link::Link>),
    /// Routes repository reads (and writes when no scripted host socket is
    /// present) to `git`, while keeping all other host operations on `host`.
    #[cfg(feature = "test-host")]
    TestHost {
        host: Option<PathBuf>,
        git: PathBuf,
    },
    /// The local environment named no usable host; each request reports why.
    #[cfg(feature = "local-host")]
    Unavailable(String),
}
impl Client {
    #[cfg(feature = "local-host")]
    pub fn from_environment() -> Result<Self, Error> {
        let path = if let Some(path) = std::env::var_os("PLAIT_HOST_STORE") {
            PathBuf::from(path).join("host.sock")
        } else {
            PathBuf::from(std::env::var_os("HOME").ok_or_else(|| {
                Error::Transport("HOME is not set; cannot locate the local host.".into())
            })?)
            .join(".local/share/plait/host.sock")
        };
        if !path.is_absolute() {
            return Err(Error::Transport(
                "PLAIT_HOST_STORE must be an absolute path.".into(),
            ));
        }
        Ok(Self {
            transport: Transport::Local(path),
        })
    }

    /// This Mac's host, or a client whose every request returns the
    /// configuration error, so a bad environment is a connection error,
    /// not a failure to start.
    #[cfg(feature = "local-host")]
    pub fn local_or_unavailable() -> Self {
        Self::from_environment().unwrap_or_else(|error| Self {
            transport: Transport::Unavailable(error.to_string()),
        })
    }

    /// A client for a test host's socket.
    #[cfg(all(test, feature = "local-host"))]
    pub(crate) fn at_socket(path: PathBuf) -> Self {
        Self {
            transport: Transport::Local(path),
        }
    }

    /// An in-process request/reply host for scenes that exercise checkout reads.
    #[cfg(feature = "test-host")]
    pub fn with_test_reads(
        handler: impl Fn(serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
    ) -> Self {
        Self {
            transport: Transport::TestReads(std::sync::Arc::new(handler)),
        }
    }

    /// A client whose repository reads (and unscripted writes) go to `git`,
    /// while all other host operations use `host` (if present).
    #[cfg(feature = "test-host")]
    pub fn with_test_git_host(host: Option<PathBuf>, git: PathBuf) -> Self {
        Self {
            transport: Transport::TestHost { host, git },
        }
    }

    pub fn over_link(link: std::sync::Arc<crate::remote::link::Link>) -> Self {
        Self {
            transport: Transport::Link(link),
        }
    }

    fn socket(&self) -> std::io::Result<UnixStream> {
        match &self.transport {
            #[cfg(feature = "test-host")]
            Transport::TestReads(_) => Err(std::io::Error::other(
                "The test read handler has no streaming connection.",
            )),
            #[cfg(feature = "local-host")]
            Transport::Local(path) => crate::local_socket::connect(path),
            Transport::Link(link) => link.open(),
            #[cfg(feature = "test-host")]
            Transport::TestHost {
                host: Some(path), ..
            } => crate::local_socket::connect(path),
            #[cfg(feature = "test-host")]
            Transport::TestHost { host: None, .. } => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No test host socket is configured.",
            )),
            #[cfg(feature = "local-host")]
            Transport::Unavailable(reason) => Err(std::io::Error::other(reason.clone())),
        }
    }

    #[cfg(not(feature = "test-host"))]
    fn socket_for_encoded(&self, _bytes: &[u8]) -> std::io::Result<UnixStream> {
        self.socket()
    }

    #[cfg(feature = "test-host")]
    fn socket_for_encoded(&self, bytes: &[u8]) -> std::io::Result<UnixStream> {
        let Transport::TestHost { host, git } = &self.transport else {
            return self.socket();
        };
        #[derive(Deserialize)]
        struct Op<'a> {
            #[serde(borrow)]
            op: &'a str,
        }
        match serde_json::from_slice::<Op<'_>>(bytes).ok().map(|v| v.op) {
            Some("git_read" | "git_read_page") => crate::local_socket::connect(git),
            Some("git_write") => {
                if let Some(path) = host.as_ref().filter(|p| p.exists()) {
                    crate::local_socket::connect(path)
                } else {
                    crate::local_socket::connect(git)
                }
            }
            _ => self.socket(),
        }
    }

    fn connect(&self, request: &Request) -> Result<Frames, Error> {
        let bytes = serde_json::to_vec(request)
            .map_err(|_| Error::Protocol("Cannot encode host request.".into()))?;
        if bytes.len() + 1 > REQUEST_LIMIT {
            return Err(Error::Protocol(
                "Message exceeds the host's 64 KiB request limit.".into(),
            ));
        }
        let mut socket = self.socket()?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        socket.write_all(&bytes)?;
        socket.write_all(b"\n")?;
        Ok(Frames {
            reader: BufReader::new(socket),
            pending: Vec::new(),
            partial_since: None,
        })
    }

    /// One request and reply. A versioned status request is the negotiation: an
    /// older host rejects a version it does not know, so ask for the one below,
    /// and finally without the field, which every host answers as version 1.
    pub fn request<T: DeserializeOwned>(&self, request: &Request) -> Result<T, Error> {
        let mut result = self.exchange(request);
        let Request::Status {
            protocol_version: Some(newest),
        } = request
        else {
            return result;
        };
        let mut version = *newest;
        while version > 1
            && matches!(&result, Err(Error::Rejected(error)) if error.code == "invalid_request")
        {
            version -= 1;
            result = self.exchange(&Request::Status {
                protocol_version: (version > 1).then_some(version),
            });
        }
        result
    }

    fn exchange<T: DeserializeOwned>(&self, request: &Request) -> Result<T, Error> {
        let mut frames = self.connect(request)?;
        let line = frames.next(Some(Instant::now() + Duration::from_secs(15)))?;
        if let Request::CreateThread { checkout, .. } = request {
            validate_created_checkout(&line, checkout)?;
        }
        decode(&line)
    }

    pub fn watch(&self, thread_id: String, protocol: u32) -> Result<Watch, Error> {
        Ok(Watch {
            frames: self.connect(&Request::Watch {
                thread_id: thread_id.clone(),
                // Version 1 hosts reject the field; newer ones answer in the version asked.
                protocol_version: (protocol >= 2).then_some(protocol.min(PROTOCOL_VERSION)),
            })?,
            thread_id,
            identity: None,
            first: true,
            initial_deadline: Instant::now() + Duration::from_secs(15),
        })
    }
}

struct Frames {
    reader: BufReader<UnixStream>,
    pending: Vec<u8>,
    partial_since: Option<Instant>,
}
impl Frames {
    fn readable(&self, timeout: Option<Duration>) -> std::io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd: self.reader.get_ref().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let milliseconds = timeout.map_or(-1, |value| {
            value.as_millis().clamp(1, i32::MAX as u128) as i32
        });
        let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        // HUP still permits reading a reply buffered before the peer closed.
        Ok(result > 0)
    }

    fn next(&mut self, deadline: Option<Instant>) -> Result<Vec<u8>, Error> {
        loop {
            let deadline = deadline.or_else(|| {
                self.partial_since
                    .map(|start| start + Duration::from_secs(15))
            });
            let timeout = if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(Error::Transport("Host frame timed out. The request may have been accepted; reconnect before retrying.".into()));
                }
                Some(remaining)
            } else {
                None
            };
            // macOS rejects SO_RCVTIMEO after a peer closes, even with unread
            // reply bytes. Poll keeps the deadline without changing that socket.
            if self.reader.buffer().is_empty() && !self.readable(timeout)? {
                continue;
            }
            let available = match self.reader.fill_buf() {
                Ok(bytes) => bytes,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(Error::Transport("Host frame timed out. Reconnect before retrying; the request may have been accepted.".into()));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            if available.is_empty() {
                return Err(Error::Transport(
                    "Host disconnected. Reconnect to resume this thread.".into(),
                ));
            }
            let end = available.iter().position(|byte| *byte == b'\n');
            let count = end.map_or(available.len(), |index| index + 1);
            if self.pending.len() + count > REPLY_LIMIT {
                return Err(Error::Protocol(
                    "Host reply exceeds 1 MiB; connection closed.".into(),
                ));
            }
            self.partial_since.get_or_insert_with(Instant::now);
            self.pending.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            if end.is_some() {
                self.partial_since = None;
                return Ok(std::mem::take(&mut self.pending));
            }
        }
    }
}

pub mod history;
pub mod thread_naming;
pub mod threads;
mod watch;
pub use history::{HistoryField, HistoryItem, HistoryPage, HistoryText, More};
pub use threads::ThreadWatch;
pub use watch::{CancelWatch, Snapshots, ThreadLists, Updates};

pub mod admission;
pub mod agent;
pub mod automations;
pub mod build_output;
pub mod checkout_queue;
pub mod cleanup;
pub mod device;
pub mod folders;
pub mod git;

pub mod identity;
pub mod linear;
pub mod policy;
pub mod providers;
pub mod release;
pub mod review;
pub mod sign_in;
pub mod workspace;
pub mod worktree;

impl Client {
    /// One request and reply for an operation defined outside this file, such
    /// as provider sign-in. The encoded request can hold a secret, so it is
    /// overwritten once written and never appears in an error.
    pub fn send<R: Serialize, T: DeserializeOwned>(&self, request: &R) -> Result<T, Error> {
        self.send_within(request, Duration::from_secs(20))
    }

    /// `send` for an operation the host may take longer to answer, such as a
    /// Git read that runs several bounded commands.
    pub fn send_within<R: Serialize, T: DeserializeOwned>(
        &self,
        request: &R,
        wait: Duration,
    ) -> Result<T, Error> {
        #[cfg(feature = "test-host")]
        if let Transport::TestReads(reply) = &self.transport {
            let request = serde_json::to_value(request)
                .map_err(|_| Error::Protocol("Cannot encode test request.".into()))?;
            return decode(reply(request).to_string().as_bytes());
        }
        let mut bytes = serde_json::to_vec(request)
            .map_err(|_| Error::Protocol("Cannot encode host request.".into()))?;
        let result = self.send_encoded(&bytes);
        bytes.fill(0);
        std::hint::black_box(&bytes);
        decode(&result?.next(Some(Instant::now() + wait))?)
    }

    fn send_encoded(&self, bytes: &[u8]) -> Result<Frames, Error> {
        if bytes.len() + 1 > REQUEST_LIMIT {
            return Err(Error::Protocol(
                "Request exceeds the host's 64 KiB limit.".into(),
            ));
        }
        let mut socket = self.socket_for_encoded(bytes)?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        socket.write_all(bytes)?;
        socket.write_all(b"\n")?;
        Ok(Frames {
            reader: BufReader::new(socket),
            pending: Vec::new(),
            partial_since: None,
        })
    }
}

pub struct Watch {
    frames: Frames,
    thread_id: String,
    identity: Option<PathBuf>,
    first: bool,
    initial_deadline: Instant,
}
impl Watch {
    pub fn next_snapshot(&mut self) -> Result<Snapshot, Error> {
        let line = self
            .frames
            .next(self.first.then_some(self.initial_deadline))?;
        let snapshot = if self.first {
            let reply: ThreadReply = decode(&line)?;
            self.first = false;
            reply.thread
        } else {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Event {
                event: String,
                thread: Snapshot,
            }
            let event: Event = serde_json::from_slice(&line).map_err(|_| {
                Error::Protocol("Invalid host event; reconnect to load current state.".into())
            })?;
            if event.event != "snapshot" {
                return Err(Error::Protocol("Unsupported host event.".into()));
            }
            event.thread
        };
        // Validate before coalescing so a later good frame cannot hide an identity violation.
        if snapshot.id != self.thread_id {
            return Err(Error::Protocol("Host returned the wrong thread.".into()));
        }
        // The checkout is fixed for a thread's life. Its model can change between runs.
        if let Some(checkout) = &self.identity {
            if *checkout != snapshot.checkout {
                return Err(Error::Protocol(
                    "Host changed this thread's saved checkout.".into(),
                ));
            }
        } else {
            self.identity = Some(snapshot.checkout.clone());
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_reply_requires_the_requested_checkout() {
        let line = br#"{"ok":true,"result":{"thread":{"checkout":"/a"}}}"#;
        assert!(validate_created_checkout(line, std::path::Path::new("/a")).is_ok());
        assert!(validate_created_checkout(line, std::path::Path::new("/b")).is_err());
    }

    #[test]
    fn client_round_trip_checks_socket_ownership_and_permissions() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let directory = std::env::temp_dir().join(format!("plait-client-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["op"], "status");
            socket.write_all(b"{\"ok\":true,\"result\":{\"protocol_version\":1,\"default_model\":null,\"models\":[]}}\n").unwrap();
        });
        let client = Client {
            transport: Transport::Local(path.clone()),
        };
        let reply: Status = client
            .request(&Request::Status {
                protocol_version: None,
            })
            .unwrap();
        assert_eq!(reply.protocol_version, 1);
        server.join().unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(matches!(client.request::<Status>(&Request::Status {
                protocol_version: None
            }), Err(Error::Transport(message)) if message.contains("owner-only")));
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Answers each connection with the next scripted line and reports what was asked.
    fn scripted(
        name: &str,
        replies: Vec<impl Into<String>>,
    ) -> (
        Client,
        std::thread::JoinHandle<Vec<serde_json::Value>>,
        PathBuf,
    ) {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let directory = std::env::temp_dir().join(format!("plait-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let replies = replies.into_iter().map(Into::into).collect::<Vec<String>>();
        let server = std::thread::spawn(move || {
            replies
                .into_iter()
                .map(|reply| {
                    let (mut socket, _) = listener.accept().unwrap();
                    let mut line = String::new();
                    BufReader::new(socket.try_clone().unwrap())
                        .read_line(&mut line)
                        .unwrap();
                    socket.write_all(reply.as_bytes()).unwrap();
                    socket.write_all(b"\n").unwrap();
                    serde_json::from_str(&line).unwrap()
                })
                .collect()
        });
        (
            Client {
                transport: Transport::Local(path),
            },
            server,
            directory,
        )
    }

    #[test]
    fn checkout_queue_capability_never_breaks_the_thread_list() {
        let (client, server, directory) = scripted(
            "queue-capability",
            vec![
                r#"{"ok":false,"error":{"code":"invalid_request","message":"Unknown operation."}}"#,
                r#"{"ok":true,"result":{"schema":1,"checkout_queue_v1":true,"limit":32,"payload_bytes":33554432}}"#,
                r#"{"ok":true,"result":{"schema":1,"checkout_queue_v1":true,"limit":1000,"payload_bytes":33554432}}"#,
                r#"{"ok":false,"error":{"code":"busy","message":"Try again."}}"#,
            ],
        );
        assert!(!checkout_queue::available(&client).unwrap());
        assert!(checkout_queue::available(&client).unwrap());
        // A capability this build does not know, or a refused probe, leaves Wait
        // unavailable; the thread listing that asks first must still load.
        assert!(!checkout_queue::available(&client).unwrap());
        assert!(!checkout_queue::available(&client).unwrap());
        for request in server.join().unwrap() {
            assert_eq!(request, serde_json::json!({"op":"checkout_queue_info"}));
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn image_downloads_accept_large_parts_and_refuse_oversized_parts() {
        use crate::images::{IMAGE_BYTES, READ_CHUNK};
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut bytes = crate::images::TEST_PNG_HEADER.to_vec();
        bytes.resize(IMAGE_BYTES, 0);
        let reference = crate::images::inspect(&bytes).unwrap();
        assert_eq!(reference.size, 5 * 1024 * 1024);
        let parts = bytes.chunks(READ_CHUNK).enumerate().map(|(index, chunk)| {
            let part = serde_json::json!({"ok":true,"result":{"image":reference,"offset":index*READ_CHUNK,"data":STANDARD.encode(chunk)}}).to_string();
            assert!(part.len() + 1 < REPLY_LIMIT);
            part
        }).collect::<Vec<_>>();
        assert_eq!(parts.len(), 10);
        let (client, server, directory) = scripted("image-parts", parts);
        assert_eq!(client.image("t", &reference).unwrap(), bytes);
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 10);
        assert_eq!(requests[9]["offset"], 9 * READ_CHUNK);
        std::fs::remove_dir_all(directory).unwrap();
        let too_big = serde_json::json!({"ok":true,"result":{"image":reference,"offset":0,"data":STANDARD.encode(vec![0;READ_CHUNK+1])}}).to_string();
        let (client, server, directory) = scripted("image-part-refused", vec![too_big]);
        assert!(client.image("t", &reference).is_err());
        server.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        // The combined boundary is also enforced before any socket or disk work.
        let mut another = reference.clone();
        another.id = "1".repeat(64);
        another.size = 1;
        assert!(crate::images::validate_message(std::slice::from_ref(&reference)).is_ok());
        assert!(crate::images::validate_message(&[reference, another]).is_err());
    }

    #[test]
    fn version_five_record_requests_and_replies() {
        let request = Request::turn_records("t".into(), Some("4:500:0:0".into()), Some(2), 5);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"op":"turn_records","protocol_version":5,"thread_id":"t","before":"4:500:0:0","limit":2})
        );
        let request = Request::undo_turn("t".into(), "r".into(), "u".into(), true);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"op":"undo_turn","protocol_version":5,"thread_id":"t","record_id":"r","request_id":"u","leave_changed":true})
        );
        let reply: UndoTurnReply = serde_json::from_value(serde_json::json!({"state":"blocked","changed_since":["a"],"changed_since_total":1,"not_covered":[],"not_covered_total":0,"record_id":"r","after_restart":true})).unwrap();
        assert_eq!(reply.state, UndoState::Blocked);
        assert!(reply.after_restart);
        let page: TurnRecordsReply = serde_json::from_value(serde_json::json!({"records":[{"id":"r","request_ids":["initial","steer"],"request_ids_offset":0,"request_ids_total":2,"kind":"turn","undoes":null,"state":"ready","head_moved":false,"after_restart":false,"paths":[{"path":"a","change":"modified","added":1,"removed":2}],"paths_offset":0,"paths_total":1,"not_covered":[],"not_covered_offset":0,"not_covered_total":0,"created_at":1}],"before":null})).unwrap();
        assert_eq!(page.records[0].paths[0].change, RecordChange::Modified);
    }

    #[test]
    fn old_host_refuses_undo_without_retrying_at_an_older_version() {
        let (client, server, directory) = scripted("undo-old-host", vec![UNKNOWN_VERSION]);
        assert!(matches!(
            client.request::<UndoTurnReply>(&Request::undo_turn(
                "t".into(),
                "r".into(),
                "u".into(),
                false
            )),
            Err(Error::Rejected(_))
        ));
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["protocol_version"], 5);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn image_requests_and_replies_keep_version_five_text_shapes() {
        let plain = Request::submit("t".into(), "r".into(), "look".into(), None);
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("images")
                .is_none()
        );
        let image = crate::images::ImageRef {
            id: "a".repeat(64),
            media_type: "image/png".into(),
            size: 42,
            width: 1,
            height: 1,
        };
        let pending = crate::host_state::PendingSubmit {
            checkout_wait: false,
            request_id: "r".into(),
            text: String::new(),
            images: vec![image.clone()],
            approval: None,
            when_busy: None,
            draft_revision: Some(1),
        };
        let wire = serde_json::to_value(pending.request("t")).unwrap();
        assert_eq!(wire["protocol_version"], 6);
        assert_eq!(wire["images"], serde_json::json!([image.id]));
        let old = serde_json::json!({"id":"t","checkout":"/repo","model":{"provider":"p","model_id":"m"},"status":"idle","messages":[{"id":"r","role":"user","text":"look"}],"tools":[],"live_text":"","pending_approval":null,"error":null,"truncated":false,"records_revision":0});
        let mut new = old.clone();
        new["image_input"] = serde_json::json!("yes");
        new["messages"][0]["images"] = serde_json::json!([image]);
        let old: Snapshot = serde_json::from_value(old).unwrap();
        let new: Snapshot = serde_json::from_value(new).unwrap();
        assert!(old.messages[0].images.is_empty());
        assert!(old.image_input.is_none());
        assert_eq!(new.messages[0].images.len(), 1);
        assert_eq!(new.image_input.as_deref(), Some("yes"));
    }

    const UNKNOWN_VERSION: &str = r#"{"ok":false,"error":{"code":"invalid_request","message":"Unsupported protocol_version."}}"#;
    const UNKNOWN_FIELD: &str = r#"{"ok":false,"error":{"code":"invalid_request","message":"Unknown operation or field."}}"#;

    #[test]
    fn negotiation_steps_down_from_seven_to_one() {
        let newest = Request::Status {
            protocol_version: Some(PROTOCOL_VERSION),
        };
        let asked = |requests: Vec<serde_json::Value>| -> Vec<serde_json::Value> {
            requests
                .into_iter()
                .map(|request| request["protocol_version"].clone())
                .collect()
        };
        let (client, server, directory) = scripted(
            "negotiate-5",
            vec![
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                r#"{"ok":true,"result":{"protocol_version":5,"default_model":null,"models":[]}}"#,
            ],
        );
        let status: Status = client.request(&newest).unwrap();
        assert_eq!(status.protocol_version, 5);
        assert_eq!(
            asked(server.join().unwrap()),
            [
                serde_json::json!(7),
                serde_json::json!(6),
                serde_json::json!(5)
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();

        // A version 4 host answers after rejecting versions 7 through 5.
        let (client, server, directory) = scripted(
            "negotiate-4",
            vec![
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                r#"{"ok":true,"result":{"protocol_version":4,"default_model":null,"models":[]}}"#,
            ],
        );
        let status: Status = client.request(&newest).unwrap();
        assert_eq!(status.protocol_version, 4);
        assert_eq!(
            asked(server.join().unwrap()),
            [
                serde_json::json!(7),
                serde_json::json!(6),
                serde_json::json!(5),
                serde_json::json!(4)
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();

        // A version 3 host answers the step below, with model capabilities.
        let (client, server, directory) = scripted(
            "negotiate-3",
            vec![
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                r#"{"ok":true,"result":{"protocol_version":3,"default_model":null,"models":[{"provider":"p","model_id":"m","name":"M","authenticated":true,"reasoning":true,"thinking_levels":["off","xhigh","max"],"context_window":200000,"input":["text","image"]}]}}"#,
            ],
        );
        let status: Status = client.request(&newest).unwrap();
        assert_eq!(status.protocol_version, 3);
        assert_eq!(
            status.models[0].thinking_levels.as_deref(),
            Some(&[ThinkingLevel::Off, ThinkingLevel::Xhigh, ThinkingLevel::Max][..])
        );
        assert_eq!(status.models[0].context_window, Some(200000));
        assert_eq!(
            asked(server.join().unwrap()),
            [
                serde_json::json!(7),
                serde_json::json!(6),
                serde_json::json!(5),
                serde_json::json!(4),
                serde_json::json!(3)
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();

        // A version 2 host knows the field but not the number.
        let (client, server, directory) = scripted(
            "negotiate-2",
            vec![
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                r#"{"ok":true,"result":{"protocol_version":2,"default_model":null,"models":[]}}"#,
            ],
        );
        let status: Status = client.request(&newest).unwrap();
        assert_eq!(status.protocol_version, 2);
        assert_eq!(
            asked(server.join().unwrap()),
            [
                serde_json::json!(7),
                serde_json::json!(6),
                serde_json::json!(5),
                serde_json::json!(4),
                serde_json::json!(3),
                serde_json::json!(2)
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();

        // A version 1 host rejects the field itself, so the last request omits it.
        let (client, server, directory) = scripted(
            "negotiate-1",
            vec![
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_VERSION,
                UNKNOWN_FIELD,
                UNKNOWN_FIELD,
                UNKNOWN_FIELD,
                r#"{"ok":true,"result":{"protocol_version":1,"default_model":null,"models":[{"provider":"p","model_id":"m","name":"M","authenticated":false}]}}"#,
            ],
        );
        let status: Status = client.request(&newest).unwrap();
        assert_eq!(status.protocol_version, 1);
        assert_eq!(status.models[0].reasoning, None);
        assert_eq!(
            asked(server.join().unwrap()),
            [
                serde_json::json!(7),
                serde_json::json!(6),
                serde_json::json!(5),
                serde_json::json!(4),
                serde_json::json!(3),
                serde_json::json!(2),
                serde_json::Value::Null
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();

        // Any other failure is not a version answer and is not retried.
        let (client, server, directory) = scripted(
            "negotiate-busy",
            vec![r#"{"ok":false,"error":{"code":"internal","message":"Host operation failed."}}"#],
        );
        assert!(
            matches!(client.request::<Status>(&newest), Err(Error::Rejected(error)) if error.code == "internal")
        );
        assert_eq!(server.join().unwrap().len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_three_requests_have_the_documented_wire_shapes() {
        let wire = |request: Request| serde_json::to_value(&request).unwrap();
        let json = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
        assert_eq!(
            wire(Request::list_threads_page(None, None, None, false)),
            json(r#"{"op":"list_threads","protocol_version":3}"#)
        );
        assert_eq!(
            wire(Request::list_threads_page(
                Some("/repo".into()),
                Some(50),
                Some("9:4".into()),
                true
            )),
            json(
                r#"{"op":"list_threads","protocol_version":3,"checkout":"/repo","limit":50,"cursor":"9:4","archived":true}"#
            )
        );
        assert_eq!(
            wire(Request::WatchThreads {
                archived: false,
                protocol_version: 3
            }),
            json(r#"{"op":"watch_threads","protocol_version":3}"#)
        );
        assert_eq!(
            wire(Request::history("t".into(), None, None)),
            json(r#"{"op":"history","thread_id":"t","protocol_version":3}"#)
        );
        assert_eq!(
            wire(Request::history(
                "t".into(),
                Some("7:0:0".into()),
                Some(100)
            )),
            json(
                r#"{"op":"history","thread_id":"t","before":"7:0:0","limit":100,"protocol_version":3}"#
            )
        );
        assert_eq!(
            wire(Request::history_text("t".into(), "e7.0.text".into(), 65536)),
            json(
                r#"{"op":"history_text","thread_id":"t","ref":"e7.0.text","offset":65536,"protocol_version":3}"#
            )
        );
        let choice = ModelChoice {
            provider: "p".into(),
            model_id: "m".into(),
        };
        let create_in = |thinking, harness, protocol| {
            wire(Request::create_thread(
                "/c".into(),
                &choice,
                "r".into(),
                thinking,
                harness,
                protocol,
            ))
        };
        let create = |thinking, protocol| create_in(thinking, harness::HarnessId::Pi, protocol);
        // A starting level goes only to a version 3 host, which then answers in version 3.
        assert_eq!(
            create(Some(ThinkingLevel::High), 3),
            json(
                r#"{"op":"create_thread","checkout":"/c","provider":"p","model_id":"m","request_id":"r","thinking":"high","protocol_version":3}"#
            )
        );
        let plain = json(
            r#"{"op":"create_thread","checkout":"/c","provider":"p","model_id":"m","request_id":"r"}"#,
        );
        assert_eq!(create(Some(ThinkingLevel::High), 2), plain);
        assert_eq!(create(None, 3), plain);
        // Pi is never named, so a Pi thread's request stays as version 3 sends it.
        assert_eq!(create(None, 4), plain);
        assert_eq!(
            create_in(None, harness::HarnessId::Codex, 4),
            json(
                r#"{"op":"create_thread","checkout":"/c","provider":"p","model_id":"m","request_id":"r","harness":"codex","protocol_version":4}"#
            )
        );
        assert_eq!(create_in(None, harness::HarnessId::Codex, 3), plain);
        let with_mode = |harness, protocol| {
            wire(
                Request::create_thread("/c".into(), &choice, "r".into(), None, harness, protocol)
                    .with_mode(Some("ask".into())),
            )
        };
        assert_eq!(
            with_mode(harness::HarnessId::Codex, 4),
            json(
                r#"{"op":"create_thread","checkout":"/c","provider":"p","model_id":"m","request_id":"r","harness":"codex","mode":"ask","protocol_version":4}"#
            )
        );
        // A Pi thread, or a host too old to name the harness, never gets a mode.
        assert_eq!(with_mode(harness::HarnessId::Pi, 4), plain);
        assert_eq!(with_mode(harness::HarnessId::Codex, 3), plain);
        assert_eq!(
            wire(Request::set_thread_mode("t".into(), "read_only".into())),
            json(
                r#"{"op":"configure_thread","thread_id":"t","mode":"read_only","protocol_version":4}"#
            )
        );
        assert_eq!(
            wire(Request::rename_thread("t".into(), "Name".into())),
            json(r#"{"op":"rename_thread","thread_id":"t","title":"Name","protocol_version":3}"#)
        );
        assert_eq!(
            wire(Request::archive_thread("t".into(), true)),
            json(r#"{"op":"archive_thread","thread_id":"t","archived":true,"protocol_version":3}"#)
        );
        assert_eq!(
            wire(Request::configure_thread(
                "t".into(),
                Some(ModelChoice {
                    provider: "p".into(),
                    model_id: "m".into()
                }),
                None
            )),
            json(
                r#"{"op":"configure_thread","thread_id":"t","provider":"p","model_id":"m","protocol_version":3}"#
            )
        );
        assert_eq!(
            wire(Request::configure_thread(
                "t".into(),
                None,
                Some(ThinkingLevel::Xhigh)
            )),
            json(
                r#"{"op":"configure_thread","thread_id":"t","thinking":"xhigh","protocol_version":3}"#
            )
        );
        assert_eq!(
            wire(Request::cancel_queued("t".into(), "r".into())),
            json(r#"{"op":"cancel_queued","thread_id":"t","request_id":"r","protocol_version":3}"#)
        );
        assert_eq!(
            wire(Request::changed_files("t".into())),
            json(r#"{"op":"changed_files","thread_id":"t","protocol_version":3}"#)
        );
        assert_eq!(
            wire(Request::submit_when_busy(
                "t".into(),
                "r".into(),
                "hi".into(),
                ApprovalMode::Ask,
                QueueMode::Steer
            )),
            json(
                r#"{"op":"submit","thread_id":"t","request_id":"r","text":"hi","protocol_version":3,"approval":"ask","when_busy":"steer"}"#
            )
        );
        // Requests that existed before keep their exact shape.
        assert_eq!(
            wire(Request::submit(
                "t".into(),
                "r".into(),
                "hi".into(),
                Some(ApprovalMode::Auto)
            )),
            json(
                r#"{"op":"submit","thread_id":"t","request_id":"r","text":"hi","protocol_version":2,"approval":"auto"}"#
            )
        );
        assert_eq!(
            wire(Request::ListThreads {
                identity: None,
                checkout: "/repo".into()
            }),
            json(r#"{"op":"list_threads","checkout":"/repo"}"#)
        );
    }

    #[test]
    fn version_three_replies_decode_and_older_listings_still_do() {
        let page: ThreadsPage = decode(br#"{"ok":true,"result":{"threads":[{"id":"t","checkout":"/repo","model":{"provider":"p","model_id":"m"},"status":"awaiting_approval","title":"Fix it","created_at":5,"updated_at":9,"archived":false,"pending_approval":true}],"next_cursor":"9:1"}}"#).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("9:1"));
        assert_eq!(page.threads[0].title.as_deref(), Some("Fix it"));
        assert_eq!(
            (page.threads[0].updated_at, page.threads[0].pending_approval),
            (Some(9), Some(true))
        );
        let last: ThreadsPage =
            decode(br#"{"ok":true,"result":{"threads":[],"next_cursor":null}}"#).unwrap();
        assert!(last.next_cursor.is_none());
        let older: ThreadsReply = decode(br#"{"ok":true,"result":{"threads":[{"id":"t","checkout":"/repo","model":{"provider":"p","model_id":"m"},"status":"idle"}]}}"#).unwrap();
        assert_eq!(
            (older.threads[0].title.clone(), older.threads[0].archived),
            (None, None)
        );
        assert!(decode::<ThreadsPage>(br#"{"ok":true,"result":{"threads":[{"id":"t","checkout":"/repo","model":{"provider":"p","model_id":"m"},"status":"idle","extra":1}],"next_cursor":null}}"#).is_err());
    }

    #[test]
    fn unknown_harness_survives_snapshot_and_summary_decoding() {
        let reply: ThreadReply = decode(br#"{"ok":true,"result":{"thread":{"id":"t","checkout":"/repo","model":{"provider":"p","model_id":"m"},"status":"idle","messages":[],"live_text":"","tools":[],"pending_approval":null,"error":null,"truncated":false,"harness":{"id":"antigravity"}}}}"#).unwrap();
        assert_eq!(
            reply.thread.harness.as_ref().unwrap().id,
            harness::HarnessId::Other
        );
        assert_eq!(
            ThreadSummary::from(&reply.thread).harness,
            Some(harness::HarnessId::Other)
        );
        let page: ThreadsPage = decode(br#"{"ok":true,"result":{"threads":[{"id":"t","checkout":"/repo","model":{"provider":"p","model_id":"m"},"status":"idle","harness":"antigravity"}],"next_cursor":null}}"#).unwrap();
        assert_eq!(page.threads[0].harness, Some(harness::HarnessId::Other));
    }

    #[test]
    fn claude_code_threads_decode_in_summaries_and_snapshots() {
        let page: ThreadsPage = decode(br#"{"ok":true,"result":{"threads":[{"id":"t","checkout":"/repo","model":{"provider":"claude","model_id":"default"},"status":"awaiting_approval","title":"Fix it","created_at":5,"updated_at":9,"archived":false,"pending_approval":true,"harness":"claude"}],"next_cursor":null}}"#).unwrap();
        assert_eq!(page.threads[0].harness, Some(harness::HarnessId::Claude));
        let reply: ThreadReply = decode(br#"{"ok":true,"result":{"thread":{"id":"t","checkout":"/repo","model":{"provider":"claude","model_id":"default"},"status":"idle","messages":[],"live_text":"","tools":[],"pending_approval":null,"error":null,"truncated":false,"approval_mode":"ask","title":"Fix it","created_at":5,"updated_at":9,"archived":false,"thinking_level":"high","queued":[],"usage":null,"live_reasoning":"","retry":null,"harness":{"id":"claude","version":"2.1.326","mode":"auto","reported":"custom"}}}}"#).unwrap();
        let thread = reply.thread.harness.unwrap();
        assert_eq!(thread.id, harness::HarnessId::Claude);
        assert_eq!(thread.version.as_deref(), Some("2.1.326"));
        assert_eq!(
            (thread.mode.as_deref(), thread.reported.as_deref()),
            (Some("auto"), Some("custom"))
        );
    }

    /// Replies recorded from the real host and runtime in a scripted session, so the
    /// strict decoders are checked against what the host sends, not a hand-written copy.
    /// Paths are replaced and strings over 200 characters are shortened.
    #[test]
    fn decodes_version_three_replies_recorded_from_the_host() {
        let recorded: serde_json::Value =
            serde_json::from_str(include_str!("host/testdata/replies-v3.json")).unwrap();
        fn read<T: DeserializeOwned>(recorded: &serde_json::Value, name: &str) -> T {
            decode(recorded[name].to_string().as_bytes())
                .unwrap_or_else(|error| panic!("{name}: {error}"))
        }
        let status: Status = read(&recorded, "status");
        assert_eq!(status.protocol_version, 3);
        assert_eq!(status.models[1].reasoning, Some(true));
        assert!(
            status
                .models
                .iter()
                .all(|model| model.thinking_levels.is_some()
                    && model.context_window.is_some()
                    && model.input.is_some())
        );

        for name in ["create", "configure", "cancel", "rename", "archive", "stop"] {
            let reply: ThreadReply = read(&recorded, name);
            assert!(reply.thread.title.is_some() && reply.thread.created_at.is_some());
        }
        let configured: ThreadReply = read(&recorded, "configure");
        assert_eq!(configured.thread.thinking_level, Some(ThinkingLevel::High));
        let busy: ThreadReply = read(&recorded, "watch_busy");
        assert_eq!(
            busy.thread
                .queued
                .iter()
                .map(|queued| (queued.request_id.as_str(), queued.mode, queued.approval))
                .collect::<Vec<_>>(),
            [
                ("f1", QueueMode::FollowUp, ApprovalMode::Ask),
                ("f2", QueueMode::Steer, ApprovalMode::Ask)
            ]
        );
        let idle: ThreadReply = read(&recorded, "watch_idle");
        assert!(idle.thread.queued.is_empty() && idle.thread.retry.is_none());
        assert!(
            idle.thread
                .usage
                .as_ref()
                .is_some_and(|usage| usage.total > 0)
        );
        assert!(
            idle.thread
                .messages
                .iter()
                .any(|message| message.text.is_empty() && !message.reasoning.is_empty())
        );
        let archived: ThreadReply = read(&recorded, "archive");
        assert_eq!(
            (archived.thread.archived, archived.thread.title.as_deref()),
            (Some(true), Some("Recorded thread"))
        );
        for name in ["submit", "submit_queued"] {
            assert!(read::<SubmitReply>(&recorded, name).accepted);
        }

        let page: ThreadsPage = read(&recorded, "list");
        assert_eq!(page.threads[0].archived, Some(true));
        assert!(page.next_cursor.is_none());

        let long: HistoryPage = read(&recorded, "history_long");
        let more = &long.items[1].more()[0];
        assert_eq!(
            (more.field, more.offset, more.length),
            (HistoryField::Text, 65536, 200010)
        );
        let part: HistoryText = read(&recorded, "history_text");
        assert_eq!((part.offset, part.next_offset), (65536, Some(131072)));
        let last: HistoryText = read(&recorded, "history_text_last");
        assert_eq!((last.next_offset, last.length), (None, 200010));
        let history: HistoryPage = read(&recorded, "history");
        assert_eq!(history.items.len(), 2);
        assert!(history.before.is_some());

        // The thread-list frames run through the real reader.
        use std::os::unix::net::UnixStream;
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let mut watch = threads::ThreadWatch::reading(Frames {
            reader: BufReader::new(socket),
            pending: Vec::new(),
            partial_since: None,
        });
        let frames = recorded["thread_frames"].as_array().unwrap();
        for frame in frames {
            peer.write_all(format!("{frame}\n").as_bytes()).unwrap();
        }
        let mut list = Vec::new();
        for index in 0..frames.len() {
            list = watch.next_list().unwrap();
            if index == 0 {
                assert_eq!(list.len(), 1);
            }
        }
        // The last recorded frame removes the archived thread.
        assert!(list.is_empty());
    }

    #[test]
    fn rejects_inconsistent_or_unknown_replies() {
        assert!(decode::<DefaultReply>(br#"{"ok":true,"result":{"default_model":{"provider":"p","model_id":"m"}},"error":{"code":"internal","message":"bad"}}"#).is_err());
        assert!(decode::<DefaultReply>(br#"{"ok":true,"result":{"default_model":{"provider":"p","model_id":"m"}},"extra":true}"#).is_err());
        assert!(matches!(
            decode::<DefaultReply>(br#"{"ok":false,"error":{"code":"busy","message":"Running"}}"#),
            Err(Error::Rejected(_))
        ));
    }

    #[test]
    fn replies_remain_readable_after_the_peer_closes() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"first\nsecond\npartial").unwrap();
        drop(writer);
        let mut frames = Frames {
            reader: BufReader::new(reader),
            pending: Vec::new(),
            partial_since: None,
        };
        assert_eq!(
            frames
                .next(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
            b"first\n"
        );
        assert_eq!(frames.next(None).unwrap(), b"second\n");
        assert!(matches!(frames.next(None), Err(Error::Transport(_))));
        assert_eq!(frames.pending, b"partial");
    }

    #[test]
    fn drip_fed_frames_obey_absolute_deadline() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..100 {
                if writer.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let mut frames = Frames {
            reader: BufReader::new(reader),
            pending: Vec::new(),
            partial_since: None,
        };
        let deadline = Instant::now() + Duration::from_millis(20);
        assert!(matches!(
            frames.next(Some(deadline)),
            Err(Error::Transport(_))
        ));
        assert!(!frames.pending.is_empty());
        drop(frames);
        writer.join().unwrap();
    }

    #[test]
    fn frames_keep_partial_data_and_bound_size() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let mut frames = Frames {
            reader: BufReader::new(reader),
            pending: Vec::new(),
            partial_since: None,
        };
        writer.write_all(b"partial").unwrap();
        assert!(
            frames
                .next(Some(Instant::now() + Duration::from_millis(5)))
                .is_err()
        );
        writer.write_all(b"\nnext\n").unwrap();
        assert_eq!(frames.next(None).unwrap(), b"partial\n");
        assert_eq!(frames.next(None).unwrap(), b"next\n");
        frames.pending = vec![b'x'; REPLY_LIMIT];
        writer.write_all(b"x").unwrap();
        assert!(matches!(frames.next(None), Err(Error::Protocol(_))));
    }
}
