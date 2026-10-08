//! CSV（RFC 4180）の取り込みと書き出し（15 章 6）。
//!
//! 取り込みは全コアで並列に行う: ファイルを区画に分け、各区画を「先頭が引用符の外」「中」の 2 通りで
//! 読んで終わりの状態を求め、先頭から順に本当の状態を決めてから、区画ごとに値を列のチャンクに詰める
//! （`yy-delimited` の状態機械を使う。引用符の扱いはエディタの CSV モードと同じく寛容）。
//!
//! 区切り文字は任意（タブ・`|`・制御コードの US など）、レコードの終わりも改行（LF・CR LF）のほか任意の
//! バイト列（CR だけ・RS（0x1E）など。[`RecordEnd`]）にできる。改行以外で終わるレコードでは、値の中の
//! 改行はただの文字。
//!
//! 文字コードは `yy-encoding` で判別・変換する。改行（LF）で区切って読める文字コード（UTF-8・
//! Shift_JIS・EUC-JP など）は区画ごとに並列に UTF-8 にし、それ以外（UTF-16 など）は先に作業用の
//! UTF-8 のファイルに変換する。

use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;
use yy_delimited::{Dialect, LineState, Scanner};
use yy_encoding::{Encoding, EscapeMode};
use yy_numfmt::{DateSystem, Parsed, parse_input};

use crate::Context;
use crate::budget::Part;
use crate::chunk::{Builder, CellRef, Chunk, MAX_ROWS};
use crate::column::{Column, Piece};
use crate::sheet::{Sheet, Table};

// ---- 設定 ----------------------------------------------------------------------------

/// 列の型（取り込みのとき）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColType {
    /// 値ごとに解釈する（見本に値がなかった列）
    Auto,
    /// 文字列のまま
    Text,
    /// 数値（日付・時刻・百分率などを含む）。表示形式（`None` なら標準）
    Number(Option<String>),
    Bool,
}

/// レコードの終わり。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RecordEnd {
    /// 改行（読むときは LF。前の CR も外す。書くときは [`ExportOptions::crlf`] で CR LF か LF）
    #[default]
    Newline,
    /// 指定したバイト列（1〜8 バイトの ASCII。CR だけ・RS（0x1E）・NUL など）
    Custom(Vec<u8>),
}

impl RecordEnd {
    /// 改行以外を指定していれば、そのバイト列。
    pub fn custom(&self) -> Option<&[u8]> {
        match self {
            RecordEnd::Newline => None,
            RecordEnd::Custom(b) => Some(b),
        }
    }

    /// 表示用の名前（`改行`・`<RS>`・`<CR>` など）。
    pub fn label(&self) -> String {
        match self {
            RecordEnd::Newline => "改行".into(),
            RecordEnd::Custom(b) => yy_delimited::describe_bytes(b),
        }
    }
}

/// 区切り文字とレコードの終わりの組み合わせを確かめる。
pub fn check_separators(d: &Dialect, end: &RecordEnd) -> Result<(), String> {
    let delim = d.delimiter();
    match end {
        RecordEnd::Newline => {
            if delim.iter().any(|&b| b == b'\n' || b == b'\r') {
                return Err(
                    "レコードの終わりが改行のときは、区切り文字に CR・LF を使えません。".into(),
                );
            }
        }
        RecordEnd::Custom(t) => {
            if t.is_empty() || t.len() > 8 {
                return Err("レコードの終わりは 1〜8 バイトにしてください。".into());
            }
            if !t.is_ascii() || !delim.is_ascii() {
                return Err(
                    "レコードの終わりを指定するときは、区切り文字もレコードの終わりも ASCII の文字（制御コードを含む）にしてください。"
                        .into(),
                );
            }
            if t.starts_with(delim) || delim.starts_with(t) {
                return Err("区切り文字とレコードの終わりは、一方が他方の始めと同じにならないようにしてください。".into());
            }
            if let Some(q) = d.quote
                && t.contains(&q)
            {
                return Err("レコードの終わりに引用符を含められません。".into());
            }
        }
    }
    Ok(())
}

/// 取り込みの設定。
#[derive(Clone, Debug)]
pub struct CsvOptions {
    pub dialect: Dialect,
    /// レコードの終わり
    pub record_end: RecordEnd,
    pub encoding: Encoding,
    /// 1 行目を見出しにする
    pub header: bool,
    /// 列ごとの型（足りない列は `Auto`）。空なら見本から推定する
    pub types: Vec<ColType>,
    /// すべて文字列として取り込む
    pub all_text: bool,
    pub date_system: DateSystem,
}

/// 取り込みの前の見本（ダイアログに出す）。
#[derive(Clone, Debug)]
pub struct CsvPreview {
    pub options: CsvOptions,
    /// 先頭のレコード（見出しを含む）
    pub rows: Vec<Vec<String>>,
}

fn data_err(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "中止しました")
}

// ---- レコードの解析 ------------------------------------------------------------------

/// 読んだフィールドとレコードの受け手。
trait Sink {
    /// フィールド（レコード内の番号と値）
    fn field(&mut self, i: usize, v: &str);
    /// レコードの終わり（フィールドの数）
    fn end(&mut self, n: usize);
}

/// フィールドを集める受け手（見本・見出し用）。
#[derive(Default)]
struct Collect {
    rows: Vec<Vec<String>>,
    cur: Vec<String>,
}

impl Sink for Collect {
    fn field(&mut self, _: usize, v: &str) {
        self.cur.push(v.to_owned());
    }
    fn end(&mut self, _: usize) {
        self.rows.push(std::mem::take(&mut self.cur));
    }
}

/// UTF-8 のバイト列のレコードを順に読んで `sink` に渡す（UTF-8 として正しくなければ U+FFFD に
/// 置き換えてから）。`limit` レコードで止める。
fn parse_records(
    data: &[u8],
    d: &Dialect,
    end: &RecordEnd,
    limit: usize,
    sink: &mut dyn Sink,
) -> usize {
    match std::str::from_utf8(data) {
        Ok(text) => parse_text(text, d, end, limit, sink),
        Err(_) => parse_text(&String::from_utf8_lossy(data), d, end, limit, sink),
    }
}

/// [`parse_records`] の本体。区切り（区切り文字・引用符・レコードの終わり）は文字の境目にあるので、
/// その位置で切った部分も UTF-8 として正しい。
fn parse_text(
    text: &str,
    d: &Dialect,
    end: &RecordEnd,
    limit: usize,
    sink: &mut dyn Sink,
) -> usize {
    let data = text.as_bytes();
    let delim = d.delimiter();
    let q = d.quote;
    // レコードの終わり（改行なら LF で、前の CR も外す）
    let (term, strip_cr): (&[u8], bool) = match end {
        RecordEnd::Newline => (b"\n", true),
        RecordEnd::Custom(t) => (t, false),
    };
    let n = data.len();
    let mut i = 0;
    let mut scratch = String::new();
    let mut records = 0;
    while i < n && records < limit {
        let mut field = 0;
        loop {
            // 1 つのフィールド
            let (rec_end, next);
            if i < n && q == Some(data[i]) {
                let qc = data[i];
                scratch.clear();
                let mut j = i + 1;
                loop {
                    match memchr::memchr(qc, &data[j..]) {
                        None => {
                            scratch.push_str(&text[j..]);
                            j = n;
                            break;
                        }
                        Some(k) => {
                            scratch.push_str(&text[j..j + k]);
                            let p = j + k;
                            if data.get(p + 1) == Some(&qc) {
                                scratch.push(qc as char);
                                j = p + 2;
                            } else {
                                j = p + 1;
                                break;
                            }
                        }
                    }
                }
                // 閉じ引用符の後の余計な文字（寛容に値へ含める）
                let mut k = j;
                loop {
                    if k >= n {
                        rec_end = true;
                        next = n;
                        break;
                    }
                    if data[k..].starts_with(term) {
                        rec_end = true;
                        next = k + term.len();
                        break;
                    }
                    if data[k..].starts_with(delim) {
                        rec_end = false;
                        next = k + delim.len();
                        break;
                    }
                    k += 1;
                }
                let mut extra_end = k;
                if strip_cr && rec_end && extra_end > j && data[extra_end - 1] == b'\r' {
                    extra_end -= 1;
                }
                scratch.push_str(&text[j..extra_end]);
                sink.field(field, &scratch);
            } else {
                let mut k = i;
                loop {
                    match memchr::memchr2(delim[0], term[0], &data[k..]) {
                        None => {
                            k = n;
                            rec_end = true;
                            next = n;
                            break;
                        }
                        Some(m) => {
                            let p = k + m;
                            if data[p..].starts_with(term) {
                                k = p;
                                rec_end = true;
                                next = p + term.len();
                                break;
                            }
                            if data[p..].starts_with(delim) {
                                k = p;
                                rec_end = false;
                                next = p + delim.len();
                                break;
                            }
                            k = p + 1;
                        }
                    }
                }
                let mut e = k;
                if strip_cr && rec_end && e > i && data[e - 1] == b'\r' {
                    e -= 1;
                }
                sink.field(field, &text[i..e]);
            }
            field += 1;
            i = next;
            if rec_end {
                break;
            }
            if i >= n {
                // 区切り文字で終わった: 最後の空のフィールド
                sink.field(field, "");
                field += 1;
                break;
            }
        }
        sink.end(field);
        records += 1;
    }
    i
}

// ---- 見本と推定 ----------------------------------------------------------------------

/// 先頭を読んで、文字コード・区切り文字・レコードの終わり・見出し・列の型を推定する。
pub fn preview(path: &Path) -> io::Result<CsvPreview> {
    preview_as(path, None, None, None)
}

/// [`preview`]（区切り文字・レコードの終わり・文字コードを決めて。`None` のものは推定する）。
pub fn preview_as(
    path: &Path,
    dialect: Option<Dialect>,
    record_end: Option<RecordEnd>,
    encoding: Option<Encoding>,
) -> io::Result<CsvPreview> {
    let (head, complete) = yy_io::read_shared_head(path, 1 << 20)?;
    let (encoding, bom_len) = match encoding {
        Some(e) => {
            let bom = e.bom();
            (
                e,
                if !bom.is_empty() && head.starts_with(bom) {
                    bom.len()
                } else {
                    0
                },
            )
        }
        None => {
            let det = yy_encoding::detect(&head, complete);
            (det.encoding, det.bom_len)
        }
    };
    let text = utf8_of(encoding, &head[bom_len.min(head.len())..]);
    let record_end = record_end.unwrap_or_else(|| sniff_record_end(&text));
    let dialect = dialect.unwrap_or_else(|| {
        sniff_dialect(&text, &record_end)
            .or_else(|| {
                let ext = path.extension()?.to_str()?.to_ascii_lowercase();
                Dialect::for_extension(&ext)
            })
            .unwrap_or_else(Dialect::csv)
    });
    let rows = sample_records(&text, &dialect, &record_end, 2000, !complete);
    let header = guess_header(&rows);
    let types = infer_types(&rows[header as usize..], DateSystem::D1900);
    Ok(CsvPreview {
        options: CsvOptions {
            dialect,
            record_end,
            encoding,
            header,
            types,
            all_text: false,
            date_system: DateSystem::D1900,
        },
        rows: rows.into_iter().take(50).collect(),
    })
}

/// 改行以外でレコードが終わるファイルの候補（ふつうの文章には現れない区切りの制御コード）。
const RECORD_END_CANDIDATES: [u8; 3] = [0x1E, 0x1D, 0x1C];

/// レコードの終わりを推定する。RS・GS・FS が LF と同じくらい（LF の 1/4 以上）あればそれ、LF がなく
/// CR があれば CR、そうでなければ改行。
fn sniff_record_end(text: &[u8]) -> RecordEnd {
    let lf = memchr::memchr_iter(b'\n', text).count();
    let best = RECORD_END_CANDIDATES
        .iter()
        .map(|&b| (memchr::memchr_iter(b, text).count(), b))
        .max_by_key(|&(n, _)| n);
    if let Some((n, b)) = best
        && n >= 2
        && n * 4 >= lf
    {
        return RecordEnd::Custom(vec![b]);
    }
    if lf == 0 && memchr::memchr_iter(b'\r', text).count() >= 2 {
        return RecordEnd::Custom(vec![b'\r']);
    }
    RecordEnd::Newline
}

/// 区切り文字を推定する（よくある区切り文字か、ASCII の区切りの US（0x1F））。
fn sniff_dialect(text: &[u8], end: &RecordEnd) -> Option<Dialect> {
    // 推定は LF で区切る前提なので、改行以外の終わりは LF に読み替える（見本だけ。値の中の LF は空白に）
    let lf;
    let text = match end.custom() {
        Some(t) if t.len() == 1 => {
            lf = text
                .iter()
                .map(|&b| match b {
                    b'\n' | b'\r' if b != t[0] => b' ',
                    b if b == t[0] => b'\n',
                    b => b,
                })
                .collect::<Vec<u8>>();
            &lf
        }
        _ => text,
    };
    // US（0x1F）が行の数ほどあれば、それが区切り文字（ふつうの文章には現れない）
    let lines = text
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .count();
    let us = memchr::memchr_iter(0x1F, text).count();
    if us > 0 && us * 2 >= lines {
        return Dialect::new(b"\x1f", Some(b'"'));
    }
    yy_delimited::sniff(text)
}

fn utf8_of(enc: Encoding, bytes: &[u8]) -> Vec<u8> {
    if enc == Encoding::Utf8 {
        bytes.to_vec()
    } else {
        yy_encoding::decode_all(enc, bytes, false).0
    }
}

/// 先頭のレコード（`drop_last` なら途中で切れたかもしれない最後のレコードを除く）。
fn sample_records(
    text: &[u8],
    d: &Dialect,
    end: &RecordEnd,
    limit: usize,
    drop_last: bool,
) -> Vec<Vec<String>> {
    let mut c = Collect::default();
    parse_records(text, d, end, limit + 1, &mut c);
    let mut rows = c.rows;
    if drop_last && rows.len() > 1 {
        rows.pop();
    }
    rows.truncate(limit);
    rows
}

fn is_textual(s: &str) -> bool {
    matches!(parse_input(s, DateSystem::D1900), Parsed::Text) && !s.trim().is_empty()
}

/// 1 行目が見出しらしいか: すべて空でない文字列で、重なりがなく、2 行目以降に文字列でない値がある列が
/// あるか、すべての列が文字列のときは 1 行目の値が 2 行目以降に出てこない。
fn guess_header(rows: &[Vec<String>]) -> bool {
    let Some(first) = rows.first() else {
        return false;
    };
    if first.is_empty() || !first.iter().all(|s| is_textual(s)) {
        return false;
    }
    let mut uniq = std::collections::HashSet::new();
    if !first.iter().all(|s| uniq.insert(s)) {
        return false;
    }
    if rows.len() < 2 {
        return true;
    }
    let body = &rows[1..];
    let typed = (0..first.len()).any(|c| {
        body.iter()
            .filter_map(|r| r.get(c))
            .any(|s| !s.trim().is_empty() && !is_textual(s))
    });
    typed || (0..first.len()).all(|c| !body.iter().any(|r| r.get(c) == Some(&first[c])))
}

/// 見本から列の型を推定する。
pub fn infer_types(rows: &[Vec<String>], sys: DateSystem) -> Vec<ColType> {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    (0..cols)
        .map(|c| {
            let mut kind: Option<ColType> = None;
            let mut formats: Vec<Option<&'static str>> = Vec::new();
            for r in rows {
                let Some(s) = r.get(c) else { continue };
                if s.trim().is_empty() {
                    continue;
                }
                let k = match parse_input(s, sys) {
                    Parsed::Number(_, f) => {
                        formats.push(f);
                        ColType::Number(None)
                    }
                    Parsed::Bool(_) => ColType::Bool,
                    Parsed::Text => return ColType::Text,
                };
                match &kind {
                    None => kind = Some(k),
                    Some(prev) if *prev == k => {}
                    _ => return ColType::Text,
                }
            }
            match kind {
                None => ColType::Auto,
                Some(ColType::Number(_)) => {
                    // 日付・時刻が混じれば日時の形、全部同じ形ならその形
                    let dates: Vec<&str> = formats
                        .iter()
                        .flatten()
                        .copied()
                        .filter(|f| f.contains('y') || f.contains('h'))
                        .collect();
                    let fmt = if !dates.is_empty() {
                        if dates.iter().any(|f| f.contains('y') && f.contains('h')) {
                            Some("yyyy/m/d h:mm")
                        } else if dates.iter().all(|f| f.contains('y')) {
                            Some("yyyy/m/d")
                        } else {
                            dates.first().copied()
                        }
                    } else if let Some(first) = formats.iter().flatten().next()
                        && formats.iter().flatten().all(|f| f == first)
                    {
                        // 形のある値（1,200 など）と形のない値（300）が混じれば、形のある方
                        Some(*first)
                    } else {
                        None
                    };
                    ColType::Number(fmt.map(str::to_owned))
                }
                Some(k) => k,
            }
        })
        .collect()
}

// ---- 取り込み ------------------------------------------------------------------------

/// 値を列の型に合わせて足す。
fn push_value(b: &mut Builder, ty: &ColType, s: &str, sys: DateSystem) {
    if s.is_empty() {
        b.push(CellRef::Empty);
        return;
    }
    match ty {
        ColType::Text => b.push(CellRef::Text(s)),
        ColType::Number(_) | ColType::Bool | ColType::Auto => match parse_input(s, sys) {
            Parsed::Number(n, _) if !matches!(ty, ColType::Bool) => b.push(CellRef::Number(n)),
            Parsed::Bool(v) if !matches!(ty, ColType::Number(_)) => b.push(CellRef::Bool(v)),
            _ => b.push(CellRef::Text(s)),
        },
    }
}

/// 列の一部（区画の結果）。
#[derive(Clone)]
enum Seg {
    Piece(Piece),
    /// 空の行（区画の途中から現れた列の、それより前の行）
    Empty(u64),
}

/// 区画の結果。
struct Part0 {
    cols: Vec<Vec<Seg>>,
    rows: u64,
}

/// 区画を読んでチャンクに詰める受け手。
struct Section<'a> {
    ctx: &'a Context,
    opts: &'a CsvOptions,
    types: &'a [ColType],
    rows_per_chunk: usize,
    builders: Vec<Builder>,
    cols: Vec<Vec<Seg>>,
    /// 今のチャンクの行数
    in_chunk: usize,
    rows: u64,
    err: Option<io::Error>,
}

impl Section<'_> {
    fn flush(&mut self) {
        let n = self.in_chunk;
        for (c, b) in self.builders.iter_mut().enumerate() {
            let data = std::mem::take(b).finish();
            debug_assert_eq!(data.len(), n);
            match Chunk::create(self.ctx, data) {
                Ok(ch) => self.cols[c].push(Seg::Piece(Piece {
                    len: ch.rows,
                    chunk: ch,
                    start: 0,
                })),
                Err(e) => self.err = Some(e),
            }
        }
        self.in_chunk = 0;
    }
}

impl Sink for Section<'_> {
    fn field(&mut self, i: usize, v: &str) {
        while self.builders.len() <= i {
            // 新しい列: 今のチャンクのそれまでの行は空、それより前のチャンクの分は空の区間
            let mut b = Builder::default();
            for _ in 0..self.in_chunk {
                b.push(CellRef::Empty);
            }
            self.builders.push(b);
            let before = self.rows - self.in_chunk as u64;
            self.cols.push(if before > 0 {
                vec![Seg::Empty(before)]
            } else {
                Vec::new()
            });
        }
        let ty = if self.opts.all_text {
            &ColType::Text
        } else {
            self.types.get(i).unwrap_or(&ColType::Auto)
        };
        push_value(&mut self.builders[i], ty, v, self.opts.date_system);
    }

    fn end(&mut self, n: usize) {
        // 足りないフィールドは空
        for b in self.builders.iter_mut().skip(n) {
            b.push(CellRef::Empty);
        }
        self.in_chunk += 1;
        self.rows += 1;
        if self.in_chunk == self.rows_per_chunk {
            self.flush();
        }
    }
}

/// 区画を読んでチャンクを作る。
fn parse_section(
    ctx: &Context,
    text: &[u8],
    opts: &CsvOptions,
    types: &[ColType],
    rows_per_chunk: usize,
) -> io::Result<Part0> {
    let mut s = Section {
        ctx,
        opts,
        types,
        rows_per_chunk,
        builders: Vec::new(),
        cols: Vec::new(),
        in_chunk: 0,
        rows: 0,
        err: None,
    };
    parse_records(text, &opts.dialect, &opts.record_end, usize::MAX, &mut s);
    if s.in_chunk > 0 {
        s.flush();
    }
    if let Some(e) = s.err {
        return Err(e);
    }
    Ok(Part0 {
        cols: s.cols,
        rows: s.rows,
    })
}

/// CSV を取り込んで新しいシートを作る。`progress(読んだバイト数, 全体)` が `false` を返したら中止する。
pub fn import(
    ctx: &Context,
    path: &Path,
    opts: &CsvOptions,
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<Sheet> {
    import_with(ctx, path, opts, progress, None)
}

/// [`import`]（区画の大きさを決められる。試験用）。
fn import_with(
    ctx: &Context,
    path: &Path,
    opts: &CsvOptions,
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
    section_override: Option<usize>,
) -> io::Result<Sheet> {
    let trace = std::env::var_os("YYSHEET_TRACE").is_some();
    let t0 = std::time::Instant::now();
    let lap = |what: &str| {
        if trace {
            eprintln!("[import] {what}: {:.3}s", t0.elapsed().as_secs_f64());
        }
    };
    check_separators(&opts.dialect, &opts.record_end).map_err(data_err)?;
    let term = opts.record_end.custom();
    let file = yy_io::open_file(path)?;
    let raw = file.bytes();
    let enc = opts.encoding;
    let bom = if !enc.bom().is_empty() && raw.starts_with(enc.bom()) {
        enc.bom().len()
    } else {
        0
    };
    // UTF-16 など、LF で区切って読めない文字コードは先に UTF-8 にする。改行以外で終わるときは、
    // 区切りが 2 バイト目に現れない制御コード（0x40 未満）だけのときに限って区画ごとに変換する
    let low = |b: &[u8]| b.iter().all(|&x| x < 0x40);
    let per_section_ok =
        enc.splits_at_lf() && term.is_none_or(|t| low(t) && low(opts.dialect.delimiter()));
    let transcoded;
    let (data, per_section): (&[u8], Option<Encoding>) = if enc == Encoding::Utf8 {
        (&raw[bom..], None)
    } else if per_section_ok {
        (&raw[bom..], Some(enc))
    } else {
        transcoded = transcode_to_temp(enc, &raw[bom..], progress)?;
        (transcoded.bytes(), None)
    };
    let total = data.len() as u64;
    let d = opts.dialect;

    // 見出し
    let mut start = 0usize;
    let mut names: Vec<String> = Vec::new();
    if opts.header && !data.is_empty() {
        let head_end = match term {
            None => first_record_end(data, &d),
            Some(t) => walk(data, &d, t, false, 1).pos,
        };
        let head = match per_section {
            Some(e) => yy_encoding::decode_all(e, &data[..head_end], false).0,
            None => data[..head_end].to_vec(),
        };
        let mut c = Collect::default();
        parse_records(&head, &d, &opts.record_end, 1, &mut c);
        names = c.rows.into_iter().next().unwrap_or_default();
        start = head_end;
    }
    let body = &data[start..];

    // 1 レコードの平均の大きさから、チャンクの行数と区画の大きさを決める
    let sample = &body[..body.len().min(1 << 20)];
    let sample_text = match per_section {
        Some(e) => yy_encoding::decode_all(e, sample, false).0,
        None => sample.to_vec(),
    };
    let sample_rows = sample_records(
        &sample_text,
        &d,
        &opts.record_end,
        5000,
        sample.len() < body.len(),
    );
    let types: Vec<ColType> = if !opts.types.is_empty() {
        opts.types.clone()
    } else {
        infer_types(&sample_rows, opts.date_system)
    };
    let avg = if sample_rows.is_empty() {
        64
    } else {
        (sample.len() / sample_rows.len()).max(1)
    };
    let rows_per_chunk = ((64usize << 20) / avg)
        .clamp(4096, MAX_ROWS)
        .next_power_of_two()
        .min(MAX_ROWS);
    // チャンク 2 つ分ほど。ただし全コアに行き渡るよう、少なくともスレッドの 4 倍の区画にする
    let threads = rayon::current_num_threads().max(1);
    let section = section_override.unwrap_or_else(|| {
        (rows_per_chunk * avg * 2)
            .min(body.len() / (threads * 4) + 1)
            .clamp(4 << 20, 256 << 20)
    });
    let rows_per_chunk = if section_override.is_some() {
        7
    } else {
        rows_per_chunk
    };

    // 区画の境目（LF・レコードの終わりの直後）
    let term_bytes: &[u8] = term.unwrap_or(b"\n");
    let finder = memchr::memmem::Finder::new(term_bytes);
    let mut bounds = vec![0usize];
    let mut pos = section.min(body.len());
    while pos < body.len() {
        match finder.find(&body[pos..]) {
            Some(k) => {
                let b = pos + k + term_bytes.len();
                if b < body.len() {
                    bounds.push(b);
                }
                pos = b.saturating_add(section);
            }
            None => break,
        }
    }
    bounds.push(body.len());
    let nsec = bounds.len() - 1;

    lap(&format!(
        "sections {nsec} x {section} rows/chunk {rows_per_chunk}"
    ));
    // 1 回目: 各区画の終わりの状態を 2 通り求める
    let ends: Vec<(bool, bool)> = (0..nsec)
        .into_par_iter()
        .map(|k| {
            let s = &body[bounds[k]..bounds[k + 1]];
            if let Some(t) = term {
                return (
                    walk(s, &d, t, false, usize::MAX).in_quotes,
                    walk(s, &d, t, true, usize::MAX).in_quotes,
                );
            }
            let mut out = Scanner::new(d);
            out.feed(s);
            let mut inq = Scanner::at_line(
                d,
                LineState {
                    in_quotes: true,
                    field: 0,
                },
            );
            inq.feed(s);
            (out.line_state().in_quotes, inq.line_state().in_quotes)
        })
        .collect();
    let mut starts_in_quotes = vec![false; nsec];
    for k in 1..nsec {
        let prev = starts_in_quotes[k - 1];
        starts_in_quotes[k] = if prev { ends[k - 1].1 } else { ends[k - 1].0 };
    }
    // 引用符の中で始まる区画は、最初のレコードの終わりから読む
    let mut eff: Vec<usize> = (0..nsec)
        .into_par_iter()
        .map(|k| {
            if !starts_in_quotes[k] {
                return bounds[k];
            }
            if let Some(t) = term {
                return bounds[k] + walk(&body[bounds[k]..], &d, t, true, 1).pos;
            }
            let mut s = Scanner::at_line(
                d,
                LineState {
                    in_quotes: true,
                    field: 0,
                },
            );
            let mut p = bounds[k];
            while p < body.len() {
                match memchr::memchr(b'\n', &body[p..]) {
                    Some(m) => {
                        let before = s.records;
                        s.feed(&body[p..p + m + 1]);
                        p += m + 1;
                        if s.records > before {
                            return p;
                        }
                    }
                    None => return body.len(),
                }
            }
            body.len()
        })
        .collect();
    eff.push(body.len());
    for k in 1..eff.len() {
        eff[k] = eff[k].max(eff[k - 1]);
    }

    lap("pass 1");
    // 2 回目: 区画ごとに値を詰める（作業領域の予算に収まる数ずつ並列に）
    let per = (section as u64).saturating_mul(4);
    let par =
        (ctx.budget.of(Part::Work) / per).clamp(1, rayon::current_num_threads() as u64) as usize;
    let done = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    let mut parts: Vec<Part0> = Vec::with_capacity(nsec);
    let idx: Vec<usize> = (0..nsec).collect();
    for batch in idx.chunks(par) {
        let res: Vec<io::Result<Part0>> = batch
            .par_iter()
            .map(|&k| {
                if stop.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                let _lease = ctx.budget.lease(Part::Work, per);
                let slice = &body[eff[k]..eff[k + 1]];
                let decoded;
                let text: &[u8] = match per_section {
                    Some(e) => {
                        decoded = yy_encoding::decode_all(e, slice, false).0;
                        &decoded
                    }
                    None => slice,
                };
                let p = parse_section(ctx, text, opts, &types, rows_per_chunk)?;
                let n = done.fetch_add(slice.len() as u64, Ordering::Relaxed) + slice.len() as u64;
                if !progress(n, total) {
                    stop.store(true, Ordering::Relaxed);
                }
                Ok(p)
            })
            .collect();
        for r in res {
            parts.push(r?);
        }
        if stop.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
    }

    lap("pass 2");
    // 組み立て
    let ncols = parts
        .iter()
        .map(|p| p.cols.len())
        .max()
        .unwrap_or(0)
        .max(names.len());
    let mut columns: Vec<Vec<Piece>> = vec![Vec::new(); ncols];
    let mut rows: u64 = 0;
    for p in parts {
        for (c, col) in columns.iter_mut().enumerate() {
            match p.cols.get(c) {
                Some(segs) => {
                    for sg in segs {
                        match sg {
                            Seg::Piece(pc) => col.push(pc.clone()),
                            Seg::Empty(n) => col.extend(empty_run(ctx, *n)?),
                        }
                    }
                }
                None => col.extend(empty_run(ctx, p.rows)?),
            }
        }
        rows += p.rows;
    }
    let mut cols = Vec::with_capacity(ncols);
    for (c, pieces) in columns.into_iter().enumerate() {
        let name = names
            .get(c)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| crate::col_name(c as u32));
        let mut col = Column::from_pieces(&name, pieces);
        if col.rows() < rows {
            col.extend_empty(ctx, rows - col.rows())?;
        }
        if let Some(ColType::Number(Some(f))) = types.get(c)
            && !opts.all_text
        {
            col.format = Some(Arc::from(f.as_str()));
        }
        cols.push(col);
    }
    if cols.iter().any(|c| c.rows() != rows) {
        return Err(data_err("取り込んだ列の行数が揃っていません".into()));
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sheet1".into());
    let mut sheet = Sheet::new(&name);
    sheet.table = Table {
        columns: Arc::new(cols),
        rows,
        header: opts.header,
    };
    progress(total, total);
    Ok(sheet)
}

/// 空の区間（`n` 行）。
fn empty_run(ctx: &Context, n: u64) -> io::Result<Vec<Piece>> {
    let mut c = Column::new("");
    c.extend_empty(ctx, n)?;
    Ok(c.pieces().to_vec())
}

/// [`walk`] の結果。
struct Walked {
    /// 読み終えた位置（`records` 個目のレコードの終わりの直後か、データの終わり）
    pos: usize,
    /// 引用符の中で終わったか
    in_quotes: bool,
}

/// レコードの終わりが `term` のデータを、`max_records` レコードまで読み進める（値は取り出さない。
/// 引用符の扱いは [`parse_text`] と同じ）。`start_quoted` なら引用符の中から読み始める。
fn walk(data: &[u8], d: &Dialect, term: &[u8], start_quoted: bool, max_records: usize) -> Walked {
    let delim = d.delimiter();
    let q = d.quote;
    let n = data.len();
    let mut i = 0;
    let mut quoted = start_quoted && q.is_some();
    let mut field_start = !start_quoted;
    let mut records = 0;
    while i < n {
        if quoted {
            let qc = q.unwrap_or(b'"');
            match memchr::memchr(qc, &data[i..]) {
                None => {
                    i = n;
                    break;
                }
                Some(k) => {
                    let p = i + k;
                    if data.get(p + 1) == Some(&qc) {
                        i = p + 2;
                    } else {
                        // 閉じ引用符（この後は区切りまで普通の文字）
                        quoted = false;
                        field_start = false;
                        i = p + 1;
                    }
                }
            }
            continue;
        }
        if field_start && q == Some(data[i]) {
            quoted = true;
            field_start = false;
            i += 1;
            continue;
        }
        // 引用符の外: 区切り文字かレコードの終わりまで
        let mut k = i;
        loop {
            match memchr::memchr2(delim[0], term[0], &data[k..]) {
                None => {
                    i = n;
                    break;
                }
                Some(m) => {
                    let p = k + m;
                    if data[p..].starts_with(term) {
                        i = p + term.len();
                        records += 1;
                        field_start = true;
                        break;
                    }
                    if data[p..].starts_with(delim) {
                        i = p + delim.len();
                        field_start = true;
                        break;
                    }
                    k = p + 1;
                }
            }
        }
        if records >= max_records {
            break;
        }
    }
    Walked {
        pos: i.min(n),
        in_quotes: quoted,
    }
}

/// 最初のレコードの終わり（次のレコードの先頭）。
fn first_record_end(data: &[u8], d: &Dialect) -> usize {
    let mut s = Scanner::new(*d);
    let mut p = 0;
    while p < data.len() {
        match memchr::memchr(b'\n', &data[p..]) {
            Some(m) => {
                s.feed(&data[p..p + m + 1]);
                p += m + 1;
                if s.records > 0 {
                    return p;
                }
            }
            None => return data.len(),
        }
    }
    data.len()
}

/// UTF-8 に変換した作業用のファイル。
fn transcode_to_temp(
    enc: Encoding,
    bytes: &[u8],
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<yy_io::OpenedFile> {
    let tmp = yy_io::temp_path("csv-utf8");
    let r = (|| {
        let mut out = BufWriter::new(std::fs::File::create(&tmp)?);
        let mut dec = enc.new_decoder(false);
        let mut buf = Vec::with_capacity(4 << 20);
        let total = bytes.len() as u64;
        for (i, part) in bytes.chunks(2 << 20).enumerate() {
            buf.clear();
            let last = (i + 1) * (2 << 20) >= bytes.len();
            dec.decode(part, &mut buf, last);
            out.write_all(&buf)?;
            if !progress(((i + 1) * (2 << 20)).min(bytes.len()) as u64 / 4, total) {
                return Err(cancelled());
            }
        }
        out.flush()?;
        drop(out);
        let src = yy_io::map_temp(&tmp)?;
        let len = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        src.unlink();
        Ok(yy_io::OpenedFile {
            path: tmp.clone(),
            file_len: len,
            source: Some(src),
            guard: None,
        })
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

// ---- 書き出し ------------------------------------------------------------------------

/// 書き出しの設定。
#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub dialect: Dialect,
    pub encoding: Encoding,
    /// レコードの終わり（改行なら [`ExportOptions::crlf`] で CR LF か LF）
    pub record_end: RecordEnd,
    /// 改行を CRLF にする（`false` なら LF）
    pub crlf: bool,
    /// BOM を付ける（Unicode のとき）
    pub bom: bool,
    /// 表示形式を当てた文字列で書く（`false` なら元の値。日付は ISO 8601）
    pub formatted: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            dialect: Dialect::csv(),
            encoding: Encoding::Utf8,
            record_end: RecordEnd::Newline,
            crlf: true,
            bom: false,
            formatted: true,
        }
    }
}

/// 書き出しの結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub rows: u64,
    /// 文字コードに変換できなかった文字の数（`?` で書いた）
    pub unencodable: u64,
}

fn is_date_format(f: &str) -> bool {
    let l = f.to_ascii_lowercase();
    l.contains('y') || l.contains('d') || l.contains('h') || l.contains('s')
}

/// `v` が `pat` を含むか。
fn contains(v: &[u8], pat: &[u8]) -> bool {
    match pat {
        [] => false,
        [b] => memchr::memchr(*b, v).is_some(),
        _ => memchr::memmem::find(v, pat).is_some(),
    }
}

/// フィールドを書く（区切り文字・引用符・改行・レコードの終わりを含めば引用符で囲む）。
fn push_field(buf: &mut Vec<u8>, v: &[u8], d: &Dialect, term: &[u8]) {
    let Some(q) = d.quote else {
        buf.extend_from_slice(v);
        return;
    };
    let needs = v.iter().any(|&b| b == q || b == b'\n' || b == b'\r')
        || contains(v, d.delimiter())
        || contains(v, term);
    if !needs {
        buf.extend_from_slice(v);
        return;
    }
    buf.push(q);
    for &b in v {
        if b == q {
            buf.push(q);
        }
        buf.push(b);
    }
    buf.push(q);
}

/// 1 列の表で値が空の行は、空行と区別できるよう `""` にする。
fn empty_record(buf: &mut Vec<u8>, row_start: usize, cols: usize, d: &Dialect) {
    if cols == 1
        && buf.len() == row_start
        && let Some(q) = d.quote
    {
        buf.extend_from_slice(&[q, q]);
    }
}

/// セルを書く。
fn push_cell(
    buf: &mut Vec<u8>,
    v: CellRef<'_>,
    format: Option<&str>,
    opts: &ExportOptions,
    sys: DateSystem,
) {
    let term = opts.terminator();
    match v {
        CellRef::Empty => {}
        CellRef::Text(s) => {
            let s = crate::value::figurative_label(s).unwrap_or(s);
            push_field(buf, s.as_bytes(), &opts.dialect, term)
        }
        _ => {
            let s = cell_text(v, format, opts, sys);
            push_field(buf, s.as_bytes(), &opts.dialect, term);
        }
    }
}

/// 1 レコードを書く（終わりの区切りも）。
fn push_record<V: AsRef<[u8]>>(buf: &mut Vec<u8>, fields: &[V], opts: &ExportOptions) {
    let row_start = buf.len();
    let term = opts.terminator();
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            buf.extend_from_slice(opts.dialect.delimiter());
        }
        push_field(buf, f.as_ref(), &opts.dialect, term);
    }
    empty_record(buf, row_start, fields.len(), &opts.dialect);
    buf.extend_from_slice(term);
}

impl ExportOptions {
    /// レコードの終わりに書くバイト列。
    pub fn terminator(&self) -> &[u8] {
        match &self.record_end {
            RecordEnd::Custom(t) => t,
            RecordEnd::Newline if self.crlf => b"\r\n",
            RecordEnd::Newline => b"\n",
        }
    }
}

/// 値の文字列（書き出し用）。
fn cell_text(
    v: CellRef<'_>,
    format: Option<&str>,
    opts: &ExportOptions,
    sys: DateSystem,
) -> String {
    match v {
        CellRef::Empty => String::new(),
        CellRef::Number(n) => match format {
            Some(f) if opts.formatted => yy_numfmt::format_number(f, n, sys),
            Some(f) if is_date_format(f) => {
                iso_date(n, f, sys).unwrap_or_else(|| yy_numfmt::general(n))
            }
            _ => yy_numfmt::general(n),
        },
        CellRef::Text(s) => crate::value::figurative_label(s).unwrap_or(s).to_owned(),
        CellRef::Bool(true) => "TRUE".into(),
        CellRef::Bool(false) => "FALSE".into(),
        CellRef::Error(e) => e.text().into(),
    }
}

fn iso_date(n: f64, format: &str, sys: DateSystem) -> Option<String> {
    let dt = yy_numfmt::date::datetime_from_serial(sys, n)?;
    let l = format.to_ascii_lowercase();
    let has_date = l.contains('y') || l.contains('d');
    let has_time = l.contains('h') || l.contains('s');
    Some(match (has_date, has_time) {
        (true, false) => format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day),
        (false, true) => format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second),
        _ => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
        ),
    })
}

/// シートを CSV に書き出す。`order` があれば、その順の表の行だけを書く（並べ替え・絞り込みの結果）。
pub fn export(
    ctx: &Context,
    sheet: &Sheet,
    sys: DateSystem,
    path: &Path,
    opts: &ExportOptions,
    order: Option<&[u32]>,
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<ExportReport> {
    let tmp = path.with_extension("yysheet-export.tmp");
    let r = export_to(ctx, sheet, sys, &tmp, opts, order, progress);
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

fn export_to(
    ctx: &Context,
    sheet: &Sheet,
    sys: DateSystem,
    path: &Path,
    opts: &ExportOptions,
    order: Option<&[u32]>,
    progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<ExportReport> {
    check_separators(&opts.dialect, &opts.record_end).map_err(data_err)?;
    let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    let mut enc = opts.encoding.new_encoder(EscapeMode::Reject);
    let mut report = ExportReport::default();
    let term = opts.terminator();
    let mut write = |text: &[u8], last: bool, report: &mut ExportReport| -> io::Result<()> {
        if opts.encoding == Encoding::Utf8 {
            return out.write_all(text);
        }
        let mut buf = Vec::with_capacity(text.len() + 16);
        let mut bad = 0u64;
        enc.encode(text, &mut buf, last, &mut |_| bad += 1);
        report.unencodable += bad;
        out.write_all(&buf)
    };
    if opts.bom && opts.encoding.supports_bom() {
        // UTF-16 などはエンコーダが U+FEFF を BOM の形にする
        write("\u{FEFF}".as_bytes(), false, &mut report)?;
    }
    let t = &sheet.table;
    let (grid_rows, grid_cols) = sheet.extent();
    // 表示形式の書式が列全体だけなら、列ごとの形式にまとめる（一部の範囲の形式は格子を書く）
    let whole_col_formats = sheet
        .styles
        .layers()
        .iter()
        .all(|l| (l.style.num_fmt.is_none() && !l.clear) || l.rect.whole_cols());
    // 自由なセルがなければ（または行の順が決まっていれば）表だけを速く書く
    let simple = (sheet.cells.is_empty() || order.is_some())
        && whole_col_formats
        && sheet.formulas.results.is_empty()
        && sheet.formulas.shared.is_empty();
    let cols: Vec<&Column> = t.columns.iter().collect();
    let col_formats: Vec<Option<std::sync::Arc<str>>> = (0..cols.len() as u32)
        .map(|c| {
            let mut f = cols[c as usize].format.clone();
            for l in sheet.styles.layers() {
                if l.rect.whole_cols() && (l.rect.left..=l.rect.right).contains(&c) {
                    if l.clear {
                        f = cols[c as usize].format.clone();
                    }
                    if let Some(x) = &l.style.num_fmt {
                        f = Some(x.clone());
                    }
                }
            }
            f
        })
        .collect();
    let formats: Vec<Option<&str>> = col_formats.iter().map(|f| f.as_deref()).collect();
    let mut line = Vec::new();
    if t.header && !cols.is_empty() {
        let names: Vec<&[u8]> = cols.iter().map(|c| c.name.as_bytes()).collect();
        push_record(&mut line, &names, opts);
        write(&line, false, &mut report)?;
    }
    if simple {
        // 表の行を、チャンクの大きさの塊ごとに並列に組み立てる
        let total = order.map_or(t.rows, |o| o.len() as u64);
        const BLOCK: u64 = 16_384;
        let blocks: Vec<u64> = (0..total.div_ceil(BLOCK)).collect();
        let par = rayon::current_num_threads().max(1) * 2;
        for batch in blocks.chunks(par) {
            let texts: Vec<io::Result<Vec<u8>>> = batch
                .par_iter()
                .map(|&b| {
                    let lo = b * BLOCK;
                    let hi = (lo + BLOCK).min(total);
                    let n = (hi - lo) as usize;
                    let mut buf = Vec::with_capacity(n * cols.len() * 8);
                    match order {
                        None => {
                            // 列ごとに展開したチャンクの部分を、行ごとに順にたどる
                            let segs: Vec<Vec<(Arc<crate::chunk::Data>, usize, usize)>> = cols
                                .iter()
                                .map(|c| c.segments(ctx, lo..hi))
                                .collect::<io::Result<_>>()?;
                            let mut cur: Vec<(usize, usize)> = vec![(0, 0); cols.len()];
                            for k in 0..n {
                                let row = lo + k as u64;
                                let row_start = buf.len();
                                for (ci, c) in cols.iter().enumerate() {
                                    if ci > 0 {
                                        buf.extend_from_slice(opts.dialect.delimiter());
                                    }
                                    let (si, so) = &mut cur[ci];
                                    let (d, start, len) = &segs[ci][*si];
                                    let v = match c.delta().get(&row) {
                                        Some(v) => CellRef::of(v),
                                        None => d.get(start + *so),
                                    };
                                    push_cell(&mut buf, v, formats[ci], opts, sys);
                                    *so += 1;
                                    if *so == *len {
                                        *si += 1;
                                        *so = 0;
                                    }
                                }
                                empty_record(&mut buf, row_start, cols.len(), &opts.dialect);
                                buf.extend_from_slice(term);
                            }
                        }
                        Some(o) => {
                            for &r in &o[lo as usize..hi as usize] {
                                let row_start = buf.len();
                                for (ci, c) in cols.iter().enumerate() {
                                    if ci > 0 {
                                        buf.extend_from_slice(opts.dialect.delimiter());
                                    }
                                    let v = c.get(ctx, r as u64)?;
                                    push_cell(&mut buf, CellRef::of(&v), formats[ci], opts, sys);
                                }
                                empty_record(&mut buf, row_start, cols.len(), &opts.dialect);
                                buf.extend_from_slice(term);
                            }
                        }
                    }
                    Ok(buf)
                })
                .collect();
            for tx in texts {
                write(&tx?, false, &mut report)?;
            }
            let done = ((batch.last().copied().unwrap_or(0) + 1) * BLOCK).min(total);
            if !progress(done, total) {
                return Err(cancelled());
            }
        }
        report.rows = total + t.header as u64;
    } else {
        // 自由なセルがあるとき: 格子をそのまま書く
        for r in 0..grid_rows {
            if t.header && r == 0 && !cols.is_empty() {
                continue;
            }
            let mut row = Vec::with_capacity(grid_cols as usize);
            for c in 0..grid_cols {
                let v = sheet.get(ctx, r, c)?;
                let f = sheet.format_at(r, c);
                row.push(cell_text(CellRef::of(&v), f.as_deref(), opts, sys));
            }
            line.clear();
            push_record(&mut line, &row, opts);
            write(&line, false, &mut report)?;
            if r % 65_536 == 0 && !progress(r, grid_rows) {
                return Err(cancelled());
            }
        }
        report.rows = grid_rows;
    }
    write(&[], true, &mut report)?;

    out.flush()?;
    out.get_ref().sync_data()?;
    Ok(report)
}

#[cfg(test)]
mod tests;
