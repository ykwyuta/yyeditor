# YY3Designer のビルド・検証

コマンドはリポジトリルートから実行します。

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

LinuxではGUI以外のコアを検証する。Windows UIとLinuxコアのCIは `.github/workflows/yy3designer.yml`。
## 作例の生成

サンプルは外部素材を使わず、次のプログラムで再生成します。

```powershell
cargo run --manifest-path apps/yy3designer/Cargo.toml -p yy3-core --example make_demo --locked -- demo/yy3designer
cargo run --manifest-path apps/yy3designer/Cargo.toml -p yy3-core --example scenes --locked -- demo/yy3designer
```
