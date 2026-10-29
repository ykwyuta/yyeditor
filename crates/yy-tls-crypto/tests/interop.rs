use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn certificate(kind: &str) -> CertificateDer<'static> {
    CertificateDer::from_pem_file(fixture(&format!("{kind}.pem"))).unwrap()
}
fn key(name: &str) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from_pem_file(fixture(&format!("{name}.pem"))).unwrap()
}
fn roots() -> Arc<RootCertStore> {
    let mut roots = RootCertStore::empty();
    roots.add(certificate("ca")).unwrap();
    Arc::new(roots)
}
fn connections(
    kind: &str,
    client_provider: CryptoProvider,
    server_provider: CryptoProvider,
    host: &'static str,
) -> (ClientConnection, ServerConnection) {
    let client_provider = Arc::new(client_provider);
    let server_provider = Arc::new(server_provider);
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        roots(),
        server_provider.clone(),
    )
    .build()
    .unwrap();
    let server = ServerConfig::builder_with_provider(server_provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![certificate(kind)], key(&format!("{kind}-key")))
        .unwrap();
    let mut client = ClientConfig::builder_with_provider(client_provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots())
        .with_client_auth_cert(vec![certificate(kind)], key(&format!("{kind}-key")))
        .unwrap();
    client.enable_secret_extraction = true;
    (
        ClientConnection::new(Arc::new(client), ServerName::try_from(host).unwrap()).unwrap(),
        ServerConnection::new(Arc::new(server)).unwrap(),
    )
}
fn pump(client: &mut ClientConnection, server: &mut ServerConnection) -> Result<(), rustls::Error> {
    for _ in 0..64 {
        let mut progress = false;
        if client.wants_write() {
            let mut bytes = Vec::new();
            client.write_tls(&mut bytes).unwrap();
            let mut cursor = Cursor::new(bytes);
            while cursor.position() < cursor.get_ref().len() as u64 {
                assert!(server.read_tls(&mut cursor).unwrap() > 0);
                server.process_new_packets()?;
            }
            progress = true;
        }
        if server.wants_write() {
            let mut bytes = Vec::new();
            server.write_tls(&mut bytes).unwrap();
            let mut cursor = Cursor::new(bytes);
            while cursor.position() < cursor.get_ref().len() as u64 {
                assert!(client.read_tls(&mut cursor).unwrap() > 0);
                client.process_new_packets()?;
            }
            progress = true;
        }
        if !progress {
            return Ok(());
        }
    }
    panic!("TLS exchange did not quiesce");
}

#[test]
fn every_suite_group_certificate_and_direction_supports_mutual_tls_and_data() {
    let pure = yy_tls_crypto::provider();
    let native = rustls::crypto::ring::default_provider();
    assert_eq!(pure.cipher_suites.len(), 9);
    assert_eq!(pure.kx_groups.len(), 3);
    let data: Vec<u8> = (0..65_536).map(|i| (i % 251) as u8).collect();
    let mut cases = 0;
    for suite in &pure.cipher_suites {
        let rsa_only = format!("{:?}", suite.suite()).contains("ECDHE_RSA");
        let ecdsa_only = format!("{:?}", suite.suite()).contains("ECDHE_ECDSA");
        for kind in ["rsa", "p256", "p384", "ed25519"] {
            if (rsa_only && kind != "rsa") || (ecdsa_only && kind == "rsa") {
                continue;
            }
            for group in &pure.kx_groups {
                for (client_pure, server_pure) in [(true, false), (false, true), (true, true)] {
                    let restrict = |mut provider: CryptoProvider| {
                        provider
                            .cipher_suites
                            .retain(|s| s.suite() == suite.suite());
                        provider.kx_groups.retain(|g| g.name() == group.name());
                        provider
                    };
                    let (mut client, mut server) = connections(
                        kind,
                        restrict(if client_pure {
                            pure.clone()
                        } else {
                            native.clone()
                        }),
                        restrict(if server_pure {
                            pure.clone()
                        } else {
                            native.clone()
                        }),
                        "localhost",
                    );
                    pump(&mut client, &mut server).unwrap_or_else(|e| panic!("suite={:?} group={:?} cert={kind} pure=({client_pure},{server_pure}): {e:?}", suite.suite(), group.name()));
                    assert!(!client.is_handshaking() && !server.is_handshaking());
                    assert_eq!(
                        client.negotiated_cipher_suite().unwrap().suite(),
                        suite.suite()
                    );
                    assert!(server.peer_certificates().is_some());
                    for chunk in data.chunks(8192) {
                        client.writer().write_all(chunk).unwrap();
                        pump(&mut client, &mut server).unwrap();
                        let mut received = vec![0; chunk.len()];
                        server.reader().read_exact(&mut received).unwrap();
                        assert_eq!(received, chunk);
                        server.writer().write_all(chunk).unwrap();
                        pump(&mut client, &mut server).unwrap();
                        client.reader().read_exact(&mut received).unwrap();
                        assert_eq!(received, chunk);
                    }
                    // Exercise TLS 1.3 KeyUpdate when available, then transfer again.
                    if client.protocol_version() == Some(rustls::ProtocolVersion::TLSv1_3) {
                        client.refresh_traffic_keys().unwrap();
                        pump(&mut client, &mut server).unwrap();
                        client.writer().write_all(b"after key update").unwrap();
                        pump(&mut client, &mut server).unwrap();
                        let mut received = [0; 16];
                        server.reader().read_exact(&mut received).unwrap();
                        assert_eq!(&received, b"after key update");
                    }
                    let secrets = client.dangerous_extract_secrets().unwrap();
                    for (_, secret) in [secrets.tx, secrets.rx] {
                        let (key, iv, algorithm) = match secret {
                            rustls::ConnectionTrafficSecrets::Aes128Gcm { key, iv } => {
                                (key, iv, "AES_128")
                            }
                            rustls::ConnectionTrafficSecrets::Aes256Gcm { key, iv } => {
                                (key, iv, "AES_256")
                            }
                            rustls::ConnectionTrafficSecrets::Chacha20Poly1305 { key, iv } => {
                                (key, iv, "CHACHA20")
                            }
                            _ => panic!("unexpected traffic secret algorithm"),
                        };
                        assert!(format!("{:?}", suite.suite()).contains(algorithm));
                        assert_eq!(
                            key.as_ref().len(),
                            if algorithm == "AES_128" { 16 } else { 32 }
                        );
                        assert_eq!(iv.as_ref().len(), 12);
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 216);
}

#[test]
fn invalid_hostname_and_mismatched_private_key_are_rejected() {
    let pure = yy_tls_crypto::provider();
    let (mut client, mut server) = connections("rsa", pure.clone(), pure.clone(), "wrong.example");
    assert!(matches!(
        pump(&mut client, &mut server),
        Err(rustls::Error::InvalidCertificate(_))
    ));
    assert!(
        rustls::sign::CertifiedKey::from_der(vec![certificate("p256")], key("p384-key"), &pure)
            .is_err()
    );
    assert!(
        pure.key_provider
            .load_private_key(PrivateKeyDer::Pkcs8(vec![0; 32].into()))
            .is_err()
    );
    assert!(
        pure.key_provider
            .load_private_key(key("weak-rsa-key"))
            .is_err()
    );
    use x509_parser::prelude::FromDer;
    let weak = certificate("weak-rsa");
    let (_, weak) = x509_parser::certificate::X509Certificate::from_der(&weak).unwrap();
    for algorithm in pure
        .signature_verification_algorithms
        .all
        .iter()
        .filter(|a| a.signature_alg_id() == rustls::pki_types::alg_id::RSA_PKCS1_SHA256)
    {
        assert!(
            algorithm
                .verify_signature(
                    &weak.public_key().subject_public_key.data,
                    weak.tbs_certificate.as_ref(),
                    &weak.signature_value.data
                )
                .is_err()
        );
    }
}

#[test]
fn native_key_formats_and_all_signature_schemes_interoperate() {
    use x509_parser::prelude::FromDer;
    let pure = yy_tls_crypto::provider();
    let native = rustls::crypto::ring::default_provider();
    for name in [
        "rsa-key",
        "rsa-pkcs1",
        "p256-key",
        "p256-sec1",
        "p384-key",
        "p384-sec1",
        "ed25519-key",
    ] {
        for (signing, verifying) in [(&pure, &native), (&native, &pure)] {
            let signer = signing.key_provider.load_private_key(key(name)).unwrap();
            let spki = signer.public_key().unwrap();
            let (_, spki) =
                x509_parser::x509::SubjectPublicKeyInfo::from_der(spki.as_ref()).unwrap();
            for (scheme, algorithms) in verifying.signature_verification_algorithms.mapping {
                let Some(signer) = signer.choose_scheme(&[*scheme]) else {
                    continue;
                };
                let signature = signer.sign(b"TLS signature interop").unwrap();
                assert!(
                    algorithms.iter().any(|alg| alg
                        .verify_signature(
                            &spki.subject_public_key.data,
                            b"TLS signature interop",
                            &signature
                        )
                        .is_ok()),
                    "{name} {scheme:?}"
                );
                assert!(algorithms.iter().all(|alg| {
                    alg.verify_signature(&spki.subject_public_key.data, b"tampered", &signature)
                        .is_err()
                }));
            }
        }
    }
    // Certificate signature AlgorithmIdentifiers accepted by ring must remain supported.
    for algorithm in native.signature_verification_algorithms.all {
        assert!(
            pure.signature_verification_algorithms
                .all
                .iter()
                .any(
                    |candidate| candidate.public_key_alg_id() == algorithm.public_key_alg_id()
                        && candidate.signature_alg_id() == algorithm.signature_alg_id()
                )
        );
    }
}
