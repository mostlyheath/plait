use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
    pub home: PathBuf,
    pub git: bool,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub git: bool,
}
