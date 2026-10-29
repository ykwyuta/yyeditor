//! Compile-time contracts for persistent secret state owned by this provider.
//!
//! These assertions verify upstream trait/feature contracts, not physical erasure
//! of every temporary, register, or compiler-generated copy.

const _: fn() = || {
    fn clears_on_drop<T: zeroize::ZeroizeOnDrop>() {}

    clears_on_drop::<aes_gcm::Aes128Gcm>();
    clears_on_drop::<aes_gcm::Aes256Gcm>();
    clears_on_drop::<chacha20poly1305::ChaCha20Poly1305>();
    clears_on_drop::<sha2::Sha256>();
    clears_on_drop::<sha2::Sha384>();
    clears_on_drop::<sha2::Sha512>();
    clears_on_drop::<rsa::RsaPrivateKey>();
    clears_on_drop::<p256::ecdsa::SigningKey>();
    clears_on_drop::<p384::ecdsa::SigningKey>();
    clears_on_drop::<p256::ecdh::EphemeralSecret>();
    clears_on_drop::<p384::ecdh::EphemeralSecret>();
    clears_on_drop::<ed25519_dalek::SigningKey>();
    clears_on_drop::<x25519_dalek::StaticSecret>();

    // Hmac 0.13 does not implement ZeroizeOnDrop on its outer wrapper. Its
    // HmacCore contains two digest cores, and its wrapper also owns a buffer.
    // Check those member types rather than asserting an unavailable marker.
    clears_on_drop::<<sha2::Sha256 as digest::block_api::CoreProxy>::Core>();
    clears_on_drop::<<sha2::Sha384 as digest::block_api::CoreProxy>::Core>();
    clears_on_drop::<digest::block_api::Buffer<hmac::block_api::HmacCore<sha2::Sha256>>>();
    clears_on_drop::<digest::block_api::Buffer<hmac::block_api::HmacCore<sha2::Sha384>>>();
};
