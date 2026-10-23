# YY3Designer

ドット絵を描き、写真・イラストから変換し、厚みと最大6方向の画像で立体にするWindowsアプリです。ルートとは別のCargoワークスペース・ロックファイル・CIを持ちます。

- 鉛筆・消しゴム・塗りつぶし・スポイト・線・四角・楕円・範囲選択、コピー／貼り付け、反転・回転、Undo／Redo。
- 方向ごとのレイヤー、不透明度・表示・並び順、複数オブジェクト。
- 写真等を任意のドット寸法へ縮小し、8〜256色の減色・ディザリング。
- 1枚の押し出しから、正面・背面・左右・上下の輪郭の共通部分を使う3Dモデルまで。同じ原画から厚み・丸みを再調整。ドットごとの厚み削り・復元、6面と斜めの視点ボタン。
- `.yy3d`保存／読み込み、透過PNG、角丸PNG、OBJ＋MTL、3DプレビューPNG出力。
- 共通のUDEV Gothic、アイコン生成部品、ビルド時リソース埋め込み、MarkdownヘルプとWebView2なしのテキストヘルプを流用。

操作・座標・保存形式・制限は[同梱ヘルプ](help/help.md)を参照してください。丸みは外側の角と辺だけに付け、隣接ドットの接続を保ちます。写真からの奥行き推定には対応していません。

椅子・テーブル・子供・車・猫の編集できる作例は[demo/yy3designer](../../demo/yy3designer/README.md)にあります。配布版では`examples/examples.yy3d`を開いてください。

```powershell
cargo build --manifest-path apps/yy3designer/Cargo.toml --release --locked
apps/yy3designer/target/release/yy3designer.exe
```

```powershell
cargo fmt --manifest-path apps/yy3designer/Cargo.toml --all -- --check
cargo clippy --manifest-path apps/yy3designer/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path apps/yy3designer/Cargo.toml --workspace --locked
apps/yy3designer/target/release/yy3designer.exe --ui-smoke-test yy3-smoke
```

LinuxではGUI以外の編集・変換・保存・3Dコアを検証します。[専用CI](../../.github/workflows/yy3designer.yml)でWindows UIとLinuxのコア試験を分け、成果物も独立して出力します。
