# yyeditor's russh 0.63.1 source patch

This directory contains the crates.io release `russh 0.63.1`, upstream commit
`d3ae702a43a163946f258297e398dc216339d5ce` (see `.cargo_vcs_info.json`).
Upstream: https://github.com/warp-tech/russh . License: Apache-2.0;
the original copyright headers and `LICENSE-APACHE` are retained.

Local changes (2026-10-29):

- `Cargo.toml`: optional `rustcrypto` feature and `yy-ssh-crypto` path dependency.
- `src/lib.rs`, `src/helpers.rs`: accept the additional crypto backend.
- `src/cipher/mod.rs`: route AES-GCM to `gcm_rustcrypto.rs` when enabled.
- `src/cipher/gcm_rustcrypto.rs`: adapt RustCrypto AES-GCM and the SSH nonce counter.
- `src/cipher/chacha20poly1305.rs`: route OpenSSH ChaCha20-Poly1305 to the adapter.

The packet encryption implementations and differential tests live in
`crates/yy-ssh-crypto`. No SSH algorithm is removed from the application's
preferences. The `rustcrypto` feature takes precedence if combined with a native
backend feature. The upstream native implementations remain available for
comparison, but yyeditor enables only `rustcrypto`, `rsa`, and `flate2`.

The upstream package files are preserved, except Cargo's download marker and
the upstream development lockfile. `Cargo.toml.orig` records the unmodified
published source manifest. This is a local source patch, not a published upstream
release. Root and yymerge lockfiles must resolve the same directory.
