//! Kernel process identity; no command is launched to inspect or stop a group.
use std::io;
#[cfg(target_os = "macos")]
use std::mem::MaybeUninit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Process {
    pub pid: i32,
    pub group: i32,
    pub birth: [u64; 2],
}

#[cfg(target_os = "macos")]
pub(super) fn inspect(pid: i32) -> io::Result<Process> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read != size as i32 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(Process {
        pid,
        group: info.pbi_pgid as i32,
        birth: [info.pbi_start_tvsec, info.pbi_start_tvusec],
    })
}

#[cfg(target_os = "linux")]
pub(super) fn inspect(pid: i32) -> io::Result<Process> {
    // This also runs between fork and exec: only stack buffers and syscalls.
    let mut path = [0u8; 48];
    path[..6].copy_from_slice(b"/proc/");
    let mut digits = [0u8; 10];
    let mut number = pid as u32;
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (number % 10) as u8;
        number /= 10;
        if number == 0 {
            break;
        }
    }
    let end = 6 + digits.len() - at;
    path[6..end].copy_from_slice(&digits[at..]);
    path[end..end + 5].copy_from_slice(b"/stat");
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = [0u8; 4096];
    let count = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
    let error = io::Error::last_os_error();
    unsafe {
        libc::close(fd);
    }
    if count < 0 {
        return Err(error);
    }
    parse_stat(pid, &bytes[..count as usize]).ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))
}

#[cfg(any(target_os = "linux", test))]
fn parse_stat(pid: i32, bytes: &[u8]) -> Option<Process> {
    let end = bytes.iter().rposition(|byte| *byte == b')')?;
    let mut fields = bytes.get(end + 2..)?.split(|byte| *byte == b' ');
    let number = |field: &[u8]| {
        if field.is_empty() {
            return None;
        }
        field.iter().try_fold(0u64, |value, digit| {
            digit.is_ascii_digit().then_some(())?;
            value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
        })
    };
    fields.next()?;
    fields.next()?; // state, parent
    let group = i32::try_from(number(fields.next()?)?).ok()?;
    let start = number(fields.nth(16)?)?; // field 22, after process group (field 5)
    Some(Process {
        pid,
        group,
        birth: [start, 0],
    })
}

/// No process, not even a zombie, holds this PID.
pub(super) fn pid_free(pid: i32) -> bool {
    pid > 0
        && unsafe { libc::kill(pid, 0) } != 0
        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

pub(super) fn group_gone(group: i32) -> bool {
    group > 0
        && unsafe { libc::kill(-group, 0) } != 0
        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// A record from another boot names processes that are all gone. Zero means
/// the boot is unknown and is never treated as a different boot.
#[cfg(target_os = "macos")]
pub(super) fn boot() -> io::Result<u64> {
    // The kernel's per-boot UUID; unlike kern.boottime it ignores clock changes.
    let mut id = [0u8; 64];
    let mut size = id.len();
    let read = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            id.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(boot_digest(&id[..size]))
}

#[cfg(target_os = "linux")]
pub(super) fn boot() -> io::Result<u64> {
    // A PID and boot-relative start tick must not identify a process after reboot.
    Ok(boot_digest(&std::fs::read(
        "/proc/sys/kernel/random/boot_id",
    )?))
}

fn boot_digest(id: &[u8]) -> u64 {
    use sha2::{Digest, Sha256};
    let id = id.strip_suffix(b"\0").unwrap_or(id);
    u64::from_le_bytes(Sha256::digest(id)[..8].try_into().unwrap()).max(1)
}

/// Different only when both boots are known.
pub(super) fn other_boot(saved: u64) -> io::Result<bool> {
    Ok(saved != 0 && boot()? != saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_identity_matches_the_current_kernel_group() {
        let identity = inspect(std::process::id() as i32).unwrap();
        assert_eq!(identity.group, unsafe { libc::getpgrp() });
        assert_ne!(identity.birth, [0, 0]);
        assert_eq!(identity, inspect(identity.pid).unwrap());
    }

    #[test]
    fn the_boot_identity_is_known_and_stable() {
        let boot = boot().unwrap();
        assert_ne!(boot, 0, "a reboot must be recognizable");
        assert_eq!(boot, super::boot().unwrap());
        assert!(other_boot(boot ^ 1).unwrap());
        assert!(!other_boot(boot).unwrap());
        assert!(
            !other_boot(0).unwrap(),
            "an unknown boot is not another boot"
        );
    }

    #[test]
    fn linux_stat_identity_handles_names_with_spaces_and_parentheses() {
        let stat =
            b"123 (name ) with (spaces)) S 1 456 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 98765 99";
        assert_eq!(
            parse_stat(123, stat).unwrap(),
            Process {
                pid: 123,
                group: 456,
                birth: [98765, 0],
            }
        );
        for invalid in [
            b"123 missing".as_slice(),
            b"123 (name) S 1 456",
            b"123 (name) S 1 99999999999999999999999999",
        ] {
            assert!(parse_stat(123, invalid).is_none());
        }
    }
}
