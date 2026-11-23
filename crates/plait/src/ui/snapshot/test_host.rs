//! In-process test host for desktop scene tests.
//!
//! Serves repository reads, pages, writes, file reads, and checkout identity
//! operations over a private owner-only Unix socket using `plait-engine`, so
//! the desktop exercises its real host client without linking `plait-engine`
//! into production builds. Non-repository operations (and scripted `git_write`
//! when a test binds `$PLAIT_HOST_STORE/host.sock`) continue to use the test's
//! scratch host socket directly.

use base64::{Engine, engine::general_purpose::STANDARD};
use plait_client::host::Client;
use plait_engine::{git, host_git, lease::DirIdentity};
use plait_protocol::host::git::{HelperInput, Outcome, Read, WireIdentity, WriteInput};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, BufRead, BufReader, Write as _},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// Raw bytes per page; matches `runtime/src/git-read.ts` so base64 stays under 1 MiB.
const PAGE_BYTES: usize = 600 * 1024;
const RESULT_LIMIT: usize = 64 * 1024 * 1024;
const MAX_DOCUMENTS: usize = 16;

pub(super) struct TestHost {
    dir: PathBuf,
    socket: PathBuf,
    host_socket: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestHost {
    pub(super) fn start() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Keep the socket path short for macOS's 104-byte Unix socket limit.
        let dir = PathBuf::from("/tmp").join(format!(
            "plait-th-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test host directory");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join("h.sock");
        let listener = UnixListener::bind(&socket).expect("bind test host socket");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        let pages = Arc::new(Mutex::new(Pages::default()));
        // Only point non-Git ops at a test's explicit scratch store, never a developer's real host.
        let store = std::env::var_os("PLAIT_HOST_STORE").map(PathBuf::from);
        let host_socket = store.as_ref().map(|root| root.join("host.sock"));
        let effective_store = store.unwrap_or_else(|| dir.clone());

        let thread = std::thread::spawn(move || {
            for incoming in listener.incoming() {
                if quit.load(Ordering::Acquire) {
                    break;
                }
                let Ok(stream) = incoming else {
                    break;
                };
                let pages = pages.clone();
                let store = effective_store.clone();
                std::thread::spawn(move || handle_connection(stream, &pages, &store));
            }
        });

        Self {
            dir,
            socket,
            host_socket,
            stop,
            thread: Some(thread),
        }
    }

    pub(super) fn client(&self) -> Client {
        Client::with_test_git_host(self.host_socket.clone(), self.socket.clone())
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the blocking `accept()` immediately.
        let _ = UnixStream::connect(&self.socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Default)]
struct Pages {
    next_id: u64,
    order: VecDeque<String>,
    documents: HashMap<String, Vec<u8>>,
}

impl Pages {
    fn store(&mut self, bytes: Vec<u8>) -> Result<Value, (&'static str, &'static str)> {
        if bytes.len() > RESULT_LIMIT {
            return Err(("git_read_limit", "This read is too large to show."));
        }
        if bytes.len() <= PAGE_BYTES {
            return Ok(json!({
                "total": bytes.len(),
                "data": STANDARD.encode(&bytes),
                "token": Value::Null,
            }));
        }
        self.next_id += 1;
        let token = format!("00000000-0000-4000-8000-{:012x}", self.next_id);
        self.documents.insert(token.clone(), bytes);
        self.order.push_back(token.clone());
        while self.documents.len() > MAX_DOCUMENTS {
            if let Some(oldest) = self.order.pop_front() {
                self.documents.remove(&oldest);
            }
        }
        self.page(&token, 0)
    }

    fn page(&mut self, token: &str, offset: usize) -> Result<Value, (&'static str, &'static str)> {
        let Some(bytes) = self.documents.get(token) else {
            return Err(("not_found", "This read expired. Refresh to read it again."));
        };
        if offset >= bytes.len() {
            return Err(("invalid_request", "Invalid page offset."));
        }
        let total = bytes.len();
        let end = (offset + PAGE_BYTES).min(total);
        let data = STANDARD.encode(&bytes[offset..end]);
        let next = if end < total {
            Some(token.to_owned())
        } else {
            self.documents.remove(token);
            None
        };
        Ok(json!({
            "total": total,
            "data": data,
            "token": next,
        }))
    }
}

fn handle_connection(mut stream: UnixStream, pages: &Mutex<Pages>, store: &Path) {
    let Some(line) = read_line(&mut stream) else {
        return;
    };
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(request) => match request.get("op").and_then(Value::as_str) {
            Some("git_read") => handle_git_read(&request, pages, store),
            Some("git_read_page") => handle_git_read_page(&request, pages),
            Some("git_write") => handle_git_write(&request, pages),
            _ => err("invalid_request", "Unknown operation."),
        },
        Err(_) => err("invalid_request", "Invalid request."),
    };
    let _ = write_envelope(&mut stream, reply);
}

fn read_line(stream: &mut UnixStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut line = String::new();
    let count = BufReader::new(stream.try_clone().ok()?)
        .read_line(&mut line)
        .ok()?;
    (count > 0).then_some(line)
}

fn write_envelope(stream: &mut UnixStream, value: Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn ok(result: Value) -> Value {
    json!({"ok": true, "result": result})
}

fn err(code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message}})
}

fn handle_git_read(request: &Value, pages: &Mutex<Pages>, store: &Path) -> Value {
    let mut input: HelperInput = match serde_json::from_value(request.clone()) {
        Ok(input) => input,
        Err(_) => return err("invalid_request", "Invalid Git read."),
    };
    input.store = Some(store.to_owned());
    let bytes = match &input.read {
        Read::Identity => execute_identity(&input),
        Read::MainCheckout => execute_main_checkout(&input),
        _ => host_git::execute(&input),
    };
    match pages.lock().unwrap().store(bytes) {
        Ok(page) => ok(page),
        Err((code, message)) => err(code, message),
    }
}

fn handle_git_read_page(request: &Value, pages: &Mutex<Pages>) -> Value {
    let Some(token) = request.get("token").and_then(Value::as_str) else {
        return err("invalid_request", "Invalid token.");
    };
    let Some(offset) = request.get("offset").and_then(Value::as_u64) else {
        return err("invalid_request", "Invalid page offset.");
    };
    let Ok(offset) = usize::try_from(offset) else {
        return err("invalid_request", "Invalid page offset.");
    };
    match pages.lock().unwrap().page(token, offset) {
        Ok(page) => ok(page),
        Err((code, message)) => err(code, message),
    }
}

fn handle_git_write(request: &Value, pages: &Mutex<Pages>) -> Value {
    let mut forward = request.clone();
    if let Some(object) = forward.as_object_mut() {
        object.remove("op");
        object.remove("op_id");
    }
    let input: WriteInput = match serde_json::from_value(forward) {
        Ok(input) => input,
        Err(_) => return err("invalid_request", "Invalid Git write."),
    };
    let bytes = host_git::execute_write(&input);
    match pages.lock().unwrap().store(bytes) {
        Ok(page) => ok(page),
        Err((code, message)) => err(code, message),
    }
}

fn checked_directory(input: &HelperInput) -> Result<(PathBuf, DirIdentity), String> {
    if !input.checkout.is_absolute() {
        return Err("The checkout path must be absolute.".into());
    }
    let root = std::fs::canonicalize(&input.checkout)
        .map_err(|_| "The checkout is missing.".to_string())?;
    let identity = dir_identity(&root)?;
    if let Some(expected) = &input.identity
        && expected.parse()? != identity
    {
        return Err("Checkout identity changed.".into());
    }
    Ok((root, identity))
}

fn verify_unchanged(original: &Path, canonical: &Path, before: DirIdentity) -> Result<(), String> {
    let current =
        std::fs::canonicalize(original).map_err(|_| "Checkout identity changed.".to_string())?;
    if current != canonical || dir_identity(canonical)? != before {
        return Err("Checkout identity changed.".into());
    }
    Ok(())
}

fn dir_identity(path: &Path) -> Result<DirIdentity, String> {
    use std::os::unix::fs::MetadataExt;
    let meta =
        std::fs::symlink_metadata(path).map_err(|_| "The checkout is missing.".to_string())?;
    if !meta.is_dir() {
        return Err("The checkout is not a folder.".into());
    }
    Ok(DirIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

fn encode_outcome<T: Serialize>(result: Result<(DirIdentity, T), String>) -> Vec<u8> {
    let outcome = match result {
        Ok((identity, value)) => Outcome::Ok {
            identity: identity.into(),
            result: value,
        },
        Err(message) => Outcome::Error { message },
    };
    serde_json::to_vec(&outcome).unwrap_or_default()
}

fn execute_identity(input: &HelperInput) -> Vec<u8> {
    #[derive(Serialize)]
    struct IdentityPayload {
        #[serde(with = "plait_protocol::wire::path")]
        root: PathBuf,
        identity: WireIdentity,
    }
    encode_outcome((|| {
        let (root, before) = checked_directory(input)?;
        verify_unchanged(&input.checkout, &root, before)?;
        Ok((
            before,
            IdentityPayload {
                root,
                identity: before.into(),
            },
        ))
    })())
}

fn execute_main_checkout(input: &HelperInput) -> Vec<u8> {
    #[derive(Serialize)]
    struct PathPayload(#[serde(with = "plait_protocol::wire::path")] PathBuf);
    encode_outcome((|| {
        let (root, before) = checked_directory(input)?;
        let main = git::worktree::main_checkout(&root)?;
        verify_unchanged(&input.checkout, &root, before)?;
        Ok((before, PathPayload(main)))
    })())
}
