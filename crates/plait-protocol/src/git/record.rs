use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// A directory Git ignores, such as build output.
    IgnoredDirectory,
    /// An ignored file over `IGNORED_FILE_LIMIT`.
    IgnoredLarge,
    /// An ignored file beyond `IGNORED_COUNT_LIMIT` or `IGNORED_TOTAL_LIMIT`.
    IgnoredLimit,
    /// A file over `FILE_LIMIT`.
    TooLarge,
    /// A repository inside the checkout.
    NestedRepository,
    /// A submodule of the repository.
    Submodule,
    /// A socket, pipe, or device.
    Special,
    /// A name that is not UTF-8, which the host cannot name to the desktop.
    Name,
    /// A file that changed while it was being recorded.
    Changing,
    /// A file Git could not read, or one cut short while it was read.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Uncovered {
    /// Checkout-relative; a directory ends with `/`.
    pub path: String,
    pub reason: Reason,
}
