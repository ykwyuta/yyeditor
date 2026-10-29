# yyeditor Parquet 56.2.1 source patch

Source: https://crates.io/crates/parquet/56.2.1
Original package SHA-256: `d3abbfef8a25900f4925c86e4cb881ea24672ca3c31ee4fb50a8083c4c56d313`.
The original Apache-2.0 license and Apache notices are in `LICENSE.txt` and
`NOTICE.txt`. `Cargo.toml.orig` retains the original source manifest.

Local changes (2026-10-29):

- `Cargo.toml`: add `zstd-rust` and the optional local `yy-parquet-zstd` adapter.
- `src/compression.rs`: select the adapter when `zstd-rust` is enabled.

The yydbmanager dependency explicitly disables defaults and selects
`arrow,snap,zstd-rust`. Native Zstd is retained as an upstream optional feature
for comparison, but is not enabled in the application. The Rust backend takes
precedence if both backend features are selected. All compression levels 1..=22
are passed through to zstandard 0.1.9 without substitution. The library's default
compression settings and the application's default export settings are unchanged.

All other package source files are preserved. Cargo's download marker and the
upstream development lockfile are omitted. This is a local source patch, not an
upstream release. See `docs/developer/yydbmanager.md` for checks and limitations.
