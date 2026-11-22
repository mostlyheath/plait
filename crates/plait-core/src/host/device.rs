//! Per-device state stored on a host. These additive requests carry no version.
use super::{Client, Error};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request<'a> {
    DeviceStateGet {
        device: &'a str,
    },
    DeviceStateSet {
        device: &'a str,
        revision: u64,
        state: &'a Value,
    },
}

#[derive(Deserialize)]
struct State {
    revision: u64,
    state: Option<Value>,
}
#[derive(Deserialize)]
struct Revision {
    revision: u64,
}

pub fn get(client: &Client, device: &str) -> Result<(u64, Option<Value>), Error> {
    let reply: State = client.send(&Request::DeviceStateGet { device })?;
    Ok((reply.revision, reply.state))
}

pub fn set(client: &Client, device: &str, revision: u64, state: &Value) -> Result<u64, Error> {
    let reply: Revision = client.send(&Request::DeviceStateSet {
        device,
        revision,
        state,
    })?;
    Ok(reply.revision)
}

pub fn is_conflict(error: &Error) -> bool {
    matches!(error, Error::Rejected(error) if error.code == "conflict")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_have_no_protocol_version() {
        assert_eq!(
            serde_json::to_value(Request::DeviceStateGet { device: "mac" }).unwrap(),
            json!({"op":"device_state_get","device":"mac"})
        );
        assert_eq!(
            serde_json::to_value(Request::DeviceStateSet {
                device: "mac",
                revision: 2,
                state: &json!({"tabs": []})
            })
            .unwrap(),
            json!({"op":"device_state_set","device":"mac","revision":2,"state":{"tabs":[]}})
        );
    }
}
