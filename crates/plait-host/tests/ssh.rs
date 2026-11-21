#![cfg(unix)]
use plait_core::remote::ssh::{SshDestination, SshOptions, connect};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

/// These tests write a script and then run it. Another test thread forking
/// while one is written can briefly hold it open for writing, and Linux then
/// refuses to run it (ETXTBSY), so the tests here run one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("plait-ssh-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn script(&self, body: &str) -> PathBuf {
        let path = self.0.join("ssh");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn options(program: PathBuf) -> SshOptions {
    SshOptions {
        program: program.into(),
        ..Default::default()
    }
}
fn destination() -> SshDestination {
    SshDestination::new("example").unwrap()
}
fn wait_for(mut predicate: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < until, "timed out");
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn ssh_early_exit_reports_stderr() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("early");
    let script = fixture.script("echo 'Permission denied (publickey).' >&2; exit 255");
    let error = match connect(&destination(), options(script)) {
        Ok(_) => panic!("connected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Permission denied (publickey)."));
}
#[test]
fn ssh_bridge_lifecycle() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("bridge");
    let socket = fixture.0.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert!(line.ends_with('\n'));
            stream.write_all(b"{\"ok\":true}\n").unwrap();
        }
    });
    let pidfile = fixture.0.join("pid");
    let script = fixture.script(&format!("echo $$ > '{}'; while [ \"$1\" != plait-host ]; do [ $# -gt 0 ] || exit 1; shift; done; [ \"$2\" = bridge ] || exit 1; exec '{}' bridge --store '{}'", pidfile.display(), env!("CARGO_BIN_EXE_plait-host"), fixture.0.display()));
    let link = connect(&destination(), options(script)).unwrap();
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut stream = link.open().unwrap();
    stream.write_all(b"{}\n").unwrap();
    let mut reply = String::new();
    BufReader::new(&mut stream).read_line(&mut reply).unwrap();
    assert_eq!(reply, "{\"ok\":true}\n");
    server.join().unwrap();
    drop(stream);
    drop(link);
    wait_for(|| unsafe { libc::kill(pid, 0) } == -1);
}
#[test]
fn ssh_child_death_closes_streams() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("death");
    let socket = fixture.0.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        stream.write_all(b"{}\n").unwrap();
    });
    let pidfile = fixture.0.join("pid");
    let script = fixture.script(&format!(
        "echo $$ > '{}'; exec '{}' bridge --store '{}'",
        pidfile.display(),
        env!("CARGO_BIN_EXE_plait-host"),
        fixture.0.display()
    ));
    let link = connect(&destination(), options(script)).unwrap();
    server.join().unwrap();
    let mut stream = link.open().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let pid: i32 = fs::read_to_string(pidfile).unwrap().trim().parse().unwrap();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    wait_for(|| link.is_dead());
    assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    wait_for(|| unsafe { libc::kill(pid, 0) } == -1);
}
