// Copyright 2016 Pierre-Étienne Meunier
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

// http://cvsweb.openbsd.org/cgi-bin/cvsweb/src/usr.bin/ssh/PROTOCOL.chacha20poly1305?annotate=HEAD

// Derived from russh 0.63.1 cipher/gcm.rs (Apache-2.0).
// Modified for yyeditor: use yy-ssh-crypto instead of native AEAD backends.

use rand_core::Rng;
use yy_ssh_crypto::gcm::{Key, Nonce};

use crate::{Error, keys::key::safe_rng, mac::MacAlgorithm};

pub enum Algorithm {
    Aes128,
    Aes256,
}
pub static AES_128_GCM: Algorithm = Algorithm::Aes128;
pub static AES_256_GCM: Algorithm = Algorithm::Aes256;
pub struct GcmCipher(pub(crate) &'static Algorithm);

impl super::Cipher for GcmCipher {
    fn key_len(&self) -> usize {
        match self.0 {
            Algorithm::Aes128 => 16,
            Algorithm::Aes256 => 32,
        }
    }
    fn nonce_len(&self) -> usize {
        12
    }
    fn make_opening_key(
        &self,
        key: &[u8],
        nonce: &[u8],
        _: &[u8],
        _: &dyn MacAlgorithm,
    ) -> Box<dyn super::OpeningKey + Send> {
        Box::new(OpeningKey(
            Key::new(key).unwrap_or_else(|_| unreachable!("invalid derived AES key")),
            Nonce::new(
                nonce
                    .try_into()
                    .unwrap_or_else(|_| unreachable!("invalid derived AES nonce")),
            ),
        ))
    }
    fn make_sealing_key(
        &self,
        key: &[u8],
        nonce: &[u8],
        _: &[u8],
        _: &dyn MacAlgorithm,
    ) -> Box<dyn super::SealingKey + Send> {
        Box::new(SealingKey(
            Key::new(key).unwrap_or_else(|_| unreachable!("invalid derived AES key")),
            Nonce::new(
                nonce
                    .try_into()
                    .unwrap_or_else(|_| unreachable!("invalid derived AES nonce")),
            ),
        ))
    }
}

pub struct OpeningKey(Key, Nonce);
pub struct SealingKey(Key, Nonce);

impl super::OpeningKey for OpeningKey {
    fn decrypt_packet_length(&self, _: u32, length: &[u8]) -> [u8; 4] {
        length
            .try_into()
            .unwrap_or_else(|_| unreachable!("invalid SSH length field"))
    }
    fn tag_len(&self) -> usize {
        16
    }
    fn open<'a>(&mut self, _: u32, packet: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let nonce = self.1.advance().map_err(|_| Error::DecryptionError)?;
        self.0
            .open(&nonce, packet)
            .map_err(|_| Error::DecryptionError)
    }
}

impl super::SealingKey for SealingKey {
    fn padding_length(&self, payload: &[u8]) -> usize {
        let block_size = 16;
        let extra_len = super::PACKET_LENGTH_LEN + super::PADDING_LENGTH_LEN;
        let padding_len = if payload.len() + extra_len <= super::MINIMUM_PACKET_LEN {
            super::MINIMUM_PACKET_LEN - payload.len() - super::PADDING_LENGTH_LEN
        } else {
            block_size - ((super::PADDING_LENGTH_LEN + payload.len()) % block_size)
        };
        if padding_len < super::PACKET_LENGTH_LEN {
            padding_len + block_size
        } else {
            padding_len
        }
    }
    fn fill_padding(&self, padding_out: &mut [u8]) {
        safe_rng().fill_bytes(padding_out);
    }
    fn tag_len(&self) -> usize {
        16
    }
    fn seal(&mut self, _: u32, packet: &mut [u8], tag: &mut [u8]) {
        let nonce = self.1.advance().unwrap_or_else(|_| {
            unreachable!("AES-GCM invocation counter exhausted; rekey required")
        });
        let result = self
            .0
            .seal(&nonce, packet)
            .unwrap_or_else(|_| unreachable!("invalid bounded SSH packet"));
        tag.copy_from_slice(&result);
    }
}
