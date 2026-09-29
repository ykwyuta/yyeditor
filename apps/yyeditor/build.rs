//! アプリケーションマニフェストとアイコンを実行ファイルに埋め込む。
//!
//! マニフェストでは PerMonitorV2 DPI、長いパス、Common Controls v6、UTF-8 コードページを宣言する
//! （MSVC リンカーのみ）。
//!
//! アイコン（`res/yyeditor.ico`、`tools/gen-icon` で生成）は、リソースコンパイラーを使わずに
//! ここでリソースファイル（.res）に変換してリンクする（ID 1 の RT_GROUP_ICON。エクスプローラーと
//! ウィンドウのアイコンになる）。

use std::path::{Path, PathBuf};

/// リソースの種類
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// アイコンのリソース ID（yy-win の `APP_ICON_ID` と同じ）
const APP_ICON_ID: u16 = 1;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("res");
    let manifest = dir.join("yyeditor.manifest");
    let icon = dir.join("yyeditor.ico");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed={}", icon.display());
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    if target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
    let ico = std::fs::read(&icon).expect("res/yyeditor.ico");
    let res = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("yyeditor.res");
    std::fs::write(&res, icon_res(&ico)).expect("write yyeditor.res");
    if target_env == "msvc" {
        // link.exe は .res をそのまま受け付ける
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        // GNU ld は .res を読めないので、windres で COFF のオブジェクトにする
        let obj = res.with_extension("o");
        match to_coff(&res, &obj) {
            Ok(()) => println!("cargo:rustc-link-arg-bins={}", obj.display()),
            Err(e) => println!("cargo:warning=アイコンを埋め込めません（windres: {e}）"),
        }
    }
}

/// .res を windres（環境変数 `WINDRES` か、MinGW の windres）で COFF のオブジェクトにする。
fn to_coff(res: &Path, obj: &Path) -> Result<(), String> {
    println!("cargo:rerun-if-env-changed=WINDRES");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let candidates = match std::env::var("WINDRES") {
        Ok(w) => vec![w],
        Err(_) => vec![format!("{arch}-w64-mingw32-windres"), "windres".into()],
    };
    let mut last = String::new();
    for w in candidates {
        let status = std::process::Command::new(&w)
            .args(["-J", "res", "-O", "coff", "-i"])
            .arg(res)
            .arg("-o")
            .arg(obj)
            .status();
        match status {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => last = format!("{w}: {s}"),
            Err(e) => last = format!("{w}: {e}"),
        }
    }
    Err(last)
}

fn u16le(v: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([v[at], v[at + 1]])
}

fn u32le(v: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([v[at], v[at + 1], v[at + 2], v[at + 3]])
}

/// ICO ファイルを、各画像の RT_ICON と、それをまとめる RT_GROUP_ICON からなる .res にする。
fn icon_res(ico: &[u8]) -> Vec<u8> {
    assert_eq!(u16le(ico, 2), 1, "not an icon file");
    let count = u16le(ico, 4) as usize;
    // 先頭は空のリソース（.res ファイルの目印）
    let mut out = Vec::new();
    push_resource(&mut out, 0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for i in 0..count {
        let e = 6 + 16 * i;
        let size = u32le(ico, e + 8) as usize;
        let offset = u32le(ico, e + 12) as usize;
        let id = (i + 1) as u16;
        push_resource(&mut out, RT_ICON, id, 0x1010, &ico[offset..offset + size]);
        // GRPICONDIRENTRY: ICONDIRENTRY の先頭 12 バイト＋リソース ID
        group.extend_from_slice(&ico[e..e + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    push_resource(&mut out, RT_GROUP_ICON, APP_ICON_ID, 0x1030, &group);
    out
}

/// 種類・名前が番号のリソースを 1 つ書く。
fn push_resource(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    let header_size = 32u32;
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    for v in [0xFFFF, kind, 0xFFFF, name] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&flags.to_le_bytes()); // MemoryFlags
    // 言語: 英語（米国）。空のリソースは 0
    let lang: u16 = if kind == 0 { 0 } else { 0x0409 };
    out.extend_from_slice(&lang.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}
