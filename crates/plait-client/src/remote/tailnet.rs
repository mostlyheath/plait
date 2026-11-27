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
    let link = socket_link(socket)?;
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
    // A refusing host closes the connection, so the link is dead before its
    // streams fail. The local stream's error kind varies: macOS can report
    // EPROTOTYPE for a write that races the teardown.
    result.map_err(|e| {
        let refused = link.is_dead()
            || matches!(
                e.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::NotConnected
            );
        Error::Transport(if refused {
            "The host refused this device. On that machine run: plait-host tailnet allow <your login>".into()
        } else {
            format!("Tailnet startup failed: {e}")
        })
    })?;
    Ok(link)
}

fn socket_link(socket: TcpStream) -> Result<Arc<Link>, Error> {
    let writer = socket
        .try_clone()
        .map_err(|e| Error::Transport(e.to_string()))?;
    let closer = writer
        .try_clone()
        .map_err(|e| Error::Transport(e.to_string()))?;
    let link = Arc::new(Link::new(socket, writer, Settings::default()));
    link.on_dead(move || {
        let _ = closer.shutdown(std::net::Shutdown::Both);
    });
    Ok(link)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropping_idle_link_closes_peer_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let link =
            socket_link(TcpStream::connect(listener.local_addr().unwrap()).unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        drop(link);
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }
}
