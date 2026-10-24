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
Officeの実レンダリングはLibreOfficeを導入した専用試験で行い、通常試験のignoreを成功扱いにしません。
Git連携のプロセス試験は`apps/yymerge/app/tests/git_tools.rs`の一時リポジトリを使います。

| 作例 | 再生成コマンド | 生成時の追加ライブラリ |
| --- | --- | --- |
| Excel・PowerPoint | `python demo/yymerge/office/generate.py` | openpyxl、python-pptx、Pillow |
| 画像 | `python demo/yymerge/images/generate.py` | Pillow |
| PDF | `python demo/yymerge/pdf/generate.py` | Python標準ライブラリ |
| 暗号化PDF | `python demo/yymerge/pdf/generate-password.py` | pypdf |

これらのPythonライブラリは作例生成用で、yymergeによる比較実行には不要です。
日付付きの試験・測定結果は[検証履歴](../history/verification/yymerge-2026-10-24.md)へ分離しています。
