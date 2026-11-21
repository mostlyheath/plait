//! Release signing for update manifests. Not shipped in the app.
//!
//!   update-key generate <key-file>   create a key; prints its public half
//!   update-key sign <key-file> <manifest>   writes <manifest>.sig
//!   update-key verify <manifest>     checks against the key built into Plait
//!
//! The key file holds the 32-byte secret as hex. Keep it out of Git.

use ed25519_dalek::{Signer, SigningKey};
use std::{
    env, fs,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::ExitCode,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn load(path: &Path) -> io::Result<SigningKey> {
    let text = fs::read_to_string(path)?;
    let text = text.trim();
    let mut secret = [0u8; 32];
    if text.len() != 64 {
        return Err(io::Error::other("key file must hold 64 hex digits"));
    }
    for (index, byte) in secret.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(io::Error::other)?;
    }
    Ok(SigningKey::from_bytes(&secret))
}

fn generate(path: &Path) -> io::Result<()> {
    let mut secret = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut secret)?;
    // create_new refuses to overwrite an existing key; 0600 keeps it private.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| {
            io::Write::write_all(&mut file, format!("{}\n", hex(&secret)).as_bytes())
        })?;
    println!(
        "{}",
        hex(SigningKey::from_bytes(&secret).verifying_key().as_bytes())
    );
    Ok(())
}

fn sign(key: &Path, manifest: &Path) -> io::Result<()> {
    let key = load(key)?;
    let bytes = fs::read(manifest)?;
    let expected = plait_core::update::public_key().map_err(io::Error::other)?;
    if key.verifying_key() != expected {
        return Err(io::Error::other(
            "this key does not match the public key built into Plait",
        ));
    }
    let signature = format!("{}\n", hex(&key.sign(&bytes).to_bytes()));
    let mut target = manifest.as_os_str().to_owned();
    target.push(".sig");
    fs::write(&target, &signature)?;
    // Prove the published pair passes the same check the app runs.
    check(manifest, &bytes, signature.as_bytes(), &expected)?;
    Ok(())
}

/// The check a client runs on this file: schema 1 for `update.json`, the
/// component manifests for `update-client.json` and `update-host.json`.
fn check(
    manifest: &Path,
    bytes: &[u8],
    signature: &[u8],
    key: &ed25519_dalek::VerifyingKey,
) -> io::Result<(String, usize)> {
    use plait_core::update::{Component, verify, verify_component};
    let component = match manifest.file_name().and_then(|name| name.to_str()) {
        Some("update-client.json") => Some(Component::Client),
        Some("update-host.json") => Some(Component::Host),
        _ => None,
    };
    match component {
        Some(component) => {
            verify_component(bytes, signature, key, component).map(|m| (m.tag, m.artifacts.len()))
        }
        None => verify(bytes, signature, key).map(|m| (m.tag, m.artifacts.len())),
    }
    .map_err(io::Error::other)
}

fn verify(manifest: &Path) -> io::Result<()> {
    let bytes = fs::read(manifest)?;
    let mut target = manifest.as_os_str().to_owned();
    target.push(".sig");
    let signature = fs::read(&target)?;
    let key = plait_core::update::public_key().map_err(io::Error::other)?;
    let (tag, artifacts) = check(manifest, &bytes, &signature, &key)?;
    println!("{tag} ({artifacts} artifacts) is signed by the Plait release key");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["generate", key] => generate(Path::new(key)),
        ["sign", key, manifest] => sign(Path::new(key), Path::new(manifest)),
        ["verify", manifest] => verify(Path::new(manifest)),
        _ => Err(io::Error::other(
            "usage: update-key generate <key> | sign <key> <manifest> | verify <manifest>",
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("update-key: {error}");
            ExitCode::FAILURE
        }
    }
}
