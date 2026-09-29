//! シンタックスハイライトの結果を表示行の色に変換する（10 章 6）。

use yy_config::Colors;
use yy_layout::Row;
use yy_syntax::{LineTokens, Syntax};

use crate::render::RowTokens;

/// トークンの種類ごとの色の番号（[`Colors::syntax`] の順。色のないトークンは `None`）。
/// 細分類（`keyword.control`）の色がなければ親（`keyword`）の色を使う。
pub(crate) fn token_palette(syntax: &Syntax, colors: &Colors) -> Vec<Option<u16>> {
    let keys: Vec<&String> = colors.syntax.keys().collect();
    syntax
        .token_names()
        .iter()
        .map(|name| {
            let mut n = name.as_str();
            loop {
                if let Some(i) = keys.iter().position(|k| k.as_str() == n) {
                    return Some(i as u16);
                }
                n = &n[..n.rfind('.')?];
            }
        })
        .collect()
}

/// 表示行ごとのトークンの色（`Row::text` 内の範囲）。`lines` は表示範囲を含む論理行。
pub(crate) fn row_tokens(
    rows: &[Row],
    lines: &[LineTokens],
    palette: &[Option<u16>],
) -> Vec<RowTokens> {
    rows.iter()
        .map(|row| {
            let k = lines.partition_point(|l| l.start <= row.start);
            let Some(line) = k.checked_sub(1).map(|k| &lines[k]) else {
                return Vec::new();
            };
            let mut v = Vec::new();
            for s in &line.spans {
                let a = line.start + s.range.start as u64;
                let b = line.start + s.range.end as u64;
                if b <= row.start || a >= row.end {
                    continue;
                }
                let Some(Some(color)) = palette.get(s.token as usize) else {
                    continue;
                };
                let ta = row.text_index(a.max(row.start));
                let tb = row.text_index(b.min(row.end));
                if tb > ta {
                    v.push((ta..tb, *color));
                }
            }
            v
        })
        .collect()
}
