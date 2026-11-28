//! Native editor, terminal, and Finder handoffs. Run process work in the background.
use plait_client::handoff::{EditorChoice, Launch, Purpose, Spot, TerminalChoice, template};
use plait_client::model::{DiffKind, DiffLine};
use std::{
    io::Read,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
};
const REX_BUNDLE: &str = "com.superlogical.rex";
const OSASCRIPT: &str = "/usr/bin/osascript";
const OPEN: &str = "/usr/bin/open";
const TERMINAL_BUNDLE: &str = "com.apple.Terminal";
const NO_APPLICATION: &str = "(-1728)";
const NOT_ALLOWED: &str = "(-1743)";

fn osascript(lines: Vec<String>) -> Launch {
    let args = lines.into_iter().flat_map(|line| ["-e".to_owned(), line]);
    Launch {
        rex: true,
        ..Launch::new(OSASCRIPT, args.collect())
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
        EditorChoice::RexNeovim => osascript(rex_editor_script(file_text, line, project_text)),
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
        TerminalChoice::Rex => osascript(rex_terminal_script(text)),
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
#[cfg_attr(test, allow(dead_code))]
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
#[cfg_attr(test, allow(dead_code))]
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
#[cfg_attr(test, allow(dead_code))]
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
#[cfg_attr(test, allow(dead_code))]
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
#[cfg_attr(test, allow(dead_code))]
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
        let scratch =
            plait_test_support::temp_dir().join(format!("plait-rex-{}", std::process::id()));
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
