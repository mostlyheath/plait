//! Owner-only local connections shared by clients and the daemon.

pub use plait_transport::local_socket::{connect, require_same_user};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn client_rejects_public_parents_and_symlink_paths() {
        use std::os::unix::{
            fs::{PermissionsExt, symlink},
            net::UnixListener,
        };
        let root = std::env::temp_dir().join(format!(
            "plait-socket-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("host.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            connect(&path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        connect(&path).unwrap();
        let alias = root.join("alias.sock");
        symlink(&path, &alias).unwrap();
        assert_eq!(
            connect(&alias).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
