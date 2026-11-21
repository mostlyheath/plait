//! Versionless host release information and update draining.
use super::{Client, Error};
use serde::{Deserialize, Serialize};
#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    HostInfo,
    DrainForUpdate,
    CancelDrain,
}
#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainReply {
    pub draining: bool,
    pub active_runs: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelReply {
    pub draining: bool,
}
fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(d)
}
fn invalid() -> Error {
    Error::Protocol("Invalid host release reply.".into())
}
impl HostInfo {
    fn verified(self) -> Result<Self, Error> {
        if self.schema != 1
            || self.protocol_min > self.protocol_max
            || self.capabilities.len() > 64
            || self
                .capabilities
                .iter()
                .any(|s| s.len() > 64 || !s.is_ascii())
        {
            return Err(invalid());
        }
        Ok(self)
    }
}
impl DrainReply {
    fn verified(self) -> Result<Self, Error> {
        if !self.draining || self.active_runs > 9_007_199_254_740_991 {
            return Err(invalid());
        }
        Ok(self)
    }
}
impl CancelReply {
    fn verified(self) -> Result<Self, Error> {
        if self.draining {
            return Err(invalid());
        }
        Ok(self)
    }
}
fn info_result(result: Result<HostInfo, Error>) -> Result<Option<HostInfo>, Error> {
    match result {
        Ok(info) => info.verified().map(Some),
        Err(Error::Rejected(e)) if e.code == "invalid_request" => Ok(None),
        Err(e) => Err(e),
    }
}
pub fn host_info(client: &Client) -> Result<Option<HostInfo>, Error> {
    info_result(client.send(&Request::HostInfo))
}
pub fn drain(client: &Client) -> Result<DrainReply, Error> {
    client
        .send::<_, DrainReply>(&Request::DrainForUpdate)?
        .verified()
}
pub fn cancel_drain(client: &Client) -> Result<CancelReply, Error> {
    client
        .send::<_, CancelReply>(&Request::CancelDrain)?
        .verified()
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn old_host_rejection_is_none() {
        let error = super::super::HostError {
            code: "invalid_request".into(),
            message: "unknown".into(),
            retry_after_seconds: None,
        };
        assert!(info_result(Err(Error::Rejected(error))).unwrap().is_none());
    }
    #[test]
    fn release_replies_are_checked() {
        let info = json!({"schema":1,"release":null,"build_id":null,"protocol_min":1,"protocol_max":6,"capabilities":["drain_v1"],"storage_schema":1});
        assert!(
            serde_json::from_value::<HostInfo>(info.clone())
                .unwrap()
                .verified()
                .is_ok()
        );
        for (key, value) in [
            ("schema", json!(2)),
            ("protocol_min", json!(7)),
            ("capabilities", json!(["é"])),
            ("capabilities", json!(["x".repeat(65)])),
            ("capabilities", json!(vec!["x"; 65])),
        ] {
            let mut bad = info.clone();
            bad[key] = value;
            assert!(
                serde_json::from_value::<HostInfo>(bad)
                    .unwrap()
                    .verified()
                    .is_err()
            );
        }
        let mut extra = info;
        extra["extra"] = json!(1);
        assert!(serde_json::from_value::<HostInfo>(extra).is_err());
        assert!(
            serde_json::from_value::<DrainReply>(json!({"draining":true,"active_runs":1}))
                .unwrap()
                .verified()
                .is_ok()
        );
        assert!(
            serde_json::from_value::<DrainReply>(
                json!({"draining":true,"active_runs":9007199254740992u64})
            )
            .unwrap()
            .verified()
            .is_err()
        );
        assert!(
            serde_json::from_value::<CancelReply>(json!({"draining":false}))
                .unwrap()
                .verified()
                .is_ok()
        );
        assert!(
            serde_json::from_value::<CancelReply>(json!({"draining":true}))
                .unwrap()
                .verified()
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(Request::HostInfo).unwrap(),
            json!({"op":"host_info"})
        );
    }
}
