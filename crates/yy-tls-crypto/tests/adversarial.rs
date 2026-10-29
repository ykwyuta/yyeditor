//! Negative tests use ring as an independent acceptance oracle, including AES-128
//! keys obtained through the public traffic-secret API rather than fabricated keys.
use std::io::{Cursor, Write};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;
use rustls::crypto::cipher::{InboundOpaqueMessage, MessageDecrypter};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject};
use rustls::{
    ClientConfig, ClientConnection, ConnectionTrafficSecrets, RootCertStore, ServerConfig,
    ServerConnection, SupportedCipherSuite,
};
use x509_parser::prelude::FromDer;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.pem"))
}

fn transfer(client: &mut ClientConnection, server: &mut ServerConnection) {
    for _ in 0..32 {
        let mut progress = false;
        if client.wants_write() {
            let mut wire = Vec::new();
            client.write_tls(&mut wire).unwrap();
            server.read_tls(&mut Cursor::new(wire)).unwrap();
            server.process_new_packets().unwrap();
            progress = true;
        }
        if server.wants_write() {
            let mut wire = Vec::new();
            server.write_tls(&mut wire).unwrap();
            client.read_tls(&mut Cursor::new(wire)).unwrap();
            client.process_new_packets().unwrap();
            progress = true;
        }
        if !progress {
            assert!(!client.is_handshaking() && !server.is_handshaking());
            return;
        }
    }
    panic!("handshake did not quiesce");
}

fn pair(suite: SupportedCipherSuite) -> (ClientConnection, ServerConnection) {
    let restrict = |mut provider: CryptoProvider| {
        provider
            .cipher_suites
            .retain(|s| s.suite() == suite.suite());
        Arc::new(provider)
    };
    let cert = CertificateDer::from_pem_file(fixture("p256")).unwrap();
    // TLS 1.2 RSA suites require an RSA certificate.
    let rsa = format!("{:?}", suite.suite()).contains("ECDHE_RSA");
    let kind = if rsa { "rsa" } else { "p256" };
    let cert = if rsa {
        CertificateDer::from_pem_file(fixture(kind)).unwrap()
    } else {
        cert
    };
    let key = PrivateKeyDer::from_pem_file(fixture(&format!("{kind}-key"))).unwrap();
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_file(fixture("ca")).unwrap())
        .unwrap();
    let mut client =
        ClientConfig::builder_with_provider(restrict(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    client.enable_secret_extraction = true;
    let mut server = ServerConfig::builder_with_provider(restrict(yy_tls_crypto::provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    server.enable_secret_extraction = true;
    // Avoid unconsumed tickets so sequence numbers refer to application records.
    server.send_tls13_tickets = 0;
    let mut client =
        ClientConnection::new(Arc::new(client), ServerName::try_from("localhost").unwrap())
            .unwrap();
    let mut server = ServerConnection::new(Arc::new(server)).unwrap();
    transfer(&mut client, &mut server);
    (client, server)
}

fn decoder(
    suite: SupportedCipherSuite,
    secret: ConnectionTrafficSecrets,
) -> Box<dyn MessageDecrypter> {
    let (key, iv) = match secret {
        ConnectionTrafficSecrets::Aes128Gcm { key, iv }
        | ConnectionTrafficSecrets::Aes256Gcm { key, iv }
        | ConnectionTrafficSecrets::Chacha20Poly1305 { key, iv } => (key, iv),
        _ => panic!("unexpected traffic secret"),
    };
    match suite {
        SupportedCipherSuite::Tls13(s) => s.aead_alg.decrypter(key, iv),
        SupportedCipherSuite::Tls12(s) => s.aead_alg.decrypter(
            key,
            &iv.as_ref()[..s.aead_alg.key_block_shape().fixed_iv_len],
        ),
    }
}

#[test]
fn every_suite_authenticates_every_record_byte_and_tls12_headers() {
    let native = rustls::crypto::ring::default_provider();
    let mut suites = 0;
    for suite in yy_tls_crypto::provider().cipher_suites {
        let (mut client, server) = pair(suite);
        let inbound = server.dangerous_extract_secrets().unwrap().rx;
        client
            .writer()
            .write_all(b"authenticated application content")
            .unwrap();
        let mut wire = Vec::new();
        client.write_tls(&mut wire).unwrap();
        assert_eq!(
            wire.len(),
            5 + u16::from_be_bytes([wire[3], wire[4]]) as usize
        );
        let outbound = client.dangerous_extract_secrets().unwrap().tx;
        assert_eq!(inbound.0, outbound.0 - 1);
        let reference = *native
            .cipher_suites
            .iter()
            .find(|s| s.suite() == suite.suite())
            .unwrap();
        let mut ours = decoder(suite, inbound.1);
        let mut theirs = decoder(reference, outbound.1);
        let typ = rustls::ContentType::from(wire[0]);
        let version = rustls::ProtocolVersion::from(u16::from_be_bytes([wire[1], wire[2]]));
        let seq = inbound.0;
        let original = &wire[5..];
        for decrypter in [&mut ours, &mut theirs] {
            let mut valid = original.to_vec();
            assert_eq!(
                decrypter
                    .decrypt(InboundOpaqueMessage::new(typ, version, &mut valid), seq)
                    .unwrap()
                    .payload,
                b"authenticated application content"
            );
            for offset in 0..original.len() {
                let mut bytes = original.to_vec();
                bytes[offset] ^= 1;
                assert!(
                    decrypter
                        .decrypt(InboundOpaqueMessage::new(typ, version, &mut bytes), seq)
                        .is_err(),
                    "{:?} byte={offset}",
                    suite.suite()
                );
            }
            for len in 0..original.len() {
                let mut bytes = original[..len].to_vec();
                assert!(
                    decrypter
                        .decrypt(InboundOpaqueMessage::new(typ, version, &mut bytes), seq)
                        .is_err()
                );
            }
            let mut extended = original.to_vec();
            extended.push(0);
            assert!(
                decrypter
                    .decrypt(InboundOpaqueMessage::new(typ, version, &mut extended), seq)
                    .is_err()
            );
            let mut replay = original.to_vec();
            assert!(
                decrypter
                    .decrypt(
                        InboundOpaqueMessage::new(typ, version, &mut replay),
                        seq + 1
                    )
                    .is_err()
            );
            if matches!(suite, SupportedCipherSuite::Tls12(_)) {
                for (changed_type, changed_version) in [
                    (rustls::ContentType::Alert, version),
                    (typ, rustls::ProtocolVersion::TLSv1_1),
                ] {
                    let mut bytes = original.to_vec();
                    assert!(
                        decrypter
                            .decrypt(
                                InboundOpaqueMessage::new(
                                    changed_type,
                                    changed_version,
                                    &mut bytes
                                ),
                                seq
                            )
                            .is_err()
                    );
                }
            }
        }
        suites += 1;
    }
    assert_eq!(suites, 9);
}

fn hex(input: &str) -> Vec<u8> {
    assert_eq!(input.len() % 2, 0);
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn tls13_authenticated_padding_and_missing_inner_type() {
    use rustls::crypto::cipher::OutboundPlainMessage;
    for suite in yy_tls_crypto::provider().cipher_suites {
        let SupportedCipherSuite::Tls13(pure_suite) = suite else {
            continue;
        };
        let native = rustls::crypto::ring::default_provider();
        let native_suite = native
            .cipher_suites
            .iter()
            .find(|s| s.suite() == suite.suite())
            .unwrap()
            .tls13()
            .unwrap();
        for payload in [
            b"body\x17\0\0\0".to_vec(),
            b"\x17".to_vec(),
            vec![0; 32],
            vec![],
            vec![0x17; 16385],
        ] {
            let (client, server) = pair(suite);
            let inbound = server.dangerous_extract_secrets().unwrap().rx;
            let outbound = client.dangerous_extract_secrets().unwrap().tx;
            assert_eq!(inbound.0, outbound.0);
            let (key, iv) = match outbound.1 {
                ConnectionTrafficSecrets::Aes128Gcm { key, iv }
                | ConnectionTrafficSecrets::Aes256Gcm { key, iv }
                | ConnectionTrafficSecrets::Chacha20Poly1305 { key, iv } => (key, iv),
                _ => panic!("unexpected traffic secret"),
            };
            let record = native_suite
                .aead_alg
                .encrypter(key, iv)
                .encrypt(
                    OutboundPlainMessage {
                        // Encrypt adds a trailing zero, deliberately treated as padding.
                        typ: rustls::ContentType::from(0),
                        version: rustls::ProtocolVersion::TLSv1_2,
                        payload: payload.as_slice().into(),
                    },
                    inbound.0,
                )
                .unwrap();
            let mut bytes = record.payload.as_ref().to_vec();
            let decrypted = decoder(SupportedCipherSuite::Tls13(pure_suite), inbound.1).decrypt(
                InboundOpaqueMessage::new(record.typ, record.version, &mut bytes),
                inbound.0,
            );
            if payload.is_empty() || payload.iter().all(|b| *b == 0) || payload.len() > 16384 {
                assert!(decrypted.is_err());
            } else {
                let decrypted = decrypted.unwrap();
                assert_eq!(decrypted.typ, rustls::ContentType::ApplicationData);
                assert_eq!(
                    decrypted.payload,
                    if payload[0] == b'b' {
                        &b"body"[..]
                    } else {
                        &b""[..]
                    }
                );
            }
        }
    }
}

#[test]
fn rfc8032_ed25519_vector_and_noncanonical_signatures() {
    // RFC 8032 section 7.1 TEST 1 (empty message), not generated by this backend.
    let public = hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
    let signature = hex(concat!(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155",
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    ));
    for (is_pure, provider) in [
        (true, yy_tls_crypto::provider()),
        (false, rustls::crypto::ring::default_provider()),
    ] {
        let algorithm = provider
            .signature_verification_algorithms
            .mapping
            .iter()
            .find(|(s, _)| *s == rustls::SignatureScheme::ED25519)
            .unwrap()
            .1[0];
        algorithm
            .verify_signature(&public, b"", &signature)
            .unwrap();
        let mut attacks = vec![
            vec![0; 64],
            vec![0xff; 64],
            signature[..63].to_vec(),
            [signature.as_slice(), &[0]].concat(),
        ];
        for position in 0..64 {
            let mut bad = signature.clone();
            bad[position] ^= 1;
            attacks.push(bad);
        }
        let mut noncanonical = signature.clone();
        noncanonical[32..].fill(0xff);
        attacks.push(noncanonical);
        for bad in attacks {
            assert!(algorithm.verify_signature(&public, b"", &bad).is_err());
        }
        for bad_public in [
            vec![0; 32],
            vec![1; 32],
            vec![0xff; 32],
            public[..31].to_vec(),
            [public.as_slice(), &[0]].concat(),
        ] {
            assert!(
                algorithm
                    .verify_signature(&bad_public, b"", &signature)
                    .is_err()
            );
        }
        // Identity public key and R with S=0 is an algebraic forgery for lax verifiers.
        let mut identity = [0; 32];
        identity[0] = 1;
        let mut forgery = [0; 64];
        forgery[0] = 1;
        // ring accepts this degenerate key; strict RustCrypto must reject it.
        assert_eq!(
            algorithm
                .verify_signature(&identity, b"forged", &forgery)
                .is_err(),
            is_pure
        );
    }
}

#[test]
fn low_order_x25519_aliases_and_non_tls_sec1_encodings_are_rejected() {
    for provider in [
        yy_tls_crypto::provider(),
        rustls::crypto::ring::default_provider(),
    ] {
        for group in &provider.kx_groups {
            if group.name() == rustls::NamedGroup::X25519 {
                let mut identity = vec![0; 32];
                identity[0] = 1;
                let mut low_order = vec![vec![0; 32], identity];
                // p-1, p, p+1 (p = 2^255-19), including their masked-high-bit aliases.
                for first in [0xec, 0xed, 0xee] {
                    let mut point = vec![0xff; 32];
                    point[0] = first;
                    point[31] = 0x7f;
                    low_order.push(point);
                }
                for point in low_order {
                    for high_bit in [0, 0x80] {
                        let mut point = point.clone();
                        point[31] |= high_bit;
                        assert!(group.start().unwrap().complete(&point).is_err());
                    }
                }
            } else {
                let valid = group.start().unwrap().pub_key().to_vec();
                let mut malformed = vec![vec![0], vec![4], vec![0xff; valid.len()]];
                for prefix in [0, 2, 3, 6, 7] {
                    let mut point = valid.clone();
                    point[0] = prefix;
                    malformed.push(point);
                }
                let mut compressed = valid[..valid.len().div_ceil(2)].to_vec();
                compressed[0] = 2 | (valid.last().unwrap() & 1);
                malformed.push(compressed);
                // A valid point for the other named curve must not be reinterpreted.
                let other = provider
                    .kx_groups
                    .iter()
                    .find(|g| g.name() != group.name() && g.name() != rustls::NamedGroup::X25519)
                    .unwrap();
                malformed.push(other.start().unwrap().pub_key().to_vec());
                for point in malformed {
                    assert!(group.start().unwrap().complete(&point).is_err());
                }
            }
        }
    }
}

#[test]
fn signatures_reject_trailing_bytes_truncation_wrong_curves_and_wrong_schemes() {
    let pure = yy_tls_crypto::provider();
    let native = rustls::crypto::ring::default_provider();
    for kind in ["rsa", "p256", "p384", "ed25519"] {
        let key = PrivateKeyDer::from_pem_file(fixture(&format!("{kind}-key"))).unwrap();
        let signing_key = native.key_provider.load_private_key(key).unwrap();
        let cert = CertificateDer::from_pem_file(fixture(kind)).unwrap();
        let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&cert).unwrap();
        let public = cert.public_key().subject_public_key.data.as_ref();
        for (scheme, _) in native.signature_verification_algorithms.mapping {
            let Some(signer) = signing_key.choose_scheme(&[*scheme]) else {
                continue;
            };
            let signature = signer.sign(b"scheme separation").unwrap();
            for provider in [&pure, &native] {
                for (candidate, algorithms) in provider.signature_verification_algorithms.mapping {
                    let accepted = algorithms.iter().any(|a| {
                        a.verify_signature(public, b"scheme separation", &signature)
                            .is_ok()
                    });
                    assert_eq!(
                        accepted,
                        candidate == scheme,
                        "{kind} actual={scheme:?} candidate={candidate:?}"
                    );
                    if candidate != scheme {
                        continue;
                    }
                    for malformed in [
                        signature[..signature.len() - 1].to_vec(),
                        [signature.as_slice(), &[0]].concat(),
                        vec![],
                    ] {
                        assert!(algorithms.iter().all(|a| {
                            a.verify_signature(public, b"scheme separation", &malformed)
                                .is_err()
                        }));
                    }
                    if kind == "p256" || kind == "p384" {
                        let mut malformed = public.to_vec();
                        malformed[0] = 0;
                        assert!(algorithms.iter().all(|a| {
                            a.verify_signature(&malformed, b"scheme separation", &signature)
                                .is_err()
                        }));
                    }
                }
            }
        }
    }
}

#[test]
fn trust_validity_eku_tampering_and_same_algorithm_key_mismatch() {
    let ca_key = yy_cert::KeyPair::generate().unwrap();
    let mut ca_params = ca_key.certificate_params(vec![]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    ca_params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    ca_params.not_after = rcgen::date_time_ymd(2035, 1, 1);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let leaf_key = yy_cert::KeyPair::generate().unwrap();
    let make_leaf = |start, end, purpose| {
        let mut params = leaf_key
            .certificate_params(vec!["localhost".into()])
            .unwrap();
        params.not_before = rcgen::date_time_ymd(start, 1, 1);
        params.not_after = rcgen::date_time_ymd(end, 1, 1);
        params.extended_key_usages = vec![purpose];
        params.signed_by(&leaf_key, &issuer).unwrap().der().clone()
    };
    let valid = make_leaf(2025, 2030, rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let expired = make_leaf(2020, 2021, rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let future = make_leaf(2030, 2031, rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let client_only = make_leaf(2025, 2030, rcgen::ExtendedKeyUsagePurpose::ClientAuth);
    let untrusted_key = yy_cert::KeyPair::generate().unwrap();
    let untrusted = untrusted_key
        .certificate_params(vec!["localhost".into()])
        .unwrap()
        .self_signed(&untrusted_key)
        .unwrap();
    let mut tampered = valid.as_ref().to_vec();
    *tampered.last_mut().unwrap() ^= 1;
    let tampered = CertificateDer::from(tampered);
    // Fixed 2026-01-01, independent of wall-clock and fixture expiration.
    let now = UnixTime::since_unix_epoch(Duration::from_secs(1_767_225_600));
    for provider in [
        yy_tls_crypto::provider(),
        rustls::crypto::ring::default_provider(),
    ] {
        let mut roots = RootCertStore::empty();
        roots.add(ca.der().clone()).unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(provider.clone()),
        )
        .build()
        .unwrap();
        verifier
            .verify_server_cert(
                &valid,
                &[],
                &ServerName::try_from("localhost").unwrap(),
                &[],
                now,
            )
            .unwrap();
        for bad in [&expired, &future, &client_only, untrusted.der(), &tampered] {
            assert!(matches!(
                verifier.verify_server_cert(
                    bad,
                    &[],
                    &ServerName::try_from("localhost").unwrap(),
                    &[],
                    now
                ),
                Err(rustls::Error::InvalidCertificate(_))
            ));
        }
        assert!(
            verifier
                .verify_server_cert(
                    &valid,
                    &[],
                    &ServerName::try_from("attacker.example").unwrap(),
                    &[],
                    now
                )
                .is_err()
        );
        let wrong_key = yy_cert::KeyPair::generate().unwrap();
        assert!(
            rustls::sign::CertifiedKey::from_der(
                vec![valid.clone()],
                PrivateKeyDer::Pkcs8(wrong_key.serialize_der().unwrap().into()),
                &provider
            )
            .is_err()
        );
    }
}
