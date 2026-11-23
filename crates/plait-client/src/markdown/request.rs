//! Request projection keeps instructions literal but separates quoted context.

use super::{BLOCK_LIMIT, Block, Kind, Message, autolink};
use pulldown_cmark::{Event, Parser, Tag};

impl Message {
    /// Keep request syntax as written, except for a quote's outer `>` marks.
    pub fn request(source: &str) -> Self {
        if !source.contains('>') {
            return Self::plain(source);
        }
        let mut blocks = Vec::new();
        let mut end = 0;
        // The parser identifies quotes without mistaking fenced code for one.
        // Nested quotes remain literal inside their outer quote.
        for (event, range) in Parser::new(source).into_offset_iter() {
            if !matches!(event, Event::Start(Tag::BlockQuote(_))) || range.start < end {
                continue;
            }
            let (before_end, indent) = quote_indent(source, end, range.start);
            push(&mut blocks, Kind::Paragraph, &source[end..before_end]);
            let quote: String = source[range.clone()]
                .split_inclusive('\n')
                .map(|line| quote_line(line, indent))
                .collect();
            if quote.trim().is_empty() {
                // Keep empty `>` lines literal instead of dropping them.
                push(&mut blocks, Kind::Paragraph, &source[before_end..range.end]);
            } else {
                push(&mut blocks, Kind::Quote, &quote);
            }
            end = range.end;
            if blocks.len() >= BLOCK_LIMIT {
                return Self::plain(source);
            }
        }
        if end == 0 || !blocks.iter().any(|block| block.kind == Kind::Quote) {
            return Self::plain(source);
        }
        push(&mut blocks, Kind::Paragraph, &source[end..]);
        Self {
            source: source.into(),
            blocks,
        }
    }
}

fn push(blocks: &mut Vec<Block>, kind: Kind, text: &str) {
    let text = text.trim_matches(['\r', '\n']);
    if text.trim().is_empty() {
        return;
    }
    let mut block = Block {
        kind,
        text: text.into(),
        ..Block::default()
    };
    autolink(&mut block);
    blocks.push(block);
}

/// Leading spaces before an indented blockquote belong to its container rather
/// than the paragraph above, and allow continuation lines at that column.
fn quote_indent(source: &str, end: usize, start: usize) -> (usize, usize) {
    let line_start = source[end..start]
        .rfind('\n')
        .map_or(end, |offset| end + offset + 1);
    let prefix = &source[line_start..start];
    if prefix.bytes().all(|byte| byte == b' ') {
        (start - prefix.len(), prefix.len())
    } else {
        (start, prefix.chars().count())
    }
}

fn quote_line(line: &str, indent: usize) -> &str {
    let inset = line.len() - line.trim_start_matches(' ').len();
    if inset > indent + 3 {
        return line;
    }
    line[inset..]
        .strip_prefix('>')
        .map(|text| text.strip_prefix(' ').unwrap_or(text))
        .unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_are_context_not_request_emphasis() {
        let source = "Before\n\n> Keep **this** literal.\n> See https://example.com/é.\n\nYes, build it.\nKeep `code` and # headings as written.";
        let message = Message::request(source);
        assert_eq!(message.source, source);
        assert_eq!(message.blocks.len(), 3);
        assert_eq!(message.blocks[0].text, "Before");
        assert_eq!(message.blocks[1].kind, Kind::Quote);
        assert_eq!(
            message.blocks[1].text,
            "Keep **this** literal.\nSee https://example.com/é."
        );
        assert_eq!(message.blocks[1].links, ["https://example.com/é"]);
        assert_eq!(
            message.blocks[2].text,
            "Yes, build it.\nKeep `code` and # headings as written."
        );
        for block in &message.blocks {
            for (range, style) in &block.spans {
                assert!(block.text.is_char_boundary(range.start));
                assert!(block.text.is_char_boundary(range.end));
                assert_eq!(
                    block.link_at(range.start),
                    Some(block.links[usize::from(style.link.unwrap())].as_str())
                );
                assert!(!style.strong && !style.code);
            }
        }
    }

    #[test]
    fn ordinary_requests_and_fenced_quotes_stay_literal() {
        for source in [
            "",
            "\n",
            ">",
            ">\n\n>",
            "# Title\n\n**bold** and `code`\n",
            "a > b",
            "```sh\n> output.txt\n```\n",
            "~~~\n> not context\n~~~",
            "    > indented code\n",
        ] {
            let message = Message::request(source);
            assert_eq!(message.blocks.len(), 1);
            assert_eq!(message.blocks[0].kind, Kind::Paragraph);
            assert_eq!(message.blocks[0].text, source);
        }
    }

    #[test]
    fn multiline_nested_and_indented_quotes_keep_their_content() {
        for (source, text) in [
            ("> first\ncontinued\n\nNext", "first\ncontinued"),
            ("  > first\n  > second\n\nNext", "first\nsecond"),
            ("> outer\n> > inner\n\nNext", "outer\n> inner"),
            ("> first\r\n> second\r\n\r\nNext", "first\r\nsecond"),
        ] {
            let message = Message::request(source);
            assert_eq!(message.source, source);
            assert_eq!(message.blocks.len(), 2, "{source}");
            assert_eq!(message.blocks[0].kind, Kind::Quote);
            assert_eq!(message.blocks[0].text, text);
            assert_eq!(message.blocks[1].text, "Next");
        }
    }

    #[test]
    fn too_many_quotes_fall_back_to_one_literal_block() {
        let source = "> context\n\nInstruction\n\n".repeat(BLOCK_LIMIT);
        let message = Message::request(&source);
        assert_eq!(message.blocks.len(), 1);
        assert_eq!(message.blocks[0].text, source);
    }

    #[test]
    fn empty_quotes_and_indented_list_quotes_keep_their_text() {
        let trailing = Message::request("> real\n\n> ");
        assert_eq!(trailing.blocks.len(), 2);
        assert_eq!(trailing.blocks[0].kind, Kind::Quote);
        assert_eq!(trailing.blocks[0].text, "real");
        assert_eq!(trailing.blocks[1].kind, Kind::Paragraph);
        assert_eq!(trailing.blocks[1].text, "> ");

        let middle = Message::request("> real\n\n> \n\nAfter");
        assert_eq!(middle.blocks.len(), 3);
        assert_eq!(middle.blocks[0].kind, Kind::Quote);
        assert_eq!(middle.blocks[1].kind, Kind::Paragraph);
        assert_eq!(middle.blocks[1].text, "> ");
        assert_eq!(middle.blocks[2].text, "After");

        let only_empty = Message::request("Before\n\n> \n\nAfter");
        assert_eq!(only_empty.blocks.len(), 1);
        assert_eq!(only_empty.blocks[0].kind, Kind::Paragraph);
        assert_eq!(only_empty.blocks[0].text, "Before\n\n> \n\nAfter");

        let nested_in_list = Message::request("100. item\n     > quoted\n     > second");
        assert_eq!(nested_in_list.blocks.len(), 2);
        assert_eq!(nested_in_list.blocks[0].kind, Kind::Paragraph);
        assert_eq!(nested_in_list.blocks[0].text, "100. item");
        assert_eq!(nested_in_list.blocks[1].kind, Kind::Quote);
        assert_eq!(nested_in_list.blocks[1].text, "quoted\nsecond");

        // Lazy continuation at 4+ spaces from column 0 stays literal.
        let lazy = Message::request("> quoted\n    > lazy");
        assert_eq!(lazy.blocks.len(), 1);
        assert_eq!(lazy.blocks[0].kind, Kind::Quote);
        assert_eq!(lazy.blocks[0].text, "quoted\n    > lazy");
    }
}
