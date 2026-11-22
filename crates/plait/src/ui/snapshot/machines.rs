//! Settings → Machines with injected state: no scene starts ssh.
use super::*;
use plait_core::{machines::Transport, remote::ssh::SshDestination};

fn open(h: &mut Harness) {
    h.update(|p, window, cx| {
        p.show_settings(true, window, cx);
        p.settings_tab = SettingsTab::Machines;
    });
    h.settle();
}

/// Adds the workstation machine to preferences and returns its id.
fn workstation(h: &mut Harness) -> plait_core::machines::MachineId {
    let mut id = None;
    h.update(|p, _, _| {
        let added = p
            .preferences
            .value
            .machines
            .add(
                "workstation",
                Transport::Ssh {
                    destination: SshDestination::new("you@workstation").unwrap(),
                    port: None,
                    store: None,
                },
            )
            .unwrap();
        id = Some(added);
    });
    id.unwrap()
}

type Scene = (&'static str, fn(&mut Harness), &'static [&'static str]);

fn scenes() -> [Scene; 5] {
    [
        (
            "machines-list",
            |h| {
                workstation(h);
            },
            &["Use workstation", "Remove workstation…", "Add machine…"],
        ),
        (
            "machines-form-error",
            |h| {
                h.update(|p, _, cx| p.open_machine_form_for_test(cx));
                h.update(|p, _, cx| p.add_machine_for_test(cx));
            },
            &["Machine name", "SSH destination", "Add", "Cancel"],
        ),
        (
            "machines-connecting",
            |h| {
                let id = workstation(h);
                h.update(|p, _, _| p.machines.connecting = Some(id));
            },
            &["Use workstation"],
        ),
        (
            "machines-failed",
            |h| {
                workstation(h);
                h.update(|p, _, _| {
                    p.machines.note = Some((
                        "Could not reach workstation. SSH connection failed: Permission denied (publickey).".into(),
                        true,
                    ))
                });
            },
            &["Use workstation"],
        ),
        (
            "machines-remove-confirm",
            |h| {
                let id = workstation(h);
                h.update(|p, _, _| p.machines.confirm_remove = Some(id));
            },
            &["Remove workstation", "Cancel"],
        ),
    ]
}

pub(super) fn walk(repo: &Path, out: Option<&Path>, suffix: &str, width: f32, height: f32) {
    for (name, install, _) in scenes() {
        let mut h = Harness::new(repo, out, width, height);
        h.connect(repo, false);
        open(&mut h);
        install(&mut h);
        h.capture(&format!("{name}-{suffix}"));
    }
}

/// Only the Machines scenes, for reviewing their layout quickly.
#[core::prelude::v1::test]
#[ignore = "writes PNG files; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn machines_snapshots() {
    if !isolated("machines::machines_snapshots") {
        return;
    }
    let output = PathBuf::from(std::env::var_os("PLAIT_SNAPSHOT_DIR").expect("PLAIT_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&output).unwrap();
    let scratch = std::env::temp_dir().join(format!("plait-machines-shots-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    walk(&repo, Some(&output), "wide", 1380., 860.);
    walk(&repo, Some(&output), "narrow", 900., 600.);
    for (width, height, suffix) in [(1380., 860., "wide"), (900., 600., "narrow")] {
        let mut h = Harness::new(&repo, Some(&output), width, height);
        h.connect(&repo, false);
        h.update(|p, window, cx| {
            p.host_local = false;
            p.show_tab(super::super::Workspace::Changes, window, cx);
        });
        h.capture(&format!("remote-changes-{suffix}"));
        h.update(|p, window, cx| p.open_repository(&super::super::OpenRepository, window, cx));
        h.capture(&format!("remote-path-{suffix}"));
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn machines_are_labelled_and_validate_input() {
    if !isolated("machines::machines_are_labelled_and_validate_input") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-machines-scenes-{}", std::process::id()));
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
    // An empty form is refused with a reason; a valid one adds the machine.
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    open(&mut h);
    h.update(|p, _, cx| {
        p.open_machine_form_for_test(cx);
        p.add_machine_for_test(cx);
        assert!(p.machines.form.as_ref().unwrap().error.is_some());
        let form = p.machines.form.as_ref().unwrap();
        let (name, destination) = (form.name.clone(), form.destination.clone());
        name.update(cx, |f, cx| f.set_text("workstation", cx));
        destination.update(cx, |f, cx| f.set_text("-oProxyCommand=x", cx));
        p.add_machine_for_test(cx);
        assert!(
            p.machines.form.as_ref().unwrap().error.is_some(),
            "an option is not a destination"
        );
        destination.update(cx, |f, cx| f.set_text("you@workstation", cx));
        p.add_machine_for_test(cx);
        assert!(p.machines.form.is_none());
        assert_eq!(p.preferences.value.machines.list().len(), 2);
        assert_eq!(p.machine_label(), "This Mac", "adding does not switch");
    });
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn remote_path_validation() {
    assert!(super::super::valid_remote_path("/srv/project"));
    assert!(!super::super::valid_remote_path(""));
    assert!(!super::super::valid_remote_path("relative/path"));
    assert!(!super::super::valid_remote_path("/srv/\nproject"));
}

#[core::prelude::v1::test]
fn remote_handoffs_are_blocked() {
    if !isolated("machines::remote_handoffs_are_blocked") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-remote-handoffs-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, _, cx| {
        p.host_local = false;
        p.open_in_editor(cx);
        p.show_in_finder(cx);
        p.open_terminal(cx);
        assert!(p.ways_out.started.is_empty());
        assert_eq!(p.notice.as_deref(), Some(super::super::REMOTE_UNAVAILABLE));
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn remote_project_path_opens_only_absolute_paths() {
    if !isolated("machines::remote_project_path_opens_only_absolute_paths") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-remote-path-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        p.host_local = false;
        // As after a real switch: the link is new, so opening a project does not relink.
        p.machines.fresh_link = true;
        p.run_command(
            super::super::commands::Command::OpenRemoteProject("relative".into()),
            None,
            window,
            cx,
        );
        assert!(!p.lists().opened.contains(&PathBuf::from("relative")));
        p.run_command(
            super::super::commands::Command::OpenRemoteProject("/remote/project".into()),
            None,
            window,
            cx,
        );
        assert!(p.lists().opened.contains(&PathBuf::from("/remote/project")));
        // Another machine's path is never saved as one of this Mac's projects.
        assert!(
            !p.preferences
                .value
                .navigation
                .opened
                .contains(&PathBuf::from("/remote/project"))
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn remote_machine_reads_git_through_its_host() {
    if !isolated("machines::remote_machine_reads_git_through_its_host") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-remote-jobs-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.settle();
    h.update(|p, _, cx| {
        assert!(matches!(p.state.repository.snapshot(), Load::Ready(_)));
        // The test's host has no socket, so a read sent to it fails; this
        // Mac's Git would have read the fixture and succeeded.
        p.host_local = false;
        p.state.repository.refresh();
        p.pump(cx);
    });
    h.settle();
    h.update(|p, _, _| {
        assert!(
            matches!(p.state.repository.snapshot(), Load::Error(_)),
            "this Mac's Git answered for a remote checkout"
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A remote machine's window lists only what its host names, under the
/// host's own name, and never what this Mac had open.
#[core::prelude::v1::test]
fn a_remote_window_shows_only_its_hosts_workspace() {
    if !isolated("machines::a_remote_window_shows_only_its_hosts_workspace") {
        return;
    }
    use plait_core::host::workspace::{HostIdentity, Project, View};
    let scratch = std::env::temp_dir().join(format!("plait-remote-ws-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        let mac = p.lists().opened.clone();
        // Switching machines starts empty: none of this Mac's threads remain.
        let remote = plait_core::machines::MachineId::new("workstation").unwrap();
        let client = p.host_client();
        p.use_machine_client(client, remote, false, window, cx);
        assert!(p.host.connection.threads.is_empty());
        // The host's view arrives once the link is up, as it would be here.
        p.host.connection.connecting = false;
        p.host.connection.connected = true;
        p.machines.fresh_link = true;
        let generation = p.host_workspace_generation();
        let view = View {
            schema: 1,
            host: HostIdentity {
                id: "host-1".into(),
                name: "arthur".into(),
                platform: "linux".into(),
            },
            revision: 3,
            projects: vec![Project {
                path: "/srv/api".into(),
                hidden: false,
            }],
            worktrees: Vec::new(),
            seen: Vec::new(),
            status: Default::default(),
        };
        assert!(p.apply_workspace(generation, view, window, cx));
        let names = p
            .state
            .projects
            .iter()
            .map(|project| project.checkout.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![PathBuf::from("/srv/api")]);
        assert_eq!(p.machine_label(), "arthur");
        assert!(!p.state.projects[0].machines[0].local);
        // This Mac's saved lists are untouched.
        assert_eq!(p.preferences.value.navigation.opened, mac);
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A change the host saw in the open checkout makes it read again; nothing polls.
#[core::prelude::v1::test]
fn a_watched_change_rereads_the_open_checkout() {
    if !isolated("machines::a_watched_change_rereads_the_open_checkout") {
        return;
    }
    use plait_core::host::workspace::{HostIdentity, Project, View};
    let scratch = std::env::temp_dir().join(format!("plait-watched-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.settle();
    h.update(|p, window, cx| {
        let open = p.state.repository.path().to_owned();
        let view = |count: u64| View {
            schema: 1,
            host: HostIdentity {
                id: "this-mac".into(),
                name: "mac".into(),
                platform: "macos".into(),
            },
            revision: 1,
            projects: vec![Project {
                path: open.clone(),
                hidden: false,
            }],
            worktrees: Vec::new(),
            seen: Vec::new(),
            status: [(open.clone(), count)].into_iter().collect(),
        };
        let generation = p.host_workspace_generation();
        assert!(p.apply_workspace(generation, view(0), window, cx));
        // Drain the reread the first view asks for.
        while p.state.next_repository_job().is_some() {}
        p.state.repository.data.snapshot = plait_core::state::Load::Empty;
        assert!(p.apply_workspace(generation, view(0), window, cx));
        assert!(
            p.state.next_repository_job().is_none(),
            "an unchanged counter reread"
        );
        assert!(p.apply_workspace(generation, view(1), window, cx));
        assert!(
            matches!(
                p.state.repository.snapshot(),
                plait_core::state::Load::Loading
            ),
            "a moved counter did not reread the open checkout"
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
