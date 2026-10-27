# yymerge のビルド・検証

コマンドはリポジトリルートから実行します。

```powershell
cargo build --manifest-path apps/yymerge/Cargo.toml --release --locked
apps/yymerge/target/release/yymergew.exe
apps/yymerge/target/release/yymerge.exe diff demo/yymerge/left.txt demo/yymerge/right.txt
```

```sh
cargo build --manifest-path apps/yymerge/Cargo.toml -p yymerge --no-default-features --bin yymerge --release --locked
cargo test --manifest-path apps/yymerge/Cargo.toml --workspace --locked
```
## 検査と作例の生成

```text
cargo fmt --manifest-path apps/yymerge/Cargo.toml --all -- --check
cargo clippy --manifest-path apps/yymerge/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path apps/yymerge/Cargo.toml --workspace --locked
```

Linuxは`--no-default-features`を付けたヘッドレス構成も検証します。
Excel・PowerPointは内容比較のみです。PDFは内蔵Hayroで直接描画し、通常のコア・CLI試験とWindows UIスモークで検証します。外部Office変換ツールは不要です。
Git連携のプロセス試験は`apps/yymerge/app/tests/git_tools.rs`の一時リポジトリを使います。

| 作例 | 再生成コマンド | 生成時の追加ライブラリ |
| --- | --- | --- |
| Excel・PowerPoint | `python demo/yymerge/office/generate.py` | openpyxl、python-pptx、Pillow |
| 画像 | `python demo/yymerge/images/generate.py` | Pillow |
| PDF | `python demo/yymerge/pdf/generate.py` | Python標準ライブラリ |
| 暗号化PDF | `python demo/yymerge/pdf/generate-password.py` | pypdf |

これらのPythonライブラリは作例生成用で、yymergeによる比較実行には不要です。
日付付きの試験・測定結果は[検証履歴](../history/verification/yymerge-2026-10-24.md)へ分離しています。

検証ジョブは[共通CI](../../.github/workflows/ci.yml)に統合しています。Cargoワークスペースとlockfileは独立したままです。

## 巨大テキストとリモート入力

`yy-merge-core::large`は不変のUTF-8作業ファイルをマップし、近傍32行の整列結果を49バイト固定レコードのディスク索引へ出力します。
相違行番号の8バイト索引を別に持ち、次／前の相違を二分探索します。ハッシュ一致後に本文を照合し、改行と終端改行を含めて判定します。
近傍整列を最短編集列や3方向自動マージへ流用しません。`files::Snapshot`の通常の安全境界は維持します。

読み終えた自前の不変ファイルマップだけをページ単位で解放します。Unixは`madvise(MADV_DONTNEED)`、
Windowsは[VirtualUnlock](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualunlock)を使います。
Windowsの未ロックページでは戻り値が失敗でもワーキングセットから除去されることが公式仕様です。
匿名・ヒープ領域や変更済み私有ページにはこの操作を行いません。

`remote`はyy-config・yy-remote・yy-sshを再利用します。SFTPのパイプライン転送と事前事後の属性確認を使い、
URIの拡張子を保持した専用一時フォルダーで入力を管理します。GUIの転送・索引作成はワーカー上で実行し、
中止・終了後にワーカーとファイルハンドルを回収してから作業領域を削除します。

Linuxの`YYMERGE_REQUIRE_SFTP=1`でSFTP試験の必須導入を検査します。
[ssh-live.sh](../../apps/yymerge/tests/ssh-live.sh)は実SSHを使う隔離コンテナ専用試験です。
パッケージ導入と試験ユーザー作成を伴うため、ホストで直接実行しないでください。
共通CIはUbuntuコンテナ内でローカル対リモート・リモート同士の40 MiB比較と、変更されたホスト鍵の拒否を検証します。
Windowsは通常画面に加え、32 MiB超の自動切替・索引読み取り・実描画・リサイズを検証します。

容量・実メモリ・実SSHの測定条件と結果は[検証記録](../history/verification/yymerge-large-remote-2026-10-24.md)を参照してください。
