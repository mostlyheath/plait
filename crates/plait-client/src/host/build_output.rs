//! Additive build-output policy and event-driven creation progress.
use super::{
    Client, Error, decode,
    watch::Updates,
    worktree::{bounded_id, identity, path},
};
use crate::build_output::{MAX_PATHS, Outcome, relative, store::policy::Policy};
use crate::git::worktree::Identity;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Worktree,
    Admission,
}
#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    BuildOutputInfo {
        checkout: PathBuf,
        /// Only the Refresh control sends this: refresh the seed whatever its age.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        refresh: bool,
    },
    SetBuildOutput {
        checkout: PathBuf,
        identity: Identity,
        revision: u64,
        paths: Vec<String>,
    },
    WatchBuildOutput {
        request_id: String,
        kind: Kind,
    },
    SkipBuildOutput {
        request_id: String,
        kind: Kind,
    },
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    pub schema: u32,
    pub worktree_build_output_v1: bool,
    pub policy: Policy,
    pub suggestions: Vec<String>,
    /// The published seed, when the host reports one.
    #[serde(default)]
    pub seed: Option<crate::build_output::store::SeedSummary>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Waiting,
    Cloning,
    Warm,
    Cold,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub schema: u32,
    pub worktree_build_output_v1: bool,
    pub request_id: String,
    pub thread_id: Option<String>,
    pub phase: Phase,
    pub files: u64,
    pub outcome: Option<Outcome>,
}
fn invalid() -> Error {
    Error::Protocol("Invalid build output reply. Refresh before trying again.".into())
}
fn paths(values: &[String]) -> bool {
    values.len() <= MAX_PATHS && values.iter().all(|s| relative(s).is_ok())
}
pub fn valid_outcome(outcome: &Outcome) -> bool {
    paths(&outcome.directories)
        && !outcome.reason.is_empty()
        && outcome.reason.len() <= 8192
        && outcome.counts.notes.len() <= 20
        && outcome.counts.notes.iter().all(|s| s.len() <= 8192)
        && [
            outcome.counts.files,
            outcome.counts.links,
            outcome.counts.skipped,
            outcome.counts.bytes,
            outcome.counts.elapsed_ms,
        ]
        .into_iter()
        .all(|n| n <= 9_007_199_254_740_991)
        && (!outcome.warm || !outcome.directories.is_empty())
}
impl Progress {
    pub fn verified(self, request: &str) -> Result<Self, Error> {
        if self.schema != 1
            || !self.worktree_build_output_v1
            || self.request_id != request
            || !bounded_id(request)
            || self.thread_id.as_ref().is_some_and(|s| !bounded_id(s))
            || self.files > 32_000_000
            || self
                .outcome
                .as_ref()
                .is_some_and(|o| !valid_outcome(o) || o.warm != (self.phase == Phase::Warm))
            || matches!(self.phase, Phase::Waiting | Phase::Cloning) && self.outcome.is_some()
            || self.phase == Phase::Warm && self.outcome.is_none()
        {
            return Err(invalid());
        }
        Ok(self)
    }
    pub fn done(&self) -> bool {
        matches!(self.phase, Phase::Warm | Phase::Cold)
    }
}
fn verified(info: Info) -> Result<Info, Error> {
    if info.schema != 1
        || !info.worktree_build_output_v1
        || info.policy.schema != 1
        || !path(&info.policy.project)
        || !identity(&info.policy.identity)
        || !paths(&info.policy.paths)
        || !paths(&info.suggestions)
        || info.policy.revision > 9_007_199_254_740_991
        || info.seed.as_ref().is_some_and(|seed| {
            [seed.bytes, seed.files, seed.verified_at]
                .into_iter()
                .any(|n| n > 9_007_199_254_740_991)
        })
    {
        return Err(invalid());
    }
    Ok(info)
}
pub fn info(client: &Client, checkout: PathBuf, refresh: bool) -> Result<Option<Info>, Error> {
    match client.send(&Request::BuildOutputInfo { checkout, refresh }) {
        Ok(info) => verified(info).map(Some),
        Err(Error::Rejected(e)) if e.code == "invalid_request" => Ok(None),
        Err(e) => Err(e),
    }
}
pub fn set(client: &Client, request: &Request) -> Result<Info, Error> {
    verified(client.send(request)?)
}
pub fn skip(client: &Client, request: String, kind: Kind) -> Result<Progress, Error> {
    client
        .send::<_, Progress>(&Request::SkipBuildOutput {
            request_id: request.clone(),
            kind,
        })?
        .verified(&request)
}
pub fn subscribe(client: &Client, request: String, kind: Kind) -> Result<Updates<Progress>, Error> {
    let bytes = serde_json::to_vec(&Request::WatchBuildOutput {
        request_id: request.clone(),
        kind,
    })
    .map_err(|_| invalid())?;
    let mut frames = client.send_encoded(&bytes)?;
    let socket = frames.reader.get_ref().try_clone()?;
    let mut first = true;
    let deadline = Instant::now() + Duration::from_secs(20);
    Updates::spawn(socket, move || {
        let line = frames.next(first.then_some(deadline))?;
        let state = if first {
            first = false;
            decode::<Progress>(&line)?
        } else {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Event {
                event: String,
                build_output: Progress,
            }
            let event: Event = serde_json::from_slice(&line).map_err(|_| invalid())?;
            if event.event != "build_output" {
                return Err(invalid());
            }
            event.build_output
        };
        state.verified(&request)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_the_refresh_control_asks_for_a_seed_refresh() {
        let request = |refresh| {
            serde_json::to_value(Request::BuildOutputInfo {
                checkout: "/repo".into(),
                refresh,
            })
            .unwrap()
        };
        assert_eq!(
            request(false),
            json!({"op":"build_output_info","checkout":"/repo"})
        );
        assert_eq!(
            request(true),
            json!({"op":"build_output_info","checkout":"/repo","refresh":true})
        );
    }
    #[test]
    fn build_output_replies_require_capability_identity_bounds_and_consistent_state() {
        let value = json!({"schema":1,"worktree_build_output_v1":true,"request_id":"creation","thread_id":null,"phase":"cloning","files":42,"outcome":null});
        assert!(
            serde_json::from_value::<Progress>(value.clone())
                .unwrap()
                .verified("creation")
                .is_ok()
        );
        for (field, bad) in [
            ("schema", json!(2)),
            ("worktree_build_output_v1", json!(false)),
            ("request_id", json!("other")),
            ("files", json!(u64::MAX)),
            ("phase", json!("warm")),
        ] {
            let mut changed = value.clone();
            changed[field] = bad;
            assert!(
                serde_json::from_value::<Progress>(changed)
                    .unwrap()
                    .verified("creation")
                    .is_err()
            );
        }
        let mut extra = value.clone();
        extra["unknown"] = json!(true);
        assert!(serde_json::from_value::<Progress>(extra).is_err());
        let mut info = json!({"schema":1,"worktree_build_output_v1":true,"policy":{"schema":1,"project":"/repo","identity":{"dev":"1","ino":"2"},"revision":1,"paths":["target"]},"suggestions":["runtime/node_modules"]});
        assert!(verified(serde_json::from_value(info.clone()).unwrap()).is_ok());
        info["seed"] = json!({"bytes":2048,"files":3,"verified_at":1});
        assert_eq!(
            verified(serde_json::from_value(info.clone()).unwrap())
                .unwrap()
                .seed
                .unwrap()
                .bytes,
            2048
        );
        info["seed"] = json!({"bytes":u64::MAX,"files":3,"verified_at":1});
        assert!(verified(serde_json::from_value(info.clone()).unwrap()).is_err());
        info["seed"] = json!(null);
        info["policy"]["paths"] = json!(["../other"]);
        assert!(verified(serde_json::from_value(info).unwrap()).is_err());
    }
}
