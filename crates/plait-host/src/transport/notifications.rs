use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

/// The reserved name the runtime sends when any thread summary changed.
pub(super) const THREADS: &str = "#";
/// The workspace changed: projects, worktree ownership, or seen state.
pub(super) const WORKSPACE: &str = "%";

#[derive(Default)]
pub(super) struct Notifications {
    watchers: Mutex<HashMap<String, Vec<Weak<Wake>>>>,
}

pub(super) struct Subscription {
    id: String,
    owner: Arc<Notifications>,
    reader: UnixStream,
    wake: Arc<Wake>,
}

struct Wake {
    writer: UnixStream,
    pending: AtomicBool,
}

impl Wake {
    fn notify(&self) {
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        loop {
            match (&self.writer).write(&[1]) {
                Ok(_) => return,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => {
                    let _ = self.writer.shutdown(std::net::Shutdown::Both);
                    return;
                }
            }
        }
    }
}

impl Notifications {
    pub(super) fn subscribe(self: &Arc<Self>, id: String) -> io::Result<Subscription> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        let wake = Arc::new(Wake {
            writer,
            pending: AtomicBool::new(false),
        });
        self.watchers
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .push(Arc::downgrade(&wake));
        Ok(Subscription {
            id,
            owner: Arc::clone(self),
            reader,
            wake,
        })
    }

    fn notify(&self, id: &str) {
        let watchers = self.watchers.lock().unwrap();
        for (_, listeners) in watchers
            .iter()
            .filter(|(key, _)| id == "*" || key.as_str() == id)
        {
            for listener in listeners.iter().filter_map(Weak::upgrade) {
                // One pending wakeup is enough: the consumer reads the latest state.
                listener.notify();
            }
        }
    }
}

impl Subscription {
    pub(super) fn wait(&mut self, client: &UnixStream) -> io::Result<bool> {
        let mut descriptors = [
            libc::pollfd {
                fd: client.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        loop {
            let result =
                unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, -1) };
            if result >= 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        // The protocol has one request per connection. EOF or more input ends a watch.
        if descriptors[0].revents != 0 {
            return Ok(false);
        }
        let mut bytes = [0u8; 4096];
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) => return Err(io::Error::other("notification channel closed")),
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    // Clear before fetching state. Later changes must schedule another read.
                    self.wake.pending.store(false, Ordering::Release);
                    return Ok(true);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut watchers = self.owner.watchers.lock().unwrap();
        if let Some(listeners) = watchers.get_mut(&self.id) {
            listeners.retain(|listener| !listener.ptr_eq(&Arc::downgrade(&self.wake)));
            if listeners.is_empty() {
                watchers.remove(&self.id);
            }
        }
    }
}

pub(super) fn relay(reader: impl Read, notifications: &Notifications) -> io::Result<()> {
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = Vec::new();
        // IDs use the public protocol's 128-byte bound; '*' invalidates all watches
        // and '#' wakes thread-list watches.
        reader.by_ref().take(130).read_until(b'\n', &mut line)?;
        if line.last() != Some(&b'\n') || line.len() > 129 {
            return Err(io::Error::other("invalid notification frame"));
        }
        line.pop();
        if line.is_empty()
            || (line != b"*"
                && line != THREADS.as_bytes()
                && line != WORKSPACE.as_bytes()
                && !line
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(byte)))
        {
            return Err(io::Error::other("invalid notification ID"));
        }
        notifications.notify(std::str::from_utf8(&line).map_err(io::Error::other)?);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(subscription: &Subscription) -> bool {
        let mut descriptor = libc::pollfd {
            fd: subscription.reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut descriptor, 1, 0) > 0 }
    }

    #[test]
    fn routes_only_matching_ids_and_reclaims_closed_watches() {
        let notifications = Arc::new(Notifications::default());
        let first = notifications.subscribe("one".into()).unwrap();
        let second = notifications.subscribe("two".into()).unwrap();
        notifications.notify("one");
        assert!(pending(&first));
        assert!(!pending(&second));
        notifications.notify("*");
        assert!(pending(&second));
        drop(first);
        drop(second);
        assert!(notifications.watchers.lock().unwrap().is_empty());
    }

    #[test]
    fn slow_watches_coalesce_without_blocking_and_disconnect_wakes() {
        let notifications = Arc::new(Notifications::default());
        let mut subscription = notifications.subscribe("one".into()).unwrap();
        let (client, peer) = UnixStream::pair().unwrap();
        for _ in 0..10000 {
            notifications.notify("one");
        }
        assert!(subscription.wait(&client).unwrap());
        assert!(!pending(&subscription));
        drop(peer);
        assert!(!subscription.wait(&client).unwrap());
    }

    #[test]
    fn relay_preserves_combined_frames_and_rejects_invalid_input() {
        let notifications = Arc::new(Notifications::default());
        let first = notifications.subscribe("one".into()).unwrap();
        let second = notifications.subscribe("two".into()).unwrap();
        let threads = notifications.subscribe(THREADS.into()).unwrap();
        assert!(relay(&b"one\ntwo\n"[..], &notifications).is_err());
        assert!(pending(&first) && pending(&second) && !pending(&threads));
        assert!(relay(&b"#\n"[..], &notifications).is_err());
        assert!(pending(&threads));
        for invalid in [
            b"\n".to_vec(),
            b"bad/id\n".to_vec(),
            b"missing-newline".to_vec(),
            vec![b'x'; 130],
        ] {
            assert!(relay(invalid.as_slice(), &notifications).is_err());
        }
    }
}
