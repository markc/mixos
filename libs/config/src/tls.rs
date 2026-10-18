// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Shared node TLS loading and SNI identity selection (feature `tls`).

pub mod sni;
pub use sni::{SniCertResolver, TlsIdentity, pick_identity};

/// Load a PEM chain and private key into a server acceptor without client auth.
/// Installs the ring provider on first call. Matches the single-identity loader
/// used by existing services, including its PEM filtering behaviour.
pub fn load_tls_config(cert_path: &str, key_path: &str) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
    use std::sync::Arc;

    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert_data = std::fs::read(cert_path)?;
    let key_data = std::fs::read(key_path)?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_data[..])
        .filter_map(|r| r.ok())
        .collect();
    let key = rustls_pemfile::private_key(&mut &key_data[..])
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("no private key found in {key_path}"))?;
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(tls_config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Arc;

    fn pair(dir: &std::path::Path) -> (String, String, rustls::pki_types::CertificateDer<'static>) {
        let pair = rcgen::generate_simple_self_signed(vec!["mail.example.org".into()]).unwrap();
        let cert = dir.join("cert.pem");
        let key = dir.join("key.pem");
        std::fs::write(&cert, pair.cert.pem()).unwrap();
        std::fs::write(&key, pair.key_pair.serialize_pem()).unwrap();
        (cert.to_str().unwrap().into(), key.to_str().unwrap().into(), pair.cert.der().clone())
    }

    fn client(cert: rustls::pki_types::CertificateDer<'static>, name: &str, sni: bool) -> rustls::ClientConnection {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert).unwrap();
        let mut config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        config.enable_sni = sni;
        rustls::ClientConnection::new(Arc::new(config), name.to_owned().try_into().unwrap()).unwrap()
    }

    #[test]
    fn single_cert_loader_completes_a_verified_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, der) = pair(dir.path());
        let acceptor = load_tls_config(&cert, &key).unwrap();
        let mut server = rustls::ServerConnection::new(acceptor.config().clone()).unwrap();
        let mut client = client(der.clone(), "mail.example.org", true);
        for _ in 0..10 {
            let mut wire = Vec::new();
            client.write_tls(&mut wire).unwrap();
            server.read_tls(&mut Cursor::new(wire)).unwrap();
            server.process_new_packets().unwrap();
            let mut wire = Vec::new();
            server.write_tls(&mut wire).unwrap();
            client.read_tls(&mut Cursor::new(wire)).unwrap();
            client.process_new_packets().unwrap();
            if !client.is_handshaking() && !server.is_handshaking() {
                break;
            }
        }
        assert!(!client.is_handshaking());
        assert!(!server.is_handshaking());
        assert_eq!(client.peer_certificates().unwrap(), &[der]);
        assert_eq!(server.server_name(), Some("mail.example.org"));
    }

    #[test]
    fn single_cert_loader_rejects_missing_and_invalid_material() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, _) = pair(dir.path());
        assert!(load_tls_config("/nonexistent/cert.pem", &key).is_err());
        assert!(load_tls_config(&cert, "/nonexistent/key.pem").is_err());
        std::fs::write(&key, "not a key").unwrap();
        let error = load_tls_config(&cert, &key).err().unwrap();
        assert!(error.to_string().contains(&key));
        let (_, key, _) = pair(dir.path());
        std::fs::write(&cert, "not a certificate").unwrap();
        assert!(load_tls_config(&cert, &key).is_err());
    }

    #[test]
    fn real_client_hello_uses_sni_policy() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, der) = pair(dir.path());
        let identity = crate::node::TlsIdentityConfig {
            server_name: "Mail.Example.Org".into(), cert, key,
            default: true, no_sni_fallback: true,
        };
        for (strict, name, sni, accepted) in [
            (false, "unknown.example.org", true, true),
            (false, "mail.example.org", false, true),
            (true, "MAIL.EXAMPLE.ORG", true, true),
            (true, "unknown.example.org", true, false),
            (true, "mail.example.org", false, false),
        ] {
            let resolver = SniCertResolver::from_config(std::slice::from_ref(&identity), strict).unwrap();
            assert_eq!(resolver.strict_sni(), strict);
            assert_eq!(resolver.default_identity().unwrap().name, "mail.example.org");
            let config = rustls::ServerConfig::builder().with_no_client_auth().with_cert_resolver(Arc::new(resolver));
            let mut server = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            let mut client = client(der.clone(), name, sni);
            let mut wire = Vec::new();
            client.write_tls(&mut wire).unwrap();
            server.read_tls(&mut Cursor::new(wire)).unwrap();
            assert_eq!(server.process_new_packets().is_ok(), accepted, "strict={strict}, name={name}, sni={sni}");
        }
    }
}
