//! Smooth reveal of streaming text. Providers deliver text in uneven bursts,
//! several words at a time; drawing each burst as it lands looks choppy however
//! fast the display is. The view instead shows a prefix that grows every frame
//! toward what has arrived, so text flows at the display's own rate.
//!
//! The reveal trails the stream by about `CATCH_UP`: a larger backlog drains
//! faster, so a burst spreads over the next frames instead of popping in.

use crate::markdown::{Block, Kind};
use std::time::{Duration, Instant};

/// How long a backlog takes to shrink to about a third (one time constant).
const CATCH_UP: Duration = Duration::from_millis(120);
/// The slowest pace, in bytes per second, so a stalled stream's tail finishes.
const MIN_RATE: f64 = 240.;
/// A longer gap between frames counts as this long, so a pause does not dump text.
const MAX_STEP: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
pub struct Reveal {
    key: Option<String>,
    shown: usize,
    at: Option<Instant>,
}

impl Reveal {
    /// Bytes of a `total`-byte stream to show at `now`. `key` names the stream:
    /// text already there when a new key is first seen shows at once, and a
    /// stream that shrank (a new response) starts over from nothing.
    pub fn advance(&mut self, key: &str, total: usize, now: Instant) -> usize {
        if self.key.as_deref() != Some(key) {
            // Nothing to show yet: wait, so the first text under this key counts as already there.
            if total == 0 {
                return 0;
            }
            self.key = Some(key.to_owned());
            self.shown = total;
            self.at = Some(now);
            return total;
        }
        if total < self.shown {
            self.shown = 0;
        }
        let elapsed = self
            .at
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at))
            .min(MAX_STEP);
        self.at = Some(now);
        let backlog = total - self.shown;
        if backlog == 0 {
            return total;
        }
        let seconds = elapsed.as_secs_f64();
        let step = (backlog as f64 * seconds / CATCH_UP.as_secs_f64()).max(MIN_RATE * seconds);
        self.shown = (self.shown + (step.ceil() as usize).max(1)).min(total);
        self.shown
    }

    /// Whether more frames are needed to reach `total`.
    pub fn pending(&self, total: usize) -> bool {
        self.shown < total
    }
}

/// Bytes of text across `blocks`, the unit `Reveal` counts in.
pub fn total(blocks: &[Block]) -> usize {
    blocks.iter().map(|block| block.text.len()).sum()
}

/// The blocks fully inside the first `shown` bytes, and a cut copy of the
/// block the reveal is in. Tables, diffs, and rules appear whole.
pub fn visible(blocks: &[Block], shown: usize) -> (usize, Option<Block>) {
    let mut start = 0;
    for (index, block) in blocks.iter().enumerate() {
        let end = start + block.text.len();
        if shown < end {
            let cut = shown.saturating_sub(start);
            return (index, cut_block(block, cut));
        }
        start = end;
    }
    (blocks.len(), None)
}

fn cut_block(block: &Block, cut: usize) -> Option<Block> {
    let mut cut = match block.kind {
        Kind::Table(_) | Kind::Diff(_) | Kind::Rule => return Some(block.clone()),
        // The bullet is part of the text; never show half of one.
        Kind::List { marker, .. } => cut.max(marker),
        _ => cut,
    };
    cut = cut.min(block.text.len());
    while !block.text.is_char_boundary(cut) {
        cut -= 1;
    }
    if cut == 0 {
        return None;
    }
    let spans = block
        .spans
        .iter()
        .filter(|(range, _)| range.start < cut)
        .map(|(range, style)| (range.start..range.end.min(cut), *style))
        .collect();
    Some(Block {
        kind: block.kind.clone(),
        text: block.text[..cut].to_owned(),
        spans,
        links: block.links.clone(),
        indent: block.indent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Style;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn text_already_there_shows_at_once_and_new_text_flows_in() {
        let start = Instant::now();
        let mut reveal = Reveal::default();
        assert_eq!(reveal.advance("a", 500, start), 500);
        assert!(!reveal.pending(500));
        // A burst of 100 bytes spreads over frames instead of landing at once.
        let first = reveal.advance("a", 600, start + ms(8));
        assert!(first > 500 && first < 520, "{first}");
        let mut shown = first;
        let mut frames = 1;
        while reveal.pending(600) {
            frames += 1;
            let next = reveal.advance("a", 600, start + ms(8 * frames));
            assert!(next > shown, "every frame moves");
            shown = next;
        }
        // About 120 ms at 120 Hz: neither instant nor sluggish.
        assert!((8..=60).contains(&frames), "{frames}");
    }

    #[test]
    fn a_new_response_starts_over_and_another_thread_shows_whole() {
        let start = Instant::now();
        let mut reveal = Reveal::default();
        reveal.advance("a", 400, start);
        assert!(
            reveal.advance("a", 30, start + ms(8)) < 30,
            "a new response animates"
        );
        assert_eq!(reveal.advance("b", 900, start + ms(16)), 900);
        // Switching to a thread before its text has been read in: still shown whole.
        assert_eq!(reveal.advance("c", 0, start + ms(24)), 0);
        assert_eq!(reveal.advance("c", 700, start + ms(32)), 700);
    }

    #[test]
    fn a_long_pause_does_not_dump_the_backlog() {
        let start = Instant::now();
        let mut reveal = Reveal::default();
        reveal.advance("a", 10, start);
        let shown = reveal.advance("a", 10_010, start + Duration::from_secs(5));
        assert!(shown < 10_010 / 2, "{shown}");
    }

    #[test]
    fn blocks_are_cut_on_character_boundaries_with_spans_and_bullets_kept_whole() {
        let bold = Style {
            strong: true,
            ..Style::default()
        };
        let blocks = vec![
            Block {
                text: "héllo".into(),
                spans: vec![(0..6, bold)],
                ..Block::default()
            },
            Block {
                kind: Kind::List {
                    depth: 0,
                    // "• " in bytes.
                    marker: 4,
                },
                text: "• item".into(),
                ..Block::default()
            },
        ];
        assert_eq!(total(&blocks), 6 + 8);
        // Byte 2 is inside "é"; the cut steps back to a boundary.
        let (whole, cut) = visible(&blocks, 2);
        assert_eq!(whole, 0);
        let cut = cut.unwrap();
        assert_eq!(cut.text, "h");
        assert_eq!(cut.spans, vec![(0..1, bold)]);
        let (whole, cut) = visible(&blocks, 7);
        assert_eq!((whole, cut.unwrap().text.as_str()), (1, "• "));
        assert_eq!(visible(&blocks, 14).0, 2);
        assert!(visible(&blocks, 0).1.is_none());
    }
}
