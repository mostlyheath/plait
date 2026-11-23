//! Presentation for precomputed conversation blocks. Parsing never runs here.

use super::{
    Plait,
    selection::{Selectable, Site, selection_fill},
    theme::*,
};
use gpui::{prelude::*, *};
use plait_client::{
    diff::Change,
    highlight::Token,
    markdown::{self, Align, Block, DiffLine, Kind, Style, Table},
};
use std::ops::Range;

/// Left inset for each level of list nesting, and the width of a bullet column.
const LIST_STEP: f32 = 20.;
/// Width given to one character of a table cell, wide enough for monospace code.
const CELL_CHARACTER: f32 = 8.4;
const CELL_PADDING: f32 = 10.;

/// Syntax classes drawn with the existing palette; see DESIGN.md.
pub(super) fn token_color(token: Token) -> u32 {
    match token {
        Token::Comment => colors().subtle,
        Token::Keyword => colors().violet,
        Token::String | Token::Inserted => colors().mint,
        Token::Number => colors().amber,
        Token::Type | Token::Property | Token::Meta => colors().blue,
        Token::Function | Token::Heading => colors().text,
        Token::Punctuation => colors().muted,
        Token::Deleted => colors().red,
    }
}

fn highlight(style: &Style) -> HighlightStyle {
    let link = style.link.is_some();
    let color = if link {
        Some(colors().blue)
    } else {
        style.token.map(token_color)
    };
    HighlightStyle {
        color: color.map(|color| rgb(color).into()),
        font_weight: (style.strong || style.token == Some(Token::Heading))
            .then_some(FontWeight::BOLD),
        font_style: style.emphasis.then_some(FontStyle::Italic),
        background_color: style.code.then(|| rgb(colors().panel).into()),
        // A link is underlined as well as tinted, so it never depends on color.
        underline: link.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(rgb(colors().blue).into()),
            wavy: false,
        }),
        strikethrough: style.strike.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: None,
        }),
        ..HighlightStyle::default()
    }
}

/// Spans are sorted and non-overlapping, so only this slice can intersect.
pub(super) fn overlapping_spans<'a>(
    block: &'a Block,
    range: &Range<usize>,
) -> &'a [(Range<usize>, Style)] {
    if range.is_empty() {
        return &block.spans[..0];
    }
    let start = block
        .spans
        .partition_point(|(span, _)| span.end <= range.start);
    let end = block
        .spans
        .partition_point(|(span, _)| span.start < range.end)
        .max(start);
    &block.spans[start..end]
}

/// Clip a block's spans to `range` and make them relative to it.
fn local_spans(block: &Block, range: &Range<usize>) -> Vec<(Range<usize>, Style)> {
    overlapping_spans(block, range)
        .iter()
        .map(|(span, style)| {
            let local =
                span.start.max(range.start) - range.start..span.end.min(range.end) - range.start;
            (local, *style)
        })
        .collect()
}

impl Plait {
    /// Styled, selectable text for `range` of a block.
    pub(super) fn block_text(
        &self,
        site: &Site,
        block: &Block,
        range: Range<usize>,
        by_line: bool,
    ) -> Selectable {
        let spans = local_spans(block, &range);
        let code: Vec<_> = spans
            .iter()
            .filter(|(_, style)| style.code)
            .map(|(range, _)| (range.clone(), mono()))
            .collect();
        let mut runs: Vec<_> = spans
            .iter()
            .map(|(range, style)| (range.clone(), highlight(style)))
            .collect();
        // Found text first, so a selection over it still reads as selected.
        self.mark_found(site, &range, &mut runs);
        if let Some(selected) = self.workspace.reading.selected(
            self.workspace.live_layout.raw_index(site.row),
            site.block,
            block,
            &range,
        ) {
            markdown::restyle(&mut runs, selected, |style| {
                style.background_color = Some(selection_fill())
            });
        }
        let text = StyledText::new(SharedString::from(block.text[range.clone()].to_owned()))
            .with_highlights(runs)
            .with_font_family_overrides(code);
        self.selectable(site, block, range, text, by_line, &spans)
    }

    /// The monospace body of a code block: one line per source line, scrolling sideways.
    pub(super) fn code_text(&self, site: &Site, block: &Block, id: ElementId) -> Stateful<Div> {
        div()
            .id(id)
            .w_full()
            .overflow_x_scroll()
            .font_family(mono())
            .text_size(px(code_size()))
            .line_height(px(code_line_height()))
            .whitespace_nowrap()
            .child(self.block_text(site, block, 0..block.text.len(), true))
    }

    fn code_block(
        &self,
        site: &Site,
        block: &Block,
        language: &str,
        key: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copy = block.text.clone();
        let group = SharedString::from(format!("code-group-{key}"));
        column()
            .group(group.clone())
            .w_full()
            .min_w_0()
            .rounded_lg()
            .bg(rgb(colors().rail))
            .border_1()
            .border_color(rgb(colors().border))
            .overflow_hidden()
            .child(
                row()
                    .pl_3()
                    .pr_1()
                    .h(px(ROW_HEIGHT))
                    .justify_between()
                    .child(tag(
                        SharedString::from(format!("code-lang-{key}")),
                        if language.is_empty() {
                            SharedString::from("Code")
                        } else {
                            SharedString::from(language.to_owned())
                        },
                        colors().subtle,
                    ))
                    .child(
                        quiet_button(
                            SharedString::from(format!("copy-code-{key}")),
                            "Copy code",
                            group,
                        )
                        .aria_label("Copy code block")
                        .on_activate(cx, move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }),
                    ),
            )
            .child(
                self.code_text(
                    site,
                    block,
                    SharedString::from(format!("code-{key}")).into(),
                )
                .px_3()
                .pb_2(),
            )
            .into_any_element()
    }

    fn list_item(&self, site: &Site, block: &Block, depth: usize, marker: usize) -> AnyElement {
        let marker = marker.min(block.text.len());
        div()
            .flex()
            .items_start()
            .w_full()
            .min_w_0()
            .pl(px(depth as f32 * LIST_STEP))
            .child(
                prose()
                    .flex_shrink_0()
                    .min_w(px(LIST_STEP))
                    .pr(px(6.))
                    .text_color(rgb(colors().muted))
                    .child(SharedString::from(
                        block.text[..marker].trim_end().to_owned(),
                    )),
            )
            .child(prose().flex_1().min_w_0().child(self.block_text(
                site,
                block,
                marker..block.text.len(),
                false,
            )))
            .into_any_element()
    }

    fn table_cell(
        &self,
        site: &Site,
        block: &Block,
        range: Range<usize>,
        width: f32,
        align: Align,
    ) -> Div {
        let cell = div()
            .flex()
            .w(px(width))
            .flex_shrink_0()
            .px(px(CELL_PADDING))
            .py(px(5.));
        // Text stays left-aligned inside its own box, so hit-testing is the
        // same in every column; the box moves instead.
        match align {
            Align::Left => cell,
            Align::Center => cell.justify_center(),
            Align::Right => cell.justify_end(),
        }
        .child(
            div()
                .min_w_0()
                .child(self.block_text(site, block, range, false)),
        )
    }

    /// Cells keep a width estimated from their text, so every row lines up
    /// without measuring; a table wider than the column scrolls sideways.
    fn table(&self, site: &Site, block: &Block, table: &Table, key: &str) -> AnyElement {
        let widths: Vec<f32> = table
            .columns
            .iter()
            .map(|column| {
                (column.width as f32 * CELL_CHARACTER + 2. * CELL_PADDING).clamp(56., 360.)
            })
            .collect();
        let total: f32 = widths.iter().sum();
        let rows = table.rows.iter().enumerate().map(|(index, cells)| {
            div()
                .flex()
                .items_stretch()
                .when(index > 0, |el| {
                    el.border_t_1().border_color(rgb(colors().border))
                })
                .when(index == 0, |el| {
                    el.bg(rgb(colors().rail)).font_weight(FontWeight::MEDIUM)
                })
                .children(cells.iter().zip(&widths).zip(&table.columns).map(
                    |((cell, width), column)| {
                        self.table_cell(site, block, cell.clone(), *width, column.align)
                    },
                ))
        });
        // The outer row lets a narrow table keep its own width.
        div()
            .flex()
            .w_full()
            .min_w_0()
            .child(
                div()
                    .id(SharedString::from(format!("table-{key}")))
                    .flex()
                    .min_w_0()
                    .max_w_full()
                    .overflow_x_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors().border))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .child(column().flex_none().w(px(total)).children(rows)),
            )
            .into_any_element()
    }

    /// A line diff: removals then additions, each marked in a gutter and tinted.
    pub(super) fn diff(
        &self,
        site: &Site,
        block: &Block,
        lines: &[DiffLine],
        key: &str,
    ) -> AnyElement {
        let rows = lines.iter().map(|line| {
            let (mark, tint, fill) = match line.change {
                Change::Context => (" ", colors().muted, None),
                Change::Removed => ("−", colors().red, Some(colors().removed_fill)),
                Change::Added => ("+", colors().mint, Some(colors().added_fill)),
            };
            div()
                .flex()
                .when_some(fill, |el, fill| el.bg(rgba(fill)))
                .child(
                    div()
                        .w(px(18.))
                        .flex_shrink_0()
                        .text_center()
                        .text_color(rgb(tint))
                        .child(mark),
                )
                .child(
                    div()
                        .pr_3()
                        .text_color(rgb(if line.change == Change::Context {
                            colors().muted
                        } else {
                            colors().text
                        }))
                        .child(self.block_text(site, block, line.range.clone(), false)),
                )
        });
        div()
            .id(SharedString::from(format!("diff-{key}")))
            .flex()
            .w_full()
            .overflow_x_scroll()
            .py_1()
            .font_family(mono())
            .text_size(px(code_size()))
            .line_height(px(code_line_height()))
            .whitespace_nowrap()
            // Tints reach the widest line, or the edge when every line is short.
            .child(column().flex_none().min_w_full().children(rows))
            .into_any_element()
    }

    fn block(&self, site: &Site, block: &Block, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let whole = 0..block.text.len();
        let body = match &block.kind {
            Kind::Code(language) => self.code_block(site, block, language, key, cx),
            Kind::Rule => div()
                .w_full()
                .my_1()
                .border_b_1()
                .border_color(rgb(colors().border))
                .into_any_element(),
            Kind::List { depth, marker } => self.list_item(site, block, *depth, *marker),
            Kind::Table(table) => self.table(site, block, table, key),
            Kind::Diff(lines) => self.diff(site, block, lines, key),
            // Keep reply headings within the interface's compact type scale.
            Kind::Heading(level) => prose()
                .w_full()
                .min_w_0()
                .font_weight(FontWeight::BOLD)
                .text_size(px(match level {
                    1 => 15.,
                    2 => 14.,
                    _ => 13.,
                }))
                .line_height(px(22.))
                .child(self.block_text(site, block, whole, false))
                .into_any_element(),
            Kind::Quote => prose()
                .w_full()
                .min_w_0()
                .pl_3()
                .border_l_2()
                .border_color(rgb(colors().edge))
                .text_color(rgb(colors().muted))
                .child(self.block_text(site, block, whole, false))
                .into_any_element(),
            Kind::Paragraph => prose()
                .w_full()
                .min_w_0()
                .child(self.block_text(site, block, whole, false))
                .into_any_element(),
        };
        if block.indent == 0 {
            return body;
        }
        div()
            .w_full()
            .min_w_0()
            .pl(px(block.indent as f32 * LIST_STEP))
            .child(body)
            .into_any_element()
    }

    /// Keep instructions literal; only quoted context gets a separate inset.
    pub(super) fn request(&self, site: &Site, blocks: &[Block]) -> AnyElement {
        if let [block] = blocks
            && !matches!(block.kind, Kind::Quote)
        {
            return prose()
                .w_full()
                .min_w_0()
                .child(self.block_text(site, block, 0..block.text.len(), false))
                .into_any_element();
        }
        let blocks = blocks.iter().enumerate().map(|(index, block)| {
            let site = Site {
                block: index,
                ..site.clone()
            };
            prose()
                .w_full()
                .min_w_0()
                .when(matches!(block.kind, Kind::Quote), |el| {
                    el.pl_3()
                        .border_l_1()
                        .border_color(rgb(colors().edge))
                        .text_color(rgb(colors().muted))
                })
                .child(self.block_text(&site, block, 0..block.text.len(), false))
        });
        column()
            .w_full()
            .min_w_0()
            .gap_2()
            .children(blocks.collect::<Vec<_>>())
            .into_any_element()
    }

    /// Every block of one row. `key` is unique to the row, for element IDs.
    pub(super) fn markdown(
        &self,
        site: &Site,
        blocks: &[Block],
        key: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.markdown_with(site, blocks, None, key, cx)
    }

    /// `blocks`, then `tail` as the next block: a streaming reply's revealed part.
    pub(super) fn markdown_with(
        &self,
        site: &Site,
        blocks: &[Block],
        tail: Option<&Block>,
        key: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut previous: Option<&Kind> = None;
        let blocks = blocks.iter().chain(tail).enumerate().map(|(index, block)| {
            let site = Site {
                block: index,
                ..site.clone()
            };
            // Items of one list sit close together; a heading opens a section.
            let gap = match (previous, &block.kind) {
                (None, _) => 0.,
                (Some(Kind::List { .. }), Kind::List { .. }) => 2.,
                (_, Kind::Heading(_)) => 14.,
                _ => 8.,
            };
            previous = Some(&block.kind);
            div().w_full().min_w_0().mt(px(gap)).child(self.block(
                &site,
                block,
                &format!("{key}-{index}"),
                cx,
            ))
        });
        column()
            .w_full()
            .min_w_0()
            .children(blocks.collect::<Vec<_>>())
            .into_any_element()
    }
}

#[cfg(test)]
mod span_tests {
    use super::{Block, Style, overlapping_spans};

    #[test]
    fn overlapping_slice_tracks_range_and_boundaries() {
        let block = Block {
            spans: vec![
                (0..3, Style::default()),
                (3..6, Style::default()),
                (8..10, Style::default()),
            ],
            ..Block::default()
        };
        assert_eq!(overlapping_spans(&block, &(3..5)).len(), 1);
        assert_eq!(overlapping_spans(&block, &(5..9)).len(), 2);
        assert!(overlapping_spans(&block, &(6..8)).is_empty());
        assert!(overlapping_spans(&block, &(4..4)).is_empty());
    }
}
