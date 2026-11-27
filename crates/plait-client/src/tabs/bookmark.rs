//! Semantic locations only: never transcript text, pixels, tasks or UI handles.
use crate::thread_view::ThreadView;
use serde::{Deserialize, Serialize};

/// Inline bookmarks leave room for checkout identity in the 16 KiB cold record.
pub const INLINE_BOOKMARK_LIMIT: usize = 8 * 1024;
pub const EXTENDED_BOOKMARK_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadBookmark {
    pub view: ThreadView,
    pub conversation: ConversationBookmark,
    pub changes: ChangesBookmark,
    pub history: HistoryBookmark,
    pub files: FileBookmark,
    pub draft: DraftBookmark,
    pub focus: FocusTarget,
    pub panels: PanelBookmark,
    pub details: ViewDetails,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationBookmark {
    pub scroll: ConversationScroll,
    /// The host exposes backward cursors, not arbitrary message-ID lookup.
    pub before: Option<String>,
    pub pages: u16,
    pub find: FindBookmark,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationScroll {
    #[default]
    Latest,
    Anchored {
        row: String,
        offset_millipoints: i32,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FindBookmark {
    pub open: bool,
    pub query: String,
    pub current: Option<TextPosition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPosition {
    /// Stable message/tool ID, or a file content fingerprint.
    pub source: String,
    pub block: u32,
    pub byte: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangesBookmark {
    /// Checkout-relative path; never a status-list index.
    pub path: Option<String>,
    pub staged: bool,
    pub scope: ChangeScope,
    pub anchor: Option<PatchAnchor>,
    pub selection: Option<(u32, u32)>,
    pub list_offset_millipoints: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChangeScope {
    #[default]
    Current,
    Recorded {
        record: String,
        original: String,
        compare_current: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchAnchor {
    /// Reject an obsolete line selection when the patch content changes.
    pub fingerprint: String,
    pub hunk: u32,
    pub line: u32,
    pub offset_millipoints: i32,
    #[serde(default)]
    pub horizontal_millipoints: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryBookmark {
    pub commit: Option<String>,
    pub anchor: Option<PatchAnchor>,
    pub list_offset_millipoints: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileBookmark {
    pub path: Option<String>,
    pub line: u32,
    pub column: u32,
    pub offset_millipoints: i32,
    pub horizontal_millipoints: i32,
    pub find: FindBookmark,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DraftBookmark {
    /// Text/images and exact pending requests remain in the unchanged journal.
    pub revision: u64,
    pub cursor: u32,
    pub selection_end: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusTarget {
    #[default]
    Composer,
    Conversation,
    ConversationFind,
    Changes,
    History,
    Files,
    FileFind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelBookmark {
    pub changed_files_collapsed: bool,
    pub context_open: bool,
    pub approval_open: bool,
    pub enlarged_image: Option<String>,
    pub undo_conflict_record: Option<String>,
    pub selected_turn: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewDetails {
    pub expanded: Vec<String>,
    pub folded: Vec<String>,
    pub selection: Option<(TextPosition, TextPosition)>,
    pub commit_message: String,
}

/// Only an opaque name is retained for a large bookmark. The UI must durably
/// save that bounded blob before parking; a tab never opens a path itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum BookmarkRecord {
    Worktrees {
        selected: Option<std::path::PathBuf>,
        size_sort: bool,
    },
    /// The Issues tab's selected issue, by Linear's stable ID.
    Issues {
        selected: Option<String>,
    },
    /// The Graph tab's selected draft node ID.
    Graph {
        selected: Option<String>,
    },
    Inline {
        view: ThreadView,
        json: Box<str>,
    },
    Stored {
        view: ThreadView,
        blob: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookmarkError {
    Invalid,
    NeedsStorage,
    TooLarge,
}

impl Default for BookmarkRecord {
    fn default() -> Self {
        // A fixed default is valid without any fallible serialization.
        Self::Inline {
            view: ThreadView::Conversation,
            json: "{}".into(),
        }
    }
}

impl BookmarkRecord {
    pub fn capture(bookmark: &ThreadBookmark) -> Result<Self, BookmarkError> {
        let bytes = encode_extended(bookmark)?;
        if bytes.len() > INLINE_BOOKMARK_LIMIT {
            return Err(BookmarkError::NeedsStorage);
        }
        let json = String::from_utf8(bytes).map_err(|_| BookmarkError::Invalid)?;
        Ok(Self::Inline {
            view: bookmark.view,
            json: json.into_boxed_str(),
        })
    }

    pub fn stored(view: ThreadView, blob: String) -> Result<Self, BookmarkError> {
        if !blob_id(&blob) {
            return Err(BookmarkError::Invalid);
        }
        Ok(Self::Stored { view, blob })
    }

    pub fn view(&self) -> ThreadView {
        match self {
            Self::Inline { view, .. } | Self::Stored { view, .. } => *view,
            Self::Worktrees { .. } | Self::Issues { .. } | Self::Graph { .. } => {
                ThreadView::Conversation
            }
        }
    }

    /// Call only when activating. A stored bookmark explicitly needs blob I/O.
    pub fn inline(&self) -> Option<ThreadBookmark> {
        match self {
            Self::Inline { view, json } => {
                decode_extended(json.as_bytes()).filter(|b| b.view == *view)
            }
            Self::Stored { .. }
            | Self::Worktrees { .. }
            | Self::Issues { .. }
            | Self::Graph { .. } => None,
        }
    }

    pub(crate) fn valid(&self) -> bool {
        match self {
            Self::Inline { json, .. } => {
                json.len() <= INLINE_BOOKMARK_LIMIT && self.inline().is_some()
            }
            Self::Stored { blob, .. } => blob_id(blob),
            Self::Issues { selected } | Self::Graph { selected } => {
                selected.as_ref().is_none_or(|id| {
                    !id.is_empty()
                        && id.len() <= 128
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                })
            }
            Self::Worktrees { selected, .. } => selected.as_ref().is_none_or(|p| {
                p.is_absolute()
                    && p.as_os_str().len() <= 4096
                    && p.to_str().is_some_and(|s| !s.contains('\0'))
            }),
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Inline { json, .. } => json.len(),
                Self::Stored { blob, .. } => blob.capacity(),
                Self::Worktrees { selected, .. } => selected.as_ref().map_or(0, |p| p.capacity()),
                Self::Issues { selected } | Self::Graph { selected } => {
                    selected.as_ref().map_or(0, String::capacity)
                }
            }
    }
}

pub fn encode_extended(bookmark: &ThreadBookmark) -> Result<Vec<u8>, BookmarkError> {
    if !valid_bookmark(bookmark) {
        return Err(BookmarkError::Invalid);
    }
    let bytes = serde_json::to_vec(bookmark).map_err(|_| BookmarkError::Invalid)?;
    if bytes.len() > EXTENDED_BOOKMARK_LIMIT {
        return Err(BookmarkError::TooLarge);
    }
    Ok(bytes)
}

pub fn decode_extended(bytes: &[u8]) -> Option<ThreadBookmark> {
    if bytes.len() > EXTENDED_BOOKMARK_LIMIT {
        return None;
    }
    serde_json::from_slice(bytes).ok().filter(valid_bookmark)
}

fn valid_bookmark(b: &ThreadBookmark) -> bool {
    let text = |s: &str| s.len() <= 4096 && !s.contains('\0');
    let semantic_id = |s: &str| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
    let position = |p: &TextPosition| semantic_id(&p.source);
    let find = |f: &FindBookmark| text(&f.query) && f.current.as_ref().is_none_or(position);
    let patch = |p: &PatchAnchor| super::identifier(&p.fingerprint);
    let path = |p: &String| relative_path(p);
    let oid = |s: &String| matches!(s.len(), 40 | 64) && s.bytes().all(|c| c.is_ascii_hexdigit());
    let scroll = match &b.conversation.scroll {
        ConversationScroll::Latest => true,
        ConversationScroll::Anchored { row, .. } => semantic_id(row),
    };
    let scope = match &b.changes.scope {
        ChangeScope::Current => true,
        ChangeScope::Recorded {
            record, original, ..
        } => super::identifier(record) && super::identifier(original),
    };
    scroll
        && scope
        && b.conversation.pages <= 64
        && b.conversation.before.as_deref().is_none_or(text)
        && find(&b.conversation.find)
        && find(&b.files.find)
        && b.changes.path.as_ref().is_none_or(path)
        && b.files.path.as_ref().is_none_or(path)
        && b.changes.anchor.as_ref().is_none_or(patch)
        && b.history.anchor.as_ref().is_none_or(patch)
        && b.history.commit.as_ref().is_none_or(oid)
        && [
            &b.panels.enlarged_image,
            &b.panels.undo_conflict_record,
            &b.panels.selected_turn,
        ]
        .iter()
        .all(|id| id.as_deref().is_none_or(super::identifier))
        && b.details.expanded.len() <= 4096
        && b.details.folded.len() <= 4096
        && b.details
            .expanded
            .iter()
            .chain(&b.details.folded)
            .all(|s| semantic_id(s))
        && b.details
            .selection
            .as_ref()
            .is_none_or(|(a, z)| position(a) && position(z))
        && b.details.commit_message.len() <= 256 * 1024
}

fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && !path.starts_with('/')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn blob_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}
