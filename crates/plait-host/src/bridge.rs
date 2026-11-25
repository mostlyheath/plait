//! Multiplex an authenticated stdio connection onto owner-only local sockets.
use plait_protocol::frame::{Frame, Kind, REPLY_LIMIT};
use plait_transport::frame::FrameIo;
use plait_transport::local_socket;
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

type Streams = Arc<Mutex<HashMap<u32, UnixStream>>>;
type Output<W> = Arc<Mutex<W>>;

struct DeadlineReader<R> {
    inner: R,
    started: Option<Instant>,
    timeout: Duration,
}
impl<R: Read + AsRawFd> Read for DeadlineReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(start) = self.started {
            loop {
                let remaining = (start + self.timeout)
                    .checked_duration_since(Instant::now())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::TimedOut, "remote frame timed out")
                    })?;
                let mut fd = libc::pollfd {
                    fd: self.inner.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let result = unsafe {
                    libc::poll(
                        &mut fd,
                        1,
                        remaining.as_millis().clamp(1, i32::MAX as u128) as i32,
                    )
                };
                if result > 0 {
                    break;
                }
                if result == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "remote frame timed out",
                    ));
                }
                if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        let count = self.inner.read(bytes)?;
        if count > 0 {
            self.started.get_or_insert_with(Instant::now);
        }
        Ok(count)
    }
}

fn send<W: Write>(output: &Output<W>, stream: u32, kind: Kind, payload: Vec<u8>) -> io::Result<()> {
    let mut output = output
        .lock()
        .map_err(|_| io::Error::other("bridge writer failed"))?;
    Frame {
        stream,
        kind,
        payload,
    }
    .write_to(&mut *output)?;
    output.flush()
}

fn forward<W: Write + Send + 'static>(
    id: u32,
    socket: UnixStream,
    streams: Streams,
    output: Output<W>,
) {
    let mut reader = BufReader::new(socket);
    loop {
        let mut line = Vec::new();
        let result = reader
            .by_ref()
            .take(REPLY_LIMIT as u64 + 1)
            .read_until(b'\n', &mut line);
        match result {
            Ok(0) => {
                let _ = send(&output, id, Kind::Close, vec![]);
                break;
            }
            Ok(_) if line.len() <= REPLY_LIMIT && line.last() == Some(&b'\n') => {
                if send(&output, id, Kind::Frame, line).is_err() {
                    break;
                }
            }
            _ => {
                let _ = send(
                    &output,
                    id,
                    Kind::Error,
                    b"Host connection failed.".to_vec(),
                );
                break;
            }
        }
    }
    if let Ok(mut streams) = streams.lock() {
        streams.remove(&id);
    }
}

pub fn run<R: Read + AsRawFd, W: Write + Send + 'static>(
    input: R,
    output: W,
    store: &Path,
) -> io::Result<()> {
    let mut input = DeadlineReader {
        inner: input,
        started: None,
        timeout: Duration::from_secs(15),
    };
    let output = Arc::new(Mutex::new(output));
    let streams: Streams = Arc::new(Mutex::new(HashMap::new()));
    let result = (|| loop {
        input.started = None;
        let Some(frame) = Frame::read_from(&mut input)? else {
            return Ok(());
        };
        match frame.kind {
            Kind::Ping => send(&output, 0, Kind::Ping, vec![])?,
            Kind::Close => {
                if let Some(socket) = streams
                    .lock()
                    .map_err(|_| io::Error::other("bridge streams failed"))?
                    .get(&frame.stream)
                {
                    let _ = socket.shutdown(Shutdown::Both);
                }
            }
            Kind::Open => {
                let mut active = streams
                    .lock()
                    .map_err(|_| io::Error::other("bridge streams failed"))?;
                if active.contains_key(&frame.stream) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate stream",
                    ));
                }
                if active.len() >= 32 {
                    send(
                        &output,
                        frame.stream,
                        Kind::Error,
                        b"Too many open streams.".to_vec(),
                    )?;
                    continue;
                }
                let mut socket = match local_socket::connect(&store.join("host.sock")) {
                    Ok(socket) => socket,
                    Err(_) => {
                        send(
                            &output,
                            frame.stream,
                            Kind::Error,
                            b"Cannot connect to the host.".to_vec(),
                        )?;
                        continue;
                    }
                };
                socket.set_write_timeout(Some(Duration::from_secs(5)))?;
                if socket.write_all(&frame.payload).is_err() {
                    send(
                        &output,
                        frame.stream,
                        Kind::Error,
                        b"Host connection failed.".to_vec(),
                    )?;
                    continue;
                }
                active.insert(frame.stream, socket.try_clone()?);
                let streams = Arc::clone(&streams);
                let output = Arc::clone(&output);
                thread::spawn(move || forward(frame.stream, socket, streams, output));
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected remote frame",
                ));
            }
        }
    })();
    if let Ok(mut streams) = streams.lock() {
        for (_, socket) in streams.drain() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::mpsc,
    };

    #[test]
    fn bridge_forwards_reply_and_closes_on_input_eof() {
        let dir = std::env::temp_dir().join(format!(
            "plait-bridge-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 3];
            socket.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"{}\n");
            socket.write_all(b"ok\n").unwrap();
        });
        let (mut peer, input) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let output = input.try_clone().unwrap();
        let store = dir.clone();
        let (done, finished) = mpsc::channel();
        thread::spawn(move || done.send(run(input, output, &store)).unwrap());
        Frame {
            stream: 1,
            kind: Kind::Open,
            payload: b"{}\n".to_vec(),
        }
        .write_to(&mut peer)
        .unwrap();
        assert_eq!(
            Frame::read_from(&mut peer).unwrap().unwrap().payload,
            b"ok\n"
        );
        assert_eq!(
            Frame::read_from(&mut peer).unwrap().unwrap().kind,
            Kind::Close
        );
        peer.shutdown(Shutdown::Write).unwrap();
        finished
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        server.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
