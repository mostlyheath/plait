//! Checkout documents use the conversation projection without its message-length fallback.
use super::{Block, Event, Kind, Options, Parser, Projection, Tag, TagEnd};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::Path,
};

use crate::doc_links::{Anchors, LinkTarget, resolve_link};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub rows: Vec<Row>,
    pub headings: Vec<usize>,
    pub anchors: HashMap<String, usize>,
    pub images: Vec<(usize, String)>,
    pub metadata: Option<String>,
    pub metadata_rows: Vec<Range<usize>>,
    pub text: String,
    pub ranges: Vec<Range<usize>>,
    // (block, projected byte, original source byte) at Markdown line boundaries.
    source_positions: Vec<(usize, usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub block: usize,
    pub bytes: Range<usize>,
    pub table_row: Option<usize>,
}

pub fn markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

pub fn target(href: &str, path: &Path, root: &Path) -> LinkTarget {
    if href.starts_with('/') && !href.starts_with("//") {
        let (file, anchor) = href
            .split_once('#')
            .map_or((href, None), |(f, a)| (f, Some(a.to_owned())));
        if let Ok(local) = Path::new(file).strip_prefix(root)
            && local
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return LinkTarget::Document {
                path: local.into(),
                anchor,
            };
        }
    }
    resolve_link(href, path)
}

impl Document {
    pub fn parse(source: &str) -> Self {
        let mut doc = Self::default();
        let mut projection = Projection {
            document: true,
            ..Projection::default()
        };
        let (metadata, body) = frontmatter(source);
        doc.metadata = metadata.map(str::to_owned);
        if let Some(metadata) = metadata {
            let mut offset = 0;
            for line in metadata.split_inclusive('\n') {
                let start = offset;
                offset += line.len();
                let end = start + line.trim_end_matches(['\r', '\n']).len();
                fragments(&metadata[start..end], |bytes| {
                    doc.metadata_rows
                        .push(start + bytes.start..start + bytes.end)
                });
            }
        }
        let options = Options::ENABLE_TASKLISTS
            | Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_FOOTNOTES;
        let body_offset = source.len() - body.len();
        for (event, range) in Parser::new_ext(body, options).into_offset_iter() {
            let block = projection.blocks.len();
            let byte = projection.current.text.len();
            if matches!(
                event,
                Event::Text(_)
                    | Event::Code(_)
                    | Event::Html(_)
                    | Event::InlineHtml(_)
                    | Event::Rule
                    | Event::TaskListMarker(_)
            ) || (matches!(event, Event::End(TagEnd::Image | TagEnd::Link))
                && projection
                    .links
                    .last()
                    .is_some_and(|(_, start)| *start == byte))
            {
                doc.source_positions
                    .push((block, byte, body_offset + range.start));
                if let Event::Code(ref code) = event {
                    let raw = &body[range.clone()];
                    inline_code_lines(raw, code, |projected, source| {
                        doc.source_positions.push((
                            block,
                            byte + projected,
                            body_offset + range.start + source,
                        ));
                    });
                }
                if let Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) = &event {
                    for ((i, _), (source, _)) in text
                        .match_indices('\n')
                        .zip(body[range.clone()].match_indices('\n'))
                    {
                        if i + 1 < text.len() {
                            doc.source_positions.push((
                                block,
                                byte + i + 1,
                                body_offset + range.start + source + 1,
                            ));
                        }
                    }
                }
            }
            match event {
                Event::FootnoteReference(label) => projection.text(&format!("[{label}]"), false),
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    projection.flush();
                    projection.text(&format!("[{label}]"), false);
                }
                Event::End(TagEnd::FootnoteDefinition) => projection.flush(),
                event => projection.event(event),
            }
        }
        projection.flush();
        doc.blocks = projection.blocks;
        doc.images = projection.images;
        let mut anchors = Anchors::new();
        for (i, block) in doc.blocks.iter().enumerate() {
            if matches!(block.kind, Kind::Heading(_)) {
                doc.headings.push(i);
                doc.anchors.insert(anchors.add(&block.text), i);
            }
            if let Kind::Table(table) = &block.kind {
                for (n, cells) in table.rows.iter().enumerate() {
                    let bytes =
                        cells.first().map_or(0, |c| c.start)..cells.last().map_or(0, |c| c.end);
                    doc.rows.push(Row {
                        block: i,
                        bytes,
                        table_row: Some(n),
                    });
                }
            } else if matches!(block.kind, Kind::Code(_)) {
                let mut start = 0;
                for line in block.text.split('\n') {
                    let end = start + line.len();
                    doc.rows.push(Row {
                        block: i,
                        bytes: start..end,
                        table_row: None,
                    });
                    start = end + 1;
                }
            } else {
                fragments(&block.text, |bytes| {
                    doc.rows.push(Row {
                        block: i,
                        bytes,
                        table_row: None,
                    })
                });
            }
            let start = doc.text.len();
            doc.text.push_str(&block.text);
            doc.ranges.push(start..doc.text.len());
            doc.text.push('\n');
        }
        doc
    }

    pub fn find(&self, query: &str, metadata_open: bool) -> Vec<Range<usize>> {
        let mut matches = crate::file_view::find(&self.text, query);
        if metadata_open
            && matches.len() < crate::file_view::FIND_LIMIT
            && let Some(metadata) = &self.metadata
        {
            let offset = self.text.len();
            matches.extend(
                crate::conversation_find::find(
                    metadata,
                    query,
                    crate::file_view::FIND_LIMIT - matches.len(),
                )
                .into_iter()
                .map(|r| offset + r.start..offset + r.end),
            );
        }
        matches
    }
    /// Check each destination once. File-only links need just a host stamp.
    pub fn check_links(
        &self,
        path: &Path,
        root: &Path,
        mut read: impl FnMut(&Path, bool) -> Result<Option<HashSet<String>>, String>,
    ) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut files = HashMap::new();
        let mut issues = Vec::new();
        for href in self
            .blocks
            .iter()
            .flat_map(|b| &b.links)
            .filter(|h| seen.insert(*h))
        {
            let issue = match target(href, path, root) {
                LinkTarget::Outside => Some("outside this project".to_owned()),
                LinkTarget::Anchor(id) if self.heading(&id).is_none() => {
                    Some("missing heading".into())
                }
                LinkTarget::Document { path, anchor } => {
                    let headings = anchor.is_some();
                    match files
                        .entry((path.clone(), headings))
                        .or_insert_with(|| read(&path, headings))
                    {
                        Err(error) => Some(error.clone()),
                        Ok(ids)
                            if anchor.as_ref().is_some_and(|id| {
                                !ids.as_ref().is_some_and(|ids| ids.contains(id))
                            }) =>
                        {
                            Some("missing heading".into())
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(issue) = issue {
                issues.push(format!("{href} — {issue}"));
            }
        }
        issues
    }

    pub fn source_byte(&self, block: usize, byte: usize) -> usize {
        let end = self
            .source_positions
            .partition_point(|&(b, i, _)| (b, i) <= (block, byte));
        self.source_positions
            .get(end.saturating_sub(1))
            .filter(|&&(b, _, _)| b == block)
            .or_else(|| self.source_positions.get(end))
            .map_or(0, |&(_, _, source)| source)
    }

    pub fn row_at(&self, block: usize, byte: usize) -> usize {
        self.rows
            .partition_point(|row| (row.block, row.bytes.start) <= (block, byte))
            .saturating_sub(1)
    }
    pub fn heading(&self, id: &str) -> Option<usize> {
        self.anchors.get(id).copied()
    }
}

// Code spans fold line endings to spaces and can lose one padding space at each end.
fn inline_code_lines(raw: &str, code: &str, mut add: impl FnMut(usize, usize)) {
    if !raw.contains('\n') {
        return;
    }
    let ticks = raw.bytes().take_while(|&b| b == b'`').count();
    let inner = &raw[ticks..raw.len() - ticks];
    let first_line = inner.split(['\r', '\n']).next().unwrap_or_default();
    let padded = if first_line.bytes().any(|b| b != b' ') {
        // A closing delimiter on its own container line can also trim padding.
        first_line.starts_with(' ') && !code.starts_with(first_line)
    } else {
        inner.starts_with([' ', '\r', '\n'])
            && inner.ends_with([' ', '\r', '\n'])
            && code.bytes().any(|b| b != b' ')
    };
    let mut projected = 0;
    let mut source = ticks;
    for (index, line) in inner.split_inclusive(['\r', '\n']).enumerate() {
        let Some(mut content) = line.strip_suffix(['\r', '\n']) else {
            break;
        };
        if index == 0 && padded {
            content = content.strip_prefix(' ').unwrap_or(content);
        }
        // Continuation lines may start with a list or blockquote prefix which
        // the parser omitted. Match the remaining suffix against its projection.
        while index > 0 && !code.as_bytes()[projected..].starts_with(content.as_bytes()) {
            let Some(c) = content.chars().next() else {
                break;
            };
            content = &content[c.len_utf8()..];
        }
        let folded_space = !(index == 0 && padded && inner.starts_with(['\r', '\n']));
        projected = (projected + content.len() + usize::from(folded_space)).min(code.len());
        source += line.len();
        add(projected, source);
    }
}

/// Byte ranges within one fenced line, measured independently by the renderer.
pub fn code_fragments(text: &str, line: Range<usize>) -> impl Iterator<Item = Range<usize>> + '_ {
    std::iter::successors(
        Some(line.start..text.floor_char_boundary((line.start + 4096).min(line.end))),
        move |previous| {
            let start = previous.end;
            (start < line.end)
                .then(|| start..text.floor_char_boundary((start + 4096).min(line.end)))
        },
    )
}

/// Bound the amount of native text shaping in a single visible item.
fn fragments(text: &str, mut add: impl FnMut(Range<usize>)) {
    let mut start = 0;
    loop {
        let limit = text.floor_char_boundary((start + 4096).min(text.len()));
        let end = if limit == text.len() {
            limit
        } else {
            text[start..limit]
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map_or(limit, |(i, c)| start + i + c.len_utf8())
        };
        add(start..end);
        if end == text.len() {
            return;
        }
        start = end;
    }
}

fn frontmatter(source: &str) -> (Option<&str>, &str) {
    let mut lines = source.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    let marker = first.trim_end_matches(['\r', '\n']);
    if matches!(marker, "---" | "+++") {
        let mut offset = first.len();
        for line in lines {
            if line.trim_end_matches(['\r', '\n']) == marker {
                return (
                    Some(&source[first.len()..offset]),
                    &source[offset + line.len()..],
                );
            }
            offset += line.len();
        }
    }
    (None, source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_extensions_keep_literal_content() {
        for marker in ["---", "+++"] {
            let source = format!(
                "{marker}\r\nname: demo\r\n{marker}\r\n# Hello\n\n- [x] ready\n\nText[^a]\n\n[^a]: note\n\n<!--literal-->\n\n<b>raw</b>"
            );
            let doc = Document::parse(&source);
            assert_eq!(doc.metadata.as_deref(), Some("name: demo\r\n"));
            assert_eq!(doc.heading("hello"), Some(0));
            assert!(doc.find("name: demo", false).is_empty());
            assert_eq!(
                doc.find("name: demo", true),
                vec![doc.text.len()..doc.text.len() + 10]
            );
            for text in [
                "☑ ready",
                "Text[a]",
                "[a]",
                "note",
                "<!--literal-->",
                "<b>raw</b>",
            ] {
                assert!(doc.text.contains(text), "{text}");
            }
        }
    }
    #[test]
    fn check_links_reads_each_local_target_once_and_ignores_the_network() {
        let doc = Document::parse(
            "# Top\n\n[a](#top) [b](other.md#yes) [c](other.md#no) [d](https://example.org) [e](missing.md) [f](../outside) [mail](mailto:a@example.org)",
        );
        let mut calls = Vec::new();
        let issues = doc.check_links(Path::new("readme.md"), Path::new("/repo"), |p, headings| {
            calls.push((p.to_owned(), headings));
            if p == Path::new("other.md") {
                Ok(Some(HashSet::from(["yes".into()])))
            } else {
                Err("missing file".into())
            }
        });
        assert_eq!(calls.len(), 2);
        assert_eq!(
            issues,
            [
                "other.md#no — missing heading",
                "missing.md — missing file",
                "../outside — outside this project"
            ]
        );
    }

    #[test]
    fn code_rows_preserve_long_lines_and_source_positions() {
        let line = "界".repeat(5000);
        let source = format!("---\nk: v\n---\n# Title\n\n```\n{line}\nnext\n```");
        let doc = Document::parse(&source);
        let rows = &doc.rows[1..];
        assert_eq!(rows.len(), 2);
        let block = &doc.blocks[1].text;
        let pieces: Vec<_> = code_fragments(block, rows[0].bytes.clone())
            .map(|range| &block[range])
            .collect();
        assert!(pieces.iter().all(|piece| piece.len() <= 4096));
        assert_eq!(pieces.concat(), line);
        assert_eq!(doc.row_at(1, 4097), 1);
        assert_eq!(&block[rows[1].bytes.clone()], "next");
        assert_eq!(
            doc.source_byte(1, block.find("next").unwrap()),
            source.find("next").unwrap()
        );
        assert_eq!(source[doc.source_byte(0, 0)..].chars().next(), Some('T'));
    }

    #[test]
    fn multiline_inline_code_maps_find_to_the_second_source_line() {
        let source = "`first\nsecond needle`";
        let doc = Document::parse(source);
        let hit = doc.find("needle", false).pop().unwrap();
        assert_eq!(
            doc.source_byte(0, hit.start),
            source.find("second").unwrap()
        );
    }

    #[test]
    fn multiline_inline_html_maps_find_to_the_second_source_line() {
        let source = "Intro <!-- first\nneedle -->";
        let doc = Document::parse(source);
        let hit = doc.find("needle", false).pop().unwrap();
        assert_eq!(
            doc.source_byte(0, hit.start),
            source.find("needle").unwrap()
        );
    }

    #[test]
    fn empty_labels_map_find_matches_to_their_source_line() {
        for label in ["![](diagram.png)", "[](diagram.png)"] {
            let source = format!("# Title\n\nintro\n\n{label}");
            let doc = Document::parse(&source);
            let hit = doc.find("diagram.png", false).pop().unwrap();
            let block = crate::file_view::line_of(&doc.ranges, hit.start);
            assert_eq!(
                doc.source_byte(block, hit.start - doc.ranges[block].start),
                source.find(label).unwrap()
            );
        }
    }

    #[test]
    fn parses_a_megabyte() {
        let paragraph =
            "## Heading\n\nA paragraph with *emphasis*, `code` and [links](other.md).\n\n";
        let source = paragraph.repeat(1_048_576 / paragraph.len() + 1);
        let best = (0..3)
            .map(|_| {
                let start = std::time::Instant::now();
                assert!(Document::parse(&source).blocks.len() > 10000);
                start.elapsed()
            })
            .min()
            .unwrap();
        assert!(best < std::time::Duration::from_secs(5), "{best:?}");
    }
}
