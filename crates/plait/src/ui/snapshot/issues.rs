//! The Issues tab with Linear state injected: no scene reaches Linear.
use super::*;
use plait_client::{
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
        // Long enough to overflow the list column unless it truncates.
        issue("i4", "PLA-7", "The three-dot menu opens at the bottom of the left sidebar; it should open as a small popover beside the row instead", "Todo"),
    ], "has_more": true, "next_cursor": "C"});
    v.as_object_mut()
        .unwrap()
        .extend(cache(stale).as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

/// A description written the way Linear's editor saves one: Markdown with
/// headings, a list, code, a link and an uploaded image.
const DESCRIPTION: &str = "Requests that get a **429** fail at once, and the thread only says *failed*.\n\n## What should happen\n\n- Retry with exponential backoff, at most three times.\n- Honour `Retry-After` when the server sends it.\n- Say in the thread that a retry is waiting.\n\n```rust\nlet delay = base * 2u32.pow(attempt);\n```\n\n> The client already reads the header; nothing uses it yet.\n\n![Failure in the thread](https://uploads.linear.app/acme/ab12/failure.png)\n\nSee the [rate-limit guide](https://linear.app/developers/rate-limiting) for the numbers.";

fn detail() -> IssueReply<IssueDetail> {
    let mut d = issue(
        "i1",
        "PLA-142",
        "Retry rate-limited requests with backoff",
        "In Progress",
    );
    d.as_object_mut().unwrap().extend(serde_json::json!({
        "description": DESCRIPTION,
        "attachments": [{"id": "a", "title": "Failure log", "url": "https://example.com/log"}],
        "comments": [{"id": "c", "body": "The client already reads `Retry-After`. **Keep** the cap at three.", "created_at": "2026-11-17T09:00:00.000Z", "updated_at": "2026-11-17T09:00:00.000Z", "author": "Sam"}],
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
            view.writes = Some(true);
            view.states = Some(states());
        }
    });
}

fn states() -> Vec<plait_client::host::linear::WorkflowState> {
    [
        ("s-backlog", "Backlog", "backlog", 0.),
        ("s-todo", "Todo", "unstarted", 1.),
        ("s", "In Progress", "started", 2.),
        ("s-done", "Done", "completed", 3.),
    ]
    .into_iter()
    .map(|(id, name, kind, position)| {
        serde_json::from_value(
            serde_json::json!({"id": id, "name": name, "type": kind, "position": position}),
        )
        .unwrap()
    })
    .collect()
}

/// A status change to Done that ended `status`, beneath the issue's details.
fn changed(h: &mut Harness, status: plait_client::linear_issues::ChangeStatus) {
    linked(h, false, true);
    h.update(|p, _, _| {
        p.issues.view.state_change = Some(plait_client::linear_issues::StateChange {
            issue_id: "i1".into(),
            state_id: "s-done".into(),
            state_name: "Done".into(),
            request_id: "desktop-1".into(),
            status,
        });
    });
}

type Scene = (&'static str, fn(&mut Harness), &'static [&'static str]);

fn scenes() -> [Scene; 7] {
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
            &[
                "Open in Linear",
                "Start thread",
                "Add to thread…",
                "Status: In Progress",
            ],
        ),
        (
            "issues-status-sending",
            |h| changed(h, plait_client::linear_issues::ChangeStatus::Sending),
            &["Status: Moving to Done…"],
        ),
        (
            "issues-status-failed",
            |h| {
                changed(
                    h,
                    plait_client::linear_issues::ChangeStatus::Failed {
                        note: "The status was not changed. Linear could not be reached. Try again when online.".into(),
                        retry: true,
                    },
                )
            },
            &["Try again", "Dismiss", "Status: In Progress"],
        ),
        (
            "issues-status-unknown",
            |h| changed(h, plait_client::linear_issues::ChangeStatus::Unknown),
            &["Dismiss", "Status: In Progress"],
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
        capture_both(&mut h, &format!("{name}-{suffix}"));
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
            plait_client::tabs::bookmark::BookmarkRecord::Issues { selected: Some(id) } if id == "i2"
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

/// Connected with no tab open and no thread selected, as on a fresh launch.
fn fresh_launch(h: &mut Harness) {
    h.update(|p, _, cx| {
        p.park_workspace(cx);
        p.tabs = Default::default();
        p.host.thread.selected = None;
        p.host.thread.snapshot = None;
        p.refresh_tab_rows();
    });
    h.settle();
}

/// Opens Issues the way the sidebar, palette and shortcut do.
fn open_from_menu(h: &mut Harness) {
    h.update(|p, window, cx| {
        let root = p.host.creation.checkout.clone();
        p.open_issues(root, window, cx);
    });
    h.settle();
}

fn assert_issues_drawn(h: &mut Harness, case: &str) {
    h.update(|p, _, _| assert!(p.issues_visible(), "{case}: Issues is the active tab"));
    let frame = h.draw_a11y();
    let labels: Vec<_> = frame.nodes.iter().filter_map(|n| n.label.clone()).collect();
    assert!(
        labels.iter().any(|label| label == "Search issues"),
        "{case}: the Issues tab is drawn; drawn: {labels:?}"
    );
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn opening_issues_shows_them_at_once() {
    if !isolated("issues::opening_issues_shows_them_at_once") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-issues-open-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let out = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    let mut h = Harness::new(&repo, out.as_deref(), 1380., 860.);
    h.connect(&repo, false);
    fresh_launch(&mut h);
    open_from_menu(&mut h);
    linked(&mut h, false, false);
    h.capture("issues-fresh-launch");
    assert_issues_drawn(&mut h, "fresh launch");

    let mut h = Harness::new(&repo, out.as_deref(), 1380., 860.);
    h.connect(&repo, false);
    open_from_menu(&mut h);
    linked(&mut h, false, false);
    h.capture("issues-from-thread");
    assert_issues_drawn(&mut h, "thread open");
    std::fs::remove_dir_all(scratch).unwrap();
}
