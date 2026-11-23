//! SSH destination and reconnection state, available on every client.
use crate::host::Error;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct SshDestination(String);
impl SshDestination {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 253
            || value.starts_with('-')
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_@:[]".contains(&b))
        {
            return Err(Error::Transport("Invalid SSH destination.".into()));
        }
        Ok(Self(value))
    }
}

impl<'de> serde::Deserialize<'de> for SshDestination {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Default, Debug)]
pub struct Reconnect {
    failures: u32,
}
impl Reconnect {
    pub fn next_delay(&mut self) -> Duration {
        let seconds = if self.failures >= 5 {
            30
        } else {
            1 << self.failures
        };
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(seconds)
    }
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(feature = "ssh")]
#[path = "ssh_process.rs"]
mod process;
#[cfg(feature = "ssh")]
pub(crate) use process::base_arguments;
#[cfg(feature = "ssh")]
pub use process::{SshOptions, connect};
