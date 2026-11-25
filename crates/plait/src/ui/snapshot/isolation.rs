//! Checkout write-exclusion scene (`isolation-wait`) and desktop non-blocking render tests.

use super::Harness;
use crate::ui::Workspace;
use plait_client::{host::ThreadStatus, model::DiffMode};
use std::path::Path;

const CHECKOUT_BUSY: &str = "This checkout is in use by an active run or another Plait write. Wait until it finishes, then try again.";

/// Captures `isolation-wait`: Changes view when a desktop Git mutation is
/// refused because an active run holds the checkout lease. No new control.
pub(super) fn walk(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut harness = Harness::new(repository, output, width, height);
    harness.connect(repository, false);
    harness.update(|plait, window, cx| {
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
        plait.show_tab(Workspace::Changes, window, cx);
        plait.select_file(1, DiffMode::Unstaged, window, cx);
        plait.state.repository.write_mut().note = Some((CHECKOUT_BUSY.into(), true));
    });
    harness.capture(&format!("isolation-wait-{suffix}"));
    harness.change_snapshot(|snapshot| snapshot.status = ThreadStatus::Stopped);
    harness.update(|plait, _, _| {
        plait.refresh_tab_rows();
        plait.cleanup.key = Some((
            plait.host.connection.epoch,
            plait.host.creation.generation,
            plait.host.creation.checkout.clone(),
        ));
        plait.cleanup.value = Some(plait_protocol::lease::cleanup::Cleanup {
            process: None,
            token: "sample-cleanup".into(),
            reason: "Codex did not confirm its turn ended and its app-server has not exited."
                .into(),
        });
        plait.state.repository.write_mut().note =
            Some((plait.cleanup.value.as_ref().unwrap().message(), true));
    });
    harness.capture(&format!("isolation-cleanup-uncertain-{suffix}"));

    let mut creation = Harness::new(repository, output, width, height);
    creation.connect(repository, false);
    creation.update(|plait, _, _| {
        plait.worktree_busy = true;
        plait.notice = Some("Creating worktree… You can keep working in another tab.".into());
    });
    creation.capture(&format!("isolation-create-worktree-{suffix}"));
    creation.update(|plait, _, _| {
        plait.worktree_busy = false;
        plait.notice = Some("Worktree creation was interrupted. Existing files and branch were preserved; inspect them before making a new request.".into());
    });
    creation.capture(&format!("isolation-create-failed-{suffix}"));
    creation.update(|plait, _, cx| {
        let path = repository.with_extension("worktrees").join("123456781234");
        plait.preferences.value.navigation.remember_worktree(
            path.clone(),
            repository.into(),
            "plait/123456781234".into(),
        );
        let model = plait.host.connection.threads()[0].model.clone();
        plait
            .host
            .connection
            .threads_mut()
            .push(plait_client::host::ThreadSummary {
                id: "isolated-background".into(),
                checkout: path,
                model,
                title: Some("New worktree thread".into()),
                harness: Some(plait_client::host::harness::HarnessId::Codex),
                ..Default::default()
            });
        plait.rebuild_projects(cx);
        plait.notice =
            Some("Worktree thread created in the background. Open it from the sidebar.".into());
    });
    creation.capture(&format!("isolation-created-in-background-{suffix}"));
    let mut busy = Harness::new(repository, output, width, height);
    busy.connect(repository, false);
    busy.update(|plait, window, cx| {
        plait.admission.queue = true;
        plait.open_draft(window, cx);
        // The choice follows a Send, so the draft always holds its message.
        let composer = plait.draft.composer.clone().unwrap();
        composer.update(cx, |composer, cx| {
            composer.restore_draft(
                "Add a retry limit to the status refresh".into(),
                1,
                None,
                cx,
            )
        });
        super::many_drafts::set_pending(plait, "choice");
        let pending = plait.draft.pending.as_mut().unwrap();
        pending.creating = false;
        pending.choice = true;
    });
    busy.capture(&format!("isolation-busy-choice-{suffix}"));
    let mut many = Harness::new(repository, output, width, height);
    many.connect(repository, false);
    super::tabs::install(&mut many, 20);
    many.update(|plait, _, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 6;
        plait.tabs.model = plait_client::tabs::WindowTabs::new(crate::ui::tabs::fresh_window_id());
        let mut first = None;
        let epoch = plait.host.connection.epoch;
        for (i, thread) in plait.host.connection.threads_mut().iter_mut().enumerate() {
            if i > 0 {
                thread.checkout = repository
                    .with_extension("worktrees")
                    .join(format!("run-{i}"));
            }
            thread.status = ThreadStatus::Running;
            thread.pending_approval = Some(false);
            if i > 0 {
                plait.preferences.value.navigation.remember_worktree(
                    thread.checkout.clone(),
                    repository.into(),
                    format!("plait/run-{i}"),
                );
            }
            let tab = plait
                .tabs
                .model
                .open(
                    plait_client::tabs::TabTarget::Existing {
                        id: thread.id.clone(),
                        checkout: plait_client::tabs::CheckoutKey {
                            host: plait.tabs.host.clone(),
                            root: thread.checkout.clone(),
                            directory: None,
                        },
                    },
                    Some(epoch),
                )
                .unwrap();
            first.get_or_insert(tab);
        }
        plait.tabs.model.activate(first.unwrap()).unwrap();
        plait.refresh_tab_rows();
        plait.rebuild_projects(cx);
    });
    many.capture(&format!("isolation-twenty-running-{suffix}"));
    many.change_snapshot(|snapshot| {
        snapshot.live_text.clear();
        snapshot.live_reasoning.clear();
        for tool in &mut snapshot.tools {
            tool.status = plait_client::host::ToolStatus::Done;
        }
    });
    many.update(|plait, _, _| {
        plait.admission.preparing = Some((
            super::THREAD.into(),
            Some(plait_client::host::admission::Phase::ResourceWait),
        ));
    });
    many.capture(&format!("isolation-resource-wait-{suffix}"));
    let mut queue = Harness::new(repository, output, width, height);
    queue.connect(repository, false);
    for (name, state) in [
        (
            "isolation-queued",
            Some(plait_client::host::checkout_queue::State::Waiting),
        ),
        (
            "isolation-queue-held",
            Some(plait_client::host::checkout_queue::State::Held),
        ),
        ("isolation-queue-cancelled", None),
    ] {
        install_queue(&mut queue, state);
        queue.capture(&format!("{name}-{suffix}"));
    }
}

/// Queue facts are injected only into this test scene, just as catalog events arrive.
pub(super) fn install_queue(
    harness: &mut Harness,
    state: Option<plait_client::host::checkout_queue::State>,
) {
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Idle;
        snapshot.error = None;
    });
    harness.update(|plait, _, cx| {
        plait.admission.queue = true;
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 6;
        let thread = plait
            .host
            .connection
            .threads_mut()
            .iter_mut()
            .find(|t| t.id == super::THREAD)
            .unwrap();
        thread.status = ThreadStatus::Idle;
        thread.pending_approval = Some(false);
        thread.checkout_queue = state.map(|state| {
            plait_client::host::checkout_queue::Summary::new("queued-first".into(), state, 3, 1)
        });
        if state.is_none() {
            plait.set_thread_notice(
                super::THREAD.into(),
                "Cancelled. Your waiting message will not run.".into(),
                false,
            );
        }
        plait.rebuild_projects(cx);
    });
}

/// A slow host write must not block drawing, and its refusal reaches the view.
#[core::prelude::v1::test]
fn host_write_wait_does_not_block_desktop_render() {
    if !super::isolated("isolation::host_write_wait_does_not_block_desktop_render") {
        return;
    }
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let (scratch, _, mut harness) = super::undo::setup("isolation-host-write");
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let host = super::undo::Host::start(move |request| {
        if request["op"] == "git_write" {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Some(serde_json::json!({"ok":false,"error":{"code":"busy","message":CHECKOUT_BUSY}}))
        } else {
            Some(
                serde_json::json!({"ok":false,"error":{"code":"unavailable","message":"Unavailable in test"}}),
            )
        }
    });
    // GPUI's `TestDispatcher` runs background tasks inline on this thread during
    // `run_until_parked()`, so queue the write and draw every view before
    // pumping the background executor; if `git_write`, `show_tab`, or `draw`
    // waited on the host synchronously, they would block on `release_rx`.
    harness
        .window
        .update(&mut harness.cx, |p, window, cx| {
            p.show_tab(Workspace::Changes, window, cx);
            p.git_write(plait_protocol::git::WriteOp::StageAll, cx);
            assert!(p.state.repository.write().busy(), "write must be in flight");
        })
        .unwrap();
    for view in [
        Workspace::Conversation,
        Workspace::Changes,
        Workspace::History,
        Workspace::Files,
    ] {
        let start = Instant::now();
        harness
            .window
            .update(&mut harness.cx, |p, window, cx| {
                p.show_tab(view, window, cx)
            })
            .unwrap();
        harness.draw();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "render blocked on {view:?}"
        );
    }
    release_tx.send(()).unwrap();
    super::undo::wait(&mut harness, |p| !p.state.repository.write().busy());
    assert!(started_rx.try_recv().is_ok(), "write did not reach host");
    harness.update(|p, _, _| {
        let note = p.state.repository.write().note.as_ref().unwrap();
        assert!(note.1 && note.0.contains(CHECKOUT_BUSY), "{note:?}");
    });
    assert!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["op"] == "git_write")
    );
    drop(harness);
    drop(host);
    std::fs::remove_dir_all(scratch).unwrap();
}
