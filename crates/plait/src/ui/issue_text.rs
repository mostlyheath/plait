//! Linear's Markdown, drawn for reading. An issue's description and comments
//! are parsed on the background executor when the issue arrives, and drawn
//! here without selection or Find. Nothing in them is followed: a link shows
//! where it points, and Open in Linear is the way out.
use super::{markdown::token_color, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    host::linear::IssueDetail,
    markdown::{Block, Kind, Message, Style},
};
use std::ops::Range;

/// Text drawn from Linear: wide enough to read, never the whole window.
pub(super) const READING_WIDTH: f32 = 720.;
const BODY_SIZE: f32 = 14.;
const BODY_LINE: f32 = 22.;
const LIST_STEP: f32 = 20.;
/// The parser writes an image as `[image: alt]`; Plait does not load it.
const IMAGE_PREFIX: &str = "[image: ";

/// One issue's text, parsed once for the revision it came from.
pub(super) struct IssueText {
    issue: String,
    updated_at: String,
    pub description: Vec<Block>,
    pub comments: Vec<Vec<Block>>,
}

impl IssueText {
    /// Runs off the UI thread; Linear text is at most 64 KiB plus 25 comments.
    pub(super) fn parse(detail: &IssueDetail) -> Self {
        let description = detail
            .description
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(|text| Message::parse(text).blocks)
            .unwrap_or_default();
        Self {
            issue: detail.summary.id.clone(),
            updated_at: detail.summary.updated_at.clone(),
            description,
            comments: detail
                .comments
                .iter()
                .map(|comment| Message::parse(&comment.body).blocks)
                .collect(),
        }
    }

    /// Whether this was parsed from `detail`'s revision.
    pub(super) fn matches(&self, detail: &IssueDetail) -> bool {
        self.issue == detail.summary.id && self.updated_at == detail.summary.updated_at
    }
}

/// Every block, spaced as the conversation spaces them.
pub(super) fn blocks(blocks: &[Block], key: &str) -> Div {
    let mut previous: Option<&Kind> = None;
    column().w_full().min_w_0().children(
        blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let gap = match (previous, &block.kind) {
                    (None, _) => 0.,
                    (Some(Kind::List { .. }), Kind::List { .. }) => 4.,
                    (_, Kind::Heading(_)) => 18.,
                    _ => 10.,
                };
                previous = Some(&block.kind);
                div()
                    .w_full()
                    .min_w_0()
                    .mt(px(gap))
                    .pl(px(block.indent as f32 * LIST_STEP))
                    .child(self::block(block, &format!("{key}-{index}")))
            })
            .collect::<Vec<_>>(),
    )
}

fn block(block: &Block, key: &str) -> AnyElement {
    match &block.kind {
        Kind::Heading(level) => {
            let size = match level {
                1 => 20.,
                2 => 17.,
                _ => 15.,
            };
            div()
                .w_full()
                .min_w_0()
                .text_size(px(size))
                .line_height(px(size + 8.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(colors().text))
                .child(inline(block, 0..block.text.len()))
                .into_any_element()
        }
        Kind::Code(_) | Kind::Diff(_) | Kind::Table(_) => code(block, key),
        Kind::Quote => prose_block(block, 0..block.text.len())
            .pl_3()
            .border_l_2()
            .border_color(rgb(colors().edge))
            .text_color(rgb(colors().muted))
            .into_any_element(),
        Kind::List { depth, marker } => {
            let marker = (*marker).min(block.text.len());
            row()
                .gap_0()
                .items_start()
                .w_full()
                .min_w_0()
                .pl(px(*depth as f32 * LIST_STEP))
                .child(
                    div()
                        .flex_shrink_0()
                        .min_w(px(LIST_STEP))
                        .pr(px(6.))
                        .text_size(px(BODY_SIZE))
                        .line_height(px(BODY_LINE))
                        .text_color(rgb(colors().muted))
                        .child(SharedString::from(
                            block.text[..marker].trim_end().to_owned(),
                        )),
                )
                .child(prose_block(block, marker..block.text.len()).flex_1())
                .into_any_element()
        }
        Kind::Rule => div()
            .w_full()
            .h(px(1.))
            .my_1()
            .bg(rgb(colors().border))
            .into_any_element(),
        Kind::Paragraph if is_image(block) => image(block),
        Kind::Paragraph => prose_block(block, 0..block.text.len()).into_any_element(),
    }
}

fn prose_block(block: &Block, range: Range<usize>) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(px(BODY_SIZE))
        .line_height(px(BODY_LINE))
        .text_color(rgb(colors().text))
        .child(inline(block, range))
}

/// A paragraph that is only an image. Plait does not fetch it; say so.
fn is_image(block: &Block) -> bool {
    block.text.starts_with(IMAGE_PREFIX) && block.text.find(']').is_some()
}

fn image(block: &Block) -> AnyElement {
    let end = block.text.find(']').unwrap_or(block.text.len());
    let alt = block.text[IMAGE_PREFIX.len().min(end)..end].trim();
    let label = if alt.is_empty() {
        "Image".to_owned()
    } else {
        format!("Image: {alt}")
    };
    row()
        .gap_2()
        .w_full()
        .min_w_0()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors().border))
        .bg(rgb(colors().rail))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .text_color(rgb(colors().text))
                .child(label),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(12.))
                .text_color(rgb(colors().subtle))
                .child("Shown in Linear"),
        )
        .into_any_element()
}

fn code(block: &Block, key: &str) -> AnyElement {
    div()
        .id(SharedString::from(format!("issue-code-{key}")))
        .w_full()
        .min_w_0()
        .overflow_x_scroll()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors().border))
        .bg(rgb(colors().rail))
        .px_3()
        .py_2()
        .font_family(mono())
        .text_size(px(code_size()))
        .line_height(px(code_line_height()))
        .whitespace_nowrap()
        .text_color(rgb(colors().text))
        .child(inline(block, 0..block.text.len()))
        .into_any_element()
}

/// `range` of a block with its inline styles. A link keeps its words and is
/// followed by where it points, so nothing reads as clickable that is not.
fn inline(block: &Block, range: Range<usize>) -> StyledText {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut code = Vec::new();
    let mut cursor = range.start;
    let spans: Vec<_> = block
        .spans
        .iter()
        .filter(|(span, _)| span.end > range.start && span.start < range.end)
        .map(|(span, style)| (span.start.max(range.start)..span.end.min(range.end), *style))
        .collect();
    for (index, (span, style)) in spans.iter().enumerate() {
        if span.start > cursor {
            text.push_str(&block.text[cursor..span.start]);
        }
        let start = text.len();
        text.push_str(&block.text[span.clone()]);
        runs.push((start..text.len(), highlight(style)));
        if style.code {
            code.push((start..text.len(), mono()));
        }
        cursor = span.end;
        let link_ends = style.link.is_some()
            && spans
                .get(index + 1)
                .is_none_or(|(next, after)| after.link != style.link || next.start != span.end);
        if link_ends && let Some(url) = style.link.and_then(|i| block.links.get(usize::from(i))) {
            let start = text.len();
            text.push_str(&format!(" ({})", destination(url)));
            runs.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(rgb(colors().subtle).into()),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    if cursor < range.end {
        text.push_str(&block.text[cursor..range.end]);
    }
    StyledText::new(SharedString::from(text))
        .with_highlights(runs)
        .with_font_family_overrides(code)
}

/// Where a link points, short enough to read in a line.
fn destination(url: &str) -> String {
    let bare = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    match bare.char_indices().nth(60) {
        Some((at, _)) => format!("{}…", &bare[..at]),
        None => bare.to_owned(),
    }
}

fn highlight(style: &Style) -> HighlightStyle {
    HighlightStyle {
        color: style.token.map(|token| rgb(token_color(token)).into()),
        font_weight: style.strong.then_some(FontWeight::BOLD),
        font_style: style.emphasis.then_some(FontStyle::Italic),
        background_color: style.code.then(|| rgb(colors().panel).into()),
        strikethrough: style.strike.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: None,
        }),
        ..HighlightStyle::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{destination, is_image};
    use plait_client::markdown::Message;

    #[test]
    fn a_link_destination_is_short_and_bare() {
        assert_eq!(destination("https://linear.app/a/"), "linear.app/a");
        let long = format!("https://example.com/{}", "x".repeat(100));
        assert!(destination(&long).ends_with('…'));
        assert_eq!(destination(&long).chars().count(), 61);
    }

    #[test]
    fn an_image_paragraph_is_recognised_and_plain_text_is_not() {
        let image = Message::parse("![Failure](https://uploads.linear.app/a/b.png)");
        assert!(is_image(&image.blocks[0]), "{:?}", image.blocks[0].text);
        let text = Message::parse("Not an image [here](https://linear.app).");
        assert!(!is_image(&text.blocks[0]));
    }
}
