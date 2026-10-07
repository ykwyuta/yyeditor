//! コピーブック（IBM Enterprise COBOL のデータ記述）の解析。
//!
//! - 固定形式（1〜6 桁は一連番号、7 桁目は標識〔`*`・`/` は注記、`-` は継続、`D` はデバッグ行〕、
//!   8〜72 桁がコード）と自由形式を自動で判別する。`*>` 以降は注記。全角文字は 2 桁に数える。
//! - 句: `PIC`・`USAGE`（`COMP`・`COMP-1`〜`5`・`BINARY`・`PACKED-DECIMAL`・`DISPLAY`・`DISPLAY-1`・
//!   `NATIONAL`・`POINTER`・`INDEX`）・`REDEFINES`・`OCCURS`（`DEPENDING ON` は最大の回数の固定長と
//!   して読む）・`SIGN [IS] LEADING / TRAILING [SEPARATE]`・`SYNC`・`JUSTIFIED`・`BLANK WHEN ZERO`・
//!   `VALUE`（読み飛ばす）。集団項目の `USAGE`・`SIGN` は従属する項目に引き継ぐ。
//! - 88（条件名）は読み飛ばし、66（`RENAMES`）は使わない。`COPY` 文・`EJECT`・`SKIP1` なども飛ばす。

use crate::pic::{Attrs, Usage, kind_of};
use crate::{Field, Kind, Layout, SignPos};

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Lit,
    Period,
}

/// 表示の幅（全角は 2）。
fn char_width(c: char) -> usize {
    if c.is_ascii() || ('\u{FF61}'..='\u{FF9F}').contains(&c) {
        1
    } else {
        2
    }
}

/// 桁（1 始まり）で `from..=to` の部分。
fn columns(line: &str, from: usize, to: usize) -> String {
    let mut col = 1;
    let mut out = String::new();
    for c in line.chars() {
        let w = char_width(c);
        if col >= from && col + w - 1 <= to {
            out.push(c);
        }
        col += w;
        if col > to {
            break;
        }
    }
    out
}

/// `*>` から後ろを除く（引用符の中は除かない）。`quote` は行の始めの引用符の状態。
fn strip_inline_comment(s: &str, quote: &mut Option<char>) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match *quote {
            Some(q) => {
                if c == q {
                    *quote = None;
                }
            }
            None => {
                if c == '*' && it.peek() == Some(&'>') {
                    break;
                }
                if c == '"' || c == '\'' {
                    *quote = Some(c);
                }
            }
        }
        out.push(c);
    }
    out
}

/// 固定形式か（7 桁目が標識の位置に見える行が多く、8 桁目以降から始まる行がある）。
fn is_fixed(lines: &[&str]) -> bool {
    let body: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    if body.is_empty() {
        return false;
    }
    let ok = body
        .iter()
        .filter(|l| {
            let c: Vec<char> = l.chars().collect();
            c.len() < 7 || matches!(c[6], ' ' | '*' | '/' | '-' | 'D' | 'd')
        })
        .count();
    let indented = body
        .iter()
        .any(|l| l.chars().position(|c| c != ' ').is_some_and(|p| p >= 7));
    ok == body.len() || (ok * 10 >= body.len() * 9 && indented)
}

/// コードの部分をつなげた文字列。
fn code_text(src: &str) -> String {
    let lines: Vec<&str> = src.lines().map(|l| l.trim_end_matches('\r')).collect();
    let fixed = is_fixed(&lines);
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for l in lines {
        if fixed {
            let ind = l.chars().nth(6).unwrap_or(' ');
            if matches!(ind, '*' | '/' | 'D' | 'd') {
                continue;
            }
            let area = columns(l, 8, 72);
            if ind == '-' {
                // 継続行: 引用符の中なら、次の引用符の後から続ける
                let t = area.trim_start();
                if let Some(q) = quote
                    && let Some(rest) = t.strip_prefix(q)
                {
                    // 前の行の終わりの空白は文字列の一部だが、桁の詰めのものとして捨てる
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push_str(&strip_inline_comment(rest, &mut quote));
                } else {
                    out.push(' ');
                    out.push_str(&strip_inline_comment(t, &mut quote));
                }
                continue;
            }
            out.push(' ');
            out.push_str(&strip_inline_comment(&area, &mut quote));
        } else {
            let t = l.trim_start();
            if t.starts_with("*>") || t.starts_with('*') && l.starts_with('*') {
                continue;
            }
            out.push(' ');
            out.push_str(&strip_inline_comment(l, &mut quote));
        }
    }
    out
}

fn tokenize(code: &str) -> Vec<Tok> {
    let c: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let sep_end = |j: usize| j >= c.len() || c[j].is_whitespace();
    while i < c.len() {
        if c[i].is_whitespace() || ((c[i] == ',' || c[i] == ';') && sep_end(i + 1)) {
            i += 1;
            continue;
        }
        if c[i] == '.' && sep_end(i + 1) {
            out.push(Tok::Period);
            i += 1;
            continue;
        }
        if c[i] == '"' || c[i] == '\'' {
            let q = c[i];
            i += 1;
            while i < c.len() {
                if c[i] == q {
                    if c.get(i + 1) == Some(&q) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Tok::Lit);
            continue;
        }
        // 語（中の引用符の部分も含める: X'40'・N'…'）
        let start = i;
        let mut lit = false;
        while i < c.len() && !c[i].is_whitespace() {
            if c[i] == '"' || c[i] == '\'' {
                lit = true;
                let q = c[i];
                i += 1;
                while i < c.len() && c[i] != q {
                    i += 1;
                }
                i += 1;
                continue;
            }
            if (c[i] == '.' || c[i] == ',' || c[i] == ';') && sep_end(i + 1) {
                break;
            }
            i += 1;
        }
        let w: String = c[start..i.min(c.len())].iter().collect();
        if lit {
            out.push(Tok::Lit);
        } else if !w.is_empty() {
            out.push(Tok::Word(w));
        }
    }
    out
}

/// 句の始めの語。
fn is_clause(w: &str) -> bool {
    matches!(
        w,
        "PIC"
            | "PICTURE"
            | "USAGE"
            | "REDEFINES"
            | "OCCURS"
            | "SIGN"
            | "LEADING"
            | "TRAILING"
            | "SYNC"
            | "SYNCHRONIZED"
            | "JUST"
            | "JUSTIFIED"
            | "BLANK"
            | "VALUE"
            | "VALUES"
            | "GLOBAL"
            | "EXTERNAL"
            | "RENAMES"
            | "DATE"
            | "GROUP-USAGE"
    ) || Usage::from_word(w).is_some()
}

/// データ記述項。
#[derive(Clone, Debug, Default)]
struct Entry {
    level: u32,
    name: String,
    filler: bool,
    pic: Option<String>,
    usage: Option<Usage>,
    sign: Option<SignPos>,
    justified: bool,
    blank_zero: bool,
    redefines: Option<String>,
    occurs: u32,
    sync: bool,
    children: Vec<Entry>,
    /// 1 回分の大きさ（計算したもの）
    size: usize,
    /// 子の、集団の先頭からの位置
    child_off: Vec<usize>,
    /// 基本項目の型
    kind: Option<Kind>,
    describe: String,
}

impl Entry {
    fn is_group(&self) -> bool {
        !self.children.is_empty()
    }
}

/// 1 つの記述項（ピリオドまで）を読む。読み飛ばすものは `None`。
fn entry(toks: &[Tok], warnings: &mut Vec<String>) -> Result<Option<Entry>, String> {
    let words: Vec<Option<String>> = toks
        .iter()
        .map(|t| match t {
            Tok::Word(w) => Some(w.to_ascii_uppercase()),
            _ => None,
        })
        .collect();
    let Some(Some(first)) = words.first() else {
        return Ok(None);
    };
    let Ok(level) = first.parse::<u32>() else {
        match first.as_str() {
            "COPY" => warnings.push(format!(
                "COPY 文は読み込みません（{}）",
                words.get(1).cloned().flatten().unwrap_or_default()
            )),
            "REPLACE" | "EJECT" | "SKIP1" | "SKIP2" | "SKIP3" | "TITLE" => {}
            w => warnings.push(format!("読めない文を飛ばしました: {w}")),
        }
        return Ok(None);
    };
    if level == 88 {
        return Ok(None);
    }
    if level == 66 {
        warnings.push("66（RENAMES）は使いません".into());
        return Ok(None);
    }
    if !(1..=49).contains(&level) && level != 77 {
        return Err(format!("レベル番号 {level} は使えません"));
    }
    let mut e = Entry {
        level,
        occurs: 1,
        ..Default::default()
    };
    let mut i = 1;
    match words.get(1) {
        Some(Some(w)) if !is_clause(w) => {
            e.filler = w == "FILLER";
            e.name = if let Some(Tok::Word(orig)) = toks.get(1) {
                orig.to_ascii_uppercase()
            } else {
                w.clone()
            };
            i = 2;
        }
        _ => e.filler = true,
    }
    if e.filler {
        e.name = "FILLER".into();
    }
    let word = |k: usize| words.get(k).cloned().flatten();
    let raw = |k: usize| match toks.get(k) {
        Some(Tok::Word(w)) => Some(w.clone()),
        _ => None,
    };
    // 句でない語を飛ばす
    let skip_args = |mut k: usize| {
        while k < words.len() {
            if let Some(w) = &words[k]
                && is_clause(w)
            {
                break;
            }
            k += 1;
        }
        k
    };
    while i < words.len() {
        let Some(w) = word(i) else {
            i += 1;
            continue;
        };
        i += 1;
        match w.as_str() {
            "PIC" | "PICTURE" => {
                if word(i).as_deref() == Some("IS") {
                    i += 1;
                }
                e.pic = Some(raw(i).ok_or_else(|| format!("{} の PIC が読めません", e.name))?);
                i += 1;
            }
            "USAGE" => {
                if word(i).as_deref() == Some("IS") {
                    i += 1;
                }
                let u = word(i).unwrap_or_default();
                e.usage = Some(
                    Usage::from_word(&u)
                        .ok_or_else(|| format!("{} の USAGE {u} には対応していません", e.name))?,
                );
                i += 1;
            }
            "REDEFINES" => {
                e.redefines = word(i);
                i += 1;
            }
            "OCCURS" => {
                let n: u32 = word(i)
                    .and_then(|w| w.parse().ok())
                    .ok_or_else(|| format!("{} の OCCURS の回数が読めません", e.name))?;
                i += 1;
                e.occurs = n;
                if word(i).as_deref() == Some("TO") {
                    e.occurs = word(i + 1)
                        .and_then(|w| w.parse().ok())
                        .ok_or_else(|| format!("{} の OCCURS TO の回数が読めません", e.name))?;
                    i += 2;
                }
                if word(i).as_deref() == Some("TIMES") {
                    i += 1;
                }
                if word(i).as_deref() == Some("DEPENDING") {
                    warnings.push(format!(
                        "{}: OCCURS DEPENDING ON は最大の {} 回の固定長として読みます",
                        e.name, e.occurs
                    ));
                }
                if e.occurs == 0 || e.occurs > 100_000 {
                    return Err(format!("{} の OCCURS の回数が範囲外です", e.name));
                }
                i = skip_args(i);
            }
            "SIGN" | "LEADING" | "TRAILING" => {
                let mut k = if w == "SIGN" { i } else { i - 1 };
                if word(k).as_deref() == Some("IS") {
                    k += 1;
                }
                let lead = match word(k).as_deref() {
                    Some("LEADING") => true,
                    Some("TRAILING") => false,
                    _ => return Err(format!("{} の SIGN 句が読めません", e.name)),
                };
                k += 1;
                let sep = word(k).as_deref() == Some("SEPARATE");
                if sep {
                    k += 1;
                    if word(k).as_deref() == Some("CHARACTER") {
                        k += 1;
                    }
                }
                e.sign = Some(match (lead, sep) {
                    (true, true) => SignPos::LeadingSeparate,
                    (true, false) => SignPos::Leading,
                    (false, true) => SignPos::TrailingSeparate,
                    (false, false) => SignPos::Trailing,
                });
                i = k;
            }
            "SYNC" | "SYNCHRONIZED" => {
                e.sync = true;
                if matches!(word(i).as_deref(), Some("LEFT" | "RIGHT")) {
                    i += 1;
                }
            }
            "JUST" | "JUSTIFIED" => {
                e.justified = true;
                if word(i).as_deref() == Some("RIGHT") {
                    i += 1;
                }
            }
            "BLANK" => {
                if word(i).as_deref() == Some("WHEN") {
                    i += 1;
                }
                e.blank_zero = true;
                i += 1;
            }
            "VALUE" | "VALUES" | "DATE" | "GROUP-USAGE" => i = skip_args(i),
            "GLOBAL" | "EXTERNAL" | "IS" => {}
            "RENAMES" => i = words.len(),
            w => match Usage::from_word(w) {
                Some(u) => e.usage = Some(u),
                None => warnings.push(format!("{} の {w} は読み飛ばしました", e.name)),
            },
        }
    }
    Ok(Some(e))
}

/// 型と大きさを求める（`usage`・`sign` は集団から引き継いだもの）。
fn size(
    e: &mut Entry,
    usage: Option<Usage>,
    sign: Option<SignPos>,
    w: &mut Vec<String>,
) -> Result<(), String> {
    let usage = e.usage.or(usage);
    let sign = e.sign.or(sign);
    if e.is_group() {
        let mut cur = 0usize;
        let mut end = 0usize;
        let mut offs = Vec::with_capacity(e.children.len());
        for k in 0..e.children.len() {
            size(&mut e.children[k], usage, sign, w)?;
            let c = &e.children[k];
            let off = match &c.redefines {
                Some(target) => {
                    let t = e.children[..k]
                        .iter()
                        .position(|s| s.name.eq_ignore_ascii_case(target))
                        .ok_or_else(|| {
                            format!("{} が REDEFINES する {target} がありません", c.name)
                        })?;
                    offs[t]
                }
                None => {
                    // SYNC の 2 進数・浮動小数点は大きさの境界に揃える（集団の先頭から数える）
                    let align = if c.sync && !c.is_group() {
                        match c.kind {
                            Some(Kind::Binary { bytes, .. }) => bytes,
                            Some(Kind::Float { double }) => {
                                if double {
                                    8
                                } else {
                                    4
                                }
                            }
                            _ => 1,
                        }
                    } else {
                        1
                    };
                    cur.div_ceil(align) * align
                }
            };
            let span = c.size * c.occurs as usize;
            if c.redefines.is_none() {
                cur = off + span;
            }
            end = end.max(off + span);
            offs.push(off);
        }
        e.size = end;
        e.child_off = offs;
        if e.pic.is_some() {
            w.push(format!("{}: 集団項目の PIC は使いません", e.name));
        }
        return Ok(());
    }
    let attrs = Attrs {
        usage,
        sign,
        justified: e.justified,
        blank_zero: e.blank_zero,
    };
    let (kind, len) = kind_of(e.pic.as_deref(), &attrs).map_err(|m| format!("{}: {m}", e.name))?;
    e.describe = match (&e.pic, usage) {
        (Some(p), Some(u)) if u != Usage::Display => {
            format!("{} {}", p.to_ascii_uppercase(), u.name())
        }
        (Some(p), _) => {
            let s = match sign {
                Some(SignPos::Leading) => " SIGN LEADING",
                Some(SignPos::LeadingSeparate) => " SIGN LEADING SEPARATE",
                Some(SignPos::TrailingSeparate) => " SIGN TRAILING SEPARATE",
                _ => "",
            };
            let signed = matches!(kind, Kind::Zoned { signed: true, .. });
            format!("{}{}", p.to_ascii_uppercase(), if signed { s } else { "" })
        }
        (None, Some(u)) => u.name().to_string(),
        (None, None) => String::new(),
    };
    e.kind = Some(kind);
    e.size = len;
    Ok(())
}

/// 基本項目を並べる。
struct Flat {
    fields: Vec<(Field, String, String, String)>,
}

fn place(e: &Entry, at: usize, subs: &[u32], parent: &str, out: &mut Flat) {
    if e.is_group() {
        let me = if e.filler { parent } else { e.name.as_str() };
        for (k, c) in e.children.iter().enumerate() {
            if c.redefines.is_some() {
                continue;
            }
            for n in 0..c.occurs {
                let mut s = subs.to_vec();
                if c.occurs > 1 {
                    s.push(n + 1);
                }
                place(c, at + e.child_off[k] + n as usize * c.size, &s, me, out);
            }
        }
        return;
    }
    let sub = if subs.is_empty() {
        String::new()
    } else {
        format!(
            "({})",
            subs.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    out.fields.push((
        Field {
            name: format!("{}{sub}", e.name),
            offset: at,
            len: e.size,
            kind: e.kind.clone().unwrap_or(Kind::Alnum { justified: false }),
            describe: e.describe.clone(),
        },
        e.name.clone(),
        sub,
        parent.to_string(),
    ));
}

/// コピーブックを読んで、最初のレコードのレイアウトを作る。
pub fn parse(src: &str) -> Result<Layout, String> {
    let code = code_text(src);
    let toks = tokenize(&code);
    let mut warnings = Vec::new();
    let mut entries = Vec::new();
    for stmt in toks.split(|t| *t == Tok::Period) {
        // ピリオドのない EJECT などは語ごとに飛ばす
        let mut s = stmt;
        while let Some(Tok::Word(w)) = s.first() {
            if matches!(
                w.to_ascii_uppercase().as_str(),
                "EJECT" | "SKIP1" | "SKIP2" | "SKIP3"
            ) {
                s = &s[1..];
            } else {
                break;
            }
        }
        if s.is_empty() {
            continue;
        }
        if let Some(e) = entry(s, &mut warnings)? {
            entries.push(e);
        }
    }
    if entries.is_empty() {
        return Err("データ記述項がありません".into());
    }
    // 木にする
    let mut roots: Vec<Entry> = Vec::new();
    let mut stack: Vec<Entry> = Vec::new();
    let close = |stack: &mut Vec<Entry>, roots: &mut Vec<Entry>, level: u32| {
        while stack.last().is_some_and(|t| t.level >= level) {
            let done = stack.pop().expect("stack");
            match stack.last_mut() {
                Some(p) => p.children.push(done),
                None => roots.push(done),
            }
        }
    };
    for e in entries {
        let lv = if e.level == 77 { 1 } else { e.level };
        close(&mut stack, &mut roots, lv);
        stack.push(Entry { level: lv, ..e });
    }
    close(&mut stack, &mut roots, 0);
    // 最初のレコード（01 で始まらなければ全体を 1 つのレコードに）
    let mut root = if roots[0].level == 1 {
        if roots.len() > 1 {
            warnings.push(format!(
                "2 つ目以降のレコード（{}）は使いません（最初の {} を使います）",
                roots[1..]
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>()
                    .join("・"),
                roots[0].name
            ));
        }
        roots.swap_remove(0)
    } else {
        Entry {
            level: 1,
            name: "RECORD".into(),
            filler: true,
            occurs: 1,
            children: roots,
            ..Default::default()
        }
    };
    if root.occurs > 1 {
        warnings.push(format!("{}: レコードの OCCURS は使いません", root.name));
        root.occurs = 1;
    }
    size(&mut root, None, None, &mut warnings)?;
    let redefined = count_redefines(&root);
    if redefined > 0 {
        warnings.push(format!(
            "REDEFINES する項目（{redefined} 個）は使いません（元の定義で読み書きします）"
        ));
    }
    if has_sync(&root) {
        warnings.push("SYNC の項目は、集団の先頭から数えた境界に揃えます".into());
    }
    let mut flat = Flat { fields: Vec::new() };
    let top = if root.filler { "" } else { root.name.as_str() };
    place(&root, 0, &[], top, &mut flat);
    // 同じ名前は「名前 OF 親」、FILLER と残りの重なりは #番号
    let mut count = std::collections::HashMap::new();
    for (f, ..) in &flat.fields {
        *count.entry(f.name.clone()).or_insert(0usize) += 1;
    }
    let mut seen = std::collections::HashMap::new();
    let mut fields = Vec::with_capacity(flat.fields.len());
    for (mut f, base, sub, parent) in flat.fields {
        if count[&f.name] > 1 && base != "FILLER" && !parent.is_empty() {
            f.name = format!("{base} OF {parent}{sub}");
        }
        let n = seen.entry(f.name.clone()).or_insert(0usize);
        *n += 1;
        if *n > 1 {
            f.name = format!("{}#{}", f.name, n);
        }
        fields.push(f);
    }
    if fields.is_empty() {
        return Err("基本項目がありません".into());
    }
    Ok(Layout {
        record: root.name.clone(),
        record_len: root.size,
        fields,
        warnings,
    })
}

fn count_redefines(e: &Entry) -> usize {
    e.children
        .iter()
        .map(|c| c.redefines.is_some() as usize + count_redefines(c))
        .sum()
}

fn has_sync(e: &Entry) -> bool {
    e.sync || e.children.iter().any(has_sync)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COPY: &str = "\
000100*    顧客レコード
000200 01  CUST-REC.
000300     05  CUST-ID           PIC 9(6).
000400     05  CUST-NAME         PIC X(20).
000500     05  CUST-KANA         PIC N(10).
000600     05  BALANCE           PIC S9(7)V99 COMP-3.
000700     05  POINTS            PIC S9(9) USAGE IS BINARY.
000800     05  RATE              COMP-2.
000900     05  HISTORY OCCURS 3 TIMES.
001000         10  H-DATE        PIC 9(8).
001100         10  H-AMT         PIC S9(5) COMP-3.
001200     05  STATUS            PIC X.
001300         88  ACTIVE        VALUE 'A'.
001400     05  AMOUNT-EDIT       PIC ZZ,ZZ9.99-.
001500     05  ALT-ID REDEFINES AMOUNT-EDIT PIC X(10).
001600     05  FILLER            PIC X(5)  VALUE SPACES.
001700     05  SGN               PIC S9(3) SIGN LEADING SEPARATE.
";

    #[test]
    fn fixed_format_copybook() {
        let l = parse(COPY).unwrap();
        assert_eq!(l.record, "CUST-REC");
        let names: Vec<&str> = l.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "CUST-ID",
                "CUST-NAME",
                "CUST-KANA",
                "BALANCE",
                "POINTS",
                "RATE",
                "H-DATE(1)",
                "H-AMT(1)",
                "H-DATE(2)",
                "H-AMT(2)",
                "H-DATE(3)",
                "H-AMT(3)",
                "STATUS",
                "AMOUNT-EDIT",
                "FILLER",
                "SGN"
            ]
        );
        let off: Vec<(usize, usize)> = l.fields.iter().map(|f| (f.offset, f.len)).collect();
        assert_eq!(
            off,
            [
                (0, 6),
                (6, 20),
                (26, 20),
                (46, 5),
                (51, 4),
                (55, 8),
                (63, 8),
                (71, 3),
                (74, 8),
                (82, 3),
                (85, 8),
                (93, 3),
                (96, 1),
                (97, 10),
                (107, 5),
                (112, 4)
            ]
        );
        assert_eq!(l.record_len, 116);
        assert_eq!(l.fields[3].describe, "S9(7)V99 COMP-3");
        assert_eq!(l.fields[15].describe, "S9(3) SIGN LEADING SEPARATE");
        assert!(l.warnings.iter().any(|w| w.contains("REDEFINES")));
    }

    #[test]
    fn free_format_inheritance_and_names() {
        let src = "01 REC.\n  05 A USAGE COMP-3.\n    10 X PIC S9(3).\n    10 Y PIC 9(4).\n  05 B.\n    10 X PIC X(2). *> comment\n  05 C SIGN IS LEADING.\n    10 Z PIC S99.\n";
        let l = parse(src).unwrap();
        let names: Vec<&str> = l.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["X OF A", "Y", "X OF B", "Z"]);
        assert_eq!(l.fields[0].len, 2);
        assert_eq!(l.fields[1].len, 3);
        assert!(matches!(
            l.fields[3].kind,
            Kind::Zoned {
                sign: SignPos::Leading,
                ..
            }
        ));
        assert_eq!(l.record_len, 2 + 3 + 2 + 2);
    }

    #[test]
    fn without_01_nested_occurs_sync_and_errors() {
        let l =
            parse("05 T OCCURS 2.\n 10 V OCCURS 2 PIC X.\n05 N PIC S9(4) COMP SYNC.\n").unwrap();
        let names: Vec<&str> = l.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["V(1,1)", "V(1,2)", "V(2,1)", "V(2,2)", "N"]);
        // 4 バイトの後の 2 バイトの 2 進数は境界のまま
        assert_eq!(l.fields[4].offset, 4);
        let l = parse("01 R.\n 05 A PIC X.\n 05 N PIC S9(9) COMP SYNC.\n").unwrap();
        assert_eq!(l.fields[1].offset, 4);
        assert_eq!(l.record_len, 8);
        assert!(parse("01 R.\n 05 A.\n").is_ok() || parse("01 R.\n 05 A.\n").is_err());
        assert!(parse("01 R.\n 05 A PIC Q.\n").is_err());
        assert!(parse("* only comments\n").is_err());
        // 2 つ目の 01 は使わない
        let l = parse("01 R1.\n 05 A PIC X.\n01 R2.\n 05 B PIC X(9).\n").unwrap();
        assert_eq!(l.record_len, 1);
        assert!(l.warnings.iter().any(|w| w.contains("R2")));
        // 継続行の文字列・VALUE の引用符の中のピリオド
        let src = "       01  R.\n           05  A  PIC X(30) VALUE 'ABC. DEF\n      -    'GHI'.\n           05  B  PIC 9.\n";
        let l = parse(src).unwrap();
        assert_eq!(l.fields.len(), 2);
    }
}
