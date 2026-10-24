# yydraw のビルド・検証

コマンドはリポジトリルートから実行します。

```powershell
cargo run -p yydraw
.\target\debug\yydraw.exe diagram.yyd
```

```powershell
cargo run -p yy-draw --example sample -- target/yydraw-sample
```

公式クラウド素材の取得とカタログ再生成: `python tools/cloud-icons/fetch.py`。