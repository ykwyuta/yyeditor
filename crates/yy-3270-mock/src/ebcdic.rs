//! EBCDIC（SO/SI の 2 バイト文字を含む）と画面の組み立て。

use yy_encoding::{Ccsid, EbcdicCode};

use crate::codes::*;

const SO: u8 = 0x0E;
const SI: u8 = 0x0F;

/// 文字列を EBCDIC にする（2 バイト文字の続きは 1 組の SO〜SI）。CCSID にない文字は `?`。
pub fn encode(ccsid: Ccsid, s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut shift = false;
    for c in s.chars() {
        match ccsid.encode_char(c) {
            Some(EbcdicCode::Single(b)) => {
                if shift {
                    out.push(SI);
                    shift = false;
                }
                out.push(b);
            }
            Some(EbcdicCode::Double(d)) => {
                if !shift {
                    out.push(SO);
                    shift = true;
                }
                out.extend_from_slice(&d.to_be_bytes());
            }
            None => {
                if shift {
                    out.push(SI);
                    shift = false;
                }
                out.push(0x6F);
            }
        }
    }
    if shift {
        out.push(SI);
    }
    out
}

/// 2 バイト文字だけ（DBCS のフィールド。SO/SI なし）にする。
pub fn encode_dbcs(ccsid: Ccsid, s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in s.chars() {
        if let Some(EbcdicCode::Double(d)) = ccsid.encode_char(c) {
            out.extend_from_slice(&d.to_be_bytes());
        }
    }
    out
}

/// EBCDIC（SO/SI の混在）を読む。null は飛ばす。
pub fn decode(ccsid: Ccsid, b: &[u8]) -> String {
    let mut s = String::new();
    let mut shift = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            SO => shift = true,
            SI => shift = false,
            0x00 => {}
            c if shift && i + 1 < b.len() => {
                let code = u16::from_be_bytes([c, b[i + 1]]);
                if code == 0x4040 {
                    s.push('\u{3000}');
                } else {
                    s.push_str(&ccsid.decode_double(code).unwrap_or_else(|| "〓".into()));
                }
                i += 1;
            }
            c => s.push(ccsid.decode_single(c).unwrap_or('?')),
        }
        i += 1;
    }
    s
}

/// 2 バイト文字だけの並び（DBCS のフィールド）を読む。
pub fn decode_dbcs(ccsid: Ccsid, b: &[u8]) -> String {
    let mut wrapped = vec![SO];
    wrapped.extend(b.iter().copied().filter(|&c| c != 0));
    wrapped.push(SI);
    decode(ccsid, &wrapped)
}

/// 画面（Write のオーダーと文字）を組み立てる。
pub struct Screen {
    pub ccsid: Ccsid,
    pub cols: usize,
    pub data: Vec<u8>,
}

impl Screen {
    pub fn new(ccsid: Ccsid, cols: usize) -> Screen {
        Screen {
            ccsid,
            cols,
            data: Vec::new(),
        }
    }

    pub fn addr(&self, row: usize, col: usize) -> usize {
        (row - 1) * self.cols + (col - 1)
    }

    /// 行・桁（1 から）へ。
    pub fn at(&mut self, row: usize, col: usize) -> &mut Self {
        let a = self.addr(row, col);
        self.data.push(ORDER_SBA);
        self.data.extend_from_slice(&addr_bytes(a));
        self
    }

    pub fn sf(&mut self, fa: u8) -> &mut Self {
        self.data.push(ORDER_SF);
        self.data.push(fa_byte(fa));
        self
    }

    /// 拡張フィールド（`(種類, 値)` の並び。フィールド属性は `XA_3270`）。
    pub fn sfe(&mut self, pairs: &[(u8, u8)]) -> &mut Self {
        self.data.push(ORDER_SFE);
        self.data.push(pairs.len() as u8);
        for &(t, v) in pairs {
            self.data.push(t);
            self.data.push(if t == XA_3270 { fa_byte(v) } else { v });
        }
        self
    }

    pub fn text(&mut self, s: &str) -> &mut Self {
        let b = encode(self.ccsid, s);
        self.data.extend_from_slice(&b);
        self
    }

    pub fn ic(&mut self) -> &mut Self {
        self.data.push(ORDER_IC);
        self
    }

    /// コマンドと WCC をつけたレコード。
    pub fn record(&self, cmd: u8, wcc: u8) -> Vec<u8> {
        let mut v = vec![cmd, wcc];
        v.extend_from_slice(&self.data);
        v
    }
}
