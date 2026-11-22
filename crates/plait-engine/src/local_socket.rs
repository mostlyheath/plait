//! Verification of local socket peer ownership.
use std::{io, os::unix::net::UnixStream};
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
};

pub fn connect(path: &Path) -> io::Result<UnixStream> {
    let metadata = std::fs::symlink_metadata(path)?;
    let parent = std::fs::symlink_metadata(
        path.parent()
            .ok_or_else(|| io::Error::other("invalid host socket path"))?,
    )?;
    let uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_socket()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || !parent.is_dir()
        || parent.uid() != uid
        || parent.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Host socket and parent must be owner-only and owned by this user.",
        ));
    }
    let socket = UnixStream::connect(path)?;
    require_same_user(&socket)?;
    Ok(socket)
}

pub fn require_same_user(socket: &UnixStream) -> io::Result<()> {
    require_user(socket, unsafe { libc::geteuid() })
}

fn require_user(socket: &UnixStream, expected: libc::uid_t) -> io::Result<()> {
    // Path permissions do not establish the identity of the connected process.
    if peer_uid(socket)? != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Local host peer ownership check failed.",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn peer_uid(socket: &UnixStream) -> io::Result<libc::uid_t> {
    use std::os::fd::AsRawFd;
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if length as usize != size_of::<libc::ucred>() || credentials.pid <= 0 {
        return Err(io::Error::other("invalid local peer credentials"));
    }
    Ok(credentials.uid)
}

#[cfg(target_os = "macos")]
fn peer_uid(socket: &UnixStream) -> io::Result<libc::uid_t> {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (0, 0);
    if unsafe { libc::getpeereid(socket.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peer_uid(_: &UnixStream) -> io::Result<libc::uid_t> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Local peer verification is not supported on this platform.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_peers_must_have_the_expected_identity() {
        let (client, server) = UnixStream::pair().unwrap();
        let uid = unsafe { libc::geteuid() };
        require_same_user(&client).unwrap();
        require_same_user(&server).unwrap();
        assert_eq!(
            require_user(&client, uid.wrapping_add(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
