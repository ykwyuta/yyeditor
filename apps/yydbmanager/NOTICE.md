# yydbmanager third-party notices

yydbmanager and the yy series shared components are GPL-3.0-or-later.
Native database drivers, including rust-db2-driver, are described in the bundled
THIRD-PARTY-NOTICES.md, including the exact upstream source revision and licenses.
See the repository's COPYING. Source: https://github.com/ykwyuta/yyeditor

The Markdown help uses the shared yy-preview renderer and Microsoft Edge
WebView2 Loader, with the additional permission in COPYING.EXCEPTION.
Embedded preview assets include Mermaid, KaTeX and D3; their license files
from crates/yy-preview/assets are included in the application package.
YAML serialization uses serde-saphyr (MIT OR Apache-2.0).

The Windows application embeds UDEV Gothic, licensed under SIL Open Font
License 1.1; see crates/yy-win/fonts/LICENSE-UDEVGothic.txt.

The database workspace uses SQLite (public domain) through rusqlite and
libsqlite3-sys, and DuckDB through duckdb-rs and libduckdb-sys (MIT).
The exact versions and transitive dependencies are in this workspace's Cargo.lock.
External databases use Rust drivers. The Docker validation image is separate
from the application distribution.

## DuckDB

Copyright 2021-2025 Stichting DuckDB Foundation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

Parquet 56.2.1 uses a local Pure Rust Zstd backend (zstandard 0.1.9) for all compression levels 1 through 22. The native zstd/zstd-sys crates are not part of the application dependency graph. This does not remove the separate native SQLite or DuckDB engines. See THIRD-PARTY-NOTICES.md for Apache Parquet and zstandard notices.
