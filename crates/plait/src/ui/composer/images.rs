use super::super::{images::decode, theme::ButtonAction, thread_controls::TextTooltip};
use super::*;
use plait_client::images::{self, ImageRef};
use std::{collections::HashMap, path::PathBuf};

#[derive(Default)]
pub(super) struct Attachments {
    pub references: Vec<ImageRef>,
    pub pixels: HashMap<String, Arc<RenderImage>>,
    pub pins: HashMap<String, images::Pins>,
    pub generation: u64,
    pub loading: bool,
    pub error: Option<String>,
    pub failed: HashMap<String, String>,
    pub uploading: bool,
    pub blocked: Option<(String, String)>,
    /// Why this editor takes text only; a pasted image is refused with it.
    pub text_only: Option<String>,
}
impl Composer {
    fn pin_images(&mut self, references: &[ImageRef]) {
        self.images.pins = references
            .iter()
            .filter_map(|reference| {
                images::pin(std::slice::from_ref(reference))
                    .ok()
                    .map(|pin| (reference.id.clone(), pin))
            })
            .collect();
    }
    pub(super) fn remove_image(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.images
            .references
            .retain(|reference| reference.id != id);
        self.images.pixels.remove(id);
        self.images.pins.remove(id);
        self.images.failed.remove(id);
        self.images.error = None;
        self.draft.revision += 1;
        cx.notify();
    }
    pub fn set_images_uploading(&mut self, uploading: bool, cx: &mut Context<Self>) {
        if self.images.uploading != uploading {
            self.images.uploading = uploading;
            cx.notify();
        }
    }
    pub fn saved_images(&self) -> &[ImageRef] {
        &self.images.references
    }
    pub fn set_image_support(
        &mut self,
        protocol: u32,
        support: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let blocked = if protocol < 6 {
            Some((
                "Update host".into(),
                "Images need a newer host (version 6). Update the host before sending images."
                    .into(),
            ))
        } else if support == Some("no") {
            Some((
                "Text only".into(),
                "This model cannot take images. Remove them or choose another model.".into(),
            ))
        } else {
            None
        };
        if self.images.blocked != blocked {
            self.images.blocked = blocked;
            cx.notify();
        }
    }
    pub fn restore_images(&mut self, references: Vec<ImageRef>, cx: &mut Context<Self>) {
        self.load_originals(references, None, None, false, cx);
    }
    #[cfg(test)]
    pub fn restore_thread_images(
        &mut self,
        references: Vec<ImageRef>,
        thread: String,
        cx: &mut Context<Self>,
    ) {
        self.restore_thread_images_with_client(
            references,
            thread,
            plait_client::host::Client::from_environment().expect("local host configuration"),
            cx,
        );
    }
    pub fn restore_thread_images_with_client(
        &mut self,
        references: Vec<ImageRef>,
        thread: String,
        client: plait_client::host::Client,
        cx: &mut Context<Self>,
    ) {
        self.load_originals(references, Some(thread), Some(client), false, cx);
    }
    /// Append recovered thread images onto the draft's current attachments
    /// without dropping existing decoded thumbnails or exceeding message limits.
    pub(in crate::ui) fn apply_recovered_images(
        &mut self,
        references: Vec<ImageRef>,
        thread: String,
        client: plait_client::host::Client,
        cx: &mut Context<Self>,
    ) {
        self.draft.revision += 1;
        self.load_originals(references, Some(thread), Some(client), true, cx);
    }
    fn load_originals(
        &mut self,
        references: Vec<ImageRef>,
        thread: Option<String>,
        client: Option<plait_client::host::Client>,
        keep_pixels: bool,
        cx: &mut Context<Self>,
    ) {
        self.images.generation += 1;
        self.pin_images(&references);
        self.images.references = references.clone();
        if keep_pixels {
            self.images
                .pixels
                .retain(|id, _| references.iter().any(|r| r.id == *id));
        } else {
            self.images.pixels.clear();
        }
        self.images.failed.clear();
        self.images.error = None;
        let to_load: Vec<ImageRef> = references
            .iter()
            .filter(|reference| !self.images.pixels.contains_key(&reference.id))
            .cloned()
            .collect();
        self.images.loading = !to_load.is_empty();
        if keep_pixels {
            cx.notify();
        }
        let generation = self.images.generation;
        let pending_pin = images::pin(&references);
        let task = cx.background_executor().spawn(async move {
            let pin = pending_pin?;
            images::validate_message(&references)?;
            let prepared = to_load
                .into_iter()
                .map(|reference| {
                    let result = restore_original(&reference, thread.as_deref(), client.as_ref());
                    (reference.id, result)
                })
                .collect::<HashMap<_, _>>();
            Ok::<_, String>((prepared, pin))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.images.generation != generation {
                    return;
                }
                this.images.loading = false;
                match result {
                    Ok((prepared, _pin)) => {
                        for (id, result) in prepared {
                            // Removed attachments must retain neither pixels nor failures.
                            if !this.images.references.iter().any(|r| r.id == id) {
                                continue;
                            }
                            match result {
                                Ok(pixels) => {
                                    this.images.pixels.insert(id, pixels);
                                }
                                Err(error) => {
                                    this.images.failed.insert(id, error);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        this.images.failed = this
                            .images
                            .references
                            .iter()
                            .filter(|reference| !this.images.pixels.contains_key(&reference.id))
                            .map(|reference| (reference.id.clone(), error.clone()))
                            .collect();
                    }
                }
                cx.notify();
            });
            cx.background_executor()
                .spawn(async {
                    if let Err(error) = images::collect() {
                        eprintln!("Plait draft images: {error}");
                    }
                })
                .detach();
        })
        .detach();
    }
    pub(super) fn image_send_blocked(&self) -> bool {
        self.images.loading
            || self.images.uploading
            || (!self.images.references.is_empty()
                && (self.images.blocked.is_some()
                    || self
                        .images
                        .references
                        .iter()
                        .any(|r| !self.images.pixels.contains_key(&r.id))))
    }
    fn refuse_image(&mut self, reason: String, cx: &mut Context<Self>) {
        self.images.error = Some(reason);
        cx.notify();
    }
    pub(super) fn paste_images(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if let Some(reason) = self.images.text_only.clone() {
            return self.refuse_image(reason, cx);
        }
        // Clipboard allocation is GPUI's; reject immediately, before retaining or decoding it.
        let info = match images::describe(&bytes) {
            Ok(info) => info,
            Err(error) => return self.refuse_image(error, cx),
        };
        if self.images.references.len() >= images::COUNT {
            return self.refuse_image("A message can have at most 4 images.".into(), cx);
        }
        if self.images.references.iter().map(|r| r.size).sum::<usize>() + info.size
            > images::MESSAGE_BYTES
        {
            return self.refuse_image("Images in a message must total at most 5 MiB.".into(), cx);
        }
        self.add_images(Vec::new(), Some(bytes), cx);
    }
    pub(crate) fn drop_images(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if paths.len() + self.images.references.len() > images::COUNT {
            return self.refuse_image("A message can have at most 4 images.".into(), cx);
        }
        self.add_images(paths, None, cx);
    }
    fn add_images(&mut self, paths: Vec<PathBuf>, bytes: Option<Vec<u8>>, cx: &mut Context<Self>) {
        if self.images.loading {
            return self.refuse_image("Wait for the previous images to finish loading.".into(), cx);
        }
        self.images.loading = true;
        self.images.error = None;
        self.images.generation += 1;
        let generation = self.images.generation;
        let current = self.images.references.clone();
        let task = cx.background_executor().spawn(async move {
            let mut inputs = Vec::new();
            if let Some(bytes) = bytes {
                inputs.push(bytes);
            }
            for path in paths {
                inputs.push(images::read_file(&path)?);
            }
            let inputs = inputs
                .into_iter()
                .map(images::Prepared::new)
                .collect::<Result<Vec<_>, _>>()?;
            let mut all = current;
            all.extend(inputs.iter().map(|input| input.reference().clone()));
            images::validate_message(&all)?;
            inputs
                .into_iter()
                .map(|input| {
                    let pixels = decode(input.bytes(), 192)?;
                    let pin = images::pin(std::slice::from_ref(input.reference()))?;
                    let reference = input.persist()?;
                    Ok((reference, pixels, pin))
                })
                .collect::<Result<Vec<_>, String>>()
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.images.generation != generation {
                    return;
                }
                this.images.loading = false;
                match result {
                    Ok(added) => {
                        for (reference, pixels, pin) in added {
                            this.images.pins.insert(reference.id.clone(), pin);
                            this.images.pixels.insert(reference.id.clone(), pixels);
                            this.images.references.push(reference);
                        }
                        this.draft.revision += 1;
                    }
                    Err(error) => this.images.error = Some(error),
                }
                cx.notify();
            });
            cx.background_executor()
                .spawn(async {
                    if let Err(error) = images::collect() {
                        eprintln!("Plait draft images: {error}");
                    }
                })
                .detach();
        })
        .detach();
        cx.notify();
    }
    pub(super) fn image_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut strip = theme::row().flex_wrap().gap_2();
        for reference in &self.images.references {
            let id = reference.id.clone();
            strip = strip.child(
                theme::column()
                    .gap_1()
                    .child(
                        div()
                            .w(px(80.))
                            .h(px(64.))
                            .id(SharedString::from(format!("draft-image-{id}")))
                            .when(!self.images.pixels.contains_key(&id), |slot| {
                                let reason = self
                                    .images
                                    .failed
                                    .get(&id)
                                    .cloned()
                                    .unwrap_or_else(|| "Preparing image thumbnail.".into());
                                slot.tooltip(move |_, cx| {
                                    cx.new(|_| TextTooltip(reason.clone())).into()
                                })
                            })
                            .child(match self.images.pixels.get(&id) {
                                Some(pixels) => img(pixels.clone())
                                    .size_full()
                                    .object_fit(ObjectFit::Contain)
                                    .into_any_element(),
                                None => div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        if self.images.loading
                                            && !self.images.failed.contains_key(&id)
                                        {
                                            "Loading"
                                        } else {
                                            "Failed"
                                        },
                                    )
                                    .into_any_element(),
                            }),
                    )
                    .child(
                        theme::button(SharedString::from(format!("remove-{id}")), "Remove")
                            .h(px(22.))
                            .aria_label("Remove attached image")
                            .on_activate(cx, move |this, _, cx| {
                                this.remove_image(&id, cx);
                            }),
                    ),
            );
        }
        let state = if self.images.uploading {
            Some((
                "Uploading".into(),
                "Sending image originals to the host. The draft stays until acceptance.".into(),
            ))
        } else if !self.images.failed.is_empty() {
            // Each unavailable slot already names its failure and has the reason on hover.
            None
        } else if let Some(error) = &self.images.error {
            Some(("Refused".to_owned(), error.clone()))
        } else if self.images.loading {
            Some(("Loading".into(), "Preparing image thumbnails.".into()))
        } else if !self.images.references.is_empty() {
            self.images.blocked.clone()
        } else {
            None
        };
        if let Some((label, detail)) = state {
            strip = strip.child(
                theme::button("image-state", label)
                    .text_color(rgb(if self.images.uploading {
                        colors().blue
                    } else {
                        colors().amber
                    }))
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into()),
            );
        }
        strip.into_any_element()
    }
}

/// One unavailable original must not discard the other attachments' prepared pixels.
fn restore_original(
    reference: &ImageRef,
    thread: Option<&str>,
    client: Option<&plait_client::host::Client>,
) -> Result<Arc<RenderImage>, String> {
    let bytes = images::local(reference).or_else(|error| {
        let Some(thread) = thread else {
            return Err(error);
        };
        let bytes = client
            .ok_or_else(|| "Host client unavailable.".to_string())?
            .image(thread, reference)
            .map_err(|error| error.to_string())?;
        images::persist(&bytes)?;
        Ok(bytes)
    })?;
    decode(&bytes, 192)
}

/// Result of merging recovered images into a draft's current attachments.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::ui) struct MergedImages {
    pub references: Vec<ImageRef>,
    pub added: usize,
    pub already_attached: usize,
    pub overflow: usize,
}

/// Merge `incoming` images onto `existing` attachments without duplicates,
/// staying within the per-message image count and byte limits.
pub(in crate::ui) fn merge_draft_images(
    existing: &[ImageRef],
    incoming: &[ImageRef],
) -> MergedImages {
    let mut references = existing.to_vec();
    let mut total_bytes: usize = existing.iter().map(|image| image.size).sum();
    let mut added = 0;
    let mut already_attached = 0;
    let mut overflow = 0;
    let mut seen = std::collections::HashSet::new();
    for image in incoming {
        if !seen.insert(&image.id) {
            continue;
        }
        if existing.iter().any(|current| current.id == image.id) {
            already_attached += 1;
            continue;
        }
        if references.len() < images::COUNT
            && total_bytes.saturating_add(image.size) <= images::MESSAGE_BYTES
        {
            total_bytes += image.size;
            references.push(image.clone());
            added += 1;
        } else {
            overflow += 1;
        }
    }
    MergedImages {
        references,
        added,
        already_attached,
        overflow,
    }
}

#[cfg(test)]
impl Composer {
    pub(in crate::ui) fn scene_images(
        &mut self,
        references: Vec<ImageRef>,
        pixels: HashMap<String, Arc<RenderImage>>,
        cx: &mut Context<Self>,
    ) {
        self.pin_images(&references);
        self.images.references = references;
        self.images.pixels = pixels;
        self.images.failed.clear();
        self.images.loading = false;
        self.draft.revision += 1;
        cx.notify();
    }
    pub(in crate::ui) fn scene_refuse(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.paste_images(bytes, cx);
    }
    pub(in crate::ui) fn scene_drop(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.drop_images(paths, cx);
    }
    pub(in crate::ui) fn scene_remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.remove_image(id, cx);
    }
    pub(in crate::ui) fn scene_image_status(
        &self,
    ) -> (bool, bool, Option<&str>, Option<&str>, usize, bool) {
        (
            self.images.loading,
            self.image_send_blocked(),
            self.images.error.as_deref(),
            self.images.failed.values().next().map(String::as_str),
            self.images.pixels.len(),
            self.images.uploading,
        )
    }
    pub(in crate::ui) fn scene_image_state(&self, id: &str) -> (bool, Option<&str>) {
        (
            self.images.pixels.contains_key(id),
            self.images.failed.get(id).map(String::as_str),
        )
    }
    pub(in crate::ui) fn scene_history(&self) -> Arc<[String]> {
        self.draft.history.entries()
    }
    pub(in crate::ui) fn scene_submit(&mut self, cx: &mut Context<Self>) {
        self.submit(false, cx);
    }
}
