//! Host-owned thread build-cache accounting and idle-cache cleanup.
use super::{Client, Error};
pub use plait_protocol::host::build_cache::{ClearReply, Request, Status};

const MAX_SAFE_INT: u64 = 9_007_199_254_740_991;

fn invalid() -> Error {
    Error::Protocol("Invalid build cache reply. Update the host and desktop together.".into())
}

pub fn verified_status(status: Status) -> Result<Status, Error> {
    if status.schema != 1
        || status.idle_bytes > status.total_bytes
        || status.active_count > status.thread_count
        || [
            status.total_bytes,
            status.idle_bytes,
            status.thread_count,
            status.active_count,
            status.cap_bytes,
        ]
        .into_iter()
        .any(|n| n > MAX_SAFE_INT)
    {
        return Err(invalid());
    }
    Ok(status)
}

pub fn verified_clear(reply: ClearReply) -> Result<ClearReply, Error> {
    verified_status(reply.status())?;
    if [
        reply.cleared_threads,
        reply.cleared_bytes,
        reply.skipped_active,
    ]
    .into_iter()
    .any(|n| n > MAX_SAFE_INT)
    {
        return Err(invalid());
    }
    Ok(reply)
}

pub fn status(client: &Client) -> Result<Option<Status>, Error> {
    match client.send(&Request::BuildCacheStatus) {
        Ok(value) => verified_status(value).map(Some),
        Err(Error::Rejected(e)) if e.code == "invalid_request" => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn clear(client: &Client) -> Result<ClearReply, Error> {
    verified_clear(client.send(&Request::ClearBuildCaches)?)
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn plural(count: u64, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

/// Settings → Machines state for the connected host's build caches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub epoch: Option<u64>,
    pub status: Option<Status>,
    pub busy: bool,
    pub clearing: bool,
    pub confirm_clear: bool,
    /// Feedback from the last load or clear, and whether it was an error.
    pub note: Option<(String, bool)>,
}

impl State {
    pub fn sync(&mut self, epoch: u64) {
        if self.epoch != Some(epoch) {
            *self = Self {
                epoch: Some(epoch),
                ..Self::default()
            };
        }
    }

    pub fn complete_status(&mut self, epoch: u64, result: Result<Status, Error>) {
        if self.epoch != Some(epoch) {
            return;
        }
        self.busy = false;
        match result.and_then(verified_status) {
            Ok(value) => {
                if value.measuring {
                    self.confirm_clear = false;
                }
                self.status = Some(value);
                if self.note.as_ref().is_some_and(|(_, failed)| *failed) {
                    self.note = None;
                }
            }
            Err(error) => {
                self.note = Some((format!("Could not read build caches. {error}"), true));
            }
        }
    }

    pub fn complete_clear(&mut self, epoch: u64, result: Result<ClearReply, Error>) {
        if self.epoch != Some(epoch) {
            return;
        }
        self.clearing = false;
        self.confirm_clear = false;
        match result.and_then(verified_clear) {
            Ok(reply) => {
                self.status = Some(reply.status());
                let summary = if reply.cleared_threads == 0 && reply.skipped_active > 0 {
                    format!(
                        "No idle build caches to clear; kept {} active {}.",
                        reply.skipped_active,
                        plural(reply.skipped_active, "thread", "threads")
                    )
                } else if reply.cleared_threads == 0 {
                    "No idle build caches to clear.".into()
                } else if reply.skipped_active > 0 {
                    format!(
                        "Cleared {} of idle build caches; kept {} active {}.",
                        format_bytes(reply.cleared_bytes),
                        reply.skipped_active,
                        plural(reply.skipped_active, "thread", "threads")
                    )
                } else {
                    format!(
                        "Cleared {} of idle build caches.",
                        format_bytes(reply.cleared_bytes)
                    )
                };
                self.note = Some((summary, false));
            }
            Err(error) => {
                self.note = Some((format!("Could not clear build caches. {error}"), true));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_shapes_and_bounds_are_checked() {
        assert_eq!(
            serde_json::to_value(Request::BuildCacheStatus).unwrap(),
            json!({"op":"build_cache_status"})
        );
        assert_eq!(
            serde_json::to_value(Request::ClearBuildCaches).unwrap(),
            json!({"op":"clear_build_caches"})
        );
        let ok_status = json!({
            "schema": 1,
            "total_bytes": 4_200_000_000u64,
            "idle_bytes": 3_000_000_000u64,
            "thread_count": 3,
            "active_count": 1,
            "cap_bytes": 20_000_000_000u64
        });
        assert!(verified_status(serde_json::from_value(ok_status.clone()).unwrap()).is_ok());
        for (field, value) in [
            ("schema", json!(2)),
            ("idle_bytes", json!(5_000_000_000u64)),
            ("active_count", json!(4)),
            ("total_bytes", json!(MAX_SAFE_INT + 1)),
        ] {
            let mut bad = ok_status.clone();
            bad[field] = value;
            assert!(verified_status(serde_json::from_value(bad).unwrap()).is_err());
        }
        let mut extra = ok_status;
        extra["extra"] = json!(true);
        let parsed: Status = serde_json::from_value(extra).unwrap();
        assert!(!parsed.measuring);
        let clear_with_extra = json!({
            "schema": 1,
            "cleared_threads": 1,
            "cleared_bytes": 100,
            "skipped_active": 0,
            "total_bytes": 0,
            "idle_bytes": 0,
            "thread_count": 0,
            "active_count": 0,
            "cap_bytes": 20_000_000_000u64,
            "extra": "future"
        });
        let parsed_clear: ClearReply = serde_json::from_value(clear_with_extra).unwrap();
        assert!(!parsed_clear.measuring);
        assert!(verified_clear(parsed_clear).is_ok());
    }

    #[test]
    fn state_rejects_stale_epochs_and_reports_clear_outcome() {
        let mut state = State::default();
        state.sync(1);
        state.busy = true;
        state.complete_status(
            1,
            Ok(Status {
                schema: 1,
                total_bytes: 4_200_000_000,
                idle_bytes: 4_200_000_000,
                thread_count: 2,
                active_count: 0,
                cap_bytes: 20_000_000_000,
                measuring: false,
            }),
        );
        assert!(!state.busy);
        assert_eq!(state.status.as_ref().unwrap().total_bytes, 4_200_000_000);

        state.sync(2);
        assert!(state.status.is_none());
        state.clearing = true;
        state.confirm_clear = true;
        state.complete_clear(
            1,
            Ok(ClearReply {
                schema: 1,
                cleared_threads: 1,
                cleared_bytes: 1_000_000_000,
                skipped_active: 0,
                total_bytes: 0,
                idle_bytes: 0,
                thread_count: 0,
                active_count: 0,
                cap_bytes: 20_000_000_000,
                measuring: false,
            }),
        );
        assert!(state.clearing);
        assert!(state.status.is_none());

        state.complete_clear(
            2,
            Ok(ClearReply {
                schema: 1,
                cleared_threads: 1,
                cleared_bytes: 1_200_000_000,
                skipped_active: 1,
                total_bytes: 3_000_000_000,
                idle_bytes: 0,
                thread_count: 1,
                active_count: 1,
                cap_bytes: 20_000_000_000,
                measuring: false,
            }),
        );
        assert!(!state.clearing);
        assert!(!state.confirm_clear);
        assert_eq!(state.status.as_ref().unwrap().total_bytes, 3_000_000_000);
        let (note, failed) = state.note.as_ref().unwrap();
        assert!(!failed);
        assert!(note.contains("1.2 GB"));
        assert!(note.contains("1 active thread"));
    }

    #[test]
    fn bytes_format_for_settings() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1_500), "1.5 KB");
        assert_eq!(format_bytes(45_200_000), "45.2 MB");
        assert_eq!(format_bytes(4_200_000_000), "4.2 GB");
    }
}
