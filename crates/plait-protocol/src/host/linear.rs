use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    LinearInfo,
    LinearAuthStatus,
    LinearSetKey {
        key: String,
    },
    LinearSignOut,
    LinearListTeams {
        refresh: bool,
    },
    LinearBindProject {
        checkout: PathBuf,
        team_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        team_key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        team_name: Option<String>,
    },
    LinearGetProjectBinding {
        checkout: PathBuf,
    },
    LinearListIssues {
        team_id: String,
        filter: Filter,
        #[serde(skip_serializing_if = "Option::is_none")]
        search: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
        refresh: bool,
    },
    LinearGetIssue {
        issue_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        comments_cursor: Option<String>,
        refresh: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
    },
    LinearResolveIssue {
        checkout: PathBuf,
        reference: String,
        refresh: bool,
    },
    LinearGetLinkedIssue {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        comments_cursor: Option<String>,
        refresh: bool,
    },
    LinearLinkThread {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
        issue_id: String,
        identifier: String,
        url: String,
        title: String,
    },
    LinearUnlinkThread {
        thread_id: String,
    },
    LinearGetThreadLink {
        thread_id: String,
    },
    LinearListThreadLinks {
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// A person-initiated move to another workflow state of the issue's team.
    LinearUpdateIssueState {
        issue_id: String,
        state_id: String,
        request_id: String,
    },
}
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if matches!(self, Self::LinearSetKey { .. }) {
            f.write_str("LinearSetKey { key: [REDACTED] }")
        } else {
            f.write_str("Linear request")
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    Active,
    Assigned,
    Backlog,
    Completed,
    All,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Features {
    pub durable_write_outcomes: bool,
    pub exact_issue_resolution: bool,
    pub workspace_links: bool,
    pub typed_rate_limits: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Info {
    pub schema: u32,
    pub features: Features,
    pub write_history_limit: u64,
    pub write_history_retention: String,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    SignedIn,
    SignedOut,
    KeyRejected,
}
#[derive(Clone, Debug, Deserialize)]
pub struct AuthStatus {
    pub signed_in: bool,
    pub status: AuthState,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub workspace_url_key: Option<String>,
    pub viewer_id: Option<String>,
    pub viewer_name: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct WorkflowState {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub position: f64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Team {
    pub id: String,
    pub key: String,
    pub name: String,
    pub states: Vec<WorkflowState>,
    #[serde(default)]
    pub states_complete: Option<bool>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Cache {
    pub cached: bool,
    pub stale: bool,
    pub cached_at: u64,
    pub cache_age_seconds: u64,
    pub partial: bool,
    pub warning: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Teams {
    pub teams: Vec<Team>,
    #[serde(default)]
    pub teams_complete: Option<bool>,
    #[serde(flatten)]
    pub cache: Cache,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Binding {
    pub workspace_id: Option<String>,
    pub team_id: String,
    pub team_key: Option<String>,
    pub team_name: Option<String>,
    pub updated_at: u64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ProjectBinding {
    pub checkout: PathBuf,
    pub binding: Option<Binding>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct IssueState {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Assignee {
    pub name: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct IssueSummary {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub url: String,
    pub priority: f64,
    pub updated_at: String,
    pub team_id: String,
    pub state: IssueState,
    pub assignee: Option<Assignee>,
    pub labels: Vec<Label>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub title: String,
    pub url: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Comment {
    pub id: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub author: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct IssueDetail {
    #[serde(flatten)]
    pub summary: IssueSummary,
    pub description: Option<String>,
    pub attachments: Vec<Attachment>,
    pub comments: Vec<Comment>,
    pub comments_has_more: bool,
    pub comments_next_cursor: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Issues {
    pub issues: Vec<IssueSummary>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    #[serde(flatten)]
    pub cache: Cache,
}
#[derive(Clone, Debug, Deserialize)]
pub struct IssueReply<T> {
    pub issue: T,
    pub workspace_id: String,
    #[serde(flatten)]
    pub cache: Cache,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Link {
    pub thread_id: String,
    pub workspace_id: Option<String>,
    pub issue_id: String,
    pub identifier: String,
    pub url: String,
    pub title: String,
    pub linked_at: u64,
    pub historical: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct LinkReply {
    pub link: Link,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ThreadLink {
    pub thread_id: String,
    pub link: Option<Link>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Unlinked {
    pub thread_id: String,
    pub removed: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Links {
    pub links: Vec<Link>,
    pub next_cursor: Option<String>,
}
/// `linear_update_issue_state`'s reply. Linear reported success; `issue` is the
/// refreshed detail, or null when that read failed and `warning` says so.
#[derive(Clone, Debug, Deserialize)]
pub struct StateUpdate {
    pub issue: Option<IssueDetail>,
    #[serde(default)]
    pub partial: bool,
    pub warning: Option<String>,
    pub outcome: String,
}
