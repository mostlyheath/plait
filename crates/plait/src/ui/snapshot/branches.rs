//! The branch control and its palette: scenes, and tests that drive them
//! against real repositories.

use super::{Harness, fixture_repository, git, isolated, press, set_query, shown};
use crate::ui::{Plait, Workspace};
use plait_client::{
    git::{Branch, Branches, WriteOp},
    host::ThreadStatus,
    state::Load,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// A scratch repository: the shared fixture plus branches beside `main`.
fn repository(name: &str) -> (PathBuf, PathBuf) {
    let scratch = std::env::temp_dir().join(format!("plait-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    for branch in ["feature/login", "fix/status-race"] {
        git(&repository, &["branch", branch]);
    }
    (scratch, repository)
}

/// Run queued Git work until `done` holds.
fn wait(harness: &mut Harness, done: impl Fn(&Plait) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.settle();
        let ready = harness
            .window
            .update(&mut harness.cx, |plait, _, _| done(plait))
            .expect("window update");
        if ready {
            return;
        }
        assert!(Instant::now() < deadline, "Git did not answer");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn branch_now(plait: &Plait) -> Option<String> {
    let snapshot = plait.state.repository.snapshot().ready()?;
    Some(snapshot.branch.clone())
}

fn open(harness: &mut Harness, create: bool) {
    harness.update(move |plait, window, cx| plait.open_branches(create, window, cx));
    wait(harness, |plait| {
        plait
            .overlay
            .as_ref()
            .is_some_and(|overlay| !overlay.branches)
    });
}

/// The palette lists real local branches with the current one marked, offers
/// Create only for a name that is not a branch, and runs what is chosen.
#[core::prelude::v1::test]
fn the_branch_palette_lists_creates_and_switches() {
    if !isolated("branches::the_branch_palette_lists_creates_and_switches") {
        return;
    }
    let (scratch, repository) = repository("branch-palette");
    let mut harness = Harness::new(&repository, None, 900., 600.);
    open(&mut harness, false);
    let mut listed = shown(&mut harness);
    listed.sort();
    assert_eq!(listed, ["feature/login", "fix/status-race", "main"]);
    harness.update(|plait, _, _| {
        let (items, _, _) = plait.branch_items();
        let main = items
            .iter()
            .find(|item| item.title.as_ref() == "main")
            .unwrap();
        assert!(
            main.badge.is_some() && !main.enabled,
            "current, not choosable"
        );
    });
    // An existing name offers no Create item; a new one is offered first.
    set_query(&mut harness, "feature/login");
    assert_eq!(shown(&mut harness), ["feature/login"]);
    set_query(&mut harness, "login");
    assert_eq!(
        shown(&mut harness),
        ["Create branch login", "feature/login"]
    );
    // Part of a listed name is a search: Enter switches to the match.
    press(&mut harness, "enter");
    wait(&mut harness, |plait| {
        branch_now(plait).as_deref() == Some("feature/login")
    });
    harness.update(|plait, _, _| {
        assert_eq!(
            plait.selected_view(),
            Workspace::Changes,
            "where the result is shown"
        );
        assert_eq!(
            plait.state.repository.write().note,
            Some(("Switched to feature/login".into(), false))
        );
    });
    // A name nothing contains: Enter creates it from the current commit.
    open(&mut harness, false);
    set_query(&mut harness, "release/2026-11");
    assert_eq!(shown(&mut harness)[0], "Create branch release/2026-11");
    press(&mut harness, "enter");
    wait(&mut harness, |plait| {
        branch_now(plait).as_deref() == Some("release/2026-11")
    });
    // New Branch… creates even when the name is part of a branch.
    open(&mut harness, true);
    set_query(&mut harness, "login");
    assert_eq!(shown(&mut harness)[0], "Create branch login");
    press(&mut harness, "enter");
    wait(&mut harness, |plait| {
        branch_now(plait).as_deref() == Some("login")
    });
    // An invalid name is refused by Git's check, in the result line.
    open(&mut harness, true);
    set_query(&mut harness, "two words");
    press(&mut harness, "enter");
    wait(&mut harness, |plait| {
        plait.state.repository.write().note.is_some()
    });
    harness.update(|plait, _, _| {
        let (note, failed) = plait.state.repository.write().note.clone().unwrap();
        assert!(failed && note.contains("not a valid branch name"), "{note}");
        assert_eq!(branch_now(plait).as_deref(), Some("login"));
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// No branch write starts while another Git write runs, or while a thread is
/// running in this checkout. The palette still opens and says why.
#[core::prelude::v1::test]
fn a_branch_write_is_refused_while_git_or_a_thread_is_busy_here() {
    if !isolated("branches::a_branch_write_is_refused_while_git_or_a_thread_is_busy_here") {
        return;
    }
    let (scratch, repository) = repository("branch-refused");
    let mut harness = Harness::new(&repository, None, 900., 600.);
    // Another write queued: refused until it finishes, checked in one turn.
    harness.update(|plait, window, cx| {
        assert!(plait.branch_blocker().is_none());
        assert!(plait.state.repository.request_write(WriteOp::Fetch));
        let reason = plait.branch_blocker().expect("refused while Git works");
        assert!(reason.contains("Git is still working"));
        let (items, _, _) = plait.branch_items();
        assert!(items.iter().all(|item| !item.enabled));
        plait.branch_write(WriteOp::SwitchBranch("feature/login".into()), window, cx);
        let (note, failed) = plait.state.repository.write().note.clone().unwrap();
        assert!(failed && note.contains("Git is still working"), "{note}");
        // Only the fetch is queued; let it run.
        plait.pump(cx);
    });
    wait(&mut harness, |plait| !plait.state.repository.write().busy());
    // A thread running in this checkout: refused, and the result line says why.
    harness.connect(&repository, false);
    harness.update(|plait, _, _| {
        let root = plait
            .state
            .repository
            .snapshot()
            .ready()
            .unwrap()
            .root
            .clone();
        plait.host.connection.threads_mut()[0].checkout = root;
        plait.host.connection.threads_mut()[0].status = ThreadStatus::Running;
    });
    open(&mut harness, false);
    harness.update(|plait, window, cx| {
        let reason = plait
            .branch_blocker()
            .expect("refused under a running thread");
        assert!(reason.contains("thread is running"), "{reason}");
        let (items, _, _) = plait.branch_items();
        assert!(!items.is_empty() && items.iter().all(|item| !item.enabled));
        plait.close_overlay(true, window, cx);
    });
    harness.update(|plait, window, cx| {
        plait.branch_write(WriteOp::SwitchBranch("feature/login".into()), window, cx);
        let (note, failed) = plait.state.repository.write().note.clone().unwrap();
        assert!(failed && note.contains("thread is running"), "{note}");
        assert!(!plait.state.repository.write().busy(), "nothing was queued");
    });
    harness.update(|plait, _, _| assert_eq!(branch_now(plait).as_deref(), Some("main")));
    // The thread in another checkout, such as its own worktree, does not stop it.
    harness.update(|plait, _, _| {
        plait.host.connection.threads_mut()[0].checkout = PathBuf::from("/elsewhere");
        assert!(plait.branch_blocker().is_none());
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Sample branches for scenes, so ages and names do not depend on the fixture.
fn sample_branches() -> Arc<Branches> {
    let now = super::super::commits::now();
    let branch = |name: &str, current: bool, ago: i64, elsewhere: Option<&str>| Branch {
        name: name.into(),
        current,
        time: now - ago,
        elsewhere: elsewhere.map(Into::into),
    };
    Arc::new(Branches {
        list: vec![
            branch("main", true, 120, None),
            branch("feature/retry-status-refresh", false, 3 * 3_600, None),
            branch(
                "plait/4f81c2",
                false,
                26 * 3_600,
                Some("/Users/sample/plait.worktrees/4f81c2"),
            ),
            branch("fix/inspector-width", false, 4 * 86_400, None),
            branch(
                "spike/frame-budget-tracing-for-long-conversations",
                false,
                40 * 86_400,
                None,
            ),
        ],
        more: false,
    })
}

/// Fill the open branch palette with the sample list.
fn show_sample(harness: &mut Harness) {
    harness.update(|plait, _, cx| {
        plait.state.repository.data.branches = Load::Ready(sample_branches());
        let (items, _, note) = plait.branch_items();
        let note = plait.branch_note(note);
        let palette = plait
            .overlay
            .as_ref()
            .expect("open palette")
            .palette
            .clone();
        palette.update(cx, |palette, cx| {
            // Emptied first, so the selection starts afresh as on a first list.
            palette.set_items(Vec::new(), cx);
            palette.set_items(items, cx);
            palette.set_note(note, cx);
        });
    });
}

/// The palette over Changes, a typed new name, and the refused state.
pub(super) fn branches_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    // No thread is running here, so the control is ready.
    harness.change_snapshot(|snapshot| snapshot.status = ThreadStatus::Idle);
    harness.update(|plait, window, cx| {
        for thread in plait.host.connection.threads_mut() {
            thread.status = ThreadStatus::Idle;
        }
        plait.show_tab(Workspace::Changes, window, cx);
    });
    harness.capture(&format!("branch-control-{suffix}"));
    open(harness, false);
    show_sample(harness);
    harness.capture(&format!("branches-{suffix}"));
    set_query(harness, "feature/queue-backoff");
    harness.capture(&format!("branches-create-{suffix}"));
    // A thread running in this checkout: the control is locked and the
    // palette names the reason; nothing can be chosen.
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        let root = plait
            .state
            .repository
            .snapshot()
            .ready()
            .unwrap()
            .root
            .clone();
        plait.host.connection.threads_mut()[0].checkout = root;
    });
    // The header and the sidebar both show the thread running.
    harness.change_snapshot(|snapshot| snapshot.status = ThreadStatus::Running);
    harness.capture(&format!("branch-control-refused-{suffix}"));
    open(harness, false);
    show_sample(harness);
    harness.capture(&format!("branches-refused-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.state.repository.data.branches = Load::Empty;
        plait.workspace.tab = Workspace::Conversation;
    });
}
