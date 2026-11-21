//! Image decoding is background work; frames only draw prepared pixels.
use super::{Plait, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_core::images::{self, ImageRef};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

pub(super) fn decode(bytes: &[u8], edge: u32) -> Result<Arc<RenderImage>, String> {
    Ok(decode_image(bytes, edge)?.pixels)
}
#[derive(Clone)]
pub(super) struct Decoded {
    pub pixels: Arc<RenderImage>,
    dimensions: Option<(u32, u32)>,
}
impl From<Arc<RenderImage>> for Decoded {
    fn from(pixels: Arc<RenderImage>) -> Self {
        Self {
            pixels,
            dimensions: None,
        }
    }
}
impl Decoded {
    fn dimensions(&self, reference: &ImageRef) -> (u32, u32) {
        self.dimensions
            .unwrap_or((reference.width, reference.height))
    }
}
fn decode_image(bytes: &[u8], edge: u32) -> Result<Decoded, String> {
    let info = images::describe(bytes)?;
    let format = if info.media_type == "image/png" {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    use image::ImageDecoder;
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "The image could not be decoded.")?;
    if decoder.dimensions() != (info.width, info.height) {
        return Err("Image dimensions changed while decoding.".into());
    }
    let orientation = decoder
        .orientation()
        .map_err(|_| "The image orientation could not be read.")?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| "The image could not be decoded.")?;
    decoded.apply_orientation(orientation);
    let dimensions = Some((decoded.width(), decoded.height()));
    let mut pixels = decoded.thumbnail(edge, edge).into_rgba8();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(Decoded {
        pixels: Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),
        dimensions,
    })
}

type PreparedImage = Result<Decoded, String>;
pub(super) const THUMBNAILS: usize = 128;
type EnlargedImage = (ImageRef, Option<PreparedImage>);

#[derive(Default)]
pub(super) struct ImageCache {
    key: Option<(u64, u64, String)>,
    ready: HashMap<String, PreparedImage>,
    pending: HashSet<String>,
    order: VecDeque<String>,
    pub enlarged: Option<EnlargedImage>,
    large_generation: u64,
}

/// Both host connection and selection must still be the ones that requested the pixels.
pub(super) fn accepts(expected: &(u64, u64, String), actual: &(u64, u64, String)) -> bool {
    expected == actual
}

impl ImageCache {
    pub(super) fn complete(
        &mut self,
        expected: &(u64, u64, String),
        current: &(u64, u64, String),
        id: String,
        result: PreparedImage,
    ) -> bool {
        if !accepts(expected, current) || self.key.as_ref() != Some(expected) {
            return false;
        }
        self.pending.remove(&id);
        self.order.retain(|key| key != &id);
        while self.ready.len() >= THUMBNAILS && !self.ready.contains_key(&id) {
            if let Some(old) = self.order.pop_front() {
                self.ready.remove(&old);
            } else {
                break;
            }
        }
        self.order.push_back(id.clone());
        self.ready.insert(id, result);
        true
    }
    pub(super) fn touch(&mut self, id: &str) {
        if self.ready.contains_key(id) {
            self.order.retain(|key| key != id);
            self.order.push_back(id.to_owned());
        }
    }
    #[cfg(test)]
    pub(super) fn contains(&self, id: &str) -> bool {
        self.ready.contains_key(id)
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.ready.len()
    }
}

impl Plait {
    pub(super) fn image_row(&self, references: &[ImageRef], cx: &mut Context<Self>) -> AnyElement {
        let thread = self.host.thread.selected.clone().unwrap_or_default();
        let key = (
            self.host.connection.epoch,
            self.host.thread.watch_epoch,
            thread.clone(),
        );
        let mut missing = Vec::new();
        let row = row()
            .flex_wrap()
            .gap_2()
            .children(references.iter().enumerate().map(|(index, reference)| {
                let ready = (self.workspace.images.key.as_ref() == Some(&key))
                    .then(|| self.workspace.images.ready.get(&reference.id))
                    .flatten();
                if ready.is_none()
                    && (self.workspace.images.key.as_ref() != Some(&key)
                        || (!self.workspace.images.pending.contains(&reference.id)
                            && self.workspace.images.pending.len() < 4))
                    && self.host.accepts_watch(key.0, key.1, &key.2)
                {
                    missing.push(reference.clone());
                }
                let image = reference.clone();
                let (width, height) = ready
                    .and_then(|r| r.as_ref().ok())
                    .map(|r| r.dimensions(reference))
                    .unwrap_or((reference.width, reference.height));
                let detail = ready
                    .and_then(|r| r.as_ref().err())
                    .cloned()
                    .unwrap_or_else(|| {
                        format!(
                            "{} × {} · {} bytes · Open image",
                            width, height, reference.size
                        )
                    });
                button(SharedString::from(format!("image-{}", reference.id)), "")
                    .aria_label(format!("Open attached image {}", index + 1))
                    .w(px(96.))
                    .h(px(80.))
                    .p_1()
                    .border_1()
                    .border_color(rgb(colors().border))
                    .child(match ready {
                        Some(Ok(pixels)) => img(pixels.pixels.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        Some(Err(_)) => div().child("Unavailable").into_any_element(),
                        None => div().child("Loading").into_any_element(),
                    })
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
                    .on_activate(cx, move |this, window, cx| {
                        this.enlarge_image(image.clone(), window, cx)
                    })
            }));
        // Schedule only visible rows, after this render has returned.
        if !references.is_empty() {
            let visible = references.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.workspace.images.key.as_ref() == Some(&key) {
                        for id in visible {
                            this.workspace.images.touch(&id);
                        }
                    }
                    if !missing.is_empty() {
                        this.load_images(key, missing, cx);
                    }
                });
            });
        }
        row.into_any_element()
    }
    fn load_images(
        &mut self,
        key: (u64, u64, String),
        references: Vec<ImageRef>,
        cx: &mut Context<Self>,
    ) {
        if !self.host.accepts_watch(key.0, key.1, &key.2) {
            return;
        }
        if self.workspace.images.key.as_ref() != Some(&key) {
            self.workspace.images = ImageCache {
                key: Some(key.clone()),
                ..Default::default()
            };
        }
        for reference in references {
            if self.workspace.images.ready.contains_key(&reference.id)
                || self.workspace.images.pending.contains(&reference.id)
                || self.workspace.images.pending.len() >= 4
            {
                continue;
            }
            self.workspace.images.pending.insert(reference.id.clone());
            let expected = key.clone();
            let id = reference.id.clone();
            let cancelled = self.workspace.cancelled.clone();
            let client = self.host_client();
            let task = cx.background_executor().spawn(async move {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("Image read retired.".into());
                }
                let bytes = client
                    .image(&expected.2, &reference)
                    .map_err(|error| error.to_string())?;
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("Image decode retired.".into());
                }
                decode_image(&bytes, 192)
            });
            let key = key.clone();
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    let current = (
                        this.host.connection.epoch,
                        this.host.thread.watch_epoch,
                        this.host.thread.selected.clone().unwrap_or_default(),
                    );
                    if this.workspace.images.complete(&key, &current, id, result) {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
    pub(super) fn enlarge_image(
        &mut self,
        reference: ImageRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(thread) = self.host.thread.selected.clone() else {
            return;
        };
        let key = (
            self.host.connection.epoch,
            self.host.thread.watch_epoch,
            thread,
        );
        self.workspace.images.large_generation += 1;
        let generation = self.workspace.images.large_generation;
        self.workspace.images.enlarged = Some((reference.clone(), None));
        window.focus(&self.focus, cx);
        cx.notify();
        let expected = key.clone();
        let cancelled = self.workspace.cancelled.clone();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err("Image read retired.".into());
            }
            let bytes = client
                .image(&expected.2, &reference)
                .map_err(|error| error.to_string())?;
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err("Image read retired.".into());
            }
            decode_image(&bytes, 1600)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let actual = (
                    this.host.connection.epoch,
                    this.host.thread.watch_epoch,
                    this.host.thread.selected.clone().unwrap_or_default(),
                );
                if !accepts(&key, &actual) || this.workspace.images.large_generation != generation {
                    return;
                }
                if let Some((_, pixels)) = &mut this.workspace.images.enlarged {
                    *pixels = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn image_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (reference, pixels) = self.workspace.images.enlarged.as_ref()?;
        let bounds = window.viewport_size();
        let (width, height) = pixels
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|r| r.dimensions(reference))
            .unwrap_or((reference.width, reference.height));
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(rgba(0x000000bb))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    column()
                        .w((bounds.width - px(64.)).min(px(1080.)))
                        .gap_2()
                        .p_3()
                        .bg(rgb(colors().panel))
                        .rounded_lg()
                        .child(
                            row()
                                .justify_between()
                                .child(format!("{width} × {height}"))
                                .child(button("close-image", "Close").on_activate(
                                    cx,
                                    |this, window, cx| {
                                        this.workspace.images.enlarged = None;
                                        this.workspace.images.large_generation += 1;
                                        this.focus_composer(window, cx);
                                        cx.notify();
                                    },
                                )),
                        )
                        .child(
                            div()
                                .w_full()
                                .h((bounds.height - px(140.)).max(px(100.)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(match pixels {
                                    Some(Ok(pixels)) => img(pixels.pixels.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain)
                                        .into_any_element(),
                                    Some(Err(error)) => {
                                        div().child(error.clone()).into_any_element()
                                    }
                                    None => div().child("Loading").into_any_element(),
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
impl ImageCache {
    pub(in crate::ui) fn scene(
        key: (u64, u64, String),
        pixels: HashMap<String, Result<Arc<RenderImage>, String>>,
    ) -> Self {
        Self {
            key: Some(key),
            order: pixels.keys().cloned().collect(),
            ready: pixels
                .into_iter()
                .map(|(id, pixels)| (id, pixels.map(Into::into)))
                .collect(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
#[core::prelude::v1::test]
#[ignore = "Run scripts/bench-images.sh with its private offline host"]
fn thumbnail_transfer_bench() {
    use std::time::Instant;
    #[derive(serde::Deserialize)]
    struct Input {
        thread: String,
        image: ImageRef,
    }
    let input: Input = serde_json::from_str(&std::env::var("PLAIT_IMAGE_BENCH").unwrap()).unwrap();
    let client = plait_core::host::Client::from_environment().unwrap();
    let mut samples = Vec::new();
    for index in 0..12 {
        let started = Instant::now();
        let bytes = client.image(&input.thread, &input.image).unwrap();
        let pixels = decode(&bytes, 192).unwrap();
        std::hint::black_box(pixels);
        if index >= 2 {
            samples.push(started.elapsed().as_secs_f64() * 1000.);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "THUMBNAIL_BENCH 10 samples after 2 warmups: median {:.3} ms, p95/max {:.3} ms",
        (samples[4] + samples[5]) / 2.,
        samples[9]
    );
}

#[cfg(test)]
#[core::prelude::v1::test]
fn exif_dimensions_match_the_drawn_orientation() {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode_image(&image::DynamicImage::new_rgb8(80, 40))
        .unwrap();
    // TIFF orientation 6: rotate 90 degrees clockwise.
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut bytes = vec![255, 216, 255, 225];
    bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    bytes.extend_from_slice(exif);
    bytes.extend_from_slice(&jpeg[2..]);
    let reference = images::inspect(&bytes).unwrap();
    assert_eq!((reference.width, reference.height), (80, 40));
    for edge in [192, 1600] {
        let decoded = decode_image(&bytes, edge).unwrap();
        assert_eq!(decoded.dimensions(&reference), (40, 80));
    }
}

#[cfg(test)]
#[core::prelude::v1::test]
#[ignore = "Run scripts/bench-images.sh with its private offline host"]
fn thumbnail_scrollback_bench() {
    use std::time::Instant;
    #[derive(serde::Deserialize)]
    struct Input {
        thread: String,
        images: Vec<ImageRef>,
    }
    let input: Input =
        serde_json::from_str(&std::env::var("PLAIT_IMAGE_SCROLL_BENCH").unwrap()).unwrap();
    assert_eq!(input.images.len(), 50);
    let client = plait_core::host::Client::from_environment().unwrap();
    let key = (1, 1, input.thread.clone());
    let mut cache = ImageCache {
        key: Some(key.clone()),
        ..Default::default()
    };
    for (name, indices) in [
        ("forward", (0..50).collect::<Vec<_>>()),
        ("scroll-back", (0..50).rev().collect()),
    ] {
        let started = Instant::now();
        let mut requests = 0;
        for index in indices {
            let reference = &input.images[index];
            if !cache.contains(&reference.id) {
                let bytes = client.image(&input.thread, reference).unwrap();
                requests += reference.size.div_ceil(images::READ_CHUNK);
                cache.complete(&key, &key, reference.id.clone(), decode_image(&bytes, 192));
            }
            cache.touch(&reference.id);
        }
        if name == "scroll-back" {
            assert_eq!(requests, 0);
        }
        println!(
            "SCROLLBACK_BENCH {name}: {requests} socket requests, {:.3} ms; {} thumbnails retained",
            started.elapsed().as_secs_f64() * 1000.,
            cache.len()
        );
    }
}
