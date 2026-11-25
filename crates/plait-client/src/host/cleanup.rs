//! Additive checkout cleanup reads and explicit, token-bound confirmation.
use super::git::WireIdentity;
use super::{Client, Error};
use crate::ownership::CheckoutKey;
use plait_protocol::lease::cleanup::Cleanup;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request<'a> {
    #[serde(rename = "checkout_cleanup")]
    Read {
        checkout: &'a PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<WireIdentity>,
    },
    #[serde(rename = "stop_checkout_cleanup")]
    Stop {
        checkout: &'a PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<WireIdentity>,
        token: &'a str,
        confirm_stop: bool,
    },
    #[serde(rename = "resolve_checkout_cleanup")]
    Confirm {
        checkout: &'a PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<WireIdentity>,
        token: &'a str,
        confirm_stopped: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    checkout: PathBuf,
    cleanup: Option<Cleanup>,
}

pub fn read(
    client: &Client,
    identity: &CheckoutKey,
    action: Option<(&str, bool)>,
    supported: bool,
    _local: bool,
) -> Result<Option<Cleanup>, Error> {
    if !supported {
        return Err(Error::Protocol(
            "Update the host to verify the checkout.".into(),
        ));
    }
    let wire = supported.then(|| WireIdentity {
        dev: identity.device.to_string(),
        ino: identity.inode.to_string(),
    });
    let request = match action {
        Some((token, true)) => Request::Stop {
            checkout: &identity.root,
            identity: wire,
            token,
            confirm_stop: true,
        },
        Some((token, false)) => Request::Confirm {
            checkout: &identity.root,
            identity: wire,
            token,
            confirm_stopped: true,
        },
        None => Request::Read {
            checkout: &identity.root,
            identity: wire,
        },
    };
    let reply: Reply = client.send(&request)?;
    if reply.checkout != identity.root
        || reply.cleanup.as_ref().is_some_and(|state| {
            state.token.is_empty()
                || state.token.len() > 128
                || !state
                    .token
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                || state.reason.is_empty()
                || state.reason.len() > 1024
                || state.reason.chars().any(char::is_control)
                || state.process.as_ref().is_some_and(|process| {
                    process.group <= 0
                        || process.command.is_empty()
                        || process.command.len() > 95
                        || process.command.chars().any(char::is_control)
                })
        })
    {
        return Err(Error::Protocol("Invalid checkout cleanup reply.".into()));
    }
    Ok(reply.cleanup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_hosts_cannot_clean_up_without_identity_verification() {
        let client = Client::at_socket(PathBuf::from("/unused/host.sock"));
        let identity = CheckoutKey {
            host: None,
            root: PathBuf::from("/checkout"),
            device: 1,
            inode: 2,
        };
        for local in [false, true] {
            let error = read(&client, &identity, None, false, local).unwrap_err();
            assert!(error.to_string().contains("Update the host"));
        }
    }
}
