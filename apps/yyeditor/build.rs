//! アプリケーションマニフェストを実行ファイルに埋め込む（MSVC リンカーのみ）。
//!
//! マニフェストでは PerMonitorV2 DPI、長いパス、Common Controls v6、UTF-8 コードページを宣言する。

fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res/yyeditor.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
