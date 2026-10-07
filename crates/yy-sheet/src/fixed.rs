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
use crate::sheet::{Place, Sheet, Table};
use crate::value::Value;

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

/// 10 進数をセルの値に（15 桁までなら数値、それより長ければ桁を落とさないよう文字列）。
fn num_cell(f: &yy_cobol::Field, n: &yy_cobol::Decimal) -> Option<f64> {
    let digits = f.kind.digits_scale().map(|d| d.0).unwrap_or(0);
    (digits <= 15 && n.scale <= 15).then(|| n.to_f64())
}

/// 読んだ値をセルに。
fn push_decoded(b: &mut Builder, f: &yy_cobol::Field, d: Decoded, raw: &[u8], invalid: &mut u64) {
    match d {
        Decoded::Empty => b.push(CellRef::Empty),
        Decoded::Text(s) => b.push(CellRef::Text(&s)),
        Decoded::Float(x) => b.push(CellRef::Number(x)),
        Decoded::Num(n) => match num_cell(f, &n) {
            Some(x) => b.push(CellRef::Number(x)),
            None => b.push(CellRef::Text(&n.to_string())),
        },
        Decoded::Invalid => {
            *invalid += 1;
            b.push(CellRef::Text(&yy_cobol::hex_text(raw)))
        }
    }
}

/// 表の列の項目（固定長の設定があって、列が項目に結び付いていれば）。
pub fn column_field(sheet: &Sheet, col: u32) -> Option<(&FixedSpec, &yy_cobol::Field)> {
    let spec = sheet.fixed.as_deref()?;
    let i = sheet.table.columns.get(col as usize)?.field?;
    Some((spec, spec.layout.fields.get(i as usize)?))
}

/// 格子のセルが項目の列のデータ（見出し行・表の外は除く）なら、その項目。
pub fn field_at(sheet: &Sheet, row: u64, col: u32) -> Option<(&FixedSpec, &yy_cobol::Field)> {
    match sheet.place(row, col) {
        Place::Data(_, c) => column_field(sheet, c),
        _ => None,
    }
}

/// 項目のセルに入力した文字列を確かめて、セルの値にする（型に合わなければ理由）。先頭の `'` は
/// 文字列の印として除く。英数字の項目は数字だけでも文字列のまま（`00123` の 0 を残す）。
pub fn entry_value(spec: &FixedSpec, f: &yy_cobol::Field, text: &str) -> Result<Value, String> {
    let t = text.strip_prefix('\'').unwrap_or(text);
    Ok(match spec.codec.accept(f, t)? {
        Decoded::Empty => Value::Empty,
        Decoded::Text(s) => Value::text(&s),
        Decoded::Float(x) => Value::Number(x),
        Decoded::Num(n) => match num_cell(f, &n) {
            Some(x) => Value::Number(x),
            None => Value::text(&n.to_string()),
        },
        Decoded::Invalid => Value::text(t),
    })
}

/// 式の結果を置くセル（元の行 `src_row`・列 `col`）の項目（見出し行・項目のない列は `None`）。
fn formula_field(sheet: &Sheet, src_row: u64, col: u32) -> Option<(&FixedSpec, &yy_cobol::Field)> {
    if sheet.table.header && src_row == 0 {
        return None;
    }
    column_field(sheet, col)
}

/// 式の結果を、セル（元の行 `src_row`・列 `col`）の項目の型に合わせる（再計算で結果を置くたびに
/// 呼ぶので、参照する式・表示・書き出しはどれも合わせた値を見る）。
///
/// - 項目の列: 型に合わなければエラー値（型が違えば `#VALUE!`、桁・範囲の外なら `#NUM!`）。
///   数値の小数部の多い桁は切り捨てた値にする（書き出す値と同じ）。`CBL.LOW-VALUE()`・`CBL.HIGH-VALUE()` は
///   そのまま（書き出すときに項目のすべてのバイトを 0x00・0xFF にする）。
/// - 項目のない列: `CBL.LOW-VALUE()`・`CBL.HIGH-VALUE()` は値にできないので `#VALUE!`。
pub fn fit_formula_result(sheet: &Sheet, src_row: u64, col: u32, v: Value) -> Value {
    let fig = matches!(&v, Value::Text(s) if yy_formula::figurative(s).is_some());
    let Some((spec, f)) = formula_field(sheet, src_row, col) else {
        return if fig {
            Value::Error(crate::CellError::Value)
        } else {
            v
        };
    };
    if fig || matches!(v, Value::Error(_)) {
        return v;
    }
    match spec.codec.fit_result(f, input_of(CellRef::of(&v))) {
        Ok(Decoded::Num(n)) => match num_cell(f, &n) {
            Some(x) => Value::Number(x),
            None => Value::text(&n.to_string()),
        },
        Ok(Decoded::Float(x)) => Value::Number(x),
        Ok(Decoded::Text(_) | Decoded::Empty | Decoded::Invalid) => v,
        Err(yy_cobol::Misfit::Num) => Value::Error(crate::CellError::Num),
        Err(yy_cobol::Misfit::Value) => Value::Error(crate::CellError::Value),
    }
}

/// 式の結果を合わせる要るか（固定長の項目の列か、`CBL.LOW-VALUE()`・`CBL.HIGH-VALUE()` の結果）。速い道に使う。
pub(crate) fn needs_fit(sheet: &Sheet, src_row: u64, col: u32, v: &yy_formula::Val) -> bool {
    matches!(v, yy_formula::Val::Text(s) if yy_formula::figurative(s).is_some())
        || formula_field(sheet, src_row, col).is_some()
}

/// 読んだ値（MOVE の結果）を式の値に。
fn decoded_val(f: &yy_cobol::Field, d: Decoded, raw: &[u8]) -> yy_formula::Val {
    use yy_formula::Val;
    match d {
        Decoded::Empty => Val::Empty,
        Decoded::Text(s) => Val::text(&s),
        Decoded::Float(x) => Val::Num(x),
        Decoded::Num(n) => match num_cell(f, &n) {
            Some(x) => Val::Num(x),
            None => Val::text(&n.to_string()),
        },
        Decoded::Invalid => Val::text(&yy_cobol::hex_text(raw)),
    }
}

fn val_input(v: &yy_formula::Val) -> Input<'_> {
    use yy_formula::Val;
    match v {
        Val::Empty | Val::Array(_) => Input::Empty,
        Val::Num(x) => Input::Number(*x),
        Val::Text(s) => match yy_formula::figurative(s) {
            Some(b) => Input::Fill(b),
            None => Input::Text(s),
        },
        Val::Bool(b) => Input::Bool(*b),
        Val::Err(_) => Input::Error,
    }
}

/// `CBL.MOVE(送り出し範囲, 受け取り範囲)` の値（受け取り範囲の大きさの配列）。
///
/// - 受け取り範囲は COBOL の型のある列の、見出し行でない行だけ（そうでなければ `#VALUE!`）。式は受け取り
///   範囲の左上のセルに置く（`at` が分かればそうでなければ `#REF!`）。
/// - 列の数が同じなら列ごとに基本項目の MOVE、違えば行ごとに集団の MOVE（[`yy_cobol::Codec::move_group`]）。
///   送り出しの列の型（なければ値のまま）と、受け取りの列の型で送る。文字コードは受け取り側。
/// - 送れない組み合わせ（小数部のある数値 → 英数字など）・エラー値は `#VALUE!`。
pub(crate) fn cobol_move(
    book: &crate::Workbook,
    at: Option<(usize, u64, u32)>,
    (ss, sa): (usize, yy_formula::Area),
    (ds, da): (usize, yy_formula::Area),
    get: &dyn Fn(usize, u64, u32) -> yy_formula::Val,
) -> yy_formula::Val {
    use yy_formula::{Array, Error, Val};
    if let Some((si, r, c)) = at
        && (ds != si || da.r0 != r || da.c0 != c)
    {
        return Val::Err(Error::Ref);
    }
    let dst_sheet = &book.sheets[ds];
    let Some(spec) = dst_sheet.fixed.as_deref() else {
        return Val::Err(Error::Value);
    };
    if dst_sheet.table.header && da.r0 == 0 {
        return Val::Err(Error::Value);
    }
    let Some(dst): Option<Vec<&yy_cobol::Field>> = (da.c0..=da.c1)
        .map(|c| column_field(dst_sheet, c).map(|x| x.1))
        .collect()
    else {
        return Val::Err(Error::Value);
    };
    let src_sheet = &book.sheets[ss];
    let src: Vec<Option<&yy_cobol::Field>> = (sa.c0..=sa.c1)
        .map(|c| column_field(src_sheet, c).map(|x| x.1))
        .collect();
    let codec = &spec.codec;
    let rows = da.rows() as usize;
    let mut data = Vec::with_capacity(rows * dst.len());
    for i in 0..rows as u64 {
        let vals: Vec<Val> = (sa.c0..=sa.c1).map(|c| get(ss, sa.r0 + i, c)).collect();
        if src.len() == dst.len() {
            for ((v, sf), df) in vals.iter().zip(&src).zip(&dst) {
                data.push(match codec.move_elementary(val_input(v), *sf, df) {
                    Some((d, raw)) => decoded_val(df, d, &raw),
                    None => match v {
                        Val::Err(e) => Val::Err(*e),
                        _ => Val::Err(Error::Value),
                    },
                });
            }
        } else if let Some(Val::Err(e)) = vals.iter().find(|v| matches!(v, Val::Err(_))) {
            data.extend(std::iter::repeat_n(Val::Err(*e), dst.len()));
        } else {
            let inputs: Vec<(Input<'_>, Option<&yy_cobol::Field>)> = vals
                .iter()
                .map(val_input)
                .zip(src.iter().copied())
                .collect();
            for ((d, raw), df) in codec.move_group(&inputs, &dst).into_iter().zip(&dst) {
                data.push(decoded_val(df, d, &raw));
            }
        }
    }
    Val::Array(Arc::new(Array::new(rows, dst.len(), data)))
}

/// 自動で付けた表示形式か（なし・`0`・`0.00` など）。
fn auto_format(f: Option<&str>) -> bool {
    match f {
        None => true,
        Some(f) => {
            f == "0"
                || f.strip_prefix("0.")
                    .is_some_and(|z| !z.is_empty() && z.chars().all(|c| c == '0'))
        }
    }
}

/// 表の列 `col` の項目の型を `ty`（`S9(7)V99 COMP-3` など）にする。項目のない列なら、左の列の項目の
/// 後ろに項目を足す（名前は列の名前から）。固定長の設定がなければ `codec`・`separator` で作る。
/// コピーブックは書き直す。数値の列の表示形式（自動で付けたもの）は小数部の桁に合わせる。
pub fn set_column_type(
    sheet: &mut Sheet,
    col: u32,
    ty: &str,
    codec: Codec,
    separator: RecordSep,
) -> Result<(), String> {
    let t = &sheet.table;
    let Some(column) = t.columns.get(col as usize) else {
        return Err(if t.columns.is_empty() {
            "COBOL の型は表の列に付けます。先に データ > 固定長のレイアウト でレイアウトを設定するか、\
             表（CSV など）を開いてください"
                .into()
        } else {
            format!(
                "COBOL の型は表の列（A〜{}）に付けます",
                crate::col_name(t.cols() - 1)
            )
        });
    };
    let (src, codec, separator) = match &sheet.fixed {
        Some(s) => (s.copybook.to_string(), s.codec, s.separator),
        None => (String::new(), codec, separator),
    };
    let mut cols: Vec<Column> = t.columns.as_ref().clone();
    let src = match column.field {
        Some(i) => yy_cobol::retype(&src, i as usize, ty)?,
        None => {
            let after = cols[..col as usize].iter().rev().find_map(|c| c.field);
            let (s, idx) = yy_cobol::add_field(&src, after.map(|x| x as usize), &column.name, ty)?;
            for c in cols.iter_mut() {
                if let Some(f) = c.field.as_mut()
                    && *f as usize >= idx
                {
                    *f += 1;
                }
            }
            cols[col as usize].field = Some(idx as u32);
            s
        }
    };
    let spec = FixedSpec::new(&src, codec, separator)?;
    for c in cols.iter_mut() {
        if let Some(f) = c.field.and_then(|i| spec.layout.fields.get(i as usize))
            && auto_format(c.format.as_deref())
        {
            c.format = format_of(f);
        }
    }
    sheet.table.columns = Arc::new(cols);
    sheet.fixed = Some(Arc::new(spec));
    // 式の結果を新しい型に合わせ直す
    sheet.formulas.touch_all();
    Ok(())
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
        // LOW-VALUE()・HIGH-VALUE() の結果は項目のすべてのバイト
        CellRef::Text(s) => match yy_formula::figurative(s) {
            Some(b) => Input::Fill(b),
            None => Input::Text(s),
        },
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

    #[test]
    fn column_types_and_entry_values() {
        let ctx = Context::for_tests();
        // 固定長の設定のない表（CSV など）に型を付けていく
        let mut sh = Sheet::new("S");
        let mut cols = Vec::new();
        for name in ["code", "name", "amount"] {
            let mut c = Column::new(name);
            c.extend_empty(&ctx, 2).unwrap();
            cols.push(c);
        }
        sh.table = Table {
            columns: Arc::new(cols),
            rows: 2,
            header: true,
        };
        let ms = Codec::new(Charset::Ms932);
        assert!(set_column_type(&mut sh, 3, "X", ms, RecordSep::Crlf).is_err());
        set_column_type(&mut sh, 2, "S9(5)V99 COMP-3", ms, RecordSep::Crlf).unwrap();
        set_column_type(&mut sh, 0, "9(4)", ms, RecordSep::Lf).unwrap();
        set_column_type(&mut sh, 1, "X(10)", ms, RecordSep::Lf).unwrap();
        let spec = sh.fixed.clone().unwrap();
        // 列の並びの順に項目ができる（最初の設定の区切り）
        let names: Vec<&str> = spec.layout.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["CODE", "NAME", "AMOUNT"]);
        assert_eq!(spec.separator, RecordSep::Crlf);
        assert_eq!(spec.layout.record_len, 4 + 10 + 4);
        let fields: Vec<Option<u32>> = sh.table.columns.iter().map(|c| c.field).collect();
        assert_eq!(fields, [Some(0), Some(1), Some(2)]);
        assert_eq!(sh.table.columns[2].format.as_deref(), Some("0.00"));
        // 型を変える
        set_column_type(&mut sh, 2, "S9(7) COMP", ms, RecordSep::Lf).unwrap();
        let spec = sh.fixed.clone().unwrap();
        assert_eq!(spec.layout.fields[2].describe, "S9(7) COMP");
        assert_eq!(spec.layout.record_len, 4 + 10 + 4);
        assert_eq!(sh.table.columns[2].format, None);
        assert!(set_column_type(&mut sh, 2, "9(5)Q", ms, RecordSep::Lf).is_err());
        // 入力
        assert!(field_at(&sh, 0, 0).is_none());
        assert!(field_at(&sh, 1, 5).is_none());
        let (sp, f) = field_at(&sh, 1, 0).unwrap();
        assert_eq!(entry_value(sp, f, "0012"), Ok(Value::Number(12.0)));
        assert!(entry_value(sp, f, "12a").is_err());
        assert!(
            entry_value(sp, f, "12345")
                .unwrap_err()
                .contains("整数部は 4 桁")
        );
        let (sp, f) = field_at(&sh, 1, 1).unwrap();
        assert_eq!(entry_value(sp, f, "00123"), Ok(Value::text("00123")));
        assert_eq!(entry_value(sp, f, "'abc"), Ok(Value::text("abc")));
        assert_eq!(entry_value(sp, f, ""), Ok(Value::Empty));
        // 15 桁を超える数値は文字列
        let s = super::tests::spec(Charset::Ms932, RecordSep::Crlf);
        let big = &s.layout.fields[5];
        assert_eq!(
            entry_value(&s, big, "-12345678901234567.8"),
            Ok(Value::text("-12345678901234567.80"))
        );
    }

    #[test]
    fn formula_results_fit_the_field_and_figurative_constants() {
        use crate::Document;
        let ctx = Context::for_tests();
        let s = super::tests::spec(Charset::Ms932, RecordSep::Crlf);
        let mut d = Document::new(ctx.clone());
        d.edit(|b, ctx| {
            let sh = &mut b.sheets[0];
            apply_layout(ctx, sh, &s)?;
            // ID 9(5)・NAME X(10)・KANA N(4)・AMT S9(7)V99 COMP-3・CNT S9(4) COMP・BIG・FILLER
            let f = |sh: &mut Sheet, r: u64, c: u32, t: &str| sh.set_formula(ctx, r, c, t).unwrap();
            f(sh, 1, 0, "=CBL.LOW-VALUE()");
            f(sh, 1, 1, "=CBL.HIGH-VALUE()");
            f(sh, 1, 3, "=10/3");
            f(sh, 1, 4, "=99999");
            f(sh, 2, 0, "=-1");
            f(sh, 2, 1, "=\"ABCDEFGHIJK\"");
            f(sh, 2, 3, "=\"x\"");
            f(sh, 2, 4, "=1234");
            f(sh, 3, 0, "=A2=CBL.LOW-VALUE()");
            // 項目のない列（表の外）
            f(sh, 1, 9, "=CBL.LOW-VALUE()");
            f(sh, 1, 10, "=J2");
            Ok(())
        })
        .unwrap();
        let get = |d: &Document, r: u64, c: u32| d.book.sheets[0].get(&ctx, r, c).unwrap();
        let err = |e| Value::Error(e);
        assert_eq!(get(&d, 1, 0), Value::text(yy_formula::LOW_VALUE));
        assert_eq!(get(&d, 1, 0).general_text(), "LOW-VALUE");
        assert_eq!(get(&d, 1, 1).general_text(), "HIGH-VALUE");
        // 小数部は切り捨て、桁・範囲の外は #NUM!、型が違えば #VALUE!
        assert_eq!(get(&d, 1, 3), Value::Number(3.33));
        assert_eq!(get(&d, 1, 4), err(crate::CellError::Num));
        assert_eq!(get(&d, 2, 0), err(crate::CellError::Num));
        assert_eq!(get(&d, 2, 1), err(crate::CellError::Value));
        assert_eq!(get(&d, 2, 3), err(crate::CellError::Value));
        assert_eq!(get(&d, 2, 4), Value::Number(1234.0));
        // ID の列は 9(5) なので、真偽値は #VALUE!
        assert_eq!(get(&d, 3, 0), err(crate::CellError::Value));
        // 型のない列では LOW-VALUE() は値にならない
        assert_eq!(get(&d, 1, 9), err(crate::CellError::Value));
        assert_eq!(get(&d, 1, 10), err(crate::CellError::Value));
        // 書き出すと、LOW-VALUE は X'00…'、HIGH-VALUE は X'FF…'
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fig.dat");
        export(&ctx, &d.book.sheets[0], &path, &s, None, &|_, _| true).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let rec = &bytes[..s.layout.record_len];
        assert_eq!(&rec[0..5], &[0u8; 5]);
        assert_eq!(&rec[5..15], &[0xFFu8; 10]);
        // 型を変えると合わせ直す（CNT を S9(5) COMP-3 にすると 99999 が入る）
        d.edit(|b, _| {
            set_column_type(&mut b.sheets[0], 4, "S9(5) COMP-3", s.codec, s.separator)
                .map_err(std::io::Error::other)
        })
        .unwrap();
        assert_eq!(get(&d, 1, 4), Value::Number(99999.0));
    }

    #[test]
    fn cobol_move_function() {
        use crate::{CellError, Document};
        let ctx = Context::for_tests();
        let s = super::tests::spec(Charset::Ms932, RecordSep::Crlf);
        let mut d = Document::new(ctx.clone());
        // ID 9(5)・NAME X(10)・KANA N(4)・AMT S9(7)V99 COMP-3・CNT S9(4) COMP・BIG・FILLER
        d.edit(|b, ctx| {
            let sh = &mut b.sheets[0];
            apply_layout(ctx, sh, &s)?;
            for (r, (id, name, amt)) in [(1, "ABC", 1234.5), (2, "XY", -0.01), (3, "", 0.0)]
                .into_iter()
                .enumerate()
            {
                let r = r as u64 + 1;
                sh.set(ctx, r, 0, Value::Number(id as f64))?;
                sh.set(ctx, r, 1, Value::text(name))?;
                sh.set(ctx, r, 3, Value::Number(amt))?;
            }
            let f = |sh: &mut Sheet, r: u64, c: u32, t: &str| sh.set_formula(ctx, r, c, t).unwrap();
            f(sh, 5, 4, "=CBL.MOVE(D2:D4,E6:E8)");
            f(sh, 5, 1, "=CBL.MOVE(A2:A4,B6:B8)");
            f(sh, 9, 1, "=CBL.MOVE(A2:B2,B10)");
            f(sh, 11, 0, "=CBL.MOVE(B2,A12:B12)");
            f(sh, 13, 0, "=CBL.MOVE(A2:A4,A14:A15)");
            f(sh, 13, 9, "=CBL.MOVE(A2,J14)");
            f(sh, 15, 0, "=CBL.MOVE(A2,B16)");
            f(sh, 17, 1, "=CBL.MOVE(D2,B18)");
            f(sh, 19, 3, "=CBL.MOVE(A2:A3,D20:D21)");
            f(sh, 21, 0, "=CBL.MOVE(B2:B3,A22:A23)");
            Ok(())
        })
        .unwrap();
        let get = |r: u64, c: u32| d.book.sheets[0].get(&ctx, r, c).unwrap();
        // 数値 → 数値: 小数部は切り捨て
        assert_eq!(get(5, 4), Value::Number(1234.0));
        assert_eq!(get(6, 4), Value::Number(0.0));
        assert_eq!(get(7, 4), Value::Number(0.0));
        // 9(5) → X(10): 桁数の数字
        assert_eq!(get(5, 1), Value::text("00001"));
        assert_eq!(get(6, 1), Value::text("00002"));
        // 集団の MOVE: 2 列 → 1 列、1 列 → 2 列
        assert_eq!(get(9, 1), Value::text("00001ABC"));
        assert_eq!(get(11, 0), Value::text("X'4142432020'"));
        assert_eq!(get(11, 1), Value::Empty);
        // 行数が違う・型のない列・左上でない・送れない組み合わせ
        assert_eq!(get(13, 0), Value::Error(CellError::Value));
        assert_eq!(get(13, 9), Value::Error(CellError::Value));
        assert_eq!(get(15, 0), Value::Error(CellError::Ref));
        assert_eq!(get(17, 1), Value::Error(CellError::Value));
        // 9(5) → S9(7)V99 COMP-3、X(10) "ABC" → 9(5) は送れない
        assert_eq!(get(19, 3), Value::Number(1.0));
        assert_eq!(get(20, 3), Value::Number(2.0));
        assert_eq!(get(21, 0), Value::Error(CellError::Value));
        // 書き出すと受け取りの型のバイト
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mv.dat");
        export(&ctx, &d.book.sheets[0], &path, &s, None, &|_, _| true).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let stride = s.layout.record_len + 2;
        let rec = |grid_row: usize| {
            &bytes[(grid_row - 1) * stride..(grid_row - 1) * stride + s.layout.record_len]
        };
        assert_eq!(&rec(9)[5..15], b"00001ABC  ");
        assert_eq!(&rec(11)[0..5], b"ABC  ");
    }
}
