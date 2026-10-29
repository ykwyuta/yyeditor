# yymerge third-party notices

yymerge and the YY series shared components are GPL-3.0-or-later.
See COPYING. Source: https://github.com/ykwyuta/yyeditor

The Markdown help uses yy-preview and Microsoft Edge WebView2 Loader under
the additional permission in COPYING.EXCEPTION. Its embedded Mermaid,
KaTeX and D3 assets have license files included in the Windows package.

The Windows GUI embeds UDEV Gothic (SIL Open Font License 1.1).
See LICENSE-UDEVGothic.txt in the distribution.

The application icon is generated with the existing YY series icon generator.
Diff and merge algorithms are implemented in this repository; no proprietary
WinMerge, Beyond Compare or Araxis Merge code or assets are included.

See Cargo.lock for exact Rust dependency versions. Major dependencies include
clap, serde, serde_json, tempfile, windows and the shared yy-encoding components.

BLAKE3 is used under CC0-1.0; see LICENSE-BLAKE3-CC0.txt.
Copyright belongs to the BLAKE3 authors (see the dependency source and Cargo.lock).

Read-only Office comparison uses zip (MIT) and roxmltree (MIT), with flate2,
crc32fast, simd-adler32 and the deflate backend dependencies. Zopfli is Apache-2.0.
See LICENSE-OFFICE-DEPS.txt for their license texts and attributions.
Original comparison examples under demo/yymerge/office were generated using
openpyxl, python-pptx and Pillow; these generators are not required at runtime.

Image comparison uses image (MIT selected), png, zune-jpeg, image-webp, quick-error and the codec dependencies
listed in Cargo.lock. License texts and attributions: LICENSE-IMAGE-DEPS.txt.
Raster image comparison does not require a system image viewer or Office.
PDF page counting and rasterization use embedded Hayro (MIT selected), with
Vello CPU and its Rust dependencies. No Poppler executable or shared library
is used. See LICENSE-PDF-DEPS.txt for dependency and bundled font/CMap/ICC notices.

SSH uses the YY series yy-ssh/yy-remote components and a locally patched russh 0.63.1 client with a RustCrypto packet-encryption backend (source: vendor/russh in the yyeditor repository), with its cryptography and runtime dependencies. License texts and attributions: LICENSE-SSH-DEPS.txt.
