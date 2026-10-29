//! Zstandard adapter preserving Parquet's append and compression-level semantics.
//!
//! The zstandard crate implements all positive compression levels without FFI.
//! A numeric level is passed through, never mapped to a lower-effort preset.

use std::io;
use zstandard::{CompressionLevel, DecoderOptions, EncoderOptions};

/// Compress one page at the requested Parquet Zstd level (1..=22).
/// Append a complete frame, leaving the destination unchanged on codec errors.
pub fn compress(input: &[u8], output: &mut Vec<u8>, level: i32) -> io::Result<()> {
    if !(1..=22).contains(&level) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Parquet Zstd compression level must be 1..=22",
        ));
    }
    let compressed = zstandard::encode_all_with_options(
        input,
        EncoderOptions {
            compression_level: CompressionLevel::try_new(level)?,
            ..Default::default()
        },
    )?;
    output
        .try_reserve(compressed.len())
        .map_err(io::Error::other)?;
    output.extend_from_slice(&compressed);
    Ok(())
}

/// Decode all frames with checksum verification and the reference 128 MiB window
/// limit. A supplied Parquet page size is an exact limit, not an allocation hint.
/// Append only after the full input and resulting size have been validated.
pub fn decompress(input: &[u8], output: &mut Vec<u8>, size: Option<usize>) -> io::Result<usize> {
    let decoded = zstandard::decode_all_with_options(
        input,
        DecoderOptions {
            max_output_size: size,
            ..Default::default()
        },
    )?;
    if size.is_some_and(|expected| decoded.len() != expected) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Zstd decoded size does not match Parquet page size",
        ));
    }
    output
        .try_reserve(decoded.len())
        .map_err(io::Error::other)?;
    output.extend_from_slice(&decoded);
    Ok(decoded.len())
}
