# 開発者ガイド

[English](DEVELOPER.md) | 日本語

このリポジトリは Rust の Cargo ワークスペースです。アプリの用途と利用者向けの機能紹介は [README.ja.md](README.ja.md)、設計の背景は [docs/proposal/](docs/proposal/README.md) を参照してください。提案書には将来の計画も含まれるため、実装状況はソース・テスト・組み込みヘルプと照合してください。

## 開発環境

- Rust stable。`rust-toolchain.toml` で stable、rustfmt、Clippy を指定しています。
- Cargo ワークスペースは Rust 2024 edition、`rust-version = "1.85"` を指定しています。
- Windows アプリの通常のビルドには MSVC ツールチェーン、Visual Studio Build Tools の C++ ビルドツールと Windows SDK を用意してください。
- プレビューと yybrowser の実行・検証には Microsoft Edge WebView2 ランタイムが必要です。
- Linux では OS に依存しない中核ライブラリと yy-agent を開発・テストできます。GUI アプリの Linux 起動はサポートしていません。

## Windows アプリのビルドと起動

リポジトリのルートで実行します。

```powershell
cargo build --release -p yyeditor -p yyterm -p yysftp -p yysheet -p yyfilemanager -p yyclip -p yybrowser
```

実行ファイルは `target\release\` に生成されます。

```powershell
.\target\release\yyeditor.exe .\README.md
.\target\release\yyterm.exe .
.\target\release\yysftp.exe user@host:/path
.\target\release\yysheet.exe data.csv
.\target\release\yyclip.exe
.\target\release\yyfilemanager.exe
.\target\release\yybrowser.exe https://example.com
```

yyfilemanager は `--sync <ジョブ名>` で画面なしの同期を、yybrowser は `--profile <名前>`、`--proxy <指定>` と複数の URL を受け付けます。

yyeditor と yysheet はローカルファイルのほか `ssh://` のファイルを、yyterm はフォルダ・`ssh://` の接続先・`ユーザー@ホスト` を受け付けます。yysftp は `ssh://` の接続先、`ユーザー@ホスト:/パス`、`ユーザー@ホスト` を受け付けます。

開発中は `--release` を省略して `target\debug\` の実行ファイルを使えます。アプリ間の起動連携を確認する場合は、関連する実行ファイルを同じフォルダに揃えてください。

## リモート用エージェント

yy-agent は SSH のチャネルの標準入出力で通信する補助プログラムです。待ち受けサーバーとしては動作しません。

Linux の各アーキテクチャの環境で静的リンクの musl バイナリを作ります。

```sh
# x86_64 Linux
rustup target add x86_64-unknown-linux-musl
cargo build --release -p yy-agent --target x86_64-unknown-linux-musl

# aarch64 Linux
rustup target add aarch64-unknown-linux-musl
cargo build --release -p yy-agent --target aarch64-unknown-linux-musl
```

Windows アプリと同じフォルダに次の名前で配置します。

```text
agents/
  yy-agent-x86_64-linux
  yy-agent-aarch64-linux
```

元ファイルは `target/<ターゲット>/release/yy-agent` です。開発中は環境変数 `YY_AGENT_DIR` でエージェントのフォルダを指定できます。

接続時には CPU に合うバイナリを選び、接続先の `~/.yyeditor/agent/<版>-<ハッシュ>/yy-agent` に配置して SHA-256 を照合します。CI は x86_64 と aarch64 の Linux ランナーでそれぞれビルドします。

## 検証

CI と同じ基本チェックは以下です。

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

変更範囲に絞って確認する場合は `cargo test -p yy-buffer` など、対象のパッケージを指定してください。GUI の動作確認は Windows 上で行います。

Windows CI は WebView2 ランタイムを用意し、`YY_REQUIRE_WEBVIEW2=1` を設定してプレビューのテストを行います。さらに `YY_REQUIRE_SMB_SHARE=1` を指定し、yyfilemanager の共有フォルダを使うテストも必須にしています。

Linux の結合テストでは OpenSSH のクライアントと sftp-server、x3270 の s3270 / pr3287、GnuCOBOL を使用します。Ubuntu の CI で用意するパッケージと追加検証は次のとおりです。

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends openssh-client openssh-sftp-server s3270 pr3287 gnucobol3
cargo test --workspace
crates/yy-3270-mock/check-with-x3270.sh
crates/yy-cobol/check-with-gnucobol.sh
```

Linux から Windows UI の型検査をする場合は、CI と同じく次を実行できます。これは Windows でのビルド・実行テストとは別の検査です。

```sh
rustup target add x86_64-pc-windows-msvc
cargo check -p yy-win -p yyeditor -p yyterm -p yysftp -p yysheet -p yyfilemanager -p yyclip -p yybrowser --no-default-features --features yy-files/blake3-pure --target x86_64-pc-windows-msvc
```

この検査では Windows SDK を必要とする組み込み SSH や開発者用証明書生成などの既定機能を外します。BLAKE3 は `yy-files/blake3-pure` を指定し、Windows のアセンブラーを必要としない構成で型検査します。SSH・TLS を含む通常の構成は Windows の検証で確認します。

## リポジトリの構成

| 場所・クレート | 役割 |
| --- | --- |
| `apps/yyeditor`、`apps/yyterm`、`apps/yysftp`、`apps/yysheet`、`apps/yyfilemanager`、`apps/yybrowser` | Windows アプリのエントリーポイントとリソース |
| `apps/yyclip` | 独立したクリップボード常駐アプリ。履歴・定型文・お気に入りの実装 |
| `apps/yy-agent` | リモートでファイル操作を行うエージェント |
| `crates/yy-win` | Win32 / Direct2D / DirectWrite の共通 UI、各アプリの画面、組み込みヘルプ |
| `yy-buffer`、`yy-core`、`yy-io`、`yy-jobs` | 文書バッファ、編集、入出力、バックグラウンド処理 |
| `yy-encoding`、`yy-search`、`yy-delimited`、`yy-layout` | 文字コード、検索、CSV、テキストの表示レイアウト |
| `yy-syntax`、`yy-preview` | ハイライトと Markdown / HTML プレビュー |
| `yy-config`、`yy-git` | 設定と Git 操作 |
| `yy-proto`、`yy-remote`、`yy-ssh` | エージェントのプロトコル、リモート操作・転送、組み込み SSH |
| `yy-term` | ターミナルの中核 |
| `yy-3270`、`yy-3270-tls`、`yy-3270-macro`、`yy-3270-mock` | 3270、TLS、マクロ、試験用の模擬ホスト |
| `yy-sheet`、`yy-formula`、`yy-numfmt`、`yy-cobol` | 表の保管と編集、数式、表示形式、COBOL 固定長データ |
| `yy-files` | フォルダ同期、目録、全文索引、ファイル検索、重複・版の判定、削除と復元 |
| `yy-browser`、`yy-adblock` | ブラウザの入力・プロファイル・接続規則・ブックマーク、広告ブロック |
| `tools/` | 巨大ファイル生成・性能測定、文字コード表生成、アセット取得、アイコン生成 |
| `docs/proposal/` | 分野ごとの設計提案・要件・検証方針 |

パッケージ一覧と依存関係の正本は [Cargo.toml](Cargo.toml) です。共通 UI を変更した場合は、利用している複数アプリで動作を確認してください。yyclip は共通の yy-win UI を使う他の6アプリとは別の実装です。

## 巨大ファイル・描画の調査

データ生成と測定には `tools/gen-bigfile` を使います。大きなファイルを生成するため、保存先の空き容量を確認してください。

```sh
cargo run --release -p gen-bigfile --bin gen-bigfile -- big.log 10G log
cargo run --release -p gen-bigfile --bin gen-bigfile -- sjis.txt 1G japanese cp932
cargo run --release -p gen-bigfile --bin open-bench -- big.log
```

生成モードは `log`、`japanese`、`long`、`single`、`csv` です。`csv-bench` と `replace-bench` も同じパッケージに含まれます。計測値を記録する場合は、ビルド構成、OS、CPU、メモリ、ストレージ、入力データの大きさ・文字コードを併記してください。

Windows ではファイルの先頭画面を画面外に描画して BMP に出力できます。

```powershell
.\target\release\yyeditor.exe --render-bmp input.txt out.bmp
```

## 設定・ヘルプ・アセット

共通設定は `%APPDATA%\yyeditor\config.toml` です。設定の型と既定値は `crates/yy-config`、利用者向けの説明は [yyeditor ヘルプ](crates/yy-win/help/help.md) と [yysheet ヘルプ](crates/yy-win/help/sheet.md) にあります。yyclip の保存先は `%APPDATA%\yyclip\` で、詳細は [アプリの README](apps/yyclip/README.md) を参照してください。

- ハイライト定義: `crates/yy-syntax/syntaxes/`。利用者の追加定義は `%APPDATA%\yyeditor\syntax\*.toml`。
- 外部文字コード表: `%APPDATA%\yyeditor\mappings\*.map`。組み込み表の生成ツールは `tools/gen-tables`。
- プレビューの埋め込みアセット: `crates/yy-preview/assets/`。取得スクリプトは `tools/fetch-preview-assets/fetch.py`。バージョンは `VERSIONS.txt`、ライセンスは同じフォルダに収録。
- フォント: `crates/yy-win/fonts/`。UDEV Gothic を埋め込み、プロセス内で登録します。
- アプリのアイコンとマニフェスト: 各 `apps/<アプリ>/res/`。アイコン生成ツールは `tools/gen-icon/gen_icon.py`、埋め込み処理は `apps/yyeditor/build.rs` を各アプリで共有します。
- 接続ログ: `%APPDATA%\yyeditor\logs\remote-ssh.log`。転送ログ: 同じフォルダの `transfer.log`。

機能や設定を変更した場合は、対応する組み込みヘルプも更新してください。

## ファイル管理・ブラウザの検証

yyfilemanager の中核は `cargo test -p yy-files`、ブラウザの設定・規則・ブックマークは `cargo test -p yy-browser`、広告ブロックは `cargo test -p yy-adblock` で確認できます。画面や WebView2 を含む動作は Windows 上で確認してください。

- フォルダ同期では、計画、衝突、途中再開、差分転送、隔離と復元を対象にします。共有フォルダを使う試験の条件は `crates/yy-files` のテストと CI を参照してください。
- ブラウザでは、直接接続・プロキシ・PAC、プロファイル分離、ドメイン規則、ホスト転送、証明書の指紋照合、ブックマークの HTML 入出力、フィルタの更新・除外を確認します。
- 利用者向けの説明は [ファイル管理ヘルプ](crates/yy-win/help/filemanager.md) と [ブラウザヘルプ](crates/yy-win/help/browser.md) にあります。
- ファイル管理の設定は共通 `config.toml` の `[filemanager]`、ジョブ・索引・削除記録は設定フォルダの `filemanager/`、ログは `logs/filemanager.log` に保存します。
- ブラウザのプロファイル・フィルタ設定は設定フォルダの `browser.toml`、ブックマークは `bookmarks.toml`、開発者用証明書は `browser-devcerts/` に保存します。閲覧データは `%LOCALAPPDATA%\yyeditor\yybrowser\` にプロファイル別で保管します（yybrowser の閲覧履歴 `yybrowser-history.tsv`・ダウンロード履歴 `yybrowser-downloads.tsv` を含む。広告ブロックのフィルタのキャッシュは同じ場所の `filters\`）。

## CI と配布

[.github/workflows/ci.yml](.github/workflows/ci.yml) は次を行います。

- Linux x86_64 / aarch64 用の静的エージェントをビルドし、バージョン・ハッシュ・静的リンクを確認。
- Windows でフォーマット、Clippy、ワークスペースのテスト、7アプリのリリースビルドを実行。
- Linux で中核のテスト、x3270 / GnuCOBOL との照合、Windows UI の型検査を実行。
- `yyeditor-windows-x64` アーティファクトに yyeditor / yyterm / yysftp / yysheet / yyfilemanager / yyclip / yybrowser と両アーキテクチャのエージェントを収録。

yyclip、yyfilemanager、yybrowser も CI のビルド・配布・Windows 向け型検査に含まれています。

配布時は [COPYING](COPYING)、[COPYING.EXCEPTION](COPYING.EXCEPTION)、同梱素材のライセンスを確認してください。Microsoft Edge WebView2 Loader のリンクに関する追加の許可も、このリポジトリのライセンス条件の一部です。
