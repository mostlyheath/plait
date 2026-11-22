//! Additive checkout waiting. Older hosts keep their unchanged submission protocol.
use super::{Client, Error, HostError, Request};
use serde::Deserialize;

pub use plait_protocol::host::checkout_queue::{State, Summary};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    pub schema: u32,
    pub checkout_queue_v1: bool,
    pub limit: u32,
    pub payload_bytes: u64,
}
pub fn available(client: &Client) -> Result<bool, Error> {
    match client.request::<Info>(&Request::CheckoutQueueInfo {}) {
        Ok(info)
            if info.schema == 1
                && info.checkout_queue_v1
                && info.limit == 32
                && info.payload_bytes == 32 * 1024 * 1024 =>
        {
            Ok(true)
        }
        // The thread listing asks this first. A capability this build does not
        // know, or a refused probe, makes Wait unavailable rather than the list.
        Ok(_) | Err(Error::Rejected(_)) => Ok(false),
        Err(error) => Err(error),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiter {
    pub request_id: String,
    pub state: State,
    pub position: u32,
    pub error: Option<HostError>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub schema: u32,
    pub thread_id: String,
    pub checkout: std::path::PathBuf,
    pub waiters: Vec<Waiter>,
}
impl Reply {
    pub fn verified(&self, thread: &str, checkout: &std::path::Path) -> bool {
        let mut seen = std::collections::HashSet::new();
        self.schema == 1
            && self.thread_id == thread
            && self.checkout == checkout
            && super::worktree::path(&self.checkout)
            && self.waiters.len() <= 32
            && self.waiters.iter().all(|waiter| {
                super::worktree::bounded_id(&waiter.request_id)
                    && seen.insert(&waiter.request_id)
                    && match waiter.state {
                        State::Waiting | State::Held => {
                            (1..=32).contains(&waiter.position) && waiter.error.is_none()
                        }
                        State::Failed => waiter.position == 0 && waiter.error.is_some(),
                        State::Started | State::Cancelled => {
                            waiter.position == 0 && waiter.error.is_none()
                        }
                    }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn summary_limits_and_states_are_checked() {
        let good = json!({"request_id":"q1","state":"waiting","position":2,"count":1});
        assert!(
            serde_json::from_value::<Summary>(good.clone())
                .unwrap()
                .valid()
        );
        for (key, value) in [
            ("position", json!(0)),
            ("count", json!(33)),
            ("request_id", json!("../other")),
            ("state", json!("started")),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(serde_json::from_value::<Summary>(bad).is_err());
        }
        for (key, value) in [("state", json!("unknown")), ("extra", json!(true))] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(serde_json::from_value::<Summary>(bad).is_err());
        }
    }
    #[test]
    fn queue_receipts_keep_the_thread_checkout_and_terminal_reason() {
        let good = json!({"schema":1,"thread_id":"t","checkout":"/repo","waiters":[{"request_id":"q","state":"held","position":1,"error":null}]});
        let reply: Reply = serde_json::from_value(good.clone()).unwrap();
        assert!(reply.verified("t", std::path::Path::new("/repo")));
        assert!(!reply.verified("other", std::path::Path::new("/repo")));
        assert!(!reply.verified("t", std::path::Path::new("/other")));
        let mut duplicate = good.clone();
        duplicate["waiters"]
            .as_array_mut()
            .unwrap()
            .push(good["waiters"][0].clone());
        assert!(
            !serde_json::from_value::<Reply>(duplicate)
                .unwrap()
                .verified("t", std::path::Path::new("/repo"))
        );
        let mut bad = good;
        bad["waiters"][0]["state"] = json!("failed");
        assert!(
            !serde_json::from_value::<Reply>(bad)
                .unwrap()
                .verified("t", std::path::Path::new("/repo"))
        );
    }
    #[test]
    fn exact_queue_retry_survives_the_recovery_codec() {
        let mut submissions = crate::host_state::Submissions::default();
        let pending = submissions
            .begin_with(
                "t",
                "keep this image draft".into(),
                Some(super::super::ApprovalMode::Auto),
                None,
                Some(7),
            )
            .unwrap();
        submissions.wait_for_checkout("t", &pending.request_id);
        let pending = submissions.get("t", &pending.request_id).unwrap();
        let restored: crate::host_state::PendingSubmit =
            serde_json::from_slice(&serde_json::to_vec(pending).unwrap()).unwrap();
        assert_eq!(pending, &restored);
        let request = serde_json::to_value(restored.request("t")).unwrap();
        assert_eq!(request["op"], "submit_checkout");
        assert_eq!(request["request_id"], pending.request_id);
        assert_eq!(request["approval"], "auto");
        assert!(request.get("protocol_version").is_none());
    }
}
