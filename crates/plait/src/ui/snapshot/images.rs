use super::*;
use plait_client::images::ImageRef;
use std::collections::HashMap;

pub(super) fn fixtures() -> Vec<(ImageRef, Arc<gpui::RenderImage>)> {
    [
        include_bytes!("../../../../../runtime/test/support/images/0.png").as_slice(),
        include_bytes!("../../../../../runtime/test/support/images/1.png").as_slice(),
        include_bytes!("../../../../../runtime/test/support/images/2.png").as_slice(),
    ]
    .into_iter()
    .map(|bytes| {
        (
            plait_client::images::inspect(bytes).unwrap(),
            super::super::images::decode(bytes, 192).unwrap(),
        )
    })
    .collect()
}
pub(super) fn install(harness: &mut Harness, count: usize) {
    let fixture = fixtures();
    let references = (0..count)
        .map(|index| {
            if count <= 3 {
                fixture[index % fixture.len()].0.clone()
            } else {
                let mut bytes =
                    include_bytes!("../../../../../runtime/test/support/images/0.png").to_vec();
                bytes.extend_from_slice(&index.to_be_bytes());
                plait_client::images::inspect(&bytes).unwrap()
            }
        })
        .collect::<Vec<_>>();
    harness.update(|plait, _, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 6;
        let mut snapshot = (**plait.host.thread.snapshot.as_ref().unwrap()).clone();
        snapshot.image_input = Some("yes".into());
        snapshot.messages = (0..count)
            .map(|index| Message {
                id: format!("image-request-{index}"),
                role: MessageRole::User,
                text: format!("Review image {}.", index + 1),
                images: vec![references[index].clone()],
                ..Default::default()
            })
            .collect();
        if count == 3 {
            snapshot.messages = vec![Message {
                id: "image-request-0".into(),
                role: MessageRole::User,
                text: "Compare these three screens.".into(),
                images: references.clone(),
                ..Default::default()
            }];
        }
        snapshot.tools.clear();
        snapshot.live_text.clear();
        snapshot.status = ThreadStatus::Idle;
        plait.host.thread.snapshot = Some(Arc::new(snapshot));
        plait.workspace.history = Default::default();
        plait.workspace.formatted = Arc::new(plait_client::markdown::Transcript::update(
            &Default::default(),
            plait.host.thread.snapshot.as_ref().unwrap(),
        ));
        plait.workspace.images = super::super::images::ImageCache::scene(
            (
                plait.host.connection.epoch,
                plait.host.thread.watch_epoch,
                THREAD.into(),
            ),
            references
                .iter()
                .rev()
                .take(super::super::images::THUMBNAILS)
                .map(|reference| {
                    let pixels = fixture
                        .iter()
                        .find(|(image, _)| image.id == reference.id)
                        .unwrap_or(&fixture[0])
                        .1
                        .clone();
                    (reference.id.clone(), Ok(pixels))
                })
                .collect(),
        );
        plait.refresh_live_list(None);
        plait.workspace.live_scroll.scroll_to_end();
        plait.sync_composer(cx);
    });
}
pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.fresh_composer(repository);
    install(harness, 1);
    let fixture = fixtures();
    harness.composer(|composer, window, cx| {
        composer.type_text("Compare the attached screens.", window, cx);
        composer.scene_images(
            vec![fixture[0].0.clone()],
            HashMap::from([(fixture[0].0.id.clone(), fixture[0].1.clone())]),
            cx,
        );
    });
    harness.capture(&format!("images-composer-one-{suffix}"));
    harness.composer(|composer, _, cx| {
        composer.scene_images(
            fixture.iter().map(|(r, _)| r.clone()).collect(),
            fixture
                .iter()
                .map(|(r, p)| (r.id.clone(), p.clone()))
                .collect(),
            cx,
        )
    });
    harness.capture(&format!("images-composer-several-{suffix}"));
    harness.update(|plait, _, cx| {
        let composer = plait.workspace.composers[THREAD].clone();
        let revision = composer.update(cx, |composer, cx| {
            let revision = composer.saved_draft().1;
            composer.restore_draft(String::new(), revision, None, cx);
            revision
        });
        let pending = plait
            .host
            .submissions
            .begin_images(
                THREAD,
                String::new(),
                None,
                None,
                Some(revision),
                fixture.iter().map(|(r, _)| r.clone()).collect(),
            )
            .unwrap();
        plait
            .host
            .submissions
            .start_send(THREAD, &pending.request_id);
        plait.sync_composer(cx);
    });
    harness.capture(&format!("images-uploading-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.host.submissions = Default::default();
        plait.sync_composer(cx);
    });
    harness.composer(|composer, window, cx| {
        composer.type_text("Compare the attached screens.", window, cx);
    });

    harness.composer(|composer, _, cx| composer.set_image_support(6, Some("no"), cx));
    harness.capture(&format!("images-text-only-{suffix}"));
    harness.composer(|composer, _, cx| composer.set_image_support(5, Some("yes"), cx));
    harness.capture(&format!("images-older-host-{suffix}"));
    harness.composer(|composer, _, cx| {
        composer.set_image_support(6, Some("yes"), cx);
        composer.restore_images(vec![fixture[1].0.clone()], cx);
    });
    harness.capture(&format!("images-unavailable-{suffix}"));

    let references = fixture.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
    let _pins = plait_client::images::pin(&references).unwrap();
    for bytes in [
        include_bytes!("../../../../../runtime/test/support/images/0.png").as_slice(),
        include_bytes!("../../../../../runtime/test/support/images/2.png").as_slice(),
    ] {
        plait_client::images::persist(bytes).unwrap();
    }
    harness.composer(|composer, _, cx| composer.restore_images(references, cx));
    harness.capture(&format!("images-partial-restore-{suffix}"));
    harness.composer(|composer, _, cx| composer.scene_remove(&fixture[1].0.id, cx));
    harness.capture(&format!("images-partial-recovered-{suffix}"));

    harness.composer(|composer, _, cx| {
        composer.scene_images(Vec::new(), HashMap::new(), cx);
        composer.set_image_support(6, Some("yes"), cx);
    });
    install(harness, 3);
    harness.capture(&format!("images-conversation-{suffix}"));
    harness.update(|plait, _, _| {
        plait.workspace.images.enlarged = Some((
            fixture[0].0.clone(),
            Some(Ok(super::super::images::decode(
                include_bytes!("../../../../../runtime/test/support/images/0.png"),
                1600,
            )
            .unwrap()
            .into())),
        ))
    });
    harness.capture(&format!("images-enlarged-{suffix}"));
    harness.update(|plait, _, _| plait.workspace.images.enlarged = None);
}
