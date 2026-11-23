//! Direct transport over a Tailscale network.
use super::link::{Link, Settings};
use crate::{host::Error, machines::Transport};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

pub fn connect(host: &str, port: u16) -> Result<Arc<Link>, Error> {
    Transport::Tailnet {
        host: host.into(),
        port,
    }
    .validate()
    .map_err(Error::Transport)?;
    let host = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::Transport(format!("Could not resolve host: {e}")))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut socket = None;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(stream) = TcpStream::connect_timeout(&address, remaining) {
            socket = Some(stream);
            break;
        }
    }
    let socket =
        socket.ok_or_else(|| Error::Transport("Could not connect to tailnet host.".into()))?;
    let writer = socket
        .try_clone()
        .map_err(|e| Error::Transport(e.to_string()))?;
    let link = Arc::new(Link::new(socket, writer, Settings::default()));
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
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "no startup reply",
            ));
        }
        Ok(())
    })();
    result.map_err(|e| Error::Transport(if matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::NotConnected) { "The host refused this device. On that machine run: plait-host tailnet allow <your login>".into() } else { format!("Tailnet startup failed: {e}") }))?;
    Ok(link)
}
