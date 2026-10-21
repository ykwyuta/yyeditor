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
