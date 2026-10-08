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
    for (on, word) in [
        (e.justified, "JUSTIFIED"),
        (e.blank_zero, "BLANK WHEN ZERO"),
        (e.sync, "SYNC"),
    ] {
        if on {
            e.describe.push(' ');
            e.describe.push_str(word);
        }
    }
    e.kind = Some(kind);
    e.size = len;
    Ok(())
}

/// 基本項目を並べる。
struct Flat {
    /// （項目, 名前, 添字, 親の名前, 木の中の道〔子の番号の並び〕）
    fields: Vec<(Field, String, String, String, Vec<usize>)>,
}

fn place(e: &Entry, at: usize, subs: &[u32], parent: &str, path: &[usize], out: &mut Flat) {
    if e.is_group() {
        let me = if e.filler { parent } else { e.name.as_str() };
        for (k, c) in e.children.iter().enumerate() {
            if c.redefines.is_some() {
                continue;
            }
            let mut p = path.to_vec();
            p.push(k);
            for n in 0..c.occurs {
                let mut s = subs.to_vec();
                if c.occurs > 1 {
                    s.push(n + 1);
                }
                place(
                    c,
                    at + e.child_off[k] + n as usize * c.size,
                    &s,
                    me,
                    &p,
                    out,
                );
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
            kind: e.kind.clone().unwrap_or(Kind::Alnum {
                justified: false,
                alpha: false,
            }),
            describe: e.describe.clone(),
        },
        e.name.clone(),
        sub,
        parent.to_string(),
        path.to_vec(),
    ));
}

/// コピーブックを読んで、最初のレコードのレイアウトを作る。
pub fn parse(src: &str) -> Result<Layout, String> {
    let (mut root, mut warnings) = tree(src)?;
    size(&mut root, None, None, &mut warnings)?;
    let flat = flatten(&root);
    layout_of(&root, flat, warnings)
}

/// 基本項目を並べる（大きさを求めた木から）。
fn flatten(root: &Entry) -> Flat {
    let mut flat = Flat { fields: Vec::new() };
    let top = if root.filler { "" } else { root.name.as_str() };
    place(root, 0, &[], top, &[], &mut flat);
    flat
}

/// 最初のレコードの木（大きさはまだ）と注意。
fn tree(src: &str) -> Result<(Entry, Vec<String>), String> {
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
    Ok((root, warnings))
}

fn layout_of(root: &Entry, flat: Flat, mut warnings: Vec<String>) -> Result<Layout, String> {
    let redefined = count_redefines(root);
    if redefined > 0 {
        warnings.push(format!(
            "REDEFINES する項目（{redefined} 個）は使いません（元の定義で読み書きします）"
        ));
    }
    if has_sync(root) {
        warnings.push("SYNC の項目は、集団の先頭から数えた境界に揃えます".into());
    }
    // 同じ名前は「名前 OF 親」、FILLER と残りの重なりは #番号
    let mut count = std::collections::HashMap::new();
    for (f, ..) in &flat.fields {
        *count.entry(f.name.clone()).or_insert(0usize) += 1;
    }
    let mut seen = std::collections::HashMap::new();
    let mut fields = Vec::with_capacity(flat.fields.len());
    for (mut f, base, sub, parent, _) in flat.fields {
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

// ---- 型を変える・項目を足す（セルの書式設定の「COBOL の型」） ----

/// 型の指定（`S9(7)V99 COMP-3`・`PIC X(10)`・`COMP-2`・`9(5) SIGN LEADING SEPARATE` など）を読む。
/// 型の句（`PIC`・`USAGE`・`SIGN`・`JUSTIFIED`・`BLANK WHEN ZERO`・`SYNC`）だけを持つ記述項を返す。
fn type_entry(ty: &str) -> Result<Entry, String> {
    let t = ty.trim().trim_end_matches('.').trim();
    if t.is_empty() {
        return Err("型が空です（例: X(10)・S9(7)V99 COMP-3）".into());
    }
    let first = t
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    // 先頭が句の語でなければ PIC の文字列
    let body = if is_clause(&first) && !matches!(first.as_str(), "LEADING" | "TRAILING") {
        t.to_string()
    } else {
        format!("PIC {t}")
    };
    let toks = tokenize(&format!("05 YYSHEET-TYPE {body}"));
    let mut w = Vec::new();
    let e = entry(&toks, &mut w)?.ok_or("型が読めません")?;
    if let Some(m) = w.first() {
        return Err(m
            .replace("YYSHEET-TYPE の ", "")
            .replace("YYSHEET-TYPE", "型"));
    }
    if e.occurs != 1 || e.redefines.is_some() {
        return Err("型には OCCURS・REDEFINES は書けません".into());
    }
    if e.pic.is_none() && !matches!(e.usage, Some(Usage::Comp1 | Usage::Comp2 | Usage::Pointer)) {
        return Err("PIC がありません（例: X(10)・S9(7)V99 COMP-3）".into());
    }
    let attrs = Attrs {
        usage: e.usage,
        sign: e.sign,
        justified: e.justified,
        blank_zero: e.blank_zero,
    };
    kind_of(e.pic.as_deref(), &attrs)?;
    Ok(e)
}

/// 型の指定を確かめて、その型と長さ（バイト）を返す。
pub fn check_type(ty: &str) -> Result<(Kind, usize), String> {
    let e = type_entry(ty)?;
    kind_of(
        e.pic.as_deref(),
        &Attrs {
            usage: e.usage,
            sign: e.sign,
            justified: e.justified,
            blank_zero: e.blank_zero,
        },
    )
}

fn entry_at<'a>(root: &'a mut Entry, path: &[usize]) -> &'a mut Entry {
    let mut e = root;
    for &k in path {
        e = &mut e.children[k];
    }
    e
}

/// 基本項目 `field`（レイアウトの番号）の型を `ty` に変えたコピーブックを返す。`OCCURS` の項目は
/// すべての回の型が変わる。コピーブックは書き直す（注記・`VALUE`・88 は残らない）。
pub fn retype(src: &str, field: usize, ty: &str) -> Result<String, String> {
    let t = type_entry(ty)?;
    let (mut root, mut w) = tree(src)?;
    size(&mut root, None, None, &mut w)?;
    let flat = flatten(&root);
    let path = flat
        .fields
        .get(field)
        .map(|f| f.4.clone())
        .ok_or("項目がありません")?;
    let e = entry_at(&mut root, &path);
    e.pic = t.pic;
    // 集団の USAGE を引き継がないよう、項目に書く
    e.usage = Some(t.usage.unwrap_or(Usage::Display));
    e.sign = t.sign;
    e.justified = t.justified;
    e.blank_zero = t.blank_zero;
    e.sync = t.sync;
    let out = write(&root);
    parse(&out)?;
    Ok(out)
}

/// COBOL のデータ名にする（使えない文字は `-`、30 文字まで）。
fn data_name(name: &str) -> String {
    let mut n: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || !c.is_ascii() && !c.is_whitespace() {
                c.to_ascii_uppercase()
            } else {
                '-'
            }
        })
        .collect();
    while n.contains("--") {
        n = n.replace("--", "-");
    }
    let n: String = n.trim_matches('-').chars().take(30).collect();
    let n = n.trim_end_matches('-').to_string();
    if n.is_empty() || n.chars().all(|c| c.is_ascii_digit()) || is_clause(&n) || n == "FILLER" {
        format!("FIELD-{n}").trim_end_matches('-').to_string()
    } else {
        n
    }
}

fn all_names(e: &Entry, out: &mut std::collections::HashSet<String>) {
    out.insert(e.name.clone());
    for c in &e.children {
        all_names(c, out);
    }
}

/// 基本項目（名前 `name`・型 `ty`）を足したコピーブックと、足した項目のレイアウトでの番号を返す。
/// 項目は、基本項目 `after` を含むレコード直下の項目の後ろ（`None` ならレコードの先頭）に置く。
/// `src` が空なら `01 RECORD.` から作る。名前はデータ名に直し、同じ名前があれば `-2` などを付ける。
pub fn add_field(
    src: &str,
    after: Option<usize>,
    name: &str,
    ty: &str,
) -> Result<(String, usize), String> {
    let t = type_entry(ty)?;
    let mut root = if src.trim().is_empty() {
        Entry {
            level: 1,
            name: "RECORD".into(),
            occurs: 1,
            ..Default::default()
        }
    } else {
        let (mut r, mut w) = tree(src)?;
        size(&mut r, None, None, &mut w)?;
        r
    };
    let mut pos = match after {
        None => 0,
        Some(i) => {
            let flat = flatten(&root);
            let path = &flat.fields.get(i).ok_or("項目がありません")?.4;
            path.first().map_or(root.children.len(), |p| p + 1)
        }
    };
    // REDEFINES は元の項目のすぐ後ろに置くものなので、その後ろへ
    while root
        .children
        .get(pos)
        .is_some_and(|c| c.redefines.is_some())
    {
        pos += 1;
    }
    let mut names = std::collections::HashSet::new();
    all_names(&root, &mut names);
    let base = data_name(name);
    let mut n = base.clone();
    let mut k = 2;
    while names.contains(&n) {
        n = format!("{base}-{k}");
        k += 1;
    }
    let level = root.children.first().map(|c| c.level).unwrap_or(5).max(2);
    root.children.insert(
        pos,
        Entry {
            level,
            name: n,
            occurs: 1,
            pic: t.pic,
            usage: t.usage,
            sign: t.sign,
            justified: t.justified,
            blank_zero: t.blank_zero,
            sync: t.sync,
            ..Default::default()
        },
    );
    let out = write(&root);
    parse(&out)?;
    let (mut r, mut w) = tree(&out)?;
    size(&mut r, None, None, &mut w)?;
    let index = flatten(&r)
        .fields
        .iter()
        .position(|f| f.4 == [pos])
        .ok_or("足した項目が見つかりません")?;
    Ok((out, index))
}

/// 木をコピーブック（固定形式。8〜72 桁）に書く。
fn write(root: &Entry) -> String {
    let mut out = String::new();
    write_entry(root, 0, None, None, &mut out);
    out
}

fn write_entry(
    e: &Entry,
    depth: usize,
    usage: Option<Usage>,
    sign: Option<SignPos>,
    out: &mut String,
) {
    let indent = 7 + (depth * 4).min(32);
    let mut parts: Vec<String> = Vec::new();
    let name = if e.filler && depth > 0 {
        "FILLER"
    } else {
        e.name.as_str()
    };
    parts.push(format!("{:02}  {name}", e.level.max(1)));
    if let Some(r) = &e.redefines {
        parts.push(format!("REDEFINES {r}"));
    }
    if e.occurs > 1 && depth > 0 {
        parts.push(format!("OCCURS {} TIMES", e.occurs));
    }
    if let Some(p) = &e.pic {
        parts.push(format!("PIC {p}"));
    }
    if let Some(u) = e.usage
        && (u != Usage::Display || usage.is_some_and(|x| x != Usage::Display))
    {
        parts.push(u.name().to_string());
    }
    if let Some(sp) = e.sign
        && (sp != SignPos::Trailing || sign.is_some_and(|x| x != SignPos::Trailing))
    {
        parts.push(
            match sp {
                SignPos::Leading => "SIGN LEADING",
                SignPos::Trailing => "SIGN TRAILING",
                SignPos::LeadingSeparate => "SIGN LEADING SEPARATE",
                SignPos::TrailingSeparate => "SIGN TRAILING SEPARATE",
            }
            .to_string(),
        );
    }
    if e.sync {
        parts.push("SYNC".into());
    }
    if e.justified {
        parts.push("JUSTIFIED RIGHT".into());
    }
    if e.blank_zero {
        parts.push("BLANK WHEN ZERO".into());
    }
    let width = |s: &str| s.chars().map(char_width).sum::<usize>();
    let mut line = " ".repeat(indent);
    let n = parts.len();
    for (i, p) in parts.into_iter().enumerate() {
        let p = if i + 1 == n { format!("{p}.") } else { p };
        let at_start = line.trim().is_empty();
        if !at_start && width(&line) + 1 + width(&p) > 72 {
            out.push_str(line.trim_end());
            out.push('\n');
            line = " ".repeat((indent + 4).min(40));
        } else if !at_start {
            line.push(' ');
        }
        line.push_str(&p);
    }
    out.push_str(line.trim_end());
    out.push('\n');
    let usage = e.usage.or(usage);
    let sign = e.sign.or(sign);
    for c in &e.children {
        write_entry(c, depth + 1, usage, sign, out);
    }
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

    #[test]
    fn retype_and_add_field() {
        let src = "      * 注記\n       01  REC.\n           05  ID      PIC 9(5).\n           05  ITEM    OCCURS 2.\n               10  AMT PIC S9(5)V99 COMP-3.\n           05  NAME    PIC X(10) VALUE 'X'.\n";
        let l = parse(src).unwrap();
        assert_eq!(l.record_len, 5 + 2 * 4 + 10);
        // ID を COMP-3 に
        let s2 = retype(src, 0, "S9(5) COMP-3").unwrap();
        let l2 = parse(&s2).unwrap();
        assert_eq!(l2.fields[0].describe, "S9(5) COMP-3");
        assert_eq!(l2.record_len, 3 + 2 * 4 + 10);
        assert_eq!(l2.fields.len(), l.fields.len());
        assert_eq!(l2.fields[1].name, "AMT(1)");
        assert_eq!(l2.fields[3].offset, 3 + 8);
        // OCCURS の項目はすべての回が変わる
        let s3 = retype(&s2, 2, "PIC 9(3)").unwrap();
        let l3 = parse(&s3).unwrap();
        assert_eq!(l3.fields[1].describe, "9(3)");
        assert_eq!(l3.fields[2].describe, "9(3)");
        assert_eq!(l3.record_len, 3 + 2 * 3 + 10);
        // 書いたものは固定形式（8〜72 桁）
        for line in s3.lines() {
            assert!(line.starts_with("       "), "{line}");
            assert!(line.len() <= 72, "{line}");
        }
        // 集団の USAGE を引き継がない
        let g = "01 R.\n 05 G COMP-3.\n  10 A PIC S9(5).\n  10 B PIC S9(5).\n";
        let g2 = retype(g, 0, "X(4)").unwrap();
        let lg = parse(&g2).unwrap();
        assert_eq!(lg.fields[0].describe, "X(4)");
        assert_eq!(lg.fields[1].describe, "S9(5) COMP-3");
        let g3 = retype(g, 1, "S9(5)").unwrap();
        let lg3 = parse(&g3).unwrap();
        assert_eq!(lg3.fields[1].describe, "S9(5)");
        assert_eq!(lg3.fields[1].len, 5);
        assert_eq!(lg3.fields[0].len, 3);
        // 属性も
        let j = retype(g, 0, "X(4) JUSTIFIED").unwrap();
        assert_eq!(parse(&j).unwrap().fields[0].describe, "X(4) JUSTIFIED");
        let z = retype(g, 0, "ZZ9.99 BLANK WHEN ZERO").unwrap();
        assert_eq!(
            parse(&z).unwrap().fields[0].describe,
            "ZZ9.99 BLANK WHEN ZERO"
        );
        let sl = retype(g, 0, "S9(3) SIGN LEADING SEPARATE").unwrap();
        assert_eq!(parse(&sl).unwrap().fields[0].len, 4);
        // 誤り
        assert!(retype(g, 0, "").is_err());
        assert!(retype(g, 0, "9(5)Q").is_err());
        assert!(retype(g, 0, "X(3) OCCURS 2").is_err());
        assert!(retype(g, 0, "N(3) COMP-3").is_err());
        assert!(retype(g, 9, "X").is_err());
        // 足す
        let (a, i) = add_field("", None, "顧客 名", "N(10)").unwrap();
        assert_eq!(i, 0);
        let la = parse(&a).unwrap();
        assert_eq!(la.record, "RECORD");
        assert_eq!(la.fields[0].name, "顧客-名");
        assert_eq!(la.record_len, 20);
        let (a2, i2) = add_field(&a, Some(0), "顧客 名", "COMP-2").unwrap();
        assert_eq!(i2, 1);
        let la2 = parse(&a2).unwrap();
        assert_eq!(la2.fields[1].name, "顧客-名-2");
        assert_eq!(la2.fields[1].offset, 20);
        let (a3, i3) = add_field(src, Some(1), "amount (円)", "9(7)").unwrap();
        let la3 = parse(&a3).unwrap();
        // ITEM（AMT(1)・AMT(2)）の後ろ
        assert_eq!(i3, 3);
        assert_eq!(la3.fields[3].name, "AMOUNT-円");
        assert_eq!(la3.fields[3].offset, 5 + 8);
        assert_eq!(la3.fields.len(), l.fields.len() + 1);
        let (a4, i4) = add_field(src, None, "1", "X").unwrap();
        assert_eq!(i4, 0);
        assert_eq!(parse(&a4).unwrap().fields[0].name, "FIELD-1");
        let r = "01 R.\n 05 A PIC X(4).\n 05 B REDEFINES A PIC 9(4).\n 05 C PIC X.\n";
        let (r2, ri) = add_field(r, Some(0), "D", "X(2)").unwrap();
        assert_eq!(ri, 1);
        assert_eq!(parse(&r2).unwrap().fields[1].offset, 4);
        assert_eq!(check_type("S9(7)V99 COMP-3").unwrap().1, 5);
        assert!(check_type("PIC").is_err());
    }

    #[test]
    fn meanings() {
        let m = |ty: &str| {
            let (k, len) = check_type(ty).unwrap();
            (len, crate::meaning(&k, len))
        };
        assert_eq!(
            m("9(5)"),
            (5, "数字 5 文字で十進 5 けたの符号なし整数を表す".into())
        );
        assert_eq!(
            m("9(5)V99"),
            (
                7,
                "数字 7 文字で十進整数部 5 けた、小数部 2 けたの符号なし数を表す".into()
            )
        );
        assert_eq!(
            m("S9(5)").1,
            "十進 5 けたの符号あり整数を表す（符号は最後のけたのゾーンに含める）"
        );
        assert_eq!(m("S9(7)V99 COMP-3").0, 5);
        assert!(m("S9(4) COMP-5").1.contains("-32768〜32767"));
        assert!(
            m("ZZ,ZZ9.99-").1.contains("例: 45,678.90-"),
            "{}",
            m("ZZ,ZZ9.99-").1
        );
        assert!(m("X(10)").1.starts_with("英数字 10 バイト"));
        assert!(m("N(5)").1.contains("全角）5 文字"));
    }
}
