//! What a tool call shows: a presentation per tool, and how a long run of
//! calls folds under one summary line. Built with the conversation projection,
//! off the render thread; the view only draws the result.

use crate::display_text::escape;
use crate::{
    diff::{self, Change},
    highlight,
    host::{Snapshot, Tool, ToolStatus},
    markdown::{Block, DiffLine, Kind},
    transcript::{self, Slot},
};
use std::collections::HashMap;

/// A run still in progress keeps this many of its latest calls in view.
pub const LIVE_VISIBLE: usize = 3;
/// Lines of a file shown for `read`.
const PREVIEW_LINES: usize = 12;
/// UTF-16 units of each tool string that the host keeps, from its end.
const HOST_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Old text against new text, for `edit`.
    Diff,
    /// The new file, for `write`.
    Content,
    /// The command line, for `command`.
    Command,
    /// What a command printed, or why a call failed.
    Output,
    /// The start of what `read` returned.
    Preview,
    /// Arguments that could not be presented any other way.
    Input,
}

#[derive(Debug)]
pub struct Section {
    pub part: Part,
    pub block: Block,
    /// A limit the reader should know about, such as clipped output.
    pub note: Option<String>,
}

#[derive(Debug)]
pub struct ToolView {
    name: String,
    input: String,
    output: String,
    status: ToolStatus,
    stored: bool,
    /// Short facts for the one-line row, such as `+3 −1` or `42 lines`.
    pub facts: Option<String>,
    pub sections: Vec<Section>,
}

impl ToolView {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.capacity()
            + self.input.capacity()
            + self.output.capacity()
            + self.facts.as_ref().map_or(0, String::capacity)
            + self.sections.capacity() * std::mem::size_of::<Section>()
            + self
                .sections
                .iter()
                .map(|s| s.block.retained_bytes() + s.note.as_ref().map_or(0, String::capacity))
                .sum::<usize>()
    }
    pub fn new(tool: &Tool) -> Self {
        Self::build(tool, false)
    }

    pub fn from_history(tool: &Tool) -> Self {
        Self::build(tool, true)
    }

    fn build(tool: &Tool, stored: bool) -> Self {
        let arguments = serde_json::from_str::<serde_json::Value>(&tool.input).ok();
        let field = |name: &str| arguments.as_ref()?.get(name)?.as_str();
        let mut facts = None;
        let sections = match tool.name.as_str() {
            "edit" => edit(tool, &field, &mut facts),
            "write" => write(tool, &field, &mut facts),
            "command" => command(tool, &field, stored),
            "read" => read(tool, &field, &mut facts, stored),
            _ => None,
        };
        Self {
            name: tool.name.clone(),
            input: tool.input.clone(),
            output: tool.output.clone(),
            status: tool.status,
            stored,
            facts,
            sections: sections.unwrap_or_else(|| raw(tool, stored)),
        }
    }

    /// Whether this view was built from the tool as it is now.
    pub fn is_stored(&self) -> bool {
        self.stored
    }

    pub fn shows(&self, tool: &Tool) -> bool {
        self.status == tool.status
            && self.name == tool.name
            && self.input == tool.input
            && self.output == tool.output
    }

    /// What Copy puts on the clipboard: every section as shown.
    pub fn text(&self) -> String {
        let parts: Vec<&str> = self
            .sections
            .iter()
            .map(|section| section.block.text.as_str())
            .collect();
        parts.join("\n\n")
    }
}

type Field<'a> = &'a dyn Fn(&str) -> Option<&'a str>;

fn clipped(text: &str) -> bool {
    // The host may drop one more unit to keep a surrogate pair whole.
    text.encode_utf16().count() >= HOST_LIMIT - 1
}

fn failed(tool: &Tool) -> bool {
    matches!(tool.status, ToolStatus::Error | ToolStatus::Interrupted)
}

fn plain(part: Part, text: &str, note: Option<&str>) -> Section {
    Section {
        part,
        block: Block::code("", escape(text.trim_end_matches('\n'))),
        note: note.map(str::to_owned),
    }
}

/// The result text explains a failure; on success the change itself is the result.
fn failure(tool: &Tool, sections: &mut Vec<Section>) {
    if failed(tool) && !tool.output.is_empty() {
        sections.push(plain(Part::Output, &tool.output, None));
    }
}

fn edit(tool: &Tool, field: Field<'_>, facts: &mut Option<String>) -> Option<Vec<Section>> {
    field("path")?;
    let old = escape(field("old_text")?);
    let new = escape(field("new_text")?);
    let lines = diff::lines(&old, &new);
    let (added, removed) = diff::counts(&lines);
    *facts = Some(format!("+{added} −{removed}"));
    let mut sections = vec![Section {
        part: Part::Diff,
        block: diff_block(&lines),
        note: None,
    }];
    failure(tool, &mut sections);
    Some(sections)
}

/// One line of text per diff line, each after a two-byte marker, so the
/// copied text reads as a diff.
fn diff_block(lines: &[(Change, &str)]) -> Block {
    let mut text = String::new();
    let mut rows = Vec::with_capacity(lines.len());
    for (change, line) in lines {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(match change {
            Change::Context => "  ",
            Change::Removed => "- ",
            Change::Added => "+ ",
        });
        let start = text.len();
        text.push_str(line);
        rows.push(DiffLine {
            change: *change,
            range: start..text.len(),
        });
    }
    Block {
        kind: Kind::Diff(rows),
        text,
        ..Block::default()
    }
}

fn line_count(text: &str) -> String {
    match text.lines().count() {
        1 => "1 line".into(),
        count => format!("{count} lines"),
    }
}

fn write(tool: &Tool, field: Field<'_>, facts: &mut Option<String>) -> Option<Vec<Section>> {
    let (path, content) = (field("path")?, field("content")?);
    *facts = Some(line_count(content));
    let language = highlight::language_for_path(path);
    let mut sections = vec![Section {
        part: Part::Content,
        block: Block::code(language, escape(content.trim_end_matches('\n'))),
        note: None,
    }];
    failure(tool, &mut sections);
    Some(sections)
}

fn command(tool: &Tool, field: Field<'_>, stored: bool) -> Option<Vec<Section>> {
    let mut sections = vec![Section {
        part: Part::Command,
        block: Block::code("sh", escape(field("command")?.trim())),
        note: None,
    }];
    if !tool.output.is_empty() {
        let note =
            (!stored && clipped(&tool.output)).then_some("Earlier output was clipped by the host.");
        sections.push(plain(Part::Output, &tool.output, note));
    }
    Some(sections)
}

fn read(
    tool: &Tool,
    field: Field<'_>,
    facts: &mut Option<String>,
    stored: bool,
) -> Option<Vec<Section>> {
    let path = field("path")?;
    if failed(tool) || tool.output.is_empty() {
        let mut sections = Vec::new();
        failure(tool, &mut sections);
        return Some(sections);
    }
    let partial = !stored && clipped(&tool.output);
    let total = tool.output.lines().count();
    let shown: Vec<&str> = tool
        .output
        .lines()
        .take(if stored { usize::MAX } else { PREVIEW_LINES })
        .collect();
    if !partial {
        *facts = Some(line_count(&tool.output));
    }
    let more = match total - shown.len() {
        0 => None,
        1 => Some("1 more line".to_owned()),
        more => Some(format!("{more} more lines")),
    };
    let note = match (partial, more) {
        (true, Some(more)) => Some(format!("The host kept only the end of this file. {more}.")),
        (true, None) => Some("The host kept only the end of this file.".into()),
        (false, more) => more.map(|more| format!("{more}.")),
    };
    Some(vec![Section {
        part: Part::Preview,
        block: Block::code(
            highlight::language_for_path(path),
            escape(&shown.join("\n")),
        ),
        note,
    }])
}

/// Arguments that are clipped or unreadable are shown as they arrived.
fn raw(tool: &Tool, stored: bool) -> Vec<Section> {
    let mut sections = Vec::new();
    if !tool.input.is_empty() {
        let known = matches!(tool.name.as_str(), "read" | "edit" | "write" | "command");
        let note = if !stored && clipped(&tool.input) {
            Some("The host kept only the end of these arguments.")
        } else if known {
            Some("These arguments could not be read, so they are shown as received.")
        } else {
            None
        };
        sections.push(plain(Part::Input, &tool.input, note));
    }
    if !tool.output.is_empty() {
        let note =
            (!stored && clipped(&tool.output)).then_some("Earlier output was clipped by the host.");
        sections.push(plain(Part::Output, &tool.output, note));
    }
    sections
}

/// Consecutive tool calls with no message between them.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    /// Tool IDs in conversation order.
    pub tools: Vec<String>,
    /// Row of the first tool in the conversation layout this was planned from.
    pub first_row: usize,
    /// How many leading calls sit under the summary line; zero for a short run.
    pub grouped: usize,
    /// What the summary line says about the grouped calls.
    pub summary: String,
    /// Grouped calls that ended in an error.
    pub failed: usize,
}

impl Run {
    /// Disclosure state for the group follows its first tool.
    pub fn key(&self) -> &str {
        &self.tools[0]
    }
}

/// Where one tool sits in its run.
#[derive(Clone, Copy, Debug)]
pub struct Place<'a> {
    pub run: &'a Run,
    pub position: usize,
}

impl Place<'_> {
    /// This row also carries the group's summary line.
    pub fn leads(&self) -> bool {
        self.position == 0 && self.run.grouped > 0
    }

    /// This call is shown only while its group is open.
    pub fn folded(&self) -> bool {
        self.position < self.run.grouped
    }
}

#[derive(Debug, Default)]
pub struct Runs {
    runs: Vec<Run>,
    members: HashMap<String, (usize, usize)>,
}

impl Runs {
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        self.runs.capacity() * size_of::<Run>()
            + self
                .runs
                .iter()
                .map(|r| {
                    r.summary.capacity()
                        + r.tools.capacity() * size_of::<String>()
                        + r.tools.iter().map(String::capacity).sum::<usize>()
                })
                .sum::<usize>()
            + self.members.capacity() * (size_of::<(String, (usize, usize))>() + 2)
            + self.members.keys().map(String::capacity).sum::<usize>()
    }
    /// Within the active turn, earlier runs may group fully and the last
    /// run keeps its newest calls visible. Closed turns own their disclosures.
    pub fn plan(snapshot: &Snapshot) -> Self {
        let layout = transcript::layout(snapshot);
        let last_message = layout
            .iter()
            .rposition(|slot| matches!(slot, Slot::Message(_)));
        let last_request = layout.iter().rposition(|slot| {
            matches!(slot, Slot::Message(ix) if snapshot.messages[*ix].role == crate::host::MessageRole::User)
        });
        let mut spans: Vec<(usize, Vec<&Tool>)> = Vec::new();
        let mut open = false;
        for (row, slot) in layout.iter().enumerate() {
            let tool = match slot {
                Slot::Tool(index) => snapshot.tools.get(*index),
                _ => None,
            };
            match (tool, open) {
                (Some(tool), true) => spans.last_mut().expect("open run").1.push(tool),
                (Some(tool), false) => spans.push((row, vec![tool])),
                (None, _) => {}
            }
            open = tool.is_some();
        }
        let mut plan = Self::default();
        for (first_row, tools) in spans {
            let active = snapshot.status.busy() && last_request.is_none_or(|row| first_row > row);
            let live = active && last_message.is_none_or(|row| first_row > row);
            let grouped = if live && tools.len() > LIVE_VISIBLE + 1 {
                tools.len() - LIVE_VISIBLE
            } else if active && tools.len() > LIVE_VISIBLE + 1 {
                tools.len()
            } else {
                0
            };
            let run = plan.runs.len();
            for (position, tool) in tools.iter().enumerate() {
                plan.members.insert(tool.id.clone(), (run, position));
            }
            plan.runs.push(Run {
                first_row,
                grouped,
                summary: if grouped == 0 {
                    String::new()
                } else {
                    summary(&tools[..grouped], live)
                },
                failed: tools[..grouped]
                    .iter()
                    .filter(|tool| tool.status == ToolStatus::Error)
                    .count(),
                tools: tools.iter().map(|tool| tool.id.clone()).collect(),
            });
        }
        plan
    }

    pub fn place(&self, tool: &str) -> Option<Place<'_>> {
        let (run, position) = *self.members.get(tool)?;
        Some(Place {
            run: self.runs.get(run)?,
            position,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Run> {
        self.runs.iter()
    }
}

/// The count and the kinds of call, in the order each kind first appears.
pub(crate) fn summary(tools: &[&Tool], live: bool) -> String {
    if tools.is_empty() {
        return "0 steps".into();
    }
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    for tool in tools {
        match kinds.iter_mut().find(|(name, _)| *name == tool.name) {
            Some((_, count)) => *count += 1,
            None => kinds.push((&tool.name, 1)),
        }
    }
    let kinds: Vec<String> = kinds
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect();
    let steps = match (tools.len(), live) {
        (1, false) => "1 step".to_owned(),
        (count, false) => format!("{count} steps"),
        (1, true) => "1 earlier step".to_owned(),
        (count, true) => format!("{count} earlier steps"),
    };
    format!("{steps} · {}", kinds.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Message, MessageRole, ThreadStatus};
    use serde_json::json;

    fn tool(name: &str, status: &str, input: serde_json::Value, output: &str) -> Tool {
        serde_json::from_value(json!({
            "id": "t", "name": name, "status": status,
            "input": input.to_string(), "output": output
        }))
        .unwrap()
    }

    fn parts(view: &ToolView) -> Vec<Part> {
        view.sections.iter().map(|section| section.part).collect()
    }

    #[test]
    fn copy_joins_tool_sections_with_blank_lines() {
        let view = ToolView::new(&tool(
            "command",
            "done",
            json!({"command": "echo hello"}),
            "hello",
        ));
        assert_eq!(view.sections.len(), 2);
        assert_eq!(view.text(), "echo hello\n\nhello");
    }

    #[test]
    fn edit_shows_a_line_diff_and_copies_as_one() {
        let view = ToolView::new(&tool(
            "edit",
            "done",
            json!({"path": "src/a.rs", "old_text": "let a = 1;\nkeep();", "new_text": "let a = 2;\nlet b = 3;\nkeep();"}),
            "Edited src/a.rs",
        ));
        assert_eq!(parts(&view), [Part::Diff]);
        assert_eq!(view.facts.as_deref(), Some("+2 −1"));
        let block = &view.sections[0].block;
        assert_eq!(
            block.text,
            "- let a = 1;\n+ let a = 2;\n+ let b = 3;\n  keep();"
        );
        let Kind::Diff(lines) = &block.kind else {
            panic!("expected a diff");
        };
        let shown: Vec<_> = lines
            .iter()
            .map(|line| (line.change, &block.text[line.range.clone()]))
            .collect();
        assert_eq!(
            shown,
            [
                (Change::Removed, "let a = 1;"),
                (Change::Added, "let a = 2;"),
                (Change::Added, "let b = 3;"),
                (Change::Context, "keep();"),
            ]
        );
        assert_eq!(view.text(), block.text);
    }

    #[test]
    fn a_failed_edit_keeps_the_diff_and_adds_the_reason() {
        let view = ToolView::new(&tool(
            "edit",
            "error",
            json!({"path": "a", "old_text": "x", "new_text": "y"}),
            "old_text was not found",
        ));
        assert_eq!(parts(&view), [Part::Diff, Part::Output]);
        assert_eq!(view.sections[1].block.text, "old_text was not found");
    }

    #[test]
    fn write_highlights_the_new_file_by_its_extension() {
        let view = ToolView::new(&tool(
            "write",
            "done",
            json!({"path": "notes/plan.toml", "content": "[a]\nb = 1\n"}),
            "Created",
        ));
        assert_eq!(parts(&view), [Part::Content]);
        assert_eq!(view.facts.as_deref(), Some("2 lines"));
        let block = &view.sections[0].block;
        assert_eq!(block.kind, Kind::Code("toml".into()));
        assert_eq!(block.text, "[a]\nb = 1");
        assert!(!block.spans.is_empty());
    }

    #[test]
    fn command_separates_the_prompt_line_from_its_output() {
        let running = ToolView::new(&tool(
            "command",
            "running",
            json!({"command": " cargo test --locked\n"}),
            "",
        ));
        assert_eq!(parts(&running), [Part::Command]);
        assert_eq!(running.sections[0].block.text, "cargo test --locked");
        assert_eq!(running.sections[0].block.kind, Kind::Code("sh".into()));

        let failed = ToolView::new(&tool(
            "command",
            "error",
            json!({"command": "false"}),
            "exit status 1\n",
        ));
        assert_eq!(parts(&failed), [Part::Command, Part::Output]);
        assert_eq!(failed.sections[1].block.text, "exit status 1");
        assert!(failed.sections[1].block.spans.is_empty());
        assert_eq!(failed.sections[1].note, None);

        let long = ToolView::new(&tool(
            "command",
            "done",
            json!({"command": "yes"}),
            &"y\n".repeat(HOST_LIMIT / 2),
        ));
        assert!(
            long.sections[1]
                .note
                .as_deref()
                .unwrap()
                .contains("clipped")
        );
    }

    #[test]
    fn read_previews_the_first_lines_and_counts_the_rest() {
        let content: String = (1..=30).map(|line| format!("line {line}\n")).collect();
        let view = ToolView::new(&tool(
            "read",
            "done",
            json!({"path": "README.md"}),
            &content,
        ));
        assert_eq!(parts(&view), [Part::Preview]);
        assert_eq!(view.facts.as_deref(), Some("30 lines"));
        let section = &view.sections[0];
        assert_eq!(section.block.kind, Kind::Code("markdown".into()));
        assert_eq!(section.block.text.lines().count(), PREVIEW_LINES);
        assert!(section.block.text.ends_with("line 12"));
        assert_eq!(section.note.as_deref(), Some("18 more lines."));

        let short = ToolView::new(&tool("read", "done", json!({"path": "a"}), "one\n"));
        assert_eq!(short.facts.as_deref(), Some("1 line"));
        assert_eq!(short.sections[0].note, None);

        let pending = ToolView::new(&tool("read", "running", json!({"path": "a"}), ""));
        assert!(pending.sections.is_empty());

        let missing = ToolView::new(&tool("read", "error", json!({"path": "a"}), "No such file"));
        assert_eq!(parts(&missing), [Part::Output]);
    }

    #[test]
    fn clipped_or_unreadable_arguments_fall_back_to_the_raw_text() {
        let mut clipped = tool("edit", "done", json!({}), "ok");
        clipped.input = format!("{}\",\"new_text\":\"b\"}}", "a".repeat(HOST_LIMIT));
        let view = ToolView::new(&clipped);
        assert_eq!(parts(&view), [Part::Input, Part::Output]);
        assert_eq!(view.sections[0].block.text, clipped.input);
        assert!(view.sections[0].note.as_deref().unwrap().contains("end of"));
        assert_eq!(view.facts, None);

        // Valid JSON without the expected fields, and fields of the wrong type.
        for input in [
            json!({"path": "a"}),
            json!({"path": 1, "old_text": "a", "new_text": "b"}),
        ] {
            let view = ToolView::new(&tool("edit", "done", input, ""));
            assert_eq!(parts(&view), [Part::Input]);
            assert!(
                view.sections[0]
                    .note
                    .as_deref()
                    .unwrap()
                    .contains("could not")
            );
        }

        let other = ToolView::new(&tool("search", "done", json!({"query": "x"}), "found"));
        assert_eq!(parts(&other), [Part::Input, Part::Output]);
        assert_eq!(other.sections[0].note, None);

        let empty = ToolView::new(&tool("tool", "done", json!(null), ""));
        assert_eq!(parts(&empty), [Part::Input]);
    }

    #[test]
    fn a_view_is_reused_only_while_the_tool_is_unchanged() {
        let mut call = tool("command", "running", json!({"command": "ls"}), "");
        let view = ToolView::new(&call);
        assert!(view.shows(&call));
        call.output.push_str("a.rs");
        assert!(!view.shows(&call));
        let view = ToolView::new(&call);
        call.status = ToolStatus::Done;
        assert!(!view.shows(&call));
    }

    /// A thread whose rows follow `script`: `u` and `a` are messages, any
    /// other word is a tool of that name, and a trailing `!` marks an error.
    fn thread(script: &str, status: ThreadStatus) -> Snapshot {
        let mut snapshot: Snapshot = serde_json::from_value(json!({
            "id":"thread", "checkout":"/repo", "model":{"provider":"p", "model_id":"m"},
            "status":"idle", "messages":[], "live_text":"", "tools":[],
            "pending_approval":null, "error":null, "truncated":false
        }))
        .unwrap();
        snapshot.status = status;
        let mut last: Option<String> = None;
        for (index, word) in script.split_whitespace().enumerate() {
            if matches!(word, "u" | "a") {
                let id = format!("m{index}");
                snapshot.messages.push(Message {
                    id: id.clone(),
                    role: if word == "u" {
                        MessageRole::User
                    } else {
                        MessageRole::Assistant
                    },
                    text: "text".into(),
                    ..Default::default()
                });
                last = Some(id);
            } else {
                let name = word.trim_end_matches('!');
                let mut call = tool(
                    name,
                    if word.ends_with('!') { "error" } else { "done" },
                    json!({}),
                    "",
                );
                call.id = format!("t{index}");
                call.after = Some(last.clone());
                snapshot.tools.push(call);
            }
        }
        snapshot
    }

    fn shape(runs: &Runs) -> Vec<(usize, usize, usize, &str)> {
        runs.iter()
            .map(|run| {
                (
                    run.first_row,
                    run.tools.len(),
                    run.grouped,
                    run.summary.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn closed_turns_never_group_tools_separately() {
        for status in [
            ThreadStatus::Idle,
            ThreadStatus::Stopped,
            ThreadStatus::Interrupted,
            ThreadStatus::Error,
        ] {
            let runs = Runs::plan(&thread("u read read edit command! read a", status));
            assert_eq!(shape(&runs), [(1, 5, 0, "")]);
        }
    }

    #[test]
    fn a_running_turn_keeps_its_latest_steps_in_view() {
        let script = "u a read read read read read a read edit read command edit read";
        let running = Runs::plan(&thread(script, ThreadStatus::Running));
        assert_eq!(
            shape(&running),
            [
                // Followed by a message, so no longer the latest work.
                (2, 5, 5, "5 steps · 5 read"),
                (8, 6, 3, "3 earlier steps · 2 read, 1 edit"),
            ]
        );
        let previous = Runs::plan(&thread(
            "u read read read read read a u read read read read read",
            ThreadStatus::Running,
        ));
        assert_eq!(
            shape(&previous)[0].2,
            0,
            "tools in the previous turn stay individual"
        );
        let latest = running.place("t13").unwrap();
        assert!(!latest.folded());
        assert!(running.place("t10").unwrap().folded());
        assert!(!running.place("t11").unwrap().folded());

        // One hidden call would cost a line and save none.
        let four = Runs::plan(&thread("u read read read read", ThreadStatus::Running));
        assert_eq!(shape(&four)[0].2, 0);
        let waiting = Runs::plan(&thread(
            "u read read read read read",
            ThreadStatus::AwaitingApproval,
        ));
        assert_eq!(shape(&waiting)[0].2, 2);

        // The turn owns its work disclosure once it ends.
        let finished = Runs::plan(&thread(script, ThreadStatus::Idle));
        assert_eq!(shape(&finished)[1], (8, 6, 0, ""));
    }

    #[test]
    fn unordered_tools_from_an_old_host_form_one_trailing_run() {
        let mut snapshot = thread("u a read read read read read", ThreadStatus::Idle);
        for tool in &mut snapshot.tools {
            tool.after = None;
        }
        snapshot.live_text = "partial".into();
        let runs = Runs::plan(&snapshot);
        // Rows: two messages, the stream, then the tools.
        assert_eq!(shape(&runs), [(3, 5, 0, "")]);
    }
}

#[cfg(test)]
mod stored_tests {
    use super::*;
    #[test]
    fn stored_read_and_command_show_all_received_text_without_tail_warnings() {
        let output = (0..80)
            .map(|i| format!("line {i}: {}", "complete ".repeat(12)))
            .collect::<Vec<_>>()
            .join("\n");
        let tool = Tool {
            id: "read".into(),
            name: "read".into(),
            status: ToolStatus::Done,
            input: "{\"path\":\"file.rs\"}".into(),
            output: output.clone(),
            after: Some(None),
        };
        let view = ToolView::from_history(&tool);
        assert_eq!(view.sections[0].block.text, output);
        assert!(view.sections[0].note.is_none());
        let command = Tool {
            name: "command".into(),
            input: "{\"command\":\"test\"}".into(),
            ..tool
        };
        let view = ToolView::from_history(&command);
        assert_eq!(view.sections[1].block.text, output);
        assert!(view.sections[1].note.is_none());
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    #[test]
    fn tool_rows_escape_controls_after_decoding_command_arguments() {
        for name in ["command", "mcp:docs/echo"] {
            let tool = Tool {
                id: "controls".into(),
                name: name.into(),
                status: ToolStatus::Done,
                input: serde_json::json!({"command":"safe\u{202e}delete\u{200b}"}).to_string(),
                output: "result\u{0085}\u{2028}\u{feff}".into(),
                after: None,
            };
            for view in [ToolView::new(&tool), ToolView::from_history(&tool)] {
                let text = view.text();
                assert!(text.contains("safe\\u202edelete\\u200b"));
                assert!(text.contains("result\\u0085\\u2028\\ufeff"));
                assert!(!text.contains('\u{202e}'));
            }
        }
        let approval: crate::host::Approval = serde_json::from_value(serde_json::json!({
            "id":"a","tool_id":"t","kind":"command","summary":"Use echo from docs",
            "details":"safe\u{202e}delete\u{200b}","dirty_paths":[],"risk":"fixture"
        }))
        .unwrap();
        assert_eq!(approval.details, "safe\\u202edelete\\u200b");
    }
}
