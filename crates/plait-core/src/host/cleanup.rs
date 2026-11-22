//! Additive checkout cleanup reads and explicit, token-bound confirmation.
use super::{Client, Error};
use crate::ownership::CheckoutKeyExt;
use crate::{lease::cleanup::Cleanup, ownership::CheckoutKey};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request<'a> {
    #[serde(rename = "checkout_cleanup")]
    Read { checkout: &'a PathBuf },
    #[serde(rename = "stop_checkout_cleanup")]
    Stop {
        checkout: &'a PathBuf,
        token: &'a str,
        confirm_stop: bool,
    },
    #[serde(rename = "resolve_checkout_cleanup")]
    Confirm {
        checkout: &'a PathBuf,
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
) -> Result<Option<Cleanup>, Error> {
    identity.verify().map_err(Error::Protocol)?;
    let request = match action {
        Some((token, true)) => Request::Stop {
            checkout: &identity.root,
            token,
            confirm_stop: true,
        },
        Some((token, false)) => Request::Confirm {
            checkout: &identity.root,
            token,
            confirm_stopped: true,
        },
        None => Request::Read {
            checkout: &identity.root,
        },
    };
    let reply: Reply = client.send(&request)?;
    identity.verify().map_err(Error::Protocol)?;
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
