//! プレビューのページと埋め込みファイルをフォルダに書き出す（ブラウザでの確認用）。
//!
//!     cargo run -p yy-preview --example dump -- <入力.md> <出力フォルダ>
//!
//! 出力フォルダの `index.html` と `assets/` を `https://yy-preview.local/` として配信すると、
//! エディタのプレビューと同じ表示になる。

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: dump <input.md> <out-dir>");
        std::process::exit(2);
    };
    let out = PathBuf::from(out);
    let src = std::fs::read_to_string(&input).expect("read input");
    let reg = yy_syntax::Registry::builtin();
    let body = yy_preview::markdown_to_html(&src, Some(&reg));
    let opts = yy_preview::PageOptions {
        has_folder: false,
        token_colors: vec![
            ("keyword".into(), "#0000FF".into()),
            ("string".into(), "#A31515".into()),
            ("comment".into(), "#008000".into()),
        ],
    };
    std::fs::create_dir_all(out.join("assets/fonts")).unwrap();
    std::fs::write(
        out.join("index.html"),
        yy_preview::markdown_page(&body, &opts),
    )
    .unwrap();
    std::fs::write(out.join("body.html"), &body).unwrap();
    let mut names: Vec<String> = [
        "mermaid.min.js",
        "katex.min.js",
        "katex.min.css",
        "preview.js",
        "preview.css",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for e in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts")).unwrap() {
        names.push(format!(
            "fonts/{}",
            e.unwrap().file_name().to_string_lossy()
        ));
    }
    for n in names {
        let (data, _) = yy_preview::asset(&format!("/assets/{n}")).expect(&n);
        std::fs::write(out.join("assets").join(&n), &*data).unwrap();
    }
}
