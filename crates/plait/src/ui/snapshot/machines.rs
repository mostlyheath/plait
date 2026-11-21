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
            &["Use workstation", "Remove workstation…", "Add SSH machine…"],
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
        assert_eq!(p.machine_label(), "Local host", "adding does not switch");
    });
    std::fs::remove_dir_all(scratch).unwrap();
}
