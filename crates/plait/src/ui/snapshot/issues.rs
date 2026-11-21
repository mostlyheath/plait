//! The Issues tab with Linear state injected: no scene reaches Linear.
use super::*;
use plait_core::{
    host::linear::{IssueDetail, IssueReply, Issues},
    linear_settings::Auth,
    tabs::TabTarget,
};

fn issue(id: &str, identifier: &str, title: &str, state: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "identifier": identifier, "title": title,
        "url": format!("https://linear.app/acme/issue/{identifier}"), "priority": 2,
        "updated_at": "2026-11-18T12:00:00.000Z", "team_id": "t-plait",
        "state": {"id": "s", "name": state, "type": "started"},
        "assignee": {"name": "Alex"}, "labels": [{"id": "l", "name": "host"}]})
}

fn cache(stale: bool) -> serde_json::Value {
    serde_json::json!({"cached": stale, "stale": stale, "cached_at": 0,
        "cache_age_seconds": if stale { 5400 } else { 0 }, "partial": false, "warning": null})
}

fn page(stale: bool) -> Issues {
    let mut v = serde_json::json!({"issues": [
        issue("i1", "PLA-142", "Retry rate-limited requests with backoff", "In Progress"),
        issue("i2", "PLA-139", "Keep image attachments across a host restart", "Todo"),
        issue("i3", "PLA-131", "Explain a rejected Linear key in Settings", "In Review"),
    ], "has_more": true, "next_cursor": "C"});
    v.as_object_mut()
        .unwrap()
        .extend(cache(stale).as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

fn detail() -> IssueReply<IssueDetail> {
    let mut d = issue(
        "i1",
        "PLA-142",
        "Retry rate-limited requests with backoff",
        "In Progress",
    );
    d.as_object_mut().unwrap().extend(serde_json::json!({
        "description": "Requests that get a 429 fail at once.\n\nRetry them with exponential backoff, at most three times, and say so in the thread.",
        "attachments": [{"id": "a", "title": "Failure log", "url": "https://example.com/log"}],
        "comments": [{"id": "c", "body": "The client already reads Retry-After.", "created_at": "2026-11-17T09:00:00.000Z", "updated_at": "2026-11-17T09:00:00.000Z", "author": "Sam"}],
        "comments_has_more": false, "comments_next_cursor": null}).as_object().unwrap().clone());
    let mut v = serde_json::json!({"issue": d, "workspace_id": "ORG"});
    v.as_object_mut()
        .unwrap()
        .extend(cache(false).as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

/// Opens the Issues tab for the fixture project and applies `install`.
fn open(h: &mut Harness) {
    h.update(|p, _, cx| {
        p.park_workspace(cx);
        p.settings_open = false;
        let root = p.host.creation.checkout.clone();
        let tab = p
            .tabs
            .model
            .open(
                TabTarget::Issues {
                    checkout: p.tab_checkout(root),
                },
                Some(p.host.connection.epoch),
            )
            .unwrap();
        p.tabs.model.activate(tab).unwrap();
        p.refresh_tab_rows();
    });
    h.settle();
}

fn linked(h: &mut Harness, stale: bool, selected: bool) {
    h.update(|p, _, _| {
        p.linear.account.auth = Auth::SignedIn {
            workspace: "Acme".into(),
            viewer: "Alex".into(),
        };
        let view = &mut p.issues.view;
        view.binding = Some(Some(
            serde_json::from_value(
                serde_json::json!({"workspace_id": "ORG", "team_id": "t-plait",
                "team_key": "PLA", "team_name": "Plait", "updated_at": 1}),
            )
            .unwrap(),
        ));
        let page = page(stale);
        view.issues = page.issues;
        view.has_more = page.has_more;
        view.next_cursor = page.next_cursor;
        view.cache = Some(page.cache);
        if selected {
            view.selected = Some("i1".into());
            view.detail = Some(detail());
        }
    });
}

type Scene = (&'static str, fn(&mut Harness), &'static [&'static str]);

fn scenes() -> [Scene; 4] {
    [
        (
            "issues-signed-out",
            |h| h.update(|p, _, _| p.linear.account.auth = Auth::SignedOut),
            &["Linear settings…"],
        ),
        (
            "issues-unlinked",
            |h| {
                h.update(|p, _, _| {
                    p.linear.account.auth = Auth::SignedIn {
                        workspace: "Acme".into(),
                        viewer: "Alex".into(),
                    };
                    p.issues.view.binding = Some(None);
                })
            },
            &["Project settings…"],
        ),
        (
            "issues-list",
            |h| linked(h, true, false),
            &[
                "Search issues",
                "Active",
                "PLA-142 Retry rate-limited requests with backoff, In Progress",
                "Load more",
                "Refresh",
            ],
        ),
        (
            "issues-detail",
            |h| linked(h, false, true),
            &["Open in Linear", "Start thread", "Add to thread…"],
        ),
    ]
}

fn thread_from_issue(h: &mut Harness) {
    h.update(|p, _, _| {
        let id = p.host.thread.selected.clone().expect("a thread is open");
        p.issues.links.insert(
            id.clone(),
            serde_json::from_value(serde_json::json!({"thread_id": id, "workspace_id": "ORG",
                "issue_id": "i1", "identifier": "PLA-142", "url": "https://linear.app/acme/issue/PLA-142",
                "title": "Retry rate-limited requests with backoff", "linked_at": 1, "historical": false}))
            .unwrap(),
        );
    });
}

pub(super) fn walk(repo: &Path, out: Option<&Path>, suffix: &str, width: f32, height: f32) {
    let mut h = Harness::new(repo, out, width, height);
    h.connect(repo, false);
    thread_from_issue(&mut h);
    h.capture(&format!("thread-from-issue-{suffix}"));
    for (name, install, _) in scenes() {
        let mut h = Harness::new(repo, out, width, height);
        h.connect(repo, false);
        open(&mut h);
        install(&mut h);
        h.capture(&format!("{name}-{suffix}"));
    }
}

/// Only the Issues scenes, for reviewing their layout quickly.
#[core::prelude::v1::test]
#[ignore = "writes PNG files; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn issues_snapshots() {
    if !isolated("issues::issues_snapshots") {
        return;
    }
    let output = PathBuf::from(std::env::var_os("PLAIT_SNAPSHOT_DIR").expect("PLAIT_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&output).unwrap();
    let scratch = std::env::temp_dir().join(format!("plait-issues-shots-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    walk(&repo, Some(&output), "wide", 1380., 860.);
    walk(&repo, Some(&output), "narrow", 900., 600.);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn issues_tab_is_labelled_and_keeps_its_selection() {
    if !isolated("issues::issues_tab_is_labelled_and_keeps_its_selection") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-issues-scenes-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    for (width, height) in [(1380., 860.), (900., 600.)] {
        for (name, install, required) in scenes() {
            let mut h = Harness::new(&repo, None, width, height);
            h.connect(&repo, false);
            open(&mut h);
            install(&mut h);
            let frame = h.draw_a11y();
            frame.assert_no_duplicates(name);
            for required in required {
                assert!(
                    frame
                        .nodes
                        .iter()
                        .any(|n| n.label.as_deref() == Some(*required)),
                    "{name} missing {required} at {width}"
                );
            }
        }
    }
    // Choosing an issue is saved with the tab, so a restart returns to it.
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    open(&mut h);
    linked(&mut h, false, false);
    h.update(|p, _, cx| {
        p.select_issue("i2", cx);
        let tab = p.tabs.model.active().unwrap();
        assert!(matches!(
            p.tabs.model.get(tab).unwrap().session().bookmark(),
            plait_core::tabs::bookmark::BookmarkRecord::Issues { selected: Some(id) } if id == "i2"
        ));
    });
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn starting_a_thread_from_an_issue_fills_a_draft_and_links_on_creation() {
    if !isolated("issues::starting_a_thread_from_an_issue_fills_a_draft_and_links_on_creation") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-issues-start-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    open(&mut h);
    linked(&mut h, false, true);
    h.update(|p, window, cx| p.start_thread_from_issue(window, cx));
    h.settle();
    h.update(|p, _, cx| {
        let draft = p.draft.id.clone().expect("a new draft opened");
        let issue = p
            .issues
            .pending_links
            .get(&draft)
            .expect("linked once created");
        assert_eq!(issue.identifier, "PLA-142");
        let text = p
            .draft
            .composer
            .as_ref()
            .unwrap()
            .read(cx)
            .text()
            .to_owned();
        assert!(
            text.starts_with("Linear issue PLA-142: Retry rate-limited requests"),
            "{text}"
        );
        // Creation confirmed by the host: the pending link is used once.
        p.link_created_thread(Some(&draft), "thread-x", cx);
        assert!(p.issues.pending_links.is_empty());
    });
    std::fs::remove_dir_all(scratch).unwrap();
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn a_thread_started_from_an_issue_names_it() {
    if !isolated("issues::a_thread_started_from_an_issue_names_it") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-issues-label-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    h.update(|p, _, _| {
        let id = p.host.thread.selected.clone().expect("a thread is open");
        p.issues.links.insert(
            id.clone(),
            serde_json::from_value(serde_json::json!({"thread_id": id, "workspace_id": "ORG",
                "issue_id": "i1", "identifier": "PLA-142", "url": "https://linear.app/acme/issue/PLA-142",
                "title": "Retry rate-limited requests with backoff", "linked_at": 1, "historical": false}))
            .unwrap(),
        );
    });
    h.update(|p, _, cx| assert!(p.issue_link_line(cx).is_some(), "the line is built"));
    // Its Open in Linear control is the only one in a thread view.
    let frame = h.draw_a11y();
    assert!(
        frame
            .nodes
            .iter()
            .any(|n| n.label.as_deref() == Some("Open in Linear")),
        "the source issue line is drawn"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
