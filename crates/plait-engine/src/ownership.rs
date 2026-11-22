use plait_protocol::ownership::{CheckoutKey, HostKey};
use std::{os::unix::fs::MetadataExt, path::Path};

pub trait CheckoutKeyExt: Sized {
    fn resolve(host: Option<HostKey>, root: &Path) -> Result<Self, String>;
    fn verify(&self) -> Result<(), String>;
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
    fn verify(&self) -> Result<(), String> {
        let meta = std::fs::symlink_metadata(&self.root)
            .map_err(|_| "Checkout identity changed.".to_owned())?;
        if !meta.is_dir()
            || meta.dev() != self.device
            || meta.ino() != self.inode
            || self.root.canonicalize().ok().as_ref() != Some(&self.root)
        {
            return Err("Checkout identity changed.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_aliases_share_identity_but_replacements_do_not() {
        let scratch = std::env::temp_dir().join(format!(
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
}
