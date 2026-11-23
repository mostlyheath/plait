//! Ways out: open a file in the person's editor at a line, show it in Finder,
//! or open a terminal at a checkout.
//!
//! This builds the program and arguments to start, then starts them without
//! waiting: it learns only whether the program could not be started or
//! exited at once with an error. Starting blocks briefly; call it on the
//! background executor.
//!
//! A custom command is one line, parsed here into words with ordinary quoting
//! rules and never passed through a shell. The Rex choices are built in, not
//! templates, because they need two layers of escaping: the AppleScript
//! string, and the command line Rex runs in the person's shell.

use crate::model::{DiffKind, DiffLine};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
};

/// Rex is addressed by bundle ID, so the release and beta builds both answer.
pub const REX_BUNDLE: &str = "com.superlogical.rex";
const OSASCRIPT: &str = "/usr/bin/osascript";
const OPEN: &str = "/usr/bin/open";
const TERMINAL_BUNDLE: &str = "com.apple.Terminal";
/// A command line longer than this is not a command anyone types.
const COMMAND_LIMIT: usize = 2048;
/// AppleScript's "can't get application" error: the bundle ID is not installed.
const NO_APPLICATION: &str = "(-1728)";
/// AppleScript's "not authorized to send Apple events": Automation is off.
const NOT_ALLOWED: &str = "(-1743)";

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
    fn new(program: &str, args: Vec<String>) -> Self {
        Self {
            program: program.to_owned(),
            args,
            directory: None,
            rex: false,
        }
    }

    fn osascript(lines: Vec<String>) -> Self {
        let args = lines.into_iter().flat_map(|line| ["-e".to_owned(), line]);
        Self {
            rex: true,
            ..Self::new(OSASCRIPT, args.collect())
        }
    }

    fn at(mut self, directory: &Path) -> Self {
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
    fn fill(&self, spot: Spot) -> Launch {
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

/// One word for a POSIX shell (sh, bash, zsh): in single quotes, which keep
/// everything, with each single quote closed, escaped, and reopened.
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Words as one shell command line, each quoted.
pub fn shell_line(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Text as an AppleScript string literal. Line breaks and tabs are written as
/// escapes, so each line of the script stays one `osascript -e` argument.
pub fn applescript_string(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            ch => literal.push(ch),
        }
    }
    literal.push('"');
    literal
}

/// The command line Rex runs for Neovim. `--` ends Neovim's options, so a
/// path that begins with a dash is still a file.
pub fn neovim_line(file: &str, line: usize) -> String {
    shell_line(&["nvim", &format!("+{line}"), "--", file])
}

/// The script that opens `file` at `line` in Neovim, in a new Rex tab at the
/// checkout. One line per `osascript -e`.
pub fn rex_editor_script(file: &str, line: usize, project: &str) -> Vec<String> {
    vec![
        format!("tell application id {}", applescript_string(REX_BUNDLE)),
        "activate".into(),
        format!(
            "run command {} at {}",
            applescript_string(&neovim_line(file, line)),
            applescript_string(project)
        ),
        "end tell".into(),
    ]
}

/// The script that opens a Rex tab at the checkout.
pub fn rex_terminal_script(project: &str) -> Vec<String> {
    vec![
        format!("set checkout to POSIX file {}", applescript_string(project)),
        format!("tell application id {}", applescript_string(REX_BUNDLE)),
        "activate".into(),
        "open checkout".into(),
        "end tell".into(),
    ]
}

/// A path as text, or why it cannot be handed on. Only absolute paths are
/// handed on, so none can be read as an option.
fn path_text(path: &Path) -> Result<&str, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path.", path.display()));
    }
    path.to_str().ok_or_else(|| {
        format!(
            "{} is not UTF-8 text, so it cannot be passed on.",
            path.display()
        )
    })
}

/// Open `file` at `line` (one-based) with the chosen editor.
pub fn open_in_editor(
    choice: &EditorChoice,
    file: &Path,
    line: usize,
    project: &Path,
) -> Result<Launch, String> {
    let (file_text, project_text) = (path_text(file)?, path_text(project)?);
    let launch = match choice {
        EditorChoice::System => Launch::new(OPEN, vec![file_text.to_owned()]),
        EditorChoice::RexNeovim => {
            Launch::osascript(rex_editor_script(file_text, line, project_text))
        }
        EditorChoice::Custom(command) => template(command, Purpose::Editor)?.fill(Spot {
            file: Some(file_text),
            line,
            project: project_text,
        }),
    };
    Ok(launch.at(project))
}

/// Show `file` selected in a Finder window.
pub fn show_in_finder(file: &Path) -> Result<Launch, String> {
    Ok(Launch::new(
        OPEN,
        vec!["-R".into(), path_text(file)?.to_owned()],
    ))
}

/// Open a terminal at `project` with the chosen terminal.
pub fn open_terminal(choice: &TerminalChoice, project: &Path) -> Result<Launch, String> {
    let text = path_text(project)?;
    let launch = match choice {
        TerminalChoice::Terminal => Launch::new(
            OPEN,
            vec!["-b".into(), TERMINAL_BUNDLE.into(), text.to_owned()],
        ),
        TerminalChoice::Rex => Launch::osascript(rex_terminal_script(text)),
        TerminalChoice::Custom(command) => template(command, Purpose::Terminal)?.fill(Spot {
            file: None,
            line: 1,
            project: text,
        }),
    };
    Ok(launch.at(project))
}

/// The line a diff starts its change at: the first added or removed line,
/// as a line of the file the diff leads to. `None` with no change.
pub fn first_changed_line(lines: &[DiffLine]) -> Option<usize> {
    let first = lines
        .iter()
        .position(|line| matches!(line.kind, DiffKind::Added | DiffKind::Removed))?;
    line_near(lines, first)
}

/// Patch line `index` as a line of the file the diff leads to. A removed line
/// is no longer there: it is where the next remaining line of its hunk is,
/// or after the last one before it.
pub fn line_near(lines: &[DiffLine], index: usize) -> Option<usize> {
    let line = lines.get(index)?;
    if let Some(number) = line.new_line {
        return Some(number);
    }
    let in_hunk = |line: &&DiffLine| !matches!(line.kind, DiffKind::Hunk | DiffKind::Header);
    let after = lines[index..].iter().take_while(in_hunk);
    if let Some(number) = after.filter_map(|line| line.new_line).next() {
        return Some(number);
    }
    let before = lines[..index].iter().rev().take_while(in_hunk);
    let last = before.filter_map(|line| line.new_line).next();
    Some(last.map_or(1, |number| number + 1))
}

/// How a started process stands, checked without waiting.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still running: it is left to run.
    Running,
    /// Exited successfully, as `open` and `osascript` do once handed off.
    Done,
    /// One sentence naming the command and what went wrong.
    Failed(String),
}

/// Start the program. Its output is not read; error output is kept only to
/// say why it failed, if it exits at once.
pub fn start(launch: &Launch) -> Result<Child, String> {
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(directory) = &launch.directory {
        command.current_dir(directory);
    }
    command.spawn().map_err(|error| {
        let hint = if error.kind() == std::io::ErrorKind::NotFound && !launch.program.contains('/')
        {
            " Write its full path in Settings → Editor; Plait does not read your shell's PATH."
        } else {
            ""
        };
        format!("Could not start {}: {error}.{hint}", launch.program)
    })
}

/// Check once whether the process has exited, and how.
pub fn check(child: &mut Child, launch: &Launch) -> Outcome {
    match child.try_wait() {
        Ok(None) => Outcome::Running,
        Ok(Some(status)) if status.success() => Outcome::Done,
        Ok(Some(status)) => Outcome::Failed(failure(launch, status, &error_output(child))),
        Err(error) => Outcome::Failed(format!("Could not follow {}: {error}.", launch.program)),
    }
}

/// Leave a process that is still running to itself. A thread empties its
/// error output, so a full pipe never stops it, and reaps it when it exits.
pub fn release(mut child: Child) {
    // If no thread can be made, the child still runs; it is only not reaped.
    let _ = std::thread::Builder::new()
        .name("plait-handoff".into())
        .spawn(move || {
            if let Some(mut pipe) = child.stderr.take() {
                let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            }
            let _ = child.wait();
        });
}

/// What an exited process wrote to its error output, without waiting: a
/// process it started may still hold the pipe open.
fn error_output(child: &mut Child) -> String {
    use std::os::fd::AsRawFd;
    let Some(mut pipe) = child.stderr.take() else {
        return String::new();
    };
    // SAFETY: the descriptor belongs to `pipe`, which outlives these calls.
    let blocking = unsafe {
        let flags = libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL);
        flags < 0 || libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
    };
    if blocking {
        return String::new();
    }
    let mut bytes = vec![0; 4096];
    let read = pipe.read(&mut bytes).unwrap_or(0);
    String::from_utf8_lossy(&bytes[..read]).into_owned()
}

/// One sentence for a process that exited with an error.
fn failure(launch: &Launch, status: ExitStatus, output: &str) -> String {
    if launch.rex && output.contains(NO_APPLICATION) {
        return format!(
            "Rex is not installed: {} could not find {REX_BUNDLE}.",
            launch.program
        );
    }
    if launch.rex && output.contains(NOT_ALLOWED) {
        return format!(
            "{} is not allowed to control Rex; allow Plait in System Settings → Privacy & Security → Automation.",
            launch.program
        );
    }
    let code = status.code().map_or_else(
        || "was stopped".to_owned(),
        |code| format!("exited with status {code}"),
    );
    match output.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(reason) => format!("{} {code}: {reason}", launch.program),
        None => format!("{} {code}.", launch.program),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Paths that break naive quoting in one way or another.
    const HARD: &[&str] = &[
        "/tmp/plain.rs",
        "/tmp/with space/file name.rs",
        "/tmp/it's here.rs",
        "/tmp/\"quoted\".rs",
        "/tmp/back\\slash\\\\double.rs",
        "/tmp/$HOME and ${PATH}.rs",
        "/tmp/`whoami` $(id).rs",
        "/tmp/line\nbreak.rs",
        "/tmp/tab\there\rreturn.rs",
        "/tmp/é 日本 🦀.rs",
        "-leading-dash.rs",
        "/tmp/;|&<>*?[]!#~.rs",
        "/tmp/''\"\"''.rs",
        "",
    ];

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
    fn shell_quoting_keeps_every_word_whole() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("-x"), "'-x'");
        assert_eq!(shell_quote("$HOME`x`\\"), "'$HOME`x`\\'");
        // Read back as a shell would, with nothing but single-quoted text
        // and escaped single quotes allowed: it is the same word.
        for path in HARD {
            let quoted = shell_quote(path);
            assert_eq!(strict_unquote(&quoted).as_deref(), Some(*path), "{quoted}");
        }
    }

    /// A shell word that is only `'…'` runs and `\'`, unquoted; `None` if
    /// anything else, which a shell could expand, is outside the quotes.
    fn strict_unquote(word: &str) -> Option<String> {
        let mut text = String::new();
        let mut chars = word.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '\'' => loop {
                    match chars.next()? {
                        '\'' => break,
                        ch => text.push(ch),
                    }
                },
                '\\' if chars.next()? == '\'' => text.push('\''),
                _ => return None,
            }
        }
        Some(text)
    }

    #[test]
    fn applescript_strings_escape_quotes_backslashes_and_breaks() {
        assert_eq!(applescript_string("plain"), "\"plain\"");
        assert_eq!(applescript_string("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(applescript_string("a\nb\rc\td"), r#""a\nb\rc\td""#);
        assert_eq!(applescript_string("it's $x `y` é"), "\"it's $x `y` é\"");
        for path in HARD {
            let literal = applescript_string(path);
            assert!(!literal.contains(['\n', '\r']), "{path:?}");
            // Every quote inside is escaped: only the outer two end the string.
            let inner = &literal[1..literal.len() - 1];
            let mut escaped = false;
            for ch in inner.chars() {
                assert!(escaped || ch != '"', "{path:?}: {literal}");
                escaped = !escaped && ch == '\\';
            }
            assert!(!escaped, "{path:?} ends in a lone backslash");
        }
    }

    #[test]
    fn neovim_ends_its_options_before_the_file() {
        assert_eq!(neovim_line("-x.rs", 7), "'nvim' '+7' '--' '-x.rs'");
        assert_eq!(
            rex_editor_script("/w/it's.rs", 3, "/w"),
            [
                "tell application id \"com.superlogical.rex\"",
                "activate",
                r#"run command "'nvim' '+3' '--' '/w/it'\\''s.rs'" at "/w""#,
                "end tell",
            ]
        );
        assert_eq!(
            rex_terminal_script("/w \"x\""),
            [
                r#"set checkout to POSIX file "/w \"x\"""#,
                "tell application id \"com.superlogical.rex\"",
                "activate",
                "open checkout",
                "end tell",
            ]
        );
    }

    #[test]
    fn each_choice_builds_its_program_and_arguments() {
        let (file, project) = (Path::new("/w/src/lib.rs"), Path::new("/w"));
        let system = open_in_editor(&EditorChoice::System, file, 42, project).unwrap();
        assert_eq!(
            (system.program.as_str(), system.args.as_slice()),
            (OPEN, &["/w/src/lib.rs".to_owned()][..])
        );
        assert_eq!(system.directory.as_deref(), Some(project));
        let rex = open_in_editor(&EditorChoice::RexNeovim, file, 42, project).unwrap();
        assert_eq!(rex.program, OSASCRIPT);
        assert!(rex.rex);
        let lines = rex_editor_script("/w/src/lib.rs", 42, "/w");
        let expected = lines
            .iter()
            .flat_map(|line| ["-e".to_owned(), line.clone()]);
        assert_eq!(rex.args, expected.collect::<Vec<_>>());
        let custom = EditorChoice::Custom("zed {file}:{line}".into());
        let custom = open_in_editor(&custom, file, 42, project).unwrap();
        assert_eq!(
            (custom.program.as_str(), custom.args.as_slice()),
            ("zed", &["/w/src/lib.rs:42".to_owned()][..])
        );
        let finder = show_in_finder(file).unwrap();
        assert_eq!(finder.args, ["-R", "/w/src/lib.rs"]);
        let terminal = open_terminal(&TerminalChoice::Terminal, project).unwrap();
        assert_eq!(terminal.args, ["-b", TERMINAL_BUNDLE, "/w"]);
        let rex = open_terminal(&TerminalChoice::Rex, project).unwrap();
        assert!(rex.rex && rex.args.contains(&"open checkout".to_owned()));
        // A relative path, or a saved command that no longer reads, is refused.
        assert!(open_in_editor(&EditorChoice::System, Path::new("lib.rs"), 1, project).is_err());
        assert!(open_terminal(&TerminalChoice::Custom("kitty {file}".into()), project).is_err());
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

    fn line(kind: DiffKind, old: Option<usize>, new: Option<usize>) -> DiffLine {
        DiffLine {
            text: Arc::from(""),
            kind,
            old_line: old,
            new_line: new,
        }
    }

    #[test]
    fn a_diff_opens_at_its_first_change() {
        use DiffKind::*;
        let added = [
            line(Header, None, None),
            line(Hunk, None, None),
            line(Context, Some(9), Some(9)),
            line(Added, None, Some(10)),
        ];
        assert_eq!(first_changed_line(&added), Some(10));
        // A removed line is where the next remaining line is.
        let removed = [
            line(Hunk, None, None),
            line(Context, Some(4), Some(4)),
            line(Removed, Some(5), None),
            line(Removed, Some(6), None),
            line(Context, Some(7), Some(5)),
        ];
        assert_eq!(first_changed_line(&removed), Some(5));
        assert_eq!(line_near(&removed, 3), Some(5));
        // Removed at a hunk's end: after the last line before it.
        let tail = [
            line(Hunk, None, None),
            line(Context, Some(4), Some(4)),
            line(Removed, Some(5), None),
            line(Hunk, None, None),
            line(Context, Some(20), Some(19)),
        ];
        assert_eq!(first_changed_line(&tail), Some(5));
        // A file emptied entirely opens at its start.
        let emptied = [line(Hunk, None, None), line(Removed, Some(1), None)];
        assert_eq!(first_changed_line(&emptied), Some(1));
        assert_eq!(first_changed_line(&[line(Header, None, None)]), None);
        assert_eq!(line_near(&added, 9), None);
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

    #[test]
    fn a_failure_is_one_sentence_naming_the_command() {
        use std::os::unix::process::ExitStatusExt;
        let rex = open_terminal(&TerminalChoice::Rex, Path::new("/w")).unwrap();
        let status = ExitStatus::from_raw(1 << 8);
        let missing = failure(
            &rex,
            status,
            "0:40: execution error: Can’t get application id \"com.superlogical.rex\". (-1728)\n",
        );
        assert_eq!(
            missing,
            "Rex is not installed: /usr/bin/osascript could not find com.superlogical.rex."
        );
        let refused = failure(
            &rex,
            status,
            "execution error: Not authorized to send Apple events to Rex. (-1743)",
        );
        assert!(refused.contains("Automation"), "{refused}");
        let open = show_in_finder(Path::new("/w/a.rs")).unwrap();
        assert_eq!(
            failure(&open, status, "\nThe file /w/a.rs does not exist.\n"),
            "/usr/bin/open exited with status 1: The file /w/a.rs does not exist."
        );
        assert_eq!(
            failure(&open, status, ""),
            "/usr/bin/open exited with status 1."
        );
    }

    /// Run a program and return what it printed, or panic with its errors.
    #[cfg(target_os = "macos")]
    fn output(program: &str, args: &[&str]) -> String {
        let output = Command::new(program).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// The AppleScript literal reads back as the same text in AppleScript
    /// itself. `return` only computes a string; nothing is sent to an app.
    #[cfg(target_os = "macos")]
    #[test]
    fn applescript_reads_each_literal_back_unchanged() {
        for path in HARD {
            let script = format!("return {}", applescript_string(path));
            let printed = output(OSASCRIPT, &["-e", &script]);
            // osascript ends what it prints with one line break.
            assert_eq!(printed.strip_suffix('\n'), Some(*path), "{path:?}");
        }
    }

    /// Both layers together, as Rex receives them: the command line read
    /// from its AppleScript literal, then run by sh and zsh, gives every word
    /// back whole. `printf` stands in for Neovim.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_rex_command_line_survives_applescript_and_the_shell() {
        for shell in ["/bin/sh", "/bin/zsh"] {
            for path in HARD {
                let line = shell_line(&["printf", "%s\\0", "--", path]);
                let script = format!("return {}", applescript_string(&line));
                let received = output(OSASCRIPT, &["-e", &script]);
                let received = received.strip_suffix('\n').unwrap();
                assert_eq!(received, line, "{path:?}");
                let printed = output(shell, &["-c", received]);
                assert_eq!(printed, format!("--\0{path}\0"), "{shell} {path:?}");
            }
        }
    }

    /// The generated scripts compile against Rex's own dictionary. Compiling
    /// only resolves terms; nothing is run and Rex is not asked to do anything.
    /// Needs Rex installed, so it runs on request.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs Rex installed; run with --ignored"]
    fn rex_scripts_compile() {
        let scratch = std::env::temp_dir().join(format!("plait-rex-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let compiled = scratch.join("script.scpt");
        let scripts = HARD
            .iter()
            .flat_map(|path| [rex_editor_script(path, 12, path), rex_terminal_script(path)]);
        for script in scripts {
            let mut args = vec!["-o".to_owned(), compiled.to_string_lossy().into_owned()];
            args.extend(
                script
                    .iter()
                    .flat_map(|line| ["-e".to_owned(), line.clone()]),
            );
            let status = Command::new("/usr/bin/osacompile")
                .args(&args)
                .status()
                .unwrap();
            assert!(status.success(), "{script:?}");
        }
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
