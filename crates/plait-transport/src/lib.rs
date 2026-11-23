//! Shared bounded frame and owner-checked local socket I/O.
pub mod frame;
#[cfg(all(feature = "unix", unix))]
pub mod local_socket;
