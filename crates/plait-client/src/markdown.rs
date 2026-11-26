//! Native conversation projection. No HTML execution or image loading, and a
//! link is only data: the view opens one on an explicit click.
//! Parse on the background executor and retain unchanged rows by identity.

use crate::{
    diff::Change,
    highlight::{self, Token},
    host::{MessageRole, Snapshot},
    tool_view::{Runs, ToolView},
};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::{collections::HashMap, ops::Range, sync::Arc};

pub mod document;
mod request;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Paragraph,
    Heading(u8),
    /// Fenced or tool code with its language label; spans carry syntax tokens.
    Code(String),
    Quote,
    /// A list item. The first `marker` bytes of the text are its bullet or number.
    List {
        depth: usize,
        marker: usize,
    },
    Rule,
    /// Cells are ranges of the text, which holds one row per line with tabs between cells.
    Table(Table),
    /// An edit preview. Lines are ranges of the text, each after a two-byte `+ `, `- `, or
    /// two-space marker.
    Diff(Vec<DiffLine>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Column {
    pub align: Align,
    /// Characters in the widest cell, for a column width without measuring text.
    pub width: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<Column>,
    /// The first row is the header.
    pub rows: Vec<Vec<Range<usize>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub change: Change,
    pub range: Range<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub strike: bool,
    /// Index into [`Block::links`]. Only http and https destinations become links.
    pub link: Option<u16>,
    pub token: Option<Token>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub text: String,
    /// Sorted, non-overlapping, and on character boundaries.
    pub spans: Vec<(Range<usize>, Style)>,
    pub links: Vec<String>,
    /// Enclosing list levels, for a paragraph or code block inside a list item.
    pub indent: usize,
}

impl Block {
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        let kind = match &self.kind {
            Kind::Code(s) => s.capacity(),
            Kind::Diff(lines) => lines.capacity() * size_of::<DiffLine>(),
            Kind::Table(table) => {
                table.columns.capacity() * size_of::<Column>()
                    + table.rows.capacity() * size_of::<Vec<Range<usize>>>()
                    + table
                        .rows
                        .iter()
                        .map(|row| row.capacity() * size_of::<Range<usize>>())
                        .sum::<usize>()
            }
            _ => 0,
        };
        self.text.capacity()
            + self.spans.capacity() * size_of::<(Range<usize>, Style)>()
            + self.links.capacity() * size_of::<String>()
            + self.links.iter().map(String::capacity).sum::<usize>()
            + kind
    }
    /// A code block with syntax tokens for `language`, or plain when unknown.
    pub fn code(language: &str, text: impl Into<String>) -> Self {
        let text = text.into();
        let spans = highlight::highlight(language, &text)
            .into_iter()
            .map(|(range, token)| {
                let style = Style {
                    token: Some(token),
                    ..Style::default()
                };
                (range, style)
            })
            .collect();
        Self {
            kind: Kind::Code(language.into()),
            text,
            spans,
            ..Self::default()
        }
    }

    /// The destination of the link covering `offset`, if any.
    pub fn link_at(&self, offset: usize) -> Option<&str> {
        let (_, style) = self
            .spans
            .iter()
            .find(|(range, _)| range.contains(&offset))?;
        self.links.get(usize::from(style.link?)).map(String::as_str)
    }
}

/// Longest destination that may become a link.
const URL_LIMIT: usize = 2048;

/// The destination itself when it is a plain http or https URL.
pub fn safe_url(destination: &str) -> Option<&str> {
    let rest = ["http://", "https://"].iter().find_map(|scheme| {
        destination
            .get(..scheme.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
            .map(|_| &destination[scheme.len()..])
    })?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let plain = !destination
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control());
    (plain && !host.is_empty() && destination.len() <= URL_LIMIT).then_some(destination)
}

#[derive(Debug)]
pub struct Message {
    pub source: String,
    pub blocks: Vec<Block>,
}

/// A pathological message remains readable without thousands of native elements.
const BLOCK_LIMIT: usize = 512;
/// Larger tables are shown as tab-separated text.
const CELL_LIMIT: usize = 1500;

impl Message {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.source.capacity()
            + self.blocks.capacity() * std::mem::size_of::<Block>()
            + self.blocks.iter().map(Block::retained_bytes).sum::<usize>()
    }
    pub fn parse(source: &str) -> Self {
        let mut projection = Projection::default();
        let options =
            Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
        for event in Parser::new_ext(source, options) {
            projection.event(event);
            if projection.blocks.len() > BLOCK_LIMIT {
                return Self::plain(source);
            }
        }
        projection.flush();
        Self {
            source: source.into(),
            blocks: projection.blocks,
        }
    }

    /// Text shown as written, such as the user's own message. Bare links still work.
    pub fn plain(source: &str) -> Self {
        let mut block = Block {
            text: source.into(),
            ..Block::default()
        };
        autolink(&mut block);
        Self {
            source: source.into(),
            blocks: vec![block],
        }
    }
}

#[derive(Default)]
struct Projection {
    document: bool,
    images: Vec<(usize, String)>,
    blocks: Vec<Block>,
    current: Block,
    strong: usize,
    emphasis: usize,
    strike: usize,
    quotes: usize,
    lists: Vec<Option<u64>>,
    /// Open links and images: destination and where the label starts.
    links: Vec<(String, usize)>,
    table: Option<Table>,
    row: Vec<Range<usize>>,
    cell: usize,
}

impl Projection {
    fn flush(&mut self) {
        let mut block = std::mem::take(&mut self.current);
        match &block.kind {
            Kind::Rule => {}
            Kind::Code(language) => {
                let text = block.text.strip_suffix('\n').unwrap_or(&block.text);
                let code = Block::code(language, text);
                block = code;
            }
            _ if block.text.is_empty() => return,
            _ => autolink(&mut block),
        }
        if !matches!(block.kind, Kind::List { .. }) {
            block.indent = self.lists.len().min(8);
        }
        self.blocks.push(block);
    }

    fn text(&mut self, text: &str, code: bool) {
        let start = self.current.text.len();
        self.current.text.push_str(text);
        let end = self.current.text.len();
        let style = Style {
            strong: self.strong > 0,
            emphasis: self.emphasis > 0,
            strike: self.strike > 0,
            code,
            ..Style::default()
        };
        if start == end || style == Style::default() {
            return;
        }
        if let Some((range, previous)) = self.current.spans.last_mut()
            && *previous == style
            && range.end == start
        {
            range.end = end;
        } else {
            self.current.spans.push((start..end, style));
        }
    }

    fn item(&mut self) {
        self.flush();
        let depth = self.lists.len().saturating_sub(1).min(8);
        let prefix = match self.lists.last_mut() {
            Some(Some(number)) => {
                let prefix = format!("{number}. ");
                *number = number.saturating_add(1);
                prefix
            }
            _ => ["• ", "◦ ", "▪ "][depth.min(2)].into(),
        };
        self.current.kind = Kind::List {
            depth,
            marker: prefix.len(),
        };
        self.current.text.push_str(&prefix);
    }

    /// A task marker replaces the bullet of the item it starts.
    fn task(&mut self, checked: bool) {
        let mark = if checked { "☑ " } else { "☐ " };
        match &mut self.current.kind {
            Kind::List { marker, .. } if self.current.text.len() == *marker => {
                self.current.text.clear();
                self.current.text.push_str(mark);
                *marker = mark.len();
            }
            _ => self.text(mark, false),
        }
    }

    fn paragraph(&mut self) {
        // The first paragraph of a list item continues the line its marker started.
        let opens_item = matches!(
            self.current.kind,
            Kind::List { marker, .. } if self.current.text.len() == marker
        );
        if !opens_item {
            self.flush();
            if self.quotes > 0 {
                self.current.kind = Kind::Quote;
            }
        }
    }

    fn end_link(&mut self, image: bool) {
        if image && !self.document {
            self.text("]", false);
        }
        let Some((destination, start)) = self.links.pop() else {
            return;
        };
        let start = start.min(self.current.text.len());
        if image && self.document {
            self.images.push((self.blocks.len(), destination.clone()));
        }
        let url = if self.document {
            Some(destination.as_str())
                .filter(|url| url.len() <= URL_LIMIT && self.current.links.len() < 4096)
        } else {
            safe_url(&destination).filter(|_| !image && self.current.links.len() < 4096)
        };
        let Some(url) = url else {
            // Not a web link: keep the destination readable and inert.
            if !destination.is_empty() {
                self.text(&format!(" ({destination})"), false);
            }
            return;
        };
        if start == self.current.text.len() {
            self.text(url, false);
        }
        let end = self.current.text.len();
        let index = self.current.links.len() as u16;
        self.current.links.push(url.into());
        restyle(&mut self.current.spans, start..end, |style| {
            style.link = Some(index)
        });
    }

    fn start_row(&mut self) {
        if self
            .table
            .as_ref()
            .is_some_and(|table| !table.rows.is_empty())
        {
            self.current.text.push('\n');
        }
        self.row.clear();
    }

    fn end_table(&mut self) {
        let Some(mut table) = self.table.take() else {
            return;
        };
        let cells: usize = table.rows.iter().map(Vec::len).sum();
        if cells > CELL_LIMIT && !self.document {
            self.current.kind = Kind::Code(String::new());
            return self.flush();
        }
        for row in &table.rows {
            for (column, cell) in table.columns.iter_mut().zip(row) {
                let width = self.current.text[cell.clone()].chars().count();
                column.width = column.width.max(width);
            }
        }
        self.current.kind = Kind::Table(table);
        self.flush();
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                self.flush();
                self.current.kind = Kind::Code(match kind {
                    CodeBlockKind::Fenced(language) => language
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(40)
                        .collect(),
                    CodeBlockKind::Indented => String::new(),
                });
            }
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush();
                self.current.kind = Kind::Heading(level as u8);
            }
            Event::Start(Tag::Paragraph) => self.paragraph(),
            Event::Start(Tag::List(first)) => {
                self.flush();
                self.lists.push(first);
            }
            Event::Start(Tag::Item) => self.item(),
            Event::Start(Tag::BlockQuote(_)) => {
                self.flush();
                self.quotes += 1;
            }
            Event::Start(Tag::Table(alignments)) => {
                self.flush();
                let columns = alignments.iter().map(|alignment| Column {
                    align: match alignment {
                        Alignment::None | Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    },
                    width: 0,
                });
                self.table = Some(Table {
                    columns: columns.collect(),
                    rows: Vec::new(),
                });
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => self.start_row(),
            Event::Start(Tag::TableCell) => {
                if !self.row.is_empty() {
                    self.current.text.push('\t');
                }
                self.cell = self.current.text.len();
            }
            Event::End(TagEnd::TableCell) => {
                self.row.push(self.cell..self.current.text.len());
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                let row = std::mem::take(&mut self.row);
                if let Some(table) = &mut self.table {
                    table.rows.push(row);
                }
            }
            Event::End(TagEnd::Table) => self.end_table(),
            Event::Start(Tag::Strong) => self.strong += 1,
            Event::Start(Tag::Emphasis) => self.emphasis += 1,
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strong) => self.strong = self.strong.saturating_sub(1),
            Event::End(TagEnd::Emphasis) => self.emphasis = self.emphasis.saturating_sub(1),
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.links
                    .push((dest_url.into_string(), self.current.text.len()));
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                if !self.document {
                    self.text("[image: ", false);
                }
                self.links
                    .push((dest_url.into_string(), self.current.text.len()));
            }
            Event::End(TagEnd::Link) => self.end_link(false),
            Event::End(TagEnd::Image) => self.end_link(true),
            Event::End(TagEnd::List(_)) => {
                self.flush();
                self.lists.pop();
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.flush();
                self.quotes = self.quotes.saturating_sub(1);
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::HtmlBlock,
            ) => self.flush(),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                self.text(&text, false)
            }
            Event::Code(text) => self.text(&text, true),
            Event::SoftBreak => self.text(" ", false),
            // A table row is one line of text, so a break inside a cell is a space.
            Event::HardBreak if self.table.is_some() => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => {
                self.flush();
                self.current.kind = Kind::Rule;
                self.flush();
            }
            Event::TaskListMarker(checked) => self.task(checked),
            _ => {}
        }
    }
}

/// Apply `change` to every style inside `range`, splitting spans at its ends
/// and covering unstyled text, so the spans stay sorted and non-overlapping.
pub fn restyle<T: Clone + Default>(
    spans: &mut Vec<(Range<usize>, T)>,
    range: Range<usize>,
    change: impl Fn(&mut T),
) {
    if range.is_empty() {
        return;
    }
    let changed = |mut style: T| {
        change(&mut style);
        style
    };
    let mut out = Vec::with_capacity(spans.len() + 2);
    // Start of the text in `range` that no span has covered yet.
    let mut uncovered = range.start;
    for (span, style) in spans.drain(..) {
        if span.end <= range.start {
            out.push((span, style));
            continue;
        }
        let inner = span.start.max(range.start)..span.end.min(range.end);
        let gap = uncovered..inner.start.min(range.end);
        if !gap.is_empty() {
            out.push((gap, changed(T::default())));
        }
        if inner.is_empty() {
            // Entirely after the range.
            uncovered = range.end;
            out.push((span, style));
            continue;
        }
        if span.start < inner.start {
            out.push((span.start..inner.start, style.clone()));
        }
        out.push((inner.clone(), changed(style.clone())));
        uncovered = inner.end;
        if inner.end < span.end {
            out.push((inner.end..span.end, style));
        }
    }
    if uncovered < range.end {
        out.push((uncovered..range.end, changed(T::default())));
    }
    *spans = out;
}

/// Turn bare http and https URLs in prose into links.
fn autolink(block: &mut Block) {
    let mut from = 0;
    while let Some(start) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| block.text[from..].find(scheme))
        .min()
        .map(|found| from + found)
    {
        let end = url_end(&block.text, start);
        from = end.max(start + 1);
        let inside_word = block.text[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        let taken = block.spans.iter().any(|(range, style)| {
            (style.code || style.link.is_some()) && range.start < end && start < range.end
        });
        let Some(url) = safe_url(&block.text[start..end]).filter(|_| !inside_word && !taken) else {
            continue;
        };
        let Ok(index) = u16::try_from(block.links.len()) else {
            return;
        };
        block.links.push(url.into());
        restyle(&mut block.spans, start..end, |style| {
            style.link = Some(index)
        });
    }
}

/// Where a bare URL that starts at `start` ends: before whitespace, quoting,
/// and sentence punctuation or an unmatched closing bracket at its tail.
fn url_end(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    let stop = rest
        .find(|ch: char| ch.is_whitespace() || ch.is_control() || "<>\"`|".contains(ch))
        .unwrap_or(rest.len());
    let mut url = &rest[..stop];
    loop {
        let unmatched = |open: char, close: char| {
            url.ends_with(close) && url.matches(close).count() > url.matches(open).count()
        };
        let trailing = url.ends_with(['.', ',', ';', ':', '!', '?', '\'', '*', '_', '~']);
        if trailing || unmatched('(', ')') || unmatched('[', ']') || unmatched('{', '}') {
            // Every stripped character is one byte.
            url = &url[..url.len() - 1];
        } else {
            return start + url.len();
        }
    }
}

/// Everything the conversation view draws, prepared off the render thread.
#[derive(Default)]
pub struct Transcript {
    id: String,
    pub messages: HashMap<String, Arc<Message>>,
    pub message_labels: HashMap<String, Arc<str>>,
    pub live_label: Option<Arc<str>>,
    pub live: Option<Arc<Message>>,
    pub reasoning: HashMap<String, Arc<Message>>,
    pub live_reasoning: Option<Arc<Message>>,
    pub turn_summaries: HashMap<String, crate::history::TurnSummary>,
    pub tools: HashMap<String, Arc<ToolView>>,
    pub runs: Runs,
}

impl Transcript {
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        let messages = |map: &HashMap<String, Arc<Message>>| {
            map.capacity() * (size_of::<(String, Arc<Message>)>() + 2)
                + map
                    .iter()
                    .map(|(id, message)| id.capacity() + message.retained_bytes())
                    .sum::<usize>()
        };
        size_of::<Self>()
            + self.id.capacity()
            + messages(&self.messages)
            + messages(&self.reasoning)
            + self.message_labels.capacity() * (size_of::<(String, Arc<str>)>() + 2)
            + self
                .message_labels
                .iter()
                .map(|(id, label)| id.capacity() + label.len())
                .sum::<usize>()
            + self.live_label.as_ref().map_or(0, |label| label.len())
            + self
                .live
                .iter()
                .chain(&self.live_reasoning)
                .map(|m| m.retained_bytes())
                .sum::<usize>()
            + self.tools.capacity() * (size_of::<(String, Arc<ToolView>)>() + 2)
            + self
                .tools
                .iter()
                .map(|(id, t)| id.capacity() + t.retained_bytes())
                .sum::<usize>()
            + self.turn_summaries.capacity()
                * (size_of::<(String, crate::history::TurnSummary)>() + 2)
            + self
                .turn_summaries
                .iter()
                .map(|(id, t)| {
                    id.capacity()
                        + t.files.capacity() * size_of::<String>()
                        + t.files.iter().map(String::capacity).sum::<usize>()
                })
                .sum::<usize>()
            + self.runs.retained_bytes()
    }
    pub fn update(previous: &Self, snapshot: &Snapshot) -> Self {
        Self::update_stored(previous, snapshot, &std::collections::HashSet::new())
    }

    pub fn update_stored(
        previous: &Self,
        snapshot: &Snapshot,
        stored: &std::collections::HashSet<String>,
    ) -> Self {
        Self::update_cancellable(previous, snapshot, stored, &|| false)
            .expect("uncancelled projection")
    }

    pub fn update_cancellable(
        previous: &Self,
        snapshot: &Snapshot,
        stored: &std::collections::HashSet<String>,
        cancelled: &impl Fn() -> bool,
    ) -> Option<Self> {
        if cancelled() {
            return None;
        }
        let previous = (previous.id == snapshot.id).then_some(previous);
        let messages = snapshot
            .messages
            .iter()
            .map(|message| {
                if cancelled() {
                    return None;
                }
                let cached = previous
                    .and_then(|p| p.messages.get(&message.id))
                    .filter(|cached| cached.source == message.text);
                let parsed = cached.cloned().unwrap_or_else(|| {
                    Arc::new(match message.role {
                        MessageRole::User => Message::request(&message.text),
                        MessageRole::Assistant => Message::parse(&message.text),
                    })
                });
                Some((message.id.clone(), parsed))
            })
            .collect::<Option<HashMap<_, _>>>()?;
        if cancelled() {
            return None;
        }
        let live = (!snapshot.live_text.is_empty()).then(|| {
            previous
                .and_then(|p| p.live.as_ref())
                .filter(|cached| cached.source == snapshot.live_text)
                .cloned()
                .unwrap_or_else(|| Arc::new(Message::parse(&snapshot.live_text)))
        });
        let reasoning = snapshot
            .messages
            .iter()
            .filter(|message| !message.reasoning.is_empty())
            .map(|message| {
                if cancelled() {
                    return None;
                }
                let cached = previous
                    .and_then(|p| p.reasoning.get(&message.id))
                    .filter(|cached| cached.source == message.reasoning);
                Some((
                    message.id.clone(),
                    cached
                        .cloned()
                        .unwrap_or_else(|| Arc::new(Message::parse(&message.reasoning))),
                ))
            })
            .collect::<Option<HashMap<_, _>>>()?;
        if cancelled() {
            return None;
        }
        let live_reasoning = (!snapshot.live_reasoning.is_empty()).then(|| {
            previous
                .and_then(|p| p.live_reasoning.as_ref())
                .filter(|cached| cached.source == snapshot.live_reasoning)
                .cloned()
                .unwrap_or_else(|| Arc::new(Message::parse(&snapshot.live_reasoning)))
        });
        let tools = snapshot
            .tools
            .iter()
            .map(|tool| {
                if cancelled() {
                    return None;
                }
                let cached = previous
                    .and_then(|p| p.tools.get(&tool.id))
                    .filter(|cached| {
                        cached.shows(tool) && cached.is_stored() == stored.contains(&tool.id)
                    });
                let view = cached.cloned().unwrap_or_else(|| {
                    Arc::new(if stored.contains(&tool.id) {
                        ToolView::from_history(tool)
                    } else {
                        ToolView::new(tool)
                    })
                });
                Some((tool.id.clone(), view))
            })
            .collect::<Option<HashMap<_, _>>>()?;
        if cancelled() {
            return None;
        }
        let message_labels = snapshot
            .messages
            .iter()
            .map(|message| {
                let prefix = if message.role == MessageRole::User {
                    "Your message: "
                } else {
                    "Plait message: "
                };
                let label = previous
                    .and_then(|p| {
                        p.messages
                            .get(&message.id)
                            .zip(p.message_labels.get(&message.id))
                    })
                    .filter(|(old, label)| old.source == message.text && label.starts_with(prefix))
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| format!("{prefix}{}", message.text).into());
                (message.id.clone(), label)
            })
            .collect();
        let live_label = (!snapshot.live_text.is_empty()).then(|| {
            previous
                .and_then(|p| p.live.as_ref().zip(p.live_label.as_ref()))
                .filter(|(old, _)| old.source == snapshot.live_text)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| format!("Plait is writing: {}", snapshot.live_text).into())
        });
        Some(Self {
            message_labels,
            live_label,
            id: snapshot.id.clone(),
            messages,
            live,
            reasoning,
            live_reasoning,
            turn_summaries: crate::history::turn_summaries(snapshot),
            tools,
            runs: Runs::plan(snapshot),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(message: &Message) -> String {
        message
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect()
    }

    fn assert_spans_are_ordered(message: &Message) {
        for block in &message.blocks {
            let mut end = 0;
            for (range, _) in &block.spans {
                assert!(range.start >= end && range.start < range.end, "{range:?}");
                assert!(block.text.is_char_boundary(range.start));
                assert!(block.text.is_char_boundary(range.end));
                end = range.end;
            }
            assert!(end <= block.text.len());
        }
    }

    #[test]
    fn code_is_literal_and_unclosed_streaming_fences_are_readable() {
        for source in [
            "```rust\nfn main() { **literal** }\n```",
            "```rust\nfn main() { **literal** }",
        ] {
            let message = Message::parse(source);
            assert_eq!(message.blocks.len(), 1);
            assert_eq!(message.blocks[0].kind, Kind::Code("rust".into()));
            assert_eq!(message.blocks[0].text, "fn main() { **literal** }");
        }
    }

    #[test]
    fn fenced_code_carries_syntax_tokens_for_known_languages() {
        let message =
            Message::parse("```rust\nfn main() {}\n```\n\n```nonsense\nfn main() {}\n```");
        let keyword = Style {
            token: Some(Token::Keyword),
            ..Style::default()
        };
        assert_eq!(message.blocks[0].spans[0], (0..2, keyword));
        assert!(message.blocks[1].spans.is_empty());
        assert_spans_are_ordered(&message);
    }

    #[test]
    fn paragraphs_headings_lists_and_unicode_spans_are_native_data() {
        let message = Message::parse(
            "# Result\n\n**🙂 strong** and `code` and ~~gone~~.\n\n1. First\n2. Second\n\n> A quote",
        );
        assert_eq!(message.blocks[0].kind, Kind::Heading(1));
        let styles = |pick: fn(&Style) -> bool| {
            message.blocks[1]
                .spans
                .iter()
                .filter(|(_, style)| pick(style))
                .map(|(range, _)| &message.blocks[1].text[range.clone()])
                .collect::<Vec<_>>()
        };
        assert_eq!(styles(|style| style.strong), ["🙂 strong"]);
        assert_eq!(styles(|style| style.code), ["code"]);
        assert_eq!(styles(|style| style.strike), ["gone"]);
        assert!(message.blocks.iter().any(|block| block.text == "2. Second"));
        assert!(message.blocks.iter().any(|block| block.kind == Kind::Quote));
        assert_spans_are_ordered(&message);
    }

    #[test]
    fn nested_lists_keep_depth_markers_and_continuation_indent() {
        let message = Message::parse(
            "- top\n  - nested\n    1. deep\n\n       continued\n\n       ```sh\n       ls\n       ```\n- [x] done\n- [ ] open\n\nafter",
        );
        let shape: Vec<_> = message
            .blocks
            .iter()
            .map(|block| (block.text.as_str(), block.kind.clone(), block.indent))
            .collect();
        let item = |depth, marker: &str| Kind::List {
            depth,
            marker: marker.len(),
        };
        assert_eq!(
            shape,
            [
                ("• top", item(0, "• "), 0),
                ("◦ nested", item(1, "◦ "), 0),
                ("1. deep", item(2, "1. "), 0),
                ("continued", Kind::Paragraph, 3),
                ("ls", Kind::Code("sh".into()), 3),
                ("☑ done", item(0, "☑ "), 0),
                ("☐ open", item(0, "☐ "), 0),
                ("after", Kind::Paragraph, 0),
            ]
        );
    }

    #[test]
    fn tables_keep_cells_alignment_and_inline_styles() {
        let message = Message::parse(
            "| Name | Count |\n| :-- | --: |\n| **plait** | 12 |\n| `core` wider | 3 |\n\nAfter",
        );
        let block = &message.blocks[0];
        let Kind::Table(table) = &block.kind else {
            panic!("expected a table, got {:?}", block.kind);
        };
        assert_eq!(block.text, "Name\tCount\nplait\t12\ncore wider\t3");
        let cells: Vec<Vec<&str>> = table
            .rows
            .iter()
            .map(|row| row.iter().map(|cell| &block.text[cell.clone()]).collect())
            .collect();
        assert_eq!(
            cells,
            [["Name", "Count"], ["plait", "12"], ["core wider", "3"]]
        );
        assert_eq!(
            table.columns,
            [
                Column {
                    align: Align::Left,
                    width: 10
                },
                Column {
                    align: Align::Right,
                    width: 5
                }
            ]
        );
        assert!(
            block
                .spans
                .iter()
                .any(|(range, style)| style.strong && &block.text[range.clone()] == "plait")
        );
        assert!(
            block
                .spans
                .iter()
                .any(|(range, style)| style.code && &block.text[range.clone()] == "core")
        );
        assert_eq!(message.blocks[1].text, "After");
        assert_spans_are_ordered(&message);
    }

    #[test]
    fn oversized_tables_become_tab_separated_text() {
        let row = "| a | b | c | d |\n";
        let source = format!("{row}| - | - | - | - |\n{}", row.repeat(400));
        let message = Message::parse(&source);
        assert_eq!(message.blocks.len(), 1);
        assert_eq!(message.blocks[0].kind, Kind::Code(String::new()));
        assert!(message.blocks[0].text.starts_with("a\tb\tc\td\na\tb"));
    }

    #[test]
    fn web_links_carry_their_destination_and_keep_label_styles() {
        let message = Message::parse(
            "See [the **docs**](https://example.com/a?b=1) or <https://example.org>, [](http://empty.example).",
        );
        let block = &message.blocks[0];
        assert_eq!(
            block.text,
            "See the docs or https://example.org, http://empty.example."
        );
        assert_eq!(
            block.links,
            [
                "https://example.com/a?b=1",
                "https://example.org",
                "http://empty.example"
            ]
        );
        let linked: Vec<_> = block
            .spans
            .iter()
            .filter_map(|(range, style)| {
                Some((&block.text[range.clone()], style.link?, style.strong))
            })
            .collect();
        assert_eq!(
            linked,
            [
                ("the ", 0, false),
                ("docs", 0, true),
                ("https://example.org", 1, false),
                ("http://empty.example", 2, false),
            ]
        );
        assert_eq!(block.link_at(5), Some("https://example.com/a?b=1"));
        assert_eq!(block.link_at(0), None);
        assert_spans_are_ordered(&message);
    }

    #[test]
    fn bare_urls_become_links_without_trailing_punctuation() {
        let message = Message::parse(
            "Read https://example.com/a_(b). Also (http://example.com/x), `https://code.example` and xhttps://no.example.",
        );
        let block = &message.blocks[0];
        assert_eq!(
            block.links,
            ["https://example.com/a_(b)", "http://example.com/x"]
        );
        for (range, style) in &block.spans {
            if let Some(index) = style.link {
                assert_eq!(&block.text[range.clone()], block.links[usize::from(index)]);
            }
        }
        let user = Message::plain("look at https://example.com/é?q=1, please\nhttp://");
        assert_eq!(user.blocks[0].links, ["https://example.com/é?q=1"]);
        assert_eq!(user.blocks[0].text, user.source);
        assert_spans_are_ordered(&message);
        assert_spans_are_ordered(&user);
    }

    #[test]
    fn html_images_and_unsafe_urls_are_text_not_actions() {
        let message = Message::parse(
            "<script>alert(1)</script>\n\n[x](javascript:bad) ![alt](file:///private/file) ![remote](https://example.com/a.png) [mail](mailto:a@example.com)",
        );
        let rendered = text(&message);
        assert!(rendered.contains("<script>"));
        assert!(rendered.contains("javascript:bad"));
        assert!(rendered.contains("file:///private/file"));
        assert!(rendered.contains("mailto:a@example.com"));
        let links: Vec<_> = message
            .blocks
            .iter()
            .flat_map(|block| &block.links)
            .collect();
        // An image is never fetched; its web address is an ordinary link to click.
        assert_eq!(links, ["https://example.com/a.png"]);
        assert!(rendered.contains("[image: remote] (https://example.com/a.png)"));
    }

    #[test]
    fn only_plain_http_and_https_destinations_are_safe() {
        for url in [
            "http://example.com",
            "HTTPS://Example.com/path?q=1#frag",
            "https://127.0.0.1:8080/",
        ] {
            assert_eq!(safe_url(url), Some(url));
        }
        for url in [
            "",
            "https://",
            "https:///path",
            "http://exa mple.com",
            "https://example.com/\u{7}",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://example.com",
            " https://example.com",
            "plait://open",
            "data:text/html,x",
            "//example.com",
            "é",
        ] {
            assert_eq!(safe_url(url), None, "{url:?}");
        }
        assert_eq!(
            safe_url(&format!("https://e.com/{}", "a".repeat(URL_LIMIT))),
            None
        );
    }

    #[test]
    fn restyle_splits_spans_and_fills_gaps() {
        let strong = Style {
            strong: true,
            ..Style::default()
        };
        let mut spans = vec![(2..6, strong), (8..10, strong), (20..22, strong)];
        restyle(&mut spans, 4..12, |style| style.link = Some(7));
        let linked = |style: Style| Style {
            link: Some(7),
            ..style
        };
        assert_eq!(
            spans,
            [
                (2..4, strong),
                (4..6, linked(strong)),
                (6..8, linked(Style::default())),
                (8..10, linked(strong)),
                (10..12, linked(Style::default())),
                (20..22, strong),
            ]
        );
        let mut spans = vec![(0..9, strong)];
        restyle(&mut spans, 3..5, |style| style.link = Some(1));
        assert_eq!(spans.len(), 3);
        let mut spans: Vec<(Range<usize>, Style)> = Vec::new();
        restyle(&mut spans, 3..5, |style| style.link = Some(1));
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn excessive_blocks_fall_back_without_losing_content() {
        let source = "line\n\n".repeat(BLOCK_LIMIT + 1);
        let message = Message::parse(&source);
        assert_eq!(message.blocks.len(), 1);
        assert_eq!(message.blocks[0].text, source);
    }

    #[test]
    fn streaming_retains_unchanged_message_projections() {
        let mut snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "id":"thread", "checkout":"/repo", "model":{"provider":"p","model_id":"m"},
            "status":"running", "messages":[
                {"id":"message","role":"assistant","text":"**hello**"},
                {"id":"mine","role":"user","text":"**as written**"}
            ],
            "live_text":"hello", "pending_approval":null,"error":null,"truncated":false,
            "tools":[{"id":"t1","name":"read","status":"running","input":"{\"path\":\"a.rs\"}","output":""}]
        }))
        .unwrap();
        let checks = std::cell::Cell::new(0);
        assert!(
            Transcript::update_cancellable(
                &Transcript::default(),
                &snapshot,
                &std::collections::HashSet::new(),
                &|| {
                    checks.set(checks.get() + 1);
                    checks.get() >= 3
                }
            )
            .is_none()
        );
        assert_eq!(checks.get(), 3, "retired work stops between messages");
        let first = Transcript::update(&Transcript::default(), &snapshot);
        assert!(first.retained_bytes() > snapshot.messages[0].text.capacity());
        assert_eq!(first.messages["message"].blocks[0].text, "hello");
        assert_eq!(first.messages["mine"].blocks[0].text, "**as written**");
        snapshot.live_text.push_str(" world");
        let second = Transcript::update(&first, &snapshot);
        assert!(Arc::ptr_eq(
            &first.messages["message"],
            &second.messages["message"]
        ));
        assert!(Arc::ptr_eq(&first.tools["t1"], &second.tools["t1"]));
        assert!(!Arc::ptr_eq(
            first.live.as_ref().unwrap(),
            second.live.as_ref().unwrap()
        ));
        snapshot.tools[0].output = "fn main() {}".into();
        let third = Transcript::update(&second, &snapshot);
        assert!(!Arc::ptr_eq(&second.tools["t1"], &third.tools["t1"]));
        snapshot.messages.clear();
        snapshot.tools.clear();
        let last = Transcript::update(&third, &snapshot);
        assert!(last.messages.is_empty() && last.tools.is_empty());
    }
}
