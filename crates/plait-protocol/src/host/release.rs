use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    HostInfo,
    DrainForUpdate,
    CancelDrain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostInfo {
    pub schema: u32,
    #[serde(deserialize_with = "nullable")]
    pub release: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub build_id: Option<String>,
    pub protocol_min: u32,
    pub protocol_max: u32,
    pub capabilities: Vec<String>,
    pub storage_schema: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainReply {
    pub draining: bool,
    pub active_runs: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelReply {
    pub draining: bool,
}
fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(d)
}
