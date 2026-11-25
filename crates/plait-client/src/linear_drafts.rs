//! Unsaved Linear issue text, kept across quitting and switching projects.
//!
//! One bounded record in the desktop journal holds every open form: a new
//! issue per project and edits per issue, each with its request ID. The
//! desktop writes it after typing pauses, and before any save is sent, so a
//! save after a crash names the same request and the host returns its saved
//! outcome. Nothing here talks to Linear.
//!
//! The journal belongs to one host store. Moving this into the per-machine
//! session state (#132) is a later change.
use crate::linear_issues::{FormKind, FormSlot};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The journal record's name.
pub const RECORD: &str = "linear-issue-drafts";
/// The record's size limit: forms are bounded text, and few are kept.
pub const RECORD_LIMIT: usize = 4 * 1024 * 1024;
/// Forms kept in all, oldest first out.
pub const FORM_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedKind {
    New {
        team_id: String,
        #[serde(default)]
        team_name: Option<String>,
    },
    Edit {
        issue_id: String,
        expected_updated_at: String,
        base_title: String,
        base_description: String,
    },
}

/// One form as typed: where it belongs, what it writes, its text, and the
/// request ID every save of it uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedForm {
    pub checkout: PathBuf,
    pub kind: SavedKind,
    pub title: String,
    pub description: String,
    pub request_id: String,
}

impl SavedForm {
    pub fn new(
        checkout: &Path,
        kind: &FormKind,
        title: &str,
        description: &str,
        request_id: &str,
    ) -> Self {
        let kind = match kind {
            FormKind::New { team_id, team_name } => SavedKind::New {
                team_id: team_id.clone(),
                team_name: team_name.clone(),
            },
            FormKind::Edit {
                issue_id,
                expected_updated_at,
                title,
                description,
            } => SavedKind::Edit {
                issue_id: issue_id.clone(),
                expected_updated_at: expected_updated_at.clone(),
                base_title: title.clone(),
                base_description: description.clone(),
            },
        };
        Self {
            checkout: checkout.to_owned(),
            kind,
            title: title.to_owned(),
            description: description.to_owned(),
            request_id: request_id.to_owned(),
        }
    }

    pub fn slot(&self) -> FormSlot {
        match &self.kind {
            SavedKind::New { .. } => FormSlot::New,
            SavedKind::Edit { issue_id, .. } => FormSlot::Edit(issue_id.clone()),
        }
    }

    /// The form to reopen, as it was when it started.
    pub fn form_kind(&self) -> FormKind {
        match &self.kind {
            SavedKind::New { team_id, team_name } => FormKind::New {
                team_id: team_id.clone(),
                team_name: team_name.clone(),
            },
            SavedKind::Edit {
                issue_id,
                expected_updated_at,
                base_title,
                base_description,
            } => FormKind::Edit {
                issue_id: issue_id.clone(),
                expected_updated_at: expected_updated_at.clone(),
                title: base_title.clone(),
                description: base_description.clone(),
            },
        }
    }
}

/// Every saved form, in the order they were last written.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drafts {
    pub forms: Vec<SavedForm>,
}

impl Drafts {
    /// The forms of one project.
    pub fn of<'a>(&'a self, checkout: &'a Path) -> impl Iterator<Item = &'a SavedForm> {
        self.forms.iter().filter(move |f| f.checkout == checkout)
    }

    /// Replaces one project's forms with `forms`, keeping the others, and
    /// drops the oldest beyond the limit. Returns whether anything changed.
    pub fn replace(&mut self, checkout: &Path, forms: Vec<SavedForm>) -> bool {
        if self.of(checkout).eq(forms.iter()) {
            return false;
        }
        self.forms.retain(|f| f.checkout != checkout);
        self.forms.extend(forms);
        let excess = self.forms.len().saturating_sub(FORM_LIMIT);
        self.forms.drain(..excess);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_form(root: &str, title: &str) -> SavedForm {
        SavedForm::new(
            Path::new(root),
            &FormKind::New {
                team_id: "T".into(),
                team_name: Some("Plait".into()),
            },
            title,
            "",
            "r1",
        )
    }

    #[test]
    fn each_project_keeps_its_own_forms() {
        let mut drafts = Drafts::default();
        assert!(drafts.replace(Path::new("/a"), vec![new_form("/a", "A")]));
        assert!(drafts.replace(Path::new("/b"), vec![new_form("/b", "B")]));
        assert!(
            !drafts.replace(Path::new("/a"), vec![new_form("/a", "A")]),
            "unchanged"
        );
        assert!(drafts.replace(Path::new("/a"), vec![]));
        let left: Vec<_> = drafts.forms.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(left, ["B"]);
    }

    #[test]
    fn a_form_reopens_with_its_team_version_and_request() {
        let kind = FormKind::Edit {
            issue_id: "I".into(),
            expected_updated_at: "1".into(),
            title: "Old".into(),
            description: "Body".into(),
        };
        let saved = SavedForm::new(Path::new("/a"), &kind, "Mine", "My body", "r9");
        assert_eq!(saved.form_kind(), kind);
        assert_eq!(saved.slot(), FormSlot::Edit("I".into()));
        let json = serde_json::to_string(&Drafts {
            forms: vec![saved.clone(), new_form("/a", "New")],
        })
        .unwrap();
        let back: Drafts = serde_json::from_str(&json).unwrap();
        assert_eq!(back.forms[0], saved);
        assert_eq!(back.forms[0].request_id, "r9");
        assert!(
            matches!(back.forms[1].form_kind(), FormKind::New { team_name: Some(n), .. } if n == "Plait")
        );
    }

    #[test]
    fn the_oldest_forms_go_first_beyond_the_limit() {
        let mut drafts = Drafts::default();
        let forms = (0..FORM_LIMIT + 3)
            .map(|i| new_form(&format!("/p{i}"), "x"))
            .collect::<Vec<_>>();
        for form in forms {
            let root = form.checkout.clone();
            drafts.replace(&root, vec![form]);
        }
        assert_eq!(drafts.forms.len(), FORM_LIMIT);
        assert_eq!(drafts.forms[0].checkout, Path::new("/p3"));
    }
}
