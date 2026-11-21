//! Multiplex local socket streams over a framed byte link.
use super::frame::{Frame, Kind, REQUEST_LIMIT};
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const STREAM_LIMIT: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub ping: Duration,
    pub deadline: Duration,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(15),
            deadline: Duration::from_secs(45),
        }
    }
}

struct State {
    dead: bool,
    next: u32,
    streams: HashMap<u32, UnixStream>,
    last_read: Instant,
}
struct Shared {
    state: Mutex<State>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Run once when the link dies, such as to end an ssh child, which also
    /// unblocks a write stuck on a peer that stopped reading.
    on_dead: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Shared {
    fn send(&self, stream: u32, kind: Kind, payload: Vec<u8>) -> io::Result<()> {
        let result = (|| {
            let mut writer = self
                .writer
                .lock()
                .map_err(|_| io::Error::other("link writer failed"))?;
            Frame {
                stream,
                kind,
                payload,
            }
            .write_to(&mut *writer)?;
            writer.flush()
        })();
        if result.is_err() {
            self.die();
        }
        result
    }
    fn die(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.dead = true;
            for (_, socket) in state.streams.drain() {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        if let Some(hook) = self.on_dead.lock().ok().and_then(|mut hook| hook.take()) {
            hook();
        }
    }
    /// A Ping only when the writer is free: a write blocked on a peer that
    /// stopped reading must not also stop the deadline from being checked.
    fn ping(&self) {
        let Ok(mut writer) = self.writer.try_lock() else {
            return;
        };
        let frame = Frame {
            stream: 0,
            kind: Kind::Ping,
            payload: Vec::new(),
        };
        if frame
            .write_to(&mut *writer)
            .and_then(|()| writer.flush())
            .is_err()
        {
            drop(writer);
            self.die();
        }
    }
    fn close(&self, id: u32, notify: bool) {
        let removed = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.streams.remove(&id));
        if let Some(socket) = removed {
            let _ = socket.shutdown(Shutdown::Both);
            if notify {
                let _ = self.send(id, Kind::Close, vec![]);
            }
        }
    }
}

pub struct Link {
    shared: Arc<Shared>,
}
impl Link {
    pub fn new(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        settings: Settings,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                dead: false,
                next: 1,
                streams: HashMap::new(),
                last_read: Instant::now(),
            }),
            writer: Mutex::new(Box::new(writer)),
            on_dead: Mutex::new(None),
        });
        let input = Arc::clone(&shared);
        thread::spawn(move || read_frames(reader, input));
        // Two threads: the watchdog only reads clocks, so a ping write blocked
        // on a peer that stopped reading can never stop the deadline.
        let tick = settings
            .ping
            .min(settings.deadline)
            .max(Duration::from_millis(1));
        let watchdog = Arc::downgrade(&shared);
        thread::spawn(move || {
            loop {
                thread::sleep(tick);
                let Some(shared) = watchdog.upgrade() else {
                    break;
                };
                let expired = match shared.state.lock() {
                    Ok(state) => state.dead || state.last_read.elapsed() >= settings.deadline,
                    Err(_) => true,
                };
                if expired {
                    shared.die();
                    break;
                }
            }
        });
        let pinger = Arc::downgrade(&shared);
        thread::spawn(move || {
            loop {
                thread::sleep(settings.ping.max(Duration::from_millis(1)));
                let Some(shared) = pinger.upgrade() else {
                    break;
                };
                if shared.state.lock().map_or(true, |state| state.dead) {
                    break;
                }
                shared.ping();
            }
        });
        Self { shared }
    }
    /// Runs `hook` once when the link dies, or at once if it already has.
    pub fn on_dead(&self, hook: impl FnOnce() + Send + 'static) {
        let dead = self.shared.state.lock().map_or(true, |state| state.dead);
        if dead {
            hook();
        } else if let Ok(mut slot) = self.shared.on_dead.lock() {
            *slot = Some(Box::new(hook));
        }
    }
    pub fn is_dead(&self) -> bool {
        self.shared.state.lock().map_or(true, |state| state.dead)
    }
    pub fn open(&self) -> io::Result<UnixStream> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| io::Error::other("link state failed"))?;
        if state.dead {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "link is dead",
            ));
        }
        if state.streams.len() >= STREAM_LIMIT {
            return Err(io::Error::other("too many open streams"));
        }
        let (near, far) = UnixStream::pair()?;
        let id = state.next;
        state.next = state.next.checked_add(1).unwrap_or(1);
        while state.streams.contains_key(&state.next) {
            state.next = state.next.checked_add(1).unwrap_or(1);
        }
        state.streams.insert(id, far.try_clone()?);
        drop(state);
        let shared = Arc::clone(&self.shared);
        thread::spawn(move || pump(id, far, shared));
        Ok(near)
    }
}

struct ObservedReader<R> {
    inner: R,
    shared: Arc<Shared>,
}
impl<R: Read> Read for ObservedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buf)?;
        if count > 0
            && let Ok(mut state) = self.shared.state.lock()
        {
            state.last_read = Instant::now();
        }
        Ok(count)
    }
}

fn read_frames(reader: impl Read, shared: Arc<Shared>) {
    let mut reader = ObservedReader {
        inner: reader,
        shared: Arc::clone(&shared),
    };
    while let Ok(Some(frame)) = Frame::read_from(&mut reader) {
        let socket = match shared.state.lock() {
            Ok(mut state) => {
                if state.dead {
                    break;
                }
                state.last_read = Instant::now();
                state
                    .streams
                    .get(&frame.stream)
                    .and_then(|s| s.try_clone().ok())
            }
            Err(_) => break,
        };
        match frame.kind {
            Kind::Ping => {}
            Kind::Frame => {
                if let Some(mut socket) = socket {
                    let _ = socket.set_write_timeout(Some(Duration::from_secs(5)));
                    if socket.write_all(&frame.payload).is_err() {
                        shared.close(frame.stream, true);
                    }
                }
            }
            Kind::Close | Kind::Error => shared.close(frame.stream, false),
            Kind::Open => break,
        }
    }
    shared.die();
}

fn pump(id: u32, socket: UnixStream, shared: Arc<Shared>) {
    let mut reader = BufReader::new(socket);
    let mut request = Vec::new();
    let valid = reader
        .by_ref()
        .take(REQUEST_LIMIT as u64 + 1)
        .read_until(b'\n', &mut request)
        .is_ok()
        && request.len() <= REQUEST_LIMIT
        && request.last() == Some(&b'\n');
    if !valid {
        shared.close(id, false);
        return;
    }
    if shared.send(id, Kind::Open, request).is_err() {
        return;
    }
    let mut extra = [0; 1];
    let _ = reader.read(&mut extra);
    shared.close(id, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Client;
    use std::sync::mpsc;

    fn pair() -> (Arc<Link>, UnixStream) {
        let (client, peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        (
            Arc::new(Link::new(
                client.try_clone().unwrap(),
                client,
                Settings {
                    ping: Duration::from_millis(50),
                    deadline: Duration::from_millis(300),
                },
            )),
            peer,
        )
    }
    fn next(peer: &mut UnixStream) -> Frame {
        loop {
            let frame = Frame::read_from(peer).unwrap().unwrap();
            if frame.kind != Kind::Ping {
                return frame;
            }
        }
    }
    fn send(peer: &mut UnixStream, id: u32, kind: Kind, bytes: &[u8]) {
        Frame {
            stream: id,
            kind,
            payload: bytes.to_vec(),
        }
        .write_to(peer)
        .unwrap();
    }
    #[test]
    fn remote_client_request() {
        let (link, mut peer) = pair();
        let server = thread::spawn(move || {
            let frame = next(&mut peer);
            assert_eq!(frame.kind, Kind::Open);
            assert!(frame.payload.ends_with(b"\n"));
            send(
                &mut peer,
                frame.stream,
                Kind::Frame,
                b"{\"ok\":true,\"result\":42,\"error\":null}\n",
            );
        });
        let value: u32 = Client::over_link(link)
            .send(&serde_json::json!({"op":"test"}))
            .unwrap();
        assert_eq!(value, 42);
        server.join().unwrap();
    }
    #[test]
    fn remote_multiplex_and_cancel() {
        let (link, mut peer) = pair();
        let mut streams: Vec<_> = (0..4).map(|_| link.open().unwrap()).collect();
        for (index, stream) in streams.iter_mut().enumerate() {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream.write_all(format!("{index}\n").as_bytes()).unwrap();
        }
        let mut ids = Vec::new();
        for _ in 0..4 {
            let frame = next(&mut peer);
            let index: usize = std::str::from_utf8(&frame.payload)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            ids.push((index, frame.stream));
        }
        for (_, id) in &ids {
            send(&mut peer, *id, Kind::Frame, b"event\n");
        }
        for stream in &mut streams {
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            assert_eq!(line, "event\n");
        }
        let watch_id = ids.iter().find(|(index, _)| *index == 0).unwrap().1;
        send(&mut peer, watch_id, Kind::Frame, b"second\n");
        let mut line = String::new();
        BufReader::new(&mut streams[0])
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "second\n");
        drop(streams);
        let mut closed = Vec::new();
        while closed.len() < 4 {
            let frame = next(&mut peer);
            if frame.kind == Kind::Close {
                closed.push(frame.stream);
            }
        }
        assert!(closed.contains(&watch_id));
    }
    #[test]
    fn remote_request_limit_and_cap() {
        let (link, mut peer) = pair();
        let mut stream = link.open().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let _ = stream.write_all(&vec![b'x'; REQUEST_LIMIT + 1]);
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
        let mut streams: Vec<_> = (0..STREAM_LIMIT).map(|_| link.open().unwrap()).collect();
        assert!(link.open().is_err());
        for stream in &mut streams {
            stream.write_all(b"{}\n").unwrap();
        }
        for _ in 0..STREAM_LIMIT {
            assert_eq!(next(&mut peer).kind, Kind::Open);
        }
    }
    /// A writer that never returns, as a peer that stopped reading would be.
    struct Stuck;
    impl Write for Stuck {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            loop {
                thread::sleep(Duration::from_secs(3600));
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn remote_deadline_fires_while_a_write_is_stuck() {
        let (reader, _peer) = UnixStream::pair().unwrap();
        let link = Link::new(
            reader,
            Stuck,
            Settings {
                ping: Duration::from_millis(20),
                deadline: Duration::from_millis(150),
            },
        );
        let (tx, rx) = std::sync::mpsc::channel();
        link.on_dead(move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the deadline fired and ran the hook");
        assert!(link.is_dead());
    }

    #[test]
    fn remote_deadline_closes_streams_and_rejects_opens() {
        let (link, _peer) = pair();
        let mut stream = link.open().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(b"{}\n").unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut byte = [0];
            tx.send(stream.read(&mut byte)).unwrap();
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(), 0);
        assert_eq!(
            link.open().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }
}
