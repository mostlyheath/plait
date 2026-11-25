//! In-app alerts for other threads while Plait is in front.
//!
//! While the window is active, a thread that finishes, fails, or needs the
//! person is shown as a toast instead of a system notification. The stack is
//! small and bounded: a newer alert for the same thread replaces the older
//! one, and alerts expire on their own. Time is passed in so tests need no
//! clock.

use std::time::{Duration, Instant};

/// At most this many toasts are shown; the oldest goes first.
pub const LIMIT: usize = 3;
/// How long a toast stays without being dismissed.
pub const LIFETIME: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub thread: String,
    pub title: String,
    pub detail: String,
    pub shown: Instant,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    /// Oldest first, as they stack upward from the bottom edge.
    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// Shows an alert for `thread`, replacing any earlier one for it.
    pub fn push(&mut self, thread: &str, title: String, detail: String, now: Instant) {
        self.items.retain(|toast| toast.thread != thread);
        self.items.push(Toast {
            thread: thread.to_owned(),
            title,
            detail,
            shown: now,
        });
        if self.items.len() > LIMIT {
            self.items.remove(0);
        }
    }

    /// Removes the toast for `thread`: it was dismissed, opened, or is now on screen.
    pub fn dismiss(&mut self, thread: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|toast| toast.thread != thread);
        self.items.len() != before
    }

    /// Drops toasts older than their lifetime. Returns whether any went.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.items.len();
        self.items
            .retain(|toast| now.saturating_duration_since(toast.shown) < LIFETIME);
        self.items.len() != before
    }

    /// Keeps only toasts for threads that still exist, such as after a reconnect.
    pub fn retain_threads(&mut self, exists: impl Fn(&str) -> bool) {
        self.items.retain(|toast| exists(&toast.thread));
    }

    /// When the next toast expires, so one timer can wake for it.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.items.iter().map(|toast| toast.shown + LIFETIME).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(toasts: &mut Toasts, thread: &str, now: Instant) {
        toasts.push(thread, format!("{thread} title"), "Finished".into(), now);
    }

    #[test]
    fn a_newer_alert_replaces_the_threads_older_one() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        push(&mut toasts, "a", now);
        push(&mut toasts, "b", now);
        toasts.push(
            "a",
            "a title".into(),
            "Needs approval".into(),
            now + Duration::from_secs(1),
        );
        let threads: Vec<_> = toasts
            .items()
            .iter()
            .map(|toast| toast.thread.as_str())
            .collect();
        assert_eq!(threads, ["b", "a"]);
        assert_eq!(toasts.items()[1].detail, "Needs approval");
    }

    #[test]
    fn the_stack_is_bounded_and_drops_the_oldest() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        for thread in ["a", "b", "c", "d"] {
            push(&mut toasts, thread, now);
        }
        let threads: Vec<_> = toasts
            .items()
            .iter()
            .map(|toast| toast.thread.as_str())
            .collect();
        assert_eq!(threads, ["b", "c", "d"]);
    }

    #[test]
    fn toasts_expire_after_their_lifetime() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        push(&mut toasts, "a", now);
        push(&mut toasts, "b", now + Duration::from_secs(5));
        assert_eq!(toasts.next_expiry(), Some(now + LIFETIME));
        assert!(!toasts.expire(now + LIFETIME - Duration::from_millis(1)));
        assert!(toasts.expire(now + LIFETIME));
        assert_eq!(toasts.items().len(), 1);
        assert_eq!(
            toasts.next_expiry(),
            Some(now + Duration::from_secs(5) + LIFETIME)
        );
        assert!(toasts.expire(now + Duration::from_secs(60)));
        assert_eq!(toasts.next_expiry(), None);
    }

    #[test]
    fn dismissing_and_vanished_threads_remove_toasts() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        push(&mut toasts, "a", now);
        push(&mut toasts, "b", now);
        assert!(toasts.dismiss("a"));
        assert!(!toasts.dismiss("a"));
        toasts.retain_threads(|thread| thread != "b");
        assert!(toasts.items().is_empty());
    }
}
