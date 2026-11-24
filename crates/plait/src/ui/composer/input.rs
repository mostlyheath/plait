//! The draft's text area: wrapped layout, auto-growing height, and painting.

use super::Composer;
use crate::ui::theme::colors;
use gpui::*;
use std::{collections::HashMap, ops::Range, rc::Rc};

pub(super) const LINE_HEIGHT: f32 = 21.;
/// Shown in an empty draft. It names the two triggers, so no line of key
/// hints has to sit under the draft while nothing is running.
pub(super) const PLACEHOLDER: &str = "Write a message · @ files · / commands";
/// The draft grows up to 200 points high (including 18 points of vertical
/// padding) before scrolling, while staying within a third of short windows.
const MAX_DRAFT_HEIGHT: f32 = 200.;
const DRAFT_VERTICAL_PADDING: f32 = 18.;
const MAX_ROW_LIMIT: usize = ((MAX_DRAFT_HEIGHT - DRAFT_VERTICAL_PADDING) / LINE_HEIGHT) as usize;
/// The input never scrolls before it shows this many lines, however short the window.
const MIN_ROW_LIMIT: usize = 4;

/// Lines the input may show before it scrolls: capped at 200 points total height
/// (and about a third of shorter windows), so a long draft cannot push the
/// conversation off screen.
pub(super) fn row_limit(viewport_height: Pixels) -> usize {
    let rows = (viewport_height / 3. / px(LINE_HEIGHT)).floor();
    (rows.max(0.) as usize).clamp(MIN_ROW_LIMIT, MAX_ROW_LIMIT)
}

/// One line when empty, growing with the draft up to `limit` lines.
pub(super) fn input_height(rows: usize, limit: usize) -> Pixels {
    px(rows.clamp(1, limit.max(1)) as f32 * LINE_HEIGHT)
}

/// The scroll offset that keeps a cursor line visible, moving as little as possible.
pub(super) fn reveal(scroll: Pixels, cursor_top: Pixels, height: Pixels) -> Pixels {
    let bottom = cursor_top + px(LINE_HEIGHT);
    if cursor_top < scroll {
        cursor_top
    } else if bottom > scroll + height {
        bottom - height
    } else {
        scroll
    }
}

pub(super) fn clamp_scroll(scroll: Pixels, rows: usize, height: Pixels) -> Pixels {
    let overflow = (px(rows as f32 * LINE_HEIGHT) - height).max(px(0.));
    scroll.clamp(px(0.), overflow)
}

/// Top and length of the position marker for a draft taller than its box, or
/// `None` when everything fits. It shows that text continues out of view.
pub(super) fn scroll_marker(
    scroll: Pixels,
    rows: usize,
    height: Pixels,
) -> Option<(Pixels, Pixels)> {
    let content = px(rows as f32 * LINE_HEIGHT);
    if content <= height || height <= px(0.) {
        return None;
    }
    let length = (height * (height / content)).max(px(LINE_HEIGHT).min(height));
    let travelled = (scroll / (content - height)).clamp(0., 1.);
    Some(((height - length) * travelled, length))
}

/// One wrapped line on screen and the draft bytes it shows.
pub(super) struct VisualRow {
    pub range: Range<usize>,
    pub line: Rc<ShapedLine>,
}

/// What a shaped layout depends on besides the text.
#[derive(Clone, PartialEq)]
struct Shaping {
    width: Pixels,
    font_size: Pixels,
    color: Hsla,
}

type LineRows = Rc<[(Range<usize>, Rc<ShapedLine>)]>;

/// Wrapped rows for the current draft. Kept between frames, so moving the
/// cursor shapes nothing and an edit reshapes only the lines it changed. That
/// keeps a large pasted draft responsive.
#[derive(Default)]
pub(super) struct Layout {
    pub rows: Vec<VisualRow>,
    /// The draft revision `rows` was built from.
    revision: Option<u64>,
    shaping: Option<Shaping>,
    /// Rows of each logical line in the last layout, by the line's text.
    lines: HashMap<String, LineRows>,
    pub failure: Option<String>,
}

impl Layout {
    /// Forget the rows, for a draft replaced without a revision change.
    pub fn invalidate(&mut self) {
        self.revision = None;
    }

    /// Index of the row holding `offset`.
    pub fn row_at(&self, offset: usize) -> usize {
        self.rows
            .partition_point(|row| row.range.start <= offset)
            .saturating_sub(1)
    }
}

fn shape_line_rows(
    text: &str,
    shaping: &Shaping,
    font: &Font,
    window: &mut Window,
) -> Result<LineRows, String> {
    let run = |len| TextRun {
        len,
        font: font.clone(),
        color: shaping.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let content: SharedString = text.to_owned().into();
    let wrapped = window
        .text_system()
        .shape_text(
            content,
            shaping.font_size,
            &[run(text.len())],
            Some(shaping.width),
            None,
        )
        .map_err(|error| format!("Cannot display draft: {error}"))?;
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in wrapped {
        let mut start = 0;
        let ends = line
            .wrap_boundaries()
            .iter()
            .map(|boundary| line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index)
            .chain([line.len()]);
        for end in ends {
            // Each visual row is shaped alone so positions map to one line of glyphs.
            let piece: SharedString = line.text[start..end].to_owned().into();
            let shaped = window.text_system().shape_line(
                piece,
                shaping.font_size,
                &[run(end - start)],
                None,
            );
            rows.push((offset + start..offset + end, Rc::new(shaped)));
            start = end;
        }
        offset += line.len() + 1;
    }
    Ok(rows.into())
}

impl Composer {
    /// Bring the wrapped rows up to date with the draft at `width`.
    pub(super) fn shape(&mut self, width: Pixels, style: &TextStyle, window: &mut Window) {
        let shaping = Shaping {
            width: width.max(px(1.)),
            font_size: style.font_size.to_pixels(window.rem_size()),
            color: style.color,
        };
        let layout = &mut self.layout;
        let same_shaping = layout.shaping.as_ref() == Some(&shaping);
        if same_shaping && layout.revision == Some(self.draft.revision) {
            return;
        }
        let mut previous = std::mem::take(&mut layout.lines);
        if !same_shaping {
            previous.clear();
        }
        let font = style.font();
        layout.rows.clear();
        layout.failure = None;
        let mut offset = 0;
        for text in self.draft.value.text.split('\n') {
            let known = layout.lines.get(text).cloned().or_else(|| {
                let (text, rows) = previous.remove_entry(text)?;
                layout.lines.insert(text, rows.clone());
                Some(rows)
            });
            let rows = match known {
                Some(rows) => rows,
                None => match shape_line_rows(text, &shaping, &font, window) {
                    Ok(rows) => {
                        layout.lines.insert(text.to_owned(), rows.clone());
                        rows
                    }
                    Err(error) => {
                        layout.failure = Some(error);
                        Rc::from([])
                    }
                },
            };
            layout
                .rows
                .extend(rows.iter().map(|(range, line)| VisualRow {
                    range: offset + range.start..offset + range.end,
                    line: line.clone(),
                }));
            offset += text.len() + 1;
        }
        layout.shaping = Some(shaping);
        layout.revision = Some(self.draft.revision);
        self.layout_revision = self.draft.revision;
    }
}

pub(super) struct InputElement(pub Entity<Composer>);

impl Element for InputElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let text_style = window.text_style();
        let limit = row_limit(window.viewport_size().height);
        let input = self.0.clone();
        // The height follows the wrapped draft, which is known only at the final width.
        let layout = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            input.update(cx, |input, _| {
                if let Some(width) = width {
                    input.shape(width, &text_style, window);
                }
                size(
                    width.unwrap_or_default(),
                    input_height(input.layout.rows.len(), limit),
                )
            })
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = window.text_style();
        self.0.update(cx, |input, _| {
            input.bounds = Some(bounds);
            input.shape(bounds.size.width, &style, window);
            if std::mem::take(&mut input.reveal_cursor) {
                input.scroll = reveal(input.scroll, input.cursor_position().y, bounds.size.height);
            }
            input.scroll = clamp_scroll(input.scroll, input.layout.rows.len(), bounds.size.height);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.0.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.0.clone()), cx);
        self.0.update(cx, |input, cx| {
            if std::mem::take(&mut input.input_changed) {
                window.notify_text_input_changed(&input.focus);
            }
            let mut failure = input.layout.failure.clone();
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                if input.draft.value.text.is_empty()
                    && let Err(error) = paint_placeholder(bounds, window, cx)
                {
                    failure = Some(error);
                }
                if !input.draft.selection().is_empty() {
                    for rect in input.rectangles(input.draft.selection()) {
                        window.paint_quad(fill(rect, rgba(colors().text_selection)));
                    }
                }
                // Only the rows inside the box are painted, whatever the draft's length.
                let first = (input.scroll / px(LINE_HEIGHT)).floor().max(0.) as usize;
                let last = ((input.scroll + bounds.size.height) / px(LINE_HEIGHT)).ceil() as usize;
                let visible = first.min(input.layout.rows.len())..last.min(input.layout.rows.len());
                for (index, row) in input.layout.rows[visible.clone()].iter().enumerate() {
                    let top = px((visible.start + index) as f32 * LINE_HEIGHT) - input.scroll;
                    if let Err(error) = row.line.paint(
                        bounds.origin + point(px(0.), top),
                        px(LINE_HEIGHT),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    ) {
                        failure = Some(format!("Cannot display draft: {error}"));
                    }
                }
                if let Some(marked) = input.draft.marked.clone() {
                    for rect in input.rectangles(marked) {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(rect.left(), rect.bottom() - px(1.)),
                                size(rect.size.width, px(1.)),
                            ),
                            rgb(colors().text),
                        ));
                    }
                }
                if input.enabled
                    && input.focus.is_focused(window)
                    && input.draft.selection().is_empty()
                {
                    for rect in input.rectangles(input.draft.selection()) {
                        window.paint_quad(fill(rect, rgb(colors().blue)));
                    }
                }
            });
            // Drawn in the box's right padding, clear of the wrapped text.
            if let Some((top, length)) =
                scroll_marker(input.scroll, input.layout.rows.len(), bounds.size.height)
            {
                let origin = point(bounds.right() + px(5.), bounds.top() + top);
                window.paint_quad(
                    fill(
                        Bounds::new(origin, size(px(3.), length)),
                        rgb(colors().edge),
                    )
                    .corner_radii(px(1.5)),
                );
            }
            if input.paint_error != failure {
                input.paint_error = failure;
                cx.notify();
            }
        });
    }
}

fn paint_placeholder(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    let style = window.text_style();
    let text: SharedString = PLACEHOLDER.into();
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color: rgb(colors().subtle).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(
            text,
            style.font_size.to_pixels(window.rem_size()),
            &[run],
            None,
        )
        .paint(
            bounds.origin,
            px(LINE_HEIGHT),
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .map_err(|error| format!("Cannot display draft: {error}"))
}

#[cfg(test)]
mod tests {
    // Not a glob: GPUI exports its own `test` attribute.
    use super::{
        DRAFT_VERTICAL_PADDING, LINE_HEIGHT, MAX_DRAFT_HEIGHT, MAX_ROW_LIMIT, MIN_ROW_LIMIT,
        clamp_scroll, input_height, reveal, row_limit, scroll_marker,
    };
    use gpui::px;

    #[test]
    fn the_input_is_one_line_when_empty_and_grows_to_the_limit() {
        assert_eq!(input_height(0, 9), px(LINE_HEIGHT));
        assert_eq!(input_height(1, 9), px(LINE_HEIGHT));
        assert_eq!(input_height(5, 9), px(5. * LINE_HEIGHT));
        assert_eq!(input_height(9, 9), px(9. * LINE_HEIGHT));
        assert_eq!(input_height(400, 9), px(9. * LINE_HEIGHT));
        assert_eq!(input_height(3, 0), px(LINE_HEIGHT));
    }

    #[test]
    fn the_limit_caps_draft_height_at_200_with_a_floor() {
        assert_eq!(row_limit(px(400.)), 6);
        assert_eq!(row_limit(px(600.)), MAX_ROW_LIMIT);
        assert_eq!(row_limit(px(860.)), MAX_ROW_LIMIT);
        assert_eq!(row_limit(px(1440.)), MAX_ROW_LIMIT);
        assert_eq!(row_limit(px(120.)), MIN_ROW_LIMIT);
        assert_eq!(row_limit(px(0.)), MIN_ROW_LIMIT);
        for height in [400., 600., 860., 1440.] {
            let tallest = input_height(usize::MAX, row_limit(px(height)));
            assert!(tallest + px(DRAFT_VERTICAL_PADDING) <= px(MAX_DRAFT_HEIGHT));
            assert!(tallest <= px(height / 3.));
        }
    }

    #[test]
    fn the_cursor_line_is_revealed_with_the_smallest_scroll() {
        let height = px(4. * LINE_HEIGHT);
        let line = |row: f32| px(row * LINE_HEIGHT);
        assert_eq!(reveal(px(0.), line(2.), height), px(0.));
        assert_eq!(reveal(px(0.), line(3.), height), px(0.));
        assert_eq!(reveal(px(0.), line(4.), height), line(1.));
        assert_eq!(reveal(line(5.), line(2.), height), line(2.));
        assert_eq!(reveal(line(5.), line(6.), height), line(5.));
        // A one-line box always scrolls to the cursor's line.
        assert_eq!(reveal(px(0.), line(7.), line(1.)), line(7.));
    }

    #[test]
    fn the_marker_appears_only_on_overflow_and_spans_the_box() {
        let height = px(4. * LINE_HEIGHT);
        assert_eq!(scroll_marker(px(0.), 4, height), None);
        assert_eq!(scroll_marker(px(0.), 0, height), None);
        assert_eq!(scroll_marker(px(0.), 9, px(0.)), None);
        // Eight rows in a four-row box: the marker is half the box.
        let half = px(2. * LINE_HEIGHT);
        assert_eq!(scroll_marker(px(0.), 8, height), Some((px(0.), half)));
        assert_eq!(scroll_marker(height, 8, height), Some((half, half)));
        assert_eq!(
            scroll_marker(height / 2., 8, height),
            Some((half / 2., half))
        );
        // A very long draft keeps a marker large enough to see, inside the box.
        let (top, length) = scroll_marker(px(1e6), 5000, height).unwrap();
        assert_eq!(length, px(LINE_HEIGHT));
        assert_eq!(top + length, height);
    }

    #[test]
    fn scroll_stays_within_the_overflowing_rows() {
        let height = px(4. * LINE_HEIGHT);
        assert_eq!(clamp_scroll(px(50.), 3, height), px(0.));
        assert_eq!(clamp_scroll(px(50.), 4, height), px(0.));
        assert_eq!(clamp_scroll(px(500.), 10, height), px(6. * LINE_HEIGHT));
        assert_eq!(clamp_scroll(px(-5.), 10, height), px(0.));
        assert_eq!(clamp_scroll(px(30.), 10, height), px(30.));
    }
}
