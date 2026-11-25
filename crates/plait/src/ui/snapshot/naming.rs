//! Naming settings use injected host state, never a live model.
use super::*;
use plait_client::host::thread_naming::Settings;

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|p, window, cx| {
        p.show_settings(true, window, cx);
        p.settings_tab = SettingsTab::NewThreads;
        p.naming.sync(p.host.connection.epoch);
        p.naming.saved = Some(Settings::default());
    });
    harness.draw();
    harness.update(|p, _, _| p.settings_scroll.scroll_to_bottom());
    harness.capture(&format!("naming-off-{suffix}"));
    harness.update(|p, _, _| {
        let model = p.host.connection.status.as_ref().unwrap().models[0].choice();
        p.naming.saved = Some(Settings {
            enabled: true,
            model: Some(model),
        });
    });
    harness.capture(&format!("naming-enabled-{suffix}"));
    harness.update(|p, _, cx| {
        p.themes.forced = Some(true);
        p.apply_theme(cx);
        p.naming.saved.as_mut().unwrap().model = Some(ModelChoice {
            provider: "local".into(),
            model_id: "unavailable-local-model".into(),
        });
    });
    harness.capture(&format!("naming-unavailable-light-{suffix}"));
    harness.update(|p, _, _| {
        p.naming.saved = None;
        p.naming.unsupported = true;
        p.naming.note = Some("Update this host to configure thread naming.".into());
    });
    harness.capture(&format!("naming-older-host-{suffix}"));
    harness.update(|p, window, cx| {
        p.naming = Default::default();
        p.themes.forced = None;
        p.apply_theme(cx);
        p.show_settings(false, window, cx);
    });
}

#[core::prelude::v1::test]
#[ignore = "writes PNG files; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn naming_screenshots() {
    if !isolated("naming::naming_screenshots") {
        return;
    }
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR")
        .map(PathBuf::from)
        .expect("Set PLAIT_SNAPSHOT_DIR");
    std::fs::create_dir_all(&output).unwrap();
    let scratch =
        std::env::temp_dir().join(format!("plait-naming-captures-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    for (width, height, suffix) in [(1380., 860., "wide"), (900., 600., "narrow")] {
        let mut harness = Harness::new(&repository, Some(&output), width, height);
        walk(&mut harness, &repository, suffix);
    }
}

#[core::prelude::v1::test]
fn a_naming_choice_from_another_host_does_not_save() {
    if !isolated("naming::a_naming_choice_from_another_host_does_not_save") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-naming-scene-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|p, _, cx| {
        p.naming.sync(p.host.connection.epoch);
        p.naming.saved = Some(Settings::default());
        let old_epoch = p.host.connection.epoch.wrapping_sub(1);
        p.choose_naming_model(
            ModelChoice {
                provider: "local".into(),
                model_id: "small".into(),
            },
            old_epoch,
            cx,
        );
        assert_eq!(p.naming.saved, Some(Settings::default()));
        assert!(!p.naming.busy);
    });
}
