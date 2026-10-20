// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Node-local WireGuard key bootstrap (X25519). No secret output.
//!
//! Only the key part of the predecessor's WireGuard library is ported: generation, clamping,
//! standard base64 and public-key derivation. The wire, netlink and QR parts
//! of that crate are not needed by the signer and are not ported. Derivation
//! uses `curve25519-dalek`, the same primitive `hostd` uses for WireGuard keys.
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use curve25519_dalek::montgomery::MontgomeryPoint;
use rand::RngCore as _;
use rand::rngs::OsRng;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

const KEY_LEN: usize = 32;

/// Curve25519 scalar clamping, as `wg genkey` applies it.
fn clamp(mut bytes: [u8; KEY_LEN]) -> [u8; KEY_LEN] {
    bytes[0] &= 0b1111_1000;
    bytes[31] &= 0b0111_1111;
    bytes[31] |= 0b0100_0000;
    bytes
}

/// The X25519 public key for a private scalar (clamped on the way in).
fn public_of(private: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    MontgomeryPoint::mul_base_clamped(*private).to_bytes()
}

/// Decode a standard-base64 32-byte WireGuard private key (pads required).
fn decode_private(s: &str) -> Result<[u8; KEY_LEN]> {
    let raw = B64.decode(s.trim()).context("invalid base64")?;
    let bytes: [u8; KEY_LEN] = raw
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid key length"))?;
    Ok(clamp(bytes))
}

pub fn generate(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        bail!("private-file must be absolute");
    }
    let mut seed = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut seed);
    let private = clamp(seed);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("create new private key file (existing keys are never replaced)")?;
    writeln!(file, "{}", B64.encode(private)).context("write private key file")?;
    file.sync_all().context("sync private key file")?;
    File::open(path.parent().context("private file has no parent")?)?
        .sync_all()
        .context("sync private key directory")?;
    Ok(B64.encode(public_of(&private)))
}

pub fn public_key(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        bail!("private-file must be absolute");
    }
    // O_NONBLOCK avoids hanging on a FIFO before metadata rejects it.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("open protected private key file")?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        bail!("private key must be a regular file with no group/other permissions");
    }
    let mut encoded = String::new();
    file.take(129)
        .read_to_string(&mut encoded)
        .context("read private key file")?;
    if encoded.len() > 128 {
        bail!("private key encoding is too large");
    }
    let private =
        decode_private(&encoded).map_err(|_| anyhow::anyhow!("invalid private key encoding"))?;
    Ok(B64.encode(public_of(&private)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex32(s: &str) -> [u8; KEY_LEN] {
        let mut out = [0u8; KEY_LEN];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn public_key_matches_the_rfc_7748_vector() {
        // RFC 7748 §6.1, Alice.
        let private = hex32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let public = hex32("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        assert_eq!(public_of(&private), public);
    }

    #[test]
    fn node_key_is_private_create_only_and_recoverable() {
        let dir = std::env::temp_dir().join(format!(
            "mesh-sign-wg-key-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("wg.key");
        let public = generate(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
        assert_eq!(public_key(&path).unwrap(), public);
        assert!(generate(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(public_key(&link).is_err());
        assert!(generate(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(public_key(&path).is_err());
        assert!(generate(Path::new("relative.key")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
