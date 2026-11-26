//! GitHub-style heading anchors and checkout-relative Markdown link resolution.
//!
//! Pure and synchronous; never touches the filesystem.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// GitHub's heading anchor rules: lowercase; drop characters that are not
/// letters, digits, spaces, hyphens or underscores (Unicode letters/digits kept);
/// spaces become hyphens; nothing else collapses (two spaces -> two hyphens).
pub fn github_slug(heading: &str) -> String {
    let mut slug = String::with_capacity(heading.len());
    for ch in heading.to_lowercase().chars() {
        if ch == ' ' {
            slug.push('-');
        } else if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch);
        }
    }
    slug
}

/// Assigns unique anchors in document order the way GitHub does: the first
/// "Setup" is `setup`, the next `setup-1`, then `setup-2`; an explicit later
/// heading literally named "Setup 1" must not collide (GitHub gives it
/// `setup-1` only if free; follow GitHub: keep incrementing until unused).
#[derive(Debug, Clone, Default)]
pub struct Anchors {
    seen: HashSet<String>,
    /// Next numeric suffix to try for a repeated base slug, avoiding O(n^2) scans.
    next_suffix: HashMap<String, usize>,
}

impl Anchors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, heading: &str) -> String {
        let base = github_slug(heading);
        if self.seen.insert(base.clone()) {
            return base;
        }
        let suffix = self.next_suffix.entry(base.clone()).or_insert(1);
        loop {
            let candidate = format!("{base}-{suffix}");
            *suffix += 1;
            if self.seen.insert(candidate.clone()) {
                return candidate;
            }
        }
    }

    pub fn contains(&self, anchor: &str) -> bool {
        self.seen.contains(anchor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// http(s) URL, opened in the browser.
    Web(String),
    /// mailto: link.
    Mail(String),
    /// `#anchor` within the same document (anchor already percent-decoded, without '#').
    Anchor(String),
    /// Another file in the checkout: path relative to the checkout root, normalized
    /// (no `.` or `..` components), plus an optional decoded anchor.
    Document {
        path: PathBuf,
        anchor: Option<String>,
    },
    /// A relative or root path that would leave the checkout root.
    Outside,
    /// Any other scheme (file:, javascript:, data:, custom), or an empty/garbage href.
    Unsupported,
}

/// Resolves a Markdown link `href` found in `document` (a path relative to the
/// checkout root). Relative paths resolve against the document's folder; a
/// leading `/` resolves against the checkout root (GitHub's behaviour).
/// Percent-decoding of path and anchor (invalid encodings -> Unsupported).
/// Query strings on document links are dropped. Never touches the filesystem.
pub fn resolve_link(href: &str, document: &Path) -> LinkTarget {
    if href.trim().is_empty() || has_control(href) {
        return LinkTarget::Unsupported;
    }

    if let Some(raw_anchor) = href.strip_prefix('#') {
        if raw_anchor.is_empty() {
            return LinkTarget::Unsupported;
        }
        return match percent_decode(raw_anchor) {
            Some(decoded) if !has_control(&decoded) => LinkTarget::Anchor(decoded),
            _ => LinkTarget::Unsupported,
        };
    }

    if let Some(name) = scheme(href) {
        return classify_scheme(name, href);
    }

    // Protocol-relative URLs (such as `//example.com/path`) are external, not root-relative.
    if href.starts_with("//") {
        return LinkTarget::Unsupported;
    }

    // Split fragment before query so `?` inside a fragment stays part of the anchor.
    let (before_hash, raw_anchor) = match href.split_once('#') {
        Some((before, frag)) => (before, Some(frag)),
        None => (href, None),
    };
    let raw_path = before_hash.split_once('?').map_or(before_hash, |(p, _)| p);
    if raw_path.is_empty() {
        return LinkTarget::Unsupported;
    }

    resolve_document(raw_path, raw_anchor, document)
}

fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

/// Returns the URI scheme (RFC 3986) before `:`, or `None` for relative paths.
fn scheme(href: &str) -> Option<&str> {
    let colon = href.find(':')?;
    let prefix = &href[..colon];
    let mut bytes = prefix.bytes();
    if !bytes.next()?.is_ascii_alphabetic() {
        return None;
    }
    if bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
        Some(prefix)
    } else {
        None
    }
}

fn classify_scheme(name: &str, href: &str) -> LinkTarget {
    if href.chars().any(char::is_whitespace) {
        return LinkTarget::Unsupported;
    }
    if name.eq_ignore_ascii_case("http") || name.eq_ignore_ascii_case("https") {
        let rest = &href[name.len() + 1..];
        let Some(after_slashes) = rest.strip_prefix("//") else {
            return LinkTarget::Unsupported;
        };
        let host = after_slashes.split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() {
            return LinkTarget::Unsupported;
        }
        return LinkTarget::Web(href.to_string());
    }
    if name.eq_ignore_ascii_case("mailto") {
        let rest = &href[name.len() + 1..];
        if rest.is_empty() {
            return LinkTarget::Unsupported;
        }
        return LinkTarget::Mail(href.to_string());
    }
    LinkTarget::Unsupported
}

fn resolve_document(raw_path: &str, raw_anchor: Option<&str>, document: &Path) -> LinkTarget {
    let anchor = match raw_anchor {
        Some(raw) if !raw.is_empty() => {
            let Some(decoded) = percent_decode(raw) else {
                return LinkTarget::Unsupported;
            };
            if has_control(&decoded) {
                return LinkTarget::Unsupported;
            }
            Some(decoded)
        }
        _ => None,
    };

    let (mut stack, rest) = if let Some(rooted) = raw_path.strip_prefix('/') {
        (Vec::new(), rooted)
    } else {
        (base_segments(document), raw_path)
    };

    // Decode every segment before normalizing so a later `..` cannot hide an
    // invalid percent-encoding, and reject encoded `/` inside a single segment.
    let mut segments = Vec::new();
    for raw in rest.split('/') {
        if raw.is_empty() {
            continue;
        }
        let Some(decoded) = percent_decode(raw) else {
            return LinkTarget::Unsupported;
        };
        if decoded.contains('/') || has_control(&decoded) {
            return LinkTarget::Unsupported;
        }
        segments.push(decoded);
    }

    for segment in segments {
        match segment.as_str() {
            "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return LinkTarget::Outside;
                }
            }
            _ => stack.push(segment),
        }
    }

    if stack.is_empty() {
        return LinkTarget::Unsupported;
    }

    let mut path = PathBuf::new();
    for segment in stack {
        path.push(segment);
    }
    LinkTarget::Document { path, anchor }
}

fn base_segments(document: &Path) -> Vec<String> {
    let mut stack = Vec::new();
    let Some(parent) = document.parent() else {
        return stack;
    };
    for component in parent.components() {
        match component {
            Component::Normal(part) => stack.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                stack.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    stack
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-decodes UTF-8 bytes; rejects truncated or non-hex `%` escapes and
/// decoded byte sequences that are not valid UTF-8.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex_value(*bytes.get(i + 1)?)?;
            let lo = hex_value(*bytes.get(i + 2)?)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_drops_punctuation_and_keeps_underscores() {
        assert_eq!(github_slug("Hello, World!"), "hello-world");
        assert_eq!(github_slug("What's new? (v2.0)"), "whats-new-v20");
        assert_eq!(github_slug("foo_bar & baz/qux"), "foo_bar--bazqux");
        assert_eq!(github_slug("!!!"), "");
    }

    #[test]
    fn slug_drops_emoji_and_keeps_surrounding_spaces_as_hyphens() {
        assert_eq!(github_slug("🚀 Launch"), "-launch");
        assert_eq!(github_slug("Launch 🚀"), "launch-");
        assert_eq!(github_slug("Launch 🚀 Now"), "launch--now");
    }

    #[test]
    fn slug_keeps_unicode_letters_and_digits() {
        assert_eq!(github_slug("Résumé Über"), "résumé-über");
        assert_eq!(github_slug("日本語"), "日本語");
        assert_eq!(github_slug("日本語 の 見出し 2"), "日本語-の-見出し-2");
    }

    #[test]
    fn slug_removes_code_span_backticks() {
        assert_eq!(
            github_slug("Using `foo_bar` and `baz-qux`"),
            "using-foo_bar-and-baz-qux"
        );
    }

    #[test]
    fn slug_preserves_repeated_hyphens_and_spaces() {
        assert_eq!(github_slug("foo--bar"), "foo--bar");
        assert_eq!(github_slug("a  b"), "a--b");
        assert_eq!(github_slug("a -- b"), "a----b");
    }

    #[test]
    fn slug_turns_leading_and_trailing_spaces_into_hyphens() {
        assert_eq!(github_slug("  padded  "), "--padded--");
        assert_eq!(github_slug(" leading"), "-leading");
        assert_eq!(github_slug("trailing "), "trailing-");
    }

    #[test]
    fn anchors_deduplicate_repeated_headings() {
        let mut anchors = Anchors::new();
        assert_eq!(anchors.add("Setup"), "setup");
        assert_eq!(anchors.add("Setup"), "setup-1");
        assert_eq!(anchors.add("Setup"), "setup-2");
        assert!(anchors.contains("setup"));
        assert!(anchors.contains("setup-1"));
        assert!(anchors.contains("setup-2"));
        assert!(!anchors.contains("setup-3"));
    }

    #[test]
    fn anchors_avoid_collision_with_explicit_numbered_heading() {
        let mut anchors = Anchors::new();
        assert_eq!(anchors.add("Setup"), "setup");
        assert_eq!(anchors.add("Setup 1"), "setup-1");
        assert_eq!(anchors.add("Setup"), "setup-2");
        assert!(anchors.contains("setup"));
        assert!(anchors.contains("setup-1"));
        assert!(anchors.contains("setup-2"));

        let mut second = Anchors::new();
        assert_eq!(second.add("Setup"), "setup");
        assert_eq!(second.add("Setup"), "setup-1");
        assert_eq!(second.add("Setup 1"), "setup-1-1");
    }

    #[test]
    fn resolve_web_and_mail_links() {
        let doc = Path::new("README.md");
        assert_eq!(
            resolve_link("http://example.com/a", doc),
            LinkTarget::Web("http://example.com/a".into())
        );
        assert_eq!(
            resolve_link("https://example.com/a?b=1#c", doc),
            LinkTarget::Web("https://example.com/a?b=1#c".into())
        );
        assert_eq!(
            resolve_link("HTTPS://Example.COM", doc),
            LinkTarget::Web("HTTPS://Example.COM".into())
        );
        assert_eq!(
            resolve_link("mailto:dev@example.com", doc),
            LinkTarget::Mail("mailto:dev@example.com".into())
        );
        assert_eq!(resolve_link("http://", doc), LinkTarget::Unsupported);
        assert_eq!(
            resolve_link("http:example.com", doc),
            LinkTarget::Unsupported
        );
        assert_eq!(resolve_link("mailto:", doc), LinkTarget::Unsupported);
    }

    #[test]
    fn resolve_same_document_anchors_preserve_case_and_decode() {
        let doc = Path::new("docs/guide.md");
        assert_eq!(
            resolve_link("#Anchor", doc),
            LinkTarget::Anchor("Anchor".into())
        );
        assert_eq!(
            resolve_link("#My%20Anchor%20%C3%9Cber", doc),
            LinkTarget::Anchor("My Anchor Über".into())
        );
        assert_eq!(resolve_link("#", doc), LinkTarget::Unsupported);
        assert_eq!(resolve_link("#%zz", doc), LinkTarget::Unsupported);
    }

    #[test]
    fn resolve_relative_and_root_document_links() {
        let root_doc = Path::new("README.md");
        let nested_doc = Path::new("docs/guides/intro.md");

        assert_eq!(
            resolve_link("other.md", root_doc),
            LinkTarget::Document {
                path: PathBuf::from("other.md"),
                anchor: None,
            }
        );
        assert_eq!(
            resolve_link("other.md", nested_doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/guides/other.md"),
                anchor: None,
            }
        );
        assert_eq!(
            resolve_link("./a/../b.md#x", nested_doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/guides/b.md"),
                anchor: Some("x".into()),
            }
        );
        assert_eq!(
            resolve_link("../x.md", nested_doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/x.md"),
                anchor: None,
            }
        );
        assert_eq!(
            resolve_link("/docs/a.md", nested_doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/a.md"),
                anchor: None,
            }
        );
        assert_eq!(
            resolve_link("/docs/a.md#Top", nested_doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/a.md"),
                anchor: Some("Top".into()),
            }
        );
    }

    #[test]
    fn resolve_escaping_paths_return_outside() {
        assert_eq!(
            resolve_link("../../escape.md", Path::new("docs/guide.md")),
            LinkTarget::Outside
        );
        assert_eq!(
            resolve_link("../escape.md", Path::new("README.md")),
            LinkTarget::Outside
        );
        assert_eq!(
            resolve_link("/../escape.md", Path::new("docs/guide.md")),
            LinkTarget::Outside
        );
        assert_eq!(
            resolve_link("../../docs/guide.md", Path::new("docs/guide.md")),
            LinkTarget::Outside
        );
    }

    #[test]
    fn resolve_percent_decoding_and_invalid_escapes() {
        let doc = Path::new("docs/guide.md");
        assert_eq!(
            resolve_link("my%20dir/my%20file.md#Part%201", doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/my dir/my file.md"),
                anchor: Some("Part 1".into()),
            }
        );
        assert_eq!(resolve_link("bad%zz.md", doc), LinkTarget::Unsupported);
        assert_eq!(resolve_link("bad%2.md", doc), LinkTarget::Unsupported);
        assert_eq!(resolve_link("bad%FF.md", doc), LinkTarget::Unsupported);
        assert_eq!(resolve_link("ok.md#bad%zz", doc), LinkTarget::Unsupported);
        assert_eq!(resolve_link("a/%zz/../b.md", doc), LinkTarget::Unsupported);
        assert_eq!(resolve_link("a%2Fb.md", doc), LinkTarget::Unsupported);
    }

    #[test]
    fn resolve_drops_query_string_on_document_links() {
        let doc = Path::new("docs/guide.md");
        assert_eq!(
            resolve_link("other.md?raw=1&view=plain", doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/other.md"),
                anchor: None,
            }
        );
        assert_eq!(
            resolve_link("other.md?raw=1#Section", doc),
            LinkTarget::Document {
                path: PathBuf::from("docs/other.md"),
                anchor: Some("Section".into()),
            }
        );
    }

    #[test]
    fn resolve_rejects_unsupported_schemes_and_empty_hrefs() {
        let doc = Path::new("README.md");
        for href in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/plain,hello",
            "custom+app:open",
            "//example.com/a",
            "",
            "   ",
            "/",
            "?only=query",
        ] {
            assert_eq!(resolve_link(href, doc), LinkTarget::Unsupported);
        }
    }

    #[test]
    fn resolve_treats_backslashes_as_literal_characters() {
        let doc = Path::new("docs/guide.md");
        assert_eq!(
            resolve_link(r"a\b.md", doc),
            LinkTarget::Document {
                path: PathBuf::from(r"docs/a\b.md"),
                anchor: None,
            }
        );
        assert_eq!(
            resolve_link(r"..\escape.md", Path::new("README.md")),
            LinkTarget::Document {
                path: PathBuf::from(r"..\escape.md"),
                anchor: None,
            }
        );
        assert_eq!(
            resolve_link(r"a\..\b.md", Path::new("README.md")),
            LinkTarget::Document {
                path: PathBuf::from(r"a\..\b.md"),
                anchor: None,
            }
        );
    }
}
