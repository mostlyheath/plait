//! Small original tokenizer for code shown in a conversation.
//!
//! It assigns lexical classes only: there is no grammar, no dependency, and no
//! failure mode. Partial code from a streaming answer is highlighted as far as
//! it goes, and an unknown language stays plain text. Run it on the background
//! executor with the rest of the conversation projection.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Comment,
    Keyword,
    String,
    /// Numbers and constant literals such as `true` or `nil`.
    Number,
    Type,
    Function,
    /// Keys, assigned names, and shell variables.
    Property,
    Punctuation,
    Inserted,
    Deleted,
    /// Hunk headers, attributes, decorators, and document markers.
    Meta,
    Heading,
}

pub type Span = (Range<usize>, Token);

/// Larger inputs stay plain: highlighting must never delay a long reply.
const LIMIT: usize = 256 * 1024;

/// Style ranges for `source`, sorted, non-overlapping, and on character
/// boundaries. `label` is a fence label or a name from [`language_for_path`].
pub fn highlight(label: &str, source: &str) -> Vec<Span> {
    let mut out = Spans::default();
    if source.len() > LIMIT {
        return out.0;
    }
    match language(label) {
        Some(Language::Code(syntax)) => code(syntax, source, &mut out),
        Some(Language::Yaml) => yaml(source, &mut out),
        Some(Language::Markdown) => markdown(source, &mut out),
        Some(Language::Diff) => diff(source, &mut out),
        None => {}
    }
    out.0
}

/// The fence label for a file, from its extension; empty when unknown.
pub fn language_for_path(path: &str) -> &'static str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let extension = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        // Dotfiles such as `.zshrc` have no extension to go by.
        _ => {
            return match name {
                ".zshrc" | ".bashrc" | ".profile" | ".zprofile" | ".bash_profile" => "sh",
                _ => "",
            };
        }
    };
    match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => "typescript",
        "py" | "pyi" => "python",
        "sh" | "bash" | "zsh" => "sh",
        "json" | "jsonc" | "jsonl" => "json",
        "toml" => "toml",
        "lock" if name == "Cargo.lock" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "diff" | "patch" => "diff",
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "m" | "mm" => "cpp",
        "go" => "go",
        _ => "",
    }
}

enum Language {
    Code(&'static Syntax),
    Yaml,
    Markdown,
    Diff,
}

fn language(label: &str) -> Option<Language> {
    // Labels such as `rust,no_run` or `{.python}` name the language first.
    let name: String = label
        .trim_start_matches(['{', '.'])
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '#' | '-' | '_'))
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    Some(match name.as_str() {
        "rust" | "rs" => Language::Code(&RUST),
        "typescript" | "ts" | "tsx" | "javascript" | "js" | "jsx" | "mjs" | "cjs" => {
            Language::Code(&SCRIPT)
        }
        "python" | "py" | "python3" => Language::Code(&PYTHON),
        "sh" | "bash" | "zsh" | "shell" | "console" | "shellsession" => Language::Code(&SHELL),
        "json" | "jsonc" | "jsonl" | "json5" => Language::Code(&JSON),
        "toml" => Language::Code(&TOML),
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" | "objc" | "objective-c" => {
            Language::Code(&C)
        }
        "go" | "golang" => Language::Code(&GO),
        "yaml" | "yml" => Language::Yaml,
        "markdown" | "md" => Language::Markdown,
        "diff" | "patch" => Language::Diff,
        _ => return None,
    })
}

#[derive(Default)]
struct Spans(Vec<Span>);

impl Spans {
    fn push(&mut self, range: Range<usize>, token: Token) {
        if range.is_empty() {
            return;
        }
        if let Some((last, kind)) = self.0.last_mut()
            && *kind == token
            && last.end == range.start
        {
            last.end = range.end;
        } else {
            self.0.push((range, token));
        }
    }
}

/// Lexical rules shared by the brace-and-keyword languages and the simple
/// key/value formats.
struct Syntax {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    nested_comments: bool,
    quotes: &'static [u8],
    /// `"""` and `'''` strings, and string prefixes such as `f"…"`.
    triple_quotes: bool,
    /// Quoted strings may continue past a newline.
    multiline_strings: bool,
    /// `'x'` is one character rather than a string delimiter.
    char_literals: bool,
    /// Rust: `'a` lifetimes, `r#"…"#` strings, `#[…]` attributes, `name!` macros.
    rust: bool,
    /// Shell: command words, `$VAR`, word-start `#`, and literal single quotes.
    shell: bool,
    /// C: `#include` and friends at the start of a line.
    preprocessor: bool,
    /// `@name` before a declaration.
    decorators: bool,
    /// Identifiers may start with or contain `$`.
    dollar_identifiers: bool,
    /// Names may contain `-`, and a name before `=` is a key.
    keys: bool,
    /// A quoted string before `:` is a key.
    string_keys: bool,
    /// `[section]` at the start of a line.
    sections: bool,
    /// `Name` reads as a type and `NAME` as a constant.
    capital_types: bool,
    keywords: &'static [&'static str],
    /// Keywords after which the next name is being defined.
    definers: &'static [&'static str],
    constants: &'static [&'static str],
    types: &'static [&'static str],
}

const PLAIN: Syntax = Syntax {
    line_comments: &[],
    block_comment: None,
    nested_comments: false,
    quotes: b"\"",
    triple_quotes: false,
    multiline_strings: false,
    char_literals: false,
    rust: false,
    shell: false,
    preprocessor: false,
    decorators: false,
    dollar_identifiers: false,
    keys: false,
    string_keys: false,
    sections: false,
    capital_types: false,
    keywords: &[],
    definers: &[],
    constants: &[],
    types: &[],
};

static RUST: Syntax = Syntax {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    nested_comments: true,
    char_literals: true,
    rust: true,
    capital_types: true,
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
        "pub", "ref", "return", "self", "static", "struct", "super", "trait", "type", "union",
        "unsafe", "use", "where", "while",
    ],
    definers: &["fn"],
    constants: &["true", "false"],
    types: &[
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
        "u16", "u32", "u64", "u128", "usize",
    ],
    ..PLAIN
};

static SCRIPT: Syntax = Syntax {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    quotes: b"\"'`",
    decorators: true,
    dollar_identifiers: true,
    capital_types: true,
    keywords: &[
        "abstract",
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "finally",
        "for",
        "from",
        "function",
        "get",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "is",
        "keyof",
        "let",
        "namespace",
        "new",
        "of",
        "override",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "satisfies",
        "set",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "try",
        "type",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    definers: &["function"],
    constants: &["true", "false", "null", "undefined", "NaN", "Infinity"],
    types: &[
        "any", "bigint", "boolean", "never", "number", "object", "string", "symbol", "unknown",
    ],
    ..PLAIN
};

static PYTHON: Syntax = Syntax {
    line_comments: &["#"],
    quotes: b"\"'",
    triple_quotes: true,
    decorators: true,
    capital_types: true,
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "case", "class", "continue", "def",
        "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in",
        "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise", "return", "try",
        "while", "with", "yield",
    ],
    definers: &["def"],
    constants: &["True", "False", "None"],
    types: &[
        "bool", "bytes", "dict", "float", "int", "list", "object", "set", "str", "tuple",
    ],
    ..PLAIN
};

static SHELL: Syntax = Syntax {
    line_comments: &["#"],
    quotes: b"\"'",
    multiline_strings: true,
    shell: true,
    keywords: &[
        "break", "case", "continue", "declare", "do", "done", "elif", "else", "esac", "eval",
        "exec", "exit", "export", "fi", "for", "function", "if", "in", "local", "readonly",
        "return", "select", "set", "shift", "source", "then", "time", "trap", "unset", "until",
        "while",
    ],
    ..PLAIN
};

static JSON: Syntax = Syntax {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    string_keys: true,
    constants: &["true", "false", "null"],
    ..PLAIN
};

static TOML: Syntax = Syntax {
    line_comments: &["#"],
    quotes: b"\"'",
    triple_quotes: true,
    keys: true,
    sections: true,
    constants: &["true", "false", "inf", "nan"],
    ..PLAIN
};

static C: Syntax = Syntax {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    char_literals: true,
    preprocessor: true,
    capital_types: true,
    keywords: &[
        "alignas",
        "alignof",
        "auto",
        "break",
        "case",
        "catch",
        "class",
        "co_await",
        "co_return",
        "co_yield",
        "concept",
        "const",
        "const_cast",
        "consteval",
        "constexpr",
        "constinit",
        "continue",
        "decltype",
        "default",
        "delete",
        "do",
        "dynamic_cast",
        "else",
        "enum",
        "explicit",
        "export",
        "extern",
        "final",
        "for",
        "friend",
        "goto",
        "if",
        "inline",
        "mutable",
        "namespace",
        "new",
        "noexcept",
        "operator",
        "override",
        "private",
        "protected",
        "public",
        "register",
        "reinterpret_cast",
        "requires",
        "restrict",
        "return",
        "sizeof",
        "static",
        "static_assert",
        "static_cast",
        "struct",
        "switch",
        "template",
        "this",
        "thread_local",
        "throw",
        "try",
        "typedef",
        "typeid",
        "typename",
        "union",
        "using",
        "virtual",
        "volatile",
        "while",
    ],
    constants: &["true", "false", "nullptr", "NULL"],
    types: &[
        "bool", "char", "char8_t", "char16_t", "char32_t", "double", "float", "int", "int8_t",
        "int16_t", "int32_t", "int64_t", "long", "short", "signed", "size_t", "ssize_t", "uint8_t",
        "uint16_t", "uint32_t", "uint64_t", "unsigned", "void", "wchar_t",
    ],
    ..PLAIN
};

static GO: Syntax = Syntax {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    quotes: b"\"`",
    char_literals: true,
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
    ],
    definers: &["func"],
    constants: &["true", "false", "nil", "iota"],
    types: &[
        "any",
        "bool",
        "byte",
        "complex64",
        "complex128",
        "error",
        "float32",
        "float64",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "rune",
        "string",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
    ],
    ..PLAIN
};

fn code(syntax: &Syntax, source: &str, out: &mut Spans) {
    let bytes = source.as_bytes();
    let mut at = 0;
    // Only whitespace so far on this line.
    let mut line_start = true;
    // Shell: the next word names a command.
    let mut command = true;
    // The next name is being defined by `fn`, `def`, or `func`.
    let mut defining = false;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'\n' {
            (line_start, command) = (true, true);
            at += 1;
            continue;
        }
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let first = std::mem::take(&mut line_start);
        if let Some(end) = comment_end(syntax, bytes, at) {
            out.push(at..end, Token::Comment);
            at = end;
            continue;
        }
        if first && let Some(end) = line_prefix(syntax, source, at, out) {
            at = end;
            continue;
        }
        if let Some(end) = string_end(syntax, source, at) {
            let key = syntax.string_keys && next_visible(bytes, end) == Some(b':');
            out.push(at..end, if key { Token::Property } else { Token::String });
            (at, command) = (end, false);
            continue;
        }
        if syntax.shell {
            if let Some(end) = shell_word(syntax, source, at, &mut command, out) {
                at = end;
                continue;
            }
        } else if let Some(end) = special(syntax, source, at, out) {
            at = end;
            continue;
        }
        if byte.is_ascii_digit() {
            let end = number_end(bytes, at);
            out.push(at..end, Token::Number);
            at = end;
            continue;
        }
        if let Some(end) = word_end(syntax, source, at) {
            at = word(syntax, source, at..end, &mut defining, out);
            continue;
        }
        if byte.is_ascii_punctuation() {
            out.push(at..at + 1, Token::Punctuation);
            // Only these leave a command still to come; `[ -f x ]` does not.
            command =
                syntax.shell && matches!(byte, b'|' | b'&' | b';' | b'(' | b'`' | b'!' | b'{');
            at += 1;
        } else {
            // Other symbols stay plain; step over the whole character.
            at += source[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
}

fn next_visible(bytes: &[u8], from: usize) -> Option<u8> {
    bytes[from..]
        .iter()
        .copied()
        .find(|byte| !matches!(byte, b' ' | b'\t'))
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| from + offset)
}

fn comment_end(syntax: &Syntax, bytes: &[u8], at: usize) -> Option<usize> {
    let rest = &bytes[at..];
    for marker in syntax.line_comments {
        // In a shell, `#` opens a comment only at a word start: not in `$#` or `a#b`.
        let word_start =
            at == 0 || matches!(bytes[at - 1], b' ' | b'\t' | b'\n' | b';' | b'(' | b'`');
        if rest.starts_with(marker.as_bytes()) && (!syntax.shell || word_start) {
            return Some(line_end(bytes, at));
        }
    }
    let (open, close) = syntax.block_comment?;
    if !rest.starts_with(open.as_bytes()) {
        return None;
    }
    let (mut depth, mut offset) = (1, open.len());
    while offset < rest.len() {
        if rest[offset..].starts_with(close.as_bytes()) {
            depth -= 1;
            offset += close.len();
            if depth == 0 {
                return Some(at + offset);
            }
        } else if syntax.nested_comments && rest[offset..].starts_with(open.as_bytes()) {
            depth += 1;
            offset += open.len();
        } else {
            offset += 1;
        }
    }
    // An unclosed comment in a partial answer runs to the end.
    Some(bytes.len())
}

/// Constructs that only count at the start of a line.
fn line_prefix(syntax: &Syntax, source: &str, at: usize, out: &mut Spans) -> Option<usize> {
    let bytes = source.as_bytes();
    if syntax.preprocessor && bytes[at] == b'#' {
        let mut end = at + 1;
        while bytes
            .get(end)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            end += 1;
        }
        while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
            end += 1;
        }
        out.push(at..end, Token::Keyword);
        // `#include <header>` names a file, not a comparison.
        if source[at..end].ends_with("include") && next_visible(bytes, end) == Some(b'<') {
            let stop = line_end(bytes, end);
            out.push(end..stop, Token::String);
            return Some(stop);
        }
        return Some(end);
    }
    if syntax.sections && bytes[at] == b'[' {
        let stop = line_end(bytes, at);
        let end = source[at..stop]
            .rfind(']')
            .map_or(stop, |close| at + close + 1);
        out.push(at..end, Token::Type);
        return Some(end);
    }
    None
}

fn string_end(syntax: &Syntax, source: &str, at: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let quote = bytes[at];
    if quote == b'\'' && syntax.char_literals {
        return char_end(source, at);
    }
    if !syntax.quotes.contains(&quote) {
        return None;
    }
    if syntax.triple_quotes && bytes[at..].starts_with(&[quote; 3]) {
        return Some(delimited_end(bytes, at + 3, &[quote; 3], true, true));
    }
    // Backticks and shell single quotes hold their content literally.
    let literal = (quote == b'`' && !syntax.dollar_identifiers) || (quote == b'\'' && syntax.shell);
    let multiline = quote == b'`' || syntax.multiline_strings;
    Some(delimited_end(bytes, at + 1, &[quote], !literal, multiline))
}

/// The end of a string whose content starts at `from`, or where a partial one stops.
fn delimited_end(bytes: &[u8], from: usize, close: &[u8], escapes: bool, multiline: bool) -> usize {
    let mut at = from;
    while at < bytes.len() {
        if escapes && bytes[at] == b'\\' {
            at += 2;
        } else if bytes[at..].starts_with(close) {
            return at + close.len();
        } else if bytes[at] == b'\n' && !multiline {
            return at;
        } else {
            at += 1;
        }
    }
    bytes.len()
}

/// `'x'` or `'\n'`. A lone quote, such as a Rust lifetime, is not a string.
fn char_end(source: &str, at: usize) -> Option<usize> {
    let rest = &source[at + 1..];
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if first == '\\' {
        let stop = rest.find('\n').unwrap_or(rest.len()).min(12);
        let close = rest.as_bytes()[..stop]
            .iter()
            .skip(2)
            .position(|byte| *byte == b'\'')?;
        return Some(at + 1 + 2 + close + 1);
    }
    let (next, second) = chars.next()?;
    (first != '\'' && second == '\'').then_some(at + 1 + next + 1)
}

/// Constructs introduced by punctuation in the non-shell languages.
fn special(syntax: &Syntax, source: &str, at: usize, out: &mut Spans) -> Option<usize> {
    let bytes = source.as_bytes();
    let name_end = |from: usize| {
        let length: usize = source[from..]
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .map(char::len_utf8)
            .sum();
        from + length
    };
    match bytes[at] {
        b'\'' if syntax.rust => {
            let end = name_end(at + 1);
            (end > at + 1).then(|| {
                out.push(at..end, Token::Type);
                end
            })
        }
        b'#' if syntax.rust => {
            let open = at + 1 + usize::from(bytes.get(at + 1) == Some(&b'!'));
            (bytes.get(open) == Some(&b'[')).then(|| {
                let end = bracket_end(bytes, open);
                out.push(at..end, Token::Meta);
                end
            })
        }
        b'@' if syntax.decorators => {
            let mut end = name_end(at + 1);
            while bytes.get(end) == Some(&b'.') && name_end(end + 1) > end + 1 {
                end = name_end(end + 1);
            }
            (end > at + 1).then(|| {
                out.push(at..end, Token::Meta);
                end
            })
        }
        _ => None,
    }
}

/// The end of a `[…]` group that opens at `open`, allowing nesting.
fn bracket_end(bytes: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (offset, byte) in bytes[open..].iter().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset + 1;
                }
            }
            _ => {}
        }
    }
    bytes.len()
}

fn number_end(bytes: &[u8], at: usize) -> usize {
    let mut end = at;
    while let Some(&byte) = bytes.get(end) {
        let digit_follows = bytes.get(end + 1).is_some_and(u8::is_ascii_digit);
        let exponent = matches!(bytes[end - usize::from(end > at)], b'e' | b'E')
            && !bytes[at..end].starts_with(b"0x");
        let part = byte.is_ascii_alphanumeric()
            || byte == b'_'
            // `1.5`, but not the range `1..5` or the call `1.max(2)`.
            || byte == b'.' && digit_follows
            || matches!(byte, b'+' | b'-') && exponent && digit_follows;
        if !part {
            break;
        }
        end += 1;
    }
    end
}

fn word_end(syntax: &Syntax, source: &str, at: usize) -> Option<usize> {
    let starts =
        |ch: char| ch.is_alphabetic() || ch == '_' || syntax.dollar_identifiers && ch == '$';
    let continues = |ch: char| starts(ch) || ch.is_alphanumeric() || syntax.keys && ch == '-';
    let mut chars = source[at..].chars();
    let first = chars.next().filter(|ch| starts(*ch))?;
    let rest: usize = chars
        .take_while(|ch| continues(*ch))
        .map(char::len_utf8)
        .sum();
    Some(at + first.len_utf8() + rest)
}

/// Classify one identifier and return where lexing continues.
fn word(
    syntax: &Syntax,
    source: &str,
    range: Range<usize>,
    defining: &mut bool,
    out: &mut Spans,
) -> usize {
    let bytes = source.as_bytes();
    let text = &source[range.clone()];
    let next = bytes.get(range.end).copied();
    let defined = std::mem::take(defining);
    if let Some(end) = prefixed_string(syntax, source, &range) {
        out.push(range.start..end, Token::String);
        return end;
    }
    if syntax.rust
        && next == Some(b'!')
        && matches!(bytes.get(range.end + 1), Some(b'(' | b'[' | b'{'))
    {
        out.push(range.start..range.end + 1, Token::Function);
        return range.end + 1;
    }
    let capital = text.starts_with(|ch: char| ch.is_ascii_uppercase());
    let shouting = text.len() > 1
        && !text
            .bytes()
            .any(|byte| byte.is_ascii_lowercase() || byte >= 0x80);
    // A shell keyword counts only where a command may start, apart from `in`.
    let token = if syntax.keywords.contains(&text) && (!syntax.shell || text == "in") {
        *defining = syntax.definers.contains(&text);
        Some(Token::Keyword)
    } else if syntax.constants.contains(&text) {
        Some(Token::Number)
    } else if syntax.keys && next_visible(bytes, range.end) == Some(b'=') {
        Some(Token::Property)
    } else if syntax.types.contains(&text) {
        Some(Token::Type)
    } else if defined || (next == Some(b'(') && !(syntax.capital_types && capital)) {
        Some(Token::Function)
    } else if syntax.capital_types && capital {
        Some(if shouting { Token::Number } else { Token::Type })
    } else {
        None
    };
    if let Some(token) = token {
        out.push(range.clone(), token);
    }
    range.end
}

/// Rust `r#"…"#` and `b"…"`, and Python `f"…"`, start with what looks like a name.
fn prefixed_string(syntax: &Syntax, source: &str, range: &Range<usize>) -> Option<usize> {
    let bytes = source.as_bytes();
    let text = &source[range.clone()];
    if syntax.rust && matches!(text, "r" | "b" | "br" | "c" | "cr") {
        let hashes = bytes[range.end..]
            .iter()
            .take_while(|byte| **byte == b'#')
            .count();
        let open = range.end + hashes;
        if bytes.get(open) != Some(&b'"') || hashes > 0 && !text.ends_with('r') {
            return None;
        }
        let mut close = vec![b'"'];
        close.resize(1 + hashes, b'#');
        return Some(delimited_end(
            bytes,
            open + 1,
            &close,
            !text.ends_with('r'),
            true,
        ));
    }
    let prefix = syntax.triple_quotes
        && text.len() <= 2
        && text.bytes().all(|byte| b"rbfuRBFU".contains(&byte));
    let quoted = bytes
        .get(range.end)
        .is_some_and(|byte| syntax.quotes.contains(byte));
    (prefix && quoted)
        .then(|| string_end(syntax, source, range.end))
        .flatten()
}

/// Shell words: variables, assignments, and the command at the start of a pipeline.
fn shell_word(
    syntax: &Syntax,
    source: &str,
    at: usize,
    command: &mut bool,
    out: &mut Spans,
) -> Option<usize> {
    let bytes = source.as_bytes();
    let name_end = |from: usize| {
        from + bytes[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count()
    };
    if bytes[at] == b'$' {
        let end = match bytes.get(at + 1) {
            Some(b'{') => delimited_end(bytes, at + 2, b"}", false, false),
            Some(b'(') => {
                out.push(at..at + 2, Token::Punctuation);
                *command = true;
                return Some(at + 2);
            }
            Some(byte) if byte.is_ascii_alphanumeric() || *byte == b'_' => name_end(at + 1),
            Some(b'@' | b'*' | b'#' | b'?' | b'$' | b'!' | b'-') => at + 2,
            // A `$ ` prompt in a console transcript.
            _ => {
                out.push(at..at + 1, Token::Punctuation);
                return Some(at + 1);
            }
        };
        out.push(at..end, Token::Property);
        *command = false;
        return Some(end);
    }
    if !*command {
        return None;
    }
    let end = at
        + bytes[at..]
            .iter()
            .take_while(|byte| {
                !byte.is_ascii_whitespace()
                    && !matches!(
                        byte,
                        b'|' | b'&'
                            | b';'
                            | b'('
                            | b')'
                            | b'<'
                            | b'>'
                            | b'"'
                            | b'\''
                            | b'`'
                            | b'$'
                            | b'='
                            | b'#'
                    )
            })
            .count();
    if end == at {
        return None;
    }
    let text = &source[at..end];
    if bytes.get(end) == Some(&b'=') && name_end(at) == end {
        // `NAME=value command` leaves the command still to come.
        out.push(at..end, Token::Property);
        return Some(shell_value_end(bytes, end + 1));
    } else if syntax.keywords.contains(&text) {
        out.push(at..end, Token::Keyword);
        *command = matches!(
            text,
            "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "time" | "exec" | "eval"
        );
    } else if text
        .starts_with(|ch: char| ch.is_alphanumeric() || matches!(ch, '.' | '/' | '_' | '~'))
    {
        out.push(at..end, Token::Function);
        *command = false;
    } else {
        return None;
    }
    Some(end)
}

/// The end of an assigned value: one word, which may contain quoted parts.
fn shell_value_end(bytes: &[u8], from: usize) -> usize {
    let mut at = from;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' | b'\'' => at = delimited_end(bytes, at + 1, &[byte], byte == b'"', true),
            _ if byte.is_ascii_whitespace() || matches!(byte, b';' | b'|' | b'&' | b')') => break,
            _ => at += 1,
        }
    }
    at.min(bytes.len())
}

/// Lines with their byte offsets, without the line terminator.
fn lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = 0;
    source.split_inclusive('\n').map(move |line| {
        let at = start;
        start += line.len();
        (at, line.trim_end_matches(['\n', '\r']))
    })
}

fn diff(source: &str, out: &mut Spans) {
    for (at, line) in lines(source) {
        let token = if line.starts_with("@@") {
            Token::Meta
        } else if ["+++ ", "--- ", "diff ", "index "]
            .iter()
            .any(|header| line.starts_with(header))
        {
            Token::Comment
        } else if line.starts_with('+') {
            Token::Inserted
        } else if line.starts_with('-') {
            Token::Deleted
        } else {
            continue;
        };
        out.push(at..at + line.len(), token);
    }
}

fn markdown(source: &str, out: &mut Spans) {
    let mut fence: Option<&str> = None;
    for (start, line) in lines(source) {
        let text = line.trim_start();
        let at = start + line.len() - text.len();
        if let Some(marker) = fence {
            if text.starts_with(marker) {
                fence = None;
                out.push(at..at + text.len(), Token::Punctuation);
            }
            continue;
        }
        if text.starts_with("```") || text.starts_with("~~~") {
            fence = Some(&text[..3]);
            out.push(at..at + text.len(), Token::Punctuation);
            continue;
        }
        let hashes = text.bytes().take_while(|byte| *byte == b'#').count();
        if (1..=6).contains(&hashes) && matches!(text.as_bytes().get(hashes), None | Some(b' ')) {
            out.push(at..at + text.len(), Token::Heading);
        } else if text.starts_with('>') {
            out.push(at..at + text.len(), Token::Comment);
        } else {
            let marker = list_marker(text);
            out.push(at..at + marker, Token::Punctuation);
            markdown_inline(&text[marker..], at + marker, out);
        }
    }
}

/// Bytes of a leading `- `, `* `, `+ `, or `12. ` marker.
fn list_marker(text: &str) -> usize {
    let bytes = text.as_bytes();
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    match bytes {
        [b'-' | b'*' | b'+', b' ', ..] => 1,
        _ if digits > 0
            && matches!(bytes.get(digits), Some(b'.' | b')'))
            && bytes.get(digits + 1) == Some(&b' ') =>
        {
            digits + 1
        }
        _ => 0,
    }
}

fn markdown_inline(text: &str, at: usize, out: &mut Spans) {
    let bytes = text.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'`' {
            let end = delimited_end(bytes, offset + 1, b"`", false, false);
            out.push(at + offset..at + end, Token::String);
            offset = end;
        } else if bytes[offset..].starts_with(b"](") {
            let end = delimited_end(bytes, offset + 2, b")", false, false);
            let closed = end > offset + 2 && bytes[end - 1] == b')';
            out.push(
                at + offset + 2..at + end - usize::from(closed),
                Token::Property,
            );
            offset = end;
        } else {
            offset += 1;
        }
    }
}

fn yaml(source: &str, out: &mut Spans) {
    for (start, line) in lines(source) {
        let mut text = line.trim_start();
        let mut at = start + line.len() - text.len();
        if text.starts_with('#') {
            out.push(at..at + text.len(), Token::Comment);
            continue;
        }
        if matches!(text, "---" | "...") {
            out.push(at..at + 3, Token::Meta);
            continue;
        }
        while text == "-" || text.starts_with("- ") {
            out.push(at..at + 1, Token::Punctuation);
            let rest = text[1..].trim_start();
            at += text.len() - rest.len();
            text = rest;
        }
        if let Some(colon) = yaml_key_end(text) {
            out.push(at..at + colon, Token::Property);
            out.push(at + colon..at + colon + 1, Token::Punctuation);
            let rest = text[colon + 1..].trim_start();
            at += text.len() - rest.len();
            text = rest;
        }
        yaml_value(text, at, out);
    }
}

/// The byte offset of the `:` that ends a mapping key, if the line starts with one.
fn yaml_key_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let from = match bytes.first()? {
        quote @ (b'"' | b'\'') => delimited_end(bytes, 1, &[*quote], *quote == b'"', false),
        b'{' | b'[' | b'#' | b'&' | b'*' | b'!' | b'|' | b'>' => return None,
        _ => 0,
    };
    let colon = from + bytes[from..].iter().position(|byte| *byte == b':')?;
    let ends_key = matches!(bytes.get(colon + 1), None | Some(b' ' | b'\t'));
    let commented = text[..colon].contains(" #");
    (ends_key && !commented && (from == 0 || colon == from)).then_some(colon)
}

fn yaml_value(text: &str, at: usize, out: &mut Spans) {
    let bytes = text.as_bytes();
    let Some(&first) = bytes.first() else {
        return;
    };
    let end = if matches!(first, b'"' | b'\'') {
        let end = delimited_end(bytes, 1, &[first], first == b'"', false);
        out.push(at..at + end, Token::String);
        end
    } else {
        let end = text.find(" #").unwrap_or(text.len());
        let scalar = text[..end].trim_end();
        let lower = scalar.to_ascii_lowercase();
        let token = if matches!(
            lower.as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
        ) || scalar.parse::<f64>().is_ok()
        {
            Some(Token::Number)
        } else if matches!(first, b'&' | b'*' | b'!') {
            Some(Token::Meta)
        } else if matches!(first, b'|' | b'>') && scalar.len() <= 3 {
            Some(Token::Punctuation)
        } else {
            None
        };
        if let Some(token) = token {
            out.push(at..at + scalar.len(), token);
        }
        end
    };
    let rest = &text[end..];
    let comment = rest.trim_start();
    if comment.starts_with('#') {
        let from = at + end + rest.len() - comment.len();
        out.push(from..from + comment.len(), Token::Comment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of every span with the given token.
    fn tokens<'a>(label: &str, source: &'a str, token: Token) -> Vec<&'a str> {
        highlight(label, source)
            .into_iter()
            .filter(|(_, kind)| *kind == token)
            .map(|(range, _)| &source[range])
            .collect()
    }

    fn assert_well_formed(label: &str, source: &str) {
        let spans = highlight(label, source);
        let mut end = 0;
        for (range, _) in &spans {
            assert!(range.start >= end, "{label}: overlapping {range:?}");
            assert!(range.start < range.end, "{label}: empty {range:?}");
            assert!(source.is_char_boundary(range.start), "{label}: {range:?}");
            assert!(source.is_char_boundary(range.end), "{label}: {range:?}");
            end = range.end;
        }
        assert!(end <= source.len());
    }

    #[test]
    fn rust_separates_keywords_types_strings_and_comments() {
        let source = "// note\n#[derive(Debug)]\npub fn parse<'a>(input: &'a str) -> Option<u32> {\n    let raw = r#\"a \"quoted\" b\"#;\n    println!(\"{}\", MAX_LEN + 0x1f + 1.5e-3);\n    let c = 'x'; /* outer /* inner */ still */\n    None\n}";
        assert_eq!(
            tokens("rust", source, Token::Keyword),
            ["pub", "fn", "let", "let"]
        );
        assert_eq!(
            tokens("rust", source, Token::Comment),
            ["// note", "/* outer /* inner */ still */"]
        );
        assert_eq!(
            tokens("rust", source, Token::String),
            ["r#\"a \"quoted\" b\"#", "\"{}\"", "'x'"]
        );
        assert_eq!(
            tokens("rust", source, Token::Function),
            ["parse", "println!"]
        );
        assert_eq!(
            tokens("rust", source, Token::Type),
            ["'a", "'a", "str", "Option", "u32", "None"]
        );
        assert_eq!(
            tokens("rust", source, Token::Number),
            ["MAX_LEN", "0x1f", "1.5e-3"]
        );
        assert_eq!(tokens("rust", source, Token::Meta), ["#[derive(Debug)]"]);
    }

    #[test]
    fn script_python_go_and_c_use_their_own_rules() {
        let script = "import { a } from './a';\nconst s = `x ${y}`; // done\n@Injectable()\nexport class Foo { run(n: number) { return null; } }";
        assert_eq!(tokens("ts", script, Token::String), ["'./a'", "`x ${y}`"]);
        assert_eq!(tokens("tsx", script, Token::Meta), ["@Injectable"]);
        assert_eq!(tokens("js", script, Token::Type), ["Foo", "number"]);
        assert_eq!(tokens("javascript", script, Token::Number), ["null"]);

        let python = "def load(path):\n    \"\"\"Doc\nstring\"\"\"\n    name = f'{path}'  # why\n    return None";
        assert_eq!(tokens("python", python, Token::Function), ["load"]);
        assert_eq!(
            tokens("py", python, Token::String),
            ["\"\"\"Doc\nstring\"\"\"", "f'{path}'"]
        );
        assert_eq!(tokens("py", python, Token::Comment), ["# why"]);

        let go = "func Sum(xs []int) int {\n\tfmt.Println(`raw \\n`, 'x', nil)\n}";
        assert_eq!(tokens("go", go, Token::Function), ["Sum", "Println"]);
        assert_eq!(tokens("go", go, Token::String), ["`raw \\n`", "'x'"]);
        assert_eq!(tokens("golang", go, Token::Type), ["int", "int"]);

        let c = "#include <stdio.h>\nint main(void) { size_t n = sizeof(int); return 0; }";
        assert_eq!(
            tokens("c", c, Token::Keyword),
            ["#include", "sizeof", "return"]
        );
        assert_eq!(tokens("cpp", c, Token::String), [" <stdio.h>"]);
        assert_eq!(tokens("c++", c, Token::Function), ["main"]);
    }

    #[test]
    fn shell_marks_commands_variables_and_word_start_comments() {
        let source = "FOO=1 cargo test --locked | tee \"$HOME/log\" # run\necho ${#items} $1; if true; then ./run.sh; fi";
        assert_eq!(
            tokens("sh", source, Token::Function),
            ["cargo", "tee", "echo", "true", "./run.sh"]
        );
        assert_eq!(
            tokens("bash", source, Token::Property),
            ["FOO", "${#items}", "$1"]
        );
        assert_eq!(tokens("zsh", source, Token::Comment), ["# run"]);
        assert_eq!(
            tokens("shell", source, Token::Keyword),
            ["if", "then", "fi"]
        );
        assert_eq!(tokens("console", "$ ls -la", Token::Function), ["ls"]);
    }

    #[test]
    fn data_formats_distinguish_keys_from_values() {
        let json = "{\"name\": \"plait\", \"count\": 3, \"ok\": true}";
        assert_eq!(
            tokens("json", json, Token::Property),
            ["\"name\"", "\"count\"", "\"ok\""]
        );
        assert_eq!(tokens("json", json, Token::String), ["\"plait\""]);
        assert_eq!(tokens("json", json, Token::Number), ["3", "true"]);

        let toml = "# pinned\n[workspace.package]\nrust-version = \"1.98\"\nlto = true";
        assert_eq!(tokens("toml", toml, Token::Type), ["[workspace.package]"]);
        assert_eq!(
            tokens("toml", toml, Token::Property),
            ["rust-version", "lto"]
        );
        assert_eq!(tokens("toml", toml, Token::Comment), ["# pinned"]);

        let yaml = "---\nname: build # ci\nsteps:\n  - run: \"cargo test\"\n    retries: 2\nurl: http://example.com";
        assert_eq!(
            tokens("yaml", yaml, Token::Property),
            ["name", "steps", "run", "retries", "url"]
        );
        assert_eq!(tokens("yml", yaml, Token::String), ["\"cargo test\""]);
        assert_eq!(tokens("yaml", yaml, Token::Number), ["2"]);
        assert_eq!(tokens("yaml", yaml, Token::Comment), ["# ci"]);
    }

    #[test]
    fn markdown_and_diff_are_line_oriented() {
        let markdown = "# Title\n- item with `code`\n```\n# not a heading\n```\n> quote";
        assert_eq!(tokens("md", markdown, Token::Heading), ["# Title"]);
        assert_eq!(tokens("markdown", markdown, Token::String), ["`code`"]);
        assert_eq!(tokens("md", markdown, Token::Comment), ["> quote"]);

        let diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n same";
        assert_eq!(tokens("diff", diff, Token::Deleted), ["-old"]);
        assert_eq!(tokens("patch", diff, Token::Inserted), ["+new"]);
        assert_eq!(tokens("diff", diff, Token::Meta), ["@@ -1 +1 @@"]);
    }

    #[test]
    fn unknown_languages_and_oversized_input_stay_plain() {
        assert!(highlight("", "fn main() {}").is_empty());
        assert!(highlight("brainfuck", "+++").is_empty());
        assert!(highlight("rust", &"a ".repeat(LIMIT)).is_empty());
        assert_eq!(
            tokens("rust,no_run", "fn main() {}", Token::Keyword),
            ["fn"]
        );
    }

    #[test]
    fn partial_and_unicode_input_yields_ordered_boundary_spans() {
        let samples = [
            "let s = \"unterminated 🙂",
            "/* never closed 🙂",
            "'\\",
            "r###\"raw",
            "x = '🙂' + \"\\🙂\" # é",
            "名前 = \"値\"\nfn 関数() {}",
            "\"key\\",
            "$",
            "${unclosed",
            "- - key: 'v\n",
            "```\nopen fence",
            "[link](",
            "[link](héé",
            "A=\"é b\" ünï",
            "1e+",
            "#[unclosed",
            "@",
        ];
        for label in [
            "rust", "ts", "python", "sh", "json", "toml", "yaml", "md", "diff", "cpp", "go",
        ] {
            for sample in samples {
                assert_well_formed(label, sample);
                // Every prefix is a possible streaming state.
                for (end, _) in sample.char_indices() {
                    assert_well_formed(label, &sample[..end]);
                }
            }
        }
    }

    #[test]
    fn paths_map_to_languages() {
        assert_eq!(language_for_path("src/main.rs"), "rust");
        assert_eq!(language_for_path("a/b/App.TSX"), "typescript");
        assert_eq!(language_for_path("Cargo.lock"), "toml");
        assert_eq!(language_for_path(".zshrc"), "sh");
        assert_eq!(language_for_path("Makefile"), "");
        assert_eq!(language_for_path("notes.txt"), "");
    }
}
