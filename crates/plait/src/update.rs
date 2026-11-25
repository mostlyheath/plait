pub mod install;
pub mod setup;
use plait_client::update::*;
fn invalid(detail: impl Into<String>) -> Error {
    Error::Invalid(detail.into())
}
