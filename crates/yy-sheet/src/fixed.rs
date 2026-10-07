//! 固定長ファイル（COBOL のコピーブックのレイアウト。15 章 6.5）の取り込みと書き出し。
//!
//! レイアウトの基本項目 1 つが 1 列になり、列はどの項目かを覚えている（[`Column::field`]）。列の
//! 並べ替え・挿入・削除をしても、書き出すときは項目の順にレコードを組み立てる（列のない項目は空白・0）。
//! シートは固定長の設定（コピーブック・文字コード・レコードの区切り）を持ち（[`Sheet::fixed`]）、
//! `.yys` に保存する。
//!
//! - 取り込み: レコードは決まった長さなので、レコードの塊ごとに全コアで並列に読み、列のチャンクに詰める。
//!   数値の項目は、15 桁までなら数値、それより長ければ桁を落とさないよう文字列にする。数値として読め
//!   ない項目は `X'…'` の文字列にし、書き出すときに元のバイトに戻す。
//! - 書き出し: 文字コードを変えて書ける（MS932 ⇔ EBCDIC。ゾーン 10 進数の符号の形・浮動小数点の形も
//!   文字コードに合わせる）。桁あふれ・長すぎる文字列・文字コードにない文字は数えて報告する。

use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;
use yy_cobol::{Codec, Decoded, Input, Issues, Layout};

use crate::Context;
use crate::budget::Part;
use crate::chunk::{Builder, CellRef, Chunk, MAX_ROWS};
use crate::column::{Column, Piece};
use crate::sheet::{Sheet, Table};

/// レコードの区切り。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordSep {
    /// 区切りなし（レコードが続けて並ぶ）
    None,
    /// CR LF（Windows）
    Crlf,
    /// LF
    Lf,
    /// NL（EBCDIC の 0x15）
    Nl,
}

impl RecordSep {
    pub const ALL: [RecordSep; 4] = [
        RecordSep::None,
        RecordSep::Crlf,
        RecordSep::Lf,
        RecordSep::Nl,
    ];

    pub fn bytes(self) -> &'static [u8] {
        match self {
            RecordSep::None => b"",
            RecordSep::Crlf => b"\r\n",
            RecordSep::Lf => b"\n",
            RecordSep::Nl => b"\x15",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RecordSep::None => "なし（固定長が続く）",
            RecordSep::Crlf => "CR LF",
            RecordSep::Lf => "LF",
            RecordSep::Nl => "NL（EBCDIC の 0x15）",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RecordSep::None => "none",
            RecordSep::Crlf => "crlf",
            RecordSep::Lf => "lf",
            RecordSep::Nl => "nl",
        }
    }

    pub fn from_name(s: &str) -> Option<RecordSep> {
        RecordSep::ALL.into_iter().find(|r| r.name() == s)
    }
}

/// 固定長ファイルの設定（シートが持つ）。
#[derive(Clone, Debug, PartialEq)]
pub struct FixedSpec {
    /// コピーブックの文字列（`.yys` に保存する）
    pub copybook: Arc<str>,
    pub layout: Arc<Layout>,
    pub codec: Codec,
    pub separator: RecordSep,
}

impl FixedSpec {
    /// コピーブックから。
    pub fn new(copybook: &str, codec: Codec, separator: RecordSep) -> Result<FixedSpec, String> {
        let layout = yy_cobol::parse(copybook)?;
        if layout.record_len == 0 {
            return Err("レコード長が 0 です".into());
        }
        Ok(FixedSpec {
            copybook: Arc::from(copybook),
            layout: Arc::new(layout),
            codec,
            separator,
        })
    }

    fn stride(&self) -> usize {
        self.layout.record_len + self.separator.bytes().len()
    }
}

/// レコードの数と、余りのバイト数（最後のレコードの後の区切りはなくてもよい）。
pub fn record_count(len: u64, reclen: usize, sep: RecordSep) -> (u64, u64) {
    let stride = (reclen + sep.bytes().len()) as u64;
    let mut n = len / stride;
    let mut rest = len % stride;
    if rest >= reclen as u64 && !sep.bytes().is_empty() {
        n += 1;
        rest -= reclen as u64;
    }
    (n, rest)
}

/// レコードの区切りを推定する（先頭のレコードの後ろを見る）。
pub fn detect_separator(bytes: &[u8], reclen: usize) -> RecordSep {
    let fits = |sep: RecordSep| {
        let s = sep.bytes();
        let stride = reclen + s.len();
        let n = (bytes.len() / stride.max(1)).clamp(1, 20);
        (0..n).all(|k| {
            let at = k * stride + reclen;
            at + s.len() <= bytes.len() && &bytes[at..at + s.len()] == s
        })
    };
    for sep in [RecordSep::Crlf, RecordSep::Lf, RecordSep::Nl] {
        if bytes.len() > reclen && fits(sep) {
            return sep;
        }
    }
    RecordSep::None
}

/// 取り込み・書き出しの報告。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FixedReport {
    pub records: u64,
    /// レコードにならなかった末尾のバイト数
    pub remainder: u64,
    /// 数値として読めず `X'…'` にした項目の数（取り込み）
    pub invalid: u64,
    /// 書き出しの注意
    pub issues: Issues,
    /// 最初の注意の場所（表の行・項目の名前）
    pub first: Option<(u64, String)>,
}

/// 読んだ値をセルに。
fn push_decoded(b: &mut Builder, f: &yy_cobol::Field, d: Decoded, raw: &[u8], invalid: &mut u64) {
    match d {
        Decoded::Empty => b.push(CellRef::Empty),
        Decoded::Text(s) => b.push(CellRef::Text(&s)),
        Decoded::Float(x) => b.push(CellRef::Number(x)),
        Decoded::Num(n) => {
            let digits = f.kind.digits_scale().map(|d| d.0).unwrap_or(0);
            if digits <= 15 && n.scale <= 15 {
                b.push(CellRef::Number(n.to_f64()))
            } else {
                b.push(CellRef::Text(&n.to_string()))
            }
        }
        Decoded::Invalid => {
            *invalid += 1;
            b.push(CellRef::Text(&yy_cobol::hex_text(raw)))
        }
    }
}

/// 項目の列の表示形式（小数部のある数値）。
fn format_of(f: &yy_cobol::Field) -> Option<Arc<str>> {
    match f.kind.digits_scale() {
        Some((d, s)) if d <= 15 && s > 0 => {
            Some(Arc::from(format!("0.{}", "0".repeat(s as usize))))
        }
        _ => None,
    }
}

/// 先頭の `n` レコードを文字列にする（取り込みのダイアログの見本）。
pub fn preview(bytes: &[u8], spec: &FixedSpec, n: usize) -> Vec<Vec<String>> {
    let reclen = spec.layout.record_len;
    let stride = spec.stride();
    let mut rows = Vec::new();
    for k in 0..n {
        let at = k * stride;
        if at + reclen > bytes.len() {
            break;
        }
        let rec = &bytes[at..at + reclen];
        rows.push(
            spec.layout
                .fields
                .iter()
                .map(|f| {
                    let raw = &rec[f.offset..f.offset + f.len];
                    match spec.codec.decode(f, raw) {
                        Decoded::Empty => String::new(),
                        Decoded::Text(s) => s,
                        Decoded::Float(x) => format!("{x}"),
                        Decoded::Num(d) => d.to_string(),
                        Decoded::Invalid => yy_cobol::hex_text(raw),
                    }
                })
                .collect(),
        );
    }
    rows
}

/// レイアウトの説明（ダイアログに出す）: レコード長・注意・項目の一覧。`sample` があれば（ファイルの
/// 先頭のバイト列・ファイルの大きさ）レコードの数と 1 件目の値も。行は CR LF で区切る。
pub fn describe(spec: &FixedSpec, sample: Option<(&[u8], u64)>) -> String {
    let l = &spec.layout;
    let mut out = format!(
        "レコード {}: {} バイト・項目 {} 個（文字コード {}・区切り {}）\r\n",
        if l.record.is_empty() { "-" } else { &l.record },
        l.record_len,
        l.fields.len(),
        spec.codec.charset.name(),
        spec.separator.label()
    );
    let first = sample.and_then(|(bytes, len)| {
        let (n, rest) = record_count(len, l.record_len, spec.separator);
        out.push_str(&format!("ファイル: {n} レコード"));
        if rest > 0 {
            out.push_str(&format!(
                "（末尾の {rest} バイトはレコードになりません。レコード長・区切りを確かめてください）"
            ));
        }
        out.push_str("\r\n");
        preview(bytes, spec, 1).into_iter().next()
    });
    for w in &l.warnings {
        out.push_str(&format!("注意: {w}\r\n"));
    }
    out.push_str("\r\n位置\t長さ\t名前\t型");
    if first.is_some() {
        out.push_str("\t1 件目");
    }
    out.push_str("\r\n");
    for (i, f) in l.fields.iter().enumerate() {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}",
            f.offset + 1,
            f.len,
            f.name,
            f.describe
        ));
        if let Some(v) = first.as_ref().and_then(|r| r.get(i)) {
            out.push_str(&format!("\t{v}"));
        }
        out.push_str("\r\n");
    }
    out
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "中止しました")
}

/// 固定長ファイルを取り込んで新しいシートを作る。`progress(読んだバイト数, 全体)` が `false` を返したら
/// 中止する。
pub fn import(
    ctx: &Context,
    path: &Path,
    spec: &FixedSpec,
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<(Sheet, FixedReport)> {
    let file = yy_io::open_file(path)?;
    let raw = file.bytes();
    let reclen = spec.layout.record_len;
    let stride = spec.stride();
    let (records, remainder) = record_count(raw.len() as u64, reclen, spec.separator);
    let fields = &spec.layout.fields;
    // 1 つの塊を 1 つのチャンクに（64 MB ほど）
    let rows_per_chunk = ((64usize << 20) / reclen.max(1))
        .clamp(4096, MAX_ROWS)
        .next_power_of_two()
        .min(MAX_ROWS) as u64;
    let blocks = records.div_ceil(rows_per_chunk);
    let total = raw.len() as u64;
    let per = rows_per_chunk * reclen as u64 * 4;
    let par = (ctx.budget.of(Part::Work) / per.max(1)).clamp(1, rayon::current_num_threads() as u64)
        as usize;
    let done = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    let invalid = AtomicU64::new(0);
    let mut columns: Vec<Vec<Piece>> = vec![Vec::new(); fields.len()];
    let idx: Vec<u64> = (0..blocks).collect();
    for batch in idx.chunks(par) {
        let res: Vec<io::Result<Vec<Piece>>> = batch
            .par_iter()
            .map(|&k| {
                if stop.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                let _lease = ctx.budget.lease(Part::Work, per);
                let lo = k * rows_per_chunk;
                let hi = (lo + rows_per_chunk).min(records);
                let mut builders: Vec<Builder> =
                    (0..fields.len()).map(|_| Builder::default()).collect();
                let mut bad = 0u64;
                for r in lo..hi {
                    let at = r as usize * stride;
                    let rec = &raw[at..at + reclen];
                    for (f, b) in fields.iter().zip(builders.iter_mut()) {
                        let bytes = &rec[f.offset..f.offset + f.len];
                        push_decoded(b, f, spec.codec.decode(f, bytes), bytes, &mut bad);
                    }
                }
                invalid.fetch_add(bad, Ordering::Relaxed);
                let mut pieces = Vec::with_capacity(fields.len());
                for b in builders {
                    let ch = Chunk::create(ctx, b.finish())?;
                    pieces.push(Piece {
                        len: ch.rows,
                        chunk: ch,
                        start: 0,
                    });
                }
                let n = done.fetch_add((hi - lo) * stride as u64, Ordering::Relaxed)
                    + (hi - lo) * stride as u64;
                if !progress(n.min(total), total) {
                    stop.store(true, Ordering::Relaxed);
                }
                Ok(pieces)
            })
            .collect();
        for r in res {
            for (c, p) in r?.into_iter().enumerate() {
                columns[c].push(p);
            }
        }
        if stop.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
    }
    let mut cols = Vec::with_capacity(fields.len());
    for (i, (f, pieces)) in fields.iter().zip(columns).enumerate() {
        let mut col = Column::from_pieces(&f.name, pieces);
        if col.rows() < records {
            col.extend_empty(ctx, records - col.rows())?;
        }
        col.format = format_of(f);
        col.field = Some(i as u32);
        cols.push(col);
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sheet1".into());
    let mut sheet = Sheet::new(&name);
    sheet.table = Table {
        columns: Arc::new(cols),
        rows: records,
        header: true,
    };
    sheet.fixed = Some(Arc::new(spec.clone()));
    progress(total, total);
    Ok((
        sheet,
        FixedReport {
            records,
            remainder,
            invalid: invalid.into_inner(),
            ..Default::default()
        },
    ))
}

/// 空のシートにレイアウトを当てる（固定長ファイルの作成）。表の列があれば、名前の同じ列を項目に
/// 結び付け、ない項目の列を後ろに足す。足した列の数を返す。
pub fn apply_layout(ctx: &Context, sheet: &mut Sheet, spec: &FixedSpec) -> io::Result<usize> {
    let rows = sheet.table.rows;
    let mut cols: Vec<Column> = sheet.table.columns.as_ref().clone();
    for c in cols.iter_mut() {
        c.field = None;
    }
    let mut added = 0;
    for (i, f) in spec.layout.fields.iter().enumerate() {
        match cols
            .iter_mut()
            .find(|c| c.field.is_none() && c.name.eq_ignore_ascii_case(&f.name))
        {
            Some(c) => c.field = Some(i as u32),
            None => {
                let mut c = Column::new(&f.name);
                c.extend_empty(ctx, rows)?;
                c.format = format_of(f);
                c.field = Some(i as u32);
                cols.push(c);
                added += 1;
            }
        }
    }
    sheet.table = Table {
        columns: Arc::new(cols),
        rows,
        header: true,
    };
    sheet.fixed = Some(Arc::new(spec.clone()));
    Ok(added)
}

fn input_of(v: CellRef<'_>) -> Input<'_> {
    match v {
        CellRef::Empty => Input::Empty,
        CellRef::Number(x) => Input::Number(x),
        CellRef::Text(s) => Input::Text(s),
        CellRef::Bool(b) => Input::Bool(b),
        CellRef::Error(_) => Input::Error,
    }
}

/// シートの表を固定長ファイルに書き出す（`spec` の文字コード・区切りで）。`order` があれば、その順の
/// 表の行だけを書く（並べ替え・絞り込みの結果）。
pub fn export(
    ctx: &Context,
    sheet: &Sheet,
    path: &Path,
    spec: &FixedSpec,
    order: Option<&[u32]>,
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<FixedReport> {
    let tmp = path.with_extension("yysheet-export.tmp");
    let r = export_to(ctx, sheet, &tmp, spec, order, progress);
    match r {
        Ok(rep) => {
            std::fs::rename(&tmp, path)?;
            Ok(rep)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 書いた塊（バイト列・注意・最初の注意の場所）。
type Block = (Vec<u8>, Issues, Option<(u64, String)>);

/// 列の展開したチャンクの部分（データ・始まり・行数）。
type Segments = Vec<(Arc<crate::chunk::Data>, usize, usize)>;

fn export_to(
    ctx: &Context,
    sheet: &Sheet,
    path: &Path,
    spec: &FixedSpec,
    order: Option<&[u32]>,
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<FixedReport> {
    let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    let t = &sheet.table;
    let fields = &spec.layout.fields;
    let reclen = spec.layout.record_len;
    let sep = spec.separator.bytes();
    let space = spec.codec.charset.space();
    // 項目 → 列
    let col_of: Vec<Option<usize>> = (0..fields.len())
        .map(|i| t.columns.iter().position(|c| c.field == Some(i as u32)))
        .collect();
    // 式・表の外のセル（表の下に入力した行）があれば格子の値を読む
    let grid = !sheet.formulas.cells.is_empty()
        || !sheet.formulas.shared.is_empty()
        || !sheet.cells.is_empty();
    let head = t.header as u64;
    let total = match order {
        Some(o) => o.len() as u64,
        None if grid => sheet.source_extent().0.saturating_sub(head).max(t.rows),
        None => t.rows,
    };
    const BLOCK: u64 = 16_384;
    let blocks: Vec<u64> = (0..total.div_ceil(BLOCK)).collect();
    let par = rayon::current_num_threads().max(1) * 2;
    let mut report = FixedReport {
        records: total,
        ..Default::default()
    };
    for batch in blocks.chunks(par) {
        let parts: Vec<io::Result<Block>> = batch
            .par_iter()
            .map(|&b| {
                let lo = b * BLOCK;
                let hi = (lo + BLOCK).min(total);
                let mut buf = Vec::with_capacity((hi - lo) as usize * (reclen + sep.len()));
                let mut issues = Issues::default();
                let mut first = None;
                // 速い道: 並べ替え・式なし（列のチャンクを順にたどる）
                let segs: Option<Vec<Option<Segments>>> = if order.is_none() && !grid {
                    Some(
                        col_of
                            .iter()
                            .map(|c| c.map(|c| t.columns[c].segments(ctx, lo..hi)).transpose())
                            .collect::<io::Result<_>>()?,
                    )
                } else {
                    None
                };
                let mut cur: Vec<(usize, usize)> = vec![(0, 0); fields.len()];
                for k in 0..(hi - lo) {
                    let row = match order {
                        Some(o) => o[(lo + k) as usize] as u64,
                        None => lo + k,
                    };
                    let start = buf.len();
                    buf.resize(start + reclen, space);
                    for (fi, f) in fields.iter().enumerate() {
                        let owned;
                        let v: CellRef<'_> = match (col_of[fi], &segs) {
                            (None, _) => CellRef::Empty,
                            (Some(c), Some(segs)) => {
                                let s = segs[fi].as_ref().expect("segments");
                                let (si, so) = &mut cur[fi];
                                let (d, st, len) = &s[*si];
                                let col = &t.columns[c];
                                let v = match col.delta().get(&row) {
                                    Some(v) => CellRef::of(v),
                                    None => d.get(st + *so),
                                };
                                *so += 1;
                                if *so == *len {
                                    *si += 1;
                                    *so = 0;
                                }
                                v
                            }
                            (Some(c), None) => {
                                owned = if grid {
                                    sheet.get_source(ctx, row + head, c as u32)?
                                } else {
                                    t.columns[c].get(ctx, row)?
                                };
                                CellRef::of(&owned)
                            }
                        };
                        let before = issues.total();
                        spec.codec.encode(
                            f,
                            input_of(v),
                            &mut buf[start + f.offset..start + f.offset + f.len],
                            &mut issues,
                        );
                        if first.is_none() && issues.total() > before {
                            first = Some((row, f.name.clone()));
                        }
                    }
                    buf.extend_from_slice(sep);
                }
                Ok((buf, issues, first))
            })
            .collect();
        for p in parts {
            let (buf, issues, first) = p?;
            out.write_all(&buf)?;
            report.issues.add(&issues);
            if report.first.is_none() {
                report.first = first;
            }
        }
        let done = ((batch.last().copied().unwrap_or(0) + 1) * BLOCK).min(total);
        if !progress(done, total) {
            return Err(cancelled());
        }
    }
    out.flush()?;
    out.get_ref().sync_data()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yy_cobol::{Charset, Codec};
    use yy_encoding::Ccsid;

    const COPY: &str = "
       01  REC.
           05  ID          PIC 9(5).
           05  NAME        PIC X(10).
           05  KANA        PIC N(4).
           05  AMT         PIC S9(7)V99 COMP-3.
           05  CNT         PIC S9(4) COMP.
           05  BIG         PIC S9(17)V99 COMP-3.
           05  FILLER      PIC X(3).
";

    fn spec(cs: Charset, sep: RecordSep) -> FixedSpec {
        FixedSpec::new(COPY, Codec::new(cs), sep).unwrap()
    }

    fn sheet_with_rows(ctx: &Context, s: &FixedSpec) -> Sheet {
        let mut sh = Sheet::new("S");
        apply_layout(ctx, &mut sh, s).unwrap();
        let rows: [[&str; 6]; 3] = [
            ["1", "ABC", "カナ", "1234.5", "-7", "12345678901234567.89"],
            ["2", "漢字テ", "", "-0.01", "300", "-1.00"],
            ["99999", "", "ＡＢ", "0", "0", "0.00"],
        ];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let val = if v.is_empty() {
                    crate::Value::Empty
                } else if c == 5 {
                    crate::Value::text(v)
                } else {
                    match v.parse::<f64>() {
                        Ok(x) => crate::Value::Number(x),
                        Err(_) => crate::Value::text(v),
                    }
                };
                sh.set(ctx, r as u64 + 1, c as u32, val).unwrap();
            }
        }
        sh
    }

    fn text_at(sh: &Sheet, ctx: &Context, r: u64, c: u32) -> String {
        sh.get(ctx, r, c).unwrap().general_text()
    }

    #[test]
    fn layout_create_export_import_roundtrip() {
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        for (cs, sep) in [
            (Charset::Ms932, RecordSep::Crlf),
            (Charset::Ebcdic(Ccsid::Ibm930), RecordSep::None),
            (Charset::Ebcdic(Ccsid::Ibm1399), RecordSep::Nl),
        ] {
            let s = spec(cs, sep);
            assert_eq!(s.layout.record_len, 5 + 10 + 8 + 5 + 2 + 10 + 3);
            let sh = sheet_with_rows(&ctx, &s);
            assert_eq!(sh.table.cols(), 7);
            let path = dir.path().join(format!("out-{}.dat", cs.name()));
            let rep = export(&ctx, &sh, &path, &s, None, &|_, _| true).unwrap();
            assert_eq!(rep.records, 3);
            assert_eq!(rep.issues, Issues::default(), "{cs:?}");
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(bytes.len(), 3 * (s.layout.record_len + sep.bytes().len()));
            assert_eq!(detect_separator(&bytes, s.layout.record_len), sep);
            let (back, rep) = import(&ctx, &path, &s, &|_, _| true).unwrap();
            assert_eq!((rep.records, rep.remainder, rep.invalid), (3, 0, 0));
            for r in 1..=3 {
                for c in 0..7 {
                    assert_eq!(
                        text_at(&back, &ctx, r, c),
                        text_at(&sh, &ctx, r, c),
                        "{cs:?} {r} {c}"
                    );
                }
            }
            // 17 桁の整数部は文字列で、桁を落とさない
            assert_eq!(text_at(&back, &ctx, 1, 5), "12345678901234567.89");
            assert_eq!(back.table.columns[3].format.as_deref(), Some("0.00"));
            // 書き出すとバイト単位で同じ
            let path2 = dir.path().join("again.dat");
            export(&ctx, &back, &path2, &s, None, &|_, _| true).unwrap();
            assert_eq!(std::fs::read(&path2).unwrap(), bytes);
        }
    }

    #[test]
    fn charset_change_invalid_bytes_and_reordered_columns() {
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let ebc = spec(Charset::Ebcdic(Ccsid::Ibm930), RecordSep::None);
        let sh = sheet_with_rows(&ctx, &ebc);
        let p1 = dir.path().join("e.dat");
        export(&ctx, &sh, &p1, &ebc, None, &|_, _| true).unwrap();
        // 壊れたパック 10 進数
        let mut bytes = std::fs::read(&p1).unwrap();
        let amt = &ebc.layout.fields[3];
        bytes[amt.offset..amt.offset + amt.len].copy_from_slice(&[0x40; 5]);
        std::fs::write(&p1, &bytes).unwrap();
        let (back, rep) = import(&ctx, &p1, &ebc, &|_, _| true).unwrap();
        assert_eq!(rep.invalid, 1);
        assert_eq!(text_at(&back, &ctx, 1, 3), "X'4040404040'");
        // MS932 に変えて書く
        let ms = FixedSpec {
            codec: Codec::new(Charset::Ms932),
            separator: RecordSep::Crlf,
            ..ebc.clone()
        };
        let p2 = dir.path().join("m.dat");
        export(&ctx, &back, &p2, &ms, None, &|_, _| true).unwrap();
        let m = std::fs::read(&p2).unwrap();
        assert!(m.starts_with(b"00001ABC       "));
        let (back2, _) = import(&ctx, &p2, &ms, &|_, _| true).unwrap();
        assert_eq!(text_at(&back2, &ctx, 2, 1), "漢字テ");
        assert_eq!(text_at(&back2, &ctx, 2, 4), "300");
        // 列を並べ替えても項目の順に書く
        let mut moved = back2.clone();
        let mut cols = moved.table.columns.as_ref().clone();
        cols.reverse();
        moved.table.columns = Arc::new(cols);
        let p3 = dir.path().join("r.dat");
        export(&ctx, &moved, &p3, &ms, None, &|_, _| true).unwrap();
        assert_eq!(std::fs::read(&p3).unwrap(), m);
        // 桁あふれ・長すぎる文字列は数える
        let mut bad = back2.clone();
        bad.set(&ctx, 1, 0, crate::Value::Number(123456.0)).unwrap();
        bad.set(&ctx, 2, 1, crate::Value::text("ABCDEFGHIJKL"))
            .unwrap();
        let rep = export(&ctx, &bad, &dir.path().join("b.dat"), &ms, None, &|_, _| {
            true
        })
        .unwrap();
        assert_eq!((rep.issues.overflow, rep.issues.truncated), (1, 1));
        assert_eq!(rep.first, Some((0, "ID".into())));
        // 余りのバイト
        let mut tail = m.clone();
        tail.extend_from_slice(b"XYZ");
        std::fs::write(&p2, &tail).unwrap();
        let (_, rep) = import(&ctx, &p2, &ms, &|_, _| true).unwrap();
        assert_eq!((rep.records, rep.remainder), (3, 3));
        assert_eq!(preview(&m, &ms, 2)[1][1], "漢字テ");
        let d = describe(&ms, Some((&tail, tail.len() as u64)));
        assert!(d.contains("43 バイト・項目 7 個"), "{d}");
        assert!(d.contains("ファイル: 3 レコード（末尾の 3 バイト"), "{d}");
        assert!(d.contains("16\t8\tKANA\tN(4)\tカナ"), "{d}");
    }

    #[test]
    fn saved_in_yys() {
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let s = FixedSpec {
            codec: Codec {
                charset: Charset::Ebcdic(Ccsid::Ibm939),
                little_endian: true,
            },
            ..spec(Charset::Ms932, RecordSep::Lf)
        };
        let sh = sheet_with_rows(&ctx, &s);
        let mut doc = crate::Document::new(ctx.clone());
        doc.book.sheets = vec![sh];
        let p = dir.path().join("f.yys");
        crate::yys::save(&mut doc, &p, &mut |_, _| true).unwrap();
        let back = crate::yys::open(ctx.clone(), &p).unwrap();
        let b = &back.book.sheets[0];
        assert_eq!(b.fixed.as_deref(), Some(&s));
        let fields: Vec<Option<u32>> = b.table.columns.iter().map(|c| c.field).collect();
        assert_eq!(fields, (0..7).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn many_records_parallel() {
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let s = FixedSpec::new(
            "01 R.\n 05 N PIC 9(6).\n 05 P PIC S9(5)V9 COMP-3.\n",
            Codec::new(Charset::Ebcdic(Ccsid::Ibm037)),
            RecordSep::None,
        )
        .unwrap();
        let n = 200_000u64;
        let mut bytes = Vec::new();
        let mut rec = vec![0u8; s.layout.record_len];
        let mut is = Issues::default();
        for i in 0..n {
            let (a, b) = rec.split_at_mut(6);
            s.codec
                .encode(&s.layout.fields[0], Input::Number(i as f64), a, &mut is);
            s.codec.encode(
                &s.layout.fields[1],
                Input::Number(-(i as f64) / 10.0),
                b,
                &mut is,
            );
            bytes.extend_from_slice(&rec);
        }
        let p = dir.path().join("n.dat");
        std::fs::write(&p, &bytes).unwrap();
        let (sh, rep) = import(&ctx, &p, &s, &|_, _| true).unwrap();
        assert_eq!(rep.records, n);
        assert_eq!(
            sh.get(&ctx, n, 0).unwrap(),
            crate::Value::Number((n - 1) as f64)
        );
        assert_eq!(sh.get(&ctx, 1001, 1).unwrap(), crate::Value::Number(-100.0));
        let p2 = dir.path().join("n2.dat");
        export(&ctx, &sh, &p2, &s, None, &|_, _| true).unwrap();
        assert_eq!(std::fs::read(&p2).unwrap(), bytes);
    }
}
