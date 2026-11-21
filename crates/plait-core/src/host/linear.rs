//! Additive, bounded Linear host operations.
use super::{Client, Error};
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

fn send<T: serde::de::DeserializeOwned>(c: &Client, request: &Request) -> Result<T, Error> {
    c.send(request).map_err(|e| {
        if matches!(e, Error::Protocol(_)) {
            invalid()
        } else {
            e
        }
    })
}
fn invalid() -> Error {
    Error::Protocol("Invalid Linear host reply.".into())
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max
}
fn opt(s: &Option<String>, max: usize) -> bool {
    s.as_ref().is_none_or(|v| text(v, max))
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn oid(s: &Option<String>) -> bool {
    s.as_ref().is_none_or(|s| id(s))
}
fn identifier(s: &str) -> bool {
    let Some((prefix, number)) = s.split_once('-') else {
        return false;
    };
    (1..=16).contains(&prefix.len())
        && prefix.as_bytes()[0].is_ascii_uppercase()
        && prefix
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && (1..=9).contains(&number.len())
        && number.bytes().all(|b| b.is_ascii_digit())
}
fn https(s: &str) -> bool {
    if s.len() > 2048
        || s.bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return false;
    }
    let Some(rest) = s.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') || s.contains('\\') {
        return false;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, suffix)) = rest.split_once(']') else {
            return false;
        };
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        (
            host,
            suffix
                .strip_prefix(':')
                .or_else(|| suffix.is_empty().then_some("")),
        )
    } else {
        let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        {
            return false;
        }
        (host, Some(port))
    };
    !host.is_empty() && port.is_some_and(|p| p.is_empty() || p.parse::<u16>().is_ok())
}
fn cache(c: &Cache) -> bool {
    opt(&c.warning, 2048)
        && c.cached_at <= 9_007_199_254_740_991
        && c.cache_age_seconds <= 9_007_199_254_740_991
}
fn summary(s: &IssueSummary) -> bool {
    id(&s.id)
        && identifier(&s.identifier)
        && text(&s.title, 512)
        && https(&s.url)
        && s.priority.is_finite()
        && text(&s.updated_at, 64)
        && id(&s.team_id)
        && id(&s.state.id)
        && text(&s.state.name, 256)
        && text(&s.state.kind, 64)
        && s.assignee.as_ref().is_none_or(|a| text(&a.name, 256))
        && s.labels.len() <= 8
        && s.labels.iter().all(|l| id(&l.id) && text(&l.name, 256))
}
fn detail(d: &IssueDetail) -> bool {
    summary(&d.summary)
        && opt(&d.description, 65536)
        && d.attachments.len() <= 20
        && d.attachments
            .iter()
            .all(|a| id(&a.id) && text(&a.title, 512) && text(&a.url, 2048))
        && d.comments.len() <= 25
        && d.comments.iter().all(|c| {
            id(&c.id)
                && text(&c.body, 16384)
                && text(&c.created_at, 64)
                && text(&c.updated_at, 64)
                && opt(&c.author, 256)
        })
        && opt(&d.comments_next_cursor, 512)
}
fn link(l: &Link) -> bool {
    id(&l.thread_id)
        && oid(&l.workspace_id)
        && id(&l.issue_id)
        && identifier(&l.identifier)
        && https(&l.url)
        && text(&l.title, 512)
        && l.linked_at <= 9_007_199_254_740_991
}
fn checked<T>(value: T, valid: bool) -> Result<T, Error> {
    if valid { Ok(value) } else { Err(invalid()) }
}

/// Classification of host rejections. Invalid request means an older host only for a capability probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    NeedsNewerHost,
    Offline,
    RateLimited(Option<u32>),
    Conflict,
    NotFound,
    KeyRejected,
    SignedOut,
    Other,
}
pub fn classify(error: &Error, capability_probe: bool) -> Rejection {
    let Error::Rejected(e) = error else {
        return Rejection::Other;
    };
    match e.code.as_str() {
        "invalid_request" if capability_probe => Rejection::NeedsNewerHost,
        "linear_offline" => Rejection::Offline,
        "linear_rate_limited" => Rejection::RateLimited(e.retry_after_seconds),
        "conflict" => Rejection::Conflict,
        "not_found" => Rejection::NotFound,
        "linear_key_rejected" => Rejection::KeyRejected,
        "linear_not_signed_in" => Rejection::SignedOut,
        _ => Rejection::Other,
    }
}
pub fn info(c: &Client) -> Result<Info, Error> {
    let v: Info = send(c, &Request::LinearInfo)?;
    let ok = v.schema == 1
        && v.write_history_limit <= 9_007_199_254_740_991
        && text(&v.write_history_retention, 64);
    checked(v, ok)
}
fn auth(c: &Client, r: Request) -> Result<AuthStatus, Error> {
    let v: AuthStatus = send(c, &r)?;
    let ok = oid(&v.workspace_id)
        && opt(&v.workspace_name, 256)
        && opt(&v.workspace_url_key, 256)
        && oid(&v.viewer_id)
        && opt(&v.viewer_name, 256)
        && v.signed_in == (v.status == AuthState::SignedIn);
    checked(v, ok)
}
pub fn auth_status(c: &Client) -> Result<AuthStatus, Error> {
    auth(c, Request::LinearAuthStatus)
}
pub fn set_key(c: &Client, key: String) -> Result<AuthStatus, Error> {
    auth(c, Request::LinearSetKey { key }).map_err(|error| match error {
        Error::Rejected(mut host) => {
            if !matches!(
                host.code.as_str(),
                "invalid_request"
                    | "linear_not_signed_in"
                    | "linear_key_rejected"
                    | "linear_offline"
                    | "linear_rate_limited"
                    | "conflict"
                    | "not_found"
                    | "internal"
            ) {
                host.code = "internal".into();
            }
            host.message = "Linear sign-in failed.".into();
            Error::Rejected(host)
        }
        other => other,
    })
}
pub fn sign_out(c: &Client) -> Result<AuthStatus, Error> {
    auth(c, Request::LinearSignOut)
}
pub fn list_teams(c: &Client, refresh: bool) -> Result<Teams, Error> {
    let v: Teams = send(c, &Request::LinearListTeams { refresh })?;
    let ok = cache(&v.cache)
        && v.teams.len() <= 50
        && v.teams.iter().all(|t| {
            id(&t.id)
                && text(&t.key, 32)
                && text(&t.name, 256)
                && t.states.len() <= 50
                && t.states.iter().all(|s| {
                    id(&s.id) && text(&s.name, 256) && text(&s.kind, 64) && s.position.is_finite()
                })
        });
    checked(v, ok)
}
fn binding(
    c: &Client,
    r: Request,
    expected: Option<Option<String>>,
) -> Result<ProjectBinding, Error> {
    let v: ProjectBinding = send(c, &r)?;
    let ok = v.checkout.is_absolute()
        && v.checkout.as_os_str().len() <= 4096
        && v.binding.as_ref().is_none_or(|b| {
            oid(&b.workspace_id)
                && id(&b.team_id)
                && opt(&b.team_key, 32)
                && opt(&b.team_name, 256)
                && b.updated_at <= 9_007_199_254_740_991
        })
        && expected
            .as_ref()
            .is_none_or(|e| v.binding.as_ref().map(|b| &b.team_id) == e.as_ref());
    checked(v, ok)
}
pub fn bind_project(
    c: &Client,
    checkout: PathBuf,
    team_id: Option<String>,
    team_key: Option<String>,
    team_name: Option<String>,
) -> Result<ProjectBinding, Error> {
    binding(
        c,
        Request::LinearBindProject {
            checkout,
            team_id: team_id.clone(),
            team_key,
            team_name,
        },
        Some(team_id),
    )
}
pub fn get_project_binding(c: &Client, checkout: PathBuf) -> Result<ProjectBinding, Error> {
    binding(c, Request::LinearGetProjectBinding { checkout }, None)
}
pub fn list_issues(
    c: &Client,
    team_id: String,
    filter: Filter,
    search: Option<String>,
    cursor: Option<String>,
    refresh: bool,
) -> Result<Issues, Error> {
    let v: Issues = send(
        c,
        &Request::LinearListIssues {
            team_id: team_id.clone(),
            filter,
            search,
            cursor,
            refresh,
        },
    )?;
    let ok = cache(&v.cache)
        && opt(&v.next_cursor, 512)
        && v.issues.len() <= 50
        && v.issues.iter().all(|s| summary(s) && s.team_id == team_id);
    checked(v, ok)
}
pub fn get_issue(
    c: &Client,
    issue_id: String,
    comments_cursor: Option<String>,
    refresh: bool,
    workspace_id: Option<String>,
) -> Result<IssueReply<IssueDetail>, Error> {
    let v: IssueReply<IssueDetail> = send(
        c,
        &Request::LinearGetIssue {
            issue_id: issue_id.clone(),
            comments_cursor,
            refresh,
            workspace_id: workspace_id.clone(),
        },
    )?;
    let s = &v.issue.summary;
    let ok = detail(&v.issue)
        && id(&v.workspace_id)
        && workspace_id.is_none_or(|w| w == v.workspace_id)
        && (s.id == issue_id || s.identifier.eq_ignore_ascii_case(&issue_id))
        && cache(&v.cache);
    checked(v, ok)
}
pub fn resolve_issue(
    c: &Client,
    checkout: PathBuf,
    reference: String,
    refresh: bool,
) -> Result<IssueReply<IssueSummary>, Error> {
    let v: IssueReply<IssueSummary> = send(
        c,
        &Request::LinearResolveIssue {
            checkout,
            reference: reference.clone(),
            refresh,
        },
    )?;
    let input = reference.trim();
    let expected = if input.starts_with("https://") {
        input
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .split('/')
            .nth(5)
            .unwrap_or("")
    } else {
        input
    };
    let ok = summary(&v.issue)
        && v.issue.identifier.eq_ignore_ascii_case(expected)
        && id(&v.workspace_id)
        && cache(&v.cache);
    checked(v, ok)
}
pub fn get_linked_issue(
    c: &Client,
    thread_id: String,
    comments_cursor: Option<String>,
    refresh: bool,
) -> Result<IssueReply<IssueDetail>, Error> {
    let v: IssueReply<IssueDetail> = send(
        c,
        &Request::LinearGetLinkedIssue {
            thread_id,
            comments_cursor,
            refresh,
        },
    )?;
    let ok = detail(&v.issue) && id(&v.workspace_id) && cache(&v.cache);
    checked(v, ok)
}
#[allow(clippy::too_many_arguments)]
pub fn link_thread(
    c: &Client,
    thread_id: String,
    workspace_id: Option<String>,
    issue_id: String,
    identifier: String,
    url: String,
    title: String,
) -> Result<LinkReply, Error> {
    let v: LinkReply = send(
        c,
        &Request::LinearLinkThread {
            thread_id: thread_id.clone(),
            workspace_id: workspace_id.clone(),
            issue_id: issue_id.clone(),
            identifier: identifier.clone(),
            url,
            title,
        },
    )?;
    let ok = link(&v.link)
        && v.link.thread_id == thread_id
        && v.link.issue_id == issue_id
        && v.link.identifier == identifier
        && workspace_id.is_none_or(|w| v.link.workspace_id.as_deref() == Some(&w));
    checked(v, ok)
}
fn valid_unlinked(v: &Unlinked, thread_id: &str) -> bool {
    v.thread_id == thread_id && id(&v.thread_id)
}
pub fn unlink_thread(c: &Client, thread_id: String) -> Result<Unlinked, Error> {
    let v: Unlinked = send(
        c,
        &Request::LinearUnlinkThread {
            thread_id: thread_id.clone(),
        },
    )?;
    let ok = valid_unlinked(&v, &thread_id);
    checked(v, ok)
}
pub fn get_thread_link(c: &Client, thread_id: String) -> Result<ThreadLink, Error> {
    let v: ThreadLink = send(
        c,
        &Request::LinearGetThreadLink {
            thread_id: thread_id.clone(),
        },
    )?;
    let ok = v.thread_id == thread_id
        && id(&v.thread_id)
        && v.link
            .as_ref()
            .is_none_or(|l| link(l) && l.thread_id == thread_id);
    checked(v, ok)
}
pub fn list_thread_links(
    c: &Client,
    limit: Option<u16>,
    cursor: Option<String>,
) -> Result<Links, Error> {
    let v: Links = send(c, &Request::LinearListThreadLinks { limit, cursor })?;
    let ok = v.links.len() <= usize::from(limit.unwrap_or(200).min(500))
        && v.links.iter().all(link)
        && opt(&v.next_cursor, 145);
    checked(v, ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn sample() -> Value {
        json!({"id":"ISSUE_ID","identifier":"PLA-142","title":"Fix retry backoff","url":"https://linear.app/acme/issue/PLA-142","priority":2,"updated_at":"2026-11-18T12:00:00.000Z","team_id":"TEAM_ID","state":{"id":"STATE_ID","name":"In Progress","type":"started"},"assignee":{"name":"Alex"},"labels":[{"id":"LABEL_ID","name":"host"}]})
    }
    fn cache() -> Value {
        json!({"cached":false,"stale":false,"cached_at":1763300000000u64,"cache_age_seconds":0,"partial":false,"warning":null})
    }
    fn with_cache(mut v: Value) -> Value {
        v.as_object_mut()
            .unwrap()
            .extend(cache().as_object().unwrap().clone());
        v
    }
    fn example_link() -> Value {
        json!({"thread_id":"THREAD_ID","workspace_id":"ORG_ID","issue_id":"ISSUE_ID","identifier":"PLA-142","url":"https://linear.app/acme/issue/PLA-142","title":"Fix retry backoff","linked_at":1763300000000u64,"historical":false})
    }
    #[test]
    fn representative_replies_decode() {
        let info: Info = serde_json::from_value(json!({"schema":1,"features":{"durable_write_outcomes":true,"exact_issue_resolution":true,"workspace_links":true,"typed_rate_limits":true},"write_history_limit":10000,"write_history_retention":"store_lifetime"})).unwrap();
        assert_eq!(info.schema, 1);
        for _ in 0..3 {
            let _: AuthStatus = serde_json::from_value(json!({"signed_in":true,"status":"signed_in","workspace_id":"ORG_ID","workspace_name":"Acme","workspace_url_key":"acme","viewer_id":"USER_ID","viewer_name":"Alex"})).unwrap();
        }
        let _: Teams = serde_json::from_value(with_cache(json!({"teams":[{"id":"TEAM_ID","key":"PLA","name":"Plait","states":[{"id":"STATE_ID","name":"In Progress","type":"started","position":3}],"states_complete":true}],"teams_complete":true}))).unwrap();
        for _ in 0..2 {
            let _: ProjectBinding = serde_json::from_value(json!({"checkout":"/canonical/checkout","binding":{"workspace_id":"ORG_ID","team_id":"TEAM_ID","team_key":"PLA","team_name":"Plait","updated_at":1763300000000u64}})).unwrap();
        }
        let _: Issues = serde_json::from_value(with_cache(
            json!({"issues":[sample()],"has_more":true,"next_cursor":"CURSOR"}),
        ))
        .unwrap();
        let mut d = sample();
        d.as_object_mut().unwrap().extend(json!({"description":null,"attachments":[],"comments":[],"comments_has_more":false,"comments_next_cursor":null}).as_object().unwrap().clone());
        for _ in 0..2 {
            let v: IssueReply<IssueDetail> =
                serde_json::from_value(with_cache(json!({"issue":d,"workspace_id":"ORG_ID"})))
                    .unwrap();
            assert!(detail(&v.issue));
        }
        let _: IssueReply<IssueSummary> = serde_json::from_value(with_cache(
            json!({"issue":sample(),"workspace_id":"ORG_ID"}),
        ))
        .unwrap();
        let _: LinkReply = serde_json::from_value(json!({"link":example_link()})).unwrap();
        let _: Unlinked =
            serde_json::from_value(json!({"thread_id":"THREAD_ID","removed":true})).unwrap();
        let _: ThreadLink =
            serde_json::from_value(json!({"thread_id":"THREAD_ID","link":example_link()})).unwrap();
        let _: Links =
            serde_json::from_value(json!({"links":[example_link()],"next_cursor":null})).unwrap();
    }
    #[test]
    fn rejects_bad_values() {
        let valid: IssueSummary = serde_json::from_value(sample()).unwrap();
        assert!(summary(&valid));
        for (field, value) in [
            ("title", json!("x".repeat(513))),
            ("url", json!("http://example.com")),
            ("url", json!("https://user:pass@example.com")),
            ("identifier", json!("pla-142")),
            ("labels", json!(vec![json!({"id":"X","name":"n"}); 9])),
        ] {
            let mut v = sample();
            v[field] = value;
            assert!(!summary(&serde_json::from_value(v).unwrap()), "{field}");
        }
        let mut l: Link = serde_json::from_value(example_link()).unwrap();
        assert!(link(&l));
        l.url = "https://".into();
        assert!(!link(&l));
        assert!(checked((), false).is_err());
    }
    #[test]
    fn key_debug_is_redacted_and_requests_are_additive() {
        let r = Request::LinearSetKey {
            key: "secret-value".into(),
        };
        assert!(!format!("{r:?}").contains("secret-value"));
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            json!({"op":"linear_set_key","key":"secret-value"})
        );
    }
    #[test]
    fn typed_errors_are_classified() {
        for (code, expected) in [
            ("invalid_request", Rejection::NeedsNewerHost),
            ("linear_offline", Rejection::Offline),
            ("linear_rate_limited", Rejection::RateLimited(Some(12))),
            ("conflict", Rejection::Conflict),
            ("not_found", Rejection::NotFound),
            ("linear_key_rejected", Rejection::KeyRejected),
            ("linear_not_signed_in", Rejection::SignedOut),
        ] {
            let e: super::super::HostError = serde_json::from_value(
                json!({"code":code,"message":"fixed","retry_after_seconds":12}),
            )
            .unwrap();
            assert_eq!(classify(&Error::Rejected(e), true), expected);
        }
    }

    #[test]
    fn existing_host_errors_still_decode_and_probes_are_explicit() {
        // Every op shares HostError: a reply without the new field must decode.
        let plain: super::super::HostError =
            serde_json::from_value(json!({"code":"invalid_request","message":"fixed"})).unwrap();
        assert_eq!(plain.retry_after_seconds, None);
        // Outside a capability probe, a refused request is not an older host.
        assert_eq!(
            classify(&Error::Rejected(plain.clone()), false),
            Rejection::Other
        );
        assert_eq!(
            classify(&Error::Rejected(plain), true),
            Rejection::NeedsNewerHost
        );
        assert_eq!(
            classify(&Error::Transport("down".into()), true),
            Rejection::Other
        );
    }

    #[test]
    fn rejects_a_reply_for_another_thread() {
        let v: Unlinked =
            serde_json::from_value(json!({"thread_id":"OTHER","removed":true})).unwrap();
        assert!(!valid_unlinked(&v, "THREAD_ID"));
        assert!(valid_unlinked(&v, "OTHER"));
    }
}
