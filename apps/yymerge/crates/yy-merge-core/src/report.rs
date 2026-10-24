//! Standalone, inert HTML reviews. Rendering never normalizes or writes source files.
use crate::{Cancellation, MAX_BYTES, Result, alignment, files::Snapshot, rules};

struct Html {
    text: String,
    limit: usize,
}
impl Html {
    fn push(&mut self, text: &str) -> Result<()> {
        if text.len() > self.limit.saturating_sub(self.text.len()) {
            return Err("HTML report incomplete: 32 MiB output limit".into());
        }
        self.text.push_str(text);
        Ok(())
    }
    fn escaped(&mut self, text: &str, cancel: &Cancellation) -> Result<()> {
        let mut start = 0;
        for (count, (i, c)) in text.char_indices().enumerate() {
            if count & 1023 == 0 {
                cancel.check()?;
            }
            let replacement = match c {
                '&' => Some("&amp;"),
                '<' => Some("&lt;"),
                '>' => Some("&gt;"),
                '"' => Some("&quot;"),
                '\'' => Some("&#39;"),
                _ => None,
            };
            if let Some(value) = replacement {
                self.push(&text[start..i])?;
                self.push(value)?;
                start = i + c.len_utf8();
            } else if c.is_control() && c != '\t' {
                self.push(&text[start..i])?;
                self.push(&format!("\\u{{{:04X}}}", c as u32))?;
                start = i + c.len_utf8();
            }
        }
        self.push(&text[start..])
    }
    fn cell(&mut self, index: Option<usize>, lines: &[&str], cancel: &Cancellation) -> Result<()> {
        if let Some(index) = index {
            let line = lines.get(index).ok_or("invalid HTML alignment index")?;
            let (body, ending) = if let Some(body) = line.strip_suffix("\r\n") {
                (body, "CRLF")
            } else if let Some(body) = line.strip_suffix('\n') {
                (body, "LF")
            } else {
                (*line, "EOF")
            };
            self.push(&format!(
                "<td class=number>{}</td><td class=source data-line=\"{}\"><span class=content>",
                index + 1,
                index + 1
            ))?;
            self.escaped(body, cancel)?;
            self.push(&format!(
                "</span><span class=eol data-eol=\"{ending}\">{}</span></td>",
                if ending == "EOF" {
                    "改行なし"
                } else {
                    ending
                }
            ))?;
        } else {
            self.push("<td class=number></td><td class=missing aria-label=\"対応行なし\">—</td>")?;
        }
        Ok(())
    }
}

pub fn html(
    left: &Snapshot,
    right: &Snapshot,
    rules: rules::Rules,
    compared: &rules::Comparison,
    cancel: &Cancellation,
) -> Result<String> {
    render(left, right, rules, compared, cancel, MAX_BYTES as usize)
}
fn render(
    left: &Snapshot,
    right: &Snapshot,
    rules: rules::Rules,
    compared: &rules::Comparison,
    cancel: &Cancellation,
    limit: usize,
) -> Result<String> {
    cancel.check()?;
    let rows = alignment::align(&compared.diff, cancel)?;
    let a = crate::lines(&left.text);
    let b = crate::lines(&right.text);
    let identical = left.original_identical(right);
    let equivalent = left.equivalent(right, compared.diff.edits.is_empty());
    let (status, label) = if identical && equivalent {
        ("Identical", "元データ一致")
    } else if equivalent && left.office.is_some() && left.text == right.text {
        (
            "EquivalentOfficeContent",
            "Office抽出データ一致（元パッケージは異なります）",
        )
    } else if equivalent {
        (
            "EquivalentUnderRules",
            "規則上は同値（元データは異なります）",
        )
    } else {
        ("Different", "相違あり")
    };
    let mut out = Html {
        text: String::new(),
        limit,
    };
    out.push("<!doctype html><html lang=ja><head><meta charset=utf-8><meta name=viewport content=\"width=device-width, initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\"><meta name=generator content=\"yymerge HTML report v1\"><title>yymerge 比較レポート</title><style>")?;
    out.push(include_str!("report.css"))?;
    out.push("</style></head><body><header><h1>yymerge 比較レポート</h1>")?;
    out.push(&format!("<p class=status data-status=\"{status}\">{label}</p><p>変更領域 {} / 規則で無視した行対 {}</p><p>適用規則: ", compared.diff.edits.len(), compared.ignored.len()))?;
    out.escaped(&rules.label(), cancel)?;
    if let Some(mode) = left.review {
        out.push(&format!("</p><p>読み取り専用バイト表示 / 文字コード・BOM差を無視: {}。不正バイトは &#92;xHH、制御文字は &#92;u{{XXXX}}、元のバックスラッシュは二重化。パック十進数の数値解釈は行いません。", mode.ignore_encoding))?;
    }
    if left.office.is_some() {
        out.push("</p><p class=note>Office抽出データの読み取り専用比較です。表示行番号は原本の行番号ではありません。書式・画像・ノート等の内部データも比較しますが、Officeで描画した見た目の比較ではありません。数式は再計算せず、マクロ・外部リンクは実行しません。ZIPの順序・圧縮・日時だけの差は同値になります。元ファイルは変更しません。</p><p>")?;
    }
    out.push("</p><p class=note>比較用テキストの表示レポートです。同値は元バイト一致を意味しません。通常のテキスト入力では文字コードとBOMも判定します。行末のLF・CRLF・改行なしを明示し、タブは4桁幅、その他の制御文字は &#92;u{XXXX} で表示します。</p><dl>")?;
    for (side, input) in [("LEFT", left), ("RIGHT", right)] {
        out.push(&format!("<dt>{side}</dt><dd>"))?;
        out.escaped(&input.path.to_string_lossy(), cancel)?;
        out.push("<br>")?;
        if let Some(kind) = input.office {
            out.escaped(&format!("{}抽出 / 表示テキスト: ", kind.label()), cancel)?;
        }
        out.escaped(&input.encoding.spec(), cancel)?;
        out.push(&format!(" / BOM: {:?}</dd>", input.bom))?;
    }
    out.push("</dl><p class=legend>同一 / <span class=changed>変更</span> / <span class=deleted>左のみ</span> / <span class=inserted>右のみ</span> / <span class=ignored>≈ 規則で無視</span></p></header><main><div class=table-scroll><table><colgroup><col class=line-number><col><col class=line-number><col></colgroup><thead><tr><th colspan=2>LEFT（左表示）</th><th colspan=2>RIGHT（右表示）</th></tr></thead><tbody>")?;
    for (i, row) in rows.iter().enumerate() {
        if i & 1023 == 0 {
            cancel.check()?;
        }
        let ignored = row.kind == alignment::RowKind::Equal
            && row.left.and_then(|n| a.get(n)) != row.right.and_then(|n| b.get(n));
        let class = match row.kind {
            alignment::RowKind::Equal if ignored => "ignored",
            alignment::RowKind::Equal => "equal",
            alignment::RowKind::Changed => "changed",
            alignment::RowKind::LeftOnly => "deleted",
            alignment::RowKind::RightOnly => "inserted",
        };
        out.push(&format!("<tr class={class}>"))?;
        out.cell(row.left, &a, cancel)?;
        out.cell(row.right, &b, cancel)?;
        out.push("</tr>")?;
    }
    out.push("</tbody></table></div></main><footer>yymerge HTML report v1 · 全行を比較表示の順序で表示 · 空ファイルは0行 · 比較入力は変更しません</footer></body></html>\n")?;
    cancel.check()?;
    Ok(out.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inputs(a: &str, b: &str) -> (tempfile::TempDir, Snapshot, Snapshot) {
        let dir = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        let left = dir.path().join("LEFT & '日本語.txt");
        let right = dir.path().join("RIGHT.txt");
        std::fs::write(&left, a).unwrap();
        std::fs::write(&right, b).unwrap();
        let a = Snapshot::open(&left, None, &cancel).unwrap();
        let b = Snapshot::open(&right, None, &cancel).unwrap();
        (dir, a, b)
    }
    #[test]
    fn html_is_inert_and_preserves_original_rows_endings_and_rules() {
        let (_dir, a, b) = inputs(
            "<script>alert('x')</script>\r\nALPHA \t\r\nonly left\nfinal",
            "<script>alert('x')</script>\nalpha\nchanged\nfinal\nextra\n",
        );
        let cancel = Cancellation::default();
        let rules = rules::Rules {
            ignore_case: true,
            ignore_trailing_space: true,
            ignore_eol: true,
            ..Default::default()
        };
        let compared = rules::compare(&a.text, &b.text, rules, &cancel).unwrap();
        let text = html(&a, &b, rules, &compared, &cancel).unwrap();
        assert!(!text.contains("<script>"));
        assert!(text.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"));
        assert!(text.contains("LEFT &amp; &#39;日本語.txt"));
        assert!(text.contains("<tr class=ignored>"));
        assert!(text.contains("<tr class=changed>"));
        assert!(text.contains("<tr class=inserted>"));
        assert_eq!(text.matches("data-line=").count(), 9);
        assert!(text.contains("ALPHA \t"));
        for marker in ["CRLF", "LF", "EOF"] {
            assert!(text.contains(&format!("data-eol=\"{marker}\"")));
        }
        assert!(text.contains("data-status=\"Different\""));
        assert!(text.contains("default-src 'none'"));
        assert!(!text.contains("<script "));
    }
    #[test]
    fn equivalence_metadata_empty_files_and_missing_rows_are_distinct() {
        let cancel = Cancellation::default();
        let rules = rules::Rules {
            ignore_case: true,
            ..Default::default()
        };
        for (a, b, status, class) in [
            ("A\n", "a\n", "EquivalentUnderRules", "ignored"),
            ("", "", "Identical", ""),
            ("one\n", "", "Different", "deleted"),
        ] {
            let (_dir, a, mut b) = inputs(a, b);
            let compared = rules::compare(&a.text, &b.text, rules, &cancel).unwrap();
            let text = html(&a, &b, rules, &compared, &cancel).unwrap();
            assert!(text.contains(&format!("data-status=\"{status}\"")));
            if !class.is_empty() {
                assert!(text.contains(&format!("<tr class={class}>")));
            } else {
                assert!(!text.contains("data-line="));
            }
            b.bom = vec![0xef, 0xbb, 0xbf];
            assert!(
                html(&a, &b, rules, &compared, &cancel)
                    .unwrap()
                    .contains("data-status=\"Different\"")
            );
        }
    }
    #[test]
    fn escaped_expansion_budget_and_cancellation_never_return_partial_html() {
        let (_dir, a, b) = inputs(&"<&".repeat(5000), "x\r\u{1b}\u{7f}");
        let cancel = Cancellation::default();
        let rules = rules::Rules::default();
        let compared = rules::compare(&a.text, &b.text, rules, &cancel).unwrap();
        assert!(render(&a, &b, rules, &compared, &cancel, 8000).is_err());
        let text = html(&a, &b, rules, &compared, &cancel).unwrap();
        assert!(text.contains("x\\u{000D}\\u{001B}\\u{007F}"));
        cancel.cancel();
        assert_eq!(
            html(&a, &b, rules, &compared, &cancel).err().unwrap(),
            "cancelled"
        );
    }
}
