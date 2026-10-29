//! Differential tests against the independently implemented original backend.
use ring::aead::{self, Aad, LessSafeKey, UnboundKey};
use yy_ssh_crypto::{chacha20_poly1305_openssh as pure, gcm};

fn packet(size: usize) -> Vec<u8> {
    let mut bytes = (size as u32).to_be_bytes().to_vec();
    bytes.extend((0..size).map(|n| (n % 251) as u8));
    bytes
}

#[test]
fn openssh_chacha_matches_ring_and_rejects_tampering() {
    for key in [[0; 64], [0xff; 64], std::array::from_fn(|n| n as u8)] {
        let sealer = pure::SealingKey::new(&key);
        let opener = pure::OpeningKey::new(&key);
        let original = aead::chacha20_poly1305_openssh::SealingKey::new(&key);
        let original_open = aead::chacha20_poly1305_openssh::OpeningKey::new(&key);
        for seq in [0, 1, 255, 256, 65_535, 0x8000_0000, u32::MAX] {
            for size in [0, 1, 15, 16, 31, 32, 63, 64, 65, 1024, 32_768] {
                let plaintext = packet(size);
                let mut expected = plaintext.clone();
                let mut actual = plaintext.clone();
                let mut expected_tag = [0; 16];
                let mut tag = [0; 16];
                original.seal_in_place(seq, &mut expected, &mut expected_tag);
                sealer.seal_in_place(seq, &mut actual, &mut tag);
                assert_eq!(actual, expected, "sequence {seq}, size {size}");
                assert_eq!(tag, expected_tag);
                assert_eq!(
                    opener.decrypt_packet_length(seq, actual[..4].try_into().unwrap()),
                    plaintext[..4]
                );
                assert_eq!(
                    original_open
                        .open_in_place(seq, &mut expected, &tag)
                        .unwrap(),
                    &plaintext[4..]
                );
                let mut invalid_tag = tag;
                invalid_tag[0] ^= 1;
                let original_ciphertext = actual.clone();
                assert!(
                    opener
                        .open_in_place(seq, &mut actual, &invalid_tag)
                        .is_err()
                );
                assert_eq!(
                    actual, original_ciphertext,
                    "no plaintext before authentication"
                );
                assert!(
                    opener
                        .open_in_place(seq.wrapping_add(1), &mut actual, &tag)
                        .is_err()
                );
                for index in [0, actual.len() - 1] {
                    let mut altered = actual.clone();
                    altered[index] ^= 1;
                    assert!(opener.open_in_place(seq, &mut altered, &tag).is_err());
                }
                assert_eq!(
                    opener.open_in_place(seq, &mut actual, &tag).unwrap(),
                    &plaintext[4..]
                );
            }
        }
    }
    for len in 0..4 {
        assert!(
            pure::OpeningKey::new(&[0; 64])
                .open_in_place(0, &mut vec![0; len], &[0; 16])
                .is_err()
        );
    }
}

#[test]
fn aes_gcm_matches_ring_and_rejects_tampering() {
    for (key_bytes, algorithm) in [
        (vec![7; 16], &aead::AES_128_GCM),
        (vec![9; 32], &aead::AES_256_GCM),
    ] {
        let key = gcm::Key::new(&key_bytes).unwrap();
        let original = LessSafeKey::new(UnboundKey::new(algorithm, &key_bytes).unwrap());
        let mut nonces = gcm::Nonce::new([0x5a; 12]);
        for size in [0, 1, 15, 16, 31, 32, 63, 64, 65, 1024, 32_768] {
            let nonce = nonces.advance().unwrap();
            let plaintext = packet(size);
            let mut expected = plaintext.clone();
            let aad = expected[..4].to_vec();
            let expected_tag = original
                .seal_in_place_separate_tag(
                    aead::Nonce::assume_unique_for_key(nonce),
                    Aad::from(&aad),
                    &mut expected[4..],
                )
                .unwrap();
            let mut actual = plaintext.clone();
            let tag = key.seal(&nonce, &mut actual).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(&tag, expected_tag.as_ref());
            actual.extend(tag);
            for index in [0, actual.len() - 1, 4] {
                let mut altered = actual.clone();
                altered[index] ^= 1;
                let original_ciphertext = altered.clone();
                assert!(key.open(&nonce, &mut altered).is_err());
                assert_eq!(altered, original_ciphertext);
            }
            expected.extend(tag);
            assert_eq!(
                original
                    .open_in_place(
                        aead::Nonce::assume_unique_for_key(nonce),
                        Aad::from(&aad),
                        &mut expected[4..]
                    )
                    .unwrap(),
                &plaintext[4..]
            );
            assert_eq!(key.open(&nonce, &mut actual).unwrap(), &plaintext[4..]);
        }
        for size in 0..20 {
            assert!(key.open(&[0; 12], &mut vec![0; size]).is_err());
        }
    }
    assert!(gcm::Key::new(&[0; 24]).is_err());
}

#[test]
fn gcm_invocation_counter_carries_and_refuses_wrap() {
    let mut initial = [0xff; 12];
    initial[..4].copy_from_slice(&[1, 2, 3, 4]);
    initial[4..].copy_from_slice(&0xffu64.to_be_bytes());
    let mut counter = gcm::Nonce::new(initial);
    assert_eq!(counter.advance().unwrap(), initial);
    initial[4..].copy_from_slice(&0x100u64.to_be_bytes());
    assert_eq!(counter.advance().unwrap(), initial);
    initial[4..].fill(0xff);
    let mut counter = gcm::Nonce::new(initial);
    assert_eq!(counter.advance().unwrap(), initial);
    assert!(counter.advance().is_err());
    assert!(counter.advance().is_err());
}
