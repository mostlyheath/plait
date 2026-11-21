//! Settings → Linear, with account state injected: no scene reaches Linear.
use super::*;
use plait_core::linear_settings::{Auth, Pending};

fn open(h: &mut Harness) {
    h.update(|p, window, cx| {
        p.show_settings(true, window, cx);
        p.settings_tab = SettingsTab::Linear;
    });
    // The sync that runs on that change opens the key field.
    h.settle();
}

fn signed_in() -> Auth {
    Auth::SignedIn {
        workspace: "Acme".into(),
        viewer: "Alex".into(),
    }
}

fn set(h: &mut Harness, change: impl FnOnce(&mut plait_core::linear_settings::Account)) {
    h.update(|p, _, _| change(&mut p.linear.account));
}

/// Each scene's name, how to reach it, and labels it must expose.
type Scene = (&'static str, fn(&mut Harness), &'static [&'static str]);

fn scenes() -> [Scene; 6] {
    [
        (
            "linear-signed-out",
            |h| set(h, |a| a.auth = Auth::SignedOut),
            &["Linear API key", "Save key", "Reveal"],
        ),
        (
            "linear-saving",
            |h| {
                set(h, |a| {
                    a.auth = Auth::SignedOut;
                    a.pending = Pending::Saving;
                })
            },
            &["Save key"],
        ),
        (
            "linear-key-rejected",
            |h| set(h, |a| a.auth = Auth::KeyRejected),
            &["Save key"],
        ),
        (
            "linear-signed-in",
            |h| {
                set(h, |a| {
                    a.auth = signed_in();
                    a.note = Some("Key saved on the host.".into());
                })
            },
            &["Replace key", "Sign out…"],
        ),
        (
            "linear-sign-out-confirm",
            |h| {
                set(h, |a| {
                    a.auth = signed_in();
                    a.ask_sign_out();
                })
            },
            &["Sign out", "Cancel"],
        ),
        (
            "linear-older-host",
            |h| set(h, |a| a.auth = Auth::OlderHost),
            &[],
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

/// Only the Linear settings scenes, for reviewing their layout quickly.
#[core::prelude::v1::test]
#[ignore = "writes PNG files; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn linear_settings_snapshots() {
    if !isolated("linear_settings::linear_settings_snapshots") {
        return;
    }
    let output = PathBuf::from(std::env::var_os("PLAIT_SNAPSHOT_DIR").expect("PLAIT_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&output).unwrap();
    let scratch = std::env::temp_dir().join(format!("plait-linear-shots-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    walk(&repo, Some(&output), "wide", 1380., 860.);
    walk(&repo, Some(&output), "narrow", 900., 600.);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn linear_settings_are_labelled_wide_and_narrow() {
    if !isolated("linear_settings::linear_settings_are_labelled_wide_and_narrow") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-linear-scenes-{}", std::process::id()));
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
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn saving_clears_the_key_and_sign_out_waits_for_confirmation() {
    if !isolated("linear_settings::saving_clears_the_key_and_sign_out_waits_for_confirmation") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-linear-actions-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 900., 600.);
    h.connect(&repo, false);
    open(&mut h);
    set(&mut h, |a| a.auth = Auth::SignedOut);
    h.update(|p, _, cx| {
        let field = p
            .linear
            .field
            .clone()
            .expect("the Linear tab opens a key field");
        field.update(cx, |field, cx| field.set_text("lin_api_SECRET", cx));
        p.save_linear_key(cx);
        assert_eq!(
            field.read(cx).text(),
            "",
            "the key is cleared as it is sent"
        );
        assert_eq!(p.linear.account.pending, Pending::Saving);
    });
    set(&mut h, |a| {
        a.pending = Pending::None;
        a.auth = signed_in();
    });
    h.update(|p, _, cx| {
        p.sign_out_of_linear(cx);
        assert_eq!(
            p.linear.account.pending,
            Pending::None,
            "not without confirming"
        );
        p.linear.account.ask_sign_out();
        p.sign_out_of_linear(cx);
        assert_eq!(p.linear.account.pending, Pending::SigningOut);
    });
    // Leaving the tab empties the field, whatever was typed.
    h.update(|p, _, cx| {
        let field = p.linear.field.clone().unwrap();
        field.update(cx, |field, cx| field.set_text("typed", cx));
        p.settings_tab = SettingsTab::About;
    });
    h.settle();
    h.update(|p, _, cx| assert_eq!(p.linear.field.as_ref().unwrap().read(cx).text(), ""));
    std::fs::remove_dir_all(scratch).unwrap();
}
