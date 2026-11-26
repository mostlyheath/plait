//! Native presentation of checkout Markdown. Only visible blocks become GPUI elements.
use super::{
    Plait,
    file_view::FileViewer,
    markdown::{highlight, local_spans},
    theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_client::{
    doc_links::LinkTarget,
    file_view::Contents,
    markdown::{
        Block, Kind,
        document::{Document, target},
        restyle,
    },
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};

#[derive(Default)]
pub(super) struct DocumentView {
    pub source: bool,
    pub list: Option<ListState>,
    pub outline: bool,
    outline_cursor: usize,
    outline_scroll: UniformListScrollHandle,
    table_scroll: HashMap<usize, ScrollHandle>,
    pub metadata: bool,
    pub metadata_scroll: UniformListScrollHandle,
    pub anchor: Option<String>,
    pub results: Option<Arc<Vec<String>>>,
    wanted_images: Vec<(String, PathBuf)>,
    attempted_images: HashSet<String>,
    loading_image: Option<Arc<()>>,
    pub pictures: HashMap<String, Arc<RenderImage>>,
}

impl FileViewer {
    pub(super) fn document(&self) -> Option<&Arc<Document>> {
        match &self.opened.as_ref()?.contents {
            Contents::Text(text) => text.document.as_ref(),
            _ => None,
        }
    }
    pub(super) fn rendered(&self) -> bool {
        self.document().is_some() && !self.doc.source
    }
    fn jump(&self, row: usize) {
        if let Some(list) = &self.doc.list {
            list.scroll_to(ListOffset {
                item_ix: self.document().map_or(row, |doc| doc.row_at(row, 0)),
                offset_in_item: px(0.),
            });
        }
    }
    pub(super) fn prepare_document(&mut self) {
        let Some(doc) = self.document().cloned() else {
            return;
        };
        self.doc.table_scroll = doc
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| matches!(block.kind, Kind::Table(_)))
            .map(|(i, _)| (i, ScrollHandle::new()))
            .collect();
        self.doc.wanted_images.clear();
        self.doc.attempted_images.clear();
        self.doc.loading_image = None;
        self.doc.list = Some(ListState::new(doc.rows.len(), ListAlignment::Top, px(160.)));
        if let Some(id) = self.doc.anchor.take()
            && let Some(row) = doc.heading(&id)
        {
            self.jump(row);
        }
    }
}

impl Plait {
    pub(super) fn remember_document_mode(&mut self, path: &std::path::Path, source: bool) {
        if let Some(checkout) = self.state.checkout_key() {
            let key = (checkout.clone(), path.to_owned());
            if source {
                self.document_sources.insert(key);
            } else {
                self.document_sources.remove(&key);
            }
        }
        if self.workspace.file_viewer.path.as_deref() == Some(path) {
            self.workspace.file_viewer.doc.source = source;
        }
    }

    pub(super) fn checkout_document_buttons(&self, path: PathBuf, cx: &mut Context<Self>) -> Div {
        let mut modes = segmented();
        for (id, label, source) in [
            ("checkout-rendered", "Rendered", false),
            ("checkout-source", "Source", true),
        ] {
            let path = path.clone();
            modes = modes.child(
                segment(id, label, false)
                    .aria_label(format!("Open checkout file in {label}"))
                    .on_activate(cx, move |this, window, cx| {
                        this.remember_document_mode(&path, source);
                        this.open_file_in_view(path.clone(), window, cx);
                    }),
            );
        }
        modes
    }

    pub(super) fn load_document_images(&mut self, cx: &mut Context<Self>) {
        let viewer = &self.workspace.file_viewer;
        let Some(list) = viewer.doc.list.as_ref() else {
            return;
        };
        let top = list.logical_scroll_top().item_ix;
        list.set_scroll_handler(cx.listener(|this, event: &ListScrollEvent, _, cx| {
            this.request_document_images(event.visible_range.clone(), cx);
        }));
        self.request_document_images(top..top.saturating_add(48), cx);
    }

    fn request_document_images(&mut self, visible: Range<usize>, cx: &mut Context<Self>) {
        let viewer = &mut self.workspace.file_viewer;
        if !viewer.rendered() {
            return;
        }
        let (Some(doc), Some(path)) = (viewer.document().cloned(), &viewer.path) else {
            return;
        };
        let Some(first) = doc.rows.get(visible.start) else {
            return;
        };
        let last = doc
            .rows
            .get(visible.end.saturating_sub(1))
            .map_or(usize::MAX, |r| r.block);
        let start = doc.images.partition_point(|(b, _)| *b < first.block);
        let mut seen = HashSet::new();
        let wanted: Vec<_> = doc.images[start..]
            .iter()
            .take_while(|(b, _)| *b <= last)
            .filter_map(|(_, href)| {
                if !seen.insert(href) {
                    return None;
                }
                match target(href, path, &viewer.root) {
                    LinkTarget::Document { path, .. } => Some((href.clone(), path)),
                    _ => None,
                }
            })
            .take(16)
            .collect();
        viewer
            .doc
            .pictures
            .retain(|href, _| wanted.iter().any(|(h, _)| h == href));
        viewer
            .doc
            .attempted_images
            .retain(|href| wanted.iter().any(|(h, _)| h == href));
        viewer.doc.wanted_images = wanted;
        self.read_next_document_image(cx);
    }

    fn read_next_document_image(&mut self, cx: &mut Context<Self>) {
        let viewer = &mut self.workspace.file_viewer;
        if viewer.doc.loading_image.is_some() {
            return;
        }
        let Some((href, path)) = viewer
            .doc
            .wanted_images
            .iter()
            .find(|(h, _)| !viewer.doc.attempted_images.contains(h))
            .cloned()
        else {
            return;
        };
        let Some(opened) = viewer.opened.clone() else {
            return;
        };
        let pending = Arc::new(());
        viewer.doc.loading_image = Some(pending.clone());
        viewer.doc.attempted_images.insert(href.clone());
        let root = viewer.root.clone();
        let backend = self.repository_backend();
        let key = self.state.checkout_key().cloned();
        let task = cx.background_executor().spawn(async move {
            let bytes = backend.document_image(&root, key.as_ref(), &path)?;
            super::images::document_image(&bytes)
        });
        cx.spawn(async move |this, cx| {
            let picture = task.await;
            let _ = this.update(cx, |this, cx| {
                let viewer = &mut this.workspace.file_viewer;
                if !viewer
                    .opened
                    .as_ref()
                    .is_some_and(|now| Arc::ptr_eq(now, &opened))
                {
                    return;
                }
                if !viewer
                    .doc
                    .loading_image
                    .as_ref()
                    .is_some_and(|now| Arc::ptr_eq(now, &pending))
                {
                    return;
                }
                viewer.doc.loading_image = None;
                if let Ok(pixels) = picture
                    && viewer.doc.attempted_images.contains(&href)
                {
                    viewer.doc.pictures.insert(href, pixels);
                    if let Some(list) = &viewer.doc.list {
                        let top = list.logical_scroll_top().item_ix;
                        list.remeasure_items(top..top.saturating_add(64).min(list.item_count()));
                    }
                    cx.notify();
                }
                this.read_next_document_image(cx);
            });
        })
        .detach();
    }

    pub(super) fn follow_document(&mut self, href: &str, cx: &mut Context<Self>) {
        let viewer = &self.workspace.file_viewer;
        let Some(path) = &viewer.path else { return };
        match target(href, path, &viewer.root) {
            LinkTarget::Web(url) | LinkTarget::Mail(url) => cx.open_url(&url),
            LinkTarget::Anchor(id) => {
                if let Some(row) = viewer.document().and_then(|d| d.heading(&id)) {
                    viewer.jump(row);
                    self.load_document_images(cx);
                    cx.notify();
                }
            }
            LinkTarget::Document { path, anchor } => {
                self.remember_document_mode(&path, false);
                self.open_file(path, None, cx);
                let viewer = &mut self.workspace.file_viewer;
                if let Some(doc) = viewer.document() {
                    if let Some(row) = anchor.as_deref().and_then(|a| doc.heading(a)) {
                        viewer.jump(row);
                    }
                } else {
                    viewer.doc.anchor = anchor;
                }
            }
            _ => {}
        }
    }

    pub(super) fn check_document_links(&mut self, cx: &mut Context<Self>) {
        let viewer = &self.workspace.file_viewer;
        let (Some(doc), Some(path), Some(opened)) = (
            viewer.document().cloned(),
            viewer.path.clone(),
            viewer.opened.clone(),
        ) else {
            return;
        };
        let root = viewer.root.clone();
        let key = self.state.checkout_key().cloned();
        let backend = self.repository_backend();
        let pending = Arc::new(vec!["Checking links…".into()]);
        self.workspace.file_viewer.doc.results = Some(pending.clone());
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            doc.check_links(&path, &root, |path, headings| {
                if !headings {
                    backend.file_stamp(&root, key.as_ref(), path)?;
                    return Ok(None);
                }
                match backend.file(&root, key.as_ref(), path)?.contents {
                    Contents::Text(text) => Ok(text
                        .document
                        .map(|doc| doc.anchors.keys().cloned().collect())),
                    _ => Err("Cannot check headings in this file.".into()),
                }
            })
        });
        cx.spawn(async move |this, cx| {
            let mut issues = task.await;
            if issues.is_empty() {
                issues.push("No broken local links.".into());
            }
            let _ = this.update(cx, |this, cx| {
                let viewer = &mut this.workspace.file_viewer;
                if viewer
                    .opened
                    .as_ref()
                    .is_some_and(|now| Arc::ptr_eq(now, &opened))
                    && viewer
                        .doc
                        .results
                        .as_ref()
                        .is_some_and(|now| Arc::ptr_eq(now, &pending))
                {
                    viewer.doc.results = Some(Arc::new(issues));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn document_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let viewer = &self.workspace.file_viewer;
        let rendered = viewer.rendered();
        let mut modes = segmented();
        for (id, label, on) in [
            ("file-rendered", "Rendered", true),
            ("file-source", "Source", false),
        ] {
            let selected = on == rendered;
            modes = modes.child(segment(id, label, selected).on_activate(
                cx,
                move |this, _, cx| {
                    if let Some(path) = this.workspace.file_viewer.path.clone() {
                        this.remember_document_mode(&path, !on);
                    }
                    if on {
                        this.load_document_images(cx);
                    }
                    this.update_find(cx);
                    cx.notify();
                },
            ));
        }
        let mut bar = row().flex_wrap().gap_1().px_4().child(modes);
        if rendered {
            bar = bar.child(self.outline_control(cx)).child(
                button("file-check-links", "Check Links")
                    .on_activate(cx, |this, _, cx| this.check_document_links(cx)),
            );
        }
        bar.into_any_element()
    }

    fn current_heading(&self, doc: &Document) -> usize {
        let viewer = &self.workspace.file_viewer;
        let top = viewer
            .doc
            .list
            .as_ref()
            .and_then(|list| doc.rows.get(list.logical_scroll_top().item_ix));
        doc.headings
            .partition_point(|&block| top.is_some_and(|row| block <= row.block))
            .saturating_sub(1)
    }

    fn outline_control(&self, cx: &mut Context<Self>) -> Div {
        let viewer = &self.workspace.file_viewer;
        let doc = viewer.document().expect("rendered document").clone();
        let current = self.current_heading(&doc);
        let open = viewer.doc.outline;
        let trigger = button("file-outline", "Outline").on_activate(cx, move |this, window, cx| {
            let state = &mut this.workspace.file_viewer.doc;
            state.outline = !open;
            state.outline_cursor = current;
            state
                .outline_scroll
                .scroll_to_item(current, ScrollStrategy::Nearest);
            window.focus(&this.focus, cx);
            cx.notify();
        });
        let anchor = div().relative().child(trigger);
        if !viewer.doc.outline {
            return anchor;
        }
        let count = doc.headings.len();
        let cursor = viewer.doc.outline_cursor;
        let menu = column()
            .id("outline-menu")
            .role(Role::Menu)
            .aria_label("Document outline")
            .audited()
            .absolute()
            .top(px(30.))
            .right_0()
            .w(px(192.))
            .p_1()
            .bg(rgb(colors().raised))
            .border_1()
            .border_color(rgb(colors().edge))
            .rounded(px(SURFACE_CORNER))
            .shadow(floating_shadow())
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.workspace.file_viewer.doc.outline = false;
                cx.notify();
            }))
            .when(count == 0, |el| el.child("No headings"))
            .child(
                uniform_list(
                    "document-outline",
                    count,
                    cx.processor(move |_, range: Range<usize>, _, cx| {
                        range
                            .map(|i| {
                                let block = doc.headings[i];
                                let heading = &doc.blocks[block];
                                let Kind::Heading(level) = heading.kind else {
                                    unreachable!()
                                };
                                let label = heading.text.clone();
                                button(("document-heading", i), label)
                                    .role(Role::MenuItem)
                                    .aria_selected(i == current)
                                    .w_full()
                                    .justify_start()
                                    .pl(px(8. + 10. * f32::from(level - 1)))
                                    .border_l_2()
                                    .border_color(rgb(if i == current {
                                        colors().blue
                                    } else {
                                        colors().raised
                                    }))
                                    .when(i == cursor, |el| el.bg(rgb(colors().selected)))
                                    .on_activate(cx, move |this, _, cx| {
                                        this.choose_document_heading(block, cx)
                                    })
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&viewer.doc.outline_scroll)
                .h(px(28. * count.min(8) as f32)),
            );
        anchor.child(deferred(menu))
    }

    pub(super) fn outline_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let viewer = &mut self.workspace.file_viewer;
        if !viewer.doc.outline || !viewer.rendered() {
            return false;
        }
        let doc = viewer.document().unwrap().clone();
        let cursor = &mut viewer.doc.outline_cursor;
        match key {
            "escape" => viewer.doc.outline = false,
            "up" => *cursor = cursor.saturating_sub(1),
            "down" => *cursor = (*cursor + 1).min(doc.headings.len().saturating_sub(1)),
            "enter" => {
                if let Some(&block) = doc.headings.get(*cursor) {
                    self.choose_document_heading(block, cx);
                }
                return true;
            }
            _ => return false,
        }
        viewer
            .doc
            .outline_scroll
            .scroll_to_item(*cursor, ScrollStrategy::Nearest);
        cx.notify();
        true
    }

    pub(super) fn choose_document_heading(&mut self, block: usize, cx: &mut Context<Self>) {
        let viewer = &mut self.workspace.file_viewer;
        viewer.doc.outline = false;
        viewer.jump(block);
        self.load_document_images(cx);
        cx.notify();
    }

    pub(super) fn document_body(&self, doc: &Arc<Document>, cx: &mut Context<Self>) -> AnyElement {
        let viewer = &self.workspace.file_viewer;
        let mut body = column().flex_1().min_h_0();
        if let Some(results) = &viewer.doc.results {
            let results = results.clone();
            body = body
                .child(
                    button("close-link-results", "Close link results").on_activate(
                        cx,
                        |this, _, cx| {
                            this.workspace.file_viewer.doc.results = None;
                            cx.notify();
                        },
                    ),
                )
                .child(
                    uniform_list("link-results", results.len(), {
                        let results = results.clone();
                        move |range: Range<usize>, _, _| {
                            range
                                .map(|i| div().truncate().child(results[i].clone()))
                                .collect::<Vec<_>>()
                        }
                    })
                    .h(px(24. * results.len().min(6) as f32)),
                );
        }
        if doc.metadata.is_some() {
            body = body.child(button("document-metadata", "Metadata").on_activate(
                cx,
                |this, _, cx| {
                    let d = &mut this.workspace.file_viewer.doc;
                    d.metadata = !d.metadata;
                    this.update_find(cx);
                    cx.notify();
                },
            ));
            if viewer.doc.metadata {
                let doc = doc.clone();
                let n = doc.metadata_rows.len();
                body = body.child(
                    uniform_list(
                        "metadata-text",
                        n,
                        cx.processor(move |this, range: Range<usize>, _, _| {
                            let meta = doc.metadata.as_deref().unwrap_or_default();
                            range
                                .map(|i| {
                                    let bytes = &doc.metadata_rows[i];
                                    let start = doc.text.len() + bytes.start;
                                    let end = doc.text.len() + bytes.end;
                                    let runs = this.document_hits(start..end, Vec::new());
                                    div().font_family(mono()).child(
                                        StyledText::new(meta[bytes.clone()].to_owned())
                                            .with_highlights(runs),
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&viewer.doc.metadata_scroll)
                    .h(px(20. * n.min(6) as f32)),
                );
            }
        }
        let state = viewer
            .doc
            .list
            .clone()
            .unwrap_or_else(|| ListState::new(doc.rows.len(), ListAlignment::Top, px(160.)));
        let doc = doc.clone();
        body.child(
            list(
                state,
                cx.processor(move |this, index, _, cx| this.document_block(&doc, index, cx)),
            )
            .flex_1()
            .min_h_0(),
        )
        .into_any_element()
    }

    fn document_block(&self, doc: &Document, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let fragment = &doc.rows[index];
        let block_index = fragment.block;
        let block = &doc.blocks[block_index];
        let offset = doc.ranges[block_index].start;
        let text =
            |range, cx: &mut Context<Self>| self.document_span(block, range, index, offset, cx);
        let mut line = column()
            .w_full()
            .max_w(px(540.))
            .px_4()
            .py_1()
            .min_w_0()
            .text_size(px(TYPE_BODY_SIZE))
            .line_height(px(TYPE_BODY_LINE));
        match &block.kind {
            Kind::Rule => line = line.child(div().border_b_1().border_color(rgb(colors().border))),
            Kind::Table(table) => {
                let i = fragment.table_row.expect("table row");
                let cells = &table.rows[i];
                let width = |c: &plait_client::markdown::Column| {
                    (c.width as f32 * 9.5 + 32.).clamp(56., 360.)
                };
                let total_width = table.columns.iter().map(width).sum::<f32>();
                let cells =
                    div()
                        .flex()
                        .items_stretch()
                        .w(px(total_width))
                        .when(i == 0, |el| {
                            el.bg(rgb(colors().panel)).font_weight(FontWeight::MEDIUM)
                        })
                        .children(cells.iter().zip(&table.columns).enumerate().map(
                            |(col, (r, c))| {
                                let align = match c.align {
                                    plait_client::markdown::Align::Left => TextAlign::Left,
                                    plait_client::markdown::Align::Center => TextAlign::Center,
                                    plait_client::markdown::Align::Right => TextAlign::Right,
                                };
                                div()
                                    .flex_shrink_0()
                                    .text_align(align)
                                    .w(px(width(c)))
                                    .px_3()
                                    .py_1()
                                    .border_r_1()
                                    .border_b_1()
                                    .when(col == 0, |el| el.border_l_1())
                                    .when(i == 0, |el| el.border_t_1())
                                    .border_color(rgb(colors().border))
                                    .child(text(r.clone(), cx))
                            },
                        ));
                line = line.py_0().child(
                    div()
                        .id(("document-table", index))
                        .overflow_x_scroll()
                        .track_scroll(&self.workspace.file_viewer.doc.table_scroll[&block_index])
                        .child(cells),
                );
            }
            Kind::List { depth, marker }
                if fragment.bytes.start == 0
                    && matches!(block.text.get(..*marker), Some("☑ " | "☐ ")) =>
            {
                let checked = block.text.starts_with('☑');
                let mark = div()
                    .flex_shrink_0()
                    .size(px(14.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(colors().edge))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(checked, |el| {
                        el.bg(rgb(colors().blue))
                            .text_color(rgb(colors().panel))
                            .text_size(px(12.))
                            .line_height(px(14.))
                            .child("✓")
                    });
                line = line.pl(px(16. + *depth as f32 * 20.)).child(
                    row()
                        .child(mark)
                        .child(text(*marker..fragment.bytes.end, cx)),
                );
            }
            kind => {
                line = match kind {
                    Kind::Heading(level) => line
                        .pt_3()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(if *level == 1 { 20. } else { 16. })),
                    Kind::Code(_) => line.font_family(mono()).bg(rgb(colors().panel)),
                    Kind::Quote => line
                        .border_l_2()
                        .border_color(rgb(colors().edge))
                        .text_color(rgb(colors().muted)),
                    Kind::List { depth, .. } => line.pl(px(16. + *depth as f32 * 20.)),
                    _ => line,
                }
                .child(text(fragment.bytes.clone(), cx));
            }
        }
        if fragment.bytes.start != 0 {
            return line.into_any_element();
        }
        let first = doc.images.partition_point(|(row, _)| *row < block_index);
        for (_, href) in doc.images[first..]
            .iter()
            .take_while(|(row, _)| *row == block_index)
        {
            if let Some(image) = self.workspace.file_viewer.doc.pictures.get(href) {
                line = line.child(
                    img(image.clone())
                        .w(px(image.size(0).width.0 as f32))
                        .max_w_full()
                        .max_h(px(480.))
                        .object_fit(ObjectFit::Contain),
                );
            }
        }
        line.into_any_element()
    }

    fn document_hits(
        &self,
        range: Range<usize>,
        mut runs: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let find = &self.workspace.file_viewer.find;
        let base = range.start;
        let end = range.end;
        let start = find.matches.partition_point(|hit| hit.end <= base);
        for (i, hit) in find.matches[start..]
            .iter()
            .enumerate()
            .take_while(|(_, h)| h.start < end)
        {
            restyle(
                &mut runs,
                hit.start.max(base) - base..hit.end.min(end) - base,
                |s| super::file_view::find_highlight(s, start + i == find.current),
            );
        }
        runs
    }
    fn document_span(
        &self,
        block: &Block,
        range: Range<usize>,
        index: usize,
        offset: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spans = local_spans(block, &range);
        let runs: Vec<_> = spans
            .iter()
            .map(|(r, s)| (r.clone(), highlight(s)))
            .collect();
        let links: Vec<_> = spans
            .iter()
            .filter_map(|(r, s)| Some((r.clone(), block.links.get(usize::from(s.link?))?.clone())))
            .collect();
        let runs = self.document_hits(offset + range.start..offset + range.end, runs);
        let value = StyledText::new(block.text[range.clone()].to_owned())
            .with_highlights(runs)
            .with_font_family_overrides(
                spans
                    .iter()
                    .filter(|(_, s)| s.code)
                    .map(|(r, _)| (r.clone(), mono())),
            );
        let tips = links.clone();
        let entity = cx.weak_entity();
        super::a11y::linked_text(
            SharedString::from(format!("document-{index}-{}", range.start)),
            value,
            |element| {
                element
                    .tooltip(move |byte, _, cx| {
                        let (_, href) = tips.iter().find(|(r, _)| r.contains(&byte))?;
                        Some(cx.new(|_| TextTooltip(href.clone())).into())
                    })
                    .on_click(
                        links.iter().map(|(r, _)| r.clone()).collect(),
                        move |i, _, cx| {
                            let _ =
                                entity.update(cx, |this, cx| this.follow_document(&links[i].1, cx));
                        },
                    )
            },
        )
        .into_any_element()
    }
}
