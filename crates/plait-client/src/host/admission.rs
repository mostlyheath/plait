//! Additive placement negotiation; an unavailable capability never grants a choice.
use super::{ApprovalMode, Client, Error, HostError, ThinkingLevel, harness::HarnessId, worktree};
use crate::git::worktree::{Identity, ManagedWorktree, Parent};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    RunPreparation {
        thread_id: String,
    },
    AdmissionInfo {
        checkout: PathBuf,
    },
    SetProjectIsolation {
        checkout: PathBuf,
        identity: Identity,
        auto_worktree: bool,
    },
    CreateAdmittedThread {
        request_id: String,
        checkout: PathBuf,
        placement: Placement,
        harness: HarnessId,
        provider: String,
        model_id: String,
        approval: ApprovalMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<ThinkingLevel>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_thread_id: Option<String>,
    },
    AdmissionStatus {
        request_id: String,
    },
    ThreadOrigin {
        thread_id: String,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Project,
    Worktree,
    Wait,
}
impl Placement {
    /// Saved files name Wait as an in-place placement plus a flag, because
    /// released builds reject the whole workspace on a placement they do not know.
    pub fn journaled(self) -> (Self, bool) {
        if self == Self::Wait {
            (Self::Project, true)
        } else {
            (self, false)
        }
    }
    pub fn from_journal(self, wait: bool) -> Self {
        if wait && self == Self::Project {
            Self::Wait
        } else {
            self
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    pub schema: u32,
    pub worktree_at_start_v1: bool,
    pub linked_continuation_v1: bool,
    pub queue: bool,
    pub strict_reader: bool,
    pub checkout: PathBuf,
    pub identity: Identity,
    pub project: PathBuf,
    pub project_identity: Identity,
    pub busy: bool,
    pub auto_worktree: bool,
    pub parent: Option<Parent>,
    pub refusal: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Choice,
    Preparing,
    Ready,
    Failed,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub request_id: String,
    pub state: Stage,
    pub thread_id: Option<String>,
    pub checkout: PathBuf,
    pub worktree: Option<ManagedWorktree>,
    pub source_thread_id: Option<String>,
    pub error: Option<HostError>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    schema: u32,
    admission: Admission,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    schema: u32,
    admission: Option<Admission>,
}
fn invalid() -> Error {
    Error::Protocol("Invalid admission reply. Reconnect before retrying.".into())
}
impl Info {
    pub fn verified(self) -> Result<Self, Error> {
        if self.schema != 1
            || !self.worktree_at_start_v1
            || !self.linked_continuation_v1
            || !worktree::path(&self.checkout)
            || !worktree::path(&self.project)
            || !worktree::identity(&self.identity)
            || !worktree::identity(&self.project_identity)
            || self.refusal.as_ref().is_some_and(|s| s.len() > 8192)
            || self.parent.as_ref().is_some_and(|p| {
                !worktree::parent(p)
                    || p.root != self.checkout
                    || p.identity != self.identity
                    || p.main != self.project
                    || p.main_identity != self.project_identity
            })
        {
            return Err(invalid());
        }
        Ok(self)
    }
}
impl Admission {
    pub fn verify_request(&self, request: &Request) -> Result<(), Error> {
        let Request::CreateAdmittedThread {
            request_id,
            checkout,
            placement,
            source_thread_id,
            ..
        } = request
        else {
            return Err(invalid());
        };
        if self.request_id != *request_id
            || self.source_thread_id != *source_thread_id
            || self
                .worktree
                .as_ref()
                .is_some_and(|tree| tree.parent.root != *checkout)
            || self.worktree.is_none() && self.checkout != *checkout
            || *placement == Placement::Wait && self.worktree.is_some()
            || self.state == Stage::Ready
                && *placement == Placement::Worktree
                && self.worktree.is_none()
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn verified(self) -> Result<Self, Error> {
        if !worktree::bounded_id(&self.request_id)
            || !worktree::path(&self.checkout)
            || self
                .thread_id
                .as_ref()
                .is_some_and(|id| !worktree::bounded_id(id))
            || self
                .source_thread_id
                .as_ref()
                .is_some_and(|id| !worktree::bounded_id(id))
            || self.state == Stage::Ready && (self.thread_id.is_none() || self.error.is_some())
            || self.state == Stage::Choice
                && (self.thread_id.is_some() || self.worktree.is_some() || self.error.is_some())
            || self.state == Stage::Failed && self.error.is_none()
        {
            return Err(invalid());
        }
        if let Some(tree) = &self.worktree {
            if self.thread_id.as_ref() != Some(&tree.thread_id) || self.checkout != tree.path {
                return Err(invalid());
            }
            worktree::Operation {
                request_id: self.request_id.clone(),
                thread_id: tree.thread_id.clone(),
                state: if self.state == Stage::Ready {
                    worktree::Stage::Ready
                } else {
                    worktree::Stage::Preparing
                },
                worktree: Some(tree.clone()),
                error: self.error.clone(),
            }
            .verified()?;
        }
        Ok(self)
    }
}
pub fn info(client: &Client, checkout: PathBuf) -> Result<Option<Info>, Error> {
    match client.send::<_, Info>(&Request::AdmissionInfo { checkout }) {
        Ok(info) => info.verified().map(Some),
        Err(Error::Rejected(error)) if error.code == "invalid_request" => Ok(None),
        Err(error) => Err(error),
    }
}
pub fn set(client: &Client, request: &Request) -> Result<Info, Error> {
    client.send::<_, Info>(request)?.verified()
}
pub fn admission(client: &Client, request: &Request) -> Result<Admission, Error> {
    let reply: Reply = client.send(request)?;
    if reply.schema != 1 {
        return Err(invalid());
    }
    reply.admission.verified()
}
impl Origin {
    fn verified(self, thread_id: &str) -> Result<Option<Admission>, Error> {
        if self.schema != 1
            || self
                .admission
                .as_ref()
                .is_some_and(|receipt| receipt.thread_id.as_deref() != Some(thread_id))
        {
            return Err(invalid());
        }
        self.admission.map(Admission::verified).transpose()
    }
}
pub fn origin(client: &Client, thread_id: String) -> Result<Option<Admission>, Error> {
    let reply: Origin = client.send(&Request::ThreadOrigin {
        thread_id: thread_id.clone(),
    })?;
    reply.verified(&thread_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn origin_rejects_a_receipt_for_another_thread() {
        let reply = json!({"schema":1,"admission":{"request_id":"r1","state":"ready","thread_id":"new","checkout":"/repo","worktree":null,"source_thread_id":"original","error":null}});
        assert!(
            serde_json::from_value::<Origin>(reply.clone())
                .unwrap()
                .verified("new")
                .is_ok()
        );
        assert!(
            serde_json::from_value::<Origin>(reply)
                .unwrap()
                .verified("other")
                .is_err()
        );
    }
    #[test]
    fn admission_rejects_unknown_and_inconsistent_replies() {
        let good = json!({"request_id":"r1","state":"choice","thread_id":null,"checkout":"/repo","worktree":null,"source_thread_id":null,"error":null});
        assert!(
            serde_json::from_value::<Admission>(good.clone())
                .unwrap()
                .verified()
                .is_ok()
        );
        for (key, value) in [("state", json!("waiting")), ("extra", json!(true))] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(serde_json::from_value::<Admission>(bad).is_err());
        }
        for (key, value) in [
            ("state", json!("ready")),
            ("checkout", json!("/repo/../else")),
            ("thread_id", json!("existing")),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(
                serde_json::from_value::<Admission>(bad)
                    .unwrap()
                    .verified()
                    .is_err()
            );
        }
    }
    #[test]
    fn admission_reply_must_keep_its_request_and_source_checkout() {
        let request = Request::CreateAdmittedThread {
            request_id: "one".into(),
            checkout: "/repo".into(),
            placement: Placement::Worktree,
            harness: HarnessId::Pi,
            provider: "faux".into(),
            model_id: "faux-1".into(),
            approval: ApprovalMode::Ask,
            mode: None,
            thinking: None,
            source_thread_id: Some("original".into()),
        };
        let mut receipt = Admission {
            request_id: "one".into(),
            state: Stage::Preparing,
            thread_id: None,
            checkout: "/repo".into(),
            worktree: None,
            source_thread_id: Some("original".into()),
            error: None,
        };
        assert!(receipt.verify_request(&request).is_ok());
        receipt.request_id = "other".into();
        assert!(receipt.verify_request(&request).is_err());
        receipt.request_id = "one".into();
        receipt.source_thread_id = None;
        assert!(receipt.verify_request(&request).is_err());
        receipt.source_thread_id = Some("original".into());
        receipt.checkout = "/elsewhere".into();
        assert!(receipt.verify_request(&request).is_err());
        receipt.checkout = "/repo".into();
        receipt.state = Stage::Ready;
        receipt.thread_id = Some("new".into());
        assert!(receipt.verify_request(&request).is_err());
    }
    #[test]
    fn admission_request_has_no_protocol_version_or_queue_choice() {
        assert!(
            serde_json::from_value::<Request>(
                json!({"op":"admission_info","checkout":"/repo","protocol_version":6})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<Placement>(json!("queue")).is_err());
        assert_eq!(
            serde_json::from_value::<Placement>(json!("wait")).unwrap(),
            Placement::Wait
        );
    }
    #[test]
    fn explicit_wait_cannot_be_rebound_to_a_worktree() {
        let request: Request = serde_json::from_value(json!({"op":"create_admitted_thread","request_id":"r","checkout":"/repo","placement":"wait","harness":"pi","provider":"p","model_id":"m","approval":"ask"})).unwrap();
        let mut receipt: Admission = serde_json::from_value(json!({"request_id":"r","state":"ready","thread_id":"t","checkout":"/repo","worktree":null,"source_thread_id":null,"error":null})).unwrap();
        assert!(receipt.verify_request(&request).is_ok());
        let identity = json!({"dev":"1","ino":"2"});
        receipt.worktree = Some(serde_json::from_value(json!({"thread_id":"t","parent":{"root":"/repo","identity":identity,"main":"/repo","main_identity":identity,"common":"/repo/.git","common_identity":identity,"base":"a".repeat(40)},"path":"/other","branch":"plait/t","identity":identity,"stage":"ready","error":null})).unwrap());
        receipt.checkout = "/other".into();
        assert!(receipt.verify_request(&request).is_err());
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    ResourceWait,
    Running,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    schema: u32,
    thread_id: String,
    phase: Option<Phase>,
}
pub fn preparation(client: &Client, thread_id: String) -> Result<Option<Phase>, Error> {
    let reply: Preparation = client.send(&Request::RunPreparation {
        thread_id: thread_id.clone(),
    })?;
    if reply.schema != 1 || reply.thread_id != thread_id {
        return Err(invalid());
    }
    Ok(reply.phase)
}
