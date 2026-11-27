//! Settings → Machines with injected state: no scene starts ssh.
use super::*;
use plait_client::{machines::Transport, remote::ssh::SshDestination};

fn open(h: &mut Harness) {
    h.update(|p, window, cx| {
        p.show_settings(true, window, cx);
        p.settings_tab = SettingsTab::Machines;
    });
    h.settle();
}

/// Adds the workstation machine to preferences and returns its id.
fn workstation(h: &mut Harness) -> plait_client::machines::MachineId {
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

fn scenes() -> [Scene; 9] {
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
            "machines-install",
            |h| {
                let id = workstation(h);
                h.update(|p, _, _| {
                    p.machines.note = Some((
                        "Could not reach workstation. sh: 1: exec: /home/you/.local/bin/plait-host: not found".into(),
                        true,
                    ));
                    p.machines.install = Some(super::super::machine_install::MachineInstall {
                        id,
                        name: "workstation".into(),
                        stage: super::super::machine_install::Stage::Missing(
                            plait_client::remote::install::Probe {
                                platform: Some("linux-x86_64".into()),
                                system: "Linux x86_64".into(),
                                installed: false,
                                downloader: true,
                            },
                        ),
                    });
                });
            },
            &["Install Plait on workstation"],
        ),
        (
            "machines-installing",
            |h| {
                let id = workstation(h);
                h.update(|p, _, _| {
                    p.machines.install = Some(super::super::machine_install::MachineInstall {
                        id,
                        name: "workstation".into(),
                        stage: super::super::machine_install::Stage::Installing,
                    });
                });
            },
            &[],
        ),
        (
            "machines-update-available",
            |h| {
                let id = workstation(h);
                h.update(|p, _, _| {
                    p.machines.active = id.clone();
                    p.host.connection.connected = true;
                    p.machines.update = Some(super::super::machine_update::MachineUpdate {
                        id,
                        name: "workstation".into(),
                        stage: super::super::machine_update::Stage::Available(
                            "Plait v1 is available for workstation.".into(),
                        ),
                    });
                });
            },
            &["Update host", "Check for host update"],
        ),
        (
            "machines-updating",
            |h| {
                let id = workstation(h);
                h.update(|p, _, _| {
                    p.machines.active = id.clone();
                    p.host.connection.connected = true;
                    p.machines.update = Some(super::super::machine_update::MachineUpdate {
                        id,
                        name: "workstation".into(),
                        stage: super::super::machine_update::Stage::Updating,
                    });
                });
            },
            &[],
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
        // A remote machine's own projects, named by its host, with the switcher open.
        let mut h = Harness::new(&repo, Some(&output), width, height);
        h.connect(&repo, false);
        let mut existing = None;
        h.update(|p, _, _| {
            existing = p
                .preferences
                .value
                .machines
                .list()
                .iter()
                .find(|m| m.name == "workstation")
                .map(|m| m.id.clone());
        });
        let id = existing.unwrap_or_else(|| workstation(&mut h));
        h.update(|p, window, cx| {
            p.settings_open = false;
            let client = p.host_client();
            p.use_machine_client(client, id.clone(), false, window, cx);
            p.host.connection.connecting = false;
            p.host.connection.connected = true;
            p.machines.fresh_link = true;
            let generation = p.host_workspace_generation();
            let view = plait_client::host::workspace::View {
                schema: 1,
                host: plait_client::host::workspace::HostIdentity {
                    id: "scene-host".into(),
                    name: "workstation".into(),
                    platform: "linux".into(),
                },
                revision: 1,
                projects: ["/srv/api", "/srv/web"]
                    .into_iter()
                    .map(|path| plait_client::host::workspace::Project {
                        path: path.into(),
                        hidden: false,
                        ..Default::default()
                    })
                    .collect(),
                worktrees: Vec::new(),
                seen: Vec::new(),
                providers_revision: 0,
                status: Default::default(),
            };
            p.apply_workspace(generation, view, window, cx);
            p.navigation.menu = Some("machines".into());
            cx.notify();
        });
        h.capture(&format!("remote-switcher-{suffix}"));
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
            &p.machines.active.clone(),
            super::super::commands::Command::OpenRemoteProject("relative".into()),
            None,
            window,
            cx,
        );
        assert!(!p.lists().opened.contains(&PathBuf::from("relative")));
        p.run_command(
            &p.machines.active.clone(),
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
        // Point the remote machine at an unavailable host socket, so a read
        // sent to it fails; this Mac's host must not answer for it.
        p.host_local = false;
        p.host_client = plait_client::host::Client::local_or_unavailable();
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
    use plait_client::host::workspace::{HostIdentity, Project, View};
    let scratch = std::env::temp_dir().join(format!("plait-remote-ws-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        let mac = p.lists().opened.clone();
        // Switching machines starts empty: none of this Mac's threads remain.
        let remote = plait_client::machines::MachineId::new("workstation").unwrap();
        let client = p.host_client();
        p.use_machine_client(client, remote, false, window, cx);
        assert!(p.host.connection.threads().is_empty());
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
                ..Default::default()
            }],
            worktrees: Vec::new(),
            seen: Vec::new(),
            providers_revision: 0,
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

        // Back on this Mac, the other machine's folder is not one of its projects.
        let client = p.host_client();
        p.use_machine_client(client, Default::default(), true, window, cx);
        assert!(
            !p.state
                .projects
                .iter()
                .any(|project| project.checkout == std::path::Path::new("/srv/api"))
        );
        // A late view from the machine just left is refused.
        let late = View {
            schema: 1,
            host: HostIdentity {
                id: "host-1".into(),
                name: "arthur".into(),
                platform: "linux".into(),
            },
            revision: 4,
            projects: Vec::new(),
            worktrees: Vec::new(),
            seen: Vec::new(),
            providers_revision: 0,
            status: Default::default(),
        };
        assert!(!p.apply_workspace(generation, late, window, cx));
        let generation = p.host_workspace_generation();
        let view = View {
            schema: 1,
            host: HostIdentity {
                id: "local-host".into(),
                name: "mac".into(),
                platform: "macos".into(),
            },
            revision: 3,
            projects: vec![Project {
                path: repo.clone(),
                hidden: false,
                ..Default::default()
            }],
            worktrees: Vec::new(),
            seen: Vec::new(),
            providers_revision: 0,
            status: Default::default(),
        };
        assert!(p.apply_workspace(generation, view, window, cx));
        assert_eq!(p.host.creation.checkout, repo);
        assert!(
            !p.state
                .projects
                .iter()
                .any(|project| project.checkout == std::path::Path::new("/srv/api"))
        );
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
    use plait_client::host::workspace::{HostIdentity, Project, View};
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
                ..Default::default()
            }],
            worktrees: Vec::new(),
            seen: Vec::new(),
            providers_revision: 0,
            status: [(open.clone(), count)].into_iter().collect(),
        };
        let generation = p.host_workspace_generation();
        assert!(p.apply_workspace(generation, view(0), window, cx));
        // Drain the reread the first view asks for.
        while p.state.next_repository_job().is_some() {}
        p.state.repository.data.snapshot = plait_client::state::Load::Empty;
        assert!(p.apply_workspace(generation, view(0), window, cx));
        assert!(
            p.state.next_repository_job().is_none(),
            "an unchanged counter reread"
        );
        assert!(p.apply_workspace(generation, view(1), window, cx));
        assert!(
            matches!(
                p.state.repository.snapshot(),
                plait_client::state::Load::Loading
            ),
            "a moved counter did not reread the open checkout"
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Each machine keeps its own tabs: switching puts this one's away and starts
/// empty, and restoring waits for the new host's copy for this device.
#[core::prelude::v1::test]
fn switching_machines_puts_the_previous_tabs_away() {
    if !isolated("machines::switching_machines_puts_the_previous_tabs_away") {
        return;
    }
    use plait_client::host::workspace::{HostIdentity, View};
    let scratch = std::env::temp_dir().join(format!("plait-machine-tabs-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        assert!(!p.tabs.model.tabs().is_empty(), "the fixture opens a tab");
        let remote = plait_client::machines::MachineId::new("workstation").unwrap();
        let client = p.host_client();
        p.use_machine_client(client, remote, false, window, cx);
        assert!(
            p.tabs.model.tabs().is_empty(),
            "this Mac's tabs followed the window"
        );
        assert!(!p.tabs.persistence.restored);
        // The new host names itself: its tabs for this device are read before any restore.
        p.host.connection.connecting = false;
        p.host.connection.connected = true;
        p.machines.fresh_link = true;
        let generation = p.host_workspace_generation();
        let view = View {
            schema: 1,
            host: HostIdentity {
                id: "host-2".into(),
                name: "arthur".into(),
                platform: "linux".into(),
            },
            revision: 0,
            projects: Vec::new(),
            worktrees: Vec::new(),
            seen: Vec::new(),
            providers_revision: 0,
            status: Default::default(),
        };
        assert!(p.apply_workspace(generation, view, window, cx));
        assert!(
            p.tabs.persistence.host_loading,
            "the host's tabs were not asked for"
        );
        p.try_restore_tabs(window, cx);
        assert!(
            !p.tabs.persistence.restored,
            "restored before the host's tabs arrived"
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Folding a project on one machine leaves the same path open on another (#132).
#[core::prelude::v1::test]
fn folded_projects_stay_with_their_machine() {
    if !isolated("machines::folded_projects_stay_with_their_machine") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-same-fold-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, _, _| {
        let local = p.machines.active.clone();
        p.set_project_collapsed(repo.clone(), true);
        assert!(p.project_collapsed(&repo));
        p.machines.active = plait_client::machines::MachineId::new("workstation").unwrap();
        assert!(!p.project_collapsed(&repo));
        p.machines.active = local;
        assert!(p.project_collapsed(&repo));
        p.set_project_collapsed(repo.clone(), false);
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Switching machines saves the outgoing workspace while its locality still
/// applies, and the next host is not asked about the folder carried over from
/// the previous machine (#132).
#[core::prelude::v1::test]
fn a_switch_saves_and_probes_only_what_belongs_to_each_machine() {
    if !isolated("machines::a_switch_saves_and_probes_only_what_belongs_to_each_machine") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-switch-save-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    h.update(|p, window, cx| {
        let local = p.machines.active.clone();
        let remote = plait_client::machines::MachineId::new("workstation").unwrap();
        p.session.data = Some(plait_client::session::Session::empty("/host".into()));
        assert_eq!(p.probed_checkout(), Some(repo.clone()));
        // On the remote machine, with one of its folders open.
        let client = p.host_client();
        p.use_machine_client(client, remote, false, window, cx);
        // The folder still open is this computer's, carried over: not probed.
        assert_eq!(p.probed_checkout(), None);
        p.host_workspace.checkout_carried = false;
        p.host.creation.checkout = "/srv/remote-project".into();
        // Switching home saves the remote folder as the remote's, not as this
        // computer's restored folder.
        p.session.data.as_mut().unwrap().checkout = None;
        let client = p.host_client();
        p.use_machine_client(client, local, true, window, cx);
        assert_eq!(p.session.data.as_ref().unwrap().checkout, None);
        p.session.data = None;
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
