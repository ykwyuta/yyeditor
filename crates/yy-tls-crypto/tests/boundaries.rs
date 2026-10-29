use rustls::crypto::cipher::{
    AeadKey, InboundOpaqueMessage, Iv, MessageDecrypter, MessageEncrypter, OutboundPlainMessage,
};
use rustls::{ContentType, ProtocolVersion, SupportedCipherSuite};

struct Ciphers {
    enc: Box<dyn MessageEncrypter>,
    dec: Box<dyn MessageDecrypter>,
}
fn ciphers(suite: SupportedCipherSuite) -> Option<Ciphers> {
    // The public AeadKey constructor exposes 32-byte keys. AES-128 is covered by
    // the full handshakes and secret-extraction tests, which derive 16-byte keys.
    let key = || AeadKey::from([0x51; 32]);
    match suite {
        SupportedCipherSuite::Tls13(s) if s.aead_alg.key_len() == 32 => Some(Ciphers {
            enc: s.aead_alg.encrypter(key(), Iv::from([0x37; 12])),
            dec: s.aead_alg.decrypter(key(), Iv::from([0x37; 12])),
        }),
        SupportedCipherSuite::Tls12(s) if s.aead_alg.key_block_shape().enc_key_len == 32 => {
            let shape = s.aead_alg.key_block_shape();
            let iv = [0x37; 12];
            let explicit = [0x46; 8];
            Some(Ciphers {
                enc: s.aead_alg.encrypter(
                    key(),
                    &iv[..shape.fixed_iv_len],
                    &explicit[..shape.explicit_nonce_len],
                ),
                dec: s.aead_alg.decrypter(key(), &iv[..shape.fixed_iv_len]),
            })
        }
        _ => None,
    }
}

#[test]
fn record_bytes_match_ring_and_bad_records_are_rejected_without_panics() {
    let pure = yy_tls_crypto::provider();
    let native = rustls::crypto::ring::default_provider();
    let mut cases = 0;
    for suite in pure.cipher_suites {
        if ciphers(suite).is_none() {
            continue;
        }
        let native_suite = *native
            .cipher_suites
            .iter()
            .find(|s| s.suite() == suite.suite())
            .unwrap();
        for seq in [0, 1, 0x1234_5678_9abc_def0, u64::MAX] {
            for len in [0, 1, 15, 16, 17, 1024, 16384, 16385] {
                let payload: Vec<_> = (0..len).map(|i| (i % 251) as u8).collect();
                let message = || OutboundPlainMessage {
                    typ: ContentType::ApplicationData,
                    version: ProtocolVersion::TLSv1_2,
                    payload: payload.as_slice().into(),
                };
                let mut ours = ciphers(suite).unwrap();
                let ours_record = ours.enc.encrypt(message(), seq).unwrap();
                let native_record = ciphers(native_suite)
                    .unwrap()
                    .enc
                    .encrypt(message(), seq)
                    .unwrap();
                assert_eq!(
                    ours_record.payload.as_ref(),
                    native_record.payload.as_ref(),
                    "{:?} seq={seq} len={len}",
                    suite.suite()
                );
                let bytes = ours_record.payload.as_ref().to_vec();
                let mut valid = bytes.clone();
                let decrypted = ours.dec.decrypt(
                    InboundOpaqueMessage::new(ours_record.typ, ours_record.version, &mut valid),
                    seq,
                );
                if len > 16384 {
                    assert!(matches!(
                        decrypted,
                        Err(rustls::Error::PeerSentOversizedRecord)
                    ));
                } else {
                    assert_eq!(decrypted.unwrap().payload, payload);
                }
                let mut tampered = bytes.clone();
                *tampered.last_mut().unwrap() ^= 1;
                assert!(
                    ciphers(suite)
                        .unwrap()
                        .dec
                        .decrypt(
                            InboundOpaqueMessage::new(
                                ours_record.typ,
                                ours_record.version,
                                &mut tampered
                            ),
                            seq
                        )
                        .is_err()
                );
                let mut wrong_seq = bytes;
                assert!(
                    ciphers(suite)
                        .unwrap()
                        .dec
                        .decrypt(
                            InboundOpaqueMessage::new(
                                ours_record.typ,
                                ours_record.version,
                                &mut wrong_seq
                            ),
                            seq.wrapping_add(1)
                        )
                        .is_err()
                );
                cases += 1;
            }
        }
        for len in 0..25 {
            let mut bytes = vec![0; len];
            assert!(
                ciphers(suite)
                    .unwrap()
                    .dec
                    .decrypt(
                        InboundOpaqueMessage::new(
                            ContentType::ApplicationData,
                            ProtocolVersion::TLSv1_2,
                            &mut bytes
                        ),
                        0
                    )
                    .is_err()
            );
        }
    }
    assert_eq!(cases, 192);
}

#[test]
fn invalid_key_shares_are_rejected_and_confidentiality_limits_match_ring() {
    let pure = yy_tls_crypto::provider();
    let native = rustls::crypto::ring::default_provider();
    for group in pure.kx_groups {
        let peer = *native
            .kx_groups
            .iter()
            .find(|g| g.name() == group.name())
            .unwrap();
        let ours = group.start().unwrap();
        let theirs = peer.start().unwrap();
        let our_public = ours.pub_key().to_vec();
        let their_public = theirs.pub_key().to_vec();
        assert_eq!(
            ours.complete(&their_public).unwrap().secret_bytes(),
            theirs.complete(&our_public).unwrap().secret_bytes()
        );
        for bytes in [
            vec![],
            vec![0; 31],
            vec![0; 32],
            vec![0; 33],
            vec![4; 65],
            vec![4; 97],
        ] {
            assert!(
                group.start().unwrap().complete(&bytes).is_err(),
                "{:?}",
                group.name()
            );
        }
        if group.name() == rustls::NamedGroup::X25519 {
            let mut low_order = [0; 32];
            low_order[0] = 1;
            assert!(group.start().unwrap().complete(&low_order).is_err());
        } else {
            let mut compressed = their_public[..(their_public.len() - 1) / 2 + 1].to_vec();
            compressed[0] = 2 | (their_public.last().unwrap() & 1);
            assert!(group.start().unwrap().complete(&compressed).is_err());
        }
    }
    let limit = |suite: SupportedCipherSuite| match suite {
        SupportedCipherSuite::Tls12(s) => s.common.confidentiality_limit,
        SupportedCipherSuite::Tls13(s) => s.common.confidentiality_limit,
    };
    for suite in pure.cipher_suites {
        let reference = *native
            .cipher_suites
            .iter()
            .find(|s| s.suite() == suite.suite())
            .unwrap();
        assert_eq!(limit(suite), limit(reference));
    }
}
