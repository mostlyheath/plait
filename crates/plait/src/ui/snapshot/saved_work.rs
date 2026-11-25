//! Per-machine saved work (#132): one regression per high finding from the
//! three reviews of #164, each asserting the saved text survives, plus the
//! migration and legacy-copy cases and timings. Switches go through
//! `use_machine_client`; reconnects advance the connection epoch; quits
//! round-trip the saved workspace through its file form.
use super::*;
use plait_client::{
    host_state::PendingSubmit,
    machines::MachineId,
    session::{Draft, DraftTab, Namespace, Session, Thread},
    tabs::HostKey,
};

fn host_key(id: &str) -> HostKey {
    HostKey {
        id: id.into(),
        store: "/store".into(),
    }
}

fn host_ns(id: &str) -> Namespace {
    Namespace::Host { host: host_key(id) }
}

fn remote() -> MachineId {
    MachineId::new("workstation").unwrap()
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

fn data(p: &Plait) -> &Session {
    p.session.data.as_ref().unwrap()
}

/// A switch through the real path. The new machine is listed again with THREAD
/// at `root`, as its host would list a thread with the same ID and folder.
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

/// Quit and relaunch: the saved workspace through its file form, nothing in memory.
fn relaunch(p: &mut Plait, cx: &mut Context<Plait>) {
    let bytes = serde_json::to_vec(data(p)).unwrap();
    p.park_workspace(cx);
    p.forget_saved_work();
    p.session.data = Some(serde_json::from_slice(&bytes).unwrap());
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
        // As a fresh load: nothing in memory is bound to the data replaced here.
        p.session.data = Some(Session::empty("/host".into()));
        p.forget_saved_work();
        p.workspace.bindings.clear();
        body(p, &repo, window, cx);
        p.session.data = None;
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// R1-1: Reopen during a switch never offers, or claims, this computer's
/// older draft for the machine switched to.
#[core::prelude::v1::test]
fn r1_1_reopen_during_a_switch_offers_no_other_machines_draft() {
    if !isolated("saved_work::r1_1_reopen_during_a_switch_offers_no_other_machines_draft") {
        return;
    }
    scene("r1-1", |p, repo, window, cx| {
        p.session
            .data
            .as_mut()
            .unwrap()
            .draft_tabs
            .insert("older".into(), draft_tab(repo, "older build's draft"));
        p.capture_session(cx);
        assert_eq!(
            p.test_saved_drafts()["older"].draft.text,
            "older build's draft"
        );
        switch(p, remote(), false, repo, window, cx);
        assert!(p.verified_host().is_none());
        assert!(p.saved_drafts().is_empty() && p.recoverable_tab_drafts().is_empty());
        p.capture_session(cx);
        let saved = data(p).draft_in(&host_ns("fixture-host"), "older").unwrap();
        assert_eq!(saved.tab.draft.text, "older build's draft");
        assert!(
            data(p)
                .space(&Namespace::Provisional { machine: remote() })
                .is_none_or(|s| s.drafts.is_empty())
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
        let here = host_ns("fixture-host");
        p.session
            .data
            .as_mut()
            .unwrap()
            .put_thread(
                &here,
                THREAD,
                thread_record(repo, "A's text", &["w-a"]),
                None,
            )
            .unwrap();
        p.capture_session(cx);
        assert_eq!(p.host.submissions.accepted(THREAD).len(), 1);
        switch(p, remote(), false, repo, window, cx);
        p.verify_identity_for_test(host_key("workstation-host"));
        p.capture_session(cx);
        // Nothing of A's is in memory to reconcile against B's empty receipts.
        assert!(p.host.submissions.accepted(THREAD).is_empty());
        let a = &data(p).thread_in(&here, THREAD, repo).unwrap().thread;
        assert_eq!(a.draft.text, "A's text");
        assert_eq!(a.queued_submissions[0].request_id, "w-a");
    });
}

/// R1-3: before B is verified, A's text for the same thread is not shown in
/// B's composer, and what is typed there does not replace it.
#[core::prelude::v1::test]
fn r1_3_an_unverified_machine_cannot_read_or_replace_anothers_text() {
    if !isolated("saved_work::r1_3_an_unverified_machine_cannot_read_or_replace_anothers_text") {
        return;
    }
    scene("r1-3", |p, repo, window, cx| {
        let here = host_ns("fixture-host");
        p.session
            .data
            .as_mut()
            .unwrap()
            .put_thread(&here, THREAD, thread_record(repo, "A's text", &[]), None)
            .unwrap();
        switch(p, remote(), false, repo, window, cx);
        let composer = mount(p, window, cx);
        assert_eq!(composer.read(cx).saved_draft().0, "");
        composer.update(cx, |c, cx| c.restore_draft("B's text".into(), 2, None, cx));
        p.capture_session(cx);
        assert_eq!(
            data(p)
                .thread_in(&here, THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "A's text"
        );
        let b = Namespace::Provisional { machine: remote() };
        assert_eq!(
            data(p)
                .thread_in(&b, THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "B's text"
        );
    });
}

/// R1-4: a draft typed on a remote machine before its check is saved for
/// that machine and never offered on this computer.
#[core::prelude::v1::test]
fn r1_4_an_unverified_remote_draft_is_never_offered_here() {
    if !isolated("saved_work::r1_4_an_unverified_remote_draft_is_never_offered_here") {
        return;
    }
    scene("r1-4", |p, repo, window, cx| {
        let local = p.machines.active.clone();
        switch(p, remote(), false, repo, window, cx);
        p.draft.id = Some("remote-draft".into());
        p.draft.composer = Some(typed("typed on the remote", window, cx));
        p.capture_session(cx);
        switch(p, local, true, repo, window, cx);
        p.verify_identity_for_test(fixture_host());
        p.capture_session(cx);
        assert!(!p.saved_drafts().iter().any(|(id, _)| id == "remote-draft"));
        let saved = data(p)
            .draft_in(
                &Namespace::Provisional { machine: remote() },
                "remote-draft",
            )
            .unwrap();
        assert_eq!(saved.tab.draft.text, "typed on the remote");
    });
}

/// R2-1: a draft open during a switch is saved, with its text, for its own
/// machine and put away; on the other machine no send path sends it.
#[core::prelude::v1::test]
fn r2_1_a_draft_open_during_a_switch_stays_with_its_machine() {
    if !isolated("saved_work::r2_1_a_draft_open_during_a_switch_stays_with_its_machine") {
        return;
    }
    scene("r2-1", |p, repo, window, cx| {
        p.draft.id = Some("here".into());
        p.draft.composer = Some(typed("Typed on this computer", window, cx));
        switch(p, remote(), false, repo, window, cx);
        let here = host_ns("fixture-host");
        assert_eq!(
            data(p).draft_in(&here, "here").unwrap().tab.draft.text,
            "Typed on this computer"
        );
        assert!(p.draft.id.is_none() && p.draft.composer.is_none());
        // Brought back on the remote machine, verified: still refused on
        // every send path, and its text stays where it was.
        p.verify_identity_for_test(host_key("workstation-host"));
        p.draft = p.tabs.lifecycle.drafts.remove("here").unwrap();
        assert!(p.draft_send_refusal().is_some());
        assert!(p.draft_blocked().is_some());
        p.start_draft_creation(window, cx);
        assert!(p.draft.error.is_some());
        p.capture_session(cx);
        assert_eq!(
            data(p).draft_in(&here, "here").unwrap().tab.draft.text,
            "Typed on this computer"
        );
        assert!(
            data(p)
                .draft_in(&host_ns("workstation-host"), "here")
                .is_none()
        );
    });
}

/// R2-2: a listed thread's text typed before any check is saved
/// provisionally and is back after a quit and relaunch, still unverified.
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
        p.capture_session(cx);
        relaunch(p, cx);
        let composer = mount(p, window, cx);
        assert_eq!(composer.read(cx).saved_draft().0, "typed early");
    });
}

/// R2-3: a waiting message saved for the same thread ID at another folder is
/// neither loaded nor rewritten.
#[core::prelude::v1::test]
fn r2_3_another_folders_wait_is_left_alone() {
    if !isolated("saved_work::r2_3_another_folders_wait_is_left_alone") {
        return;
    }
    scene("r2-3", |p, repo, _, cx| {
        let here = host_ns("fixture-host");
        let elsewhere = repo.with_file_name("elsewhere");
        let saved = thread_record(&elsewhere, "Elsewhere's text", &["w-x"]);
        p.session
            .data
            .as_mut()
            .unwrap()
            .put_thread(&here, THREAD, saved.clone(), None)
            .unwrap();
        p.capture_session(cx);
        assert!(p.host.submissions.accepted(THREAD).is_empty());
        assert_eq!(
            data(p).thread_in(&here, THREAD, &elsewhere).unwrap().thread,
            saved
        );
    });
}

/// R3-1: after a reconnect, a composer mounted before the check shows the
/// saved text, read-only; a save does not blank it, and an edit is kept and
/// then copied to the host once it is verified again.
#[core::prelude::v1::test]
fn r3_1_a_reconnect_and_remount_keeps_the_saved_text() {
    if !isolated("saved_work::r3_1_a_reconnect_and_remount_keeps_the_saved_text") {
        return;
    }
    scene("r3-1", |p, repo, window, cx| {
        let here = host_ns("fixture-host");
        p.session
            .data
            .as_mut()
            .unwrap()
            .put_thread(
                &here,
                THREAD,
                thread_record(repo, "saved before", &[]),
                None,
            )
            .unwrap();
        p.capture_session(cx);
        p.host.connection.epoch += 1;
        assert!(p.verified_host().is_none());
        p.park_workspace(cx);
        let composer = mount(p, window, cx);
        assert_eq!(composer.read(cx).saved_draft().0, "saved before");
        p.capture_session(cx);
        assert_eq!(
            data(p)
                .thread_in(&here, THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "saved before"
        );
        composer.update(cx, |c, cx| {
            c.restore_draft("edited after the reconnect".into(), 4, None, cx)
        });
        p.capture_session(cx);
        assert_eq!(
            data(p)
                .thread_in(&here, THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "saved before"
        );
        // Verified again: the edit, which started from the host's record,
        // replaces it; the source goes once the copy is durable.
        p.verify_identity_for_test(fixture_host());
        p.capture_session(cx);
        assert_eq!(
            data(p)
                .thread_in(&here, THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "edited after the reconnect"
        );
        let provisional = Namespace::Provisional {
            machine: p.machines.active.clone(),
        };
        assert!(data(p).space(&provisional).is_some());
        let copies = p.saved_work.migrations.len();
        p.complete_migrations(copies, cx);
        // The still-mounted composer saves its unchanged text again; the copy follows it.
        assert_eq!(
            data(p)
                .thread_in(&here, THREAD, repo)
                .unwrap()
                .thread
                .draft
                .text,
            "edited after the reconnect"
        );
    });
}

/// R3-2: a new draft typed before the check is saved, provisionally.
#[core::prelude::v1::test]
fn r3_2_a_new_draft_typed_before_the_check_is_saved() {
    if !isolated("saved_work::r3_2_a_new_draft_typed_before_the_check_is_saved") {
        return;
    }
    scene("r3-2", |p, _, window, cx| {
        p.host.connection.epoch += 1;
        p.draft.id = Some("early".into());
        p.draft.composer = Some(typed("before the check", window, cx));
        p.capture_session(cx);
        let provisional = Namespace::Provisional {
            machine: p.machines.active.clone(),
        };
        assert_eq!(
            data(p)
                .draft_in(&provisional, "early")
                .unwrap()
                .tab
                .draft
                .text,
            "before the check"
        );
    });
}

/// R3-3: closing a background draft tab whose text is parked, not yet
/// saved, keeps the draft; nothing is deleted from the journal.
#[core::prelude::v1::test]
fn r3_3_closing_a_parked_drafts_tab_keeps_its_text() {
    if !isolated("saved_work::r3_3_closing_a_parked_drafts_tab_keeps_its_text") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-saved-r3-3-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    let (first, tab) = h
        .window
        .update(&mut h.cx, |p, window, cx| {
            super::tabs::show_tabs(p);
            p.session.data = Some(Session::empty("/host".into()));
            p.forget_saved_work();
            p.workspace.bindings.clear();
            p.open_draft(window, cx);
            let first = p.draft.id.clone().unwrap();
            let namespace = p.write_namespace();
            // Saved empty, as it was when offline; the text is only parked.
            p.session.data.as_mut().unwrap().put_draft(&namespace, &first, draft_tab(&repo, ""), None).unwrap();
            p.draft.saved.text = "parked, not saved yet".into();
            p.draft.composer = None;
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
    h.update(|p, w, cx| p.request_tab(super::super::tab_lifecycle::Intent::Close(tab), w, cx));
    let deadline = Instant::now() + Duration::from_secs(10);
    while h
        .window
        .update(&mut h.cx, |p, _, _| p.tabs.lifecycle.busy)
        .unwrap()
    {
        h.settle();
        assert!(Instant::now() < deadline);
    }
    h.update(|p, _, _| {
        // Kept, in memory or saved: either way its text is there to reopen.
        let namespace = p.write_namespace();
        let saved = data(p).draft_in(&namespace, &first).unwrap();
        assert_eq!(saved.tab.draft.text, "parked, not saved yet");
        p.session.data = None;
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// R3-4: a waiting message loaded on one machine, with no composer mounted,
/// stays saved for it; another machine listing the same thread cannot erase it.
#[core::prelude::v1::test]
fn r3_4_a_wait_without_a_composer_stays_with_its_machine() {
    if !isolated("saved_work::r3_4_a_wait_without_a_composer_stays_with_its_machine") {
        return;
    }
    scene("r3-4", |p, repo, window, cx| {
        let local = p.machines.active.clone();
        let here = host_ns("fixture-host");
        p.workspace.composers.remove(THREAD);
        p.session
            .data
            .as_mut()
            .unwrap()
            .put_thread(&here, THREAD, thread_record(repo, "", &["w-a"]), None)
            .unwrap();
        p.capture_session(cx);
        assert_eq!(p.host.submissions.accepted(THREAD).len(), 1);
        switch(p, remote(), false, repo, window, cx);
        p.verify_identity_for_test(host_key("workstation-host"));
        p.capture_session(cx);
        p.host.submissions.forget_accepted(THREAD, "w-a");
        p.capture_session(cx);
        switch(p, local, true, repo, window, cx);
        p.verify_identity_for_test(fixture_host());
        p.capture_session(cx);
        assert_eq!(p.host.submissions.accepted(THREAD)[0].request_id, "w-a");
        assert_eq!(
            data(p)
                .thread_in(&here, THREAD, repo)
                .unwrap()
                .thread
                .queued_submissions[0]
                .request_id,
            "w-a"
        );
    });
}

/// R3-5: each machine's tab document has its own writer: the next machine's
/// saves cannot make the previous one's pending write stand down.
#[core::prelude::v1::test]
fn r3_5_each_machines_tab_document_has_its_own_writer() {
    if !isolated("saved_work::r3_5_each_machines_tab_document_has_its_own_writer") {
        return;
    }
    scene("r3-5", |p, repo, window, cx| {
        let writer = p.tabs.persistence.tab_writer();
        switch(p, remote(), false, repo, window, cx);
        // The switch flushed this computer's document with its own writer.
        let flushed = writer.revision();
        let next = p.tabs.persistence.tab_writer();
        assert!(!std::sync::Arc::ptr_eq(&writer, &next));
        let _ = p.flush_tab_preferences(Some(window), cx);
        let _ = p.flush_tab_preferences(Some(window), cx);
        assert_eq!(writer.revision(), flushed);
        assert!(next.revision() >= 2);
    });
}

/// Medium: switching home saves the outgoing remote folder as the remote's,
/// not as this computer's folder to reopen.
#[core::prelude::v1::test]
fn a_switch_saves_the_outgoing_folder_with_its_own_locality() {
    if !isolated("saved_work::a_switch_saves_the_outgoing_folder_with_its_own_locality") {
        return;
    }
    scene("locality", |p, repo, window, cx| {
        let local = p.machines.active.clone();
        switch(p, remote(), false, repo, window, cx);
        p.host_workspace.checkout_carried = false;
        p.host.creation.checkout = "/srv/remote-project".into();
        p.session.data.as_mut().unwrap().checkout = None;
        switch(p, local, true, repo, window, cx);
        assert_eq!(data(p).checkout, None);
    });
}

/// Saving stays off the hot path: a capture after a keystroke clones and
/// compares in memory, and the write is debounced on the background executor;
/// a switch saves and parks without touching the disk. Measured with a full
/// namespace of saved work.
#[core::prelude::v1::test]
fn saving_and_switching_stay_within_budget() {
    if !isolated("saved_work::saving_and_switching_stay_within_budget") {
        return;
    }
    scene("timing", |p, repo, window, cx| {
        let here = host_ns("fixture-host");
        let text = "x".repeat(2048);
        {
            let data = p.session.data.as_mut().unwrap();
            for n in 0..200 {
                data.put_draft(&here, &format!("draft-{n}"), draft_tab(repo, &text), None)
                    .unwrap();
                data.put_thread(
                    &here,
                    &format!("thread-{n}"),
                    thread_record(repo, &text, &[]),
                    None,
                )
                .unwrap();
            }
        }
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
            switched < Duration::from_millis(250),
            "switch took {switched:?}"
        );
    });
}
