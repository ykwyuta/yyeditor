//! 独自形式のファイル（`.yys`。15 章 5）。
//!
//! ```text
//! ヘッダー（16 バイト: "YYSHEET\0"・形式の版・0）
//! チャンク …（8 バイト境界に揃える）
//! 目次（postcard。シート・列・区間・差分・自由なセル・列幅・チャンクの位置と統計）
//! 末尾（32 バイト: 目次の位置・長さ・チェックサム・"YYSEND01"）
//! ```
//!
//! 開くときは末尾と目次だけを読む。保存は、開いたファイルなら変わったチャンクと新しい目次・末尾を
//! 書き足すだけ（使われなくなった部分が 30% を超えたら、新しいファイルに書き直して置き換える）。
//! 書き足しの途中で落ちても、前の末尾が残っているので前に保存した状態で開ける。

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yy_numfmt::DateSystem;

use crate::Context;
use crate::chunk::{Chunk, Stats};
use crate::column::{Column, Piece};
use crate::query::{ColFilter, SortKey};
use crate::sheet::{Document, Sheet, Table, Workbook};
use crate::store::{Region, Store};
use crate::style::{Layer, Styles};
use crate::value::Value;

pub const MAGIC: &[u8; 8] = b"YYSHEET\0";
pub const END_MAGIC: &[u8; 8] = b"YYSEND01";
/// 形式の版
pub const VERSION: u32 = 1;
const TRAILER: u64 = 32;
/// 使われなくなった部分がこの割合を超えたら書き直す
const GARBAGE_RATIO: f64 = 0.3;

#[derive(Serialize, Deserialize)]
struct Dir {
    version: u32,
    date1904: bool,
    chunks: Vec<ChunkDir>,
    sheets: Vec<SheetDir>,
    /// 前の目次の位置（0 ならない）
    prev: u64,
}

/// 目次のあとに続ける追加の情報（ないファイルは既定）。版を重ねるごとに記録を後ろに足していき、
/// 古いファイル（記録が少ない）も読めるようにする。
#[derive(Serialize, Deserialize, Default)]
struct Ext {
    /// （シートの番号, 絞り込みの段階, 並べ替えのキー）
    views: Vec<(u32, Vec<ColFilter>, Vec<SortKey>)>,
}

/// 2 つ目の追加の記録: 書式。
#[derive(Serialize, Deserialize, Default)]
struct Ext2 {
    /// （シートの番号, 書式の層）
    styles: Vec<(u32, Vec<Layer>)>,
}

/// シートの式（シートの番号, （行, 列, 式の文字列））。
type SheetFormulas = (u32, Vec<(u64, u32, String)>);

/// 共有式（列, 始めの行, 終わりの行, 1 行目の式）。
type SharedDir = (u32, u64, u64, String);

/// 4 つ目の追加の記録: 共有式（シートの番号ごと）。
#[derive(Serialize, Deserialize, Default)]
struct Ext4 {
    shared: Vec<(u32, Vec<SharedDir>)>,
}

/// 5 つ目の追加の記録: 固定長ファイルの設定（シートの番号ごと）。
#[derive(Serialize, Deserialize, Default)]
struct Ext5 {
    fixed: Vec<(u32, FixedDir)>,
}

#[derive(Serialize, Deserialize)]
struct FixedDir {
    copybook: String,
    charset: String,
    little_endian: bool,
    separator: String,
    /// 列ごとの項目の番号
    fields: Vec<Option<u32>>,
}

/// 6 つ目の追加の記録: マルチレイアウト（シートの番号, （名前, コピーブック）の並び）。文字コード・
/// 区切り・列の項目は 5 つ目の記録に書く。
#[derive(Serialize, Deserialize, Default)]
struct Ext6 {
    multi: Vec<(u32, Vec<(String, String)>)>,
}

/// 3 つ目の追加の記録: 数式（結果は開いたときに計算し直す）。
#[derive(Serialize, Deserialize, Default)]
struct Ext3 {
    formulas: Vec<SheetFormulas>,
}

#[derive(Serialize, Deserialize)]
struct ChunkDir {
    offset: u64,
    len: u64,
    rows: u32,
    stats: Stats,
}

#[derive(Serialize, Deserialize)]
struct SheetDir {
    name: String,
    header: bool,
    rows: u64,
    columns: Vec<ColDir>,
    cells: Vec<(u64, u32, Value)>,
    widths: Vec<(u32, f32)>,
    frozen: (u32, u32),
}

#[derive(Serialize, Deserialize)]
struct ColDir {
    name: String,
    format: Option<String>,
    /// （チャンクの番号, 始まり, 行数）
    pieces: Vec<(u32, u32, u32)>,
    delta: Vec<(u64, Value)>,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

fn checksum(b: &[u8]) -> u64 {
    // FNV-1a
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 進みを知らせる（`false` を返したら中止）。
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64) -> bool;

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "中止しました")
}

/// ブックが使っているチャンク（重なりなし、出てきた順）。
fn live_chunks(book: &Workbook) -> Vec<Arc<Chunk>> {
    let mut seen = HashMap::new();
    let mut out = Vec::new();
    for s in &book.sheets {
        for c in s.table.columns.iter() {
            for p in c.pieces() {
                seen.entry(p.chunk.id).or_insert_with(|| {
                    out.push(p.chunk.clone());
                });
            }
        }
    }
    out
}

/// 保存の方法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveKind {
    /// 開いたファイルに書き足した
    Appended,
    /// 新しいファイルに書いて置き換えた
    Rewritten,
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// 保存の前の状態（文書の外でバックグラウンドに保存するため）。
#[derive(Clone, Debug)]
pub struct SaveState {
    /// 開いた（保存した）独自形式のファイル
    pub file: Option<Arc<Store>>,
    /// そのファイルの最後の目次の位置
    pub last_dir: u64,
}

/// 保存の結果（[`Document::saved`] で文書に戻す）。
#[derive(Debug)]
pub struct Saved {
    pub kind: SaveKind,
    pub path: PathBuf,
    file: Arc<Store>,
    last_dir: u64,
}

impl Document {
    /// 保存に要る状態（ブックは `self.book.clone()` で O(1) に複製できる）。
    pub fn save_state(&self) -> SaveState {
        SaveState {
            file: self.file.clone(),
            last_dir: self.last_dir,
        }
    }

    /// 保存の結果を戻す。
    pub fn saved(&mut self, s: Saved) {
        self.file = Some(s.file);
        self.last_dir = s.last_dir;
        self.path = Some(s.path);
        self.dirty = false;
    }
}

/// 保存する。
pub fn save(doc: &mut Document, path: &Path, progress: Progress<'_>) -> io::Result<SaveKind> {
    let s = save_book(&doc.book, doc.save_state(), path, progress)?;
    let kind = s.kind;
    doc.saved(s);
    Ok(kind)
}

/// ブックを保存する（文書を借りずに。結果は [`Document::saved`] で戻す）。
pub fn save_book(
    book: &Workbook,
    state: SaveState,
    path: &Path,
    progress: Progress<'_>,
) -> io::Result<Saved> {
    let chunks = live_chunks(book);
    let total: u64 = chunks.iter().map(|c| c.loc().len).sum();
    // 書き足せるか
    if let Some(store) = state.file.clone()
        && store.writable()
        && same_path(&store.path(), path)
    {
        let live_in_file: u64 = chunks
            .iter()
            .map(|c| c.loc())
            .filter(|l| l.store.id == store.id)
            .map(|l| l.len)
            .sum();
        let new_bytes = total - live_in_file;
        let after = store.len() + new_bytes;
        let garbage = after.saturating_sub(live_in_file + new_bytes);
        if (garbage as f64) <= after as f64 * GARBAGE_RATIO {
            let last_dir = write_into(book, state.last_dir, &store, &chunks, progress, total)?;
            return Ok(Saved {
                kind: SaveKind::Appended,
                path: path.to_owned(),
                file: store,
                last_dir,
            });
        }
    }
    // 新しいファイルに書いて置き換える
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "book.yys".into());
    let tmp = dir
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(format!(".{name}.{}.saving", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let store = Store::create(&tmp)?;
    let r = (|| {
        let mut header = Vec::with_capacity(16);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        store.append(&header)?;
        write_into(book, 0, &store, &chunks, progress, total)
    })();
    let last_dir = match r {
        Ok(d) => d,
        Err(e) => {
            store.delete_when_dropped();
            return Err(e);
        }
    };
    // 開いているファイルを置き換えるなら、退避してから
    if let Some(old) = state.file.clone()
        && same_path(&old.path(), path)
        && path.exists()
    {
        let aside = tmp.with_extension(format!("old{}", old.id));
        std::fs::rename(path, &aside)?;
        old.set_path(&aside);
        old.delete_when_dropped();
    }
    std::fs::rename(&tmp, path)?;
    store.set_path(path);
    Ok(Saved {
        kind: SaveKind::Rewritten,
        path: path.to_owned(),
        file: store,
        last_dir,
    })
}

/// チャンク（`store` にないもの）・目次・末尾を `store` に書き足す。書いた目次の位置を返す。
fn write_into(
    book: &Workbook,
    prev: u64,
    store: &Arc<Store>,
    chunks: &[Arc<Chunk>],
    progress: Progress<'_>,
    total: u64,
) -> io::Result<u64> {
    let mut index = HashMap::new();
    let mut dirs = Vec::with_capacity(chunks.len());
    let mut moved: Vec<(Arc<Chunk>, Region)> = Vec::new();
    let mut done = 0u64;
    for (i, c) in chunks.iter().enumerate() {
        let loc = c.loc();
        let offset = if loc.store.id == store.id {
            loc.offset
        } else {
            let bytes = loc.read()?;
            let off = store.append(&bytes)?;
            moved.push((
                c.clone(),
                Region {
                    store: store.clone(),
                    offset: off,
                    len: loc.len,
                },
            ));
            off
        };
        done += loc.len;
        if i % 64 == 0 && !progress(done, total) {
            return Err(cancelled());
        }
        index.insert(c.id, i as u32);
        dirs.push(ChunkDir {
            offset,
            len: loc.len,
            rows: c.rows,
            stats: c.stats,
        });
    }
    let sheets = book
        .sheets
        .iter()
        .map(|s| SheetDir {
            name: s.name.to_string(),
            header: s.table.header,
            rows: s.table.rows,
            columns: s
                .table
                .columns
                .iter()
                .map(|c| ColDir {
                    name: c.name.to_string(),
                    format: c.format.as_deref().map(str::to_owned),
                    pieces: c
                        .pieces()
                        .iter()
                        .map(|p| (index[&p.chunk.id], p.start, p.len))
                        .collect(),
                    delta: c.delta().iter().map(|(r, v)| (*r, v.clone())).collect(),
                })
                .collect(),
            cells: s
                .cells
                .iter()
                .map(|(&(r, c), v)| (r, c, v.clone()))
                .collect(),
            widths: s.col_widths.iter().map(|(&c, &w)| (c, w)).collect(),
            frozen: s.frozen,
        })
        .collect();
    let dir = Dir {
        version: VERSION,
        date1904: book.date_system == DateSystem::D1904,
        chunks: dirs,
        sheets,
        prev,
    };
    let ext = Ext {
        views: book
            .sheets
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.view.is_empty())
            .map(|(i, s)| (i as u32, s.view.filters.clone(), s.view.sort.clone()))
            .collect(),
    };
    let mut bytes = postcard::to_allocvec(&dir).map_err(|e| invalid(&e.to_string()))?;
    bytes.extend(postcard::to_allocvec(&ext).map_err(|e| invalid(&e.to_string()))?);
    let ext2 = Ext2 {
        styles: book
            .sheets
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.styles.is_empty())
            .map(|(i, s)| (i as u32, s.styles.layers().to_vec()))
            .collect(),
    };
    bytes.extend(postcard::to_allocvec(&ext2).map_err(|e| invalid(&e.to_string()))?);
    let ext3 = Ext3 {
        formulas: book
            .sheets
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.formulas.cells.is_empty())
            .map(|(i, s)| {
                (
                    i as u32,
                    s.formulas
                        .cells
                        .iter()
                        .map(|(&(r, c), f)| (r, c, f.text.to_string()))
                        .collect(),
                )
            })
            .collect(),
    };
    bytes.extend(postcard::to_allocvec(&ext3).map_err(|e| invalid(&e.to_string()))?);
    let ext4 = Ext4 {
        shared: book
            .sheets
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.formulas.shared.is_empty())
            .map(|(i, s)| {
                (
                    i as u32,
                    s.formulas
                        .shared
                        .iter()
                        .map(|x| (x.col, x.r0, x.r1, x.formula.text.to_string()))
                        .collect(),
                )
            })
            .collect(),
    };
    bytes.extend(postcard::to_allocvec(&ext4).map_err(|e| invalid(&e.to_string()))?);
    let ext5 = Ext5 {
        fixed: book
            .sheets
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let f = s.fixed.as_ref()?;
                Some((
                    i as u32,
                    FixedDir {
                        copybook: f.copybook.to_string(),
                        charset: f.codec.charset.name().to_string(),
                        little_endian: f.codec.little_endian,
                        separator: f.separator.name().to_string(),
                        fields: s.table.columns.iter().map(|c| c.field).collect(),
                    },
                ))
            })
            .collect(),
    };
    bytes.extend(postcard::to_allocvec(&ext5).map_err(|e| invalid(&e.to_string()))?);
    let ext6 = Ext6 {
        multi: book
            .sheets
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let f = s.fixed.as_ref().filter(|f| f.is_multi())?;
                Some((
                    i as u32,
                    f.multi
                        .iter()
                        .map(|m| (m.name.to_string(), m.copybook.to_string()))
                        .collect(),
                ))
            })
            .collect(),
    };
    bytes.extend(postcard::to_allocvec(&ext6).map_err(|e| invalid(&e.to_string()))?);
    let dir_off = store.append(&bytes)?;
    let mut trailer = Vec::with_capacity(TRAILER as usize);
    trailer.extend_from_slice(&dir_off.to_le_bytes());
    trailer.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    trailer.extend_from_slice(&checksum(&bytes).to_le_bytes());
    trailer.extend_from_slice(END_MAGIC);
    store.append(&trailer)?;
    store.sync()?;
    // 書けたので、チャンクの置き場所を新しいファイルに移す（Undo の履歴の分も同じチャンク）
    for (c, loc) in moved {
        c.relocate(loc);
    }
    progress(total, total);
    Ok(dir_off)
}

/// 末尾を探して目次を読む（末尾が壊れていれば、前の末尾を後ろから探す）。
type Exts = (Ext, Ext2, Ext3, Ext4, Ext5, Ext6);

fn read_dir(store: &Store) -> io::Result<(Dir, Exts, u64)> {
    let len = store.len();
    if len < 16 + TRAILER {
        return Err(invalid("yysheet のファイルではありません（短すぎます）"));
    }
    let head = store.read(0, 16)?;
    if &head[..8] != MAGIC {
        return Err(invalid("yysheet のファイルではありません"));
    }
    let version = u32::from_le_bytes(head[8..12].try_into().unwrap());
    if version > VERSION {
        return Err(invalid(&format!(
            "新しい版（{version}）の yysheet で保存されたファイルです"
        )));
    }
    let try_at = |end: u64| -> Option<(Dir, Exts, u64)> {
        let t = store.read(end - TRAILER, TRAILER).ok()?;
        if &t[24..32] != END_MAGIC {
            return None;
        }
        let off = u64::from_le_bytes(t[0..8].try_into().unwrap());
        let n = u64::from_le_bytes(t[8..16].try_into().unwrap());
        let sum = u64::from_le_bytes(t[16..24].try_into().unwrap());
        if off < 16 || off.checked_add(n)? > end - TRAILER {
            return None;
        }
        let b = store.read(off, n).ok()?;
        if checksum(&b) != sum {
            return None;
        }
        let (d, rest): (Dir, &[u8]) = postcard::take_from_bytes(&b).ok()?;
        let (ext, rest): (Ext, &[u8]) = postcard::take_from_bytes(rest).unwrap_or_default();
        let (ext2, rest): (Ext2, &[u8]) = postcard::take_from_bytes(rest).unwrap_or_default();
        let (ext3, rest): (Ext3, &[u8]) = postcard::take_from_bytes(rest).unwrap_or_default();
        let (ext4, rest): (Ext4, &[u8]) = postcard::take_from_bytes(rest).unwrap_or_default();
        let (ext5, rest): (Ext5, &[u8]) = postcard::take_from_bytes(rest).unwrap_or_default();
        let (ext6, _): (Ext6, &[u8]) = postcard::take_from_bytes(rest).unwrap_or_default();
        Some((d, (ext, ext2, ext3, ext4, ext5, ext6), off))
    };
    if let Some(r) = try_at(len) {
        return Ok(r);
    }
    // 書き足しの途中で落ちたファイル: 後ろから前の末尾を探す（最大 1 GB）
    const BLOCK: u64 = 1 << 20;
    let mut end = len;
    let stop = len.saturating_sub(1 << 30).max(16 + TRAILER);
    while end > stop {
        let start = end.saturating_sub(BLOCK).max(16);
        let b = store.read(start, end - start)?;
        let mut i = b.len();
        while i >= 8 {
            i -= 1;
            if i + 1 >= 8 && &b[i + 1 - 8..i + 1] == END_MAGIC {
                let cand_end = start + i as u64 + 1;
                if cand_end >= 16 + TRAILER
                    && let Some(r) = try_at(cand_end)
                {
                    return Ok(r);
                }
            }
        }
        if start <= 16 {
            break;
        }
        end = start + 8; // 境目をまたぐ印を見落とさない
        if end >= len {
            break;
        }
    }
    Err(invalid("目次が見つかりません（ファイルが壊れています）"))
}

/// 開く。
pub fn open(ctx: Arc<Context>, path: &Path) -> io::Result<Document> {
    let store = Store::open(path)?;
    let (dir, (ext, ext2, ext3, ext4, ext5, ext6), dir_off) = read_dir(&store)?;
    let chunks: Vec<Arc<Chunk>> = dir
        .chunks
        .iter()
        .map(|c| {
            Chunk::stored(
                c.rows,
                c.stats,
                Region {
                    store: store.clone(),
                    offset: c.offset,
                    len: c.len,
                },
            )
        })
        .collect();
    let mut sheets = Vec::with_capacity(dir.sheets.len());
    for s in dir.sheets {
        let mut cols = Vec::with_capacity(s.columns.len());
        for c in s.columns {
            let mut pieces = Vec::with_capacity(c.pieces.len());
            for (i, start, len) in c.pieces {
                let chunk = chunks
                    .get(i as usize)
                    .ok_or_else(|| invalid("目次のチャンクの番号が正しくありません"))?
                    .clone();
                if start as u64 + len as u64 > chunk.rows as u64 {
                    return Err(invalid("目次の区間が正しくありません"));
                }
                pieces.push(Piece { chunk, start, len });
            }
            let mut col = Column::from_pieces(&c.name, pieces);
            if col.rows() != s.rows {
                return Err(invalid("列の行数が揃っていません"));
            }
            col.format = c.format.map(|f| Arc::from(f.as_str()));
            col.set_delta(c.delta.into_iter().collect());
            cols.push(col);
        }
        let mut sheet = Sheet::new(&s.name);
        sheet.table = Table {
            columns: Arc::new(cols),
            rows: s.rows,
            header: s.header,
        };
        sheet.cells = Arc::new(s.cells.into_iter().map(|(r, c, v)| ((r, c), v)).collect());
        sheet.col_widths = Arc::new(s.widths.into_iter().collect::<BTreeMap<_, _>>());
        sheet.frozen = s.frozen;
        sheets.push(sheet);
    }
    if sheets.is_empty() {
        sheets.push(Sheet::new("Sheet1"));
    }
    // 絞り込み・並べ替えの設定（表示する行は、開いたあとで計算する）
    for (i, filters, sort) in ext.views {
        if let Some(s) = sheets.get_mut(i as usize) {
            let cols = s.table.cols();
            s.view.filters = filters.into_iter().filter(|f| f.col < cols).collect();
            s.view.sort = sort.into_iter().filter(|k| k.col < cols).collect();
        }
    }
    for (i, layers) in ext2.styles {
        if let Some(s) = sheets.get_mut(i as usize) {
            s.styles = Styles::from_layers(layers);
        }
    }
    for (i, list) in ext3.formulas {
        if let Some(s) = sheets.get_mut(i as usize) {
            let cells = std::sync::Arc::make_mut(&mut s.formulas.cells);
            for (r, c, text) in list {
                // 読めない式は文字列として残す
                match crate::formula::Formula::parse(&text) {
                    Ok(f) => {
                        cells.insert((r, c), f);
                    }
                    Err(_) => {
                        std::sync::Arc::make_mut(&mut s.cells).insert((r, c), Value::text(&text));
                    }
                }
            }
        }
    }
    for (i, list) in ext4.shared {
        if let Some(s) = sheets.get_mut(i as usize) {
            let shared = std::sync::Arc::make_mut(&mut s.formulas.shared);
            for (col, r0, r1, text) in list {
                if let Ok(formula) = crate::formula::Formula::parse(&text)
                    && r0 <= r1
                {
                    shared.push(crate::shared::Shared {
                        col,
                        r0,
                        r1,
                        formula,
                        results: None,
                    });
                }
            }
        }
    }
    for (i, f) in ext5.fixed {
        let (Some(s), Some(charset), Some(sep)) = (
            sheets.get_mut(i as usize),
            yy_cobol::Charset::from_name(&f.charset),
            crate::fixed::RecordSep::from_name(&f.separator),
        ) else {
            continue;
        };
        let codec = yy_cobol::Codec {
            charset,
            little_endian: f.little_endian,
        };
        // 読めないレイアウト（新しい版の書き方など）は設定ごと外す
        let spec = match ext6.multi.iter().find(|m| m.0 == i) {
            Some((_, layouts)) => crate::fixed::FixedSpec::new_multi(layouts, codec, sep),
            None => crate::fixed::FixedSpec::new(&f.copybook, codec, sep),
        };
        if let Ok(spec) = spec {
            let n = spec.max_fields() as u32;
            let multi = spec.is_multi();
            let cols = std::sync::Arc::make_mut(&mut s.table.columns);
            for (c, fi) in cols.iter_mut().zip(f.fields) {
                c.field = fi.filter(|&x| x < n || (multi && x == crate::fixed::LAYOUT_COL));
            }
            s.fixed = Some(std::sync::Arc::new(spec));
        }
    }
    let book = Workbook {
        sheets,
        date_system: if dir.date1904 {
            DateSystem::D1904
        } else {
            DateSystem::D1900
        },
    };
    let mut book = book;
    crate::formula::recalc(&mut book, &ctx);
    let mut doc = Document::with_book(ctx, book);
    doc.file = Some(store);
    doc.path = Some(path.to_owned());
    doc.last_dir = dir_off;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{CellRef, Data};

    fn book(ctx: &Context, rows: usize) -> Workbook {
        let mut cols = Vec::new();
        for c in 0..3 {
            let mut pieces = Vec::new();
            let vals: Vec<Value> = (0..rows)
                .map(|r| match c {
                    0 => Value::Number(r as f64),
                    1 => Value::text(&format!("名前{}", r % 7)),
                    _ => Value::Bool(r % 2 == 0),
                })
                .collect();
            for part in vals.chunks(1000) {
                let ch =
                    Chunk::create(ctx, Data::from_values(part.iter().map(CellRef::of))).unwrap();
                pieces.push(Piece {
                    len: ch.rows,
                    chunk: ch,
                    start: 0,
                });
            }
            cols.push(Column::from_pieces(&format!("列{c}"), pieces));
        }
        let mut s = Sheet::new("データ");
        s.table = Table {
            columns: Arc::new(cols),
            rows: rows as u64,
            header: true,
        };
        Arc::make_mut(&mut s.cells).insert((0, 5), Value::text("集計"));
        Workbook {
            sheets: vec![s, Sheet::new("空")],
            date_system: DateSystem::D1900,
        }
    }

    fn values(ctx: &Context, d: &Document) -> Vec<Value> {
        let s = &d.book.sheets[0];
        let (rows, cols) = s.extent();
        let mut v = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                v.push(s.get(ctx, r, c).unwrap());
            }
        }
        v
    }

    #[test]
    fn keeps_filter_and_sort_settings() {
        use crate::query::{Cmp, ColFilter, Cond, SortKey};
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.yys");
        let mut b = book(&ctx, 100);
        b.sheets[0].view.filters.push(ColFilter {
            col: 0,
            cond: Cond::Number {
                op: Cmp::Lt,
                value: 10.0,
            },
        });
        b.sheets[0].view.sort.push(SortKey { col: 1, desc: true });
        let style = crate::style::Style {
            fill: Some(0x00FF_EE00),
            bold: Some(true),
            num_fmt: Some(Arc::from("0.00")),
            ..Default::default()
        };
        b.sheets[0]
            .styles
            .set(crate::style::Rect::new(1, 0, 4, 1), style.clone());
        let mut doc = Document::with_book(ctx.clone(), b);
        save(&mut doc, &path, &mut |_, _| true).unwrap();
        let d2 = open(ctx.clone(), &path).unwrap();
        let v = &d2.book.sheets[0].view;
        assert_eq!(v.filters.len(), 1);
        assert_eq!(v.sort, vec![SortKey { col: 1, desc: true }]);
        assert!(v.rows.is_none());
        assert!(d2.book.sheets[1].view.is_empty());
        let s = &d2.book.sheets[0];
        assert_eq!(s.style_at(2, 1), style);
        assert_eq!(s.format_at(2, 1).as_deref(), Some("0.00"));
        assert_eq!(s.style_at(5, 1), Default::default());
        assert!(d2.book.sheets[1].styles.is_empty());
    }

    #[test]
    fn saves_appends_and_reopens() {
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.yys");
        let mut doc = Document::with_book(ctx.clone(), book(&ctx, 2500));
        let want = values(&ctx, &doc);
        assert_eq!(
            save(&mut doc, &path, &mut |_, _| true).unwrap(),
            SaveKind::Rewritten
        );
        let len1 = std::fs::metadata(&path).unwrap().len();
        let d2 = open(ctx.clone(), &path).unwrap();
        assert_eq!(values(&ctx, &d2), want);
        assert_eq!(d2.book.sheets.len(), 2);

        // 1 セル直して保存: 書き足すだけ
        doc.edit(|b, ctx| b.sheets[0].set(ctx, 5, 0, Value::Number(-1.0)))
            .unwrap();
        assert_eq!(
            save(&mut doc, &path, &mut |_, _| true).unwrap(),
            SaveKind::Appended
        );
        let len2 = std::fs::metadata(&path).unwrap().len();
        assert!(len2 > len1 && len2 - len1 < 10_000, "{len1} {len2}");
        let d3 = open(ctx.clone(), &path).unwrap();
        assert_eq!(
            d3.book.sheets[0].get(&ctx, 5, 0).unwrap(),
            Value::Number(-1.0)
        );

        // 書き足しの途中で落ちた（末尾が壊れた）: 前の状態で開ける（開いている間はほかから書けない
        // 〔Windows〕ので写しで試す）
        let broken = dir.path().join("broken.yys");
        std::fs::copy(&path, &broken).unwrap();
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&broken)
                .unwrap();
            f.write_all(&[7u8; 100]).unwrap();
        }
        let d4 = open(ctx.clone(), &broken).unwrap();
        assert_eq!(
            d4.book.sheets[0].get(&ctx, 5, 0).unwrap(),
            Value::Number(-1.0)
        );
        drop((d2, d3, d4));

        // 大きく変えると書き直す
        doc.edit(|b, ctx| {
            let s = &mut b.sheets[0];
            for c in 0..3 {
                let col = &mut std::sync::Arc::make_mut(&mut s.table.columns)[c];
                col.compact(ctx)?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            save(&mut doc, &path, &mut |_, _| true).unwrap(),
            SaveKind::Rewritten
        );
        let d5 = open(ctx.clone(), &path).unwrap();
        let mut want = want;
        want[5 * 6] = Value::Number(-1.0);
        assert_eq!(values(&ctx, &d5), want);
        // Undo で戻した状態も保存できる（古いチャンクを写す）
        assert!(doc.undo());
        assert!(doc.undo());
        save(&mut doc, &path, &mut |_, _| true).unwrap();
        let d6 = open(ctx.clone(), &path).unwrap();
        assert_eq!(
            d6.book.sheets[0].get(&ctx, 5, 0).unwrap(),
            Value::Number(4.0)
        );
    }

    #[test]
    fn rejects_other_files() {
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.yys");
        std::fs::write(&p, b"hello, this is not a sheet at all......").unwrap();
        assert!(open(ctx, &p).is_err());
    }
}
