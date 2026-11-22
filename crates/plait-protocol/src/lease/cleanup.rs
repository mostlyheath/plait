use serde::{Deserialize, Serialize};

pub const STOP_UNCONFIRMED: &str = "Stop unconfirmed";
pub const RESTART_REASON: &str = "The host ended before it confirmed this run's cleanup.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleanup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<CleanupProcess>,
    pub token: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupProcess {
    pub group: i32,
    pub command: String,
    pub can_stop: bool,
}

impl Cleanup {
    pub fn message(&self) -> String {
        format!(
            "Stop unconfirmed. {} New runs and Git changes are blocked. Check for processes still using this checkout, then confirm they have stopped.",
            self.reason
        )
    }
}
