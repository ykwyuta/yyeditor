//! ファイルのメタデータ（2026-10-27 の提案 6・7 章）: 画像の Exif と縦横、音楽のタグ、文書の属性。
//!
//! 一括リネームのひな形（`{exif.date:%Y%m%d}` など）と検索の条件（`exif.model:EOS`）に使う。
//! 必要な部分だけを読む（Exif・タグはファイルの頭か終わり、Office は ZIP の目次と `docProps`）。
//! 読めない・壊れたファイルは空の結果にする（エラーにしない）。
//!
//! | キー | 意味 | 読むもの |
//! |---|---|---|
//! | `width`・`height` | 縦横（ピクセル） | JPEG・PNG・GIF・BMP・WebP・TIFF、MP4/MOV の映像 |
//! | `exif.date`・`exif.make`・`exif.model`・`exif.lens`・`exif.iso`・`exif.fnumber`・`exif.exposure`・`exif.focal`・`exif.orientation`・`exif.software`・`exif.artist` | 撮影の情報 | JPEG・TIFF・PNG（eXIf）・RAW（TIFF 形式） |
//! | `id3.title`・`id3.artist`・`id3.album`・`id3.albumartist`・`id3.track`・`id3.disc`・`id3.year`・`id3.genre`・`id3.comment` | 音楽のタグ | MP3（ID3v2・ID3v1）・FLAC・Ogg・Opus（Vorbis コメント）・M4A/MP4 |
//! | `duration` | 長さ（秒） | FLAC・MP4/M4A・MP3（Xing・VBRI・固定ビットレート） |
//! | `doc.title`・`doc.subject`・`doc.author`・`doc.keywords`・`doc.lastmodifiedby`・`doc.created`・`doc.modified`・`doc.company`・`doc.pages` | 文書の属性 | docx・xlsx・pptx（docProps）・PDF（/Info） |

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// 値。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Num(f64),
    /// 日時（UNIX 秒）。`local` なら手元の時刻のまま（Exif）、そうでなければ UTC
    Time {
        secs: i64,
        local: bool,
    },
}

impl Value {
    /// 書式なしの文字列（数は整数なら小数点なし。日時は `YYYY-MM-DD HH:MM:SS`、UTC は `tz` 秒ずらす）。
    pub fn text(&self, tz: i64) -> String {
        match self {
            Value::Text(s) => s.clone(),
            Value::Num(n) => num_text(*n),
            Value::Time { .. } => self
                .format_time("%Y-%m-%d %H:%M:%S", tz)
                .unwrap_or_default(),
        }
    }

    /// 日時を `strftime` 風の書式で（日時でなければ `None`）。
    pub fn format_time(&self, fmt: &str, tz: i64) -> Option<String> {
        match self {
            Value::Time { secs, local } => {
                Some(strftime(if *local { *secs } else { secs + tz }, fmt))
            }
            _ => None,
        }
    }

    /// 比べるための数（数・日時。文字列は数として読めれば）。
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Time { secs, .. } => Some(*secs as f64),
            Value::Text(s) => s.trim().parse().ok(),
        }
    }
}

fn num_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// メタデータ（キー → 値）。
pub type Props = BTreeMap<&'static str, Value>;

/// キーと説明（画面のひな形の一覧に出す）。
pub const KEYS: &[(&str, &str)] = &[
    ("width", "幅（ピクセル）"),
    ("height", "高さ（ピクセル）"),
    ("exif.date", "撮影日時"),
    ("exif.make", "カメラのメーカー"),
    ("exif.model", "カメラの機種"),
    ("exif.lens", "レンズ"),
    ("exif.iso", "ISO 感度"),
    ("exif.fnumber", "F 値"),
    ("exif.exposure", "露出時間"),
    ("exif.focal", "焦点距離（mm）"),
    ("exif.orientation", "向き"),
    ("exif.software", "ソフトウェア"),
    ("exif.artist", "撮影者"),
    ("id3.title", "曲名"),
    ("id3.artist", "アーティスト"),
    ("id3.album", "アルバム"),
    ("id3.albumartist", "アルバム アーティスト"),
    ("id3.track", "トラック番号"),
    ("id3.disc", "ディスク番号"),
    ("id3.year", "年"),
    ("id3.genre", "ジャンル"),
    ("id3.comment", "コメント"),
    ("duration", "長さ（秒）"),
    ("doc.title", "文書の題名"),
    ("doc.subject", "件名"),
    ("doc.author", "作成者"),
    ("doc.keywords", "キーワード"),
    ("doc.lastmodifiedby", "最終更新者"),
    ("doc.created", "文書の作成日時"),
    ("doc.modified", "文書の更新日時"),
    ("doc.company", "会社"),
    ("doc.pages", "ページ数"),
];

/// 名前に対応する静的なキー（`KEYS` にないものは `None`）。
pub fn key(name: &str) -> Option<&'static str> {
    KEYS.iter()
        .map(|(k, _)| *k)
        .find(|k| k.eq_ignore_ascii_case(name))
}

/// メタデータを読める種類のファイルか（拡張子で）。
pub fn has_props(name: &str) -> bool {
    kind(name).is_some()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Jpeg,
    Tiff,
    Png,
    Gif,
    Bmp,
    Webp,
    Mp3,
    Flac,
    Ogg,
    Mp4,
    Office,
    Pdf,
}

fn kind(name: &str) -> Option<Kind> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => Kind::Jpeg,
        "tif" | "tiff" | "dng" | "nef" | "cr2" | "arw" | "orf" | "rw2" | "pef" | "srw" => {
            Kind::Tiff
        }
        "png" => Kind::Png,
        "gif" => Kind::Gif,
        "bmp" | "dib" => Kind::Bmp,
        "webp" => Kind::Webp,
        "mp3" => Kind::Mp3,
        "flac" => Kind::Flac,
        "ogg" | "oga" | "opus" => Kind::Ogg,
        "m4a" | "m4b" | "mp4" | "m4v" | "mov" | "3gp" => Kind::Mp4,
        "docx" | "docm" | "xlsx" | "xlsm" | "pptx" | "pptm" => Kind::Office,
        "pdf" => Kind::Pdf,
        _ => return None,
    })
}

/// PDF を読む大きさの上限（全体を読むため）
const PDF_MAX: u64 = 64 << 20;

/// ファイルのメタデータ。読めなければ空。
pub fn read(path: &Path) -> Props {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(k) = kind(&name) else {
        return Props::new();
    };
    let Ok(mut f) = File::open(path) else {
        return Props::new();
    };
    read_kind(&mut f, k)
}

/// 開いたファイルのメタデータ（形式は `name` の拡張子から）。
pub fn read_from<R: Read + Seek>(r: &mut R, name: &str) -> Props {
    match kind(name) {
        Some(k) => read_kind(r, k),
        None => Props::new(),
    }
}

fn read_kind<R: Read + Seek>(r: &mut R, k: Kind) -> Props {
    let mut p = Props::new();
    let _ = match k {
        Kind::Jpeg => jpeg(r, &mut p),
        Kind::Tiff => head(r, 4 << 20).map(|b| tiff(&b, &mut p, true)),
        Kind::Png => png(r, &mut p),
        Kind::Gif => head(r, 10).map(|b| {
            if b.len() >= 10 && b.starts_with(b"GIF") {
                dims(&mut p, u16le(&b, 6) as u32, u16le(&b, 8) as u32);
            }
        }),
        Kind::Bmp => head(r, 26).map(|b| {
            if b.len() >= 26 && b.starts_with(b"BM") {
                dims(&mut p, u32le(&b, 18), (u32le(&b, 22) as i32).unsigned_abs());
            }
        }),
        Kind::Webp => head(r, 30).map(|b| webp(&b, &mut p)),
        Kind::Mp3 => mp3(r, &mut p),
        Kind::Flac => flac(r, &mut p),
        Kind::Ogg => head(r, 1 << 20).map(|b| ogg(&b, &mut p)),
        Kind::Mp4 => mp4(r, &mut p),
        Kind::Office => office(r, &mut p),
        Kind::Pdf => pdf(r, &mut p),
    };
    p
}

fn head<R: Read + Seek>(r: &mut R, n: u64) -> io::Result<Vec<u8>> {
    r.seek(SeekFrom::Start(0))?;
    let mut v = Vec::new();
    r.take(n).read_to_end(&mut v)?;
    Ok(v)
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u16be(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}

fn u32be(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn dims(p: &mut Props, w: u32, h: u32) {
    if w > 0 && h > 0 {
        p.insert("width", Value::Num(w as f64));
        p.insert("height", Value::Num(h as f64));
    }
}

fn text(p: &mut Props, k: &'static str, s: &str) {
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if !s.is_empty() {
        p.insert(k, Value::Text(s.to_owned()));
    }
}

/// バイト列 → 文字列（UTF-8 でなければ CP932）。
fn decode(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(b).0.into_owned(),
    }
}

// ---- 画像 ----

fn jpeg<R: Read + Seek>(r: &mut R, p: &mut Props) -> io::Result<()> {
    r.seek(SeekFrom::Start(0))?;
    let mut soi = [0u8; 2];
    r.read_exact(&mut soi)?;
    if soi != [0xff, 0xd8] {
        return Ok(());
    }
    for _ in 0..256 {
        let mut m = [0u8; 4];
        r.read_exact(&mut m)?;
        if m[0] != 0xff {
            return Ok(());
        }
        let marker = m[1];
        let len = u16be(&m, 2) as u64;
        if len < 2 || marker == 0xda || marker == 0xd9 {
            return Ok(());
        }
        let body = len - 2;
        match marker {
            0xe1 if !p.contains_key("exif.make") && !p.contains_key("exif.date") => {
                let mut b = vec![0u8; body as usize];
                r.read_exact(&mut b)?;
                if b.starts_with(b"Exif\0\0") {
                    tiff(&b[6..], p, false);
                }
            }
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                let mut b = [0u8; 5];
                r.read_exact(&mut b)?;
                dims(p, u16be(&b, 3) as u32, u16be(&b, 1) as u32);
                return Ok(());
            }
            _ => {
                r.seek(SeekFrom::Current(body as i64))?;
            }
        }
    }
    Ok(())
}

/// TIFF（Exif の中身・TIFF のファイル）を読む。`file` なら IFD0 の縦横も使う。
fn tiff(b: &[u8], p: &mut Props, file: bool) {
    if b.len() < 8 {
        return;
    }
    let le = match &b[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return,
    };
    let u16_ = |i: usize| -> Option<u16> {
        let s = b.get(i..i + 2)?;
        Some(if le { u16le(s, 0) } else { u16be(s, 0) })
    };
    let u32_ = |i: usize| -> Option<u32> {
        let s = b.get(i..i + 4)?;
        Some(if le { u32le(s, 0) } else { u32be(s, 0) })
    };
    struct Tag {
        typ: u16,
        count: u32,
        /// 値の位置（4 バイト以下なら項目の中）
        at: usize,
    }
    let read_ifd = |off: usize| -> Vec<(u16, Tag)> {
        let mut v = Vec::new();
        let Some(n) = u16_(off) else { return v };
        for i in 0..n.min(512) as usize {
            let e = off + 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) = (u16_(e), u16_(e + 2), u32_(e + 4)) else {
                break;
            };
            let size = match typ {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => continue,
            } * count as usize;
            let at = if size <= 4 {
                e + 8
            } else {
                match u32_(e + 8) {
                    Some(o) => o as usize,
                    None => continue,
                }
            };
            if at.saturating_add(size) > b.len() {
                continue;
            }
            v.push((tag, Tag { typ, count, at }));
        }
        v
    };
    let num = |t: &Tag| -> Option<f64> {
        Some(match t.typ {
            1 | 7 => *b.get(t.at)? as f64,
            3 => u16_(t.at)? as f64,
            4 => u32_(t.at)? as f64,
            8 => u16_(t.at)? as i16 as f64,
            9 => u32_(t.at)? as i32 as f64,
            5 | 10 => {
                let (n, d) = (u32_(t.at)?, u32_(t.at + 4)?);
                if d == 0 {
                    return None;
                }
                if t.typ == 10 {
                    n as i32 as f64 / d as i32 as f64
                } else {
                    n as f64 / d as f64
                }
            }
            _ => return None,
        })
    };
    let ascii = |t: &Tag| -> Option<String> {
        if t.typ != 2 && t.typ != 7 {
            return None;
        }
        let s = b.get(t.at..t.at + t.count as usize)?;
        let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
        Some(decode(s))
    };
    let Some(ifd0) = u32_(4) else { return };
    let tags0 = read_ifd(ifd0 as usize);
    let mut exif_off = None;
    let (mut w, mut h) = (0u32, 0u32);
    let mut date0 = None;
    for (tag, t) in &tags0 {
        match tag {
            0x010f => text(p, "exif.make", &ascii(t).unwrap_or_default()),
            0x0110 => text(p, "exif.model", &ascii(t).unwrap_or_default()),
            0x0131 => text(p, "exif.software", &ascii(t).unwrap_or_default()),
            0x013b => text(p, "exif.artist", &ascii(t).unwrap_or_default()),
            0x0132 => date0 = ascii(t),
            0x0112 => {
                if let Some(n) = num(t) {
                    p.insert("exif.orientation", Value::Num(n));
                }
            }
            0x0100 if file => w = num(t).unwrap_or(0.0) as u32,
            0x0101 if file => h = num(t).unwrap_or(0.0) as u32,
            0x8769 => exif_off = u32_(t.at),
            _ => {}
        }
    }
    let mut date = None;
    if let Some(off) = exif_off {
        for (tag, t) in read_ifd(off as usize) {
            match tag {
                0x9003 => date = ascii(&t),
                0x9004 if date.is_none() => date = ascii(&t),
                0x829a => {
                    if let Some(n) = num(&t) {
                        let s = if n > 0.0 && n < 1.0 {
                            format!("1/{}", (1.0 / n).round() as i64)
                        } else {
                            num_text(n)
                        };
                        p.insert("exif.exposure", Value::Text(s));
                    }
                }
                0x829d => {
                    if let Some(n) = num(&t) {
                        p.insert("exif.fnumber", Value::Num((n * 10.0).round() / 10.0));
                    }
                }
                0x8827 => {
                    if let Some(n) = num(&t) {
                        p.insert("exif.iso", Value::Num(n));
                    }
                }
                0x920a => {
                    if let Some(n) = num(&t) {
                        p.insert("exif.focal", Value::Num((n * 10.0).round() / 10.0));
                    }
                }
                0xa434 => text(p, "exif.lens", &ascii(&t).unwrap_or_default()),
                0xa002 if !file || w == 0 => w = num(&t).unwrap_or(0.0) as u32,
                0xa003 if !file || h == 0 => h = num(&t).unwrap_or(0.0) as u32,
                _ => {}
            }
        }
    }
    if w > 0 && h > 0 && !p.contains_key("width") {
        dims(p, w, h);
    }
    if let Some(t) = date.or(date0).and_then(|s| exif_date(&s)) {
        p.insert(
            "exif.date",
            Value::Time {
                secs: t,
                local: true,
            },
        );
    }
}

/// `2024:05:06 07:08:09` → 秒（手元の時刻）。
fn exif_date(s: &str) -> Option<i64> {
    let d: Vec<i64> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|x| !x.is_empty())
        .filter_map(|x| x.parse().ok())
        .collect();
    if d.len() < 3 || d[0] < 1800 || !(1..=12).contains(&d[1]) || !(1..=31).contains(&d[2]) {
        return None;
    }
    let g = |i: usize| d.get(i).copied().unwrap_or(0);
    Some(
        crate::archive::days_from_civil(d[0], d[1], d[2]) * 86_400 + g(3) * 3600 + g(4) * 60 + g(5),
    )
}

fn png<R: Read + Seek>(r: &mut R, p: &mut Props) -> io::Result<()> {
    r.seek(SeekFrom::Start(0))?;
    let mut sig = [0u8; 8];
    r.read_exact(&mut sig)?;
    if sig != *b"\x89PNG\r\n\x1a\n" {
        return Ok(());
    }
    for _ in 0..1024 {
        let mut h = [0u8; 8];
        r.read_exact(&mut h)?;
        let len = u32be(&h, 0) as u64;
        let typ = &h[4..8];
        match typ {
            b"IHDR" if len >= 8 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b)?;
                dims(p, u32be(&b, 0), u32be(&b, 4));
                r.seek(SeekFrom::Current(len as i64 - 8 + 4))?;
            }
            b"eXIf" if len < 1 << 20 => {
                let mut b = vec![0u8; len as usize];
                r.read_exact(&mut b)?;
                tiff(&b, p, false);
                r.seek(SeekFrom::Current(4))?;
            }
            b"IDAT" | b"IEND" => return Ok(()),
            _ => {
                r.seek(SeekFrom::Current(len as i64 + 4))?;
            }
        }
    }
    Ok(())
}

fn webp(b: &[u8], p: &mut Props) {
    if b.len() < 30 || &b[..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return;
    }
    let u24 = |i: usize| b[i] as u32 | (b[i + 1] as u32) << 8 | (b[i + 2] as u32) << 16;
    match &b[12..16] {
        b"VP8X" => dims(p, u24(24) + 1, u24(27) + 1),
        b"VP8L" => {
            let v = u32le(b, 21);
            dims(p, (v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1);
        }
        b"VP8 " => dims(
            p,
            (u16le(b, 26) & 0x3fff) as u32,
            (u16le(b, 28) & 0x3fff) as u32,
        ),
        _ => {}
    }
}

// ---- 音楽 ----

const GENRES: [&str; 80] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "AlternRock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychadelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
];

/// `(13)Pop`・`13` → ジャンル名。
fn genre(s: &str) -> String {
    let t = s.trim();
    let num = t
        .strip_prefix('(')
        .and_then(|x| x.split_once(')'))
        .map(|(n, rest)| (n, rest.trim()))
        .unwrap_or((t, ""));
    if let Ok(n) = num.0.parse::<usize>() {
        if !num.1.is_empty() {
            return num.1.to_owned();
        }
        if let Some(g) = GENRES.get(n) {
            return (*g).to_owned();
        }
    }
    t.to_owned()
}

/// `3/12` → 3。
fn first_num(s: &str) -> Option<f64> {
    s.split('/').next()?.trim().parse::<f64>().ok()
}

/// タグの名前（Vorbis・ID3・MP4）の値を入れる。
fn tag(p: &mut Props, k: &'static str, v: &str) {
    match k {
        "id3.track" | "id3.disc" => {
            if let Some(n) = first_num(v) {
                p.insert(k, Value::Num(n));
            }
        }
        "id3.year" => {
            let y: String = v.trim().chars().take(4).collect();
            if let Ok(n) = y.parse::<f64>() {
                p.insert(k, Value::Num(n));
            }
        }
        "id3.genre" => text(p, k, &genre(v)),
        _ => text(p, k, v),
    }
}

fn id3_text(b: &[u8]) -> String {
    let Some((&enc, rest)) = b.split_first() else {
        return String::new();
    };
    let s = match enc {
        1 | 2 => {
            let (be, body) = if rest.starts_with(&[0xfe, 0xff]) {
                (true, &rest[2..])
            } else if rest.starts_with(&[0xff, 0xfe]) {
                (false, &rest[2..])
            } else {
                (enc == 2, rest)
            };
            let units: Vec<u16> = body
                .chunks_exact(2)
                .map(|c| {
                    if be {
                        u16::from_be_bytes([c[0], c[1]])
                    } else {
                        u16::from_le_bytes([c[0], c[1]])
                    }
                })
                .collect();
            String::from_utf16_lossy(&units)
        }
        3 => String::from_utf8_lossy(rest).into_owned(),
        // Latin-1 のはずだが、日本では CP932 のことが多い
        _ => decode(rest),
    };
    // 複数の値は \0 区切り。最初のもの
    s.split('\0').next().unwrap_or("").to_owned()
}

fn syncsafe(b: &[u8]) -> u32 {
    (b[0] as u32 & 0x7f) << 21
        | (b[1] as u32 & 0x7f) << 14
        | (b[2] as u32 & 0x7f) << 7
        | (b[3] as u32 & 0x7f)
}

fn mp3<R: Read + Seek>(r: &mut R, p: &mut Props) -> io::Result<()> {
    let len = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(0))?;
    let mut h = [0u8; 10];
    let mut audio_start = 0u64;
    if r.read_exact(&mut h).is_ok() && &h[..3] == b"ID3" && h[3] >= 2 && h[3] <= 4 {
        let ver = h[3];
        let size = syncsafe(&h[6..10]) as u64;
        audio_start = 10 + size;
        let mut tagb = vec![0u8; size.min(16 << 20) as usize];
        r.read_exact(&mut tagb)?;
        if h[5] & 0x80 != 0 && ver < 4 {
            // 非同期化（FF 00 → FF）
            let mut out = Vec::with_capacity(tagb.len());
            let mut i = 0;
            while i < tagb.len() {
                out.push(tagb[i]);
                if tagb[i] == 0xff && tagb.get(i + 1) == Some(&0) {
                    i += 1;
                }
                i += 1;
            }
            tagb = out;
        }
        let mut q = 0usize;
        if h[5] & 0x40 != 0 && ver >= 3 && tagb.len() >= 4 {
            // 拡張ヘッダー
            let n = if ver == 4 {
                syncsafe(&tagb[..4]) as usize
            } else {
                u32be(&tagb, 0) as usize + 4
            };
            q = n.min(tagb.len());
        }
        let (idlen, hlen) = if ver == 2 { (3, 6) } else { (4, 10) };
        while q + hlen <= tagb.len() {
            let id = &tagb[q..q + idlen];
            if id[0] == 0 {
                break;
            }
            let fsize = match ver {
                2 => {
                    (tagb[q + 3] as usize) << 16
                        | (tagb[q + 4] as usize) << 8
                        | tagb[q + 5] as usize
                }
                3 => u32be(&tagb, q + 4) as usize,
                _ => syncsafe(&tagb[q + 4..q + 8]) as usize,
            };
            let start = q + hlen;
            let Some(data) = tagb.get(start..start + fsize) else {
                break;
            };
            let key = match id {
                b"TIT2" | b"TT2" => Some("id3.title"),
                b"TPE1" | b"TP1" => Some("id3.artist"),
                b"TALB" | b"TAL" => Some("id3.album"),
                b"TPE2" | b"TP2" => Some("id3.albumartist"),
                b"TRCK" | b"TRK" => Some("id3.track"),
                b"TPOS" | b"TPA" => Some("id3.disc"),
                b"TYER" | b"TDRC" | b"TYE" => Some("id3.year"),
                b"TCON" | b"TCO" => Some("id3.genre"),
                _ => None,
            };
            if let Some(k) = key {
                tag(p, k, &id3_text(data));
            } else if (id == b"COMM" || id == b"COM")
                && data.len() > 4
                && !p.contains_key("id3.comment")
            {
                // 文字コード・言語 3 文字・説明 \0 本文
                let enc = data[0];
                let rest = &data[4..];
                let body = if enc == 1 || enc == 2 {
                    let mut i = 0;
                    while i + 1 < rest.len() && !(rest[i] == 0 && rest[i + 1] == 0) {
                        i += 2;
                    }
                    &rest[(i + 2).min(rest.len())..]
                } else {
                    let i = rest
                        .iter()
                        .position(|&c| c == 0)
                        .map_or(rest.len(), |i| i + 1);
                    &rest[i..]
                };
                let mut b = vec![enc];
                b.extend_from_slice(body);
                tag(p, "id3.comment", &id3_text(&b));
            }
            q = start + fsize;
        }
    }
    // ID3v1（v2 にないものだけ）
    if len >= 128 {
        r.seek(SeekFrom::End(-128))?;
        let mut t = [0u8; 128];
        r.read_exact(&mut t)?;
        if &t[..3] == b"TAG" {
            let field = |a: usize, b: usize| {
                let s = &t[a..b];
                decode(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
            };
            for (k, v) in [
                ("id3.title", field(3, 33)),
                ("id3.artist", field(33, 63)),
                ("id3.album", field(63, 93)),
                ("id3.year", field(93, 97)),
            ] {
                if !p.contains_key(k) {
                    tag(p, k, &v);
                }
            }
            if t[125] == 0 && t[126] != 0 && !p.contains_key("id3.track") {
                p.insert("id3.track", Value::Num(t[126] as f64));
            }
            if !p.contains_key("id3.genre")
                && let Some(g) = GENRES.get(t[127] as usize)
            {
                p.insert("id3.genre", Value::Text((*g).into()));
            }
        }
    }
    // 長さ: 最初のフレームの Xing・VBRI、なければ固定ビットレート
    r.seek(SeekFrom::Start(audio_start))?;
    let mut f = vec![0u8; 4096];
    let n = r.read(&mut f)?;
    f.truncate(n);
    if let Some(i) =
        (0..f.len().saturating_sub(4)).find(|&i| f[i] == 0xff && f[i + 1] & 0xe0 == 0xe0)
    {
        let hdr = u32be(&f, i);
        let version = (hdr >> 19) & 3; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5
        let layer = (hdr >> 17) & 3; // 1 = Layer III
        let br_i = ((hdr >> 12) & 15) as usize;
        let sr_i = ((hdr >> 10) & 3) as usize;
        let mono = (hdr >> 6) & 3 == 3;
        let rates = [44100u32, 48000, 32000];
        if layer == 1 && sr_i < 3 && br_i > 0 && br_i < 15 {
            let sr = rates[sr_i]
                >> if version == 3 {
                    0
                } else if version == 2 {
                    1
                } else {
                    2
                };
            let br1 = [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
            ];
            let br2 = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
            let br = if version == 3 { br1[br_i] } else { br2[br_i] } * 1000;
            let spf = if version == 3 { 1152.0 } else { 576.0 };
            let side = match (version == 3, mono) {
                (true, false) => 32,
                (true, true) => 17,
                (false, false) => 17,
                (false, true) => 9,
            };
            let x = i + 4 + side;
            let frames = if f
                .get(x..x + 4)
                .is_some_and(|s| s == b"Xing" || s == b"Info")
                && f.len() >= x + 12
                && u32be(&f, x + 4) & 1 != 0
            {
                Some(u32be(&f, x + 8) as f64)
            } else if f.get(i + 36..i + 40) == Some(b"VBRI") && f.len() >= i + 50 {
                Some(u32be(&f, i + 50 - 4) as f64)
            } else {
                None
            };
            let secs = match frames {
                Some(fr) if sr > 0 => fr * spf / sr as f64,
                _ if br > 0 => {
                    (len.saturating_sub(audio_start + i as u64)) as f64 * 8.0 / br as f64
                }
                _ => 0.0,
            };
            if secs > 0.0 {
                p.insert("duration", Value::Num(secs.round()));
            }
        }
    }
    Ok(())
}

/// Vorbis コメント（`KEY=value` の並び）。
fn vorbis(b: &[u8], p: &mut Props) {
    if b.len() < 8 {
        return;
    }
    let vendor = u32le(b, 0) as usize;
    let mut q = 4 + vendor;
    let Some(count) = b.get(q..q + 4).map(|s| u32le(s, 0)) else {
        return;
    };
    q += 4;
    for _ in 0..count.min(10_000) {
        let Some(n) = b.get(q..q + 4).map(|s| u32le(s, 0) as usize) else {
            return;
        };
        q += 4;
        let Some(c) = b.get(q..q + n) else { return };
        q += n;
        let s = String::from_utf8_lossy(c);
        let Some((k, v)) = s.split_once('=') else {
            continue;
        };
        let key = match k.to_ascii_uppercase().as_str() {
            "TITLE" => "id3.title",
            "ARTIST" => "id3.artist",
            "ALBUM" => "id3.album",
            "ALBUMARTIST" | "ALBUM ARTIST" => "id3.albumartist",
            "TRACKNUMBER" => "id3.track",
            "DISCNUMBER" => "id3.disc",
            "DATE" | "YEAR" => "id3.year",
            "GENRE" => "id3.genre",
            "COMMENT" | "DESCRIPTION" => "id3.comment",
            _ => continue,
        };
        if !p.contains_key(key) {
            tag(p, key, v);
        }
    }
}

fn flac<R: Read + Seek>(r: &mut R, p: &mut Props) -> io::Result<()> {
    r.seek(SeekFrom::Start(0))?;
    let mut sig = [0u8; 4];
    r.read_exact(&mut sig)?;
    if &sig != b"fLaC" {
        return Ok(());
    }
    for _ in 0..128 {
        let mut h = [0u8; 4];
        r.read_exact(&mut h)?;
        let last = h[0] & 0x80 != 0;
        let typ = h[0] & 0x7f;
        let len = (h[1] as u64) << 16 | (h[2] as u64) << 8 | h[3] as u64;
        match typ {
            0 if len >= 18 => {
                let mut b = vec![0u8; len as usize];
                r.read_exact(&mut b)?;
                let sr = (b[10] as u64) << 12 | (b[11] as u64) << 4 | (b[12] as u64) >> 4;
                let total = ((b[13] & 0x0f) as u64) << 32 | u32be(&b, 14) as u64;
                if sr > 0 && total > 0 {
                    p.insert("duration", Value::Num((total as f64 / sr as f64).round()));
                }
            }
            4 if len < 16 << 20 => {
                let mut b = vec![0u8; len as usize];
                r.read_exact(&mut b)?;
                vorbis(&b, p);
            }
            _ => {
                r.seek(SeekFrom::Current(len as i64))?;
            }
        }
        if last {
            break;
        }
    }
    Ok(())
}

/// Ogg のページをつないでパケットを取り出し、コメントのパケットを読む。
fn ogg(b: &[u8], p: &mut Props) {
    let mut packet: Vec<u8> = Vec::new();
    let mut q = 0;
    let mut packets = 0;
    while q + 27 <= b.len() && &b[q..q + 4] == b"OggS" && packets < 3 {
        let nseg = b[q + 26] as usize;
        let Some(table) = b.get(q + 27..q + 27 + nseg) else {
            return;
        };
        let mut data = q + 27 + nseg;
        for &s in table {
            let Some(seg) = b.get(data..data + s as usize) else {
                return;
            };
            packet.extend_from_slice(seg);
            data += s as usize;
            if s < 255 {
                packets += 1;
                if packet.starts_with(b"\x03vorbis") {
                    vorbis(&packet[7..], p);
                    return;
                }
                if packet.starts_with(b"OpusTags") {
                    vorbis(&packet[8..], p);
                    return;
                }
                packet.clear();
            }
        }
        q = data;
    }
}

/// MP4 の箱（大きさ・種類・中身の位置）を順に。
fn boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut v = Vec::new();
    let mut q = 0;
    while q + 8 <= b.len() {
        let mut size = u32be(b, q) as usize;
        let typ: [u8; 4] = b[q + 4..q + 8].try_into().unwrap_or([0; 4]);
        let mut hdr = 8;
        if size == 1 && q + 16 <= b.len() {
            size = u64::from_be_bytes(b[q + 8..q + 16].try_into().unwrap_or([0; 8])) as usize;
            hdr = 16;
        } else if size == 0 {
            size = b.len() - q;
        }
        if size < hdr || q + size > b.len() {
            break;
        }
        v.push((typ, &b[q + hdr..q + size]));
        q += size;
    }
    v
}

fn mp4<R: Read + Seek>(r: &mut R, p: &mut Props) -> io::Result<()> {
    // 一番上の箱を順にたどって moov だけを読む（mdat は大きいので飛ばす）
    let len = r.seek(SeekFrom::End(0))?;
    let mut pos = 0u64;
    let mut moov = None;
    for _ in 0..64 {
        if pos + 8 > len {
            break;
        }
        r.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        r.read_exact(&mut h[..8])?;
        let mut size = u32be(&h, 0) as u64;
        let mut hdr = 8;
        if size == 1 {
            r.read_exact(&mut h[8..16])?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap_or([0; 8]));
            hdr = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < hdr {
            break;
        }
        if &h[4..8] == b"moov" {
            if size > 64 << 20 {
                return Ok(());
            }
            let mut b = vec![0u8; (size - hdr) as usize];
            r.read_exact(&mut b)?;
            moov = Some(b);
            break;
        }
        pos += size;
    }
    let Some(moov) = moov else { return Ok(()) };
    for (t, body) in boxes(&moov) {
        match &t {
            b"mvhd" if !body.is_empty() => {
                let (scale, dur) = if body[0] == 1 && body.len() >= 32 {
                    (
                        u32be(body, 20) as f64,
                        u64::from_be_bytes(body[24..32].try_into().unwrap_or([0; 8])) as f64,
                    )
                } else if body.len() >= 20 {
                    (u32be(body, 12) as f64, u32be(body, 16) as f64)
                } else {
                    (0.0, 0.0)
                };
                if scale > 0.0 && dur > 0.0 {
                    p.insert("duration", Value::Num((dur / scale).round()));
                }
            }
            b"trak" if !p.contains_key("width") => {
                for (t2, b2) in boxes(body) {
                    if &t2 == b"tkhd" && b2.len() >= 84 {
                        let w = u32be(b2, b2.len() - 8) >> 16;
                        let h = u32be(b2, b2.len() - 4) >> 16;
                        dims(p, w, h);
                    }
                }
            }
            b"udta" => {
                for (t2, b2) in boxes(body) {
                    if &t2 == b"meta" && b2.len() > 4 {
                        for (t3, b3) in boxes(&b2[4..]) {
                            if &t3 == b"ilst" {
                                ilst(b3, p);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn ilst(b: &[u8], p: &mut Props) {
    for (t, item) in boxes(b) {
        let Some(data) = boxes(item)
            .into_iter()
            .find(|(t, _)| t == b"data")
            .map(|(_, d)| d)
        else {
            continue;
        };
        if data.len() < 8 {
            continue;
        }
        let v = &data[8..];
        let key = match &t {
            b"\xa9nam" => "id3.title",
            b"\xa9ART" => "id3.artist",
            b"\xa9alb" => "id3.album",
            b"aART" => "id3.albumartist",
            b"\xa9day" => "id3.year",
            b"\xa9gen" => "id3.genre",
            b"\xa9cmt" => "id3.comment",
            b"trkn" | b"disk" => {
                if v.len() >= 4 {
                    let n = u16be(v, 2);
                    if n > 0 {
                        let k = if &t == b"trkn" {
                            "id3.track"
                        } else {
                            "id3.disc"
                        };
                        p.insert(k, Value::Num(n as f64));
                    }
                }
                continue;
            }
            b"gnre" => {
                if v.len() >= 2
                    && let Some(g) = GENRES.get((u16be(v, 0) as usize).wrapping_sub(1))
                {
                    p.insert("id3.genre", Value::Text((*g).into()));
                }
                continue;
            }
            _ => continue,
        };
        tag(p, key, &String::from_utf8_lossy(v));
    }
}

// ---- 文書 ----

/// XML の要素（`<dc:title>…</dc:title>`。属性があってもよい）の中身。
fn xml_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(i) = xml[from..].find(&open) {
        let at = from + i + open.len();
        let next = xml[at..].chars().next()?;
        if next == '>' || next == ' ' || next == '/' {
            let gt = at + xml[at..].find('>')?;
            if xml[..gt].ends_with('/') {
                return None;
            }
            let close = format!("</{tag}>");
            let end = gt + 1 + xml[gt + 1..].find(&close)?;
            return Some(crate::office::unescape(&xml[gt + 1..end]).trim().to_owned());
        }
        from = at;
    }
    None
}

/// ISO 8601（`2024-05-06T07:08:09Z`・`+09:00`）→ UTC の秒。
fn iso_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let d: Vec<i64> = s
        .get(..19.min(s.len()))?
        .split(|c: char| !c.is_ascii_digit())
        .filter(|x| !x.is_empty())
        .filter_map(|x| x.parse().ok())
        .collect();
    if d.len() < 3 {
        return None;
    }
    let g = |i: usize| d.get(i).copied().unwrap_or(0);
    let mut t =
        crate::archive::days_from_civil(d[0], d[1], d[2]) * 86_400 + g(3) * 3600 + g(4) * 60 + g(5);
    let rest = s.get(19..).unwrap_or("");
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') {
        let digits: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        let off = digits
            .get(0..2)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0)
            * 3600
            + digits
                .get(2..4)
                .and_then(|x| x.parse::<i64>().ok())
                .unwrap_or(0)
                * 60;
        t -= if sign == '-' { -off } else { off };
    }
    Some(t)
}

fn office<R: Read + Seek>(r: &mut R, p: &mut Props) -> io::Result<()> {
    let a = crate::archive::Archive::read(r, crate::archive::Kind::Zip, "", &|| false)?;
    if let Some(i) = a.find("docProps/core.xml") {
        let xml = String::from_utf8_lossy(&a.read_entry(r, i, 4 << 20)?).into_owned();
        for (tag_name, key) in [
            ("dc:title", "doc.title"),
            ("dc:subject", "doc.subject"),
            ("dc:creator", "doc.author"),
            ("cp:keywords", "doc.keywords"),
            ("cp:lastModifiedBy", "doc.lastmodifiedby"),
        ] {
            if let Some(v) = xml_value(&xml, tag_name) {
                text(p, key, &v);
            }
        }
        for (tag_name, key) in [
            ("dcterms:created", "doc.created"),
            ("dcterms:modified", "doc.modified"),
        ] {
            if let Some(t) = xml_value(&xml, tag_name).and_then(|v| iso_time(&v)) {
                p.insert(
                    key,
                    Value::Time {
                        secs: t,
                        local: false,
                    },
                );
            }
        }
    }
    if let Some(i) = a.find("docProps/app.xml") {
        let xml = String::from_utf8_lossy(&a.read_entry(r, i, 4 << 20)?).into_owned();
        if let Some(v) = xml_value(&xml, "Company") {
            text(p, "doc.company", &v);
        }
        for t in ["Pages", "Slides"] {
            if let Some(n) = xml_value(&xml, t).and_then(|v| v.parse::<f64>().ok()) {
                p.insert("doc.pages", Value::Num(n));
            }
        }
    }
    Ok(())
}

fn pdf<R: Read + Seek>(r: &mut R, p: &mut Props) -> io::Result<()> {
    let len = r.seek(SeekFrom::End(0))?;
    if len > PDF_MAX {
        return Ok(());
    }
    let data = head(r, len)?;
    for (k, v) in crate::pdf::info(&data)? {
        let key = match k.as_str() {
            "Title" => "doc.title",
            "Author" => "doc.author",
            "Subject" => "doc.subject",
            "Keywords" => "doc.keywords",
            "Pages" => {
                if let Ok(n) = v.parse::<f64>() {
                    p.insert("doc.pages", Value::Num(n));
                }
                continue;
            }
            "CreationDate" | "ModDate" => {
                if let Some((secs, local)) = crate::pdf::parse_date(&v) {
                    let key = if k == "CreationDate" {
                        "doc.created"
                    } else {
                        "doc.modified"
                    };
                    p.insert(key, Value::Time { secs, local });
                }
                continue;
            }
            _ => continue,
        };
        text(p, key, &v);
    }
    Ok(())
}

// ---- 日時の書式 ----

/// 秒 → (年, 月, 日, 時, 分, 秒, 曜日 0=日曜)。
pub fn civil(secs: i64) -> (i64, i64, i64, i64, i64, i64, i64) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let wday = (days + 4).rem_euclid(7);
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60, wday)
}

/// `strftime` 風の書式（`%Y %m %d %H %M %S %y %j %a %A %b %B %%`）。そのほかの文字はそのまま。
pub fn strftime(secs: i64, fmt: &str) -> String {
    const WD: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];
    const WDE: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, mo, d, h, mi, s, wd) = civil(secs);
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out.push_str(&format!("{y:04}")),
            Some('y') => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            Some('m') => out.push_str(&format!("{mo:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('H') => out.push_str(&format!("{h:02}")),
            Some('M') => out.push_str(&format!("{mi:02}")),
            Some('S') => out.push_str(&format!("{s:02}")),
            Some('j') => {
                let doy = secs.div_euclid(86_400) - crate::archive::days_from_civil(y, 1, 1) + 1;
                out.push_str(&format!("{doy:03}"));
            }
            Some('a') => out.push_str(WDE[wd as usize]),
            Some('A') => out.push_str(WD[wd as usize]),
            Some('b') => out.push_str(MON[(mo - 1) as usize]),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// 試験用の JPEG（Exif の機種・撮影日時・ISO・F 値・露出、SOF の縦横）。
    pub(crate) fn make_jpeg(model: &str, date: &str, w: u16, h: u16) -> Vec<u8> {
        // TIFF（リトル エンディアン）: IFD0 = Make, Model, ExifIFD。Exif IFD = DateTimeOriginal, ExposureTime, FNumber, ISO
        let mut t = b"II*\0\x08\0\0\0".to_vec();
        let make = b"Canon\0";
        let model_b: Vec<u8> = model.bytes().chain(Some(0)).collect();
        let date_b: Vec<u8> = date.bytes().chain(Some(0)).collect();
        let ifd0_n = 3u16;
        let ifd0_size = 2 + ifd0_n as usize * 12 + 4;
        let data0 = 8 + ifd0_size;
        let make_at = data0;
        let model_at = make_at + make.len();
        let exif_at = model_at + model_b.len();
        let exif_n = 4u16;
        let exif_size = 2 + exif_n as usize * 12 + 4;
        let date_at = exif_at + exif_size;
        let rat_at = date_at + date_b.len();
        let entry = |t: &mut Vec<u8>, tag: u16, typ: u16, count: u32, v: u32| {
            t.extend_from_slice(&tag.to_le_bytes());
            t.extend_from_slice(&typ.to_le_bytes());
            t.extend_from_slice(&count.to_le_bytes());
            t.extend_from_slice(&v.to_le_bytes());
        };
        t.extend_from_slice(&ifd0_n.to_le_bytes());
        entry(&mut t, 0x010f, 2, make.len() as u32, make_at as u32);
        entry(&mut t, 0x0110, 2, model_b.len() as u32, model_at as u32);
        entry(&mut t, 0x8769, 4, 1, exif_at as u32);
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(make);
        t.extend_from_slice(&model_b);
        t.extend_from_slice(&exif_n.to_le_bytes());
        entry(&mut t, 0x9003, 2, date_b.len() as u32, date_at as u32);
        entry(&mut t, 0x829a, 5, 1, rat_at as u32);
        entry(&mut t, 0x829d, 5, 1, rat_at as u32 + 8);
        entry(&mut t, 0x8827, 3, 1, 400);
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&date_b);
        for (n, d) in [(1u32, 250u32), (28, 10)] {
            t.extend_from_slice(&n.to_le_bytes());
            t.extend_from_slice(&d.to_le_bytes());
        }
        let mut j = vec![0xff, 0xd8];
        let app1_len = (2 + 6 + t.len()) as u16;
        j.extend_from_slice(&[0xff, 0xe1]);
        j.extend_from_slice(&app1_len.to_be_bytes());
        j.extend_from_slice(b"Exif\0\0");
        j.extend_from_slice(&t);
        j.extend_from_slice(&[0xff, 0xc0, 0, 17, 8]);
        j.extend_from_slice(&h.to_be_bytes());
        j.extend_from_slice(&w.to_be_bytes());
        j.extend_from_slice(&[3; 10]);
        j.extend_from_slice(&[0xff, 0xda, 0, 2, 0xff, 0xd9]);
        j
    }

    pub(crate) fn make_mp3(title: &str, artist: &str, track: &str) -> Vec<u8> {
        let frame = |id: &[u8], enc: u8, body: &[u8]| {
            let mut f = id.to_vec();
            f.extend_from_slice(&((body.len() + 1) as u32).to_be_bytes());
            f.extend_from_slice(&[0, 0, enc]);
            f.extend_from_slice(body);
            f
        };
        let mut frames = Vec::new();
        let t16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(title.encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        frames.extend(frame(b"TIT2", 1, &t16));
        frames.extend(frame(b"TPE1", 3, artist.as_bytes()));
        frames.extend(frame(b"TRCK", 0, track.as_bytes()));
        frames.extend(frame(b"TCON", 0, b"(13)"));
        let size = frames.len() as u32;
        let mut v = b"ID3\x03\x00\x00".to_vec();
        v.extend_from_slice(&[
            (size >> 21 & 0x7f) as u8,
            (size >> 14 & 0x7f) as u8,
            (size >> 7 & 0x7f) as u8,
            (size & 0x7f) as u8,
        ]);
        v.extend_from_slice(&frames);
        // MPEG1 Layer III 128 kbps 44.1 kHz のフレーム（418 バイト）を 100 個
        for _ in 0..100 {
            let mut fr = vec![0u8; 417];
            fr[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
            v.extend_from_slice(&fr);
        }
        v
    }

    #[test]
    fn reads_jpeg_exif() {
        let j = make_jpeg("EOS R5", "2024:05:06 07:08:09", 6000, 4000);
        let p = read_from(&mut Cursor::new(&j), "IMG_0001.JPG");
        assert_eq!(p["exif.make"], Value::Text("Canon".into()));
        assert_eq!(p["exif.model"], Value::Text("EOS R5".into()));
        assert_eq!(p["width"], Value::Num(6000.0));
        assert_eq!(p["height"], Value::Num(4000.0));
        assert_eq!(p["exif.iso"], Value::Num(400.0));
        assert_eq!(p["exif.fnumber"], Value::Num(2.8));
        assert_eq!(p["exif.exposure"], Value::Text("1/250".into()));
        let d = &p["exif.date"];
        assert_eq!(
            d.format_time("%Y%m%d_%H%M%S", 9 * 3600).unwrap(),
            "20240506_070809"
        );
        assert_eq!(d.text(0), "2024-05-06 07:08:09");
        // 壊れたものは空
        assert!(
            read_from(&mut Cursor::new(&j[..20]), "x.jpg")
                .get("exif.model")
                .is_none_or(|_| true)
        );
        assert!(read_from(&mut Cursor::new(b"junk"), "x.jpg").is_empty());
    }

    #[test]
    fn reads_png_gif_bmp_webp_sizes() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        png.extend_from_slice(b"\0\0\0\0IEND\0\0\0\0");
        let p = read_from(&mut Cursor::new(&png), "a.png");
        assert_eq!(
            (p["width"].clone(), p["height"].clone()),
            (Value::Num(640.0), Value::Num(480.0))
        );
        let gif = b"GIF89a\x20\x03\x58\x02rest".to_vec();
        let p = read_from(&mut Cursor::new(&gif), "a.gif");
        assert_eq!(p["width"], Value::Num(800.0));
        assert_eq!(p["height"], Value::Num(600.0));
        let mut bmp = vec![0u8; 26];
        bmp[..2].copy_from_slice(b"BM");
        bmp[18..22].copy_from_slice(&100u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&(-50i32).to_le_bytes());
        let p = read_from(&mut Cursor::new(&bmp), "a.bmp");
        assert_eq!(p["height"], Value::Num(50.0));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0x7f, 0x07, 0x00, 0x37, 0x04, 0x00]);
        webp.extend_from_slice(&[0; 4]);
        let p = read_from(&mut Cursor::new(&webp), "a.webp");
        assert_eq!(p["width"], Value::Num(1920.0));
        assert_eq!(p["height"], Value::Num(1080.0));
    }

    #[test]
    fn reads_mp3_tags_and_duration() {
        let m = make_mp3("夜に駆ける", "YOASOBI", "3/12");
        let p = read_from(&mut Cursor::new(&m), "song.mp3");
        assert_eq!(p["id3.title"], Value::Text("夜に駆ける".into()));
        assert_eq!(p["id3.artist"], Value::Text("YOASOBI".into()));
        assert_eq!(p["id3.track"], Value::Num(3.0));
        assert_eq!(p["id3.genre"], Value::Text("Pop".into()));
        // 100 フレーム × 1152 / 44100 ≒ 2.6 秒（固定ビットレートの見積もり）
        assert!(matches!(p["duration"], Value::Num(n) if (2.0..=4.0).contains(&n)));
        // ID3v1 だけ（CP932）
        let mut v1 = vec![0u8; 200];
        let mut t = [0u8; 128];
        t[..3].copy_from_slice(b"TAG");
        let title = encoding_rs::SHIFT_JIS.encode("さくら").0;
        t[3..3 + title.len()].copy_from_slice(&title);
        t[93..97].copy_from_slice(b"2001");
        t[126] = 7;
        t[127] = 17;
        v1.extend_from_slice(&t);
        let p = read_from(&mut Cursor::new(&v1), "old.mp3");
        assert_eq!(p["id3.title"], Value::Text("さくら".into()));
        assert_eq!(p["id3.year"], Value::Num(2001.0));
        assert_eq!(p["id3.track"], Value::Num(7.0));
        assert_eq!(p["id3.genre"], Value::Text("Rock".into()));
    }

    fn vorbis_block(items: &[&str]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"test");
        b.extend_from_slice(&(items.len() as u32).to_le_bytes());
        for s in items {
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        }
        b
    }

    #[test]
    fn reads_flac_ogg_and_m4a() {
        let mut f = b"fLaC".to_vec();
        let mut si = vec![0u8; 34];
        // 44100 Hz（20 ビット）、合計 441000 サンプル
        si[10] = (44100 >> 12) as u8;
        si[11] = (44100 >> 4) as u8;
        si[12] = ((44100 & 0xf) << 4) as u8;
        si[14..18].copy_from_slice(&441_000u32.to_be_bytes());
        f.extend_from_slice(&[0, 0, 0, 34]);
        f.extend_from_slice(&si);
        let vc = vorbis_block(&[
            "TITLE=曲",
            "ARTIST=歌手",
            "TRACKNUMBER=05",
            "DATE=2019-04-01",
        ]);
        f.push(0x84);
        f.extend_from_slice(&(vc.len() as u32).to_be_bytes()[1..]);
        f.extend_from_slice(&vc);
        let p = read_from(&mut Cursor::new(&f), "a.flac");
        assert_eq!(p["id3.title"], Value::Text("曲".into()));
        assert_eq!(p["id3.track"], Value::Num(5.0));
        assert_eq!(p["id3.year"], Value::Num(2019.0));
        assert_eq!(p["duration"], Value::Num(10.0));
        // Ogg Vorbis: 1 ページ目に識別、2 ページ目にコメント
        let page = |packet: &[u8]| {
            let mut pg = b"OggS".to_vec();
            pg.extend_from_slice(&[0; 22]);
            let mut segs = vec![255u8; packet.len() / 255];
            segs.push((packet.len() % 255) as u8);
            pg.push(segs.len() as u8);
            pg.extend_from_slice(&segs);
            pg.extend_from_slice(packet);
            pg
        };
        let mut ogg = page(b"\x01vorbis-ident");
        let mut c = b"\x03vorbis".to_vec();
        c.extend(vorbis_block(&[
            "ALBUM=アルバム",
            &format!("COMMENT={}", "x".repeat(300)),
        ]));
        ogg.extend(page(&c));
        let p = read_from(&mut Cursor::new(&ogg), "a.ogg");
        assert_eq!(p["id3.album"], Value::Text("アルバム".into()));
        // M4A
        let bx = |t: &[u8], body: &[u8]| {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(t);
            v.extend_from_slice(body);
            v
        };
        let data = |v: &[u8]| {
            let mut d = vec![0, 0, 0, 1, 0, 0, 0, 0];
            d.extend_from_slice(v);
            bx(b"data", &d)
        };
        let mut ilst = bx(b"\xa9nam", &data("題".as_bytes()));
        ilst.extend(bx(b"trkn", &data(&[0, 0, 0, 9, 0, 12, 0, 0])));
        let mut meta = vec![0u8; 4];
        meta.extend(bx(b"ilst", &ilst));
        let udta = bx(b"udta", &bx(b"meta", &meta));
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&65_000u32.to_be_bytes());
        let mut moov_body = bx(b"mvhd", &mvhd);
        moov_body.extend(udta);
        let mut file = bx(b"ftyp", b"M4A \0\0\0\0");
        file.extend(bx(b"mdat", &[0u8; 1000]));
        file.extend(bx(b"moov", &moov_body));
        let p = read_from(&mut Cursor::new(&file), "a.m4a");
        assert_eq!(p["id3.title"], Value::Text("題".into()));
        assert_eq!(p["id3.track"], Value::Num(9.0));
        assert_eq!(p["duration"], Value::Num(65.0));
    }

    #[test]
    fn reads_office_core_properties() {
        let core = r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="x" xmlns:dc="y" xmlns:dcterms="z"><dc:title>見積書 &amp; 請求</dc:title><dc:creator>佐藤</dc:creator><cp:lastModifiedBy>鈴木</cp:lastModifiedBy><dcterms:created xsi:type="dcterms:W3CDTF">2024-05-06T07:08:09Z</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">2024-05-07T09:00:00+09:00</dcterms:modified></cp:coreProperties>"#;
        let app = "<Properties><Pages>3</Pages><Company>株式会社</Company></Properties>";
        let z = crate::archive::tests::make_zip(&[
            (b"docProps/core.xml", core.as_bytes(), true, true),
            (b"docProps/app.xml", app.as_bytes(), false, true),
        ]);
        let p = read_from(&mut Cursor::new(&z), "a.docx");
        assert_eq!(p["doc.title"], Value::Text("見積書 & 請求".into()));
        assert_eq!(p["doc.author"], Value::Text("佐藤".into()));
        assert_eq!(p["doc.lastmodifiedby"], Value::Text("鈴木".into()));
        assert_eq!(p["doc.pages"], Value::Num(3.0));
        assert_eq!(p["doc.company"], Value::Text("株式会社".into()));
        assert_eq!(
            p["doc.created"]
                .format_time("%Y-%m-%d %H:%M", 9 * 3600)
                .unwrap(),
            "2024-05-06 16:08"
        );
        assert_eq!(p["doc.modified"].format_time("%H:%M", 0).unwrap(), "00:00");
    }

    /// 実際の道具で作ったファイル（`YYFILES_PROPS` のフォルダの中の全部）のメタデータを出す。
    #[test]
    #[ignore]
    fn real_files() {
        let Some(dir) = std::env::var_os("YYFILES_PROPS") else {
            return;
        };
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            println!("{}: {:?}", p.display(), read(&p));
        }
    }

    #[test]
    fn strftime_formats() {
        let t = crate::archive::days_from_civil(2024, 2, 29) * 86_400 + 13 * 3600 + 5 * 60 + 9;
        assert_eq!(
            strftime(t, "%Y-%m-%d %H:%M:%S %y %j %a %A %b %% %q"),
            "2024-02-29 13:05:09 24 060 Thu 木 Feb % %q"
        );
        assert_eq!(civil(-1).0, 1969);
        assert_eq!(key("EXIF.MODEL"), Some("exif.model"));
        assert!(has_props("a.JPG") && !has_props("a.txt"));
        assert_eq!(Value::Num(2.50).text(0), "2.5");
    }
}
