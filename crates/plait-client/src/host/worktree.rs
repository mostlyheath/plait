//! Additive host-owned worktree operations. Blocking I/O belongs off the UI thread.
use super::{ApprovalMode, Client, Error, HostError, ThinkingLevel, harness::HarnessId};
use crate::git::worktree::{Identity, ManagedWorktree, Parent};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const OLDER_HOST: &str = "Host-owned worktrees need a newer host.";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    ProjectWorktrees {
        checkout: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<PathBuf>,
    },
    ProjectWorktreesBuild {
        checkout: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<PathBuf>,
    },
    WorktreeInfo {
        checkout: PathBuf,
    },
    CreateWorktreeThread {
        request_id: String,
        parent: Box<Parent>,
        harness: HarnessId,
        provider: String,
        model_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<ThinkingLevel>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        approval: ApprovalMode,
    },
    WorktreeStatus {
        request_id: String,
    },
    ListWorktrees,
    RemoveWorktree {
        request_id: String,
        creation_id: String,
        identity: Identity,
        allow_unpushed: bool,
    },
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    pub schema: u32,
    pub worktree_threads_v1: bool,
    pub parent: Parent,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Preparing,
    Ready,
    Failed,
    Removing,
    Removed,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub request_id: String,
    pub thread_id: String,
    pub state: Stage,
    pub worktree: Option<ManagedWorktree>,
    pub error: Option<HostError>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    schema: u32,
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listing {
    schema: u32,
    operations: Vec<Operation>,
}
fn invalid() -> Error {
    Error::Protocol("Invalid worktree reply. Reconnect before retrying.".into())
}
pub(super) fn bounded_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub(super) fn path(path: &std::path::Path) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 4096
        && path.to_str().is_some_and(|s| {
            !s.bytes().any(|byte| byte.is_ascii_control())
                && !s.contains("//")
                && !s.split('/').any(|part| part == "." || part == "..")
        })
}
pub(super) fn identity(identity: &Identity) -> bool {
    [&identity.dev, &identity.ino].into_iter().all(|value| {
        value
            .parse::<u64>()
            .is_ok_and(|number| number.to_string() == *value)
    })
}
pub(super) fn parent(parent: &Parent) -> bool {
    [&parent.root, &parent.main, &parent.common]
        .into_iter()
        .all(|p| path(p))
        && [
            &parent.identity,
            &parent.main_identity,
            &parent.common_identity,
        ]
        .into_iter()
        .all(identity)
        && matches!(parent.base.len(), 40 | 64)
        && parent
            .base
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
impl Operation {
    pub fn verified(self) -> Result<Self, Error> {
        let hex = self.thread_id.replace('-', "");
        if !bounded_id(&self.request_id)
            || self.thread_id.len() != 36
            || !self.thread_id.bytes().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
                }
            })
        {
            return Err(invalid());
        }
        if let Some(tree) = &self.worktree {
            let suffix = tree.branch.strip_prefix("plait/").ok_or_else(invalid)?;
            if tree.thread_id != self.thread_id
                || !parent(&tree.parent)
                || !path(&tree.path)
                || !matches!(suffix.len(), 12 | 16 | 20 | 24 | 28 | 32)
                || !hex.starts_with(suffix)
                || tree.path.file_name().and_then(|p| p.to_str()) != Some(suffix)
                || tree.identity.as_ref().is_some_and(|id| !identity(id))
                || tree.error.as_ref().is_some_and(|e| e.len() > 8192)
            {
                return Err(invalid());
            }
            if matches!(self.state, Stage::Ready)
                && tree.stage != crate::git::worktree::ManagedStage::Ready
                || matches!(self.state, Stage::Removed)
                    && tree.stage != crate::git::worktree::ManagedStage::Removed
            {
                return Err(invalid());
            }
            let mut name = tree.parent.main.file_name().ok_or_else(invalid)?.to_owned();
            name.push(".worktrees");
            if tree.path.parent()
                != Some(
                    tree.parent
                        .main
                        .parent()
                        .ok_or_else(invalid)?
                        .join(name)
                        .as_path(),
                )
            {
                return Err(invalid());
            }
        }
        if matches!(self.state, Stage::Ready | Stage::Removed | Stage::Removing)
            && self
                .worktree
                .as_ref()
                .is_none_or(|tree| tree.identity.is_none())
        {
            return Err(invalid());
        }
        Ok(self)
    }
}
fn compatibility(error: Error) -> Error {
    if matches!(&error, Error::Rejected(reason) if reason.code == "invalid_request") {
        Error::Protocol(OLDER_HOST.into())
    } else {
        error
    }
}
pub fn info(client: &Client, checkout: PathBuf) -> Result<Info, Error> {
    let info: Info = client
        .send(&Request::WorktreeInfo { checkout })
        .map_err(compatibility)?;
    if info.schema != 1 || !info.worktree_threads_v1 || !parent(&info.parent) {
        return Err(invalid());
    }
    Ok(info)
}
pub fn operation(client: &Client, request: &Request) -> Result<Operation, Error> {
    let reply: Reply = client.send(request)?;
    if reply.schema != 1 {
        return Err(invalid());
    }
    reply.operation.verified()
}
pub fn list(client: &Client) -> Result<Vec<Operation>, Error> {
    let reply: Listing = client
        .send(&Request::ListWorktrees)
        .map_err(compatibility)?;
    if reply.schema != 1 || reply.operations.len() > 256 {
        return Err(invalid());
    }
    reply
        .operations
        .into_iter()
        .map(Operation::verified)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn ready() -> serde_json::Value {
        let identity = json!({"dev":"1","ino":"2"});
        json!({"request_id":"request","thread_id":"12345678-1234-4abc-8abc-123456789abc","state":"ready","error":null,
            "worktree":{"thread_id":"12345678-1234-4abc-8abc-123456789abc","parent":{"root":"/repo","main":"/repo","common":"/repo/.git","identity":identity,"main_identity":identity,"common_identity":identity,"base":"a".repeat(40)},"path":"/repo.worktrees/123456781234","branch":"plait/123456781234","identity":identity,"stage":"ready","error":null}})
    }
    #[test]
    fn strict_worktree_replies_reject_unknown_states_fields_and_forged_paths() {
        let good = ready();
        assert!(
            serde_json::from_value::<Operation>(good.clone())
                .unwrap()
                .verified()
                .is_ok()
        );
        for (field, value) in [("state", json!("future")), ("extra", json!(true))] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(serde_json::from_value::<Operation>(bad).is_err());
        }
        for (field, value) in [
            ("path", json!("/elsewhere/123456781234")),
            ("branch", json!("plait/other")),
            ("identity", json!({"dev":"1","ino":"18446744073709551616"})),
            ("thread_id", json!("different")),
        ] {
            let mut bad = good.clone();
            bad["worktree"][field] = value;
            assert!(
                serde_json::from_value::<Operation>(bad)
                    .unwrap()
                    .verified()
                    .is_err()
            );
        }
        let mut bad = good;
        bad["worktree"] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<Operation>(bad)
                .unwrap()
                .verified()
                .is_err()
        );
    }
    #[test]
    fn additive_worktree_requests_omit_version_and_reject_unrecognized_fields() {
        let request = Request::WorktreeStatus {
            request_id: "same-id".into(),
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"op":"worktree_status","request_id":"same-id"})
        );
        assert!(
            serde_json::from_value::<Request>(
                json!({"op":"worktree_status","request_id":"id","protocol_version":6})
            )
            .is_err()
        );
    }
    #[test]
    fn worktree_reply_requires_canonical_ids_and_consistent_stages() {
        let good = ready();
        let mut bad = good.clone();
        bad["thread_id"] = json!("123456781234-4abc-8abc-123456789abc-");
        let mut variants = vec![bad];
        for (field, value) in [
            ("stage", json!("removed")),
            ("identity", json!({"dev":"+1","ino":"2"})),
            ("identity", json!({"dev":"1","ino":"02"})),
            ("path", json!("/repo.worktrees//123456781234")),
        ] {
            let mut bad = good.clone();
            bad["worktree"][field] = value;
            variants.push(bad);
        }
        let mut bad = good;
        bad["worktree"]["branch"] = json!("plait/1234567812344");
        bad["worktree"]["path"] = json!("/repo.worktrees/1234567812344");
        variants.push(bad);
        for bad in variants {
            assert!(
                serde_json::from_value::<Operation>(bad)
                    .unwrap()
                    .verified()
                    .is_err()
            );
        }
    }
}

/// Additive inventory includes foreign checkouts and the host's removal decision.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub schema: u32,
    pub worktree_inventory_v1: bool,
    #[serde(default)]
    pub worktree_build_output_v1: bool,
    pub checkouts: Vec<Checkout>,
    /// Logical bytes of the project's build seed, counted in its storage.
    #[serde(default)]
    pub seed_bytes: Option<u64>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    pub path: PathBuf,
    pub main: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub base: Option<String>,
    pub thread_id: Option<String>,
    pub identity: Option<Identity>,
    pub missing: bool,
    pub removed: bool,
    pub preservation: crate::git::worktree::inventory::Preservation,
    pub creation_id: Option<String>,
    pub last_used: Option<u64>,
    #[serde(default)]
    pub build_directories: Vec<String>,
    #[serde(default)]
    pub build_output: Option<crate::build_output::Outcome>,
}
impl Inventory {
    pub fn verified(self) -> Result<Self, Error> {
        self.verified_with(|index| index == 0)
    }
    /// A reply to `only` names exactly that checkout, once, as main or not
    /// as the caller already knows it to be.
    fn verified_row(self, only: &std::path::Path, main: bool) -> Result<Checkout, Error> {
        let mut rows = self.verified_with(|_| main)?.checkouts;
        if rows.len() != 1 || rows[0].path != only {
            return Err(invalid());
        }
        Ok(rows.remove(0))
    }
    fn verified_with(self, main_at: impl Fn(usize) -> bool) -> Result<Self, Error> {
        let mut paths = std::collections::HashSet::new();
        if self.schema != 1
            || !self.worktree_inventory_v1
            || self.checkouts.is_empty()
            || self.checkouts.len() > 256
            || self.seed_bytes.is_some_and(|n| n > 9_007_199_254_740_991)
            || !self.worktree_build_output_v1 && self.seed_bytes.is_some()
        {
            return Err(invalid());
        }
        for (index, row) in self.checkouts.iter().enumerate() {
            if row.build_directories.len() > crate::build_output::MAX_PATHS
                || row
                    .build_directories
                    .iter()
                    .any(|p| crate::build_output::relative(p).is_err())
                || row
                    .build_output
                    .as_ref()
                    .is_some_and(|o| !super::build_output::valid_outcome(o))
                || (!self.worktree_build_output_v1
                    && (row.build_output.is_some()
                        || !row.build_directories.is_empty()
                        || row.preservation.build_output.is_some()))
                || !path(&row.path)
                || !paths.insert(&row.path)
                || row.main != main_at(index)
                || row.identity.as_ref().is_some_and(|id| !identity(id))
                || [&row.head, &row.base].into_iter().flatten().any(|s| {
                    !matches!(s.len(), 40 | 64)
                        || !s
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
                || [&row.thread_id, &row.creation_id]
                    .into_iter()
                    .flatten()
                    .any(|s| !bounded_id(s))
                || row
                    .branch
                    .as_ref()
                    .is_some_and(|s| s.len() > 4096 || s.chars().any(char::is_control))
                || row
                    .preservation
                    .refusal
                    .as_ref()
                    .is_some_and(|s| s.is_empty() || s.len() > 8192)
                || [
                    row.preservation.changed,
                    row.preservation.build_output,
                    row.preservation.unpushed,
                    row.preservation.exclusive,
                    row.last_used,
                ]
                .into_iter()
                .flatten()
                .any(|n| n > 9_007_199_254_740_991)
                || (row.removed && (!row.missing || row.main))
                || ((row.preservation.refusal.is_none() || row.preservation.can_keep_local)
                    && (row.main
                        || row.missing
                        || row.creation_id.is_none()
                        || row.identity.is_none()
                        || row.thread_id.is_none()
                        || row.branch.as_ref().is_none_or(|branch| branch.is_empty())
                        || row.preservation.changed != Some(0)
                        || row.preservation.exclusive != Some(0)))
                || (row.preservation.refusal.is_none() && row.preservation.unpushed != Some(0))
            {
                return Err(invalid());
            }
        }
        Ok(self)
    }
}
fn read_inventory(
    client: &Client,
    checkout: PathBuf,
    only: Option<PathBuf>,
) -> Result<Inventory, Error> {
    match client.send(&Request::ProjectWorktreesBuild {
        checkout: checkout.clone(),
        only: only.clone(),
    }) {
        Ok(reply) => Ok(reply),
        Err(Error::Rejected(e)) if e.code == "invalid_request" => client
            .send(&Request::ProjectWorktrees { checkout, only })
            .map_err(compatibility),
        Err(error) => Err(error),
    }
}
pub fn inventory(client: &Client, checkout: PathBuf) -> Result<Inventory, Error> {
    read_inventory(client, checkout, None)?.verified()
}
/// One checkout's live row, falling back to inventory on an older host.
pub fn inventory_row(
    client: &Client,
    checkout: PathBuf,
    only: PathBuf,
    main: bool,
) -> Result<Checkout, Error> {
    read_inventory(client, checkout, Some(only.clone()))?.verified_row(&only, main)
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    use serde_json::json;
    fn recorded() -> serde_json::Value {
        json!({"schema":1,"worktree_inventory_v1":true,"checkouts":[
            {"path":"/repo","main":true,"branch":"main","head":"a".repeat(40),"base":null,"thread_id":null,"identity":{"dev":"1","ino":"2"},"missing":false,"removed":false,"creation_id":null,"last_used":null,"preservation":{"changed":0,"unpushed":0,"exclusive":0,"refusal":"The main checkout cannot be removed here."}},
            {"path":"/repo.worktrees/owned","main":false,"branch":"plait/owned","head":"b".repeat(40),"base":"a".repeat(40),"thread_id":"thread","identity":{"dev":"1","ino":"3"},"missing":false,"removed":false,"creation_id":"creation","last_used":1800000000000_u64,"preservation":{"changed":0,"unpushed":0,"exclusive":0,"refusal":null}},
            {"path":"/external","main":false,"branch":"topic","head":"c".repeat(40),"base":null,"thread_id":null,"identity":null,"missing":false,"removed":false,"creation_id":null,"last_used":null,"preservation":{"changed":1,"unpushed":null,"exclusive":0,"refusal":"Plait did not create this worktree. Remove it with your Git tools."}}
        ]})
    }
    fn read(value: serde_json::Value) -> Result<Inventory, String> {
        serde_json::from_value::<Inventory>(value)
            .map_err(|e| e.to_string())?
            .verified()
            .map_err(|e| e.to_string())
    }
    #[test]
    fn inventory_preserves_foreign_missing_removed_and_exact_refusals() {
        let good = read(recorded()).unwrap();
        assert!(good.checkouts[0].main);
        assert!(good.checkouts[1].preservation.refusal.is_none());
        assert_eq!(good.checkouts[2].creation_id, None);
        for reason in [
            "Hidden index edit.",
            "This worktree still has active, queued, or recovering work.",
            "Commits kept only here.",
            "The folder is missing.",
        ] {
            let mut reply = recorded();
            reply["checkouts"][1]["preservation"]["refusal"] = json!(reason);
            reply["checkouts"][1]["missing"] = json!(true);
            assert_eq!(
                read(reply).unwrap().checkouts[1]
                    .preservation
                    .refusal
                    .as_deref(),
                Some(reason)
            );
        }
        let mut removed = recorded();
        removed["checkouts"][1]["missing"] = json!(true);
        removed["checkouts"][1]["removed"] = json!(true);
        removed["checkouts"][1]["preservation"]["refusal"] = json!("This worktree was removed.");
        assert!(read(removed).unwrap().checkouts[1].removed);
    }
    /// A one-row reply must be exactly the checkout asked for, main or not as known.
    #[test]
    fn a_one_row_reply_names_only_the_checkout_asked_for() {
        let row = |rows: serde_json::Value, only: &str, main: bool| {
            serde_json::from_value::<Inventory>(
                json!({"schema":1,"worktree_inventory_v1":true,"checkouts":rows}),
            )
            .unwrap()
            .verified_row(std::path::Path::new(only), main)
            .map(|row| row.path)
            .map_err(|e| e.to_string())
        };
        let all = recorded()["checkouts"].clone();
        let owned = json!([all[1]]);
        assert_eq!(
            row(owned.clone(), "/repo.worktrees/owned", false).unwrap(),
            PathBuf::from("/repo.worktrees/owned")
        );
        assert_eq!(
            row(json!([all[0]]), "/repo", true).unwrap(),
            PathBuf::from("/repo")
        );
        for (rows, only, main) in [
            (owned.clone(), "/repo.worktrees/owned", true),
            (owned.clone(), "/external", false),
            (json!([all[0]]), "/repo", false),
            (json!([all[1], all[2]]), "/repo.worktrees/owned", false),
            (json!([all[1], all[1]]), "/repo.worktrees/owned", false),
            (json!([]), "/repo.worktrees/owned", false),
        ] {
            assert!(row(rows.clone(), only, main).is_err(), "{rows} {only}");
        }
        let request = Request::ProjectWorktrees {
            checkout: "/repo".into(),
            only: None,
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"op":"project_worktrees","checkout":"/repo"})
        );
    }
    #[test]
    fn inventory_rejects_unknown_fields_capabilities_and_false_safe_decisions() {
        for (field, value) in [
            ("schema", json!(2)),
            ("worktree_inventory_v1", json!(false)),
            ("future", json!(true)),
        ] {
            let mut bad = recorded();
            bad[field] = value;
            assert!(read(bad).is_err());
        }
        for (field, value) in [
            ("path", json!("/repo")),
            ("main", json!(true)),
            ("head", json!("--all")),
            ("identity", json!({"dev":"01","ino":"2"})),
            ("missing", json!(true)),
            ("removed", json!(true)),
            ("creation_id", json!(null)),
            ("last_used", json!(u64::MAX)),
        ] {
            let mut bad = recorded();
            bad["checkouts"][1][field] = value;
            assert!(read(bad).is_err(), "{field}");
        }
        for field in ["changed", "unpushed", "exclusive"] {
            let mut bad = recorded();
            bad["checkouts"][1]["preservation"][field] = json!(1);
            assert!(read(bad).is_err());
        }
        let mut bad = recorded();
        bad["checkouts"] = json!([]);
        assert!(read(bad).is_err());
        assert_eq!(
            compatibility(Error::Rejected(HostError {
                retry_after_seconds: None,
                code: "invalid_request".into(),
                message: "Unknown operation".into(),
                outcome: None,
            }))
            .to_string(),
            Error::Protocol(OLDER_HOST.into()).to_string()
        );
    }
    #[test]
    fn keep_local_permission_is_explicit_and_strictly_validated() {
        assert!(
            !read(recorded()).unwrap().checkouts[1]
                .preservation
                .can_keep_local
        );
        for unpushed in [json!(null), json!(2)] {
            let mut good = recorded();
            let facts = &mut good["checkouts"][1]["preservation"];
            facts["refusal"] = json!("Publication needs confirmation.");
            facts["unpushed"] = unpushed;
            facts["can_keep_local"] = json!(true);
            assert!(
                read(good.clone()).unwrap().checkouts[1]
                    .preservation
                    .can_keep_local
            );
            for field in ["changed", "exclusive"] {
                let mut bad = good.clone();
                bad["checkouts"][1]["preservation"][field] = json!(1);
                assert!(read(bad).is_err(), "{field}");
            }
            for (field, value) in [
                ("missing", json!(true)),
                ("removed", json!(true)),
                ("creation_id", json!(null)),
                ("identity", json!(null)),
                ("thread_id", json!(null)),
                ("branch", json!(null)),
            ] {
                let mut bad = good.clone();
                bad["checkouts"][1][field] = value;
                assert!(read(bad).is_err(), "{field}");
            }
            for value in [json!(null), json!("true"), json!(1)] {
                let mut bad = good.clone();
                bad["checkouts"][1]["preservation"]["can_keep_local"] = value;
                assert!(read(bad).is_err());
            }
        }
        for row in [0, 2] {
            let mut bad = recorded();
            bad["checkouts"][row]["preservation"]["can_keep_local"] = json!(true);
            assert!(read(bad).is_err());
        }
    }
}
