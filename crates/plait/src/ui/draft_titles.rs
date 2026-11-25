//! Bounded presentation titles; the journal continues to own the full draft.
use super::{Plait, session::UNTITLED};
use gpui::*;
use plait_client::tabs::{
    TabTarget,
    row::{TabMetadata, TabState},
};

/// Characters a title keeps.
const KEPT: usize = 120;
/// Characters a title looks at, so pasted blank space is not walked on every edit.
const SCANNED: usize = 4096;

/// The first nonblank line, with each run of whitespace shown as one space,
/// as a sent thread's title shows it.
fn first_line(text: &str) -> String {
    let mut line = String::new();
    let (mut kept, mut gap) = (0, false);
    for ch in text.chars().take(SCANNED) {
        if ch == '\n' && kept > 0 {
            break;
        }
        if ch.is_whitespace() {
            gap = kept > 0;
            continue;
        }
        if kept + usize::from(gap) >= KEPT {
            break;
        }
        if gap {
            line.push(' ');
            kept += 1;
            gap = false;
        }
        line.push(ch);
        kept += 1;
    }
    line
}

fn title(text: &str) -> String {
    let line = first_line(text);
    plait_client::display_text::escape(if line.is_empty() { UNTITLED } else { &line })
}

fn rename(row: &mut TabMetadata, title: &str) {
    // The remainder is the already-derived project, machine and state description.
    row.hover.replace_range(..row.title.len(), title);
    row.title = title.into();
}

impl Plait {
    /// Runs at catalog/tab transitions, never from rendering or composer edits.
    pub(super) fn name_draft_rows(&mut self, rows: &mut [TabMetadata]) {
        let mut titles = std::collections::HashMap::new();
        self.tabs.draft_row = None;
        for (index, (tab, row)) in self.tabs.model.tabs().iter().zip(rows).enumerate() {
            let TabTarget::Draft { id, checkout } = tab.session().target() else {
                continue;
            };
            let saved = self
                .session
                .data
                .as_ref()
                .and_then(|s| s.draft_tab(id, &checkout.root));
            let parked = self.tabs.lifecycle.drafts.get(id);
            let name = self.tabs.draft_titles.get(id).cloned().unwrap_or_else(|| {
                title(
                    parked
                        .map(|d| d.saved.text.as_str())
                        .or_else(|| saved.map(|d| d.draft.text.as_str()))
                        .unwrap_or_default(),
                )
            });
            rename(row, &name);
            titles.insert(id.clone(), name);
            if row.active {
                self.tabs.draft_row = Some((id.clone(), index));
            }
            let draft = if self.draft.id.as_ref() == Some(id) {
                Some(&self.draft)
            } else {
                parked
            };
            let (harness, model) = match draft {
                Some(draft) => (draft.harness, draft.model.as_ref()),
                None => (
                    saved.and_then(|d| d.settings.harness),
                    saved.and_then(|d| d.settings.model.as_ref()),
                ),
            };
            let harness = harness.unwrap_or_else(|| self.default_harness());
            let model_known = model.is_some() || self.default_model_for(harness).is_some();
            if row.state == TabState::Idle && model_known {
                row.word = "Ready";
            }
        }
        self.tabs.draft_titles = titles;
    }

    /// Observe only the mounted text, and change one cached row only when needed.
    pub(super) fn refresh_draft_title(&mut self, cx: &mut Context<Self>) {
        let Some(id) = &self.draft.id else {
            return;
        };
        let name = title(
            self.draft
                .composer
                .as_ref()
                .map(|c| c.read(cx).saved_draft().0)
                .unwrap_or(&self.draft.saved.text),
        );
        if self.tabs.draft_titles.get(id) == Some(&name) {
            return;
        }
        if let Some((owner, index)) = &self.tabs.draft_row
            && owner == id
            && let Some(row) = self.tabs.rows.get_mut(*index)
        {
            rename(row, &name);
        }
        self.tabs.draft_titles.insert(id.clone(), name);
        // Equal-width tabs do not need layout or overflow regrouping for a new title.
        if let Some(strip) = &self.tabs.strip {
            strip.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    pub(super) fn current_draft_title(&self) -> &str {
        self.draft
            .id
            .as_ref()
            .and_then(|id| self.tabs.draft_titles.get(id))
            .map(String::as_str)
            .unwrap_or(UNTITLED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn titles_use_the_first_nonblank_line_with_bounded_unicode_storage() {
        for (text, expected) in [
            ("", UNTITLED),
            (" \n\t\r\n", UNTITLED),
            (
                " \n\t  Name this draft  \r\nAnother line",
                "Name this draft",
            ),
            ("First\nSecond", "First"),
        ] {
            assert_eq!(title(text), expected);
        }
        let pasted = "🦀".repeat(1_000_000);
        assert_eq!(title(&pasted), "🦀".repeat(120));
        assert_eq!(title(&format!("{} end", "a".repeat(119))), "a".repeat(119));
        assert_eq!(title("Spoof\u{202e}"), "Spoof\\u202e");
    }

    #[core::prelude::v1::test]
    fn titles_look_only_at_the_first_characters() {
        let blank = " \n".repeat(SCANNED / 2);
        assert_eq!(title(&format!("{blank}Too late")), UNTITLED);
        assert_eq!(title(&format!("{}Found", &blank[2..])), "Fo");
        assert_eq!(title(&format!("a{}b", " ".repeat(SCANNED))), "a");
    }

    #[core::prelude::v1::test]
    fn titles_show_each_run_of_whitespace_as_one_space() {
        assert_eq!(
            title("Tab\there\t\tand   spaces \r\nnext"),
            "Tab here and spaces"
        );
        assert_eq!(
            title(&format!("{} {}", "a".repeat(119), "b")),
            "a".repeat(119)
        );
        assert_eq!(
            title(&format!("{}  {}", "a".repeat(118), "bc")),
            format!("{} b", "a".repeat(118))
        );
    }
}
