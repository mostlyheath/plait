//! Versionless host release information and update draining.
use super::{Client, Error};
pub use plait_protocol::host::release::{CancelReply, DrainReply, HostInfo, Request};

fn invalid() -> Error {
    Error::Protocol("Invalid host release reply.".into())
}
fn verified_info(info: HostInfo) -> Result<HostInfo, Error> {
    let self_ = info;
    if self_.schema != 1
        || self_.protocol_min > self_.protocol_max
        || self_.capabilities.len() > 64
        || self_
            .capabilities
            .iter()
            .any(|s| s.len() > 64 || !s.is_ascii())
    {
        return Err(invalid());
    }
    Ok(self_)
}
fn verified_drain(info: DrainReply) -> Result<DrainReply, Error> {
    let self_ = info;
    if !self_.draining || self_.active_runs > 9_007_199_254_740_991 {
        return Err(invalid());
    }
    Ok(self_)
}
fn verified_cancel(info: CancelReply) -> Result<CancelReply, Error> {
    let self_ = info;
    if self_.draining {
        return Err(invalid());
    }
    Ok(self_)
}
fn info_result(result: Result<HostInfo, Error>) -> Result<Option<HostInfo>, Error> {
    match result {
        Ok(info) => verified_info(info).map(Some),
        Err(Error::Rejected(e)) if e.code == "invalid_request" => Ok(None),
        Err(e) => Err(e),
    }
}
pub fn host_info(client: &Client) -> Result<Option<HostInfo>, Error> {
    info_result(client.send(&Request::HostInfo))
}
pub fn drain(client: &Client) -> Result<DrainReply, Error> {
    verified_drain(client.send::<_, DrainReply>(&Request::DrainForUpdate)?)
}
pub fn cancel_drain(client: &Client) -> Result<CancelReply, Error> {
    verified_cancel(client.send::<_, CancelReply>(&Request::CancelDrain)?)
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
        assert!(verified_info(serde_json::from_value::<HostInfo>(info.clone()).unwrap()).is_ok());
        for (key, value) in [
            ("schema", json!(2)),
            ("protocol_min", json!(7)),
            ("capabilities", json!(["é"])),
            ("capabilities", json!(["x".repeat(65)])),
            ("capabilities", json!(vec!["x"; 65])),
        ] {
            let mut bad = info.clone();
            bad[key] = value;
            assert!(verified_info(serde_json::from_value::<HostInfo>(bad).unwrap()).is_err());
        }
        let mut extra = info;
        extra["extra"] = json!(1);
        assert!(serde_json::from_value::<HostInfo>(extra).is_err());
        assert!(
            verified_drain(
                serde_json::from_value::<DrainReply>(json!({"draining":true,"active_runs":1}))
                    .unwrap()
            )
            .is_ok()
        );
        assert!(
            verified_drain(
                serde_json::from_value::<DrainReply>(
                    json!({"draining":true,"active_runs":9007199254740992u64})
                )
                .unwrap()
            )
            .is_err()
        );
        assert!(
            verified_cancel(
                serde_json::from_value::<CancelReply>(json!({"draining":false})).unwrap()
            )
            .is_ok()
        );
        assert!(
            verified_cancel(
                serde_json::from_value::<CancelReply>(json!({"draining":true})).unwrap()
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(Request::HostInfo).unwrap(),
            json!({"op":"host_info"})
        );
    }
}
