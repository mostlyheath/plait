//! Handoff preferences and custom command validation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A command line longer than this is not a command anyone types.
const COMMAND_LIMIT: usize = 2048;

/// What opens a file. Unknown values read as the system default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "with", content = "command", rename_all = "snake_case")]
pub enum EditorChoice {
    /// The file's default application, through `open`. It has no line.
    #[default]
    System,
    /// `nvim +{line} -- {file}` in a new Rex tab at the checkout.
    RexNeovim,
    /// A command line with `{file}`, `{line}`, and `{project}`.
    Custom(String),
}

/// What opens a terminal. Unknown values read as Terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "with", content = "command", rename_all = "snake_case")]
pub enum TerminalChoice {
    /// A Terminal window at the checkout.
    #[default]
    Terminal,
    /// A Rex tab at the checkout.
    Rex,
    /// A command line with `{project}`.
    Custom(String),
}

impl EditorChoice {
    /// The control's label, in a word or two.
    pub fn label(&self) -> &'static str {
        match self {
            Self::System => "System default",
            Self::RexNeovim => "Neovim in Rex",
            Self::Custom(_) => "Custom",
        }
    }

    /// Whether the file opens at a line.
    pub fn takes_line(&self) -> bool {
        match self {
            Self::System => false,
            Self::RexNeovim => true,
            Self::Custom(line) => line.contains("{line}"),
        }
    }

    /// Where a file goes, for hover text: "in Neovim in a Rex tab".
    pub fn describe(&self) -> String {
        match self {
            Self::System => "in its default app".into(),
            Self::RexNeovim => "in Neovim in a Rex tab".into(),
            Self::Custom(line) => format!("with {}", program_name(line)),
        }
    }
}

impl TerminalChoice {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Rex => "Rex tab",
            Self::Custom(_) => "Custom",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Terminal => "in Terminal".into(),
            Self::Rex => "in a Rex tab".into(),
            Self::Custom(line) => format!("with {}", program_name(line)),
        }
    }
}

/// The first word of a command line, for hover text. Its quoting was checked
/// when it was saved; a line that does not parse is named as written.
fn program_name(line: &str) -> String {
    words(line)
        .ok()
        .and_then(|words| words.into_iter().next())
        .unwrap_or_else(|| line.trim().to_owned())
}

/// A program and its arguments, ready to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// Where it starts: the checkout.
    pub directory: Option<PathBuf>,
    /// It talks to Rex, so a missing Rex is named as such.
    pub rex: bool,
}

impl Launch {
    pub fn new(program: &str, args: Vec<String>) -> Self {
        Self {
            program: program.to_owned(),
            args,
            directory: None,
            rex: false,
        }
    }

    pub fn at(mut self, directory: &Path) -> Self {
        self.directory = Some(directory.to_owned());
        self
    }
}

/// Split one line into words: whitespace separates them; single quotes keep
/// everything literally; double quotes keep everything but a backslash
/// before `"` or `\`; elsewhere a backslash keeps the next character. Nothing
/// is expanded: `$`, backticks, `~`, and globs are plain characters.
pub fn words(line: &str) -> Result<Vec<String>, String> {
    if line.contains(['\n', '\r']) {
        return Err("The command must be one line.".into());
    }
    if line.len() > COMMAND_LIMIT {
        return Err(format!(
            "The command is longer than {COMMAND_LIMIT} characters."
        ));
    }
    let mut words = Vec::new();
    // `Some` once a word has begun, so `''` is an empty word, not nothing.
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        match ch {
            ch if ch.is_whitespace() => words.extend(word.take()),
            '\\' => {
                let next = chars
                    .next()
                    .ok_or("The command ends with a backslash, which escapes nothing.")?;
                word.get_or_insert_default().push(next);
            }
            '\'' => {
                let word = word.get_or_insert_default();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => word.push(ch),
                        None => return Err("A single quote is not closed.".into()),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_default();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch @ ('"' | '\\')) => word.push(ch),
                            Some(ch) => {
                                word.push('\\');
                                word.push(ch);
                            }
                            None => return Err("A double quote is not closed.".into()),
                        },
                        Some(ch) => word.push(ch),
                        None => return Err("A double quote is not closed.".into()),
                    }
                }
            }
            ch => word.get_or_insert_default().push(ch),
        }
    }
    words.extend(word);
    Ok(words)
}

/// The placeholders a command may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placeholder {
    File,
    Line,
    Project,
}

impl Placeholder {
    const ALL: [(Self, &'static str); 3] = [
        (Self::File, "{file}"),
        (Self::Line, "{line}"),
        (Self::Project, "{project}"),
    ];
}

/// One piece of a word: plain text or a placeholder.
#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Placeholder(Placeholder),
    /// `{name}` with a name that is not a placeholder.
    Unknown(&'a str),
}

/// Split a word at its placeholders in one pass, so text filled in for one
/// is never read again as another.
fn pieces(word: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = word;
    while let Some(open) = rest.find('{') {
        let after = &rest[open..];
        let known = Placeholder::ALL
            .iter()
            .find(|(_, written)| after.starts_with(written));
        let (piece, length) = match known {
            Some((placeholder, written)) => (Piece::Placeholder(*placeholder), written.len()),
            None => match unknown_name(after) {
                Some(name) => (Piece::Unknown(name), name.len()),
                None => {
                    // A brace that names nothing is plain text.
                    pieces.push(Piece::Text(&rest[..open + 1]));
                    rest = &rest[open + 1..];
                    continue;
                }
            },
        };
        if open > 0 {
            pieces.push(Piece::Text(&rest[..open]));
        }
        pieces.push(piece);
        rest = &after[length..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest));
    }
    pieces
}

/// `{name}` at the start of `text` when `name` is letters and underscores:
/// a misspelled placeholder, which is refused rather than passed on as text.
fn unknown_name(text: &str) -> Option<&str> {
    let close = text.find('}')?;
    let name = &text[1..close];
    let looks = !name.is_empty() && name.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '_');
    looks.then_some(&text[..=close])
}

/// A custom command, checked: its program, then argument templates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    words: Vec<String>,
}

/// What a custom command is for, which decides the placeholders it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Editor,
    Terminal,
}

/// Check a custom command line. The error is one sentence saying why it
/// cannot be used, shown beneath the field; nothing invalid is saved.
pub fn template(line: &str, purpose: Purpose) -> Result<Template, String> {
    let words = words(line)?;
    let Some(program) = words.first() else {
        return Err(match purpose {
            Purpose::Editor => "Write a command, such as zed {file}:{line}.".into(),
            Purpose::Terminal => "Write a command, such as wezterm start --cwd {project}.".into(),
        });
    };
    if program.is_empty() || program.contains('{') {
        return Err("Start the command with the program to run.".into());
    }
    let mut used = Vec::new();
    for word in &words {
        for piece in pieces(word) {
            match piece {
                Piece::Placeholder(placeholder) => used.push(placeholder),
                Piece::Unknown(name) => {
                    return Err(format!(
                        "{name} is not a placeholder. Use {{file}}, {{line}}, or {{project}}."
                    ));
                }
                Piece::Text(_) => {}
            }
        }
    }
    match purpose {
        Purpose::Editor if !used.contains(&Placeholder::File) => {
            Err("The command needs {file}, where the file's path goes.".into())
        }
        Purpose::Terminal if used.iter().any(|used| *used != Placeholder::Project) => {
            Err("A terminal opens at a project, so only {project} can be used.".into())
        }
        _ => Ok(Template { words }),
    }
}

/// What fills the placeholders.
#[derive(Clone, Copy, Debug)]
pub struct Spot<'a> {
    pub file: Option<&'a str>,
    /// One-based.
    pub line: usize,
    pub project: &'a str,
}

impl Template {
    pub fn program(&self) -> &str {
        &self.words[0]
    }

    /// The program and its arguments with every placeholder filled.
    pub fn fill(&self, spot: Spot) -> Launch {
        let line = spot.line.to_string();
        let fill = |word: &String| {
            pieces(word)
                .into_iter()
                .map(|piece| match piece {
                    Piece::Text(text) | Piece::Unknown(text) => text,
                    Piece::Placeholder(Placeholder::File) => spot.file.unwrap_or_default(),
                    Piece::Placeholder(Placeholder::Line) => &line,
                    Piece::Placeholder(Placeholder::Project) => spot.project,
                })
                .collect::<String>()
        };
        Launch::new(&self.words[0], self.words[1..].iter().map(fill).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn words_follow_ordinary_quoting() {
        let parsed = |line: &str| words(line).unwrap();
        assert_eq!(parsed("zed  {file}:{line}"), ["zed", "{file}:{line}"]);
        assert_eq!(parsed("  a\tb  "), ["a", "b"]);
        assert_eq!(parsed("'a b' \"c d\" e\\ f"), ["a b", "c d", "e f"]);
        // Single quotes keep backslashes and double quotes.
        assert_eq!(parsed(r#"'a\b"c'"#), [r#"a\b"c"#]);
        // Double quotes take a backslash only before `"` and `\`.
        assert_eq!(parsed(r#""a\"b\\c\$d\n""#), [r#"a"b\c\$d\n"#]);
        // Outside quotes a backslash keeps any character.
        assert_eq!(parsed(r"a\'b\\c\$"), [r"a'b\c$"]);
        // Quotes join with what touches them; empty quotes are an empty word.
        assert_eq!(parsed("a'b'\"c\"d '' \"\""), ["abcd", "", ""]);
        // Nothing is expanded.
        assert_eq!(parsed("$HOME ~ * `x`"), ["$HOME", "~", "*", "`x`"]);
        assert_eq!(parsed("é 日本"), ["é", "日本"]);
        assert!(parsed("   ").is_empty());
    }

    #[test]
    fn words_refuse_what_cannot_be_read() {
        for (line, reason) in [
            ("vim 'open", "single quote"),
            ("vim \"open", "double quote"),
            ("vim \"open\\", "double quote"),
            ("vim trailing\\", "backslash"),
            ("vim\nrm", "one line"),
            ("vim\rrm", "one line"),
        ] {
            let error = words(line).unwrap_err();
            assert!(error.contains(reason), "{line:?}: {error}");
        }
        assert!(words(&"a".repeat(COMMAND_LIMIT + 1)).is_err());
    }

    #[test]
    fn templates_need_their_placeholders_and_refuse_unknown_ones() {
        let editor = |line: &str| template(line, Purpose::Editor);
        let terminal = |line: &str| template(line, Purpose::Terminal);
        assert!(editor("zed {file}:{line}").is_ok());
        assert!(editor("/opt/bin/subl --wait '{file}' {project}").is_ok());
        assert!(editor("code -g").unwrap_err().contains("{file}"));
        assert!(editor("").unwrap_err().contains("Write a command"));
        assert!(editor("{file}").unwrap_err().contains("program"));
        assert!(editor("'' {file}").unwrap_err().contains("program"));
        assert!(
            editor("zed {File}")
                .unwrap_err()
                .contains("{File} is not a placeholder")
        );
        assert!(editor("zed {path} {file}").is_err());
        // A brace that names nothing is text.
        assert!(editor("zed {file} {} {1} {a-b} {").is_ok());
        assert!(terminal("wezterm start --cwd {project}").is_ok());
        assert!(terminal("open -a Ghostty").is_ok());
        assert!(terminal("kitty {file}").unwrap_err().contains("{project}"));
        assert!(terminal("kitty +{line}").is_err());
    }

    #[test]
    fn placeholders_fill_inside_each_word_without_a_shell() {
        let template = template(
            "zed '{file}:{line}' --root={project} {file}{file}",
            Purpose::Editor,
        )
        .unwrap();
        let file = "/a {line} $(x) {project}.rs";
        let launch = template.fill(Spot {
            file: Some(file),
            line: 42,
            project: "/p q",
        });
        assert_eq!(launch.program, "zed");
        // Text filled in for one placeholder is never read as another.
        assert_eq!(
            launch.args,
            [
                format!("{file}:42"),
                "--root=/p q".to_owned(),
                format!("{file}{file}")
            ]
        );
        assert!(!launch.rex);
    }

    #[test]
    fn choices_describe_themselves_for_hover_text() {
        assert_eq!(EditorChoice::RexNeovim.describe(), "in Neovim in a Rex tab");
        assert!(EditorChoice::RexNeovim.takes_line());
        assert!(!EditorChoice::System.takes_line());
        let custom = EditorChoice::Custom("'/Applications/My Editor' {file}".into());
        assert_eq!(custom.describe(), "with /Applications/My Editor");
        assert!(!custom.takes_line());
        assert_eq!(TerminalChoice::Rex.describe(), "in a Rex tab");
    }

    #[test]
    fn choices_read_older_and_newer_values() {
        let editor: EditorChoice = serde_json::from_str(r#"{"with":"rex_neovim"}"#).unwrap();
        assert_eq!(editor, EditorChoice::RexNeovim);
        let custom = EditorChoice::Custom("zed {file}".into());
        let written = serde_json::to_string(&custom).unwrap();
        assert_eq!(written, r#"{"with":"custom","command":"zed {file}"}"#);
        assert_eq!(
            serde_json::from_str::<EditorChoice>(&written).unwrap(),
            custom
        );
        let terminal = serde_json::to_string(&TerminalChoice::Rex).unwrap();
        assert_eq!(terminal, r#"{"with":"rex"}"#);
    }
}
