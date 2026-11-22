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

/// One JSON document kept under `field` in this device's state, as bytes, so
/// a caller that stores its own encoding need not parse it here.
pub fn get_field(
    client: &Client,
    device: &str,
    field: &str,
) -> Result<(u64, Option<Vec<u8>>), Error> {
    let (revision, state) = get(client, device)?;
    let bytes = state
        .as_ref()
        .and_then(|state| state.get(field))
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| Error::Protocol("The host sent unreadable device state.".into()))?;
    Ok((revision, bytes))
}

/// Replaces this device's state with `field` holding the JSON document `bytes`.
pub fn set_field(
    client: &Client,
    device: &str,
    revision: u64,
    field: &str,
    bytes: &[u8],
) -> Result<u64, Error> {
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|_| Error::Protocol("Device state must be JSON.".into()))?;
    let mut state = serde_json::Map::new();
    state.insert(field.into(), document);
    set(client, device, revision, &Value::Object(state))
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
