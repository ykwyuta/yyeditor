//! Shared TLS cryptography. TLS protocol and certificate validation remain in rustls/webpki.
//!
//! No process-global provider is installed: callers explicitly choose this provider.

/// RustCrypto AES-GCM/ChaCha20-Poly1305, X25519/P-256/P-384 and RSA/ECDSA/Ed25519.
pub fn provider() -> rustls::crypto::CryptoProvider {
    rustls_rustcrypto::provider()
}
