//! Failed handshakes must never commit a newly approved TOFU identity.
use super::*;
use rustls::sign::{CertifiedKey, Signer, SigningKey};

#[test]
fn tofu_write_failure_after_handshake_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_certs");
    let (cert, key, _) = self_signed(&["localhost"]);
    let (port, worker) = serve_once(server_config(cert, &key, None), false);
    let blocked = known.clone();
    let approve: Confirm = Arc::new(move |_| {
        // Lookup succeeded before approval, but storage becomes unwritable
        // before persistence. This must still fail the connection visibly.
        std::fs::create_dir(&blocked).unwrap();
        true
    });
    let options = Options {
        known_certs: Some(known.clone()),
        ..opts("localhost", port)
    };
    let result = connect(tcp(port), false, &options, approve);
    let error = result
        .err()
        .expect("cannot silently accept failed TOFU storage");
    let server_result = worker.join().unwrap();
    assert!(server_result.is_err());
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("証明書の記録"), "{error}");
    assert!(known.is_dir());
}

#[derive(Debug)]
struct InvalidSignature;
impl Signer for InvalidSignature {
    fn sign(&self, _: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        Ok(vec![0; 64])
    }
    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ECDSA_NISTP256_SHA256
    }
}
#[derive(Debug)]
struct InvalidKey(Arc<dyn SigningKey>);
impl SigningKey for InvalidKey {
    fn choose_scheme(&self, _: &[rustls::SignatureScheme]) -> Option<Box<dyn Signer>> {
        Some(Box::new(InvalidSignature))
    }
    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ECDSA
    }
    fn public_key(&self) -> Option<rustls::pki_types::SubjectPublicKeyInfoDer<'_>> {
        self.0.public_key()
    }
}
#[derive(Debug)]
struct Resolver(Arc<CertifiedKey>);
impl rustls::server::ResolvesServerCert for Resolver {
    fn resolve(&self, _: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

#[test]
fn failed_certificate_verify_or_finished_does_not_persist_tofu() {
    for failure in ["signature", "finished", "eof"] {
        for existing in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let known = dir.path().join("known_certs");
            let original = b"# retain unrelated identities\nother:992 sha256:AA:BB CN=other\n";
            if existing {
                std::fs::write(&known, original).unwrap();
            }
            let (cert, key, _) = self_signed(&["localhost"]);
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let real_key = provider
                .key_provider
                .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    key.serialize_der().unwrap(),
                )))
                .unwrap();
            let key = if failure == "signature" {
                Arc::new(InvalidKey(real_key)) as Arc<dyn SigningKey>
            } else {
                real_key
            };
            let certified = CertifiedKey::new(vec![cert], key);
            let mut config = ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[if failure == "signature" {
                    &rustls::version::TLS13
                } else {
                    &rustls::version::TLS12
                }])
                .unwrap()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(Resolver(Arc::new(certified))));
            // Keep certificate processing separate from the final protected
            // record so a damaged/truncated Finished follows TOFU approval.
            config.max_fragment_size = Some(64);
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let worker = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut conn = ServerConnection::new(Arc::new(config)).unwrap();
                while !conn.wants_write() {
                    assert_ne!(conn.read_tls(&mut socket).unwrap(), 0);
                    conn.process_new_packets().unwrap();
                }
                if failure != "signature" {
                    // TLS 1.2 authenticates the certificate/key exchange in
                    // the first flight. Damage the subsequent server Finished
                    // only after the client has accepted that certificate.
                    while conn.wants_write() {
                        conn.write_tls(&mut socket).unwrap();
                    }
                    while !conn.wants_write() {
                        assert_ne!(conn.read_tls(&mut socket).unwrap(), 0);
                        conn.process_new_packets().unwrap();
                    }
                }
                let mut flight = Vec::new();
                while conn.wants_write() {
                    conn.write_tls(&mut flight).unwrap();
                }
                if failure == "finished" {
                    *flight.last_mut().unwrap() ^= 1;
                } else if failure == "eof" {
                    let mut last = 0;
                    let mut offset = 0;
                    while offset < flight.len() {
                        last = offset;
                        offset += 5 + usize::from(u16::from_be_bytes([
                            flight[offset + 3],
                            flight[offset + 4],
                        ]));
                    }
                    assert_eq!(offset, flight.len());
                    flight.truncate(last);
                }
                socket.write_all(&flight).unwrap();
                socket.shutdown(std::net::Shutdown::Write).unwrap();
                let mut response = Vec::new();
                let _ = socket.read_to_end(&mut response);
            });
            let (approve, calls, _) = confirm(true);
            let options = Options {
                known_certs: Some(known.clone()),
                ..opts("localhost", port)
            };
            let result = connect(tcp(port), false, &options, approve);
            let rejected = result.is_err();
            drop(result);
            worker.join().unwrap();
            assert!(rejected, "{failure}");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "{failure}: must reach TOFU approval"
            );
            if existing {
                assert_eq!(std::fs::read(&known).unwrap(), original, "{failure}");
            } else {
                assert!(!known.exists(), "{failure}: failed handshake poisoned TOFU");
            }
        }
    }
}
