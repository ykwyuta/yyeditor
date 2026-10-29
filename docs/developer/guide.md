# Developer Guide

English | [日本語](guide.ja.md)

This repository is a Rust Cargo workspace. See [README.md](../../README.md) for application features and [docs/history/proposals/](../history/proposals/README.md) for design background (Japanese). Proposals also include future plans, so verify implementation status against source code, tests, and built-in help.

## Development Environment

- Rust stable. `rust-toolchain.toml` selects stable with rustfmt and Clippy.
- The workspace uses Rust edition 2024 and declares `rust-version = "1.85"`.
- Normal Windows builds require the MSVC toolchain, Visual Studio Build Tools with C++ build tools, and the Windows SDK.
- Running and testing previews and yybrowser requires the Microsoft Edge WebView2 runtime.
- Linux can be used to develop and test the platform-independent core libraries and yy-agent. The GUI applications do not run on Linux.

## Building and Running Windows Applications

Run from the repository root:

```powershell
cargo build --release -p yyeditor -p yyterm -p yysftp -p yysheet -p yyfilemanager -p yyclip -p yybrowser -p yydraw -p yytoolbox
```

Executables are generated under `target\release\`.

```powershell
.\target\release\yyeditor.exe .\README.md
.\target\release\yyterm.exe .
.\target\release\yysftp.exe user@host:/path
.\target\release\yysheet.exe data.csv
.\target\release\yyclip.exe
.\target\release\yyfilemanager.exe
.\target\release\yybrowser.exe https://example.com
```

yyfilemanager accepts `--sync <job-name>` for synchronization without a window. yybrowser accepts `--profile <name>`, `--proxy <setting>`, and multiple URLs.

yyeditor and yysheet accept local files and `ssh://` file URLs. yyterm accepts a folder, an `ssh://` destination, or `user@host`. yysftp accepts an `ssh://` destination, `user@host:/path`, or `user@host`.

During development, omit `--release` and use the executables under `target\debug\`. Place related executables in the same folder when testing application launch integration.

## Remote Agent

yy-agent is a helper that communicates through standard input and output over an SSH channel. It does not run as a listening server.

Build statically linked musl binaries on Linux for each architecture:

```sh
# x86_64 Linux
rustup target add x86_64-unknown-linux-musl
cargo build --release -p yy-agent --target x86_64-unknown-linux-musl

# aarch64 Linux
rustup target add aarch64-unknown-linux-musl
cargo build --release -p yy-agent --target aarch64-unknown-linux-musl
```

Place them beside the Windows applications using these names:

```text
agents/
  yy-agent-x86_64-linux
  yy-agent-aarch64-linux
```

The original binaries are at `target/<target>/release/yy-agent`. During development, set `YY_AGENT_DIR` to use a different agent folder.

When connecting, the application selects the binary matching the host CPU, deploys it to `~/.yyeditor/agent/<version>-<hash>/yy-agent`, and verifies SHA-256. CI builds the binaries on separate x86_64 and aarch64 Linux runners.

## Validation

The basic checks used by CI are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For focused validation, select the affected package, for example `cargo test -p yy-buffer`. Test GUI behavior on Windows.

Windows CI installs the WebView2 runtime and sets `YY_REQUIRE_WEBVIEW2=1` for preview tests. It also sets `YY_REQUIRE_SMB_SHARE=1` to require yyfilemanager tests involving network shares.

Linux integration tests use the OpenSSH client and sftp-server, x3270's s3270 / pr3287, and GnuCOBOL. Ubuntu CI installs these packages and runs the additional checks below:

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends openssh-client openssh-sftp-server s3270 pr3287 gnucobol3
cargo test --workspace
crates/yy-3270-mock/check-with-x3270.sh
crates/yy-cobol/check-with-gnucobol.sh
```

To type-check the Windows UI from Linux, use the CI command below. This check is separate from building and running tests on Windows.

```sh
rustup target add x86_64-pc-windows-msvc
cargo check -p yy-win -p yyeditor -p yyterm -p yysftp -p yysheet -p yyfilemanager -p yyclip -p yybrowser -p yydraw -p yytoolbox --no-default-features --target x86_64-pc-windows-msvc
```

This check disables default features. The 3270 TLS backend still uses ring and requires the Windows SDK; built-in SSH and development-certificate signing use RustCrypto. BLAKE3 always enables `pure`, including normal builds and independent applications, so it requires neither a C compiler nor an external assembler. The old `yy-files/blake3-pure` flag remains as a compatibility alias. BLAKE3 still lists `cc` as a build dependency, but does not compile its C/assembly implementations in this configuration. Validate the normal configuration, including SSH and TLS, on Windows.

## Repository Structure

| Location / crate | Responsibility |
| --- | --- |
| `apps/yyeditor`, `apps/yyterm`, `apps/yysftp`, `apps/yysheet`, `apps/yyfilemanager`, `apps/yybrowser` | Windows application entry points and resources |
| `apps/yyclip` | Independent clipboard tray application: history, templates, and favorites |
| `apps/yydraw`, `crates/yy-draw` | Native diagram editor and portable document / geometry / SVG / PNG implementation; see [yydraw](../../apps/yydraw/README.md) |
| `apps/yy-agent` | Agent for remote file operations |
| `crates/yy-win` | Shared Win32 / Direct2D / DirectWrite UI, application screens, and built-in help |
| `yy-buffer`, `yy-core`, `yy-io`, `yy-jobs` | Document buffers, editing, I/O, and background jobs |
| `yy-search`, `yy-delimited`, `yy-layout` | Encodings, search, CSV, and text layout |
| `yy-syntax`, `yy-preview` | Syntax highlighting and Markdown / HTML previews |
| `yy-config`, `yy-git` | Settings and Git operations |
| `yy-proto`, `yy-remote`, `yy-ssh` | Agent protocol, remote operations and transfers, and built-in SSH |
| `yy-term` | Terminal core |
| `yy-3270`, `yy-3270-tls`, `yy-3270-macro`, `yy-3270-mock` | 3270, TLS, macros, and a mock host for testing |
| `yy-sheet`, `yy-formula`, `yy-numfmt`, `yy-cobol` | Spreadsheet storage and editing, formulas, display formats, and COBOL fixed-length data |
| `yy-files` | Folder synchronization, catalogs, full-text indexes, file search, duplicate and version detection, deletion, and recovery |
| `yy-browser`, `yy-adblock` | Browser input, profiles, routing rules, bookmarks, and ad blocking |
| `tools/` | Large-file generation and benchmarks, asset fetching, and icon generation |
| `docs/history/proposals/` | Design proposals, requirements, and validation plans by topic |

[Cargo.toml](../../Cargo.toml) is the authoritative package and dependency list. When changing shared UI, verify behavior in the applications that use it. yyclip has a separate tray UI; the other main applications reuse yy-win.

## Investigating Huge Files and Rendering

Use `tools/gen-bigfile` to generate data and measure performance. Check available disk space before generating large files.

```sh
cargo run --release -p gen-bigfile --bin gen-bigfile -- big.log 10G log
cargo run --release -p gen-bigfile --bin gen-bigfile -- sjis.txt 1G japanese cp932
cargo run --release -p gen-bigfile --bin open-bench -- big.log
```

Generation modes are `log`, `japanese`, `long`, `single`, and `csv`. The package also contains `csv-bench` and `replace-bench`. When recording measurements, include the build configuration, OS, CPU, memory, storage, input size, and encoding.

On Windows, render the first screen of a file offscreen and save it as a BMP:

```powershell
.\target\release\yyeditor.exe --render-bmp input.txt out.bmp
```

## Settings, Help, and Assets

Shared settings are stored in `%APPDATA%\yyeditor\config.toml`. Setting types and defaults are defined in `crates/yy-config`; user documentation is in the [yyeditor help](../../crates/yy-win/help/help.md) and [yysheet help](../../crates/yy-win/help/sheet.md), both in Japanese. yyclip stores data under `%APPDATA%\yyclip\`; see its [README](../../apps/yyclip/README.md) (Japanese).

- Syntax definitions: `crates/yy-syntax/syntaxes/`. User definitions: `%APPDATA%\yyeditor\syntax\*.toml`.
- External encoding mappings: `%APPDATA%\yyeditor\mappings\*.map`. Built-in table generation: [YYLibrary/tools/gen-tables](https://github.com/ykwyuta/YYLibrary/tree/master/tools/gen-tables).
- Embedded preview assets: `crates/yy-preview/assets/`. Fetch script: `tools/fetch-preview-assets/fetch.py`. Versions are recorded in `VERSIONS.txt`, with licenses in the same folder.
- Fonts: `crates/yy-win/fonts/`. UDEV Gothic is embedded and registered within the process.
- Application icons and manifests: `apps/<app>/res/`. Icon generator: `tools/gen-icon/gen_icon.py`. Applications share the embedding code in `apps/yyeditor/build.rs`.
- Connection log: `%APPDATA%\yyeditor\logs\remote-ssh.log`. Transfer log: `transfer.log` in the same folder.

Update the relevant built-in help when changing features or settings.

## File Manager and Browser Validation

Test the file manager core with `cargo test -p yy-files`, browser settings, rules, and bookmarks with `cargo test -p yy-browser`, and ad blocking with `cargo test -p yy-adblock`. Validate screens and WebView2 behavior on Windows.

- For folder synchronization, cover plans, conflicts, resume, delta transfer, quarantine, and recovery. Refer to the tests in `crates/yy-files` and CI for network-share test prerequisites.
- For the browser, cover direct connections, proxies and PAC, profile isolation, domain rules, host mapping, certificate fingerprints, bookmark HTML import / export, and filter updates and exceptions.
- User documentation is in the [file manager help](../../crates/yy-win/help/filemanager.md) and [browser help](../../crates/yy-win/help/browser.md), both in Japanese.
- File manager settings are under `[filemanager]` in the shared `config.toml`. Jobs, indexes, and deletion records are stored in `filemanager/` under the settings folder; the log is `logs/filemanager.log`.
- Browser profiles and filter settings are in `browser.toml`, bookmarks in `bookmarks.toml`, and development certificates in `browser-devcerts/`, all under the settings folder. Browsing data is stored per profile under `%LOCALAPPDATA%\yyeditor\yybrowser\`, including yybrowser's history (`yybrowser-history.tsv`) and download history (`yybrowser-downloads.tsv`); ad-block filter caches are in `filters\` there.

## CI and Distribution

[.github/workflows/ci.yml](../../.github/workflows/ci.yml) performs the following:

- Builds static Linux x86_64 / aarch64 agents and verifies their versions, hashes, and static linking.
- Runs formatting checks, Clippy, workspace tests, and release builds for the nine main Windows applications on Windows.
- Runs core tests, x3270 / GnuCOBOL compatibility checks, and Windows UI type checks on Linux.
- Runs the independent yymerge Windows/Linux checks, native UI, Office content comparison and embedded PDF rendering tests, and yydbmanager Windows/Linux checks in the same workflow. yydbmanager stays on `windows-2022` for DuckDB's C++ toolchain.
- Packages yyeditor / yyterm / yysftp / yysheet / yyfilemanager / yyclip / yybrowser / yydraw / yytoolbox / yydbmanager / yymerge / yymergew and both agent architectures in `yyeditor-windows-x64`, after all Windows component jobs pass. Component notices/help stay under `yymerge/` and `yydbmanager/`.
- Keeps the standalone yymerge/yydbmanager artifacts as well. Manual dispatch with `docker_databases=true` runs the five remote DB integration jobs.

yyclip, yyfilemanager, and yybrowser are included in CI builds, distribution artifacts, and Windows-target type checks.

Before distribution, review [COPYING](../../COPYING), [COPYING.EXCEPTION](../../COPYING.EXCEPTION), and the licenses of bundled materials. The additional permission for linking the Microsoft Edge WebView2 Loader is part of this repository's license terms.

## Shared library dependency

`yy-encoding` is maintained in [YYLibrary](https://github.com/ykwyuta/YYLibrary).
Cargo fetches the exact commit pinned by `rev` and Cargo.lock; no sibling checkout or submodule is required.
The shared encoding dependency requires Rust 1.88 or newer. Change the shared implementation in YYLibrary,
then update `rev` and the lockfiles in this workspace and the independent yymerge, yydbmanager and YY3Designer workspaces.
YYSort also consumes the same revision. Verify the corresponding CI workflows when updating it.


Independent workspaces and internal module boundaries: [architecture](architecture.md). User manuals: [user index](../user/README.md). Historical proposals and measurements: [history](../history/README.md).

## Editor comparison integration

Editor tab and Git comparisons launch the sibling `yymergew.exe` without a shell.
Immutable decoded snapshots are written as UTF-8 to a unique temporary directory on a worker thread,
with display names passed separately using `--left-label` / `--right-label`.
The worker waits for yymerge before deleting its inputs; launch and child errors are reported.
Large snapshots use yymerge's disk-backed two-way viewer. There is no write-back to editor buffers.
For local development, build yymerge and copy `yymergew.exe` alongside the debug/release `yyeditor.exe`.
