//! SSH AES-GCM: clear packet length as AAD; a fixed 32-bit salt and 64-bit counter.

use crate::Error;
use aes_gcm::{AeadInOut, Aes128Gcm, Aes256Gcm, KeyInit};

pub enum Key {
    Aes128(Box<Aes128Gcm>),
    Aes256(Box<Aes256Gcm>),
}

impl Key {
    pub fn new(bytes: &[u8]) -> Result<Self, Error> {
        match bytes.len() {
            16 => Ok(Self::Aes128(Box::new(
                Aes128Gcm::new_from_slice(bytes).map_err(|_| Error)?,
            ))),
            32 => Ok(Self::Aes256(Box::new(
                Aes256Gcm::new_from_slice(bytes).map_err(|_| Error)?,
            ))),
            _ => Err(Error),
        }
    }

    pub fn seal(&self, nonce: &[u8; 12], packet: &mut [u8]) -> Result<[u8; 16], Error> {
        if packet.len() < 4 {
            return Err(Error);
        }
        let (aad, body) = packet.split_at_mut(4);
        let tag = match self {
            Self::Aes128(key) => key.encrypt_inout_detached(nonce.into(), aad, body.into()),
            Self::Aes256(key) => key.encrypt_inout_detached(nonce.into(), aad, body.into()),
        }
        .map_err(|_| Error)?;
        Ok(tag.into())
    }

    pub fn open<'a>(
        &self,
        nonce: &[u8; 12],
        packet_and_tag: &'a mut [u8],
    ) -> Result<&'a [u8], Error> {
        let tag_offset = packet_and_tag
            .len()
            .checked_sub(16)
            .filter(|&n| n >= 4)
            .ok_or(Error)?;
        let (packet, tag) = packet_and_tag.split_at_mut(tag_offset);
        let tag: &[u8; 16] = (&*tag).try_into().map_err(|_| Error)?;
        let (aad, body) = packet.split_at_mut(4);
        match self {
            Self::Aes128(key) => {
                key.decrypt_inout_detached(nonce.into(), aad, body.into(), tag.into())
            }
            Self::Aes256(key) => {
                key.decrypt_inout_detached(nonce.into(), aad, body.into(), tag.into())
            }
        }
        .map_err(|_| Error)?;
        Ok(body)
    }
}

pub struct Nonce {
    value: [u8; 12],
    exhausted: bool,
}

impl Nonce {
    pub fn new(value: [u8; 12]) -> Self {
        Self {
            value,
            exhausted: false,
        }
    }

    /// Consume one invocation counter. Refuse reuse after the 64-bit wrap.
    pub fn advance(&mut self) -> Result<[u8; 12], Error> {
        if self.exhausted {
            return Err(Error);
        }
        let current = self.value;
        let counter = u64::from_be_bytes(self.value[4..].try_into().map_err(|_| Error)?);
        if let Some(next) = counter.checked_add(1) {
            self.value[4..].copy_from_slice(&next.to_be_bytes());
        } else {
            self.exhausted = true;
        }
        Ok(current)
    }
}
