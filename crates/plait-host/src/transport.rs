use plait_engine::local_socket;
use serde_json::{Value, json};
#[cfg(test)]
use std::io::BufRead;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

mod notifications;
use notifications::Notifications;

const REQUEST_LIMIT: usize = 65536;
const OUTPUT_LIMIT: usize = 1048576;

#[cfg(test)]
pub fn frame(reader: &mut impl std::io::BufRead, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > limit || bytes.last() != Some(&b'\n') {
        return Err(io::Error::other("invalid or oversized frame"));
    }
    Ok(bytes)
}

fn ready(fd: i32, events: i16, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "frame deadline exceeded"))?;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis().max(1) as i32) };
        if result > 0 {
            return Ok(());
        }
        if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
    }
}

fn timed_frame(
    reader: &mut (impl Read + AsRawFd),
    limit: usize,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        ready(reader.as_raw_fd(), libc::POLLIN, deadline)?;
        let mut buffer = [0u8; 4096];
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Err(io::Error::other("connection closed before newline"));
        }
        let end = buffer[..count].iter().position(|byte| *byte == b'\n');
        bytes.extend_from_slice(&buffer[..end.map_or(count, |index| index + 1)]);
        if bytes.len() > limit {
            return Err(io::Error::other("frame exceeds bound"));
        }
        if end.is_some() {
            return Ok(bytes);
        }
    }
}

fn timed_write(
    writer: &mut (impl Write + AsRawFd),
    mut bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    let fd = writer.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    while !bytes.is_empty() {
        ready(fd, libc::POLLOUT, deadline)?;
        match writer.write(bytes) {
            Ok(0) => return Err(io::Error::other("connection closed")),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn status(store: &Path) -> io::Result<Vec<u8>> {
    let mut socket = local_socket::connect(&store.join("host.sock"))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.write_all(b"{\"op\":\"status\"}\n")?;
    let reply = timed_frame(
        &mut socket,
        OUTPUT_LIMIT,
        Instant::now() + Duration::from_secs(2),
    )?;
    let value: Value = serde_json::from_slice(&reply)?;
    if value["ok"] != true {
        return Err(io::Error::other("host is not ready"));
    }
    Ok(reply)
}

struct Sidecar {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
}

impl Sidecar {
    fn request(&mut self, request: &Value) -> io::Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut exchange = || -> io::Result<Value> {
            let mut bytes = serde_json::to_vec(request)?;
            bytes.push(b'\n');
            timed_write(&mut self.input, &bytes, deadline)?;
            let bytes = timed_frame(&mut self.output, OUTPUT_LIMIT, deadline)?;
            Ok(serde_json::from_slice(&bytes)?)
        };
        let result = exchange();
        if result.is_err() {
            let _ = self.child.kill();
        }
        result
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn private_store(store: &Path) -> io::Result<File> {
    fs::create_dir_all(store)?;
    let meta = fs::symlink_metadata(store)?;
    // Never follow a caller-supplied store symlink or take another user's store.
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::other("store must be an owned directory"));
    }
    fs::set_permissions(store, fs::Permissions::from_mode(0o700))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(store.join("host.lock"))?;
    if !lock.metadata()?.is_file() || lock.metadata()?.nlink() != 1 {
        return Err(io::Error::other("invalid store lock"));
    }
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::other("store already has a writer"));
    }
    Ok(lock)
}

pub fn serve(
    store: &Path,
    runtime: &Path,
    node: &std::ffi::OsStr,
    home: Option<&Path>,
) -> io::Result<()> {
    // Private creation modes also apply to SQLite WALs and runtime scratch files.
    unsafe {
        libc::umask(0o077);
    }
    let lock = private_store(store)?;
    // Validate the leaf before resolving system aliases such as /tmp.
    let store = fs::canonicalize(store)?;
    let socket_path = store.join("host.sock");
    if let Ok(meta) = fs::symlink_metadata(&socket_path) {
        if !meta.file_type().is_socket() {
            return Err(io::Error::other("socket path is not a socket"));
        }
        fs::remove_file(&socket_path)?;
    }
    // The sidecar retains the writer lock if the host dies before it handles EOF.
    if unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_SETFD, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut startup = crate::startup::Startup::new(&store);
    startup.phase("starting_sidecar")?;
    let (notify_reader, notify_writer) = UnixStream::pair()?;
    // Keep change notifications separate from the serialized request/reply pipe.
    if unsafe { libc::fcntl(notify_writer.as_raw_fd(), libc::F_SETFD, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut command = Command::new(node);
    command
        .arg("--disable-warning=ExperimentalWarning")
        .arg(runtime)
        .arg(store)
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .env("PLAIT_HOST_EXE", std::env::current_exe()?)
        .env("PLAIT_LOCK_FD", lock.as_raw_fd().to_string())
        .env("PLAIT_NOTIFY_FD", notify_writer.as_raw_fd().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(home) = home {
        command.env("HOME", home).current_dir(home);
    }
    let child = command.spawn();
    drop(notify_writer);
    let mut child = child.inspect_err(|error| startup.failed(error))?;
    startup.child(child.id());
    let input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("sidecar stdin missing"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("sidecar stdout missing"))?;
    let sidecar = Arc::new(Mutex::new(Sidecar {
        child,
        input,
        output,
    }));
    startup.phase("waiting_for_runtime")?;
    let ready = sidecar
        .lock()
        .unwrap()
        .request(&json!({"op":"status"}))
        .inspect_err(|error| startup.failed(error))?;
    if ready["ok"] != true {
        let error = io::Error::other("sidecar initialization failed");
        startup.failed(&error);
        return Err(error);
    }
    startup.phase("binding_socket")?;
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    startup.phase("ready")?;
    let notifications = Arc::new(Notifications::default());
    let relay = Arc::clone(&notifications);
    thread::spawn(move || {
        // EOF also detects sidecar death, with no periodic child-process check.
        let _ = notifications::relay(notify_reader, &relay);
        std::process::exit(1);
    });
    let clients = Arc::new(AtomicUsize::new(0));
    for socket in listener.incoming() {
        let socket = socket?;
        if clients.fetch_add(1, Ordering::Relaxed) >= 64 {
            clients.fetch_sub(1, Ordering::Relaxed);
            continue;
        }
        let sidecar = Arc::clone(&sidecar);
        let clients = Arc::clone(&clients);
        let notifications = Arc::clone(&notifications);
        thread::spawn(move || {
            let _ = connection(socket, &sidecar, &notifications);
            clients.fetch_sub(1, Ordering::Relaxed);
        });
    }
    Ok(())
}

fn send(socket: &mut UnixStream, value: &Value) -> io::Result<()> {
    // Workspace builds can enable serde_json's preserve_order through GPUI.
    // Keep the established wire order independent of Cargo feature unification.
    let mut value = value.clone();
    value.sort_all_objects();
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    if bytes.len() > OUTPUT_LIMIT {
        return Err(io::Error::other("response exceeds bound"));
    }
    timed_write(socket, &bytes, Instant::now() + Duration::from_secs(2))
}

fn connection(
    mut socket: UnixStream,
    sidecar: &Mutex<Sidecar>,
    notifications: &Arc<Notifications>,
) -> io::Result<()> {
    local_socket::require_same_user(&socket)?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let request = timed_frame(
        &mut socket,
        REQUEST_LIMIT,
        Instant::now() + Duration::from_secs(5),
    )
    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(io::Error::other));
    let request = match request {
        Ok(request) => request,
        Err(_) => {
            return send(
                &mut socket,
                &json!({"ok":false,"error":{"code":"invalid_request","message":"Expected one bounded JSON line."}}),
            );
        }
    };
    // Register before reading state so a concurrent change cannot be missed.
    let threads = request["op"] == "watch_threads" || request["op"] == "watch_threads_queue";
    let mut subscription = if request["op"] == "watch" {
        request["thread_id"]
            .as_str()
            .map(|id| notifications.subscribe(id.to_owned()))
            .transpose()?
    } else if threads || request["op"] == "watch_build_output" {
        Some(notifications.subscribe(notifications::THREADS.to_owned())?)
    } else if request["op"] == "workspace_subscribe" {
        Some(notifications.subscribe(notifications::WORKSPACE.to_owned())?)
    } else {
        None
    };
    let reply = sidecar
        .lock()
        .map_err(|_| io::Error::other("sidecar failed"))?
        .request(&request)?;
    send(&mut socket, &reply)?;
    if reply["ok"] != true {
        return Ok(());
    }
    let Some(subscription) = subscription.as_mut() else {
        return Ok(());
    };
    if threads {
        return watch_threads(socket, sidecar, &request, subscription, &reply);
    }
    if request["op"] == "workspace_subscribe" {
        return watch_workspace(socket, sidecar, subscription, &reply);
    }
    if request["op"] == "watch_build_output" {
        let mut previous = reply["result"].clone();
        while subscription.wait(&socket)? {
            let reply = sidecar
                .lock()
                .map_err(|_| io::Error::other("sidecar failed"))?
                .request(&request)?;
            if reply["ok"] != true {
                return send(&mut socket, &reply);
            }
            if reply["result"] != previous {
                send(
                    &mut socket,
                    &json!({"event":"build_output","build_output":reply["result"]}),
                )?;
                previous = reply["result"].clone();
            }
        }
        return Ok(());
    }
    let mut previous = reply["result"]["thread"].clone();
    while subscription.wait(&socket)? {
        let reply = sidecar
            .lock()
            .map_err(|_| io::Error::other("sidecar failed"))?
            .request(&request)?;
        if reply["ok"] != true {
            return send(&mut socket, &reply);
        }
        let snapshot = &reply["result"]["thread"];
        if *snapshot != previous {
            send(&mut socket, &json!({"event":"snapshot","thread":snapshot}))?;
            previous = snapshot.clone();
        }
    }
    Ok(())
}

/// Sends only the summaries that changed since the last frame this client received.
fn watch_threads(
    mut socket: UnixStream,
    sidecar: &Mutex<Sidecar>,
    request: &Value,
    subscription: &mut notifications::Subscription,
    initial: &Value,
) -> io::Result<()> {
    let mut previous = initial["result"]["threads"].clone();
    while subscription.wait(&socket)? {
        let reply = sidecar
            .lock()
            .map_err(|_| io::Error::other("sidecar failed"))?
            .request(request)?;
        if reply["ok"] != true {
            return send(&mut socket, &reply);
        }
        let current = &reply["result"]["threads"];
        if let Some(event) = thread_changes(&previous, current) {
            send(&mut socket, &event)?;
        }
        previous = current.clone();
    }
    Ok(())
}

/// Sends the workspace again whenever its revision or status changes, numbered from 1 after
/// the snapshot. A client that cannot keep up is disconnected by the write
/// timeout and subscribes again for a fresh snapshot.
fn watch_workspace(
    mut socket: UnixStream,
    sidecar: &Mutex<Sidecar>,
    subscription: &mut notifications::Subscription,
    initial: &Value,
) -> io::Result<()> {
    let view = json!({"op": "workspace_view"});
    let mut previous = initial["result"].clone();
    let mut seq = 0u64;
    while subscription.wait(&socket)? {
        let reply = sidecar
            .lock()
            .map_err(|_| io::Error::other("sidecar failed"))?
            .request(&view)?;
        if reply["ok"] != true {
            return send(&mut socket, &reply);
        }
        if let Some(event) = workspace_event(&previous, &reply["result"], seq + 1) {
            seq += 1;
            previous = reply["result"].clone();
            send(&mut socket, &event)?;
        }
    }
    Ok(())
}

/// A workspace event when the revision or status changed.
fn workspace_event(previous: &Value, current: &Value, seq: u64) -> Option<Value> {
    (current["revision"] != previous["revision"] || current["status"] != previous["status"])
        .then(|| json!({"event": "workspace", "seq": seq, "workspace": current}))
}

/// The summaries that are new or different, in the runtime's order, and the IDs that left the list.
fn thread_changes(previous: &Value, current: &Value) -> Option<Value> {
    let list = |value: &Value| value.as_array().cloned().unwrap_or_default();
    let (before, after) = (list(previous), list(current));
    let changed: Vec<&Value> = after
        .iter()
        .filter(|thread| !before.iter().any(|old| old == *thread))
        .collect();
    let removed: Vec<&Value> = before
        .iter()
        .map(|thread| &thread["id"])
        .filter(|id| !after.iter().any(|thread| thread["id"] == **id))
        .collect();
    (!changed.is_empty() || !removed.is_empty())
        .then(|| json!({"event":"threads","threads":changed,"removed":removed}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_objects_keep_sorted_keys_in_every_build() {
        let (mut input, mut output) = UnixStream::pair().unwrap();
        let value: Value = serde_json::from_str(r#"{"z":[{"z":0,"a":1}],"a":true}"#).unwrap();
        send(&mut output, &value).unwrap();
        assert_eq!(
            timed_frame(&mut input, 100, Instant::now() + Duration::from_secs(1)).unwrap(),
            b"{\"a\":true,\"z\":[{\"a\":1,\"z\":0}]}\n"
        );
    }

    #[test]
    fn thread_events_carry_only_changes_and_removals() {
        let thread = |id: &str, status: &str| json!({"id":id,"status":status});
        let before = json!([
            thread("a", "idle"),
            thread("b", "idle"),
            thread("c", "idle")
        ]);
        assert_eq!(thread_changes(&before, &before), None);
        let after = json!([
            thread("d", "idle"),
            thread("b", "running"),
            thread("a", "idle")
        ]);
        assert_eq!(
            thread_changes(&before, &after),
            Some(json!({
                "event":"threads",
                "threads":[thread("d", "idle"), thread("b", "running")],
                "removed":["c"],
            }))
        );
        // Order alone is not a change: clients sort by `updated_at`.
        let reordered = json!([
            thread("c", "idle"),
            thread("b", "idle"),
            thread("a", "idle")
        ]);
        assert_eq!(thread_changes(&before, &reordered), None);
    }

    #[test]
    fn workspace_events_follow_the_revision_and_are_numbered() {
        let view = |revision: u64| json!({"revision": revision, "projects": []});
        assert_eq!(workspace_event(&view(3), &view(3), 1), None);
        assert_eq!(
            workspace_event(&view(3), &view(4), 1),
            Some(json!({"event": "workspace", "seq": 1, "workspace": view(4)}))
        );
    }

    #[test]
    fn workspace_event_reports_status_only_changes() {
        let before = json!({"revision": 3, "status": {"/repo": 0}});
        let after = json!({"revision": 3, "status": {"/repo": 1}});
        assert_eq!(
            workspace_event(&before, &after, 2),
            Some(json!({"event": "workspace", "seq": 2, "workspace": after}))
        );
    }

    #[test]
    fn drip_input_cannot_extend_absolute_deadline() {
        let (mut input, mut output) = UnixStream::pair().unwrap();
        thread::spawn(move || {
            for _ in 0..5 {
                if output.write_all(b"x").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(40));
            }
        });
        let start = Instant::now();
        assert!(timed_frame(&mut input, 100, start + Duration::from_millis(90)).is_err());
        assert!(start.elapsed() < Duration::from_millis(180));
    }

    #[test]
    fn bounded_frames_require_newline() {
        assert!(frame(&mut &b"{}"[..], 3).is_err());
        assert!(frame(&mut &b"1234\n"[..], 4).is_err());
        assert_eq!(frame(&mut &b"{}\nnext"[..], 3).unwrap(), b"{}\n");
    }
}
