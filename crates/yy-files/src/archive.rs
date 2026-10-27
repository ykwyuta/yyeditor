//! 圧縮ファイルの中（2026-10-27 の提案 8 章）: 展開せずに目次を読み、選んだ項目だけを展開する。
//!
//! - ZIP: 中央ディレクトリ（ZIP64・UTF-8 と CP932 の名前・Info-ZIP の Unicode の名前・UT の日時・
//!   暗号化の印）。展開は無圧縮と Deflate（CRC を確かめる）。
//! - tar（ustar・GNU の長い名前・pax）、tar.gz・tgz、tar.xz・txz: 頭から順に読む。gz・xz は 1 ファイル。
//! - 7z: 目次（LZMA・LZMA2 で縮めた目次も）。展開は無圧縮・LZMA・LZMA2 の 1 段のフォルダ。
//!
//! xz と LZMA2 は展開した全体をメモリに持つ（`lzma-rs` の作り）ため、[`XZ_MAX`] までに限る。
//! 壊れた・悪意のあるファイルに備えて、項目の数・名前の長さ・目次の大きさに上限を持つ。展開先の外に
//! 出る名前（`..`・絶対パス）は展開しない。

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 項目の数の上限
pub const MAX_ENTRIES: usize = 2_000_000;
/// xz・LZMA2 で展開できる大きさの上限（展開した全体をメモリに持つため）
pub const XZ_MAX: u64 = 1 << 30;
/// 目次の大きさの上限
const MAX_INDEX: u64 = 256 << 20;
/// tar の長い名前・pax の頭の上限
const MAX_META: u64 = 1 << 20;

/// 圧縮ファイルの形式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Tar,
    TarGz,
    TarXz,
    Gz,
    Xz,
    SevenZ,
}

impl Kind {
    /// 名前（拡張子）から形式を決める。
    pub fn of(name: &str) -> Option<Kind> {
        let n = name.to_ascii_lowercase();
        Some(if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
            Kind::TarGz
        } else if n.ends_with(".tar.xz") || n.ends_with(".txz") {
            Kind::TarXz
        } else if n.ends_with(".tar") {
            Kind::Tar
        } else if n.ends_with(".gz") {
            Kind::Gz
        } else if n.ends_with(".xz") {
            Kind::Xz
        } else if n.ends_with(".7z") {
            Kind::SevenZ
        } else if n.ends_with(".zip") {
            Kind::Zip
        } else {
            return None;
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Zip => "ZIP",
            Kind::Tar => "tar",
            Kind::TarGz => "tar.gz",
            Kind::TarXz => "tar.xz",
            Kind::Gz => "gzip",
            Kind::Xz => "xz",
            Kind::SevenZ => "7z",
        }
    }
}

/// 中の 1 項目。
#[derive(Clone, Debug, Default)]
pub struct Entry {
    /// `/` 区切りのパス（頭と終わりの `/` はない）
    pub path: String,
    pub dir: bool,
    /// 展開後の大きさ（分からなければ 0）
    pub size: u64,
    /// 圧縮後の大きさ（項目ごとに分からない形式は `None`）
    pub packed: Option<u64>,
    /// 更新日時（UNIX 秒）
    pub mtime: Option<i64>,
    /// `mtime` が手元の時刻（ZIP の DOS 時刻）か。`false` なら UTC
    pub mtime_local: bool,
    /// 圧縮の方式（表示用）
    pub method: String,
    pub encrypted: bool,
    /// 展開できるか（暗号化・対応していない方式は `false`）
    pub extractable: bool,
    /// 目次になく、中の項目のパスから補ったフォルダ
    pub implied: bool,
    loc: Loc,
}

impl Entry {
    /// 名前（パスの最後）。
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// フォルダの部分（`a/b/c.txt` なら `a/b`。一番上なら空）。
    pub fn dir_part(&self) -> &str {
        self.path.rfind('/').map_or("", |i| &self.path[..i])
    }
}

#[derive(Clone, Debug, Default)]
enum Loc {
    #[default]
    None,
    Zip {
        header: u64,
        method: u16,
        packed: u64,
        crc: u32,
    },
    /// tar の `ordinal` 番目の項目（中身はファイルの `data` の位置から。圧縮した tar では使わない）
    Tar {
        ordinal: usize,
        data: u64,
    },
    /// gz・xz の 1 ファイル
    Single,
    Seven {
        folder: usize,
        offset: u64,
    },
}

/// 圧縮ファイルの目次。
#[derive(Clone, Debug)]
pub struct Archive {
    pub kind: Kind,
    /// パスの順
    pub entries: Vec<Entry>,
    /// 7z のフォルダ（展開に使う）
    folders: Vec<SevenFolder>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn check(cancel: &dyn Fn() -> bool) -> io::Result<()> {
    if cancel() {
        Err(crate::cancelled())
    } else {
        Ok(())
    }
}

/// 名前のバイト列を文字列に（UTF-8 でなければ CP932）。
fn decode_name(b: &[u8], utf8: bool) -> String {
    if utf8 {
        return String::from_utf8_lossy(b).into_owned();
    }
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(b).0.into_owned(),
    }
}

/// パスを `/` 区切りに揃える（`\`・`.`・重なった `/` を除く）。空なら `None`。
fn normalize(name: &str) -> Option<String> {
    let parts: Vec<&str> = name
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() || name.len() > 32 * 1024 {
        return None;
    }
    Some(parts.join("/"))
}

/// 展開先の `base` の下のパス。`..`・ドライブ・Windows で使えない名前を含めば `None`。
pub fn dest_path(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = base.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains(':')
            || part.chars().any(|c| (c as u32) < 0x20)
        {
            return None;
        }
        p.push(part);
    }
    Some(p)
}

impl Archive {
    /// ファイルを開いて目次を読む（形式は名前から）。
    pub fn open(path: &Path, cancel: &dyn Fn() -> bool) -> io::Result<Archive> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let kind = Kind::of(name.as_deref().unwrap_or(""))
            .ok_or_else(|| bad("圧縮ファイルの形式が分かりません"))?;
        let mut f = File::open(path)?;
        Archive::read(&mut f, kind, name.as_deref().unwrap_or(""), cancel)
    }

    /// 目次を読む。`name` は gz・xz の中の名前を決めるのに使う。
    pub fn read<R: Read + Seek>(
        r: &mut R,
        kind: Kind,
        name: &str,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<Archive> {
        let mut folders = Vec::new();
        let entries = match kind {
            Kind::Zip => zip_entries(r, cancel)?,
            Kind::Tar => {
                r.seek(SeekFrom::Start(0))?;
                let mut t = Tar::new(cancel, None);
                copy_cancel(r, &mut t, cancel)?;
                t.finish()?
            }
            Kind::TarGz => {
                r.seek(SeekFrom::Start(0))?;
                let mut t = Tar::new(cancel, None);
                let mut d = flate2::read::MultiGzDecoder::new(BufReader::new(&mut *r));
                copy_cancel(&mut d, &mut t, cancel)?;
                t.finish()?
            }
            Kind::TarXz => {
                let total = xz_size(r)?;
                if total.is_some_and(|n| n > XZ_MAX) {
                    return Err(bad(format!(
                        "xz で {} を超えるものは中を読めません",
                        crate::human_size(XZ_MAX)
                    )));
                }
                r.seek(SeekFrom::Start(0))?;
                let mut t = Tar::new(cancel, None);
                let res = lzma_rs::xz_decompress(&mut BufReader::new(&mut *r), &mut t);
                if let Err(e) = res {
                    if let Some(e) = t.error.take() {
                        return Err(e);
                    }
                    return Err(bad(format!("xz を展開できません: {e}")));
                }
                t.finish()?
            }
            Kind::Gz => vec![gz_entry(r, name)?],
            Kind::Xz => {
                let size = xz_size(r)?.unwrap_or(0);
                let packed = r.seek(SeekFrom::End(0))?;
                vec![Entry {
                    path: inner_name(name, &[".xz"]),
                    size,
                    packed: Some(packed),
                    method: "LZMA2".into(),
                    extractable: size <= XZ_MAX,
                    loc: Loc::Single,
                    ..Default::default()
                }]
            }
            Kind::SevenZ => {
                let (e, f) = seven_entries(r, cancel)?;
                folders = f;
                e
            }
        };
        let mut a = Archive {
            kind,
            entries,
            folders,
        };
        a.tidy();
        Ok(a)
    }

    /// パスの順に並べ、同じパスは後のものを残し、中の項目から分かるフォルダを補う。
    fn tidy(&mut self) {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut kept: Vec<Entry> = Vec::with_capacity(self.entries.len());
        for e in std::mem::take(&mut self.entries) {
            match seen.get(&e.path) {
                Some(&i) => kept[i] = e,
                None => {
                    seen.insert(e.path.clone(), kept.len());
                    kept.push(e);
                }
            }
        }
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        for e in &kept {
            let mut p = e.path.as_str();
            while let Some(i) = p.rfind('/') {
                p = &p[..i];
                if !seen.contains_key(p) && !dirs.insert(p.to_owned()) {
                    break;
                }
            }
        }
        for d in dirs {
            kept.push(Entry {
                path: d,
                dir: true,
                extractable: true,
                implied: true,
                ..Default::default()
            });
        }
        kept.sort_by(|a, b| a.path.cmp(&b.path));
        self.entries = kept;
    }

    /// `dir`（`""` は一番上）のすぐ下の項目の番号。
    pub fn children(&self, dir: &str) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.dir_part() == dir)
            .map(|(i, _)| i)
            .collect()
    }

    /// `dir` の下のすべての項目の番号（`""` なら全部）。
    pub fn under(&self, dir: &str) -> Vec<usize> {
        let prefix = format!("{dir}/");
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| dir.is_empty() || e.path.starts_with(&prefix))
            .map(|(i, _)| i)
            .collect()
    }

    /// パスの項目の番号。
    pub fn find(&self, path: &str) -> Option<usize> {
        self.entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .ok()
    }

    /// 項目 `indices`（ファイルだけ。フォルダは無視）を展開する。`open(i)` が書き込み先を返す
    /// （`None` ならその項目は飛ばす）。展開したバイト数の合計を返す。
    pub fn extract_with<R: Read + Seek>(
        &self,
        r: &mut R,
        indices: &[usize],
        open: &mut dyn FnMut(usize) -> io::Result<Option<Box<dyn Write>>>,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<u64> {
        let mut wanted: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| self.entries.get(i).is_some_and(|e| !e.dir))
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        if let Some(&i) = wanted.iter().find(|&&i| !self.entries[i].extractable) {
            let e = &self.entries[i];
            return Err(bad(if e.encrypted {
                format!("{} は暗号化されているため展開できません", e.path)
            } else {
                format!(
                    "{} は対応していない方式（{}）のため展開できません",
                    e.path, e.method
                )
            }));
        }
        let mut total = 0;
        match self.kind {
            Kind::Zip | Kind::Tar => {
                for i in wanted {
                    check(cancel)?;
                    let Some(mut out) = open(i)? else { continue };
                    total += self.extract_seek(r, i, &mut out, cancel)?;
                    out.flush()?;
                }
            }
            Kind::Gz | Kind::Xz => {
                if let Some(&i) = wanted.first()
                    && let Some(mut out) = open(i)?
                {
                    r.seek(SeekFrom::Start(0))?;
                    let mut w = Counted {
                        out: &mut out,
                        n: 0,
                        cancel,
                    };
                    if self.kind == Kind::Gz {
                        let mut d = flate2::read::MultiGzDecoder::new(BufReader::new(&mut *r));
                        io::copy(&mut d, &mut w)?;
                    } else {
                        lzma_rs::xz_decompress(&mut BufReader::new(&mut *r), &mut w)
                            .map_err(|e| bad(format!("xz を展開できません: {e}")))?;
                    }
                    total = w.n;
                    out.flush()?;
                }
            }
            Kind::TarGz | Kind::TarXz => {
                // 番号（tar の中の順）→ 項目
                let mut by_ordinal: HashMap<usize, usize> = HashMap::new();
                for &i in &wanted {
                    if let Loc::Tar { ordinal, .. } = self.entries[i].loc {
                        by_ordinal.insert(ordinal, i);
                    }
                }
                if by_ordinal.is_empty() {
                    return Ok(0);
                }
                let mut pick = |ordinal: usize| -> io::Result<Option<Box<dyn Write>>> {
                    match by_ordinal.get(&ordinal) {
                        Some(&i) => open(i),
                        None => Ok(None),
                    }
                };
                let last = by_ordinal.keys().copied().max().unwrap_or(0);
                r.seek(SeekFrom::Start(0))?;
                let mut t = Tar::new(cancel, Some((&mut pick, last)));
                let res = if self.kind == Kind::TarGz {
                    let mut d = flate2::read::MultiGzDecoder::new(BufReader::new(&mut *r));
                    copy_cancel(&mut d, &mut t, cancel).map(|_| ())
                } else {
                    lzma_rs::xz_decompress(&mut BufReader::new(&mut *r), &mut t)
                        .map_err(|e| bad(format!("xz を展開できません: {e}")))
                };
                if let Some(e) = t.error.take() {
                    return Err(e);
                }
                if !t.done {
                    res?;
                }
                total = t.extracted;
            }
            Kind::SevenZ => {
                let mut by_folder: HashMap<usize, Vec<usize>> = HashMap::new();
                for &i in &wanted {
                    if let Loc::Seven { folder, .. } = self.entries[i].loc {
                        by_folder.entry(folder).or_default().push(i);
                    } else {
                        // 中身のない（大きさ 0 の）ファイル
                        if let Some(mut out) = open(i)? {
                            out.flush()?;
                        }
                    }
                }
                let mut folders: Vec<_> = by_folder.into_iter().collect();
                folders.sort_unstable_by_key(|(f, _)| *f);
                for (fi, mut items) in folders {
                    items.sort_unstable_by_key(|&i| match self.entries[i].loc {
                        Loc::Seven { offset, .. } => offset,
                        _ => 0,
                    });
                    let ranges: Vec<(u64, u64, usize)> = items
                        .iter()
                        .map(|&i| match self.entries[i].loc {
                            Loc::Seven { offset, .. } => (offset, self.entries[i].size, i),
                            _ => (0, 0, i),
                        })
                        .collect();
                    let mut sink = Ranges {
                        ranges,
                        next: 0,
                        pos: 0,
                        cur: None,
                        open,
                        done: false,
                        error: None,
                        cancel,
                        extracted: 0,
                    };
                    let res = seven_decode(r, &self.folders[fi], &mut sink);
                    if let Some(e) = sink.error.take() {
                        return Err(e);
                    }
                    if !sink.done {
                        res?;
                        sink.finish_current()?;
                        if sink.next < sink.ranges.len() {
                            return Err(bad("7z の中身が途中で終わっています"));
                        }
                    }
                    total += sink.extracted;
                }
            }
        }
        Ok(total)
    }

    /// ZIP・tar の 1 項目を展開する（ファイルの中を探す）。
    fn extract_seek<R: Read + Seek>(
        &self,
        r: &mut R,
        i: usize,
        out: &mut dyn Write,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<u64> {
        let e = &self.entries[i];
        let mut w = Counted { out, n: 0, cancel };
        match e.loc {
            Loc::Zip {
                header,
                method,
                packed,
                crc,
            } => {
                r.seek(SeekFrom::Start(header))?;
                let mut h = [0u8; 30];
                r.read_exact(&mut h)?;
                if h[..4] != [0x50, 0x4b, 0x03, 0x04] {
                    return Err(bad(format!("{}: ZIP の項目の頭が違います", e.path)));
                }
                let skip = u16le(&h, 26) as i64 + u16le(&h, 28) as i64;
                r.seek(SeekFrom::Current(skip))?;
                let data = (&mut *r).take(packed);
                let mut c = Crc {
                    w: &mut w,
                    h: crc32fast::Hasher::new(),
                    limit: e.size,
                };
                match method {
                    0 => {
                        io::copy(&mut BufReader::new(data), &mut c)?;
                    }
                    8 => {
                        let mut d = flate2::read::DeflateDecoder::new(BufReader::new(data));
                        io::copy(&mut d, &mut c)?;
                    }
                    _ => return Err(bad("対応していない圧縮です")),
                }
                let got = c.h.finalize();
                if got != crc {
                    return Err(bad(format!(
                        "{}: CRC が一致しません（壊れています）",
                        e.path
                    )));
                }
                if w.n != e.size {
                    return Err(bad(format!("{}: 大きさが目次と一致しません", e.path)));
                }
            }
            Loc::Tar { data, .. } => {
                r.seek(SeekFrom::Start(data))?;
                let mut part = (&mut *r).take(e.size);
                io::copy(&mut part, &mut w)?;
                if w.n != e.size {
                    return Err(bad(format!("{}: tar が途中で終わっています", e.path)));
                }
            }
            _ => return Err(bad("展開できません")),
        }
        Ok(w.n)
    }

    /// 項目を `base` の下へ展開する（`strip` はパスの頭から除くフォルダ。`""` なら除かない）。
    /// フォルダの項目はフォルダを作る。日時を移す。`progress(バイト数)` が `false` なら止める。
    pub fn extract_to_dir(
        &self,
        archive: &Path,
        indices: &[usize],
        base: &Path,
        strip: &str,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<u64> {
        let rel_of = |e: &Entry| -> io::Result<PathBuf> {
            let rel = if strip.is_empty() {
                e.path.as_str()
            } else {
                e.path
                    .strip_prefix(strip)
                    .and_then(|s| s.strip_prefix('/'))
                    .unwrap_or(e.path.as_str())
            };
            dest_path(base, rel).ok_or_else(|| bad(format!("{}: 展開できない名前です", e.path)))
        };
        for &i in indices {
            if let Some(e) = self.entries.get(i)
                && e.dir
            {
                std::fs::create_dir_all(rel_of(e)?)?;
            }
        }
        let mut times: Vec<(PathBuf, Option<i64>)> = Vec::new();
        let mut f = File::open(archive)?;
        let total = self.extract_with(
            &mut f,
            indices,
            &mut |i| {
                let e = &self.entries[i];
                let p = rel_of(e)?;
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let out = File::create(&p)?;
                times.push((p, e.mtime.filter(|_| !e.mtime_local)));
                Ok(Some(Box::new(io::BufWriter::new(out)) as Box<dyn Write>))
            },
            cancel,
        )?;
        for (p, t) in times {
            if let Some(t) = t
                && t >= 0
                && let Ok(f) = File::options().write(true).open(&p)
            {
                let _ = f
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t as u64));
            }
        }
        Ok(total)
    }

    /// 1 項目の中身（`limit` バイトまで）。
    pub fn read_entry<R: Read + Seek>(
        &self,
        r: &mut R,
        i: usize,
        limit: u64,
    ) -> io::Result<Vec<u8>> {
        if self.entries.get(i).is_none_or(|e| e.size > limit) {
            return Err(bad("大きすぎます"));
        }
        let buf = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>, u64);
        impl Write for Shared {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                let mut v = self.0.borrow_mut();
                if v.len() as u64 + b.len() as u64 > self.1 {
                    return Err(bad("大きすぎます"));
                }
                v.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let b2 = buf.clone();
        self.extract_with(
            r,
            &[i],
            &mut |_| Ok(Some(Box::new(Shared(b2.clone(), limit)) as Box<dyn Write>)),
            &|| false,
        )?;
        Ok(buf.take())
    }
}

/// 書いたバイト数を数え、中止を確かめる。
struct Counted<'a> {
    out: &'a mut dyn Write,
    n: u64,
    cancel: &'a dyn Fn() -> bool,
}

impl Write for Counted<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        check(self.cancel)?;
        self.out.write_all(b)?;
        self.n += b.len() as u64;
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// CRC-32 を計りながら書く（目次の大きさを超えたら止める: 展開爆弾）。
struct Crc<'a, 'b> {
    w: &'a mut Counted<'b>,
    h: crc32fast::Hasher,
    limit: u64,
}

impl Write for Crc<'_, '_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.w.n + b.len() as u64 > self.limit {
            return Err(bad("目次の大きさより大きく展開されます（壊れています）"));
        }
        self.h.update(b);
        self.w.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// 中止を確かめながら写す。
fn copy_cancel(r: &mut dyn Read, w: &mut dyn Write, cancel: &dyn Fn() -> bool) -> io::Result<u64> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut n = 0u64;
    loop {
        check(cancel)?;
        let k = match r.read(&mut buf) {
            Ok(0) => return Ok(n),
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        match w.write_all(&buf[..k]) {
            Ok(()) => {}
            // 欲しいものを全部書いたので止めた
            Err(e) if e.get_ref().is_some_and(|x| x.is::<Stop>()) => return Ok(n),
            Err(e) => return Err(e),
        }
        n += k as u64;
    }
}

/// 欲しい項目を書き終えたので読むのを止める印。
#[derive(Debug)]
struct Stop;

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stop")
    }
}

impl std::error::Error for Stop {}

fn stop() -> io::Error {
    io::Error::other(Stop)
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]))
}

/// DOS の日付・時刻 → 秒（手元の時刻のまま。UNIX 秒と同じ数え方）。
fn dos_time(date: u16, time: u16) -> Option<i64> {
    let y = 1980 + (date >> 9) as i64;
    let m = ((date >> 5) & 0x0f) as i64;
    let d = (date & 0x1f) as i64;
    if !(1..=12).contains(&m) || d == 0 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    Some(
        days * 86_400
            + ((time >> 11) as i64) * 3600
            + (((time >> 5) & 0x3f) as i64) * 60
            + ((time & 0x1f) as i64) * 2,
    )
}

/// 暦の日付 → 1970-01-01 からの日数。
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---- ZIP ----

fn zip_entries<R: Read + Seek>(r: &mut R, cancel: &dyn Fn() -> bool) -> io::Result<Vec<Entry>> {
    let len = r.seek(SeekFrom::End(0))?;
    let tail_len = len.min(22 + 65_535 + 20);
    r.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    r.read_exact(&mut tail)?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| bad("ZIP ではありません（終わりの印がありません）"))?;
    let eocd_pos = len - tail_len + eocd as u64;
    let mut count = u16le(&tail, eocd + 10) as u64;
    let mut cd_size = u32le(&tail, eocd + 12) as u64;
    let mut cd_off = u32le(&tail, eocd + 16) as u64;
    let mut zip64 = false;
    // ZIP64 の終わりの位置（直前の 20 バイト）
    if eocd >= 20 && tail[eocd - 20..eocd - 16] == [0x50, 0x4b, 0x06, 0x07] {
        let at = u64le(&tail, eocd - 20 + 8);
        r.seek(SeekFrom::Start(at))?;
        let mut z = [0u8; 56];
        r.read_exact(&mut z)?;
        if z[..4] != [0x50, 0x4b, 0x06, 0x06] {
            return Err(bad("ZIP64 の終わりの記録が壊れています"));
        }
        count = u64le(&z, 32);
        cd_size = u64le(&z, 40);
        cd_off = u64le(&z, 48);
        zip64 = true;
    }
    // 前に何か付いた ZIP（自己解凍など）: 目次の位置のずれ
    let shift = if zip64 {
        0
    } else {
        eocd_pos.saturating_sub(cd_off + cd_size)
    };
    if count > MAX_ENTRIES as u64 || cd_size > MAX_INDEX || count > cd_size / 46 + 1 {
        return Err(bad("ZIP の目次が大きすぎます"));
    }
    if cd_off + shift + cd_size > len {
        return Err(bad("ZIP の目次がファイルの外を指しています"));
    }
    r.seek(SeekFrom::Start(cd_off + shift))?;
    let mut cd = vec![0u8; cd_size as usize];
    r.read_exact(&mut cd)?;
    let mut out = Vec::with_capacity(count as usize);
    let mut p = 0;
    while p + 46 <= cd.len() && cd[p..p + 4] == [0x50, 0x4b, 0x01, 0x02] {
        if out.len() % 4096 == 0 {
            check(cancel)?;
        }
        let made_by = cd[p + 5];
        let flags = u16le(&cd, p + 8);
        let mut method = u16le(&cd, p + 10);
        let time = u16le(&cd, p + 12);
        let date = u16le(&cd, p + 14);
        let crc = u32le(&cd, p + 16);
        let mut packed = u32le(&cd, p + 20) as u64;
        let mut size = u32le(&cd, p + 24) as u64;
        let name_len = u16le(&cd, p + 28) as usize;
        let extra_len = u16le(&cd, p + 30) as usize;
        let comment_len = u16le(&cd, p + 32) as usize;
        let attrs = u32le(&cd, p + 38);
        let mut header = u32le(&cd, p + 42) as u64;
        let end = p + 46 + name_len + extra_len + comment_len;
        if end > cd.len() {
            return Err(bad("ZIP の目次が途中で終わっています"));
        }
        let raw_name = &cd[p + 46..p + 46 + name_len];
        let mut name = decode_name(raw_name, flags & 0x0800 != 0);
        let mut mtime = dos_time(date, time);
        let mut local = true;
        let mut encrypted = flags & 1 != 0;
        // 追加の欄
        let extra = &cd[p + 46 + name_len..p + 46 + name_len + extra_len];
        let mut q = 0;
        while q + 4 <= extra.len() {
            let id = u16le(extra, q);
            let n = u16le(extra, q + 2) as usize;
            let Some(d) = extra.get(q + 4..q + 4 + n) else {
                break;
            };
            match id {
                0x0001 => {
                    let mut k = 0;
                    let mut take = |v: &mut u64| {
                        if *v == u32::MAX as u64 && k + 8 <= d.len() {
                            *v = u64le(d, k);
                            k += 8;
                        }
                    };
                    take(&mut size);
                    take(&mut packed);
                    take(&mut header);
                }
                0x7075 if d.len() > 5 && d[0] == 1 => {
                    // Info-ZIP の Unicode の名前（元の名前の CRC が合うときだけ）
                    if u32le(d, 1) == crc32fast::hash(raw_name) {
                        name = String::from_utf8_lossy(&d[5..]).into_owned();
                    }
                }
                0x5455 if d.len() >= 5 && d[0] & 1 != 0 => {
                    mtime = Some(u32le(d, 1) as i32 as i64);
                    local = false;
                }
                0x9901 if d.len() >= 7 => {
                    // AES
                    encrypted = true;
                    method = u16le(d, 5);
                }
                _ => {}
            }
            q += 4 + n;
        }
        let dir = name.ends_with('/')
            || name.ends_with('\\')
            || (matches!(made_by, 0 | 11 | 14) && attrs & 0x10 != 0);
        if let Some(path) = normalize(&name) {
            out.push(Entry {
                path,
                dir,
                size: if dir { 0 } else { size },
                packed: (!dir).then_some(packed),
                mtime,
                mtime_local: local,
                method: zip_method(method).into(),
                encrypted,
                extractable: !encrypted && matches!(method, 0 | 8),
                implied: false,
                loc: Loc::Zip {
                    header: header + shift,
                    method,
                    packed,
                    crc,
                },
            });
        }
        if out.len() > MAX_ENTRIES {
            return Err(bad("ZIP の項目が多すぎます"));
        }
        p = end;
    }
    Ok(out)
}

fn zip_method(m: u16) -> &'static str {
    match m {
        0 => "無圧縮",
        8 => "Deflate",
        9 => "Deflate64",
        12 => "BZip2",
        14 => "LZMA",
        93 => "Zstandard",
        95 => "XZ",
        98 => "PPMd",
        _ => "その他",
    }
}

// ---- gz・xz ----

/// `name` から圧縮の拡張子を除いた名前。
fn inner_name(name: &str, exts: &[&str]) -> String {
    let lower = name.to_ascii_lowercase();
    for e in exts {
        if lower.ends_with(e) && name.len() > e.len() {
            return name[..name.len() - e.len()].to_owned();
        }
    }
    format!("{name}.out")
}

fn gz_entry<R: Read + Seek>(r: &mut R, name: &str) -> io::Result<Entry> {
    let len = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(0))?;
    let mut h = [0u8; 10];
    r.read_exact(&mut h)?;
    if h[0] != 0x1f || h[1] != 0x8b || h[2] != 8 {
        return Err(bad("gzip ではありません"));
    }
    let flags = h[3];
    let mtime = u32le(&h, 4);
    let mut inner = None;
    if flags & 4 != 0 {
        let mut x = [0u8; 2];
        r.read_exact(&mut x)?;
        r.seek(SeekFrom::Current(u16le(&x, 0) as i64))?;
    }
    if flags & 8 != 0 {
        let mut n = Vec::new();
        let mut b = [0u8; 1];
        while n.len() < 1024 {
            r.read_exact(&mut b)?;
            if b[0] == 0 {
                break;
            }
            n.push(b[0]);
        }
        // 名前は Latin-1 と決まっているが、日本では CP932・UTF-8 のことが多い
        let s = decode_name(&n, false);
        inner = normalize(&s).map(|s| s.rsplit('/').next().unwrap_or(&s).to_owned());
    }
    // 終わりの 4 バイトは大きさ（4 GB で回る）
    let mut size = 0;
    if len >= 18 {
        r.seek(SeekFrom::End(-4))?;
        let mut t = [0u8; 4];
        r.read_exact(&mut t)?;
        size = u32le(&t, 0) as u64;
    }
    Ok(Entry {
        path: inner.unwrap_or_else(|| inner_name(name, &[".gz"])),
        size,
        packed: Some(len),
        mtime: (mtime != 0).then_some(mtime as i64),
        method: "Deflate".into(),
        extractable: true,
        loc: Loc::Single,
        ..Default::default()
    })
}

/// xz の可変長の数。
fn xz_varint(b: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..9 {
        let c = *b.get(*p)?;
        *p += 1;
        v |= ((c & 0x7f) as u64) << (7 * i);
        if c & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// xz の索引から展開後の大きさを読む（1 つのストリームだけ。分からなければ `None`）。
fn xz_size<R: Read + Seek>(r: &mut R) -> io::Result<Option<u64>> {
    let len = r.seek(SeekFrom::End(0))?;
    if len < 32 {
        return Err(bad("xz ではありません"));
    }
    r.seek(SeekFrom::Start(0))?;
    let mut head = [0u8; 6];
    r.read_exact(&mut head)?;
    if head != [0xfd, b'7', b'z', b'X', b'Z', 0] {
        return Err(bad("xz ではありません"));
    }
    r.seek(SeekFrom::End(-12))?;
    let mut foot = [0u8; 12];
    r.read_exact(&mut foot)?;
    if foot[10..12] != *b"YZ" {
        return Ok(None);
    }
    let backward = (u32le(&foot, 4) as u64 + 1) * 4;
    if backward + 12 > len || backward > MAX_INDEX {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(len - 12 - backward))?;
    let mut idx = vec![0u8; backward as usize];
    r.read_exact(&mut idx)?;
    if idx.first() != Some(&0) {
        return Ok(None);
    }
    let mut p = 1;
    let Some(n) = xz_varint(&idx, &mut p) else {
        return Ok(None);
    };
    let mut total = 0u64;
    for _ in 0..n.min(1 << 20) {
        let (Some(_unpadded), Some(size)) = (xz_varint(&idx, &mut p), xz_varint(&idx, &mut p))
        else {
            return Ok(None);
        };
        total = total.saturating_add(size);
    }
    Ok(Some(total))
}

// ---- tar ----

/// tar を順に読む（`Write` に展開したバイト列を流し込む）。`targets` があれば、その番号の項目の
/// 中身を書き込み先へ書き、最後の番号を書き終えたら止める。
struct Tar<'a> {
    hdr: Vec<u8>,
    state: TarState,
    long_name: Option<String>,
    pax: HashMap<String, String>,
    entries: Vec<Entry>,
    ordinal: usize,
    pos: u64,
    ended: bool,
    cancel: &'a dyn Fn() -> bool,
    targets: Option<Targets<'a>>,
    cur: Option<Box<dyn Write>>,
    extracted: u64,
    /// 欲しいものを書き終えた
    done: bool,
    /// 書き込み先の失敗（展開のライブラリが包んでしまうため、ここに残す）
    error: Option<io::Error>,
}

type Targets<'a> = (
    &'a mut dyn FnMut(usize) -> io::Result<Option<Box<dyn Write>>>,
    usize,
);

enum TarState {
    Header,
    /// 中身（残り・その後の詰め物）
    Data {
        left: u64,
        pad: u64,
        what: TarData,
    },
}

enum TarData {
    Skip,
    Out,
    /// 長い名前（`L`）・pax（`x`）
    Meta(u8, Vec<u8>),
}

fn octal(b: &[u8]) -> Option<u64> {
    if b.first().is_some_and(|&c| c & 0x80 != 0) {
        // base-256（GNU。大きなファイル）
        let mut v: u64 = (b[0] & 0x7f) as u64;
        for &c in &b[1..] {
            v = v.checked_mul(256)?.checked_add(c as u64)?;
        }
        return Some(v);
    }
    let s: String = b
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as char)
        .collect();
    let s = s.trim();
    if s.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(s, 8).ok()
}

fn cstr(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    &b[..n]
}

impl<'a> Tar<'a> {
    fn new(cancel: &'a dyn Fn() -> bool, targets: Option<Targets<'a>>) -> Tar<'a> {
        Tar {
            hdr: Vec::with_capacity(512),
            state: TarState::Header,
            long_name: None,
            pax: HashMap::new(),
            entries: Vec::new(),
            ordinal: 0,
            pos: 0,
            ended: false,
            cancel,
            targets,
            cur: None,
            extracted: 0,
            done: false,
            error: None,
        }
    }

    fn finish(mut self) -> io::Result<Vec<Entry>> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        if !self.ended && self.entries.is_empty() && self.pos == 0 {
            return Err(bad("tar ではありません（空です）"));
        }
        if let TarState::Data { left, .. } = self.state
            && left > 0
        {
            return Err(bad("tar が途中で終わっています"));
        }
        Ok(self.entries)
    }

    fn header(&mut self) -> io::Result<()> {
        let h = std::mem::take(&mut self.hdr);
        if h.iter().all(|&c| c == 0) {
            self.ended = true;
            return Ok(());
        }
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    c as u64
                }
            })
            .sum();
        if octal(&h[148..156]) != Some(sum) {
            return Err(bad(if self.entries.is_empty() {
                "tar ではありません".to_owned()
            } else {
                format!("tar の頭が壊れています（{} バイト目）", self.pos)
            }));
        }
        let typ = h[156];
        let size = octal(&h[124..136]).ok_or_else(|| bad("tar の大きさが読めません"))?;
        let pad = (512 - size % 512) % 512;
        if matches!(typ, b'L' | b'K' | b'x' | b'g') {
            if size > MAX_META {
                return Err(bad("tar の頭が大きすぎます"));
            }
            self.state = TarState::Data {
                left: size,
                pad,
                what: if typ == b'L' || typ == b'x' {
                    TarData::Meta(typ, Vec::new())
                } else {
                    TarData::Skip
                },
            };
            return Ok(());
        }
        let ustar = &h[257..262] == b"ustar";
        let mut name_b = cstr(&h[0..100]).to_vec();
        if ustar && h[345] != 0 {
            let mut p = cstr(&h[345..500]).to_vec();
            p.push(b'/');
            p.extend_from_slice(&name_b);
            name_b = p;
        }
        let mut name = decode_name(&name_b, false);
        if let Some(n) = self.long_name.take() {
            name = n;
        }
        let mut size = size;
        let mut mtime = octal(&h[136..148]).map(|t| t as i64);
        if let Some(p) = self.pax.remove("path") {
            name = p;
        }
        if let Some(s) = self.pax.remove("size").and_then(|s| s.parse().ok()) {
            size = s;
        }
        if let Some(t) = self.pax.remove("mtime") {
            mtime = t.split('.').next().and_then(|s| s.parse().ok()).or(mtime);
        }
        self.pax.clear();
        let pad = (512 - size % 512) % 512;
        let dir = typ == b'5' || name.ends_with('/');
        let file = matches!(typ, b'0' | 0 | b'7');
        let ordinal = self.ordinal;
        self.ordinal += 1;
        let data = self.pos;
        if let Some(path) = normalize(&name) {
            if self.entries.len() >= MAX_ENTRIES {
                return Err(bad("tar の項目が多すぎます"));
            }
            self.entries.push(Entry {
                path,
                dir,
                size: if file { size } else { 0 },
                packed: None,
                mtime,
                mtime_local: false,
                method: match typ {
                    b'1' => "リンク".into(),
                    b'2' => "シンボリック リンク".into(),
                    _ if dir || file => String::new(),
                    _ => "特殊なファイル".into(),
                },
                encrypted: false,
                extractable: dir || file,
                implied: false,
                loc: Loc::Tar { ordinal, data },
            });
        }
        let mut what = TarData::Skip;
        if file && let Some((open, last)) = self.targets.as_mut() {
            if let Some(out) = open(ordinal)? {
                self.cur = Some(out);
                what = TarData::Out;
            }
            if ordinal >= *last && self.cur.is_none() {
                self.done = true;
            }
        }
        // 中身のない欲しい項目はここで閉じる
        if size == 0
            && let Some(mut out) = self.cur.take()
        {
            out.flush()?;
            if self
                .targets
                .as_ref()
                .is_some_and(|(_, last)| ordinal >= *last)
            {
                self.done = true;
            }
        }
        self.state = if file || size > 0 && !dir {
            TarState::Data {
                left: if file { size } else { 0 },
                pad: if file { pad } else { 0 },
                what,
            }
        } else {
            TarState::Header
        };
        if let TarState::Data {
            left: 0, pad: 0, ..
        } = self.state
        {
            self.state = TarState::Header;
        }
        Ok(())
    }

    fn meta_done(&mut self, typ: u8, v: Vec<u8>) {
        if typ == b'L' {
            self.long_name = Some(decode_name(cstr(&v), false));
            return;
        }
        // pax: 「長さ キー=値\n」の並び
        let mut p = 0;
        while p < v.len() {
            let Some(sp) = v[p..].iter().position(|&c| c == b' ') else {
                break;
            };
            let Some(n) = std::str::from_utf8(&v[p..p + sp])
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
            else {
                break;
            };
            if n == 0 || p + n > v.len() {
                break;
            }
            let rec = &v[p + sp + 1..p + n];
            let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
            if let Some(eq) = rec.iter().position(|&c| c == b'=') {
                let k = String::from_utf8_lossy(&rec[..eq]).into_owned();
                let val = String::from_utf8_lossy(&rec[eq + 1..]).into_owned();
                self.pax.insert(k, val);
            }
            p += n;
        }
    }

    fn feed(&mut self, mut b: &[u8]) -> io::Result<()> {
        while !b.is_empty() {
            if self.done {
                return Err(stop());
            }
            if self.ended {
                return Ok(());
            }
            match &mut self.state {
                TarState::Header => {
                    let need = 512 - self.hdr.len();
                    let k = need.min(b.len());
                    self.hdr.extend_from_slice(&b[..k]);
                    b = &b[k..];
                    self.pos += k as u64;
                    if self.hdr.len() == 512 {
                        self.header()?;
                    }
                }
                TarState::Data { left, pad, what } => {
                    if *left > 0 {
                        let k = (*left).min(b.len() as u64) as usize;
                        match what {
                            TarData::Skip => {}
                            TarData::Out => {
                                if let Some(out) = self.cur.as_mut() {
                                    check(self.cancel)?;
                                    out.write_all(&b[..k])?;
                                    self.extracted += k as u64;
                                }
                            }
                            TarData::Meta(_, v) => v.extend_from_slice(&b[..k]),
                        }
                        *left -= k as u64;
                        b = &b[k..];
                        self.pos += k as u64;
                        if *left == 0 {
                            if let Some(mut out) = self.cur.take() {
                                out.flush()?;
                                let ord = self.ordinal - 1;
                                if self.targets.as_ref().is_some_and(|(_, last)| ord >= *last) {
                                    self.done = true;
                                }
                            }
                            if let TarData::Meta(typ, v) = std::mem::replace(what, TarData::Skip) {
                                self.meta_done(typ, v);
                            }
                        }
                    } else {
                        let k = (*pad).min(b.len() as u64) as usize;
                        *pad -= k as u64;
                        b = &b[k..];
                        self.pos += k as u64;
                    }
                    if let TarState::Data {
                        left: 0, pad: 0, ..
                    } = self.state
                    {
                        self.state = TarState::Header;
                    }
                }
            }
        }
        if self.done {
            return Err(stop());
        }
        Ok(())
    }
}

impl Write for Tar<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self.feed(b) {
            Ok(()) => Ok(b.len()),
            Err(e) => {
                if !e.get_ref().is_some_and(|x| x.is::<Stop>()) && self.error.is_none() {
                    self.error = Some(io::Error::new(e.kind(), e.to_string()));
                }
                Err(e)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---- 7z ----

const SEVEN_SIG: [u8; 6] = [b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c];

#[derive(Clone, Debug, Default)]
struct Coder {
    id: Vec<u8>,
    props: Vec<u8>,
    outs: u64,
}

#[derive(Clone, Debug, Default)]
struct SevenFolder {
    coders: Vec<Coder>,
    bind_pairs: usize,
    /// 圧縮されたデータの位置・大きさ（ファイルの頭から）
    pack_pos: u64,
    pack_size: u64,
    unpack_size: u64,
}

impl SevenFolder {
    fn method(&self) -> String {
        let names: Vec<String> = self.coders.iter().map(|c| coder_name(&c.id)).collect();
        names.join("+")
    }

    fn encrypted(&self) -> bool {
        self.coders.iter().any(|c| c.id == [6, 0xf1, 7, 1])
    }

    /// 展開できるか（1 段の無圧縮・LZMA・LZMA2）。
    fn simple(&self) -> bool {
        self.coders.len() == 1
            && self.bind_pairs == 0
            && matches!(self.coders[0].id.as_slice(), [0] | [0x21] | [3, 1, 1])
            && (self.coders[0].id != [0x21] || self.unpack_size <= XZ_MAX)
    }
}

fn coder_name(id: &[u8]) -> String {
    match id {
        [0] => "無圧縮".into(),
        [0x21] => "LZMA2".into(),
        [3, 1, 1] => "LZMA".into(),
        [3, 3, 1, 3] => "BCJ".into(),
        [3, 3, 1, 0x1b] => "BCJ2".into(),
        [3, 3, 5, 1] => "ARM".into(),
        [0xa] => "ARM64".into(),
        [3] => "Delta".into(),
        [4, 1, 8] => "Deflate".into(),
        [4, 1, 9] => "Deflate64".into(),
        [4, 2, 2] => "BZip2".into(),
        [3, 4, 1] => "PPMd".into(),
        [6, 0xf1, 7, 1] => "AES".into(),
        _ => id.iter().map(|b| format!("{b:02X}")).collect(),
    }
}

/// 7z の目次のバイト列を読む。
struct Cursor7<'a> {
    b: &'a [u8],
    p: usize,
}

impl Cursor7<'_> {
    fn byte(&mut self) -> io::Result<u8> {
        let c = *self
            .b
            .get(self.p)
            .ok_or_else(|| bad("7z の目次が途中で終わっています"))?;
        self.p += 1;
        Ok(c)
    }

    fn bytes(&mut self, n: u64) -> io::Result<&[u8]> {
        let n = usize::try_from(n).map_err(|_| bad("7z の目次が壊れています"))?;
        let s = self
            .b
            .get(
                self.p
                    ..self
                        .p
                        .checked_add(n)
                        .ok_or_else(|| bad("7z の目次が壊れています"))?,
            )
            .ok_or_else(|| bad("7z の目次が途中で終わっています"))?;
        self.p += n;
        Ok(s)
    }

    /// 7z の数（頭のバイトのビットで長さが決まる）。
    fn num(&mut self) -> io::Result<u64> {
        let first = self.byte()?;
        let mut mask = 0x80u8;
        let mut v = 0u64;
        for i in 0..8 {
            if first & mask == 0 {
                let high = (first & mask.wrapping_sub(1)) as u64;
                return Ok(v | (high << (8 * i)));
            }
            v |= (self.byte()? as u64) << (8 * i);
            mask >>= 1;
        }
        Ok(v)
    }

    fn count(&mut self, limit: usize) -> io::Result<usize> {
        let n = self.num()?;
        if n > limit as u64 {
            return Err(bad("7z の目次の数が大きすぎます"));
        }
        Ok(n as usize)
    }

    fn u32(&mut self) -> io::Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32le(b, 0))
    }

    fn u64(&mut self) -> io::Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64le(b, 0))
    }

    fn bits(&mut self, n: usize) -> io::Result<Vec<bool>> {
        let mut v = Vec::with_capacity(n);
        let mut cur = 0u8;
        for i in 0..n {
            if i % 8 == 0 {
                cur = self.byte()?;
            }
            v.push(cur & (0x80 >> (i % 8)) != 0);
        }
        Ok(v)
    }

    /// 「すべて定義」のバイトと、そうでなければビット列。
    fn defined(&mut self, n: usize) -> io::Result<Vec<bool>> {
        if self.byte()? != 0 {
            Ok(vec![true; n])
        } else {
            self.bits(n)
        }
    }
}

/// 展開の情報（目次の中のストリーム）。
#[derive(Default)]
struct Streams {
    folders: Vec<SevenFolder>,
    /// フォルダごとのファイルの数
    per_folder: Vec<usize>,
    /// ファイルごとの大きさ（フォルダの順）
    sizes: Vec<u64>,
}

fn read_streams(c: &mut Cursor7) -> io::Result<Streams> {
    let mut s = Streams::default();
    let mut pack_pos = 0u64;
    let mut pack_sizes: Vec<u64> = Vec::new();
    loop {
        match c.byte()? {
            0x00 => break,
            0x06 => {
                pack_pos = c.num()?;
                let n = c.count(MAX_ENTRIES)?;
                loop {
                    match c.byte()? {
                        0x00 => break,
                        0x09 => {
                            pack_sizes = (0..n).map(|_| c.num()).collect::<io::Result<_>>()?;
                        }
                        0x0a => {
                            let d = c.defined(n)?;
                            for _ in d.iter().filter(|&&x| x) {
                                c.u32()?;
                            }
                        }
                        _ => return Err(bad("7z の目次（PackInfo）が読めません")),
                    }
                }
            }
            0x07 => {
                if c.byte()? != 0x0b {
                    return Err(bad("7z の目次（Folder）が読めません"));
                }
                let n = c.count(MAX_ENTRIES)?;
                if c.byte()? != 0 {
                    return Err(bad("7z の外部の目次には対応していません"));
                }
                let mut packed_counts = Vec::with_capacity(n);
                for _ in 0..n {
                    let nc = c.count(64)?;
                    let mut f = SevenFolder::default();
                    let (mut ins, mut outs) = (0u64, 0u64);
                    for _ in 0..nc {
                        let flag = c.byte()?;
                        let id = c.bytes((flag & 0x0f) as u64)?.to_vec();
                        let (ci, co) = if flag & 0x10 != 0 {
                            (c.num()?, c.num()?)
                        } else {
                            (1, 1)
                        };
                        let props = if flag & 0x20 != 0 {
                            let n = c.num()?;
                            c.bytes(n)?.to_vec()
                        } else {
                            Vec::new()
                        };
                        ins += ci;
                        outs += co;
                        f.coders.push(Coder {
                            id,
                            props,
                            outs: co,
                        });
                    }
                    if outs == 0 || outs > 64 || ins > 64 {
                        return Err(bad("7z の目次（Coder）が壊れています"));
                    }
                    f.bind_pairs = (outs - 1) as usize;
                    for _ in 0..f.bind_pairs {
                        c.num()?;
                        c.num()?;
                    }
                    let packed = ins.saturating_sub(f.bind_pairs as u64);
                    if packed > 1 {
                        for _ in 0..packed {
                            c.num()?;
                        }
                    }
                    packed_counts.push(packed as usize);
                    s.folders.push(f);
                }
                if c.byte()? != 0x0c {
                    return Err(bad("7z の目次（UnpackSize）が読めません"));
                }
                for f in &mut s.folders {
                    let outs: u64 = f.coders.iter().map(|c| c.outs).sum();
                    let sizes: Vec<u64> = (0..outs).map(|_| c.num()).collect::<io::Result<_>>()?;
                    // 主な出力: 結び付け（bind pair）されていないもの。1 段なら唯一の出力
                    f.unpack_size = sizes.last().copied().unwrap_or(0);
                    if f.coders.len() == 1 {
                        f.unpack_size = sizes[0];
                    }
                }
                loop {
                    match c.byte()? {
                        0x00 => break,
                        0x0a => {
                            let d = c.defined(n)?;
                            for _ in d.iter().filter(|&&x| x) {
                                c.u32()?;
                            }
                        }
                        _ => return Err(bad("7z の目次（Folder）が読めません")),
                    }
                }
                // 圧縮されたデータの位置
                let mut at = 32 + pack_pos;
                let mut k = 0;
                for (f, &np) in s.folders.iter_mut().zip(&packed_counts) {
                    f.pack_pos = at;
                    let size: u64 = pack_sizes.iter().skip(k).take(np).sum();
                    f.pack_size = size;
                    at += size;
                    k += np;
                }
            }
            0x08 => {
                let nf = s.folders.len();
                let mut per = vec![1usize; nf];
                let mut t = c.byte()?;
                if t == 0x0d {
                    for p in per.iter_mut() {
                        *p = c.count(MAX_ENTRIES)?;
                    }
                    t = c.byte()?;
                }
                let mut sizes = Vec::new();
                let have_sizes = t == 0x09;
                for (fi, &n) in per.iter().enumerate() {
                    if n == 0 {
                        continue;
                    }
                    let total = s.folders[fi].unpack_size;
                    let mut sum = 0u64;
                    if have_sizes {
                        for _ in 0..n - 1 {
                            let x = c.num()?;
                            sum = sum.saturating_add(x);
                            sizes.push(x);
                        }
                    }
                    sizes.push(total.saturating_sub(sum));
                }
                if have_sizes {
                    t = c.byte()?;
                }
                loop {
                    match t {
                        0x00 => break,
                        0x0a => {
                            let n: usize = per.iter().sum();
                            let d = c.defined(n)?;
                            for _ in d.iter().filter(|&&x| x) {
                                c.u32()?;
                            }
                        }
                        _ => {
                            let n = c.num()?;
                            c.bytes(n)?;
                        }
                    }
                    t = c.byte()?;
                }
                s.per_folder = per;
                s.sizes = sizes;
            }
            _ => return Err(bad("7z の目次（StreamsInfo）が読めません")),
        }
    }
    if s.per_folder.is_empty() {
        s.per_folder = vec![1; s.folders.len()];
        s.sizes = s.folders.iter().map(|f| f.unpack_size).collect();
    }
    Ok(s)
}

/// フォルダを展開して `out` に書く。
fn seven_decode<R: Read + Seek>(r: &mut R, f: &SevenFolder, out: &mut dyn Write) -> io::Result<()> {
    if !f.simple() {
        return Err(bad(format!("7z の {} は展開できません", f.method())));
    }
    r.seek(SeekFrom::Start(f.pack_pos))?;
    let data = (&mut *r).take(f.pack_size);
    let coder = &f.coders[0];
    match coder.id.as_slice() {
        [0] => {
            let mut d = data;
            io::copy(&mut d, out)?;
            Ok(())
        }
        [3, 1, 1] => {
            if coder.props.len() < 5 {
                return Err(bad("7z の LZMA の設定が壊れています"));
            }
            let mut input = BufReader::new(io::Cursor::new(coder.props[..5].to_vec()).chain(data));
            let opts = lzma_rs::decompress::Options {
                unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(f.unpack_size)),
                memlimit: Some(XZ_MAX as usize),
                allow_incomplete: false,
            };
            lzma_rs::lzma_decompress_with_options(&mut input, &mut { out }, &opts)
                .map_err(|e| bad(format!("7z を展開できません: {e}")))
        }
        [0x21] => lzma_rs::lzma2_decompress(&mut BufReader::new(data), &mut { out })
            .map_err(|e| bad(format!("7z を展開できません: {e}"))),
        _ => Err(bad("7z の方式に対応していません")),
    }
}

/// 上限付きでバイト列に展開する。
struct Limited {
    v: Vec<u8>,
    limit: u64,
}

impl Write for Limited {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.v.len() as u64 + b.len() as u64 > self.limit {
            return Err(bad("7z の目次が大きすぎます"));
        }
        self.v.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn seven_entries<R: Read + Seek>(
    r: &mut R,
    cancel: &dyn Fn() -> bool,
) -> io::Result<(Vec<Entry>, Vec<SevenFolder>)> {
    let len = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(0))?;
    let mut sig = [0u8; 32];
    r.read_exact(&mut sig)?;
    if sig[..6] != SEVEN_SIG {
        return Err(bad("7z ではありません"));
    }
    let next_off = u64le(&sig, 12);
    let next_size = u64le(&sig, 20);
    if next_size == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    if next_size > MAX_INDEX || 32u64.saturating_add(next_off).saturating_add(next_size) > len {
        return Err(bad(
            "7z の目次がファイルの外を指しています（途中で切れています）",
        ));
    }
    r.seek(SeekFrom::Start(32 + next_off))?;
    let mut head = vec![0u8; next_size as usize];
    r.read_exact(&mut head)?;
    if crc32fast::hash(&head) != u32le(&sig, 28) {
        return Err(bad("7z の目次の CRC が一致しません（壊れています）"));
    }
    // 縮めた目次（何段でも）
    for _ in 0..4 {
        if head.first() != Some(&0x17) {
            break;
        }
        check(cancel)?;
        let mut c = Cursor7 { b: &head, p: 1 };
        let s = read_streams(&mut c)?;
        let f = s
            .folders
            .first()
            .ok_or_else(|| bad("7z の目次が壊れています"))?;
        if f.encrypted() {
            return Err(bad("目次が暗号化された 7z は中を読めません"));
        }
        if f.unpack_size > MAX_INDEX {
            return Err(bad("7z の目次が大きすぎます"));
        }
        let mut out = Limited {
            v: Vec::new(),
            limit: MAX_INDEX,
        };
        seven_decode(r, f, &mut out)?;
        head = out.v;
    }
    let mut c = Cursor7 { b: &head, p: 0 };
    if c.byte()? != 0x01 {
        return Err(bad("7z の目次が読めません"));
    }
    let mut streams = Streams::default();
    let mut names: Vec<String> = Vec::new();
    let mut empty_stream: Vec<bool> = Vec::new();
    let mut empty_file: Vec<bool> = Vec::new();
    let mut mtimes: Vec<Option<u64>> = Vec::new();
    let mut attrs: Vec<Option<u32>> = Vec::new();
    let mut nfiles = 0usize;
    loop {
        match c.byte()? {
            0x00 => break,
            0x02 => {
                // 書庫の属性（読み飛ばす）
                loop {
                    let t = c.num()?;
                    if t == 0 {
                        break;
                    }
                    let n = c.num()?;
                    c.bytes(n)?;
                }
            }
            0x03 => return Err(bad("7z の追加のストリームには対応していません")),
            0x04 => streams = read_streams(&mut c)?,
            0x05 => {
                nfiles = c.count(MAX_ENTRIES)?;
                loop {
                    let t = c.num()?;
                    if t == 0 {
                        break;
                    }
                    let size = c.num()?;
                    let data = c.bytes(size)?;
                    let mut d = Cursor7 { b: data, p: 0 };
                    match t {
                        0x0e => empty_stream = d.bits(nfiles)?,
                        0x0f => {
                            let n = empty_stream.iter().filter(|&&x| x).count();
                            empty_file = d.bits(n)?;
                        }
                        0x11 => {
                            if d.byte()? != 0 {
                                return Err(bad("7z の外部の名前には対応していません"));
                            }
                            let rest = &data[1..];
                            let units: Vec<u16> = rest
                                .chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                .collect();
                            names = units
                                .split(|&u| u == 0)
                                .take(nfiles)
                                .map(String::from_utf16_lossy)
                                .collect();
                        }
                        0x14 => {
                            let def = d.defined(nfiles)?;
                            if d.byte()? != 0 {
                                return Err(bad("7z の外部の日時には対応していません"));
                            }
                            mtimes = def
                                .iter()
                                .map(|&x| if x { d.u64().ok() } else { None })
                                .collect();
                        }
                        0x15 => {
                            let def = d.defined(nfiles)?;
                            if d.byte()? != 0 {
                                return Err(bad("7z の外部の属性には対応していません"));
                            }
                            attrs = def
                                .iter()
                                .map(|&x| if x { d.u32().ok() } else { None })
                                .collect();
                        }
                        _ => {}
                    }
                }
            }
            _ => return Err(bad("7z の目次（Header）が読めません")),
        }
    }
    // ファイル → フォルダの中の位置
    let mut out = Vec::with_capacity(nfiles);
    let mut folder = 0usize;
    let mut in_folder = 0usize;
    let mut offset = 0u64;
    let mut size_i = 0usize;
    let mut empty_i = 0usize;
    for i in 0..nfiles {
        if i % 4096 == 0 {
            check(cancel)?;
        }
        let empty = empty_stream.get(i).copied().unwrap_or(false);
        let attr = attrs.get(i).copied().flatten();
        let mut e = Entry {
            mtime: mtimes
                .get(i)
                .copied()
                .flatten()
                .map(|ft| (ft / 10_000_000) as i64 - 11_644_473_600),
            ..Default::default()
        };
        if empty {
            let is_file = empty_file.get(empty_i).copied().unwrap_or(false);
            empty_i += 1;
            e.dir = !is_file || attr.is_some_and(|a| a & 0x10 != 0);
            e.extractable = true;
        } else {
            while streams
                .per_folder
                .get(folder)
                .is_some_and(|&n| in_folder >= n)
            {
                folder += 1;
                in_folder = 0;
                offset = 0;
            }
            let f = streams
                .folders
                .get(folder)
                .ok_or_else(|| bad("7z のファイルとフォルダの数が合いません"))?;
            e.size = streams.sizes.get(size_i).copied().unwrap_or(0);
            size_i += 1;
            e.method = f.method();
            e.encrypted = f.encrypted();
            e.extractable = f.simple();
            if streams.per_folder[folder] == 1 {
                e.packed = Some(f.pack_size);
            }
            e.loc = Loc::Seven { folder, offset };
            offset += e.size;
            in_folder += 1;
        }
        if let Some(p) = names.get(i).and_then(|n| normalize(n)) {
            e.path = p;
            out.push(e);
        }
    }
    Ok((out, streams.folders))
}

/// 7z のフォルダの展開から、欲しい範囲を書き込み先へ分ける。
struct Ranges<'a> {
    /// （フォルダの中の位置・大きさ・項目）を位置の順に
    ranges: Vec<(u64, u64, usize)>,
    next: usize,
    pos: u64,
    cur: Option<(Box<dyn Write>, u64)>,
    open: &'a mut dyn FnMut(usize) -> io::Result<Option<Box<dyn Write>>>,
    done: bool,
    error: Option<io::Error>,
    cancel: &'a dyn Fn() -> bool,
    extracted: u64,
}

impl Ranges<'_> {
    fn finish_current(&mut self) -> io::Result<()> {
        if let Some((mut w, _)) = self.cur.take() {
            w.flush()?;
        }
        Ok(())
    }

    /// 中身が 0 バイトの項目と、`pos` で始まる項目を開く。
    fn open_due(&mut self) -> io::Result<()> {
        while self.cur.is_none() {
            let Some(&(off, size, i)) = self.ranges.get(self.next) else {
                self.done = true;
                return Ok(());
            };
            if off > self.pos {
                return Ok(());
            }
            self.next += 1;
            let w = (self.open)(i)?;
            match w {
                Some(mut w) if size == 0 => w.flush()?,
                Some(w) => self.cur = Some((w, size)),
                None if size == 0 => {}
                // 書かない項目は読み飛ばす
                None => self.cur = Some((Box::new(io::sink()), size)),
            }
        }
        Ok(())
    }

    fn feed(&mut self, mut b: &[u8]) -> io::Result<()> {
        check(self.cancel)?;
        while !b.is_empty() {
            self.open_due()?;
            if self.done {
                return Err(stop());
            }
            match self.cur.as_mut() {
                Some((w, left)) => {
                    let k = (*left).min(b.len() as u64) as usize;
                    w.write_all(&b[..k])?;
                    *left -= k as u64;
                    self.extracted += k as u64;
                    self.pos += k as u64;
                    b = &b[k..];
                    if *left == 0 {
                        self.finish_current()?;
                    }
                }
                None => {
                    // 次の項目まで読み飛ばす
                    let next = self.ranges[self.next].0;
                    let k = (next - self.pos).min(b.len() as u64) as usize;
                    self.pos += k as u64;
                    b = &b[k..];
                }
            }
        }
        self.open_due()?;
        if self.done {
            return Err(stop());
        }
        Ok(())
    }
}

impl Write for Ranges<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self.feed(b) {
            Ok(()) => Ok(b.len()),
            Err(e) => {
                if !e.get_ref().is_some_and(|x| x.is::<Stop>()) && self.error.is_none() {
                    self.error = Some(io::Error::new(e.kind(), e.to_string()));
                }
                Err(e)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    fn no() -> bool {
        false
    }

    /// 試験用の ZIP を作る（`(名前, 中身, 縮めるか, UTF-8 の印)`）。
    pub(crate) fn make_zip(items: &[(&[u8], &[u8], bool, bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data, deflate, utf8) in items {
            let comp = if *deflate {
                miniz_oxide::deflate::compress_to_vec(data, 6)
            } else {
                data.to_vec()
            };
            let crc = crc32fast::hash(data);
            let off = out.len() as u32;
            let flags: u16 = if *utf8 { 0x0800 } else { 0 };
            let method: u16 = if *deflate { 8 } else { 0 };
            // 2024-05-06 07:08:10
            let time: u16 = (7 << 11) | (8 << 5) | 5;
            let date: u16 = ((2024 - 1980) << 9) | (5 << 5) | 6;
            out.extend_from_slice(&[0x50, 0x4b, 3, 4, 20, 0]);
            out.extend_from_slice(&flags.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&time.to_le_bytes());
            out.extend_from_slice(&date.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name);
            out.extend_from_slice(&comp);
            cd.extend_from_slice(&[0x50, 0x4b, 1, 2, 20, 0, 20, 0]);
            cd.extend_from_slice(&flags.to_le_bytes());
            cd.extend_from_slice(&method.to_le_bytes());
            cd.extend_from_slice(&time.to_le_bytes());
            cd.extend_from_slice(&date.to_le_bytes());
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 8]);
            cd.extend_from_slice(&0u32.to_le_bytes());
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name);
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(&[0x50, 0x4b, 5, 6, 0, 0, 0, 0]);
        out.extend_from_slice(&(items.len() as u16).to_le_bytes());
        out.extend_from_slice(&(items.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn tar_header(name: &str, size: u64, typ: u8) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        h[136..147].copy_from_slice(format!("{:011o}", 1_700_000_000u64).as_bytes());
        h[156] = typ;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u64 = h.iter().map(|&c| c as u64).sum();
        h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        h
    }

    pub(crate) fn make_tar(items: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut add = |name: &str, data: &[u8], typ: u8| {
            out.extend_from_slice(&tar_header(name, data.len() as u64, typ));
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        };
        for (name, data) in items {
            if name.len() > 99 {
                let mut n = name.as_bytes().to_vec();
                n.push(0);
                add("././@LongLink", &n, b'L');
                add(&name[..99], data, b'0');
            } else if name.ends_with('/') {
                add(name, &[], b'5');
            } else {
                add(name, data, b'0');
            }
        }
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    fn extract_all(a: &Archive, data: &[u8]) -> HashMap<String, Vec<u8>> {
        use std::cell::RefCell;
        use std::rc::Rc;
        let got: Rc<RefCell<HashMap<String, Vec<u8>>>> = Rc::default();
        struct W(Rc<RefCell<HashMap<String, Vec<u8>>>>, String);
        impl Write for W {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0
                    .borrow_mut()
                    .entry(self.1.clone())
                    .or_default()
                    .extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.0.borrow_mut().entry(self.1.clone()).or_default();
                Ok(())
            }
        }
        let all: Vec<usize> = (0..a.entries.len()).collect();
        let g2 = got.clone();
        a.extract_with(
            &mut Cursor::new(data),
            &all,
            &mut |i| {
                Ok(Some(
                    Box::new(W(g2.clone(), a.entries[i].path.clone())) as Box<dyn Write>
                ))
            },
            &no,
        )
        .unwrap();
        got.take()
    }

    #[test]
    fn zip_lists_and_extracts() {
        let big = "あいう".repeat(1000);
        let sjis = encoding_rs::SHIFT_JIS
            .encode("資料/見積.txt")
            .0
            .into_owned();
        let z = make_zip(&[
            (b"dir/", b"", false, false),
            ("文書/メモ.txt".as_bytes(), big.as_bytes(), true, true),
            (&sjis, b"sjis", false, false),
            (b"a/b/c.bin", b"\x00\x01", false, false),
        ]);
        let a = Archive::read(&mut Cursor::new(&z), Kind::Zip, "x.zip", &no).unwrap();
        let paths: Vec<&str> = a.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "a",
                "a/b",
                "a/b/c.bin",
                "dir",
                "文書",
                "文書/メモ.txt",
                "資料",
                "資料/見積.txt"
            ]
        );
        let memo = &a.entries[a.find("文書/メモ.txt").unwrap()];
        assert_eq!(memo.size, big.len() as u64);
        assert!(memo.packed.unwrap() < memo.size);
        assert_eq!(memo.method, "Deflate");
        assert!(memo.mtime_local);
        assert_eq!(
            memo.mtime,
            Some(days_from_civil(2024, 5, 6) * 86_400 + 7 * 3600 + 8 * 60 + 10)
        );
        assert!(a.entries[a.find("dir").unwrap()].dir);
        assert!(a.entries[a.find("a").unwrap()].implied);
        assert_eq!(a.children(""), vec![0, 3, 4, 6]);
        assert_eq!(a.under("a").len(), 2);
        let got = extract_all(&a, &z);
        assert_eq!(got["文書/メモ.txt"], big.as_bytes());
        assert_eq!(got["資料/見積.txt"], b"sjis");
        assert_eq!(got["a/b/c.bin"], b"\x00\x01");
        assert_eq!(
            a.read_entry(&mut Cursor::new(&z), a.find("資料/見積.txt").unwrap(), 100)
                .unwrap(),
            b"sjis"
        );
    }

    #[test]
    fn zip_detects_corruption_and_encryption() {
        let mut z = make_zip(&[(b"a.txt", b"hello world", false, false)]);
        // 中身を壊す（CRC が合わない）
        let at = 30 + 5;
        z[at] ^= 0xff;
        let a = Archive::read(&mut Cursor::new(&z), Kind::Zip, "x.zip", &no).unwrap();
        let e = a.read_entry(&mut Cursor::new(&z), 0, 100).unwrap_err();
        assert!(e.to_string().contains("CRC"), "{e}");
        // 暗号化の印
        let mut z = make_zip(&[(b"a.txt", b"hello", false, false)]);
        z[6] |= 1;
        let cd = z.len() - 22 - 46 - 5;
        z[cd + 8] |= 1;
        let a = Archive::read(&mut Cursor::new(&z), Kind::Zip, "x.zip", &no).unwrap();
        assert!(a.entries[0].encrypted);
        assert!(!a.entries[0].extractable);
        let e = a.read_entry(&mut Cursor::new(&z), 0, 100).unwrap_err();
        assert!(e.to_string().contains("暗号化"), "{e}");
        assert!(Archive::read(&mut Cursor::new(b"not a zip"), Kind::Zip, "x.zip", &no).is_err());
    }

    #[test]
    fn zip_with_prefix_and_zip64() {
        // 前に何か付いた ZIP（自己解凍の形）
        let mut z = b"MZ-stub".repeat(10);
        z.extend(make_zip(&[(b"x.txt", b"xyz", true, false)]));
        let a = Archive::read(&mut Cursor::new(&z), Kind::Zip, "x.zip", &no).unwrap();
        assert_eq!(a.read_entry(&mut Cursor::new(&z), 0, 10).unwrap(), b"xyz");
        // ZIP64: 目次の大きさ・位置を 0xFFFFFFFF にして、追加の欄と ZIP64 の終わりに書く
        let data = b"zip64 body";
        let crc = crc32fast::hash(data);
        let mut z = Vec::new();
        z.extend_from_slice(&[0x50, 0x4b, 3, 4, 45, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        z.extend_from_slice(&crc.to_le_bytes());
        z.extend_from_slice(&(data.len() as u32).to_le_bytes());
        z.extend_from_slice(&(data.len() as u32).to_le_bytes());
        z.extend_from_slice(&5u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(b"z.txt");
        z.extend_from_slice(data);
        let cd_off = z.len() as u64;
        let mut cd = vec![0x50, 0x4b, 1, 2, 45, 3, 45, 0, 0, 0, 0, 0, 0, 0, 0x21, 0];
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        cd.extend_from_slice(&5u16.to_le_bytes());
        cd.extend_from_slice(&28u16.to_le_bytes());
        cd.extend_from_slice(&[0; 6]);
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        cd.extend_from_slice(b"z.txt");
        cd.extend_from_slice(&1u16.to_le_bytes());
        cd.extend_from_slice(&24u16.to_le_bytes());
        cd.extend_from_slice(&(data.len() as u64).to_le_bytes());
        cd.extend_from_slice(&(data.len() as u64).to_le_bytes());
        cd.extend_from_slice(&0u64.to_le_bytes());
        z.extend_from_slice(&cd);
        let z64 = z.len() as u64;
        z.extend_from_slice(&[0x50, 0x4b, 6, 6]);
        z.extend_from_slice(&44u64.to_le_bytes());
        z.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&(cd.len() as u64).to_le_bytes());
        z.extend_from_slice(&cd_off.to_le_bytes());
        z.extend_from_slice(&[0x50, 0x4b, 6, 7, 0, 0, 0, 0]);
        z.extend_from_slice(&z64.to_le_bytes());
        z.extend_from_slice(&1u32.to_le_bytes());
        z.extend_from_slice(&[0x50, 0x4b, 5, 6, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        z.extend_from_slice(&[0xff; 8]);
        z.extend_from_slice(&0u16.to_le_bytes());
        let a = Archive::read(&mut Cursor::new(&z), Kind::Zip, "x.zip", &no).unwrap();
        assert_eq!(a.entries[0].path, "z.txt");
        assert_eq!(a.entries[0].size, data.len() as u64);
        assert_eq!(a.read_entry(&mut Cursor::new(&z), 0, 100).unwrap(), data);
    }

    /// 実際の道具で作った圧縮ファイル（`YYFILES_ARCHIVES` のフォルダの中の全部）を読めるか確かめる。
    #[test]
    #[ignore]
    fn real_archives() {
        let Some(dir) = std::env::var_os("YYFILES_ARCHIVES") else {
            return;
        };
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let a = Archive::open(&p, &no).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let mut f = File::open(&p).unwrap();
            let all: Vec<usize> = (0..a.entries.len()).collect();
            let mut sizes = 0u64;
            let n = a
                .extract_with(
                    &mut f,
                    &all,
                    &mut |_| Ok(Some(Box::new(io::sink()) as Box<dyn Write>)),
                    &no,
                )
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            for e in &a.entries {
                sizes += e.size;
                println!(
                    "{}\t{}\t{}\t{:?}\t{}",
                    p.display(),
                    e.path,
                    e.size,
                    e.mtime,
                    e.method
                );
            }
            assert_eq!(n, sizes, "{}", p.display());
        }
    }

    #[test]
    fn tar_lists_and_extracts_long_names() {
        let long = format!("{}/file.txt", "d".repeat(120));
        let t = make_tar(&[
            ("top/", b""),
            ("top/a.txt", b"alpha"),
            (&long, b"long name"),
            ("top/empty", b""),
        ]);
        let a = Archive::read(&mut Cursor::new(&t), Kind::Tar, "x.tar", &no).unwrap();
        let paths: Vec<&str> = a.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                &"d".repeat(120),
                long.as_str(),
                "top",
                "top/a.txt",
                "top/empty"
            ]
        );
        assert_eq!(a.entries[3].mtime, Some(1_700_000_000));
        let got = extract_all(&a, &t);
        assert_eq!(got["top/a.txt"], b"alpha");
        assert_eq!(got[&long], b"long name");
        assert_eq!(got["top/empty"], b"");
        // gzip で包む
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&t).unwrap();
        let gz = gz.finish().unwrap();
        let a = Archive::read(&mut Cursor::new(&gz), Kind::TarGz, "x.tgz", &no).unwrap();
        assert_eq!(a.entries.len(), 5);
        let got = extract_all(&a, &gz);
        assert_eq!(got["top/a.txt"], b"alpha");
        assert_eq!(got[&long], b"long name");
        // 1 つだけ
        let i = a.find("top/a.txt").unwrap();
        assert_eq!(
            a.read_entry(&mut Cursor::new(&gz), i, 100).unwrap(),
            b"alpha"
        );
        // xz で包む
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut Cursor::new(&t), &mut xz).unwrap();
        let a = Archive::read(&mut Cursor::new(&xz), Kind::TarXz, "x.tar.xz", &no).unwrap();
        let got = extract_all(&a, &xz);
        assert_eq!(got["top/a.txt"], b"alpha");
        // 壊れた tar
        let mut bad_t = t.clone();
        bad_t[600] ^= 1;
        bad_t[148] = b'7';
        assert!(Archive::read(&mut Cursor::new(&bad_t), Kind::Tar, "x.tar", &no).is_err());
    }

    #[test]
    fn tar_pax_path_and_size() {
        let mut out = Vec::new();
        let rec = |k: &str, v: &str| {
            let body = format!(" {k}={v}\n");
            let mut n = body.len() + 1;
            loop {
                let m = body.len() + n.to_string().len();
                if m == n {
                    break;
                }
                n = m;
            }
            format!("{n}{body}")
        };
        let pax = rec("path", "日本語/とても長い名前.txt");
        out.extend_from_slice(&tar_header("PaxHeaders/x", pax.len() as u64, b'x'));
        out.extend_from_slice(pax.as_bytes());
        out.resize(out.len().div_ceil(512) * 512, 0);
        out.extend_from_slice(&tar_header("short.txt", 3, b'0'));
        out.extend_from_slice(b"abc");
        out.resize(out.len().div_ceil(512) * 512, 0);
        out.extend_from_slice(&[0u8; 1024]);
        let a = Archive::read(&mut Cursor::new(&out), Kind::Tar, "x.tar", &no).unwrap();
        let i = a.find("日本語/とても長い名前.txt").expect("pax path");
        assert_eq!(a.read_entry(&mut Cursor::new(&out), i, 10).unwrap(), b"abc");
    }

    #[test]
    fn gz_and_xz_single_files() {
        let mut gz = flate2::GzBuilder::new()
            .filename("報告.csv".as_bytes())
            .write(Vec::new(), flate2::Compression::default());
        gz.write_all(b"a,b\n1,2\n").unwrap();
        let gz = gz.finish().unwrap();
        let a = Archive::read(&mut Cursor::new(&gz), Kind::Gz, "r.csv.gz", &no).unwrap();
        assert_eq!(a.entries[0].path, "報告.csv");
        assert_eq!(a.entries[0].size, 8);
        assert_eq!(
            a.read_entry(&mut Cursor::new(&gz), 0, 100).unwrap(),
            b"a,b\n1,2\n"
        );
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut Cursor::new(b"xz data"), &mut xz).unwrap();
        let a = Archive::read(&mut Cursor::new(&xz), Kind::Xz, "log.txt.xz", &no).unwrap();
        assert_eq!(a.entries[0].path, "log.txt");
        assert_eq!(a.entries[0].size, 7);
        assert_eq!(
            a.read_entry(&mut Cursor::new(&xz), 0, 100).unwrap(),
            b"xz data"
        );
    }

    /// 7z の数を書く。
    fn num7(out: &mut Vec<u8>, v: u64) {
        if v < 0x80 {
            out.push(v as u8);
        } else if v < 0x4000 {
            out.push(0x80 | (v >> 8) as u8);
            out.push(v as u8);
        } else {
            out.push(0xff);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn utf16z(s: &str) -> Vec<u8> {
        s.encode_utf16()
            .chain(Some(0))
            .flat_map(|u| u.to_le_bytes())
            .collect()
    }

    /// 試験用の 7z（無圧縮か LZMA の 1 つのソリッドのフォルダ、`encode` なら目次を LZMA で縮める）。
    pub(crate) fn make_7z(
        files: &[(&str, &[u8])],
        dirs: &[&str],
        lzma: bool,
        encode: bool,
    ) -> Vec<u8> {
        fn lzma_raw(data: &[u8]) -> (Vec<u8>, Vec<u8>) {
            let mut out = Vec::new();
            lzma_rs::lzma_compress(&mut Cursor::new(data), &mut out).unwrap();
            // .lzma の頭: 設定 5 バイト + 大きさ 8 バイト
            (out[..5].to_vec(), out[13..].to_vec())
        }
        fn folder(h: &mut Vec<u8>, lzma: Option<&[u8]>, unpack: u64) {
            h.push(0x0b);
            num7(h, 1);
            h.push(0);
            num7(h, 1); // coders
            match lzma {
                Some(props) => {
                    h.push(0x23); // id 3 バイト + 設定あり
                    h.extend_from_slice(&[3, 1, 1]);
                    num7(h, props.len() as u64);
                    h.extend_from_slice(props);
                }
                None => {
                    h.push(0x01);
                    h.push(0);
                }
            }
            h.push(0x0c);
            num7(h, unpack);
            h.push(0);
        }
        let mut body: Vec<u8> = Vec::new();
        for (_, d) in files {
            body.extend_from_slice(d);
        }
        let (props, packed) = if lzma {
            let (p, d) = lzma_raw(&body);
            (Some(p), d)
        } else {
            (None, body.clone())
        };
        let mut h = vec![0x01, 0x04];
        // PackInfo
        h.push(0x06);
        num7(&mut h, 0);
        num7(&mut h, 1);
        h.push(0x09);
        num7(&mut h, packed.len() as u64);
        h.push(0);
        // UnpackInfo
        h.push(0x07);
        folder(&mut h, props.as_deref(), body.len() as u64);
        // SubStreamsInfo
        h.push(0x08);
        h.push(0x0d);
        num7(&mut h, files.len() as u64);
        h.push(0x09);
        for (_, d) in &files[..files.len() - 1] {
            num7(&mut h, d.len() as u64);
        }
        h.push(0);
        h.push(0); // StreamsInfo の終わり
        // FilesInfo
        let n = files.len() + dirs.len();
        h.push(0x05);
        num7(&mut h, n as u64);
        if !dirs.is_empty() {
            h.push(0x0e);
            let mut bits = vec![0u8; n.div_ceil(8)];
            for k in files.len()..n {
                bits[k / 8] |= 0x80 >> (k % 8);
            }
            num7(&mut h, bits.len() as u64);
            h.extend_from_slice(&bits);
        }
        let mut names = vec![0u8];
        for (name, _) in files {
            names.extend(utf16z(name));
        }
        for d in dirs {
            names.extend(utf16z(d));
        }
        h.push(0x11);
        num7(&mut h, names.len() as u64);
        h.extend_from_slice(&names);
        h.push(0);
        h.push(0);
        let mut packs = packed;
        let header = if encode {
            let (p, d) = lzma_raw(&h);
            let mut e = vec![0x17, 0x06];
            num7(&mut e, packs.len() as u64);
            num7(&mut e, 1);
            e.push(0x09);
            num7(&mut e, d.len() as u64);
            e.push(0);
            e.push(0x07);
            folder(&mut e, Some(&p), h.len() as u64);
            e.push(0);
            packs.extend_from_slice(&d);
            e
        } else {
            h
        };
        let mut out = SEVEN_SIG.to_vec();
        out.extend_from_slice(&[0, 4]);
        let mut start = Vec::new();
        start.extend_from_slice(&(packs.len() as u64).to_le_bytes());
        start.extend_from_slice(&(header.len() as u64).to_le_bytes());
        start.extend_from_slice(&crc32fast::hash(&header).to_le_bytes());
        out.extend_from_slice(&crc32fast::hash(&start).to_le_bytes());
        out.extend_from_slice(&start);
        out.extend_from_slice(&packs);
        out.extend_from_slice(&header);
        out
    }

    #[test]
    fn seven_zip_lists_and_extracts() {
        for (lzma, encode) in [(false, false), (true, false), (true, true)] {
            let z = make_7z(
                &[
                    ("docs/a.txt", b"first file"),
                    ("docs/写真/b.bin", &[7u8; 3000]),
                    ("c.txt", b"third"),
                ],
                &["empty"],
                lzma,
                encode,
            );
            let a = Archive::read(&mut Cursor::new(&z), Kind::SevenZ, "x.7z", &no)
                .unwrap_or_else(|e| panic!("{lzma} {encode}: {e}"));
            let paths: Vec<&str> = a.entries.iter().map(|e| e.path.as_str()).collect();
            assert_eq!(
                paths,
                [
                    "c.txt",
                    "docs",
                    "docs/a.txt",
                    "docs/写真",
                    "docs/写真/b.bin",
                    "empty"
                ]
            );
            assert!(a.entries[5].dir && !a.entries[5].implied);
            assert_eq!(a.entries[4].size, 3000);
            assert_eq!(a.entries[0].method, if lzma { "LZMA" } else { "無圧縮" });
            let got = extract_all(&a, &z);
            assert_eq!(got["docs/a.txt"], b"first file");
            assert_eq!(got["docs/写真/b.bin"], vec![7u8; 3000]);
            assert_eq!(got["c.txt"], b"third");
            // 途中の 1 つだけ
            let i = a.find("docs/写真/b.bin").unwrap();
            assert_eq!(
                a.read_entry(&mut Cursor::new(&z), i, 5000).unwrap().len(),
                3000
            );
        }
        let mut z = make_7z(&[("a", b"x")], &[], false, false);
        let n = z.len();
        z[n - 3] ^= 1;
        let e = Archive::read(&mut Cursor::new(&z), Kind::SevenZ, "x.7z", &no).unwrap_err();
        assert!(e.to_string().contains("CRC"), "{e}");
    }

    #[test]
    fn extract_to_dir_refuses_escaping_names() {
        let dir = tempfile::tempdir().unwrap();
        let z = make_zip(&[
            (b"../evil.txt", b"x", false, false),
            (b"ok/x.txt", b"fine", false, false),
        ]);
        let zp = dir.path().join("a.zip");
        std::fs::write(&zp, &z).unwrap();
        let a = Archive::open(&zp, &no).unwrap();
        let out = dir.path().join("out");
        let ok = a.find("ok/x.txt").unwrap();
        assert_eq!(a.extract_to_dir(&zp, &[ok], &out, "ok", &no).unwrap(), 4);
        assert_eq!(std::fs::read(out.join("x.txt")).unwrap(), b"fine");
        let evil = a
            .entries
            .iter()
            .position(|e| e.path.contains(".."))
            .unwrap();
        assert!(a.extract_to_dir(&zp, &[evil], &out, "", &no).is_err());
        assert!(!dir.path().join("evil.txt").exists());
        assert!(dest_path(Path::new("x"), "a/../b").is_none());
        assert!(dest_path(Path::new("x"), "C:/b").is_none());
    }

    #[test]
    fn cancel_stops_listing() {
        let t = make_tar(&[("a", b"1")]);
        let e = Archive::read(&mut Cursor::new(&t), Kind::Tar, "x.tar", &|| true).unwrap_err();
        assert!(crate::is_cancelled(&e));
    }

    #[test]
    fn kinds_from_names() {
        assert_eq!(Kind::of("A.TAR.GZ"), Some(Kind::TarGz));
        assert_eq!(Kind::of("x.tgz"), Some(Kind::TarGz));
        assert_eq!(Kind::of("x.gz"), Some(Kind::Gz));
        assert_eq!(Kind::of("x.txz"), Some(Kind::TarXz));
        assert_eq!(Kind::of("x.7z"), Some(Kind::SevenZ));
        assert_eq!(Kind::of("x.zip"), Some(Kind::Zip));
        assert_eq!(Kind::of("x.docx"), None);
    }
}
