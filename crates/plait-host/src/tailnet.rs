//! Opt-in listener, authenticated by the local Tailscale daemon.
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
const SOCKET: &str = "/var/run/tailscale/tailscaled.sock";
const LIMIT: u64 = 256 * 1024;

struct LocalApi {
    socket: PathBuf,
}
impl LocalApi {
    fn get(&self, path: &str) -> io::Result<Value> {
        let mut stream = UnixStream::connect(&self.socket)
            .map_err(|e| io::Error::other(format!("cannot reach tailscaled LocalAPI: {e}")))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: local-tailscaled.sock\r\nConnection: close\r\n\r\n"
        )?;
        let mut data = Vec::new();
        stream.take(LIMIT + 1).read_to_end(&mut data)?;
        if data.len() as u64 > LIMIT {
            return Err(io::Error::other("tailscaled response too large"));
        }
        let split = data
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| io::Error::other("invalid tailscaled HTTP response"))?;
        let header = std::str::from_utf8(&data[..split]).map_err(io::Error::other)?;
        if !header
            .lines()
            .next()
            .is_some_and(|s| s.starts_with("HTTP/1.1 200 ") || s.starts_with("HTTP/1.0 200 "))
        {
            return Err(io::Error::other("tailscaled LocalAPI request failed"));
        }
        serde_json::from_slice(&data[split + 4..]).map_err(io::Error::other)
    }
}
fn safe_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            o[0] == 100 && (64..=127).contains(&o[1])
        }
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}
fn addresses(status: &Value) -> io::Result<Vec<IpAddr>> {
    let entries = status
        .pointer("/Self/TailscaleIPs")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("tailscaled status has no Tailscale addresses"))?;
    let mut ips = Vec::new();
    for entry in entries {
        let ip: IpAddr = entry
            .as_str()
            .ok_or_else(|| io::Error::other("invalid Tailscale address"))?
            .parse()
            .map_err(io::Error::other)?;
        if !safe_ip(ip) {
            return Err(io::Error::other(format!(
                "refusing non-Tailscale address {ip}"
            )));
        }
        ips.push(ip);
    }
    if ips.is_empty() {
        return Err(io::Error::other("tailscaled has no Tailscale addresses"));
    }
    Ok(ips)
}
fn allowlist(store: &Path) -> io::Result<Value> {
    match fs::read(store.join("tailnet-allow.json")) {
        Ok(data) if data.iter().all(u8::is_ascii_whitespace) => Ok(json!({})),
        Ok(data) => serde_json::from_slice(&data).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e),
    }
}
fn listed(list: &Value, key: &str, value: &str) -> bool {
    !value.is_empty()
        && list
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(value)))
}
/// Node names compare without a trailing dot and without case: whois gives
/// `laptop.tailnet.ts.net.` where a person writes `laptop.tailnet.ts.net`.
fn node_listed(list: &Value, node: &str) -> bool {
    let normal = |name: &str| name.trim_end_matches('.').to_ascii_lowercase();
    let node = normal(node);
    !node.is_empty()
        && list
            .get("nodes")
            .and_then(Value::as_array)
            .is_some_and(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .any(|v| normal(v) == node)
            })
}
fn authorize(api: &LocalApi, peer: SocketAddr, store: &Path) -> bool {
    let result = api.get(&format!("/localapi/v0/whois?addr={peer}"));
    let info = match result {
        Ok(v) => v,
        Err(_) => {
            eprintln!("tailnet refused login <unknown> node <unknown>: whois unavailable");
            return false;
        }
    };
    let login = info
        .pointer("/UserProfile/LoginName")
        .and_then(Value::as_str)
        .unwrap_or("");
    let node = info
        .pointer("/Node/Name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let untagged = match info.pointer("/Node/Tags") {
        None | Some(Value::Null) => true,
        Some(Value::Array(tags)) => tags.is_empty(),
        _ => false,
    };
    // An untagged node is allowed by its owner's login. A tagged node has no
    // personal login (Tailscale reports `tagged-devices`), so only listing the
    // node itself allows it.
    let allowed = allowlist(store)
        .is_ok_and(|list| (untagged && listed(&list, "logins", login)) || node_listed(&list, node));
    if !allowed {
        eprintln!("tailnet refused login {:?} node {:?}", login, node);
    }
    allowed
}
fn handle(stream: TcpStream, store: &Path, allowed: bool) {
    if allowed && let Ok(output) = stream.try_clone() {
        let _ = crate::bridge::run(stream, output, store);
    }
}
pub fn serve(store: &Path, port: u16) -> io::Result<()> {
    let api = LocalApi {
        socket: SOCKET.into(),
    };
    let ips = addresses(&api.get("/localapi/v0/status")?)?;
    let list = allowlist(store)?;
    if list
        .get("logins")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        eprintln!("No tailnet logins allowed. Run: plait-host tailnet allow --me");
    }
    let listeners: Vec<_> = ips
        .into_iter()
        .map(|ip| TcpListener::bind(SocketAddr::new(ip, port)))
        .collect::<io::Result<_>>()?;
    let count = Arc::new(AtomicUsize::new(0));
    for listener in listeners {
        let count = Arc::clone(&count);
        let store = store.to_owned();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if count
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                        (n < 8).then_some(n + 1)
                    })
                    .is_err()
                {
                    continue;
                }
                let count = Arc::clone(&count);
                let store = store.clone();
                thread::spawn(move || {
                    let api = LocalApi {
                        socket: SOCKET.into(),
                    };
                    let allowed = stream
                        .peer_addr()
                        .is_ok_and(|peer| authorize(&api, peer, &store));
                    handle(stream, &store, allowed);
                    count.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
    }
    loop {
        thread::park();
    }
}
pub fn add(store: &Path, kind: &str, value: &str) -> io::Result<()> {
    add_with_api(
        store,
        kind,
        value,
        &LocalApi {
            socket: SOCKET.into(),
        },
    )
}
fn add_with_api(store: &Path, kind: &str, value: &str, api: &LocalApi) -> io::Result<()> {
    let value = if value == "--me" && kind == "allow" {
        let status = api.get("/localapi/v0/status")?;
        let id = status
            .pointer("/Self/UserID")
            .and_then(Value::as_i64)
            .ok_or_else(|| io::Error::other("tailscaled status has no self user"))?;
        status
            .pointer(&format!("/User/{id}/LoginName"))
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("tailscaled status has no self login"))?
            .to_owned()
    } else {
        value.to_owned()
    };
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(io::Error::other("invalid allowlist entry"));
    }
    let key = match kind {
        "allow" => "logins",
        "allow-node" => "nodes",
        _ => return Err(io::Error::other("expected allow or allow-node")),
    };
    let mut list = allowlist(store)?;
    if !list.is_object() {
        return Err(io::Error::other("invalid allowlist"));
    }
    let entries = list
        .as_object_mut()
        .unwrap()
        .entry(key)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| io::Error::other("invalid allowlist entries"))?;
    if !entries.iter().any(|v| v.as_str() == Some(&value)) {
        entries.push(json!(value));
    }
    fs::create_dir_all(store)?;
    let path = store.join("tailnet-allow.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut file, &list).map_err(io::Error::other)?;
    file.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "plait-tailnet-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir_all(&path).unwrap();
        // The bridge reaches only an owner-only store, as a real one is.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn api_reply(root: &Path, body: Value) -> LocalApi {
        let path = root.join("api.sock");
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = s.read(&mut request);
            let body = body.to_string();
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        LocalApi { socket: path }
    }
    #[test]
    fn authorization_and_allowlist() {
        let root = root();
        let _ = fs::remove_file(root.join("tailnet-allow.json"));
        let peer = "100.64.0.1:1234".parse().unwrap();
        let info = json!({"UserProfile":{"LoginName":"owner"},"Node":{"Name":"laptop","Tags":[]}});
        assert!(!authorize(&api_reply(&root, info.clone()), peer, &root));
        add(&root, "allow", "owner").unwrap();
        assert_eq!(
            fs::metadata(root.join("tailnet-allow.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(authorize(&api_reply(&root, info.clone()), peer, &root));
        let wrong = json!({"UserProfile":{"LoginName":"other"},"Node":{"Name":"laptop"}});
        assert!(!authorize(&api_reply(&root, wrong), peer, &root));
        // A tagged node reports no personal login and is refused unless named.
        let tagged = json!({"UserProfile":{"LoginName":"tagged-devices"},"Node":{"Name":"ci.tailnet.ts.net.","Tags":["tag:server"]}});
        assert!(!authorize(&api_reply(&root, tagged.clone()), peer, &root));
        let owner_tagged = json!({"UserProfile":{"LoginName":"owner"},"Node":{"Name":"other.tailnet.ts.net.","Tags":["tag:x"]}});
        assert!(
            !authorize(&api_reply(&root, owner_tagged), peer, &root),
            "tagged needs naming"
        );
        add(&root, "allow-node", "CI.tailnet.ts.net").unwrap();
        assert!(
            authorize(&api_reply(&root, tagged), peer, &root),
            "named without the dot, any case"
        );
        assert!(!authorize(
            &LocalApi {
                socket: root.join("missing")
            },
            peer,
            &root
        ));
        fs::write(root.join("tailnet-allow.json"), "").unwrap();
        assert!(!authorize(&api_reply(&root, info), peer, &root));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn allow_me_uses_status_owner() {
        let root = root();
        let api = api_reply(
            &root,
            json!({"Self":{"UserID":42},"User":{"42":{"LoginName":"owner"}}}),
        );
        add_with_api(&root, "allow", "--me", &api).unwrap();
        assert!(listed(&allowlist(&root).unwrap(), "logins", "owner"));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn loopback_handler_accepts_and_refuses() {
        let root = root();
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
            handle(stream, &store, true);
        });
        assert!(plait_core::remote::tailnet::connect("127.0.0.1", port).is_ok());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let store = root.clone();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle(stream, &store, false);
        });
        let error = match plait_core::remote::tailnet::connect("127.0.0.1", port) {
            Ok(_) => panic!("unexpected connection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("refused this device"));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn rejects_non_tailnet_addresses() {
        assert!(
            addresses(&json!({"Self":{"TailscaleIPs":["100.64.0.1","fd7a:115c:a1e0::1"]}})).is_ok()
        );
        assert!(addresses(&json!({"Self":{"TailscaleIPs":["127.0.0.1"]}})).is_err());
    }
}
