use std::{
    future::poll_fn,
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    task::{Poll, Waker},
};

use super::{Client, Error, Snapshot, ThreadSummary};

struct Inbox<T> {
    latest: Option<Result<T, Error>>,
    closed: bool,
    waker: Option<Waker>,
}

struct Delivery<T>(Mutex<Inbox<T>>);
impl<T> Default for Delivery<T> {
    fn default() -> Self {
        Self(Mutex::new(Inbox {
            latest: None,
            closed: false,
            waker: None,
        }))
    }
}
impl<T> Delivery<T> {
    fn publish(&self, value: Result<T, Error>) -> bool {
        let mut inbox = self.0.lock().unwrap();
        if inbox.closed {
            return false;
        }
        inbox.closed = value.is_err();
        let keep = !inbox.closed;
        // Each value replaces state. A slow UI needs the latest one, not an event backlog.
        inbox.latest = Some(value);
        let waker = inbox.waker.take();
        drop(inbox);
        if let Some(waker) = waker {
            waker.wake();
        }
        keep
    }
}

/// Dropping any cancellation handle closes only the subscription, never the run.
#[derive(Clone)]
pub struct CancelWatch(Arc<UnixStream>);
impl Drop for CancelWatch {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

/// The latest state of one subscription, read on a dedicated thread.
pub struct Updates<T> {
    delivery: Arc<Delivery<T>>,
    cancel: CancelWatch,
}
/// One thread's snapshots.
pub type Snapshots = Updates<Snapshot>;
/// The whole thread list, already merged from the host's change events.
pub type ThreadLists = Updates<Vec<ThreadSummary>>;

impl<T> Updates<T> {
    pub fn cancel_handle(&self) -> CancelWatch {
        self.cancel.clone()
    }

    pub async fn next(&mut self) -> Option<Result<T, Error>> {
        poll_fn(|cx| {
            let mut inbox = self.delivery.0.lock().unwrap();
            if let Some(value) = inbox.latest.take() {
                return Poll::Ready(Some(value));
            }
            if inbox.closed {
                return Poll::Ready(None);
            }
            inbox.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
}
impl<T> Drop for Updates<T> {
    fn drop(&mut self) {
        let mut inbox = self.delivery.0.lock().unwrap();
        inbox.closed = true;
        inbox.latest = None;
        inbox.waker = None;
        // The cancellation field then wakes the blocked reader so it can release its thread.
    }
}

impl Client {
    pub fn subscribe(&self, thread_id: String, protocol: u32) -> Result<Snapshots, Error> {
        Snapshots::start(self.watch(thread_id, protocol)?)
    }

    /// Live summaries of every thread. Needs a host that negotiated version 3.
    pub fn subscribe_threads(&self, archived: bool) -> Result<ThreadLists, Error> {
        self.subscribe_threads_queue(archived, false)
    }
    pub fn subscribe_threads_queue(
        &self,
        archived: bool,
        queue: bool,
    ) -> Result<ThreadLists, Error> {
        let mut watch = self.watch_threads_queue(archived, queue)?;
        let socket = watch.socket()?;
        Updates::spawn(socket, move || watch.next_list())
    }
}

impl<T: Send + 'static> Updates<T> {
    pub(super) fn spawn(
        socket: UnixStream,
        mut next: impl FnMut() -> Result<T, Error> + Send + 'static,
    ) -> Result<Self, Error> {
        let cancel = CancelWatch(Arc::new(socket));
        let delivery = Arc::new(Delivery::default());
        let writer = Arc::clone(&delivery);
        // An idle blocking read must not occupy GPUI's shared background worker pool.
        std::thread::Builder::new()
            .name("plait-host-watch".into())
            .spawn(move || while writer.publish(next()) {})?;
        Ok(Updates { delivery, cancel })
    }
}

impl Snapshots {
    fn start(mut watch: super::Watch) -> Result<Self, Error> {
        let socket = watch.frames.reader.get_ref().try_clone()?;
        Self::spawn(socket, move || watch.next_snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Frames, Watch};
    use std::{
        io::BufReader,
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn reader() -> (Watch, CancelWatch, UnixStream) {
        let (socket, peer) = UnixStream::pair().unwrap();
        let cancel = CancelWatch(Arc::new(socket.try_clone().unwrap()));
        let watch = Watch {
            frames: Frames {
                reader: BufReader::new(socket),
                pending: Vec::new(),
                partial_since: None,
            },
            thread_id: "thread".into(),
            identity: None,
            first: false,
            initial_deadline: Instant::now() + Duration::from_secs(15),
        };
        (watch, cancel, peer)
    }

    #[test]
    fn idle_read_stays_asleep_until_cancel_and_sends_no_stop_request() {
        use std::io::Read;
        let (mut watch, cancel, mut peer) = reader();
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || send.send(watch.next_snapshot()).unwrap());
        assert!(matches!(
            receive.recv_timeout(Duration::from_millis(1150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(cancel);
        assert!(
            receive
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        let mut bytes = [0; 64];
        assert_eq!(peer.read(&mut bytes).unwrap(), 0);
        reader.join().unwrap();
    }

    #[test]
    fn initial_snapshot_still_has_an_absolute_deadline() {
        let (mut watch, _cancel, _peer) = reader();
        watch.first = true;
        watch.initial_deadline = Instant::now() + Duration::from_millis(20);
        assert!(watch.next_snapshot().is_err());
    }

    #[test]
    fn async_delivery_wakes_once_and_receiver_drop_closes_the_socket() {
        use std::{
            future::Future,
            io::Read,
            sync::atomic::{AtomicUsize, Ordering},
            task::{Context, Wake},
        };
        struct Counter(AtomicUsize);
        impl Wake for Counter {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let (_watch, cancel, mut peer) = reader();
        let delivery = Arc::new(Delivery::default());
        let mut snapshots = Snapshots {
            delivery: Arc::clone(&delivery),
            cancel,
        };
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&counter));
        let mut cx = Context::from_waker(&waker);
        {
            let mut next = std::pin::pin!(snapshots.next());
            assert!(next.as_mut().poll(&mut cx).is_pending());
            assert!(!delivery.publish(Err(Error::Transport("closed".into()))));
            assert_eq!(counter.0.load(Ordering::SeqCst), 1);
            assert!(matches!(
                next.as_mut().poll(&mut cx),
                Poll::Ready(Some(Err(_)))
            ));
        }
        assert!(matches!(
            std::pin::pin!(snapshots.next()).as_mut().poll(&mut cx),
            Poll::Ready(None)
        ));
        drop(snapshots);
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn subscription_decodes_initial_and_combined_event_frames() {
        use std::{
            future::Future,
            io::{Read, Write},
            task::{Context, Wake},
        };
        struct WakeThread(std::thread::Thread);
        impl Wake for WakeThread {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let (mut watch, _cancel, mut peer) = reader();
        watch.first = true;
        let mut snapshots = Snapshots::start(watch).unwrap();
        let snapshot = serde_json::json!({
            "id":"thread", "checkout":"/repo", "model":{"provider":"p", "model_id":"m"},
            "status":"running", "messages":[], "live_text":"initial", "tools":[],
            "pending_approval":null, "error":null, "truncated":false
        });
        let reply = serde_json::json!({"ok":true,"result":{"thread":snapshot.clone()}});
        let mut latest = snapshot;
        latest["live_text"] = "latest".into();
        let event = serde_json::json!({"event":"snapshot","thread":latest});
        peer.write_all(format!("{reply}\n{event}\n").as_bytes())
            .unwrap();
        let waker = Waker::from(Arc::new(WakeThread(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = {
                let mut next = std::pin::pin!(snapshots.next());
                loop {
                    match next.as_mut().poll(&mut cx) {
                        Poll::Ready(value) => break value.unwrap().unwrap(),
                        Poll::Pending => {
                            assert!(Instant::now() < deadline, "subscription did not wake");
                            std::thread::park_timeout(Duration::from_millis(10));
                        }
                    }
                }
            };
            if result.live_text == "latest" {
                break;
            }
            assert_eq!(result.live_text, "initial");
        }
        drop(snapshots);
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn identity_violations_are_rejected_before_async_coalescing() {
        use std::io::Write;
        for field in ["id", "checkout"] {
            let (mut watch, _cancel, mut peer) = reader();
            watch.first = true;
            let snapshot = serde_json::json!({
                "id":"thread", "checkout":"/repo", "model":{"provider":"p", "model_id":"m"},
                "status":"running", "messages":[], "live_text":"", "tools":[],
                "pending_approval":null, "error":null, "truncated":false
            });
            let first = serde_json::json!({"ok":true,"result":{"thread":snapshot.clone()}});
            let mut changed = snapshot.clone();
            changed[field] = "/wrong".into();
            let bad = serde_json::json!({"event":"snapshot","thread":changed});
            let good = serde_json::json!({"event":"snapshot","thread":snapshot});
            peer.write_all(format!("{first}\n{bad}\n{good}\n").as_bytes())
                .unwrap();
            assert!(watch.next_snapshot().is_ok());
            assert!(matches!(watch.next_snapshot(), Err(Error::Protocol(_))));
        }
    }

    #[test]
    fn a_thread_may_change_model_between_runs_and_carry_version_three_fields() {
        use std::io::Write;
        let (mut watch, _cancel, mut peer) = reader();
        watch.first = true;
        let first = serde_json::json!({"ok":true,"result":{"thread":{
            "id":"thread", "checkout":"/repo", "model":{"provider":"p", "model_id":"m"},
            "status":"idle", "messages":[], "live_text":"", "tools":[],
            "pending_approval":null, "error":null, "truncated":false, "approval_mode":"ask",
            "title":"", "created_at":1, "updated_at":1, "archived":false, "thinking_level":"off",
            "queued":[], "usage":null, "live_reasoning":"", "retry":null
        }}});
        let next = serde_json::json!({"event":"snapshot","thread":{
            "id":"thread", "checkout":"/repo", "model":{"provider":"p", "model_id":"other"},
            "status":"running",
            "messages":[
                {"id":"request-a","role":"user","text":"Hi","reasoning":""},
                {"id":"2-0","role":"assistant","text":"","reasoning":"Plan"}],
            "live_text":"", "tools":[{"id":"t","name":"read","status":"running","input":"{}","output":"","after":"2-0"}],
            "pending_approval":null, "error":{"code":"turn_limit","message":"Paused"}, "truncated":false,
            "approval_mode":"auto", "title":"Hi", "created_at":1, "updated_at":2, "archived":true,
            "thinking_level":"xhigh",
            "queued":[{"id":"request-q","request_id":"q","text":"Next","mode":"follow_up","approval":"auto"},
                      {"id":"request-s","request_id":"s","text":"Now","mode":"steer","approval":"ask"}],
            "usage":{"input":1,"output":2,"cache_read":3,"cache_write":4,"reasoning":null,"total":10,
                     "context_used":10,"context_window":null},
            "live_reasoning":"Thinking", "retry":{"attempt":2,"at":99}
        }});
        peer.write_all(format!("{first}\n{next}\n").as_bytes())
            .unwrap();
        let initial = watch.next_snapshot().unwrap();
        assert_eq!(
            initial.thinking_level,
            Some(crate::host::ThinkingLevel::Off)
        );
        assert!(initial.queued.is_empty() && initial.usage.is_none() && initial.retry.is_none());
        let changed = watch.next_snapshot().unwrap();
        assert_eq!(changed.model.model_id, "other");
        assert_eq!(changed.title.as_deref(), Some("Hi"));
        assert_eq!(changed.archived, Some(true));
        assert_eq!(
            changed.thinking_level,
            Some(crate::host::ThinkingLevel::Xhigh)
        );
        assert_eq!(changed.messages[1].reasoning, "Plan");
        assert_eq!(changed.queued[1].mode, crate::host::QueueMode::Steer);
        assert_eq!(changed.queued[0].approval, crate::host::ApprovalMode::Auto);
        let usage = changed.usage.unwrap();
        assert_eq!(
            (usage.total, usage.reasoning, usage.context_window),
            (10, None, None)
        );
        assert_eq!(changed.retry.unwrap().attempt, 2);
        assert_eq!(changed.live_reasoning, "Thinking");
        let summary = crate::host::ThreadSummary::from(&initial);
        assert_eq!(
            (summary.title.as_deref(), summary.pending_approval),
            (Some(""), Some(false))
        );
    }

    #[test]
    fn slow_consumers_keep_one_latest_snapshot_and_observe_failure() {
        let delivery = Delivery::default();
        for index in 0..1000 {
            let snapshot: Snapshot = serde_json::from_value(serde_json::json!({
                "id":"thread", "checkout":"/repo", "model":{"provider":"p", "model_id":"m"},
                "status":"running", "messages":[], "live_text":index.to_string(), "tools":[],
                "pending_approval":null, "error":null, "truncated":false
            }))
            .unwrap();
            assert!(delivery.publish(Ok(snapshot)));
        }
        assert_eq!(
            delivery
                .0
                .lock()
                .unwrap()
                .latest
                .take()
                .unwrap()
                .unwrap()
                .live_text,
            "999"
        );
        assert!(!delivery.publish(Err(Error::Transport("disconnected".into()))));
        assert!(delivery.0.lock().unwrap().latest.take().unwrap().is_err());
        assert!(!delivery.publish(Err(Error::Transport("again".into()))));
        assert!(delivery.0.lock().unwrap().latest.is_none());
    }
}
