//! Where a checkout's Git and files are read: on this Mac, or by the host that
//! owns a remote checkout. Both run the same `git` and `file_view` functions;
//! only a remote one crosses the link.
use super::*;
use crate::{
    file_view::{self, Opened, Stamp},
    git::{LineCount, log::GraphCommit},
    host::{Client, git as remote},
    lease::DirIdentity,
    ownership::CheckoutKey,
};
use std::path::Path;

#[derive(Clone)]
pub enum Backend {
    Local,
    Host(Client),
}

fn text(error: crate::host::Error) -> String {
    error.to_string()
}

fn identity(key: &CheckoutKey) -> DirIdentity {
    DirIdentity {
        dev: key.device,
        ino: key.inode,
    }
}

/// The checkout this read is for must still be the one last identified.
fn expected<'a>(key: Option<&'a CheckoutKey>, root: &Path) -> Result<&'a CheckoutKey, String> {
    let key = key.ok_or("Checkout identity is not verified.")?;
    if key.root != root {
        return Err("Checkout identity changed.".into());
    }
    Ok(key)
}

impl Backend {
    pub fn for_host(local: bool, client: Client) -> Self {
        if local {
            Self::Local
        } else {
            Self::Host(client)
        }
    }

    /// The file viewer's read. Locally the identity is checked here before and
    /// after; a host checks it on its own machine.
    pub fn file(
        &self,
        root: &Path,
        key: Option<&CheckoutKey>,
        path: &Path,
    ) -> Result<Opened, String> {
        let key = expected(key, root)?;
        match self {
            Self::Local => {
                key.verify()?;
                let opened = file_view::read(root, path)?;
                key.verify()?;
                Ok(opened)
            }
            Self::Host(client) => remote::file(client, root, Some(identity(key)), path)
                .map(|r| r.result)
                .map_err(text),
        }
    }

    pub fn file_stamp(
        &self,
        root: &Path,
        key: Option<&CheckoutKey>,
        path: &Path,
    ) -> Result<Stamp, String> {
        let key = expected(key, root)?;
        match self {
            Self::Local => {
                key.verify()?;
                file_view::stamp(root, path)
            }
            Self::Host(client) => remote::file_stamp(client, root, Some(identity(key)), path)
                .map(|r| r.result)
                .map_err(text),
        }
    }

    pub fn line_counts(&self, root: &Path, paths: &[PathBuf]) -> Result<Vec<LineCount>, String> {
        match self {
            Self::Local => git::line_counts(root, paths),
            Self::Host(client) => remote::line_counts(client, root, None, paths)
                .map(|r| r.result)
                .map_err(text),
        }
    }

    pub fn graph(&self, root: &Path, heads: &[String]) -> Result<(Vec<GraphCommit>, bool), String> {
        match self {
            Self::Local => git::log::graph(root, heads),
            Self::Host(client) => remote::graph(client, root, None, heads)
                .map(|r| r.result)
                .map_err(text),
        }
    }
}

impl RepositoryJob {
    /// Blocking. A host checks the checkout's identity before and after each
    /// read or write on its own machine; this Mac cannot stat a remote path.
    pub(super) fn run_remote(&self, client: &Client) -> RepositoryReply {
        let expected = self.expected.as_ref().map(identity);
        let reply = match &self.job {
            Job::Write { root, op } => match expected {
                Some(id) => Reply::Write(remote::write(client, root, id, op).map_err(text)),
                None => self
                    .job
                    .failure("Checkout identity is not verified.".into()),
            },
            Job::Status { path, .. } => return self.remote_status(client, path, expected),
            Job::Diff { key, snapshot, .. } => Reply::Diff(
                remote::diff(
                    client,
                    &snapshot.root,
                    expected,
                    &snapshot.changes[key.file],
                    key.mode,
                )
                .map(|r| Arc::new(r.result))
                .map_err(text),
            ),
            Job::Files { root, .. } => Reply::Files(
                remote::files(client, root, expected)
                    .map(|r| Arc::new(r.result))
                    .map_err(text),
            ),
            Job::Log {
                root, tip, skip, ..
            } => Reply::Log(
                remote::log(client, root, expected, tip.as_deref(), *skip, HISTORY_PAGE)
                    .map(|r| r.result)
                    .map_err(text),
            ),
            Job::Show { root, id, .. } => Reply::Show(
                remote::show(client, root, expected, id)
                    .map(|r| Arc::new(r.result))
                    .map_err(text),
            ),
            Job::Branches { root, .. } => Reply::Branches(
                remote::branches(client, root, expected)
                    .map(|r| Arc::new(r.result))
                    .map_err(text),
            ),
        };
        RepositoryReply {
            identity: None,
            reply,
        }
    }

    /// A remote checkout's identity is the one its host read with the status.
    fn remote_status(
        &self,
        client: &Client,
        path: &Path,
        expected: Option<DirIdentity>,
    ) -> RepositoryReply {
        let failed = |error: String| RepositoryReply {
            identity: None,
            reply: Reply::Status(Err(error)),
        };
        let read = match remote::status(client, path, expected) {
            Ok(read) => read,
            Err(error) => return failed(text(error)),
        };
        let key = CheckoutKey {
            host: self.host.clone(),
            root: read.result.root.clone(),
            device: read.identity.dev,
            inode: read.identity.ino,
        };
        if self.expected.as_ref().is_some_and(|old| old != &key) {
            return failed("Checkout identity changed.".into());
        }
        RepositoryReply {
            identity: Some(key),
            reply: Reply::Status(Ok(Arc::new(read.result))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::git::{Outcome, WireIdentity};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::mpsc,
    };

    /// A host that answers each request with `reply(request)`, and reports
    /// what it was asked.
    fn host(
        name: &str,
        reply: impl Fn(&serde_json::Value) -> String + Send + 'static,
    ) -> (Client, mpsc::Receiver<serde_json::Value>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("plait-backend-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (seen, asked) = mpsc::channel();
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let mut socket = socket.unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let answer = reply(&request);
                let _ = seen.send(request);
                writeln!(socket, "{answer}").unwrap();
            }
        });
        let client = Client::at_socket(path);
        (client, asked, dir)
    }

    fn page<T: serde::Serialize>(outcome: &Outcome<T>) -> String {
        let bytes = serde_json::to_vec(outcome).unwrap();
        format!(
            r#"{{"ok":true,"result":{{"total":{},"data":"{}","token":null}}}}"#,
            bytes.len(),
            STANDARD.encode(bytes)
        )
    }

    fn snapshot(root: &str) -> Snapshot {
        Snapshot {
            root: root.into(),
            branch: "main".into(),
            named: true,
            tracking: None,
            changes: Vec::new(),
            elapsed: std::time::Duration::ZERO,
        }
    }

    fn job(job: Job, expected: Option<CheckoutKey>) -> RepositoryJob {
        RepositoryJob {
            owner: CheckoutId::default(),
            expected,
            host: None,
            view: 0,
            job,
        }
    }

    #[test]
    fn a_remote_status_takes_its_identity_from_the_host() {
        let (client, asked, dir) = host("status", |_| {
            page(&Outcome::Ok {
                identity: WireIdentity {
                    dev: "5".into(),
                    ino: "6".into(),
                },
                result: snapshot("/srv/project"),
            })
        });
        let status = Job::Status {
            epoch: 0,
            path: "/srv/project".into(),
        };
        let reply = job(status, None).run_on(&Backend::Host(client.clone()));
        let key = reply.identity.unwrap();
        assert_eq!(
            (key.root.to_str(), key.device, key.inode),
            (Some("/srv/project"), 5, 6)
        );
        assert!(matches!(reply.reply, Reply::Status(Ok(s)) if s.root == Path::new("/srv/project")));
        let request = asked.recv().unwrap();
        assert_eq!(request["op"], "git_read");
        assert_eq!(request["read"], "status");
        assert!(request["identity"].is_null());

        // The same path answering with another identity is another checkout.
        let old = CheckoutKey {
            host: None,
            root: "/srv/project".into(),
            device: 5,
            inode: 7,
        };
        let status = Job::Status {
            epoch: 0,
            path: "/srv/project".into(),
        };
        let reply = job(status, Some(old)).run_on(&Backend::Host(client));
        assert!(reply.identity.is_none());
        assert!(matches!(reply.reply, Reply::Status(Err(e)) if e == "Checkout identity changed."));
        assert_eq!(asked.recv().unwrap()["identity"]["ino"], "7");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unverified_writes_and_mismatched_files_never_reach_the_host() {
        let (client, asked, dir) = host("refuse", |_| unreachable!("the host was asked"));
        let write = Job::Write {
            root: "/srv/project".into(),
            op: git::WriteOp::StageAll,
        };
        let reply = job(write, None).run_on(&Backend::Host(client.clone()));
        assert!(matches!(reply.reply, Reply::Write(Err(e)) if e.contains("not verified")));
        let key = CheckoutKey {
            host: None,
            root: "/srv/other".into(),
            device: 1,
            inode: 2,
        };
        let backend = Backend::Host(client);
        let error = backend
            .file(Path::new("/srv/project"), Some(&key), Path::new("a.rs"))
            .unwrap_err();
        assert_eq!(error, "Checkout identity changed.");
        assert!(
            backend
                .file(Path::new("/srv/project"), None, Path::new("a.rs"))
                .is_err()
        );
        assert!(asked.try_recv().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_remote_write_names_the_identity_last_read() {
        let (client, asked, dir) = host("write", |_| {
            page(&Outcome::Ok {
                identity: WireIdentity {
                    dev: "5".into(),
                    ino: "6".into(),
                },
                result: git::WriteOutcome {
                    summary: "Staged 1 file".into(),
                    commit: None,
                    backup: None,
                },
            })
        });
        let key = CheckoutKey {
            host: None,
            root: "/srv/project".into(),
            device: 5,
            inode: 6,
        };
        let write = Job::Write {
            root: "/srv/project".into(),
            op: git::WriteOp::Stage(vec!["a.rs".into()]),
        };
        let reply = job(write, Some(key)).run_on(&Backend::Host(client));
        assert!(matches!(reply.reply, Reply::Write(Ok(o)) if o.summary == "Staged 1 file"));
        let request = asked.recv().unwrap();
        assert_eq!(request["op"], "git_write");
        assert_eq!(
            request["identity"],
            serde_json::json!({"dev": "5", "ino": "6"})
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
