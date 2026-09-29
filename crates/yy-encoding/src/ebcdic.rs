//! EBCDIC 系の文字コード（03 章 1.3）。
//!
//! 1 バイト系（037・500・1047・290・1027）と、SO（0x0E）/ SI（0x0F）で 1 バイト部と
//! 2 バイト部を切り替える日本語の混在系（930・939・1390・1399）。対応表は ICU が配布している
//! IBM の変換表（`tools/gen-tables ebcdic`）。
//!
//! # レコードの形式
//!
//! 文書の改行は LF なので、ファイルの改行・レコードの区切りとの対応を [`Records`] で選ぶ。
//!
//! # ロスレス往復
//!
//! エンコーダは文字の種類から SO / SI を自動で出す（2 バイト文字の前に SO、1 バイト文字・
//! 改行・入力の終わりの前に SI）。デコーダはエンコーダと同じ規則をなぞり、エンコーダが
//! 同じ位置に出さないシフト（空の SO…SI、重複したシフト、不正なバイトの直前のシフトなど）は
//! エスケープ文字として残す。エスケープ文字の SO / SI を元のバイトに戻したときは、
//! エンコーダの状態もそのシフトに合わせる。

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::OnceLock;

use crate::Sink;
use crate::tables::ebcdic as t;

/// 対応している EBCDIC の CCSID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ccsid {
    /// 米国・カナダ
    Ibm037,
    /// 国際
    Ibm500,
    /// Latin-1（z/OS の Unix System Services）
    Ibm1047,
    /// 日本語カタカナ（1 バイト）
    Ibm290,
    /// 日本語英小文字（1 バイト）
    Ibm1027,
    /// 290 ＋ 2 バイト（カタカナ系、5026 と同じ）
    Ibm930,
    /// 1027 ＋ 2 バイト（英小文字系、5035 と同じ）
    Ibm939,
    /// 930 に JIS X 0213 相当の拡張・€ を追加
    Ibm1390,
    /// 939 に JIS X 0213 相当の拡張・€ を追加
    Ibm1399,
}

impl Ccsid {
    pub const ALL: [Ccsid; 9] = [
        Ccsid::Ibm930,
        Ccsid::Ibm939,
        Ccsid::Ibm1390,
        Ccsid::Ibm1399,
        Ccsid::Ibm290,
        Ccsid::Ibm1027,
        Ccsid::Ibm037,
        Ccsid::Ibm500,
        Ccsid::Ibm1047,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Ccsid::Ibm037 => "IBM-037",
            Ccsid::Ibm500 => "IBM-500",
            Ccsid::Ibm1047 => "IBM-1047",
            Ccsid::Ibm290 => "IBM-290",
            Ccsid::Ibm1027 => "IBM-1027",
            Ccsid::Ibm930 => "IBM-930",
            Ccsid::Ibm939 => "IBM-939",
            Ccsid::Ibm1390 => "IBM-1390",
            Ccsid::Ibm1399 => "IBM-1399",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Ccsid::Ibm037 => "EBCDIC 英語",
            Ccsid::Ibm500 => "EBCDIC 国際",
            Ccsid::Ibm1047 => "EBCDIC Latin-1",
            Ccsid::Ibm290 => "EBCDIC 日本語カタカナ",
            Ccsid::Ibm1027 => "EBCDIC 日本語英小文字",
            Ccsid::Ibm930 => "EBCDIC 日本語カタカナ＋漢字",
            Ccsid::Ibm939 => "EBCDIC 日本語英小文字＋漢字",
            Ccsid::Ibm1390 => "EBCDIC 日本語カタカナ＋漢字 拡張",
            Ccsid::Ibm1399 => "EBCDIC 日本語英小文字＋漢字 拡張",
        }
    }

    /// 番号（`930` など）から。
    pub fn from_number(n: u32) -> Option<Ccsid> {
        Some(match n {
            37 => Ccsid::Ibm037,
            500 => Ccsid::Ibm500,
            1047 => Ccsid::Ibm1047,
            290 => Ccsid::Ibm290,
            1027 => Ccsid::Ibm1027,
            930 | 5026 => Ccsid::Ibm930,
            939 | 5035 => Ccsid::Ibm939,
            1390 => Ccsid::Ibm1390,
            1399 => Ccsid::Ibm1399,
            _ => return None,
        })
    }

    /// 2 バイト部があるか（SO / SI で切り替える）。
    pub fn is_mixed(self) -> bool {
        matches!(
            self,
            Ccsid::Ibm930 | Ccsid::Ibm939 | Ccsid::Ibm1390 | Ccsid::Ibm1399
        )
    }

    pub(crate) fn table(self) -> &'static Table {
        static TABLES: [OnceLock<Table>; 9] = [const { OnceLock::new() }; 9];
        let i = Ccsid::ALL.iter().position(|c| *c == self).unwrap();
        TABLES[i].get_or_init(|| TableBuilder::from_ccsid(self).finish())
    }
}

/// 生成した対応表の 1 行（符号, 文字, 結合する 2 文字目または 0, 精度）。
type SbEntry = (u8, u32, u32, u8);
type DbEntry = (u16, u32, u32, u8);

/// ファイルのレコード（行）の区切り方。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Records {
    /// NL（0x15）が改行（z/OS のテキスト）。LF（0x25）は U+0085 として読む
    Nl,
    /// LF（0x25）が改行（ICU・Java の既定の対応）。NL（0x15）は U+0085 として読む
    Lf,
    /// 改行のない固定長レコード（バイト数）。各レコードを 1 行として表示し、
    /// 保存時は短い行を空白（0x40）で埋める。最後の改行のない行はそのまま書く
    Fixed(u32),
}

impl Records {
    pub fn label(self) -> String {
        match self {
            Records::Nl => "改行 NL (0x15)".to_owned(),
            Records::Lf => "改行 LF (0x25)".to_owned(),
            Records::Fixed(n) => format!("固定長 {n} バイト"),
        }
    }
}

const SO: u8 = 0x0E;
const SI: u8 = 0x0F;
const NL: u8 = 0x15;
const LF: u8 = 0x25;
const SPACE: u8 = 0x40;
const LF_CP: u32 = 0x0A;
const NEL_CP: u32 = 0x85;

/// 1 バイトの未定義
const NONE: u32 = u32::MAX;
/// デコード表の値のフラグ（2 バイト部の 0 は未定義）
const NONCANON: u32 = 1 << 30;
const PAIR: u32 = 1 << 29;
const CP_MASK: u32 = (1 << 29) - 1;

pub(crate) struct Table {
    sb_dec: [u32; 256],
    db_dec: Vec<u32>,
    pairs: Vec<(u32, u32)>,
    /// 文字 → 符号（1 バイトなら `0x100 | byte`、2 バイトなら `0x10000 | code`）
    enc: HashMap<u32, u32>,
    enc_pairs: HashMap<(u32, u32), u32>,
    bases: HashSet<u32>,
    mixed: bool,
    pub(crate) so: u8,
    pub(crate) si: u8,
}

pub(crate) struct TableBuilder {
    mixed: bool,
    so: u8,
    si: u8,
    /// 符号 → (文字, 往復しないか)。後から加えた対応で置き換わる
    dec: HashMap<u32, (Key, bool)>,
    /// 往復する対応（加えた順）。符号が別の文字に置き換えられたものは使わない
    roundtrip: Vec<(Key, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    One(u32),
    Two(u32, u32),
}

impl Table {
    /// 1 バイト部で `b` が表示できる文字（制御文字以外）か。
    pub(crate) fn is_printable(&self, b: u8) -> bool {
        match self.sb_dec[b as usize] {
            NONE => false,
            v => char::from_u32(v & CP_MASK).is_some_and(|c| !c.is_control()),
        }
    }
}

impl TableBuilder {
    pub(crate) fn new(mixed: bool) -> TableBuilder {
        TableBuilder {
            mixed,
            so: SO,
            si: SI,
            dec: HashMap::new(),
            roundtrip: Vec::new(),
        }
    }

    /// 生成した対応表から作る。
    pub(crate) fn from_ccsid(c: Ccsid) -> TableBuilder {
        let (sb, db): (&[SbEntry], Option<&[DbEntry]>) = match c {
            Ccsid::Ibm037 => (&t::IBM037, None),
            Ccsid::Ibm500 => (&t::IBM500, None),
            Ccsid::Ibm1047 => (&t::IBM1047, None),
            Ccsid::Ibm290 => (&t::IBM290, None),
            Ccsid::Ibm1027 => (&t::IBM1027, None),
            Ccsid::Ibm930 => (&t::IBM930, Some(&t::DBCS300)),
            Ccsid::Ibm939 => (&t::IBM939, Some(&t::DBCS300)),
            Ccsid::Ibm1390 => (&t::IBM1390, Some(&t::DBCS16684)),
            Ccsid::Ibm1399 => (&t::IBM1399, Some(&t::DBCS16684)),
        };
        let mut b = TableBuilder::new(db.is_some());
        for &(code, a, b2, prec) in sb {
            b.add(&[code], a, b2, prec);
        }
        for &(code, a, b2, prec) in db.into_iter().flatten() {
            b.add(&code.to_be_bytes(), a, b2, prec);
        }
        b
    }

    /// 2 バイト部を持たせる（外部の対応表で 2 バイトの符号を加える場合）。
    pub(crate) fn set_mixed(&mut self) {
        self.mixed = true;
    }

    /// 対応を加える。`prec` は ICU の精度（0 = 往復、1 = Unicode → 符号のみ、
    /// 3 = 符号 → Unicode のみ）。同じ符号の対応があれば置き換える。
    ///
    /// Unicode → 符号のみの対応（全角英数 → 半角など）は使わない。保存すると文書と
    /// ファイルの内容が黙って食い違うため、変換できない文字として報告し、似た文字への
    /// 置き換え（[`crate::fold_compat`]）を利用者に選んでもらう。
    /// `b` は結合する 2 文字目（なければ 0）。
    pub(crate) fn add(&mut self, bytes: &[u8], a: u32, b: u32, prec: u8) {
        let key = if b == 0 { Key::One(a) } else { Key::Two(a, b) };
        let code = match *bytes {
            [x] => 0x100 | x as u32,
            [x, y] => 0x10000 | u16::from_be_bytes([x, y]) as u32,
            _ => return,
        };
        match prec {
            0 => {
                self.dec.insert(code, (key, false));
                self.roundtrip.push((key, code));
            }
            3 => {
                self.dec.insert(code, (key, true));
            }
            _ => {}
        }
    }

    /// シフトのバイトを変える（外部の対応表用）。
    pub(crate) fn shifts(&mut self, so: u8, si: u8) {
        self.so = so;
        self.si = si;
    }

    pub(crate) fn finish(self) -> Table {
        let mut t = Table {
            sb_dec: [NONE; 256],
            db_dec: if self.mixed {
                vec![0; 1 << 16]
            } else {
                Vec::new()
            },
            pairs: Vec::new(),
            enc: HashMap::new(),
            enc_pairs: HashMap::new(),
            bases: HashSet::new(),
            mixed: self.mixed,
            so: self.so,
            si: self.si,
        };
        let mut codes: Vec<_> = self.dec.iter().collect();
        codes.sort_by_key(|(code, _)| **code);
        for (&code, &(key, noncanon)) in codes {
            let mut v = match key {
                Key::One(cp) => cp,
                Key::Two(a, b) => {
                    t.pairs.push((a, b));
                    PAIR | (t.pairs.len() as u32 - 1)
                }
            };
            if noncanon {
                v |= NONCANON;
            }
            if code < 0x10000 {
                t.sb_dec[(code & 0xFF) as usize] = v;
            } else if self.mixed {
                t.db_dec[(code & 0xFFFF) as usize] = v;
            }
        }
        let set = |t: &mut Table, key: Key, code: u32| match key {
            Key::One(cp) => {
                t.enc.insert(cp, code);
            }
            Key::Two(a, b) => {
                t.enc_pairs.insert((a, b), code);
                t.bases.insert(a);
            }
        };
        for &(key, code) in &self.roundtrip {
            // 後から別の文字に置き換えられた符号は使わない
            if self.dec.get(&code) == Some(&(key, false)) {
                set(&mut t, key, code);
            }
        }
        t
    }
}

/// 文字の種類（エンコーダがシフトを出すかどうかの判断に使う）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Single,
    Double,
    /// 行（レコード）の終わり・入力の終わり
    Boundary,
    /// 固定長レコードがちょうどいっぱいになった（シフトを出す余地がない）
    FullRecord,
}

/// エンコーダのシフトの状態。
#[derive(Clone, Copy, Default)]
struct Shift {
    /// 2 バイト部にいる
    double: bool,
    /// 2 バイト部に入ってから 2 バイト文字を書いた（書き戻した SO の直後は false）
    dirty: bool,
}

impl Shift {
    /// `class` の文字の前にエンコーダが出すシフト。行・入力の終わりでは、
    /// 2 バイト文字を書いていなければ SI を出さない（SO だけのデータを書き戻せるように）。
    fn expected(self, t: &Table, class: Class) -> Option<u8> {
        match (self.double, class) {
            (false, Class::Double) => Some(t.so),
            (true, Class::Single) => Some(t.si),
            (true, Class::Boundary) if self.dirty => Some(t.si),
            _ => None,
        }
    }

    /// シフトを出した・書き戻した。
    fn apply(&mut self, t: &Table, b: u8) {
        if b == t.so {
            *self = Shift {
                double: true,
                dirty: false,
            };
        } else if b == t.si {
            *self = Shift::default();
        }
    }
}

pub(crate) struct Decoder {
    table: &'static Table,
    records: Records,
    /// バイト列が 2 バイト部か
    double: bool,
    /// ここまでの出力を保存したときのエンコーダの状態
    enc: Shift,
    /// 読んだが、まだ扱いの決まっていないシフト
    pending: Vec<u8>,
    /// 固定長レコードの中の位置
    rec_pos: u32,
}

impl Decoder {
    pub(crate) fn new(table: &'static Table, records: Records) -> Decoder {
        Decoder {
            table,
            records,
            double: false,
            enc: Shift::default(),
            pending: Vec::new(),
            rec_pos: 0,
        }
    }

    /// `class` の文字を出力する前に、保留中のシフトを片付ける。
    fn settle(&mut self, class: Class, sink: &mut Sink<'_>) {
        let t = self.table;
        let expected = self.enc.expected(t, class);
        let silent = match (self.pending.as_slice(), expected) {
            ([], None) => true,
            ([p], Some(e)) => *p == e,
            _ => false,
        };
        if silent {
            if let Some(e) = expected {
                self.enc.apply(t, e);
            }
        } else {
            for b in std::mem::take(&mut self.pending) {
                raw(b, sink);
                self.enc.apply(t, b);
            }
            if let Some(e) = self.enc.expected(t, class) {
                // エンコーダがここにシフトを加える（固定長レコード・入力の終わりが 2 バイト部の場合）
                sink.stats.noncanonical += 1;
                self.enc.apply(t, e);
            }
        }
        self.pending.clear();
        if class == Class::Double {
            self.enc.dirty = true;
        }
    }

    fn emit(&mut self, v: u32, class: Class, sink: &mut Sink<'_>) {
        self.settle(class, sink);
        if v & NONCANON != 0 {
            sink.stats.noncanonical += 1;
        }
        if v & PAIR != 0 {
            let (a, b) = self.table.pairs[(v & CP_MASK) as usize];
            sink.push_cp(a);
            sink.push_cp(b);
        } else {
            sink.push_cp(v & CP_MASK);
        }
    }

    /// 不正なバイト（保存時にそのまま書き戻す）。
    fn invalid(&mut self, bytes: &[u8], sink: &mut Sink<'_>) {
        // エンコーダは生のバイトの前にシフトを出さないので、保留中のシフトも生のバイトにする
        for b in std::mem::take(&mut self.pending) {
            raw(b, sink);
            self.enc.apply(self.table, b);
        }
        for &b in bytes {
            sink.invalid(b);
        }
    }

    /// 固定長レコードの終わり。
    fn end_record(&mut self, sink: &mut Sink<'_>) {
        // レコードの最後のバイトが SI なら、エンコーダは 1 バイト残した位置で SI を出している
        let class = if self.pending == [self.table.si] {
            Class::Boundary
        } else {
            Class::FullRecord
        };
        self.settle(class, sink);
        sink.push_cp(LF_CP);
        self.double = false;
        self.enc = Shift::default();
        self.rec_pos = 0;
    }

    /// 1 バイト部の 1 バイトの文字。
    fn single(&self, b: u8) -> u32 {
        match (self.records, b) {
            (Records::Nl, NL) => LF_CP,
            (Records::Nl, LF) => NEL_CP,
            // 固定長では LF は改行ではない（書き戻せるよう不正なバイトとして持つ）
            (Records::Fixed(_), LF) => NONE,
            _ => self.table.sb_dec[b as usize],
        }
    }

    /// `src` をデコードし、使ったバイト数を返す（`last` でなければ途中の文字を残す）。
    pub(crate) fn decode(&mut self, src: &[u8], last: bool, sink: &mut Sink<'_>) -> usize {
        let t = self.table;
        let fixed = match self.records {
            Records::Fixed(n) => Some(n.max(1)),
            _ => None,
        };
        let mut i = 0;
        while i < src.len() {
            if fixed == Some(self.rec_pos) {
                self.end_record(sink);
            }
            let room = fixed.map_or(u32::MAX, |n| n - self.rec_pos);
            let b = src[i];
            if t.mixed && (b == t.so || b == t.si) {
                let to_double = b == t.so;
                if self.double == to_double {
                    // 重複したシフト
                    self.invalid(&[], sink);
                    raw(b, sink);
                    self.enc.apply(t, b);
                } else {
                    self.pending.push(b);
                    self.double = to_double;
                }
                i += 1;
                self.rec_pos += 1;
                continue;
            }
            if self.double {
                if room < 2 {
                    // レコードの終わりをまたぐ 2 バイト文字
                    self.invalid(&[b], sink);
                    i += 1;
                    self.rec_pos += 1;
                    continue;
                }
                if i + 1 >= src.len() {
                    if !last {
                        return i;
                    }
                    self.invalid(&[b], sink);
                    i += 1;
                    self.rec_pos += 1;
                    continue;
                }
                let b2 = src[i + 1];
                if b2 == t.so || b2 == t.si {
                    // シフトは 2 バイト文字の 2 バイト目にならない
                    self.invalid(&[b], sink);
                    i += 1;
                    self.rec_pos += 1;
                    continue;
                }
                let code = u16::from_be_bytes([b, b2]);
                match t.db_dec[code as usize] {
                    0 => self.invalid(&src[i..i + 2], sink),
                    v => self.emit(v, Class::Double, sink),
                }
                i += 2;
                self.rec_pos += 2;
                continue;
            }
            match self.single(b) {
                NONE => self.invalid(&[b], sink),
                LF_CP if fixed.is_none() => {
                    self.settle(Class::Single, sink);
                    sink.push_cp(LF_CP);
                }
                cp => self.emit(cp, Class::Single, sink),
            }
            i += 1;
            self.rec_pos += 1;
        }
        if last {
            match fixed {
                Some(n) if n == self.rec_pos => self.end_record(sink),
                // SI で最後のレコードがちょうどいっぱいになる場合、エンコーダは SI を出さない
                Some(n) if self.pending.is_empty() && self.rec_pos + 1 >= n => {
                    self.settle(Class::FullRecord, sink)
                }
                _ => self.settle(Class::Boundary, sink),
            }
        }
        src.len()
    }
}

/// エンコーダが書き戻すバイトとして残す（不正なバイトとしては数えない）。
fn raw(b: u8, sink: &mut Sink<'_>) {
    if sink.escapes {
        sink.push_char(crate::escape_char(b));
    } else {
        sink.stats.noncanonical += 1;
    }
}

pub(crate) struct Encoder {
    table: &'static Table,
    records: Records,
    shift: Shift,
    /// 固定長レコードの中の位置
    rec_pos: u64,
}

impl Encoder {
    pub(crate) fn new(table: &'static Table, records: Records) -> Encoder {
        Encoder {
            table,
            records,
            shift: Shift::default(),
            rec_pos: 0,
        }
    }

    /// 文書のエスケープ文字を元のバイトに戻した。
    pub(crate) fn raw(&mut self, b: u8) {
        if self.table.mixed {
            self.shift.apply(self.table, b);
        }
        self.rec_pos += 1;
    }

    fn shift(&mut self, class: Class, dst: &mut Vec<u8>) {
        if let Some(s) = self.shift.expected(self.table, class) {
            dst.push(s);
            self.shift.apply(self.table, s);
            self.rec_pos += 1;
        }
    }

    /// 文字の符号（`0x100 | byte` / `0x10000 | code`、変換できなければ 0）。
    fn code(&self, cp: u32) -> u32 {
        match (self.records, cp) {
            (Records::Nl, LF_CP) => 0x100 | NL as u32,
            (Records::Nl, NEL_CP) => 0x100 | LF as u32,
            (Records::Fixed(_), LF_CP) => 0,
            _ => self.table.enc.get(&cp).copied().unwrap_or(0),
        }
    }

    fn put(&mut self, code: u32, dst: &mut Vec<u8>) {
        if code >= 0x10000 {
            self.shift(Class::Double, dst);
            dst.extend_from_slice(&(code as u16).to_be_bytes());
            self.shift.dirty = true;
            self.rec_pos += 2;
        } else {
            self.shift(Class::Single, dst);
            dst.push(code as u8);
            self.rec_pos += 1;
        }
    }

    /// 正しい UTF-8 の `s` をエンコードする。変換できない文字の `s` 内の範囲を報告する。
    pub(crate) fn encode(
        &mut self,
        s: &str,
        dst: &mut Vec<u8>,
        last: bool,
        bad: &mut dyn FnMut(Range<usize>),
    ) {
        let t = self.table;
        let fixed = match self.records {
            Records::Fixed(n) => Some(n.max(1) as u64),
            _ => None,
        };
        let mut it = s.char_indices().peekable();
        while let Some((i, c)) = it.next() {
            let cp = c as u32;
            let start = self.rec_pos;
            if cp == LF_CP
                && let Some(n) = fixed
            {
                // レコードがいっぱいなら SI は出さない（次のレコードは 1 バイト部から始まる）
                if self.rec_pos < n {
                    self.shift(Class::Boundary, dst);
                }
                while self.rec_pos < n {
                    dst.push(SPACE);
                    self.rec_pos += 1;
                }
                self.rec_pos = 0;
                self.shift = Shift::default();
                continue;
            }
            let mut len = c.len_utf8();
            let pair = t
                .bases
                .contains(&cp)
                .then(|| it.peek())
                .flatten()
                .and_then(|&(_, next)| t.enc_pairs.get(&(cp, next as u32)).copied());
            match pair {
                Some(code) => {
                    len += it.next().unwrap().1.len_utf8();
                    self.put(code, dst);
                }
                None => match self.code(cp) {
                    0 => bad(i..i + len),
                    code => self.put(code, dst),
                },
            }
            if let Some(n) = fixed
                && self.rec_pos > n
                && start <= n
            {
                // レコード長を超えた
                bad(i..i + len);
            }
        }
        if last && fixed.is_none_or(|n| self.rec_pos + 1 < n) {
            self.shift(Class::Boundary, dst);
        }
    }

    /// 入力が続く場合に次の入力まで持ち越す位置。[`Encoder::encode`] と同じ規則で先頭から
    /// 組を作っていき、最後の文字が組にならずに残り、次の文字と組になりうる場合はその位置。
    pub(crate) fn hold_from(&self, run: &[u8]) -> Option<usize> {
        let t = self.table;
        if t.bases.is_empty() {
            return None;
        }
        let s = std::str::from_utf8(run).ok()?;
        let mut it = s.char_indices().peekable();
        let mut lone_base = None;
        while let Some((i, c)) = it.next() {
            let cp = c as u32;
            lone_base = None;
            if !t.bases.contains(&cp) {
                continue;
            }
            match it.peek() {
                Some(&(_, next)) if t.enc_pairs.contains_key(&(cp, next as u32)) => {
                    it.next();
                }
                Some(_) => {}
                None => lone_base = Some(i),
            }
        }
        lone_base
    }
}
