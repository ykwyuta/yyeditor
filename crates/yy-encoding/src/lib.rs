//! 文字コード（03 章）。
//!
//! 文書の内容は UTF-8 で持ち、UTF-8 以外のファイルは開くときにデコード、保存するときに
//! エンコードする。どの文字コードでも「開く→保存」でバイト列が変わらないように、
//! 文字コードとして不正なバイトは私用領域の文字 `U+10FE00 + バイト値`（エスケープ文字）に
//! 変換して保持し、同じ文字コードで保存するときに元のバイトへ戻す（03 章 2.4）。
//!
//! 例外（意味は同じだがバイト列が正規化されるもの）:
//! * 同じ文字に複数の符号がある文字コード（CP932 の NEC 選定 IBM 拡張文字など）で、
//!   優先されない側の符号。デコード時に [`DecodeStats::noncanonical`] として数える
//! * ISO-2022-JP のエスケープシーケンスの冗長な出し方

mod dbcs;
mod detect;
mod ebcdic;
mod fold;
mod mapfile;
mod tables;
mod unicode;
mod web;

use std::fmt;
use std::ops::Range;

pub use detect::{Detected, detect, detect_ebcdic};
pub use ebcdic::{Ccsid, Records};
pub use fold::fold_compat;
pub use mapfile::{
    Mapping, load_dir as load_mappings, mappings, parse as parse_mapping,
    register as register_mapping,
};

/// エスケープ文字の先頭（`U+10FE00`〜`U+10FEFF` が不正なバイト 0x00〜0xFF を表す）。
pub const ESCAPE_BASE: u32 = 0x10FE00;

/// 不正なバイト `b` を表すエスケープ文字。
pub fn escape_char(b: u8) -> char {
    char::from_u32(ESCAPE_BASE + b as u32).unwrap()
}

/// エスケープ文字なら元のバイトを返す。
pub fn unescape_char(c: char) -> Option<u8> {
    let v = c as u32;
    (ESCAPE_BASE..ESCAPE_BASE + 0x100)
        .contains(&v)
        .then(|| (v - ESCAPE_BASE) as u8)
}

/// エスケープ文字の UTF-8 表現（4 バイト）の先頭 3 バイトか。
/// エスケープ文字の UTF-8 は `F4 8F B8..BB xx`。
pub fn is_escape_prefix(b: &[u8]) -> bool {
    b.len() >= 3 && b[0] == 0xF4 && b[1] == 0x8F && (0xB8..=0xBB).contains(&b[2])
}

/// 対応している文字コード。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
    /// JIS X 0208 に準拠した Shift_JIS（0x8160 = U+301C 波ダッシュなど）
    ShiftJis,
    /// Windows の Shift_JIS（Windows-31J）。NEC 特殊文字・IBM 拡張文字を含む
    Cp932,
    /// JIS X 0213:2004 の Shift_JIS
    ShiftJis2004,
    EucJp,
    /// JIS X 0213:2004 の EUC-JP
    EucJis2004,
    /// ISO-2022-JP（RFC 1468、JIS X 0201 カナを含む）
    Iso2022Jp,
    /// EBCDIC（メインフレーム）。レコード（行）の区切り方を含む
    Ebcdic(Ccsid, Records),
    /// 外部の対応表（`.map` ファイル）。EBCDIC が土台ならレコードの区切り方を使う
    Custom(&'static Mapping, Records),
    /// `encoding_rs` が対応するその他の文字コード（欧州・中国語・韓国語など）
    Web(&'static encoding_rs::Encoding),
}

impl fmt::Debug for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// UI の一覧に出す `encoding_rs` の文字コード。
static WEB_ENCODINGS: [&encoding_rs::Encoding; 22] = [
    &encoding_rs::WINDOWS_1252_INIT,
    &encoding_rs::ISO_8859_2_INIT,
    &encoding_rs::ISO_8859_5_INIT,
    &encoding_rs::ISO_8859_7_INIT,
    &encoding_rs::ISO_8859_15_INIT,
    &encoding_rs::WINDOWS_1250_INIT,
    &encoding_rs::WINDOWS_1251_INIT,
    &encoding_rs::WINDOWS_1253_INIT,
    &encoding_rs::WINDOWS_1254_INIT,
    &encoding_rs::WINDOWS_1255_INIT,
    &encoding_rs::WINDOWS_1256_INIT,
    &encoding_rs::WINDOWS_1257_INIT,
    &encoding_rs::WINDOWS_1258_INIT,
    &encoding_rs::WINDOWS_874_INIT,
    &encoding_rs::KOI8_R_INIT,
    &encoding_rs::KOI8_U_INIT,
    &encoding_rs::IBM866_INIT,
    &encoding_rs::GBK_INIT,
    &encoding_rs::GB18030_INIT,
    &encoding_rs::BIG5_INIT,
    &encoding_rs::EUC_KR_INIT,
    &encoding_rs::MACINTOSH_INIT,
];

impl Encoding {
    /// UI に表示する名前（[`Encoding::from_name`] で元に戻せる）。
    pub fn name(&self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16LE",
            Encoding::Utf16Be => "UTF-16BE",
            Encoding::Utf32Le => "UTF-32LE",
            Encoding::Utf32Be => "UTF-32BE",
            Encoding::ShiftJis => "Shift_JIS",
            Encoding::Cp932 => "CP932",
            Encoding::ShiftJis2004 => "Shift_JIS-2004",
            Encoding::EucJp => "EUC-JP",
            Encoding::EucJis2004 => "EUC-JIS-2004",
            Encoding::Iso2022Jp => "ISO-2022-JP",
            Encoding::Ebcdic(c, _) => c.name(),
            Encoding::Custom(m, _) => m.name(),
            Encoding::Web(e) => e.name(),
        }
    }

    /// 一覧に表示する説明付きの名前。
    pub fn label(&self) -> String {
        if let Encoding::Custom(m, _) = self {
            return format!("{} ({})", m.name(), m.description());
        }
        let desc = match self {
            Encoding::Utf8 => "Unicode",
            Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Utf32Le | Encoding::Utf32Be => {
                "Unicode"
            }
            Encoding::ShiftJis => "日本語 JIS X 0208 準拠",
            Encoding::Cp932 => "日本語 Windows の Shift_JIS",
            Encoding::ShiftJis2004 | Encoding::EucJis2004 => "日本語 JIS X 0213",
            Encoding::EucJp => "日本語",
            Encoding::Iso2022Jp => "日本語 JIS",
            Encoding::Ebcdic(c, _) => c.description(),
            Encoding::Custom(..) | Encoding::Web(_) => "",
        };
        if desc.is_empty() {
            self.name().to_owned()
        } else {
            format!("{} ({desc})", self.name())
        }
    }

    /// 一覧（UI のメニュー・コンボボックス用）。
    pub fn all() -> Vec<Encoding> {
        let mut v = vec![
            Encoding::Utf8,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
            Encoding::Utf32Le,
            Encoding::Utf32Be,
            Encoding::Cp932,
            Encoding::ShiftJis,
            Encoding::ShiftJis2004,
            Encoding::EucJp,
            Encoding::EucJis2004,
            Encoding::Iso2022Jp,
        ];
        v.extend(Ccsid::ALL.iter().map(|&c| Encoding::Ebcdic(c, Records::Nl)));
        v.extend(
            mappings()
                .into_iter()
                .map(|m| Encoding::Custom(m, Records::Nl)),
        );
        v.extend(WEB_ENCODINGS.iter().map(|e| Encoding::Web(e)));
        v
    }

    /// 名前・別名から探す（大文字小文字、`-` `_` の違いは無視）。
    ///
    /// EBCDIC は `IBM-930`・`CCSID 930`・`cp930` などに、レコードの区切り方を
    /// `/nl`・`/lf`・`/fixed:80` で続けられる（省略すると NL）。[`Encoding::spec`] の逆。
    /// 外部の対応表の名前（`JEF/fixed:80` など）も探す。
    pub fn from_name(name: &str) -> Option<Encoding> {
        if let Some(e) = Self::from_builtin_name(name) {
            return Some(e);
        }
        let (base, rec) = match name.split_once('/') {
            Some((a, b)) => (a, Some(b)),
            None => (name, None),
        };
        let m = mapfile::find(base.trim())?;
        let records = match rec {
            Some(r) => parse_records(r)?,
            None => Records::Nl,
        };
        Some(Encoding::Custom(m, records))
    }

    /// 組み込みの文字コードを名前・別名から探す。
    pub(crate) fn from_builtin_name(name: &str) -> Option<Encoding> {
        if let Some(e) = ebcdic_from_name(name) {
            return Some(e);
        }
        let key: String = name
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .flat_map(char::to_lowercase)
            .collect();
        let e = match key.as_str() {
            "utf8" => Encoding::Utf8,
            "utf16" | "utf16le" | "unicode" => Encoding::Utf16Le,
            "utf16be" => Encoding::Utf16Be,
            "utf32" | "utf32le" => Encoding::Utf32Le,
            "utf32be" => Encoding::Utf32Be,
            "shiftjis" | "sjis" | "jis0208" => Encoding::ShiftJis,
            "cp932" | "windows31j" | "ms932" | "mskanji" => Encoding::Cp932,
            "shiftjis2004" | "sjis2004" | "shiftjisx0213" => Encoding::ShiftJis2004,
            "eucjp" | "ujis" => Encoding::EucJp,
            "eucjis2004" | "eucjisx0213" => Encoding::EucJis2004,
            "iso2022jp" | "jis" => Encoding::Iso2022Jp,
            _ => {
                return encoding_rs::Encoding::for_label(name.trim().as_bytes())
                    .map(Self::from_web);
            }
        };
        Some(e)
    }

    /// 設定ファイルに書ける名前（[`Encoding::from_name`] で元に戻せる）。
    pub fn spec(&self) -> String {
        match self.records() {
            Some(Records::Nl) => format!("{}/nl", self.name()),
            Some(Records::Lf) => format!("{}/lf", self.name()),
            Some(Records::Fixed(n)) => format!("{}/fixed:{n}", self.name()),
            None => self.name().to_owned(),
        }
    }

    /// EBCDIC（EBCDIC が土台の外部の対応表を含む）のレコードの区切り方。それ以外は `None`。
    pub fn records(&self) -> Option<Records> {
        match self {
            Encoding::Ebcdic(_, r) => Some(*r),
            Encoding::Custom(m, r) if m.is_ebcdic() => Some(*r),
            _ => None,
        }
    }

    /// レコードの区切り方を変えたもの（EBCDIC 以外はそのまま）。
    pub fn with_records(self, records: Records) -> Encoding {
        match self {
            Encoding::Ebcdic(c, _) => Encoding::Ebcdic(c, records),
            Encoding::Custom(m, _) if m.is_ebcdic() => Encoding::Custom(m, records),
            e => e,
        }
    }

    /// 文字の対応が同じか（EBCDIC のレコードの区切り方の違いは無視する）。
    pub fn same_charset(&self, other: &Encoding) -> bool {
        match (self, other) {
            (Encoding::Ebcdic(a, _), Encoding::Ebcdic(b, _)) => a == b,
            (Encoding::Custom(a, _), Encoding::Custom(b, _)) => a == b,
            _ => self == other,
        }
    }

    /// `encoding_rs` の文字コードから（独自実装のあるものはそちらにする）。
    pub fn from_web(e: &'static encoding_rs::Encoding) -> Encoding {
        if e == encoding_rs::UTF_8 {
            Encoding::Utf8
        } else if e == encoding_rs::UTF_16LE {
            Encoding::Utf16Le
        } else if e == encoding_rs::UTF_16BE {
            Encoding::Utf16Be
        } else if e == encoding_rs::SHIFT_JIS {
            Encoding::Cp932
        } else if e == encoding_rs::EUC_JP {
            Encoding::EucJp
        } else if e == encoding_rs::ISO_2022_JP {
            Encoding::Iso2022Jp
        } else {
            Encoding::Web(e)
        }
    }

    /// BOM（付けられない文字コードでは空）。
    pub fn bom(&self) -> &'static [u8] {
        match self {
            Encoding::Utf8 => b"\xEF\xBB\xBF",
            Encoding::Utf16Le => b"\xFF\xFE",
            Encoding::Utf16Be => b"\xFE\xFF",
            Encoding::Utf32Le => b"\xFF\xFE\x00\x00",
            Encoding::Utf32Be => b"\x00\x00\xFE\xFF",
            _ => b"",
        }
    }

    pub fn supports_bom(&self) -> bool {
        !self.bom().is_empty()
    }

    pub fn is_unicode(&self) -> bool {
        matches!(
            self,
            Encoding::Utf8
                | Encoding::Utf16Le
                | Encoding::Utf16Be
                | Encoding::Utf32Le
                | Encoding::Utf32Be
        ) || matches!(self, Encoding::Web(e) if *e == encoding_rs::GB18030)
    }

    /// LF（0x0A）のバイトが常に改行で、直後から新しいデコーダで読み始めても結果が変わらないか
    /// （並列にデコードできるか）。UTF-16/32・ISO-2022-JP などは該当しない。
    pub fn splits_at_lf(&self) -> bool {
        match self {
            Encoding::Utf8
            | Encoding::ShiftJis
            | Encoding::Cp932
            | Encoding::ShiftJis2004
            | Encoding::EucJp
            | Encoding::EucJis2004 => true,
            Encoding::Custom(m, _) => !m.is_ebcdic(),
            Encoding::Web(e) => e.is_single_byte(),
            _ => false,
        }
    }

    pub fn new_decoder(&self, escapes: bool) -> Decoder {
        let imp = match self {
            Encoding::Utf8 => DecImp::Utf8,
            Encoding::Utf16Le => DecImp::Unicode(unicode::Form::Utf16Le),
            Encoding::Utf16Be => DecImp::Unicode(unicode::Form::Utf16Be),
            Encoding::Utf32Le => DecImp::Unicode(unicode::Form::Utf32Le),
            Encoding::Utf32Be => DecImp::Unicode(unicode::Form::Utf32Be),
            Encoding::ShiftJis
            | Encoding::Cp932
            | Encoding::ShiftJis2004
            | Encoding::EucJp
            | Encoding::EucJis2004 => DecImp::Dbcs(dbcs::table(*self)),
            Encoding::Iso2022Jp => DecImp::Web(web::WebDecoder::new(encoding_rs::ISO_2022_JP)),
            Encoding::Ebcdic(c, r) => DecImp::Ebcdic(ebcdic::Decoder::new(c.table(), *r)),
            Encoding::Custom(m, r) => match &m.kind {
                mapfile::Kind::Ebcdic(t) => DecImp::Ebcdic(ebcdic::Decoder::new(t, *r)),
                mapfile::Kind::Dbcs(t) => DecImp::Dbcs(t),
            },
            Encoding::Web(e) => DecImp::Web(web::WebDecoder::new(e)),
        };
        Decoder {
            imp,
            buf: Vec::new(),
            escapes,
            stats: DecodeStats::default(),
        }
    }

    /// エンコーダを作る。`escapes` は文書内のエスケープ文字の扱い。
    pub fn new_encoder(&self, escapes: EscapeMode) -> Encoder {
        let imp = match self {
            Encoding::Utf8 => EncImp::Utf8,
            Encoding::Utf16Le => EncImp::Unicode(unicode::Form::Utf16Le),
            Encoding::Utf16Be => EncImp::Unicode(unicode::Form::Utf16Be),
            Encoding::Utf32Le => EncImp::Unicode(unicode::Form::Utf32Le),
            Encoding::Utf32Be => EncImp::Unicode(unicode::Form::Utf32Be),
            Encoding::ShiftJis
            | Encoding::Cp932
            | Encoding::ShiftJis2004
            | Encoding::EucJp
            | Encoding::EucJis2004 => EncImp::Dbcs(dbcs::table(*self)),
            Encoding::Iso2022Jp => EncImp::Web(web::WebEncoder::new(encoding_rs::ISO_2022_JP)),
            Encoding::Ebcdic(c, r) => EncImp::Ebcdic(ebcdic::Encoder::new(c.table(), *r)),
            Encoding::Custom(m, r) => match &m.kind {
                mapfile::Kind::Ebcdic(t) => EncImp::Ebcdic(ebcdic::Encoder::new(t, *r)),
                mapfile::Kind::Dbcs(t) => EncImp::Dbcs(t),
            },
            Encoding::Web(e) => EncImp::Web(web::WebEncoder::new(e)),
        };
        Encoder {
            imp,
            escapes,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

/// 保存時の文書内のエスケープ文字の扱い。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeMode {
    /// 元のバイトに戻す（開いたときと同じ文字コードで保存する場合）
    Restore,
    /// 変換できない文字として報告する（開いたときと別の文字コードで保存する場合）
    Reject,
    /// 普通の文字として扱う（UTF-8 のファイルに元から含まれていた場合など）
    Literal,
}

/// デコードの統計。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeStats {
    /// 不正なバイトの数（エスケープ文字または U+FFFD にしたもの）
    pub invalid: u64,
    /// 優先されない符号で書かれた文字の数（同じ文字コードで保存すると符号が変わる）
    pub noncanonical: u64,
    /// ファイルに元から含まれていたエスケープ文字の数。
    /// 0 でなければエスケープ文字を使わずにデコードし直す必要がある
    pub literal_escapes: u64,
    /// 入力が SO / SI の 2 バイト部のまま、SI なしで終わった（EBCDIC）。同じ文字コードで
    /// 保存するときは、元のバイトに合わせて最後に SI を加えない（[`Encoder::keep_open_shift`]）
    pub open_shift_at_end: bool,
}

/// デコード結果の書き込み先。
pub(crate) struct Sink<'a> {
    pub dst: &'a mut Vec<u8>,
    pub escapes: bool,
    pub stats: &'a mut DecodeStats,
}

impl Sink<'_> {
    pub fn invalid(&mut self, b: u8) {
        self.stats.invalid += 1;
        let c = if self.escapes {
            escape_char(b)
        } else {
            char::REPLACEMENT_CHARACTER
        };
        self.push_char(c);
    }

    pub fn push_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.dst
            .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }

    /// 文字（コードポイント）を追加する。エスケープ文字と同じ文字なら数える。
    pub fn push_cp(&mut self, cp: u32) {
        if (ESCAPE_BASE..ESCAPE_BASE + 0x100).contains(&cp) {
            self.stats.literal_escapes += 1;
        }
        self.push_char(char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
}

enum DecImp {
    Utf8,
    Unicode(unicode::Form),
    Dbcs(&'static dbcs::Table),
    Ebcdic(ebcdic::Decoder),
    Web(web::WebDecoder),
}

/// ストリーミングのデコーダ（ファイルのバイト列 → 文書の UTF-8）。
pub struct Decoder {
    imp: DecImp,
    /// 前回の入力の末尾で途中まで来ているバイト列
    buf: Vec<u8>,
    escapes: bool,
    stats: DecodeStats,
}

impl Decoder {
    /// `src` をデコードして `dst` に追記する。`last` なら入力の終わり。
    /// 入力をどこで区切っても結果は同じになる。
    pub fn decode(&mut self, src: &[u8], dst: &mut Vec<u8>, last: bool) {
        let mut sink = Sink {
            dst,
            escapes: self.escapes,
            stats: &mut self.stats,
        };
        match &mut self.imp {
            DecImp::Utf8 => sink.dst.extend_from_slice(src),
            DecImp::Web(w) => w.decode(src, &mut sink, last),
            DecImp::Unicode(_) | DecImp::Dbcs(_) | DecImp::Ebcdic(_) => {
                let input: &[u8] = if self.buf.is_empty() {
                    src
                } else {
                    self.buf.extend_from_slice(src);
                    &self.buf
                };
                let used = match &mut self.imp {
                    DecImp::Unicode(f) => unicode::decode(*f, input, last, &mut sink),
                    DecImp::Dbcs(t) => t.decode(input, last, &mut sink),
                    DecImp::Ebcdic(d) => d.decode(input, last, &mut sink),
                    _ => unreachable!(),
                };
                let rest = input[used..].to_vec();
                self.buf = rest;
            }
        }
    }

    pub fn stats(&self) -> DecodeStats {
        self.stats
    }
}

enum EncImp {
    Utf8,
    Unicode(unicode::Form),
    Dbcs(&'static dbcs::Table),
    Ebcdic(ebcdic::Encoder),
    Web(web::WebEncoder),
}

/// ストリーミングのエンコーダ（文書の UTF-8 → ファイルのバイト列）。
///
/// 変換できない文字（文書内の範囲）はコールバックで報告し、出力には何も書かない。
/// 報告があった場合、出力は保存に使えない。
pub struct Encoder {
    imp: EncImp,
    escapes: EscapeMode,
    buf: Vec<u8>,
    /// `buf[0]` の文書内の位置
    pos: u64,
}

/// UTF-8 の先頭バイトから文字のバイト数を求める（不正なら 0）。
fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    }
}

/// `src` の先頭の 1 文字。不完全なら `Err(true)`、不正なら `Err(false)`。
fn next_char(src: &[u8]) -> Result<(char, usize), bool> {
    let n = utf8_len(src[0]);
    if n == 0 {
        return Err(false);
    }
    if src.len() < n {
        // 途中までが正しければ不完全
        return match std::str::from_utf8(src) {
            Err(e) if e.error_len().is_none() => Err(true),
            _ => Err(false),
        };
    }
    match std::str::from_utf8(&src[..n]) {
        Ok(s) => Ok((s.chars().next().unwrap(), n)),
        Err(_) => Err(false),
    }
}

impl Encoder {
    /// 入力の終わりが 2 バイト部でも SI を加えない（EBCDIC。開いたファイルが SI なしで
    /// 終わっていた場合に、元のバイトのまま保存するため。[`DecodeStats::open_shift_at_end`]）。
    pub fn keep_open_shift(&mut self) {
        if let EncImp::Ebcdic(e) = &mut self.imp {
            e.keep_open_shift();
        }
    }

    /// 文書のバイト列 `src` をエンコードして `dst` に追記する。
    /// 変換できない文字の文書内の範囲を `bad` に報告する。
    pub fn encode(
        &mut self,
        src: &[u8],
        dst: &mut Vec<u8>,
        last: bool,
        bad: &mut dyn FnMut(Range<u64>),
    ) {
        if let EncImp::Utf8 = self.imp {
            // UTF-8 へはそのまま書く（不正なバイトもそのまま）。
            // 別の文字コード由来のエスケープ文字は元のバイトに戻せないので報告する
            if self.escapes == EscapeMode::Literal {
                dst.extend_from_slice(src);
            } else {
                self.encode_utf8(src, dst, last, bad);
            }
            return;
        }
        self.buf.extend_from_slice(src);
        let input = std::mem::take(&mut self.buf);
        let mut i = 0;
        // 文字コードに渡す、エスケープ文字・不正バイトを含まない範囲の開始位置
        let mut run = 0;
        while i < input.len() {
            let b = input[i];
            if b < 0x80 {
                i += 1;
                continue;
            }
            match next_char(&input[i..]) {
                Err(true) if !last => break,
                Err(_) => {
                    self.flush_run(&input, run..i, dst, false, bad);
                    bad(self.pos + i as u64..self.pos + i as u64 + 1);
                    i += 1;
                    run = i;
                }
                Ok((c, n)) => {
                    if let Some(orig) = unescape_char(c)
                        && self.escapes != EscapeMode::Literal
                    {
                        self.flush_run(&input, run..i, dst, false, bad);
                        if self.escapes == EscapeMode::Restore {
                            dst.push(orig);
                            if let EncImp::Ebcdic(e) = &mut self.imp {
                                e.raw(orig);
                            }
                        } else {
                            bad(self.pos + i as u64..self.pos + (i + n) as u64);
                        }
                        run = i + n;
                    }
                    i += n;
                }
            }
        }
        // 文字コードによっては次の文字を見ないと決まらない（結合文字）ので、
        // 入力が続く場合は最後の文字を残す
        let keep_from = if last {
            i
        } else {
            match &self.imp {
                EncImp::Dbcs(t) => t.hold_from(&input[run..i]).map_or(i, |k| run + k),
                EncImp::Ebcdic(e) => e.hold_from(&input[run..i]).map_or(i, |k| run + k),
                _ => i,
            }
        };
        self.flush_run(
            &input,
            run..keep_from,
            dst,
            last && keep_from == input.len(),
            bad,
        );
        self.pos += keep_from as u64;
        self.buf = input[keep_from..].to_vec();
    }

    /// 正しい UTF-8 でエスケープ文字を含まない範囲をエンコードする。
    fn flush_run(
        &mut self,
        input: &[u8],
        range: Range<usize>,
        dst: &mut Vec<u8>,
        last: bool,
        bad: &mut dyn FnMut(Range<u64>),
    ) {
        let s = std::str::from_utf8(&input[range.clone()]).expect("validated");
        let base = self.pos + range.start as u64;
        let mut report = |r: Range<usize>| bad(base + r.start as u64..base + r.end as u64);
        match &mut self.imp {
            EncImp::Utf8 => dst.extend_from_slice(s.as_bytes()),
            EncImp::Unicode(f) => unicode::encode(*f, s, dst),
            EncImp::Dbcs(t) => t.encode(s, dst, &mut report),
            EncImp::Ebcdic(e) => e.encode(s, dst, last, &mut report),
            EncImp::Web(w) => w.encode(s, dst, last, &mut report),
        }
    }

    fn encode_utf8(
        &mut self,
        src: &[u8],
        dst: &mut Vec<u8>,
        last: bool,
        bad: &mut dyn FnMut(Range<u64>),
    ) {
        // エスケープ文字（F4 8F B8..BB xx）が区切り位置にまたがる場合に備えて末尾 3 バイトを持ち越す
        self.buf.extend_from_slice(src);
        let input = std::mem::take(&mut self.buf);
        let end = if last {
            input.len()
        } else {
            input.len().saturating_sub(3)
        };
        let mut i = 0;
        let mut copied = 0;
        while let Some(k) = memchr_f4(&input[i..end]) {
            let p = i + k;
            if p + 4 <= input.len() && is_escape_prefix(&input[p..]) {
                dst.extend_from_slice(&input[copied..p]);
                bad(self.pos + p as u64..self.pos + p as u64 + 4);
                copied = p + 4;
                i = p + 4;
            } else {
                i = p + 1;
            }
            if i >= end {
                break;
            }
        }
        let keep = end.max(copied);
        dst.extend_from_slice(&input[copied..keep]);
        self.pos += keep as u64;
        self.buf = input[keep..].to_vec();
    }
}

/// `IBM-930/fixed:80` などの EBCDIC の名前。
fn ebcdic_from_name(name: &str) -> Option<Encoding> {
    let (cs, rec) = match name.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (name, None),
    };
    let key: String = cs
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    let digits = ["ibm", "ccsid", "cp", "ebcdic"]
        .iter()
        .find_map(|p| key.strip_prefix(p))?;
    let ccsid = Ccsid::from_number(digits.parse().ok()?)?;
    let records = match rec {
        None => Records::Nl,
        Some(r) => parse_records(r)?,
    };
    Some(Encoding::Ebcdic(ccsid, records))
}

/// `nl`・`lf`・`fixed:80`
fn parse_records(r: &str) -> Option<Records> {
    let r = r.trim().to_ascii_lowercase();
    match r.as_str() {
        "nl" => Some(Records::Nl),
        "lf" => Some(Records::Lf),
        _ => {
            let n: u32 = r
                .strip_prefix("fixed")?
                .trim_start_matches([':', '='])
                .parse()
                .ok()?;
            (n > 0).then_some(Records::Fixed(n))
        }
    }
}

fn memchr_f4(s: &[u8]) -> Option<usize> {
    s.iter().position(|&b| b == 0xF4)
}

/// バイト列全体をデコードする（小さいファイル・テスト用）。
pub fn decode_all(enc: Encoding, bytes: &[u8], escapes: bool) -> (Vec<u8>, DecodeStats) {
    let mut d = enc.new_decoder(escapes);
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 2);
    d.decode(bytes, &mut out, true);
    (out, d.stats())
}

/// 文書のバイト列全体をエンコードする。変換できない文字があれば `Err(その範囲)`。
pub fn encode_all(
    enc: Encoding,
    text: &[u8],
    escapes: EscapeMode,
) -> Result<Vec<u8>, Vec<Range<u64>>> {
    let mut e = enc.new_encoder(escapes);
    let mut out = Vec::with_capacity(text.len());
    let mut bad = Vec::new();
    e.encode(text, &mut out, true, &mut |r| bad.push(r));
    if bad.is_empty() { Ok(out) } else { Err(bad) }
}
