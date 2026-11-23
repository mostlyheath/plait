//! Cold editors contain no GPUI handles, decoded pixels or completion tasks.
use super::{Composer, draft::Draft};
use gpui::*;
use plait_client::tabs::bookmark::DraftBookmark;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::ui) struct ParkedEditor {
    draft: Draft,
    scroll: f32,
}

impl ParkedEditor {
    pub(in crate::ui) fn retained_bytes(&self) -> usize {
        self.draft.retained_bytes()
    }
}

impl Composer {
    pub(in crate::ui) fn park_editor(&mut self) -> ParkedEditor {
        self.draft.finish_composition();
        self.close_popup();
        self.rank_task = None;
        ParkedEditor {
            draft: self.draft.clone(),
            scroll: self.scroll.into(),
        }
    }

    pub(in crate::ui) fn restore_editor(
        &mut self,
        mut editor: ParkedEditor,
        cx: &mut Context<Self>,
    ) {
        if editor.retained_bytes() > 8 * 1024 * 1024 {
            return;
        }
        // Replay the same acknowledgement transition to keep its undo history.
        if self.draft.revision == editor.draft.revision.saturating_add(1)
            && self.draft.value.text.is_empty()
        {
            let text = editor.draft.value.text.clone();
            let revision = editor.draft.revision;
            editor.draft.clear_revision(&text, revision);
        }
        // A newer durable edit wins over the parked presentation.
        if self.draft.revision == editor.draft.revision {
            self.draft = editor.draft;
            self.draft.normalize_selection();
            self.scroll = px(editor.scroll);
            self.layout.invalidate();
            self.input_changed = true;
            cx.notify();
        }
    }

    pub(in crate::ui) fn draft_bookmark(&self) -> DraftBookmark {
        DraftBookmark {
            revision: self.draft.revision,
            cursor: self.draft.value.head as u32,
            selection_end: self.draft.value.anchor as u32,
        }
    }

    pub(in crate::ui) fn restore_cursor(&mut self, bookmark: &DraftBookmark) {
        if self.draft.revision == bookmark.revision {
            self.draft.value.head = bookmark.cursor as usize;
            self.draft.value.anchor = bookmark.selection_end as usize;
            self.draft.normalize_selection();
            self.reveal_cursor = true;
        }
    }
}

pub(super) mod strings {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;
    pub fn serialize<S: Serializer>(
        value: &Arc<[String]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_ref().serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<[String]>, D::Error> {
        Vec::<String>::deserialize(deserializer).map(Arc::from)
    }
}
