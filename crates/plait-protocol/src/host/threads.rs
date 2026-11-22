use super::ThreadSummary;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub threads: Vec<ThreadSummary>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event: String,
    pub threads: Vec<ThreadSummary>,
    pub removed: Vec<String>,
}
