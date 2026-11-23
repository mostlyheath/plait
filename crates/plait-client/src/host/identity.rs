//! Additive identity read, available before the first thread exists.
use super::{Client, Error};
use crate::tabs::HostKey;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize)]
struct Request {
    op: &'static str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    schema: u32,
    host_id: String,
    store: PathBuf,
}
impl Identity {
    fn verified(self) -> Result<HostKey, Error> {
        if self.schema != 1
            || self.host_id.is_empty()
            || self.host_id.len() > 128
            || !self
                .host_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || !self.store.is_absolute()
            || self.store.as_os_str().len() > 4096
            || self.store.as_os_str().as_encoded_bytes().contains(&0)
        {
            return Err(Error::Protocol("Invalid host identity.".into()));
        }
        Ok(HostKey {
            id: self.host_id,
            store: self.store,
        })
    }
}
pub fn from_review(host_id: String, store: PathBuf) -> Result<HostKey, Error> {
    Identity {
        schema: 1,
        host_id,
        store,
    }
    .verified()
}
pub fn read(client: &Client) -> Result<HostKey, Error> {
    let identity: Identity = client.send(&Request {
        op: "host_identity",
    })?;
    identity.verified()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_validates_identity_and_schema() {
        let good = r#"{"schema":1,"host_id":"host-1","store":"/store"}"#;
        assert_eq!(
            serde_json::from_str::<Identity>(good)
                .unwrap()
                .verified()
                .unwrap()
                .id,
            "host-1"
        );
        for value in [
            good.replace("\"schema\":1", "\"schema\":2"),
            good.replace("host-1", "../bad"),
            good.replace("/store", "relative"),
            good.replace("host-1", ""),
        ] {
            assert!(
                serde_json::from_str::<Identity>(&value)
                    .unwrap()
                    .verified()
                    .is_err()
            );
        }
        assert!(serde_json::from_str::<Identity>(&good.replace("}", ",\"extra\":true}")).is_err());
        assert_eq!(
            serde_json::to_string(&Request {
                op: "host_identity"
            })
            .unwrap(),
            r#"{"op":"host_identity"}"#
        );
    }
}
