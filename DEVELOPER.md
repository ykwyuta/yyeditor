# Developer Guide

English | [日本語](DEVELOPER.ja.md)

This repository is a Rust Cargo workspace. See [README.md](README.md) for application features and [docs/proposal/](docs/proposal/README.md) for design background (Japanese). Proposals also include future plans, so verify implementation status against source code, tests, and built-in help.

## Development Environment

- Rust stable. `rust-toolchain.toml` selects stable with rustfmt and Clippy.
- The workspace uses Rust edition 2024 and declares `rust-version = "1.85"`.
- Normal Windows builds require the MSVC toolchain, Visual Studio Build Tools with C++ build tools, and the Windows SDK.
- Running and testing previews and yybrowser requires the Microsoft Edge WebView2 runtime.
- Linux can be used to develop and test the platform-independent core libraries and yy-agent. The GUI applications do not run on Linux.

## Building and Running Windows Applications

Run from the repository root:

```powershell
cargo build --release -p yyeditor -p yyterm -p yysftp -p yysheet -p yyfilemanager -p yyclip -p yybrowser
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
cargo check -p yy-win -p yyeditor -p yyterm -p yysftp -p yysheet -p yyfilemanager -p yyclip -p yybrowser --no-default-features --features yy-files/blake3-pure --target x86_64-pc-windows-msvc
```

This check disables default features such as built-in SSH and development-certificate generation that require the Windows SDK. `yy-files/blake3-pure` avoids requiring a Windows assembler for BLAKE3. Validate the normal configuration, including SSH and TLS, on Windows.

## Repository Structure

| Location / crate | Responsibility |
| --- | --- |
| `apps/yyeditor`, `apps/yyterm`, `apps/yysftp`, `apps/yysheet`, `apps/yyfilemanager`, `apps/yybrowser` | Windows application entry points and resources |
| `apps/yyclip` | Independent clipboard tray application: history, templates, and favorites |
| `apps/yy-agent` | Agent for remote file operations |
| `crates/yy-win` | Shared Win32 / Direct2D / DirectWrite UI, application screens, and built-in help |
| `yy-buffer`, `yy-core`, `yy-io`, `yy-jobs` | Document buffers, editing, I/O, and background jobs |
| `yy-encoding`, `yy-search`, `yy-delimited`, `yy-layout` | Encodings, search, CSV, and text layout |
| `yy-syntax`, `yy-preview` | Syntax highlighting and Markdown / HTML previews |
| `yy-config`, `yy-git` | Settings and Git operations |
| `yy-proto`, `yy-remote`, `yy-ssh` | Agent protocol, remote operations and transfers, and built-in SSH |
| `yy-term` | Terminal core |
| `yy-3270`, `yy-3270-tls`, `yy-3270-macro`, `yy-3270-mock` | 3270, TLS, macros, and a mock host for testing |
| `yy-sheet`, `yy-formula`, `yy-numfmt`, `yy-cobol` | Spreadsheet storage and editing, formulas, display formats, and COBOL fixed-length data |
| `yy-files` | Folder synchronization, catalogs, full-text indexes, file search, duplicate and version detection, deletion, and recovery |
| `yy-browser`, `yy-adblock` | Browser input, profiles, routing rules, bookmarks, and ad blocking |
| `tools/` | Large-file generation and benchmarks, encoding-table generation, asset fetching, and icon generation |
| `docs/proposal/` | Design proposals, requirements, and validation plans by topic |

[Cargo.toml](Cargo.toml) is the authoritative package and dependency list. When changing shared UI, verify behavior in the applications that use it. yyclip has a separate implementation from the other six applications using yy-win.

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

Shared settings are stored in `%APPDATA%\yyeditor\config.toml`. Setting types and defaults are defined in `crates/yy-config`; user documentation is in the [yyeditor help](crates/yy-win/help/help.md) and [yysheet help](crates/yy-win/help/sheet.md), both in Japanese. yyclip stores data under `%APPDATA%\yyclip\`; see its [README](apps/yyclip/README.md) (Japanese).

- Syntax definitions: `crates/yy-syntax/syntaxes/`. User definitions: `%APPDATA%\yyeditor\syntax\*.toml`.
- External encoding mappings: `%APPDATA%\yyeditor\mappings\*.map`. Built-in table generation: `tools/gen-tables`.
- Embedded preview assets: `crates/yy-preview/assets/`. Fetch script: `tools/fetch-preview-assets/fetch.py`. Versions are recorded in `VERSIONS.txt`, with licenses in the same folder.
- Fonts: `crates/yy-win/fonts/`. UDEV Gothic is embedded and registered within the process.
- Application icons and manifests: `apps/<app>/res/`. Icon generator: `tools/gen-icon/gen_icon.py`. Applications share the embedding code in `apps/yyeditor/build.rs`.
- Connection log: `%APPDATA%\yyeditor\logs\remote-ssh.log`. Transfer log: `transfer.log` in the same folder.

Update the relevant built-in help when changing features or settings.

## File Manager and Browser Validation

Test the file manager core with `cargo test -p yy-files`, browser settings, rules, and bookmarks with `cargo test -p yy-browser`, and ad blocking with `cargo test -p yy-adblock`. Validate screens and WebView2 behavior on Windows.

- For folder synchronization, cover plans, conflicts, resume, delta transfer, quarantine, and recovery. Refer to the tests in `crates/yy-files` and CI for network-share test prerequisites.
- For the browser, cover direct connections, proxies and PAC, profile isolation, domain rules, host mapping, certificate fingerprints, bookmark HTML import / export, and filter updates and exceptions.
- User documentation is in the [file manager help](crates/yy-win/help/filemanager.md) and [browser help](crates/yy-win/help/browser.md), both in Japanese.
- File manager settings are under `[filemanager]` in the shared `config.toml`. Jobs, indexes, and deletion records are stored in `filemanager/` under the settings folder; the log is `logs/filemanager.log`.
- Browser profiles and filter settings are in `browser.toml`, bookmarks in `bookmarks.toml`, and development certificates in `browser-devcerts/`, all under the settings folder. Browsing data is stored per profile under `%LOCALAPPDATA%\yyeditor\yybrowser\`, including yybrowser's history (`yybrowser-history.tsv`) and download history (`yybrowser-downloads.tsv`); ad-block filter caches are in `filters\` there.

## CI and Distribution

[.github/workflows/ci.yml](.github/workflows/ci.yml) performs the following:

- Builds static Linux x86_64 / aarch64 agents and verifies their versions, hashes, and static linking.
- Runs formatting checks, Clippy, workspace tests, and release builds for all seven applications on Windows.
- Runs core tests, x3270 / GnuCOBOL compatibility checks, and Windows UI type checks on Linux.
- Packages yyeditor / yyterm / yysftp / yysheet / yyfilemanager / yyclip / yybrowser and both agent architectures in the `yyeditor-windows-x64` artifact.

yyclip, yyfilemanager, and yybrowser are included in CI builds, distribution artifacts, and Windows-target type checks.

Before distribution, review [COPYING](COPYING), [COPYING.EXCEPTION](COPYING.EXCEPTION), and the licenses of bundled materials. The additional permission for linking the Microsoft Edge WebView2 Loader is part of this repository's license terms.
