//! 3270 データストリームの解釈と、端末の入力の規則（14 章 6・7・8）。
//!
//! - ホストからのレコード（コマンド・WCC・オーダー・構造化フィールド）で画面を書き換え、
//!   読み取りの要求には送るデータを返す。
//! - キー操作は 3270 の端末と同じ規則で画面を書き換える（保護・数字・MDT・自動スキップ・挿入・
//!   キーボードのロック）。AID のキーでは Read Modified の形で送るデータを返す。
//! - フィールドの編集は、フィールドの中身を「文字の並び」（1 バイト・2 バイト）に直してから行い、
//!   SO/SI を付け直して書き戻す。2 バイト文字（DBCS）の入力でも SO/SI の対応が崩れない。

use yy_encoding::{Ccsid, EbcdicCode};

use crate::codes::*;
use crate::ind_file::Dft;
use crate::query;
use crate::screen::{Ext, Screen};

/// キーボードの状態。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lock {
    /// 入力できる
    None,
    /// ホストの応答を待っている（AID を送った、接続直後）
    System,
    /// 操作の誤り（保護フィールドへの入力など。Reset で解除）
    Operator(OperatorError),
}

/// 操作の誤り。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperatorError {
    /// 保護フィールドに入力した
    Protected,
    /// 数字フィールドに数字以外を入力した
    Numeric,
    /// フィールドに入りきらない
    Overflow,
    /// 文字コードで表せない文字
    Unencodable,
    /// 2 バイト文字のフィールドに 1 バイト文字（またはその逆）
    WrongCharset,
}

impl OperatorError {
    /// OIA に出す表示。
    pub fn label(self) -> &'static str {
        match self {
            OperatorError::Protected => "X 保護",
            OperatorError::Numeric => "X 数字のみ",
            OperatorError::Overflow => "X あふれ",
            OperatorError::Unencodable => "X 文字コード",
            OperatorError::WrongCharset => "X 文字の種類",
        }
    }
}

/// キー操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Pf(u8),
    Pa(u8),
    Clear,
    SysReq,
    Attn,
    Reset,
    Tab,
    BackTab,
    Home,
    NewLine,
    Up,
    Down,
    Left,
    Right,
    Backspace,
    Delete,
    EraseEof,
    EraseInput,
    Insert,
    Dup,
    FieldMark,
    /// 画面のその位置にカーソルを移す（マウスのクリック）
    MoveTo(usize),
}

/// エミュレーターの処理の結果。
#[derive(Debug, Default)]
pub struct Reply {
    /// ホストへ送る 3270 のデータ（TN3270E のヘッダー・Telnet の枠はまだ付けない）
    pub data: Option<Vec<u8>>,
    /// Telnet の IP（Attn）を送る
    pub attn: bool,
    pub alarm: bool,
    /// 画面が変わった
    pub changed: bool,
}

/// 返答の形（Set Reply Mode）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyMode {
    Field,
    ExtendedField,
    Character,
}

/// 画面とキーボードの状態。
pub struct Emulator {
    pub screen: Screen,
    pub ccsid: Ccsid,
    pub lock: Lock,
    pub insert: bool,
    /// 最後に送った AID
    pub aid: u8,
    pub reply_mode: ReplyMode,
    /// Query Reply に使う、端末の大きさなど
    pub model: u8,
    /// IND$FILE の転送
    pub ft: Dft,
}

/// フィールドの中身の 1 文字。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Single(u8),
    Double(u16),
}

impl Emulator {
    pub fn new(model: u8, ccsid: Ccsid) -> Emulator {
        let alt = alternate_size(model);
        Emulator {
            screen: Screen::new((24, 80), alt),
            ccsid,
            lock: Lock::System,
            insert: false,
            aid: AID_NONE,
            reply_mode: ReplyMode::Field,
            model,
            ft: Dft::default(),
        }
    }

    // ---- ホストからのデータ ------------------------------------------------------------

    /// ホストからの 1 レコード（コマンドから）を処理する。
    pub fn process(&mut self, rec: &[u8]) -> Reply {
        let mut r = Reply::default();
        let Some(&cmd) = rec.first() else {
            return r;
        };
        let Some(cmd) = Command::from_byte(cmd) else {
            return r;
        };
        match cmd {
            Command::Write | Command::EraseWrite | Command::EraseWriteAlternate => {
                if cmd != Command::Write {
                    self.screen.erase(cmd == Command::EraseWriteAlternate);
                }
                self.write(&rec[1..], cmd == Command::Write, &mut r);
            }
            Command::ReadBuffer => r.data = Some(self.read_buffer()),
            Command::ReadModified => r.data = Some(self.read_modified(self.aid, false)),
            Command::ReadModifiedAll => r.data = Some(self.read_modified(self.aid, true)),
            Command::EraseAllUnprotected => {
                self.erase_all_unprotected();
                self.lock = Lock::None;
                self.aid = AID_NONE;
                r.changed = true;
            }
            Command::WriteStructuredField => self.structured_fields(&rec[1..], &mut r),
        }
        r
    }

    /// Write（WCC とオーダー）。`at_cursor` なら書き始めはカーソルの位置。
    fn write(&mut self, data: &[u8], at_cursor: bool, r: &mut Reply) {
        r.changed = true;
        let Some(&wcc) = data.first() else {
            return;
        };
        if wcc & WCC_RESET_MDT != 0 {
            self.screen.reset_mdt();
        }
        let s = &mut self.screen;
        let n = s.len();
        let mut ba = if at_cursor { s.cursor } else { 0 };
        let mut sa = Ext::default();
        let mut i = 1;
        // 直前がデータの文字だったか（PT の動きが変わる）
        let mut after_data = false;
        let addr = |d: &[u8], i: usize| -> Option<usize> {
            Some(decode_address(*d.get(i)?, *d.get(i + 1)?) % n)
        };
        while i < data.len() {
            let b = data[i];
            match b {
                ORDER_SBA => {
                    let Some(a) = addr(data, i + 1) else { break };
                    ba = a;
                    i += 3;
                    after_data = false;
                }
                ORDER_SF => {
                    let Some(&fa) = data.get(i + 1) else { break };
                    s.cells[ba].fa = Some(fa & 0x3F);
                    s.cells[ba].b = 0;
                    s.cells[ba].ext = Ext::default();
                    ba = s.next(ba);
                    i += 2;
                    after_data = false;
                }
                ORDER_SFE => {
                    let Some(&count) = data.get(i + 1) else { break };
                    let mut fa = 0;
                    let mut ext = Ext::default();
                    let mut j = i + 2;
                    for _ in 0..count {
                        let (Some(&t), Some(&v)) = (data.get(j), data.get(j + 1)) else {
                            break;
                        };
                        if t == XA_3270 {
                            fa = v & 0x3F;
                        } else {
                            ext.set(t, v);
                        }
                        j += 2;
                    }
                    s.cells[ba].fa = Some(fa);
                    s.cells[ba].b = 0;
                    s.cells[ba].ext = ext;
                    ba = s.next(ba);
                    i = j;
                    after_data = false;
                }
                ORDER_SA => {
                    let (Some(&t), Some(&v)) = (data.get(i + 1), data.get(i + 2)) else {
                        break;
                    };
                    sa.set(t, v);
                    i += 3;
                }
                ORDER_MF => {
                    let Some(&count) = data.get(i + 1) else { break };
                    let mut j = i + 2;
                    for _ in 0..count {
                        let (Some(&t), Some(&v)) = (data.get(j), data.get(j + 1)) else {
                            break;
                        };
                        let c = &mut s.cells[ba];
                        if c.fa.is_some() {
                            if t == XA_3270 {
                                c.fa = Some(v & 0x3F);
                            } else {
                                c.ext.set(t, v);
                            }
                        }
                        j += 2;
                    }
                    if s.cells[ba].fa.is_some() {
                        ba = s.next(ba);
                    }
                    i = j;
                    after_data = false;
                }
                ORDER_IC => {
                    s.cursor = ba;
                    i += 1;
                }
                ORDER_PT => {
                    // データの直後なら、フィールドの終わりまで null にする
                    if after_data && s.cells[ba].fa.is_none() {
                        let mut p = ba;
                        while s.cells[p].fa.is_none() {
                            s.cells[p].b = 0;
                            p = s.next(p);
                            if p == 0 {
                                break;
                            }
                        }
                    }
                    ba = s.next_unprotected(ba).unwrap_or(0);
                    i += 1;
                    after_data = false;
                }
                ORDER_RA => {
                    let Some(to) = addr(data, i + 1) else { break };
                    let mut j = i + 3;
                    let Some(&first) = data.get(j) else { break };
                    let mut ch = first;
                    if ch == ORDER_GE {
                        j += 1;
                        let Some(&g) = data.get(j) else { break };
                        ch = g;
                    }
                    j += 1;
                    loop {
                        s.cells[ba].b = ch;
                        s.cells[ba].fa = None;
                        s.cells[ba].ext = sa;
                        ba = s.next(ba);
                        if ba == to {
                            break;
                        }
                    }
                    i = j;
                    after_data = true;
                }
                ORDER_EUA => {
                    let Some(to) = addr(data, i + 1) else { break };
                    loop {
                        if !s.is_protected(ba) {
                            s.cells[ba].b = 0;
                        }
                        ba = s.next(ba);
                        if ba == to {
                            break;
                        }
                    }
                    i += 3;
                    after_data = false;
                }
                ORDER_GE => {
                    let Some(&g) = data.get(i + 1) else { break };
                    s.cells[ba].b = g;
                    s.cells[ba].fa = None;
                    s.cells[ba].ext = sa;
                    ba = s.next(ba);
                    i += 2;
                    after_data = true;
                }
                _ => {
                    s.cells[ba].b = b;
                    s.cells[ba].fa = None;
                    s.cells[ba].ext = sa;
                    ba = s.next(ba);
                    i += 1;
                    after_data = true;
                }
            }
        }
        if wcc & WCC_ALARM != 0 {
            r.alarm = true;
        }
        if wcc & WCC_RESTORE != 0 {
            self.lock = Lock::None;
            self.aid = AID_NONE;
        }
    }

    fn erase_all_unprotected(&mut self) {
        let s = &mut self.screen;
        if !s.formatted() {
            for c in &mut s.cells {
                c.b = 0;
            }
            s.cursor = 0;
            return;
        }
        for a in 0..s.len() {
            if !s.is_protected(a) {
                s.cells[a].b = 0;
            }
        }
        s.reset_mdt();
        s.cursor = s.first_unprotected();
    }

    fn structured_fields(&mut self, mut data: &[u8], r: &mut Reply) {
        while data.len() >= 3 {
            let len = usize::from(u16::from_be_bytes([data[0], data[1]]));
            let len = if len == 0 { data.len() } else { len };
            if len < 3 || len > data.len() {
                break;
            }
            let (sf, rest) = data.split_at(len);
            data = rest;
            let id = sf[2];
            let body = &sf[3..];
            match id {
                SF_READ_PARTITION => {
                    let typ = body.get(1).copied().unwrap_or(0);
                    r.data = Some(match typ {
                        RP_QUERY | RP_QUERY_LIST => query::reply(self),
                        RP_RB => self.read_buffer(),
                        RP_RMA => self.read_modified(self.aid, true),
                        _ => self.read_modified(self.aid, false),
                    });
                }
                SF_ERASE_RESET => {
                    let alt = body.first().is_some_and(|f| f & 0x80 != 0);
                    self.screen.erase(alt);
                    r.changed = true;
                }
                SF_SET_REPLY_MODE => {
                    self.reply_mode = match body.get(1) {
                        Some(1) => ReplyMode::ExtendedField,
                        Some(2) => ReplyMode::Character,
                        _ => ReplyMode::Field,
                    };
                }
                SF_OUTBOUND_3270DS if body.len() >= 2 => {
                    let inner = self.process(&body[1..]);
                    r.changed |= inner.changed;
                    r.alarm |= inner.alarm;
                    if inner.data.is_some() {
                        r.data = inner.data;
                    }
                }
                SF_DATA_CHUNK => {
                    if let Some(d) = self.ft.handle(sf) {
                        r.data = Some(d);
                    }
                }
                _ => {}
            }
        }
    }

    // ---- ホストへ送るデータ -------------------------------------------------------------

    fn cursor_bytes(&self) -> [u8; 2] {
        encode_address(self.screen.cursor)
    }

    /// Read Buffer の応答（AID・カーソル・画面全体）。
    pub fn read_buffer(&self) -> Vec<u8> {
        let mut out = vec![self.aid];
        out.extend_from_slice(&self.cursor_bytes());
        for c in &self.screen.cells {
            match c.fa {
                Some(fa) if self.reply_mode == ReplyMode::Field => {
                    out.push(ORDER_SF);
                    out.push(encode_fa(fa));
                }
                Some(fa) => {
                    let mut pairs = vec![(XA_3270, encode_fa(fa))];
                    for (t, v) in [
                        (XA_FOREGROUND, c.ext.fg),
                        (XA_BACKGROUND, c.ext.bg),
                        (XA_HIGHLIGHTING, c.ext.hl),
                        (XA_CHARSET, c.ext.cs),
                    ] {
                        if v != 0 {
                            pairs.push((t, v));
                        }
                    }
                    out.push(ORDER_SFE);
                    out.push(pairs.len() as u8);
                    for (t, v) in pairs {
                        out.extend_from_slice(&[t, v]);
                    }
                }
                None => out.push(c.b),
            }
        }
        out
    }

    /// Read Modified の応答。`all` なら PA・Clear でもフィールドを送る（Read Modified All）。
    pub fn read_modified(&self, aid: u8, all: bool) -> Vec<u8> {
        let mut out = vec![aid];
        if is_short_read(aid) && !all {
            return out;
        }
        out.extend_from_slice(&self.cursor_bytes());
        let s = &self.screen;
        if !s.formatted() {
            out.extend(s.cells.iter().map(|c| c.b).filter(|&b| b != 0));
            return out;
        }
        for p in s.field_attrs() {
            let fa = s.cells[p].fa.unwrap_or(0);
            if fa & FA_MDT == 0 {
                continue;
            }
            let (start, len) = s.field_range(p);
            out.push(ORDER_SBA);
            out.extend_from_slice(&encode_address(start));
            let mut a = start;
            for _ in 0..len {
                let b = s.cells[a].b;
                if b != 0 {
                    out.push(b);
                }
                a = s.next(a);
            }
        }
        out
    }

    // ---- キー操作 -------------------------------------------------------------------

    /// キーを押した。
    pub fn key(&mut self, k: Key) -> Reply {
        let mut r = Reply::default();
        match k {
            Key::Reset => {
                if matches!(self.lock, Lock::Operator(_)) {
                    self.lock = Lock::None;
                }
                self.insert = false;
                r.changed = true;
                return r;
            }
            Key::Attn => {
                r.attn = true;
                return r;
            }
            _ => {}
        }
        if self.lock != Lock::None {
            // ロック中のキーは捨てる（先打ちはしない）。カーソルの移動だけは許す
            if !matches!(k, Key::MoveTo(_)) {
                return r;
            }
        }
        r.changed = true;
        match k {
            Key::Enter => self.send_aid(AID_ENTER, &mut r),
            Key::Pf(n) => {
                if let Some(aid) = pf_aid(n) {
                    self.send_aid(aid, &mut r);
                }
            }
            Key::Pa(n) => {
                let aid = match n {
                    1 => AID_PA1,
                    2 => AID_PA2,
                    _ => AID_PA3,
                };
                self.send_aid(aid, &mut r);
            }
            Key::SysReq => self.send_aid(AID_SYSREQ, &mut r),
            Key::Clear => {
                self.screen.erase(false);
                self.send_aid(AID_CLEAR, &mut r);
            }
            Key::Char(c) => self.type_char(c),
            Key::Dup => {
                if self.put_byte(FC_DUP) {
                    self.tab();
                }
            }
            Key::FieldMark => {
                self.put_byte(FC_FM);
            }
            Key::Tab => self.tab(),
            Key::BackTab => self.back_tab(),
            Key::Home => {
                let s = &mut self.screen;
                s.cursor = if s.formatted() {
                    s.first_unprotected()
                } else {
                    0
                };
            }
            Key::NewLine => {
                let s = &mut self.screen;
                let line_start = (s.cursor / s.cols + 1) * s.cols % s.len();
                s.cursor = if s.formatted() {
                    s.next_unprotected(s.prev(line_start)).unwrap_or(0)
                } else {
                    line_start
                };
            }
            Key::Up => self.move_cursor(-(self.screen.cols as isize)),
            Key::Down => self.move_cursor(self.screen.cols as isize),
            Key::Left => self.move_cursor(-1),
            Key::Right => self.move_cursor(1),
            Key::Backspace => self.backspace(),
            Key::Delete => {
                self.edit(Edit::Delete);
            }
            Key::EraseEof => {
                self.edit(Edit::EraseEof);
            }
            Key::EraseInput => self.erase_input(),
            Key::Insert => self.insert = !self.insert,
            Key::MoveTo(a) => {
                if a < self.screen.len() {
                    self.screen.cursor = a;
                    self.snap_cursor(true);
                }
            }
            Key::Reset | Key::Attn => {}
        }
        r
    }

    fn send_aid(&mut self, aid: u8, r: &mut Reply) {
        self.aid = aid;
        self.lock = Lock::System;
        self.insert = false;
        r.data = Some(self.read_modified(aid, false));
    }

    fn move_cursor(&mut self, delta: isize) {
        let n = self.screen.len() as isize;
        self.screen.cursor = ((self.screen.cursor as isize + delta).rem_euclid(n)) as usize;
        self.snap_cursor(delta >= 0);
    }

    /// カーソルが 2 バイト文字の 2 セル目にあれば、文字の先頭（`forward` なら次の文字）に動かす。
    fn snap_cursor(&mut self, forward: bool) {
        let d = self.screen.display(self.ccsid);
        if d[self.screen.cursor].width == 0 {
            self.screen.cursor = if forward {
                self.screen.next(self.screen.cursor)
            } else {
                self.screen.prev(self.screen.cursor)
            };
        }
    }

    fn tab(&mut self) {
        let s = &mut self.screen;
        s.cursor = if s.formatted() {
            s.next_unprotected(s.cursor).unwrap_or(0)
        } else {
            0
        };
    }

    fn back_tab(&mut self) {
        let s = &mut self.screen;
        if !s.formatted() {
            s.cursor = 0;
            return;
        }
        // フィールドの途中ならその先頭、先頭なら前の非保護フィールドの先頭
        if let Some(p) = s.field_attr_addr(s.cursor)
            && !s.is_protected(s.cursor)
        {
            let start = s.next(p);
            if s.cursor != start {
                s.cursor = start;
                return;
            }
        }
        let mut a = s.prev(s.cursor);
        for _ in 0..s.len() {
            let q = s.prev(a);
            if s.cells[a].fa.is_none() && s.cells[q].fa.is_some() && !s.is_protected(a) {
                s.cursor = a;
                return;
            }
            a = q;
        }
    }

    fn backspace(&mut self) {
        let s = &self.screen;
        // フィールドの先頭より前には戻らない
        let start = s.field_attr_addr(s.cursor).map(|p| s.next(p));
        if Some(s.cursor) == start || s.cursor == 0 && start.is_none() {
            return;
        }
        self.move_cursor(-1);
        // SO/SI のセルは飛ばす
        let s = &self.screen;
        if matches!(s.cells[s.cursor].b, FC_SO | FC_SI) && Some(s.cursor) != start {
            self.move_cursor(-1);
        }
    }

    fn erase_input(&mut self) {
        let s = &mut self.screen;
        if !s.formatted() {
            for c in &mut s.cells {
                c.b = 0;
            }
            s.cursor = 0;
            return;
        }
        for a in 0..s.len() {
            if !s.is_protected(a) {
                s.cells[a].b = 0;
            }
        }
        s.reset_mdt();
        s.cursor = s.first_unprotected();
    }

    fn error(&mut self, e: OperatorError) {
        self.lock = Lock::Operator(e);
    }

    /// 書式の制御の文字（Dup・Field Mark）を入れる。入れたら `true`。
    fn put_byte(&mut self, b: u8) -> bool {
        self.edit(Edit::Put(Tok::Single(b)))
    }

    fn type_char(&mut self, c: char) {
        // 数字フィールド
        let s = &self.screen;
        if s.is_protected(s.cursor) {
            self.error(OperatorError::Protected);
            return;
        }
        if s.field_attr(s.cursor)
            .is_some_and(|fa| fa & FA_NUMERIC != 0)
            && !(c.is_ascii_digit() || c == '.' || c == '-')
        {
            self.error(OperatorError::Numeric);
            return;
        }
        let tok = match self.ccsid.encode_char(c) {
            Some(EbcdicCode::Single(b)) => Tok::Single(b),
            Some(EbcdicCode::Double(d)) => Tok::Double(d),
            None if c == '\u{3000}' && self.ccsid.is_mixed() => Tok::Double(0x4040),
            None => {
                self.error(OperatorError::Unencodable);
                return;
            }
        };
        self.edit(Edit::Put(tok));
    }

    /// カーソルのあるフィールドの範囲（書式がなければ画面全体）。
    fn field_at_cursor(&self) -> Option<(usize, usize, bool)> {
        let s = &self.screen;
        if !s.formatted() {
            return Some((0, s.len(), false));
        }
        let p = s.field_attr_addr(s.cursor)?;
        if s.is_protected(s.cursor) {
            return None;
        }
        let (start, len) = s.field_range(p);
        Some((start, len, s.cells[p].ext.cs == CS_DBCS))
    }

    /// フィールドの編集（文字の並びに直して編集し、SO/SI を付け直して書き戻す）。成功したら `true`。
    fn edit(&mut self, op: Edit) -> bool {
        let Some((start, len, dbcs_field)) = self.field_at_cursor() else {
            self.error(OperatorError::Protected);
            return false;
        };
        let n = self.screen.len();
        let mixed = self.ccsid.is_mixed();
        // フィールドの中身を文字の並びに（セルの位置も覚える）
        let mut toks: Vec<(Tok, usize)> = Vec::new();
        let mut shift = false;
        let mut i = 0;
        while i < len {
            let a = (start + i) % n;
            let b = self.screen.cells[a].b;
            if !dbcs_field && mixed && b == FC_SO {
                shift = true;
                i += 1;
                continue;
            }
            if !dbcs_field && mixed && b == FC_SI {
                shift = false;
                i += 1;
                continue;
            }
            if (dbcs_field || shift) && i + 1 < len {
                let b2 = self.screen.cells[(a + 1) % n].b;
                toks.push((Tok::Double(u16::from_be_bytes([b, b2])), i));
                i += 2;
                continue;
            }
            toks.push((Tok::Single(b), i));
            i += 1;
        }
        // カーソルの位置の文字の番号（SO/SI のセルなら次の文字）
        let offset = (self.screen.cursor + n - start) % n;
        let idx = toks
            .iter()
            .position(|(_, off)| *off >= offset)
            .unwrap_or(toks.len());
        // 末尾の null は詰められる余白
        let trailing_nulls = toks
            .iter()
            .rev()
            .take_while(|(t, _)| matches!(t, Tok::Single(0)) || matches!(t, Tok::Double(0)))
            .count();
        let mut chars: Vec<Tok> = toks.iter().map(|(t, _)| *t).collect();
        let new_index = match op {
            Edit::Put(t) => {
                if dbcs_field && matches!(t, Tok::Single(b) if b != FC_DUP && b != FC_FM) {
                    self.error(OperatorError::WrongCharset);
                    return false;
                }
                if !mixed && !dbcs_field && matches!(t, Tok::Double(_)) {
                    self.error(OperatorError::WrongCharset);
                    return false;
                }
                if self.insert {
                    if trailing_nulls == 0 {
                        self.error(OperatorError::Overflow);
                        return false;
                    }
                    // 末尾の null を 1 つ減らして差し込む（2 バイト文字なら必要に応じてさらに）
                    let pos = chars.len() - 1;
                    chars.remove(pos);
                    chars.insert(idx.min(chars.len()), t);
                } else if idx < chars.len() {
                    chars[idx] = t;
                } else {
                    self.error(OperatorError::Overflow);
                    return false;
                }
                idx + 1
            }
            Edit::Delete => {
                if idx >= chars.len() {
                    return false;
                }
                chars.remove(idx);
                idx
            }
            Edit::EraseEof => {
                chars.truncate(idx);
                idx
            }
        };
        // 符号に戻す（2 バイト文字の前に SO、1 バイト文字の前と終わりに SI）
        let Some((bytes, positions)) = encode_field(&chars, dbcs_field, mixed, len) else {
            self.error(OperatorError::Overflow);
            return false;
        };
        for (k, b) in bytes.iter().enumerate() {
            let a = (start + k) % n;
            self.screen.cells[a].b = *b;
        }
        self.screen.set_mdt(start);
        // カーソル: 編集した次の文字。フィールドの終わりに達したら次の非保護フィールドへ
        let cell = positions.get(new_index).copied();
        let s = &mut self.screen;
        match cell {
            Some(off) => s.cursor = (start + off) % n,
            None if matches!(op, Edit::Put(_)) && s.formatted() => {
                s.cursor = s.next_unprotected(s.prev((start + len) % n)).unwrap_or(0);
            }
            None => {
                s.cursor = (start + bytes.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1)) % n
            }
        }
        true
    }

    /// 文字列を貼り付ける（非保護フィールドにだけ入れる。改行は New Line）。入れた文字数を返す。
    pub fn paste(&mut self, text: &str) -> usize {
        let mut count = 0;
        for c in text.chars() {
            if self.lock != Lock::None {
                break;
            }
            match c {
                '\r' => {}
                '\n' => {
                    self.key(Key::NewLine);
                }
                '\t' => {
                    self.key(Key::Tab);
                }
                c => {
                    self.key(Key::Char(c));
                    if matches!(self.lock, Lock::Operator(_)) {
                        break;
                    }
                    count += 1;
                }
            }
        }
        count
    }
}

/// フィールドの編集の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    Put(Tok),
    Delete,
    EraseEof,
}

/// 文字の並びをフィールドの符号にする（長さ `len` まで null で埋める）。各文字の先頭のセルの
/// 位置も返す。入りきらなければ `None`。
fn encode_field(
    chars: &[Tok],
    dbcs_field: bool,
    mixed: bool,
    len: usize,
) -> Option<(Vec<u8>, Vec<usize>)> {
    let mut out = Vec::with_capacity(len);
    let mut pos = Vec::with_capacity(chars.len());
    // 末尾の null は、2 バイトの部分の外に置く（SI の後ろ）
    let last = chars
        .iter()
        .rposition(|t| !matches!(t, Tok::Single(0) | Tok::Double(0)))
        .map_or(0, |p| p + 1);
    let mut shift = false;
    for (k, t) in chars.iter().enumerate() {
        let filler = k >= last;
        match *t {
            Tok::Double(d) if !filler || dbcs_field => {
                if !dbcs_field && mixed && !shift {
                    out.push(FC_SO);
                    shift = true;
                }
                pos.push(out.len());
                out.extend_from_slice(&d.to_be_bytes());
            }
            Tok::Single(b) => {
                if shift {
                    out.push(FC_SI);
                    shift = false;
                }
                pos.push(out.len());
                out.push(b);
            }
            Tok::Double(_) => {
                if shift {
                    out.push(FC_SI);
                    shift = false;
                }
                pos.push(out.len());
                out.push(0);
            }
        }
    }
    if shift {
        out.push(FC_SI);
    }
    // 埋めの null は削って長さに合わせる
    while out.len() > len {
        if out.last() == Some(&0) {
            out.pop();
            pos.retain(|&p| p < out.len());
        } else {
            return None;
        }
    }
    out.resize(len, 0);
    Some((out, pos))
}

/// モデルの代替の大きさ（行, 桁）。
pub fn alternate_size(model: u8) -> (usize, usize) {
    match model {
        3 => (32, 80),
        4 => (43, 80),
        5 => (27, 132),
        _ => (24, 80),
    }
}

#[cfg(test)]
mod tests;
