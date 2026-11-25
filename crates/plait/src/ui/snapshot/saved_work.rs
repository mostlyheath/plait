//! Saved work per machine entry (#132): one regression per high finding from
//! the reviews of #164 and #174, each asserting the saved text survives a real
//! journal close and reopen. Switches go through `use_machine_client`,
//! reconnects advance the connection epoch, and relaunches reopen the journal
//! from disk.
use super::*;
use crate::ui::{tab_lifecycle::Intent, tabs::TabNavigation};
use plait_client::{
    host::ApprovalMode,
    host_state::PendingSubmit,
    machines::MachineId,
    session::{Draft, DraftTab, Journal, MachineWork, Thread},
    tabs::HostKey,
};

fn host_key(id: &str) -> HostKey {
    HostKey {
        id: id.into(),
        store: "/store".into(),
    }
}

fn remote() -> MachineId {
    MachineId::new("workstation").unwrap()
}

fn local() -> MachineId {
    MachineId::default()
}

fn draft_tab(root: &Path, text: &str) -> DraftTab {
    DraftTab {
        legacy: false,
        creation: None,
        checkout: root.to_owned(),
        draft: Draft {
            text: text.into(),
            revision: 1,
            ..Default::default()
        },
        settings: Default::default(),
    }
}

fn wait(id: &str) -> PendingSubmit {
    PendingSubmit {
        checkout_wait: true,
        images: Vec::new(),
        request_id: id.into(),
        text: "Wait for the checkout".into(),
        approval: None,
        when_busy: None,
        draft_revision: None,
    }
}

fn thread_record(root: &Path, text: &str, waits: &[&str]) -> Thread {
    Thread {
        checkout: root.to_owned(),
        model: choice(),
        title: "Saved".into(),
        draft: Draft {
            text: text.into(),
            revision: 1,
            ..Default::default()
        },
        pending: None,
        pending_submissions: Vec::new(),
        queued_submissions: waits.iter().map(|id| wait(id)).collect(),
        approval: None,
    }
}

fn typed(text: &str, window: &mut Window, cx: &mut Context<Plait>) -> Entity<Composer> {
    let composer = cx.new(|cx| Composer::new(window, cx));
    let text = text.to_owned();
    composer.update(cx, |c, cx| c.restore_draft(text, 2, None, cx));
    composer
}

fn work<'a>(p: &'a Plait, machine: &MachineId) -> &'a MachineWork {
    p.saved_work.machines.get(machine).expect("machine work")
}

/// Put `edit` into `machine`'s saved work, as an earlier session left it.
fn seed(p: &mut Plait, machine: &MachineId, edit: impl FnOnce(&mut MachineWork)) {
    let mut work = p
        .saved_work
        .machines
        .get(machine)
        .cloned()
        .unwrap_or_default();
    edit(&mut work);
    p.store_machine_work([(machine.clone(), work)].into_iter().collect());
}

/// This scene's own saved workspace, beside its repository. The harness's
/// default one stays locked by its quit observer, so it cannot be reopened.
fn state(repo: &Path) -> std::path::PathBuf {
    repo.with_file_name("desktop-state")
}

/// Open the real journal, as a launch would.
fn launch(p: &mut Plait, repo: &Path) {
    let (_, host) = plait_client::session::locations().unwrap();
    let (journal, data) = Journal::open(&state(repo), host).unwrap();
    p.saved_work.machines = journal.take_machines();
    p.session.journal = Some(journal);
    p.session.data = Some(data);
    p.forget_saved_work();
    p.workspace.bindings.clear();
}

/// Quit and relaunch: flush, then rebuild the window's saved work from what
/// is on disk alone, `desktop.json` and each machine's file. (Detached tab
/// writes still hold the journal inside one scene update, so its lock is not
/// taken again here; `an_older_draft_is_copied_then_edited_here` opens it.)
fn relaunch(p: &mut Plait, repo: &Path, cx: &mut Context<Plait>) {
    p.capture_session(cx);
    p.session.journal.as_ref().unwrap().flush().unwrap();
    p.park_workspace(cx);
    p.tabs.lifecycle.drafts.clear();
    p.forget_saved_work();
    p.workspace.bindings.clear();
    let bytes = std::fs::read(state(repo).join("desktop.json")).unwrap();
    p.session.data = Some(serde_json::from_slice(&bytes).unwrap());
    p.saved_work.machines = plait_client::session::read_machines(&state(repo)).unwrap();
}

/// A switch through the real path. The new machine lists THREAD at `root`,
/// as its host would a thread with the same ID and folder.
fn switch(
    p: &mut Plait,
    machine: MachineId,
    local: bool,
    root: &Path,
    window: &mut Window,
    cx: &mut Context<Plait>,
) {
    let client = p.host_client();
    p.use_machine_client(client, machine, local, window, cx);
    p.host.connection.connecting = false;
    p.host.connection.connected = true;
    *p.host.connection.threads_mut() = vec![ThreadSummary {
        id: THREAD.into(),
        checkout: root.to_owned(),
        model: choice(),
        ..Default::default()
    }];
}

fn mount(p: &mut Plait, window: &mut Window, cx: &mut Context<Plait>) -> Entity<Composer> {
    let composer = cx.new(|cx| Composer::new(window, cx));
    p.restore_composer(THREAD, &composer, cx);
    p.workspace
        .composers
        .insert(THREAD.into(), composer.clone());
    composer
}

fn scene(name: &str, body: impl FnOnce(&mut Plait, &Path, &mut Window, &mut Context<Plait>)) {
    let scratch = std::env::temp_dir().join(format!("plait-saved-{name}-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        launch(p, &repo);
        body(p, &repo, window, cx);
        p.session.journal = None;
        p.session.data = None;
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// R1-1: Reopen during a switch opens none of this computer's drafts on the
/// machine switched to; the draft is still there after a relaunch.
#[core::prelude::v1::test]
fn r1_1_reopen_during_a_switch_opens_no_other_machines_draft() {
    if !isolated("saved_work::r1_1_reopen_during_a_switch_opens_no_other_machines_draft") {
        return;
    }
    scene("r1-1", |p, repo, window, cx| {
        seed(p, &local(), |w| {
            w.drafts
                .insert("older".into(), draft_tab(repo, "this computer's draft"));
        });
        switch(p, remote(), false, repo, window, cx);
        assert!(p.saved_drafts().is_empty());
        p.request_tab(Intent::Navigate(TabNavigation::Reopen), window, cx);
        assert!(
            !p.tabs
                .model
                .tabs()
                .iter()
                .any(|t| t.session().target().is_draft())
        );
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local()).drafts["older"].draft.text,
            "this computer's draft"
        );
        assert!(
            p.saved_work
                .machines
                .get(&remote())
                .is_none_or(|w| w.drafts.is_empty())
        );
    });
}

/// R1-2: a thread with the same ID and folder on another machine neither
/// loads nor forgets this machine's waiting message.
#[core::prelude::v1::test]
fn r1_2_another_machines_same_thread_keeps_this_machines_wait() {
    if !isolated("saved_work::r1_2_another_machines_same_thread_keeps_this_machines_wait") {
        return;
    }
    scene("r1-2", |p, repo, window, cx| {
        seed(p, &local(), |w| {
            w.put_thread(
                THREAD,
                thread_record(repo, "A's text", &["w-a"]),
                Some(fixture_host()),
            )
        });
        p.capture_session(cx);
        assert_eq!(p.host.submissions.accepted(THREAD).len(), 1);
        switch(p, remote(), false, repo, window, cx);
        p.verify_identity_for_test(host_key("workstation-host"));
        p.capture_session(cx);
        assert!(p.host.submissions.accepted(THREAD).is_empty());
        relaunch(p, repo, cx);
        let a = &work(p, &local()).thread(THREAD, repo).unwrap().thread;
        assert_eq!(a.draft.text, "A's text");
        assert_eq!(a.queued_submissions[0].request_id, "w-a");
    });
}

/// R1-3: before B is verified, A's text for the same thread is not shown in
/// B's composer, and what is typed there does not replace it.
#[core::prelude::v1::test]
fn r1_3_another_machine_cannot_read_or_replace_this_machines_text() {
    if !isolated("saved_work::r1_3_another_machine_cannot_read_or_replace_this_machines_text") {
        return;
    }
    scene("r1-3", |p, repo, window, cx| {
        seed(p, &local(), |w| {
            w.put_thread(THREAD, thread_record(repo, "A's text", &[]), None)
        });
        switch(p, remote(), false, repo, window, cx);
        let composer = mount(p, window, cx);
        assert_eq!(composer.read(cx).saved_draft().0, "");
        composer.update(cx, |c, cx| c.restore_draft("B's text".into(), 2, None, cx));
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local())
                .thread(THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "A's text"
        );
        assert_eq!(
            work(p, &remote())
                .thread(THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "B's text"
        );
    });
}

/// R1-4: a draft typed on a remote machine before its check belongs to it
/// and is never offered on this computer.
#[core::prelude::v1::test]
fn r1_4_a_remote_draft_is_never_offered_here() {
    if !isolated("saved_work::r1_4_a_remote_draft_is_never_offered_here") {
        return;
    }
    scene("r1-4", |p, repo, window, cx| {
        switch(p, remote(), false, repo, window, cx);
        p.draft.id = Some("remote-draft".into());
        p.draft.composer = Some(typed("typed on the remote", window, cx));
        p.capture_session(cx);
        switch(p, local(), true, repo, window, cx);
        p.verify_identity_for_test(fixture_host());
        assert!(!p.saved_drafts().iter().any(|(id, _)| id == "remote-draft"));
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &remote()).drafts["remote-draft"].draft.text,
            "typed on the remote"
        );
        assert!(!work(p, &local()).drafts.contains_key("remote-draft"));
    });
}

/// R2-1: a draft open during a switch is saved, with its text, for its own
/// machine; on the other machine every send path refuses it.
#[core::prelude::v1::test]
fn r2_1_a_draft_open_during_a_switch_stays_with_its_machine() {
    if !isolated("saved_work::r2_1_a_draft_open_during_a_switch_stays_with_its_machine") {
        return;
    }
    scene("r2-1", |p, repo, window, cx| {
        p.draft.id = Some("here".into());
        p.draft.composer = Some(typed("Typed on this computer", window, cx));
        switch(p, remote(), false, repo, window, cx);
        assert_eq!(
            work(p, &local()).drafts["here"].draft.text,
            "Typed on this computer"
        );
        p.verify_identity_for_test(host_key("workstation-host"));
        p.draft = p.tabs.lifecycle.drafts.remove("here").unwrap();
        assert!(p.draft_send_refusal().is_some() && p.draft_blocked().is_some());
        p.start_draft_creation(window, cx);
        assert!(p.draft.error.is_some());
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local()).drafts["here"].draft.text,
            "Typed on this computer"
        );
        assert!(
            p.saved_work
                .machines
                .get(&remote())
                .is_none_or(|w| !w.drafts.contains_key("here"))
        );
    });
}

/// R2-2: a thread's text typed before any check is kept after a relaunch.
#[core::prelude::v1::test]
fn r2_2_text_typed_before_a_check_survives_a_relaunch() {
    if !isolated("saved_work::r2_2_text_typed_before_a_check_survives_a_relaunch") {
        return;
    }
    scene("r2-2", |p, repo, window, cx| {
        switch(p, remote(), false, repo, window, cx);
        let composer = mount(p, window, cx);
        composer.update(cx, |c, cx| {
            c.restore_draft("typed early".into(), 3, None, cx)
        });
        relaunch(p, repo, cx);
        p.machines.active = remote();
        let composer = mount(p, window, cx);
        assert_eq!(composer.read(cx).saved_draft().0, "typed early");
    });
}

/// R2-3: a waiting message for the same thread ID at another folder is
/// neither loaded nor rewritten.
#[core::prelude::v1::test]
fn r2_3_another_folders_wait_is_left_alone() {
    if !isolated("saved_work::r2_3_another_folders_wait_is_left_alone") {
        return;
    }
    scene("r2-3", |p, repo, _, cx| {
        let elsewhere = repo.with_file_name("elsewhere");
        let saved = thread_record(&elsewhere, "Elsewhere's text", &["w-x"]);
        seed(p, &local(), |w| w.put_thread(THREAD, saved.clone(), None));
        p.capture_session(cx);
        assert!(p.host.submissions.accepted(THREAD).is_empty());
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local()).thread(THREAD, &elsewhere).unwrap().thread,
            saved
        );
    });
}

/// R3-1: after a reconnect, a composer mounted before the check shows the
/// saved text; an edit is kept, and nothing blanks it.
#[core::prelude::v1::test]
fn r3_1_a_reconnect_and_remount_keeps_the_saved_text() {
    if !isolated("saved_work::r3_1_a_reconnect_and_remount_keeps_the_saved_text") {
        return;
    }
    scene("r3-1", |p, repo, window, cx| {
        seed(p, &local(), |w| {
            w.put_thread(THREAD, thread_record(repo, "saved before", &[]), None)
        });
        p.host.connection.epoch += 1;
        p.park_workspace(cx);
        let composer = mount(p, window, cx);
        assert_eq!(composer.read(cx).saved_draft().0, "saved before");
        p.capture_session(cx);
        assert_eq!(
            work(p, &local())
                .thread(THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "saved before"
        );
        composer.update(cx, |c, cx| {
            c.restore_draft("edited after the reconnect".into(), 4, None, cx)
        });
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local())
                .thread(THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "edited after the reconnect"
        );
    });
}

/// R3-2: a new draft typed before the check is saved, and kept on relaunch.
#[core::prelude::v1::test]
fn r3_2_a_new_draft_typed_before_the_check_is_saved() {
    if !isolated("saved_work::r3_2_a_new_draft_typed_before_the_check_is_saved") {
        return;
    }
    scene("r3-2", |p, repo, window, cx| {
        p.host.connection.epoch += 1;
        p.draft.id = Some("early".into());
        p.draft.composer = Some(typed("before the check", window, cx));
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local()).drafts["early"].draft.text,
            "before the check"
        );
    });
}

/// R3-3: closing, while connected, a background draft tab whose text is
/// parked and not yet saved keeps the text; it is there after a relaunch.
#[core::prelude::v1::test]
fn r3_3_closing_a_parked_drafts_tab_keeps_its_text() {
    if !isolated("saved_work::r3_3_closing_a_parked_drafts_tab_keeps_its_text") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-saved-r3-3-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        super::tabs::show_tabs(p);
        launch(p, &repo);
        p.open_draft(window, cx);
    });
    let (first, tab) = h
        .window
        .update(&mut h.cx, |p, window, cx| {
            let first = p.draft.id.clone().unwrap();
            // Saved empty earlier; the latest text is only parked.
            seed(p, &local(), |w| {
                w.drafts.insert(first.clone(), draft_tab(&repo, ""));
            });
            p.draft.saved.text = "parked, not saved yet".into();
            p.draft.composer = None;
            p.bind_draft();
            let parked = std::mem::take(&mut p.draft);
            p.tabs.lifecycle.drafts.insert(first.clone(), parked);
            let tab = p
                .tabs
                .model
                .tabs()
                .iter()
                .find(|t| matches!(t.session().target(), plait_client::tabs::TabTarget::Draft { id, .. } if *id == first))
                .unwrap()
                .id();
            p.open_draft(window, cx);
            (first, tab)
        })
        .unwrap();
    h.update(|p, w, cx| p.request_tab(Intent::Close(tab), w, cx));
    let deadline = Instant::now() + Duration::from_secs(10);
    while h
        .window
        .update(&mut h.cx, |p, _, _| p.tabs.lifecycle.busy)
        .unwrap()
    {
        h.settle();
        assert!(Instant::now() < deadline);
    }
    h.update(|p, _, cx| {
        assert!(p.host.connection.connected);
        relaunch(p, &repo, cx);
        assert_eq!(
            work(p, &local()).drafts[&first].draft.text,
            "parked, not saved yet"
        );
        p.session.journal = None;
        p.session.data = None;
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// R3-4: a waiting message loaded on one machine, with no composer, stays
/// saved for it; another machine listing the same thread cannot erase it.
#[core::prelude::v1::test]
fn r3_4_a_wait_without_a_composer_stays_with_its_machine() {
    if !isolated("saved_work::r3_4_a_wait_without_a_composer_stays_with_its_machine") {
        return;
    }
    scene("r3-4", |p, repo, window, cx| {
        p.workspace.composers.remove(THREAD);
        seed(p, &local(), |w| {
            w.put_thread(
                THREAD,
                thread_record(repo, "", &["w-a"]),
                Some(fixture_host()),
            )
        });
        p.capture_session(cx);
        switch(p, remote(), false, repo, window, cx);
        p.verify_identity_for_test(host_key("workstation-host"));
        p.capture_session(cx);
        switch(p, local(), true, repo, window, cx);
        p.verify_identity_for_test(fixture_host());
        p.capture_session(cx);
        assert_eq!(p.host.submissions.accepted(THREAD)[0].request_id, "w-a");
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local())
                .thread(THREAD, repo)
                .unwrap()
                .thread
                .queued_submissions[0]
                .request_id,
            "w-a"
        );
    });
}

/// R3-5 and #174's A→B→A: each machine's tab document has one writer, kept
/// across switches; both machines' documents are written by their own.
#[core::prelude::v1::test]
fn r3_5_each_machines_tab_document_keeps_one_writer() {
    if !isolated("saved_work::r3_5_each_machines_tab_document_keeps_one_writer") {
        return;
    }
    scene("r3-5", |p, repo, window, cx| {
        let a = p.tabs.persistence.tab_writer();
        switch(p, remote(), false, repo, window, cx);
        let b = p.tabs.persistence.tab_writer();
        assert!(!std::sync::Arc::ptr_eq(&a, &b));
        let a_after_switch = a.revision();
        let _ = p.flush_tab_preferences(Some(window), cx);
        assert_eq!(
            a.revision(),
            a_after_switch,
            "B's save left A's writer alone"
        );
        switch(p, local(), true, repo, window, cx);
        assert!(
            std::sync::Arc::ptr_eq(&a, &p.tabs.persistence.tab_writer()),
            "A has one writer"
        );
        let _ = p.flush_tab_preferences(Some(window), cx);
        assert!(a.revision() > a_after_switch && b.revision() >= 1);
    });
}

/// #174 finding 4: receipts settle saved Waits only from this machine's
/// verified host, the one that accepted them; not before the check, and not
/// from a host reinstalled under the same machine entry.
#[core::prelude::v1::test]
fn receipts_settle_waits_only_from_the_host_that_accepted_them() {
    if !isolated("saved_work::receipts_settle_waits_only_from_the_host_that_accepted_them") {
        return;
    }
    scene("receipts", |p, repo, window, cx| {
        seed(p, &local(), |w| {
            w.put_thread(
                THREAD,
                thread_record(repo, "", &["w-a"]),
                Some(fixture_host()),
            )
        });
        p.capture_session(cx);
        let summary = p.host.connection.threads()[0].clone();
        assert!(p.may_reconcile(&summary));
        p.host.connection.epoch += 1;
        assert!(!p.may_reconcile(&summary), "not before the check");
        p.reconcile_checkout_waits(window, cx);
        p.verify_identity_for_test(host_key("reinstalled-host"));
        assert!(!p.may_reconcile(&summary), "not from a replacement host");
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local())
                .thread(THREAD, repo)
                .unwrap()
                .thread
                .queued_submissions
                .len(),
            1
        );
    });
}

/// #174 finding 5a: a thread's own approval choice is saved even when
/// nothing else about it is, and returns after a relaunch.
#[core::prelude::v1::test]
fn a_fresh_threads_approval_choice_is_saved() {
    if !isolated("saved_work::a_fresh_threads_approval_choice_is_saved") {
        return;
    }
    scene("approval", |p, repo, _, cx| {
        p.set_thread_approval(THREAD.into(), Some(ApprovalMode::Auto), cx);
        relaunch(p, repo, cx);
        assert_eq!(
            work(p, &local())
                .thread(THREAD, repo)
                .unwrap()
                .thread
                .approval,
            Some(ApprovalMode::Auto)
        );
    });
}

/// #174 finding 3: past a machine file's size bound, the text stays in memory
/// with a notice, the file on disk is not replaced, and a switch keeps it.
#[core::prelude::v1::test]
fn work_past_the_file_bound_stays_in_memory() {
    if !isolated("saved_work::work_past_the_file_bound_stays_in_memory") {
        return;
    }
    scene("bound", |p, repo, window, cx| {
        seed(p, &local(), |w| {
            w.drafts.insert("kept".into(), draft_tab(repo, "durable"));
        });
        p.session.journal.as_ref().unwrap().flush().unwrap();
        seed(p, &local(), |w| {
            for n in 0..40 {
                w.drafts
                    .insert(format!("big-{n}"), draft_tab(repo, &"x".repeat(250 * 1024)));
            }
        });
        assert!(
            p.session
                .journal
                .as_ref()
                .unwrap()
                .flush()
                .unwrap_err()
                .contains("exceeds")
        );
        switch(p, remote(), false, repo, window, cx);
        switch(p, local(), true, repo, window, cx);
        assert_eq!(work(p, &local()).drafts.len(), 41);
        let on_disk = plait_client::session::read_machines(&state(repo)).unwrap();
        assert_eq!(on_disk[&local()].drafts.len(), 1);
    });
}

/// Older records are copied to this computer once, then edited there; the
/// edit survives a relaunch and the older record stays for older builds.
#[core::prelude::v1::test]
fn an_older_draft_is_copied_then_edited_here() {
    if !isolated("saved_work::an_older_draft_is_copied_then_edited_here") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-saved-legacy-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    // An older build saved this workspace: drafts in desktop.json only.
    {
        let (_, host) = plait_client::session::locations().unwrap();
        let (journal, mut data) = Journal::open(&state(&repo), host).unwrap();
        journal.take_machines();
        data.draft_tabs
            .insert("older".into(), draft_tab(&repo, "older build's text"));
        journal.queue(data);
        journal.flush().unwrap();
        // The open above made this computer's file; an older build would not have.
        let _ = std::fs::remove_dir_all(state(&repo).join("desktop-machines"));
    }
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        launch(p, &repo);
        assert_eq!(
            work(p, &local()).drafts["older"].draft.text,
            "older build's text"
        );
        p.restore_draft_tab("older".into(), cx);
        p.draft.composer = Some(typed("edited here", window, cx));
        relaunch(p, &repo, cx);
        assert_eq!(work(p, &local()).drafts["older"].draft.text, "edited here");
        // Left in place for older builds.
        assert_eq!(
            p.session.data.as_ref().unwrap().draft_tabs["older"]
                .draft
                .text,
            "older build's text"
        );
        p.session.journal = None;
        p.session.data = None;
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Medium: switching home saves the outgoing remote folder as the remote's,
/// not as this computer's folder to reopen.
#[core::prelude::v1::test]
fn a_switch_saves_the_outgoing_folder_with_its_own_locality() {
    if !isolated("saved_work::a_switch_saves_the_outgoing_folder_with_its_own_locality") {
        return;
    }
    scene("locality", |p, repo, window, cx| {
        switch(p, remote(), false, repo, window, cx);
        p.host_workspace.checkout_carried = false;
        p.host.creation.checkout = "/srv/remote-project".into();
        p.session.data.as_mut().unwrap().checkout = None;
        switch(p, local(), true, repo, window, cx);
        assert_eq!(p.session.data.as_ref().unwrap().checkout, None);
    });
}

/// Saving stays off the hot path: a keystroke's capture changes memory and
/// queues one machine's file; the write is debounced on the background
/// executor. A switch saves and parks without touching the disk.
#[core::prelude::v1::test]
fn saving_and_switching_stay_within_budget() {
    if !isolated("saved_work::saving_and_switching_stay_within_budget") {
        return;
    }
    scene("timing", |p, repo, window, cx| {
        let text = "x".repeat(2048);
        seed(p, &local(), |w| {
            for n in 0..200 {
                w.drafts
                    .insert(format!("draft-{n}"), draft_tab(repo, &text));
                w.put_thread(
                    &format!("thread-{n}"),
                    thread_record(repo, &text, &[]),
                    None,
                );
            }
        });
        p.capture_session(cx);
        let composer = p.workspace.composers[THREAD].clone();
        let mut worst_save = Duration::ZERO;
        for n in 0..20 {
            let started = Instant::now();
            composer.update(cx, |c, cx| {
                c.restore_draft(format!("keystroke {n}"), 10 + n, None, cx)
            });
            p.capture_session(cx);
            worst_save = worst_save.max(started.elapsed());
        }
        let started = Instant::now();
        switch(p, remote(), false, repo, window, cx);
        let switched = started.elapsed();
        eprintln!("saved-work timing: worst save {worst_save:?}, machine switch {switched:?}");
        assert!(
            worst_save < Duration::from_millis(50),
            "save took {worst_save:?}"
        );
        assert!(
            switched < Duration::from_millis(50),
            "switch took {switched:?}"
        );
    });
}
