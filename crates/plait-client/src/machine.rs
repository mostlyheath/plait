//! The local machine's display name.

/// Shown when the system gives no usable hostname.
pub const FALLBACK_NAME: &str = "This machine";

/// The host name, without a domain: `Workstation.local` is shown as `Workstation`.
pub fn local_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length; the result is NUL-terminated
    // or truncated, and only the bytes before the first NUL are read.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return FALLBACK_NAME.into();
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    display_name(&String::from_utf8_lossy(&buffer[..end]))
}

/// Keep the first label of a host name and drop control characters.
pub fn display_name(raw: &str) -> String {
    let name: String = raw
        .split('.')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let name = name.trim();
    if name.is_empty() {
        FALLBACK_NAME.into()
    } else {
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use super::{FALLBACK_NAME, display_name, local_name};

    #[test]
    fn domain_and_control_characters_are_removed() {
        assert_eq!(display_name("Workstation.local"), "Workstation");
        assert_eq!(display_name("studio"), "studio");
        assert_eq!(display_name("build-01.example.com"), "build-01");
        assert_eq!(display_name("lap\u{7}top"), "laptop");
        assert_eq!(display_name(""), FALLBACK_NAME);
        assert_eq!(display_name(".local"), FALLBACK_NAME);
        assert_eq!(display_name("  "), FALLBACK_NAME);
    }

    #[test]
    fn this_machine_has_a_name() {
        assert!(!local_name().is_empty());
    }
}
