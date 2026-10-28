// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! DKIM keypair generation — RSA-2048 (PKCS1 PEM) and Ed25519 (PKCS8 PEM).
//!
//! Thin wrapper around `mail_auth::dkim::generate::DkimKeyPair` that
//! emits the on-disk shapes [`crate::signer::sign_with_config`] expects
//! to read back, plus the matching `_domainkey` DNS TXT record snippet.
//!
//! Output materials:
//! - `private_pem`: ready to drop into `mixos:var/maild/dkim/...`
//! - `public_b64`: raw base64 (no DKIM wrapping), exposed so callers
//!   can rebuild a custom TXT record if `dns_txt_record` doesn't fit.
//! - `dns_txt_record`: the full `v=DKIM1;... p=...` value that goes
//!   into the `<selector>._domainkey.<domain>` TXT record.
//!
//! `t=s` is NOT included by default: the signer's parent-label walk-up
//! (`MailAuthSigner::lookup_active`) signs subdomain mail with the
//! org-domain key, which `t=s` would reject. Callers that pin one
//! selector to one exact domain can append `; t=s` themselves.

use crate::error::{Error, Result};
use crate::types::DkimAlgorithm;
use mail_auth::dkim::generate::DkimKeyPair;
use rsa::{pkcs1::DecodeRsaPublicKey, pkcs8::EncodePublicKey};

/// What to generate. Bits for RSA must be ≥ 1024 — 2048 is the modern
/// default; 1024 is still accepted for legacy interop but flagged by
/// receivers as weak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySpec {
    Rsa { bits: usize },
    Ed25519,
}

#[derive(Debug, Clone)]
pub struct DkimKeyMaterial {
    pub algorithm: DkimAlgorithm,
    pub private_pem: String,
    pub public_b64: String,
    pub dns_txt_record: String,
}

/// Generate a fresh keypair. RSA uses the `rsa` crate (mail-auth pulls
/// it; `ring` doesn't yet do RSA keygen — see the upstream `//TODO`
/// in `mail_auth::dkim::generate`). Ed25519 routes through `ring` via
/// `mail_auth::common::crypto::Ed25519Key::generate_pkcs8`.
pub fn generate(spec: KeySpec) -> Result<DkimKeyMaterial> {
    match spec {
        KeySpec::Rsa { bits } => {
            if bits < 1024 {
                return Err(Error::KeyLoad(format!(
                    "RSA key size {bits} is below the 1024-bit floor"
                )));
            }
            let pair = DkimKeyPair::generate_rsa(bits)
                .map_err(|e| Error::KeyLoad(format!("RSA keygen: {e}")))?;
            let private_pem = pem_encode("RSA PRIVATE KEY", pair.private_key());
            let key = rsa::RsaPublicKey::from_pkcs1_der(pair.public_key())
                .map_err(|e| Error::KeyLoad(format!("RSA public key: {e}")))?;
            let public = key
                .to_public_key_der()
                .map_err(|e| Error::KeyLoad(format!("RSA SPKI: {e}")))?;
            let public_b64 = base64_encode(public.as_bytes());
            let dns_txt_record = format!("v=DKIM1; k=rsa; p={public_b64}");
            Ok(DkimKeyMaterial {
                algorithm: DkimAlgorithm::RsaSha256,
                private_pem,
                public_b64,
                dns_txt_record,
            })
        }
        KeySpec::Ed25519 => {
            let pair = DkimKeyPair::generate_ed25519()
                .map_err(|e| Error::KeyLoad(format!("Ed25519 keygen: {e}")))?;
            let private_pem = pem_encode("PRIVATE KEY", pair.private_key());
            let public_b64 = pair.encoded_public_key();
            let dns_txt_record = format!("v=DKIM1; k=ed25519; p={public_b64}");
            Ok(DkimKeyMaterial {
                algorithm: DkimAlgorithm::Ed25519Sha256,
                private_pem,
                public_b64,
                dns_txt_record,
            })
        }
    }
}

/// Derive the DNS key from the installed PEM, without generating a new key.
pub fn public_record(pem: &[u8]) -> Result<String> {
    use rsa::{pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey};
    use rustls_pki_types::{PrivateKeyDer, pem::PemObject};
    let der = PrivateKeyDer::from_pem_slice(pem)
        .map_err(|e| Error::KeyLoad(format!("private PEM: {e}")))?;
    if crate::signer::detect_algorithm(pem)? == DkimAlgorithm::Ed25519Sha256 {
        let PrivateKeyDer::Pkcs8(der) = der else {
            return Err(Error::KeyLoad("Ed25519 requires PKCS8".into()));
        };
        let key = mail_auth::common::crypto::Ed25519Key::from_pkcs8_maybe_unchecked_der(
            der.secret_pkcs8_der(),
        )
        .map_err(|e| Error::KeyLoad(format!("Ed25519 private key: {e}")))?;
        return Ok(format!(
            "v=DKIM1; k=ed25519; p={}",
            base64_encode(&key.public_key())
        ));
    }
    let key = match der {
        PrivateKeyDer::Pkcs1(der) => rsa::RsaPrivateKey::from_pkcs1_der(der.secret_pkcs1_der())
            .map_err(|e| Error::KeyLoad(e.to_string()))?,
        PrivateKeyDer::Pkcs8(der) => rsa::RsaPrivateKey::from_pkcs8_der(der.secret_pkcs8_der())
            .map_err(|e| Error::KeyLoad(e.to_string()))?,
        _ => return Err(Error::KeyLoad("unsupported RSA private key".into())),
    };
    let public = key
        .to_public_key()
        .to_public_key_der()
        .map_err(|e| Error::KeyLoad(e.to_string()))?;
    Ok(format!(
        "v=DKIM1; k=rsa; p={}",
        base64_encode(public.as_bytes())
    ))
}

/// Compare the published key bytes and algorithm; TXT chunks are joined by DNS.
/// RSA must be SPKI: a legacy PKCS1 record is reported as a mismatch.
pub fn record_matches(pem: &[u8], txt: &str) -> Result<bool> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    fn tags(txt: &str) -> Option<std::collections::BTreeMap<&str, &str>> {
        let mut tags = std::collections::BTreeMap::new();
        for tag in txt.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (name, value) = tag.split_once('=')?;
            if tags.insert(name.trim(), value.trim()).is_some() {
                return None;
            }
        }
        Some(tags)
    }
    let expected = public_record(pem)?;
    let Some(want) = tags(&expected) else {
        unreachable!()
    };
    let Some(got) = tags(txt) else {
        return Ok(false);
    };
    let decode = |value: Option<&&str>| {
        value.and_then(|s| {
            STANDARD
                .decode(
                    s.chars()
                        .filter(|c| !c.is_ascii_whitespace())
                        .collect::<String>(),
                )
                .ok()
        })
    };
    Ok(got.get("v").is_none_or(|v| *v == "DKIM1")
        && got.get("k").copied().unwrap_or("rsa") == want["k"]
        && decode(got.get("p")).is_some_and(|key| Some(key) == decode(want.get("p"))))
}

/// Wrap DER bytes in a 64-char-wrapped PEM envelope. `label` is the
/// `-----BEGIN <label>-----` content (e.g. `"RSA PRIVATE KEY"` for
/// PKCS1, `"PRIVATE KEY"` for PKCS8). Matches OpenSSL's output so
/// `rustls_pki_types::PrivateKeyDer::from_pem_slice` round-trips.
fn pem_encode(label: &str, der: &[u8]) -> String {
    use std::fmt::Write as _;
    let b64 = base64_encode(der);
    let mut out = String::with_capacity(b64.len() + 64);
    writeln!(out, "-----BEGIN {label}-----").unwrap();
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    writeln!(out, "-----END {label}-----").unwrap();
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let chunks = bytes.chunks_exact(3);
    let remainder = chunks.remainder();
    for c in chunks {
        let n = (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2]);
        for shift in [18, 12, 6, 0] {
            out.push(ALPHA[((n >> shift) & 0x3f) as usize] as char);
        }
    }
    match remainder.len() {
        1 => {
            let n = u32::from(remainder[0]) << 16;
            out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
            out.push_str("==");
        }
        2 => {
            let n = (u32::from(remainder[0]) << 16) | (u32::from(remainder[1]) << 8);
            out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
            out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::{MailAuthSigner, Signer};
    use crate::types::{Canon, DkimSignerConfig, Domain, HeaderSpec, default_signed_headers};

    /// Satisfy the verifier's real DNS lookup with the exact printed record.
    /// Any cache miss would be a test failure; no resolver traffic is needed.
    struct PublishedKey(mail_auth::Txt);

    impl mail_auth::ResolverCache<String, mail_auth::Txt> for PublishedKey {
        fn get<Q>(&self, _: &Q) -> Option<mail_auth::Txt>
        where
            String: std::borrow::Borrow<Q>,
            Q: std::hash::Hash + Eq + ?Sized,
        {
            Some(self.0.clone())
        }

        fn remove<Q>(&self, _: &Q) -> Option<mail_auth::Txt>
        where
            String: std::borrow::Borrow<Q>,
            Q: std::hash::Hash + Eq + ?Sized,
        {
            None
        }

        fn insert(&self, _: String, _: mail_auth::Txt, _: std::time::Instant) {
            panic!("published record must satisfy the DNS lookup");
        }
    }

    fn verify_printed_record(txt: &str, message: &[u8]) {
        use mail_auth::{
            AuthenticatedMessage, DkimResult, MessageAuthenticator, Parameters,
            common::{parse::TxtRecordParser, verify::DomainKey},
        };
        let cache = PublishedKey(mail_auth::Txt::DomainKey(std::sync::Arc::new(
            DomainKey::parse(txt.as_bytes()).unwrap(),
        )));
        let parsed = AuthenticatedMessage::parse(message).unwrap();
        let auth = MessageAuthenticator::new_cloudflare().unwrap();
        let outcomes = tokio_test_block_on(
            auth.verify_dkim(Parameters::new(&parsed).with_txt_cache(&cache)),
        );
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].result(), &DkimResult::Pass);
    }

    #[test]
    fn printed_rsa_spki_parses_independently_and_verifies_generated_signature() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use rsa::{
            pkcs1::DecodeRsaPrivateKey,
            pkcs8::DecodePublicKey,
            signature::{Signer as _, Verifier as _},
        };
        use x509_parser::prelude::FromDer;
        let mat = generate(KeySpec::Rsa { bits: 2048 }).unwrap();
        let encoded = mat.dns_txt_record.split("p=").nth(1).unwrap();
        let der = STANDARD.decode(encoded).unwrap();
        let (remaining, parsed) = x509_parser::x509::SubjectPublicKeyInfo::from_der(&der).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(
            parsed.algorithm.algorithm.to_id_string(),
            "1.2.840.113549.1.1.1"
        );
        assert!(parsed.parsed().is_ok());
        let private = rsa::RsaPrivateKey::from_pkcs1_pem(&mat.private_pem).unwrap();
        let signer = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new_unprefixed(private);
        let signature = signer.sign(b"generated DKIM key round trip");
        let public = rsa::RsaPublicKey::from_public_key_der(&der).unwrap();
        rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new_unprefixed(public)
            .verify(b"generated DKIM key round trip", &signature)
            .unwrap();
        assert!(record_matches(mat.private_pem.as_bytes(), &mat.dns_txt_record).unwrap());
        let wrong = generate(KeySpec::Rsa { bits: 2048 }).unwrap();
        assert!(!record_matches(mat.private_pem.as_bytes(), &wrong.dns_txt_record).unwrap());
        assert!(!record_matches(mat.private_pem.as_bytes(), "v=DKIM1; p=").unwrap());
    }

    #[test]
    fn rsa_2048_round_trips_into_signer() {
        let mat = generate(KeySpec::Rsa { bits: 2048 }).expect("keygen");
        assert!(
            mat.private_pem
                .starts_with("-----BEGIN RSA PRIVATE KEY-----")
        );
        assert!(
            mat.private_pem
                .trim_end()
                .ends_with("-----END RSA PRIVATE KEY-----")
        );
        assert!(mat.dns_txt_record.starts_with("v=DKIM1; k=rsa; p="));
        assert!(!mat.public_b64.is_empty());

        // Feed the freshly-minted key straight into the signer — proves
        // the PEM envelope matches what `PrivateKeyDer::from_pem_slice`
        // accepts and the key works end-to-end.
        let cfg = DkimSignerConfig {
            domain: Domain::new("example.org"),
            selector: "k1".into(),
            algorithm: mat.algorithm,
            private_key_pem: mat.private_pem.into_bytes(),
            canonicalization: (Canon::Relaxed, Canon::Relaxed),
            headers_to_sign: default_signed_headers(),
            active_for_signing: true,
            allow_body_length_tag: false,
        };
        let signer = MailAuthSigner::new(vec![cfg]);
        let mut msg = b"From: a@example.org\r\nTo: b@example.org\r\n\
            Subject: t\r\nDate: Tue, 19 May 2026 10:00:00 +1000\r\n\r\nhi\r\n"
            .to_vec();
        tokio_test_block_on(signer.sign_dkim(&mut msg, &Domain::new("example.org")))
            .expect("sign with generated rsa key");
        assert!(msg.starts_with(b"DKIM-Signature: "));
        verify_printed_record(&mat.dns_txt_record, &msg);
    }

    #[test]
    fn ed25519_round_trips_into_signer() {
        let mat = generate(KeySpec::Ed25519).expect("keygen");
        assert!(mat.private_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(mat.dns_txt_record.starts_with("v=DKIM1; k=ed25519; p="));
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&mat.public_b64)
                .unwrap()
                .len(),
            32
        );
        assert!(record_matches(mat.private_pem.as_bytes(), &mat.dns_txt_record).unwrap());

        let cfg = DkimSignerConfig {
            domain: Domain::new("example.org"),
            selector: "ed1".into(),
            algorithm: mat.algorithm,
            private_key_pem: mat.private_pem.into_bytes(),
            canonicalization: (Canon::Relaxed, Canon::Relaxed),
            headers_to_sign: vec![
                HeaderSpec::Oversign("From".into()),
                HeaderSpec::Single("Subject".into()),
            ],
            active_for_signing: true,
            allow_body_length_tag: false,
        };
        let signer = MailAuthSigner::new(vec![cfg]);
        let mut msg = b"From: a@example.org\r\nSubject: t\r\n\r\nhi\r\n".to_vec();
        tokio_test_block_on(signer.sign_dkim(&mut msg, &Domain::new("example.org")))
            .expect("sign with generated ed25519 key");
        assert!(msg.starts_with(b"DKIM-Signature: "));
        verify_printed_record(&mat.dns_txt_record, &msg);
    }

    #[test]
    fn rsa_below_floor_rejected() {
        let err = generate(KeySpec::Rsa { bits: 512 }).unwrap_err();
        match err {
            Error::KeyLoad(m) => assert!(m.contains("512"), "msg: {m}"),
            other => panic!("expected KeyLoad, got {other:?}"),
        }
    }

    #[test]
    fn pem_lines_wrap_at_64_chars() {
        let der = vec![0xAB; 200];
        let pem = pem_encode("TEST", &der);
        for line in pem.lines() {
            if line.starts_with("-----") {
                continue;
            }
            assert!(line.len() <= 64, "line too long: {line:?} ({})", line.len());
        }
    }

    #[test]
    fn base64_encode_matches_known_vector() {
        // RFC 4648 vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    /// Tiny synchronous executor for the few tokio-typed sign_dkim
    /// calls — keeps this module free of #[tokio::test] when only the
    /// signer is async.
    fn tokio_test_block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }
}
