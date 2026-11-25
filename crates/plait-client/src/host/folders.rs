//! Read-only folder browsing on a host. Older hosts reject the additive operation.
use super::{Client, Error};
use serde::Serialize;
use std::path::Path;

pub use plait_protocol::host::folders::{Entry, Listing};

#[derive(Serialize)]
struct Request<'a> {
    op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a Path>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    hidden: bool,
}

pub fn browse(client: &Client, path: Option<&Path>, hidden: bool) -> Result<Listing, Error> {
    client.send(&Request {
        op: "browse_folder",
        path,
        hidden,
    })
}

pub fn needs_host_update(error: &Error) -> bool {
    matches!(error, Error::Rejected(error) if error.code == "invalid_request")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_omits_absent_fields_and_version() {
        let request = Request {
            op: "browse_folder",
            path: None,
            hidden: false,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"op":"browse_folder"})
        );
        let request = Request {
            op: "browse_folder",
            path: Some(Path::new("/srv")),
            hidden: true,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"op":"browse_folder", "path":"/srv", "hidden":true})
        );
    }

    #[test]
    fn listing_deserializes() {
        let listing: Listing = serde_json::from_str(r#"{"path":"/srv","parent":"/","home":"/home/me","git":true,"entries":[{"name":"repo","git":false}],"truncated":false}"#).unwrap();
        assert_eq!(listing.parent.as_deref(), Some(Path::new("/")));
        assert_eq!(
            listing.entries[0],
            Entry {
                name: "repo".into(),
                git: false
            }
        );
        assert!(listing.git);
    }
}
