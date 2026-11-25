//! The live thread list: an initial listing, then only what changed. Needs a version 3 host.

use std::{
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use super::{Client, Error, Frames, Request, ThreadSummary, decode};

pub struct ThreadWatch {
    frames: Frames,
    first: bool,
    initial_deadline: Instant,
    threads: Vec<ThreadSummary>,
}

pub use plait_protocol::host::threads::{Event, Listing};

impl Client {
    pub fn watch_threads(&self, archived: bool) -> Result<ThreadWatch, Error> {
        self.watch_threads_queue(archived, false)
    }
    pub fn watch_threads_queue(&self, archived: bool, queue: bool) -> Result<ThreadWatch, Error> {
        Ok(ThreadWatch {
            frames: self.connect(&if queue {
                Request::WatchThreadsQueue { archived }
            } else {
                Request::WatchThreads {
                    archived,
                    protocol_version: 3,
                }
            })?,
            first: true,
            initial_deadline: Instant::now() + Duration::from_secs(15),
            threads: Vec::new(),
        })
    }
}

impl ThreadWatch {
    #[cfg(test)]
    pub(super) fn reading(frames: Frames) -> Self {
        Self {
            frames,
            first: true,
            initial_deadline: Instant::now() + Duration::from_secs(15),
            threads: Vec::new(),
        }
    }

    pub(super) fn socket(&self) -> std::io::Result<UnixStream> {
        self.frames.reader.get_ref().try_clone()
    }

    /// Blocks for the next change and returns the whole list, most recently active first.
    pub fn next_list(&mut self) -> Result<Vec<ThreadSummary>, Error> {
        let line = self
            .frames
            .next(self.first.then_some(self.initial_deadline))?;
        if self.first {
            let listing: Listing = decode(&line)?;
            self.first = false;
            self.threads = listing.threads;
        } else {
            let event: Event = serde_json::from_slice(&line).map_err(|_| {
                Error::Protocol("Invalid host event; reconnect to load current state.".into())
            })?;
            if event.event != "threads" {
                return Err(Error::Protocol("Unsupported host event.".into()));
            }
            merge(&mut self.threads, event.threads, &event.removed);
        }
        if self.threads.iter().any(|thread| {
            thread
                .checkout_queue
                .as_ref()
                .is_some_and(|queue| !queue.valid())
        }) {
            return Err(Error::Protocol("Invalid checkout queue summary.".into()));
        }
        Ok(self.threads.clone())
    }
}

/// Applies one change event. Events carry no order, so the list is kept by activity.
fn merge(threads: &mut Vec<ThreadSummary>, changed: Vec<ThreadSummary>, removed: &[String]) {
    threads.retain(|thread| {
        !removed.contains(&thread.id) && !changed.iter().any(|next| next.id == thread.id)
    });
    // New and changed threads lead equal timestamps, as the host lists them.
    threads.splice(0..0, changed);
    threads.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufReader, Write},
        os::unix::net::UnixStream,
    };

    fn summary(id: &str, updated_at: u64, status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "checkout": "/repo", "model": {"provider":"p","model_id":"m"},
            "status": status, "title": id, "created_at": 1, "updated_at": updated_at,
            "archived": false, "pending_approval": false
        })
    }

    fn watch() -> (ThreadWatch, UnixStream) {
        let (socket, peer) = UnixStream::pair().unwrap();
        let watch = ThreadWatch {
            frames: Frames {
                reader: BufReader::new(socket),
                pending: Vec::new(),
                partial_since: None,
            },
            first: true,
            initial_deadline: Instant::now() + Duration::from_secs(15),
            threads: Vec::new(),
        };
        (watch, peer)
    }

    fn ids(threads: &[ThreadSummary]) -> Vec<&str> {
        threads.iter().map(|thread| thread.id.as_str()).collect()
    }

    #[test]
    fn events_merge_into_the_list_by_activity() {
        let (mut watch, mut peer) = watch();
        let initial = serde_json::json!({"ok":true,"result":{"threads":[
            summary("b", 20, "idle"), summary("a", 10, "idle")]}});
        let changed = serde_json::json!({"event":"threads","removed":[],"threads":[
            summary("a", 30, "running"), summary("c", 30, "idle")]});
        let removed = serde_json::json!({"event":"threads","removed":["b","missing"],"threads":[]});
        peer.write_all(format!("{initial}\n{changed}\n{removed}\n").as_bytes())
            .unwrap();
        assert_eq!(ids(&watch.next_list().unwrap()), ["b", "a"]);
        let list = watch.next_list().unwrap();
        assert_eq!(ids(&list), ["a", "c", "b"]);
        assert_eq!(list[0].status, crate::host::ThreadStatus::Running);
        assert_eq!(list[0].pending_approval, Some(false));
        assert_eq!(ids(&watch.next_list().unwrap()), ["a", "c"]);
    }

    #[test]
    fn unknown_events_and_fields_end_the_watch() {
        for bad in [
            serde_json::json!({"event":"snapshot","threads":[],"removed":[]}),
            serde_json::json!({"event":"threads","threads":[]}),
            serde_json::json!({"event":"threads","threads":[],"removed":[],"extra":1}),
        ] {
            let (mut watch, mut peer) = watch();
            watch.first = false;
            peer.write_all(format!("{bad}\n").as_bytes()).unwrap();
            assert!(matches!(watch.next_list(), Err(Error::Protocol(_))));
        }
        let (mut watch, mut peer) = watch();
        peer.write_all(b"{\"ok\":false,\"error\":{\"code\":\"invalid_request\",\"message\":\"Unknown operation or field.\"}}\n")
            .unwrap();
        assert!(matches!(watch.next_list(), Err(Error::Rejected(_))));
    }

    #[test]
    fn the_async_reader_delivers_the_latest_merged_list() {
        use std::{
            future::Future,
            sync::Arc,
            task::{Context, Poll, Wake, Waker},
        };
        struct WakeThread(std::thread::Thread);
        impl Wake for WakeThread {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let (mut watch, mut peer) = watch();
        let socket = watch.socket().unwrap();
        let mut lists = crate::host::ThreadLists::spawn(socket, move || watch.next_list()).unwrap();
        let initial =
            serde_json::json!({"ok":true,"result":{"threads":[summary("a", 10, "idle")]}});
        let event = serde_json::json!({"event":"threads","removed":[],"threads":[summary("b", 20, "idle")]});
        peer.write_all(format!("{initial}\n{event}\n").as_bytes())
            .unwrap();
        let waker = Waker::from(Arc::new(WakeThread(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let list = {
                let mut next = std::pin::pin!(lists.next());
                loop {
                    match next.as_mut().poll(&mut cx) {
                        Poll::Ready(value) => break value.unwrap().unwrap(),
                        Poll::Pending => {
                            assert!(Instant::now() < deadline, "thread list did not arrive");
                            std::thread::park_timeout(Duration::from_millis(10));
                        }
                    }
                }
            };
            if list.len() == 2 {
                assert_eq!(ids(&list), ["b", "a"]);
                break;
            }
        }
        // Dropping the receiver closes the subscription.
        drop(lists);
        use std::io::Read;
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }
}
