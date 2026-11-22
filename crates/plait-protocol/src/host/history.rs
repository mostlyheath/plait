use super::{MessageRole, ToolStatus};
use serde::Deserialize;

/// One message or tool call, in conversation order, with the snapshot's stable ID.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryItem {
    Message {
        #[serde(default)]
        images: Vec<crate::images::ImageRef>,
        id: String,
        role: MessageRole,
        text: String,
        reasoning: String,
        #[serde(default)]
        more: Vec<More>,
    },
    Tool {
        id: String,
        name: String,
        status: ToolStatus,
        input: String,
        output: String,
        #[serde(default)]
        more: Vec<More>,
    },
}

impl HistoryItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Message { id, .. } | Self::Tool { id, .. } => id,
        }
    }

    /// The fields whose text continues beyond what this item carries.
    pub fn more(&self) -> &[More] {
        match self {
            Self::Message { more, .. } | Self::Tool { more, .. } => more,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryField {
    Text,
    Reasoning,
    Input,
    Output,
}

/// A field too large for its item: the item holds the first `offset` UTF-16 units of `length`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct More {
    pub field: HistoryField,
    #[serde(rename = "ref")]
    pub reference: String,
    pub offset: u64,
    pub length: u64,
}

/// Items oldest first. `before` asks for the page older than this one; `None` is the start.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub before: Option<String>,
}

/// One part of a large field. Offsets and `length` count UTF-16 units.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryText {
    pub text: String,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub length: u64,
}
