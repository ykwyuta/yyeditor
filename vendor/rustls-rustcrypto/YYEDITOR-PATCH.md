# Local RustCrypto TLS backend

Source: https://github.com/RustCrypto/rustls-rustcrypto at full commit
`70f76c039e587192688af18a80d5d6435dedaf22`. This is not an unmodified crates.io
release of `rustls-rustcrypto 0.0.2-alpha`. Original MIT/Apache-2.0 licenses and
README (including its production-use warning) are retained.

Local changes:

- Reject non-contributory X25519 shared secrets; enforce uncompressed P-256/P-384
  TLS key-share encoding. Fallible operating-system randomness is propagated.
- Match rustls/ring AES-GCM confidentiality limits (2^24 records); prefer TLS 1.3.
- Reject truncated ChaCha20-Poly1305 TLS 1.2 records before length subtraction;
  reject authenticated TLS 1.2 plaintext exceeding 2^14 bytes.
- Extract the correct AES-128/AES-256 traffic-secret type and full TLS 1.2 IV.
- Expose RSA/ECDSA/Ed25519 public keys for certificate/private-key consistency checks.
  Validate RSA private keys and 2048..8192-bit modulus sizes; blind RSA-PSS signing.
  Debug output of RSA keys and generic signers omits private-key material.
- Parse SEC1 keys with curve/public-point validation. Use strict Ed25519 verification.
- Accept RSA PKCS#1 certificate signature identifiers with absent NULL parameters,
  matching the algorithm identifiers supported by rustls/ring.
- Disable the unimplemented QUIC module; all supported TLS suites advertise no QUIC.
  No TLS, AEAD, elliptic-curve or RSA primitives are reimplemented here.
- Declare Rust 1.87 for the dependency versions used in this repository.
  Require crypto-bigint >=0.7.10 so an older consumer lockfile cannot retain 0.7.5.

The adapter and comparison tests are in `crates/yy-tls-crypto`. ring is used only
as a development dependency for independent comparisons. This backend is not a
FIPS-certified implementation, and passing the repository tests is not a formal
security audit. Dependency updates require reviewing these patches again.

Adversarial review follow-up:
- Require rustls >=0.23.46, including the session Debug secret-redaction fix.
- Forward the default zeroize feature to AES-GCM, ChaCha20-Poly1305, HMAC and
  SHA-2 as well as Ed25519/X25519. Compile-time assertions check persistent
  secret types' ZeroizeOnDrop contracts; these do not guarantee physical erasure
  of all temporary copies or registers.
