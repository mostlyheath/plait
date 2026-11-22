use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_PATHS: usize = 32;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub files: u64,
    pub links: u64,
    pub skipped: u64,
    pub bytes: u64,
    pub elapsed_ms: u64,
    pub notes: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub warm: bool,
    pub reason: String,
    pub directories: Vec<String>,
    pub counts: Counts,
}
impl Outcome {
    pub fn cold(reason: impl Into<String>) -> Self {
        Self {
            warm: false,
            reason: reason.into(),
            directories: Vec::new(),
            counts: Counts::default(),
        }
    }
}
impl Counts {
    pub fn note(&mut self, note: String) {
        if self.notes.len() < 20 {
            self.notes.push(note.chars().take(200).collect());
        }
    }
    pub fn skip(&mut self, path: &std::path::Path, reason: &str) {
        self.skipped += 1;
        self.note(format!("{}: {reason}", path.display()));
    }
}

/// Exact relative directories, never patterns, aliases, Git data or environments
/// whose entrypoints normally carry the old checkout's absolute path.
pub fn relative(value: &str) -> Result<PathBuf, String> {
    if value.is_empty()
        || value.len() > 1024
        || value.chars().any(char::is_control)
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || p.eq_ignore_ascii_case(".git")
                || [".venv", "venv"].contains(&p)
        })
    {
        return Err(
            "Use a relative build directory; Git data and Python environments cannot be approved."
                .into(),
        );
    }
    Ok(PathBuf::from(value))
}
