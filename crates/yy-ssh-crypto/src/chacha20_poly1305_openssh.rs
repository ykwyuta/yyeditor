//! OpenSSH's 64-bit nonce construction, not the IETF ChaCha20-Poly1305 AEAD.

use chacha20::ChaCha20Legacy;
use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use poly1305::{Poly1305, universal_hash::KeyInit};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::Error;

pub const KEY_LEN: usize = 64;
pub const TAG_LEN: usize = 16;

struct Key(Zeroizing<[u8; KEY_LEN]>);

impl Key {
    fn header(&self, seq: u32) -> ChaCha20Legacy {
        ChaCha20Legacy::new(
            self.0[32..].try_into().expect("header key"),
            &(u64::from(seq).to_be_bytes()).into(),
        )
    }

    fn body(&self, seq: u32) -> ChaCha20Legacy {
        ChaCha20Legacy::new(
            self.0[..32].try_into().expect("body key"),
            &(u64::from(seq).to_be_bytes()).into(),
        )
    }

    fn tag(&self, seq: u32, ciphertext: &[u8]) -> [u8; TAG_LEN] {
        let mut one_time_key = Zeroizing::new([0; 32]);
        self.body(seq).apply_keystream(one_time_key.as_mut());
        Poly1305::new((&*one_time_key).into())
            .compute_unpadded(ciphertext)
            .into()
    }
}

pub struct SealingKey(Key);
pub struct OpeningKey(Key);

impl SealingKey {
    pub fn new(key: &[u8; KEY_LEN]) -> Self {
        Self(Key(Zeroizing::new(*key)))
    }

    /// Encrypt `packet_length || payload`, authenticating both ciphertexts.
    /// The caller must rekey before sequence numbers repeat.
    pub fn seal_in_place(&self, seq: u32, packet: &mut [u8], tag: &mut [u8; TAG_LEN]) {
        let (header, body) = packet.split_at_mut(4);
        self.0.header(seq).apply_keystream(header);
        let mut cipher = self.0.body(seq);
        cipher.seek(64u64);
        cipher.apply_keystream(body);
        *tag = self.0.tag(seq, packet);
    }
}

impl OpeningKey {
    pub fn new(key: &[u8; KEY_LEN]) -> Self {
        Self(Key(Zeroizing::new(*key)))
    }

    /// Unauthenticated length, only usable for bounded packet framing.
    pub fn decrypt_packet_length(&self, seq: u32, mut length: [u8; 4]) -> [u8; 4] {
        self.0.header(seq).apply_keystream(&mut length);
        length
    }

    /// Verify the tag before writing any plaintext. Return the body only.
    pub fn open_in_place<'a>(
        &self,
        seq: u32,
        packet: &'a mut [u8],
        tag: &[u8; TAG_LEN],
    ) -> Result<&'a [u8], Error> {
        if packet.len() < 4 || !bool::from(self.0.tag(seq, packet).ct_eq(tag)) {
            return Err(Error);
        }
        let body = &mut packet[4..];
        let mut cipher = self.0.body(seq);
        cipher.seek(64u64);
        cipher.apply_keystream(body);
        Ok(body)
    }
}
