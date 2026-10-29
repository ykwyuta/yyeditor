//! SSH packet encryption built from RustCrypto primitives.
//!
//! Nonces and sequence numbers must never repeat under a key. The SSH transport
//! owns rekeying and sequence numbers; GCM invocation counters are tracked here.

pub mod chacha20_poly1305_openssh;
pub mod gcm;

/// Invalid key, packet, authentication tag, or exhausted invocation counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid SSH encrypted packet or exhausted nonce")
    }
}

impl std::error::Error for Error {}
