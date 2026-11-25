#![cfg(unix)]
#![allow(dead_code)]
#[path = "../../plait-host/src/bridge.rs"]
mod bridge;
#[path = "../../plait-host/src/tailnet.rs"]
mod tailnet;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
/// The client's link and this bridge, end to end, against an owner-only
/// host socket: concurrent requests, and a watch ended by Close.
#[test]
fn link_and_bridge_carry_requests_and_end_watches() {
    use plait_client::remote::link::{Link, Settings};
    let dir = std::env::temp_dir().join(format!("plait-link-bridge-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("host.sock");
    let listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let (closed_tx, closed_rx) = mpsc::channel();
    // A scripted host: "watch" streams two events and reports when the
    // client hangs up; anything else gets one reply naming the request.
    thread::spawn(move || {
        for socket in listener.incoming() {
            let mut socket = socket.unwrap();
            let closed = closed_tx.clone();
            thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                if line.contains("watch") {
                    socket.write_all(b"{\"event\":1}\n{\"event\":2}\n").unwrap();
                    let mut rest = [0u8; 1];
                    let _ = socket.read(&mut rest);
                    let _ = closed.send(());
                } else {
                    socket
                        .write_all(format!("reply:{}", line).as_bytes())
                        .unwrap();
                }
            });
        }
    });
    let (client_side, bridge_side) = UnixStream::pair().unwrap();
    let bridge_out = bridge_side.try_clone().unwrap();
    let store = dir.clone();
    thread::spawn(move || bridge::run(bridge_side, bridge_out, &store));
    let link = Arc::new(Link::new(
        client_side.try_clone().unwrap(),
        client_side,
        Settings::default(),
    ));
    let ask = |text: &str| {
        let mut stream = link.open().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(format!("{text}\n").as_bytes()).unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        reply
    };
    let replies: Vec<String> = thread::scope(|scope| {
        let handles: Vec<_> = (0..3)
            .map(|n| scope.spawn(move || ask(&format!("{{\"op\":\"r{n}\"}}"))))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (n, reply) in replies.iter().enumerate() {
        assert_eq!(reply, &format!("reply:{{\"op\":\"r{n}\"}}\n"));
    }
    let mut watch = link.open().unwrap();
    watch
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    watch.write_all(b"{\"op\":\"watch\"}\n").unwrap();
    let mut events = BufReader::new(watch.try_clone().unwrap());
    for expected in ["{\"event\":1}\n", "{\"event\":2}\n"] {
        let mut line = String::new();
        events.read_line(&mut line).unwrap();
        assert_eq!(line, expected);
    }
    // Ending the watch reaches the host as a hang-up, not as a request.
    watch.shutdown(Shutdown::Both).unwrap();
    closed_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the host saw the watch end");
    let _ = fs::remove_dir_all(&dir);
}
#[test]
fn loopback_handler_accepts_and_refuses() {
    let root = std::env::temp_dir().join(format!("plait-tailnet-client-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    // The bridge reaches only an owner-only store, as a real one is.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let owner = UnixListener::bind(root.join("host.sock")).unwrap();
    fs::set_permissions(root.join("host.sock"), fs::Permissions::from_mode(0o600)).unwrap();
    thread::spawn(move || {
        let (mut socket, _) = owner.accept().unwrap();
        let mut request = [0; 16];
        let _ = socket.read(&mut request).unwrap();
        socket.write_all(b"{\"ok\":true}\n").unwrap();
    });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = root.clone();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        tailnet::handle(stream, &store, true);
    });
    assert!(plait_client::remote::tailnet::connect("127.0.0.1", port).is_ok());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = root.clone();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        tailnet::handle(stream, &store, false);
    });
    let error = match plait_client::remote::tailnet::connect("127.0.0.1", port) {
        Ok(_) => panic!("unexpected connection"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("refused this device"),
        "unexpected error: {error}"
    );
    let _ = fs::remove_dir_all(root);
}
