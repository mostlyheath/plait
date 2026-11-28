use plait_protocol::ownership::{CheckoutKey, HostKey};
use std::{os::unix::fs::MetadataExt, path::Path};

pub trait CheckoutKeyExt: Sized {
    fn resolve(host: Option<HostKey>, root: &Path) -> Result<Self, String>;
    fn verify(&self) -> Result<Self, String>;
}

impl CheckoutKeyExt for CheckoutKey {
    /// Blocking: resolve only on a background executor.
    fn resolve(host: Option<HostKey>, root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("Cannot identify checkout: {error}"))?;
        let meta = std::fs::symlink_metadata(&root)
            .map_err(|error| format!("Cannot identify checkout: {error}"))?;
        if !meta.is_dir() {
            return Err("Checkout is not a directory.".into());
        }
        Ok(Self {
            host,
            root,
            device: meta.dev(),
            inode: meta.ino(),
        })
    }

    /// Blocking. A substituted symlink is a changed identity, even if it resolves back.
    /// Returns the key with the current device number: macOS assigns device numbers
    /// at mount time, so a restart can change one while the folder (same canonical
    /// path, same inode) is unchanged.
    fn verify(&self) -> Result<Self, String> {
        let meta = std::fs::symlink_metadata(&self.root)
            .map_err(|_| "Checkout identity changed.".to_owned())?;
        if !meta.is_dir()
            || meta.ino() != self.inode
            || self.root.canonicalize().ok().as_ref() != Some(&self.root)
        {
            return Err("Checkout identity changed.".into());
        }
        Ok(Self {
            device: meta.dev(),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_aliases_share_identity_but_replacements_do_not() {
        let scratch = plait_test_support::temp_dir().join(format!(
            "plait-identity-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = scratch.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let alias = scratch.join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let key = CheckoutKey::resolve(None, &root).unwrap();
        assert_eq!(CheckoutKey::resolve(None, &alias).unwrap(), key);
        key.verify().unwrap();
        std::fs::rename(&root, scratch.join("old")).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert!(key.verify().is_err());
        assert_ne!(CheckoutKey::resolve(None, &root).unwrap(), key);
        std::fs::remove_dir(&root).unwrap();
        std::os::unix::fs::symlink(scratch.join("old"), &root).unwrap();
        assert!(key.verify().is_err());
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn current_checkout_refreshes_changed_device() {
        let mut key = CheckoutKey::resolve(None, &std::env::temp_dir()).unwrap();
        key.device = key.device.wrapping_add(1);
        let refreshed = key.verify().unwrap();
        assert_ne!(refreshed.device, key.device);
        assert_eq!(refreshed.inode, key.inode);
    }
}
