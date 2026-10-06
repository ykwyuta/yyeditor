//! Read Partition Query への応答（Query Reply。14 章 6.5）。
//!
//! 端末の能力（画面の大きさ、色、強調、返答の形、文字セット、日本語の DBCS）をホストに知らせる。
//! 値は広く使われている x3270 の 3279 の応答に合わせる。日本語の文字セットの符号
//! （CGCSGID）は CCSID から決める。

use yy_encoding::Ccsid;

use crate::codes::*;
use crate::emu::{Emulator, alternate_size};

/// 応答の構造化フィールド 1 つ（長さ・0x81・種類・中身）。
fn qr(code: u8, body: &[u8]) -> Vec<u8> {
    let len = (body.len() + 4) as u16;
    let mut v = len.to_be_bytes().to_vec();
    v.push(0x81);
    v.push(code);
    v.extend_from_slice(body);
    v
}

/// 文字セットの CGCSGID（GCSGID と CPGID）: 1 バイト部、2 バイト部。
fn cgcsgid(ccsid: Ccsid) -> (u32, Option<u32>) {
    match ccsid {
        Ccsid::Ibm037 => (0x02B9_0025, None),
        Ccsid::Ibm500 => (0x02B9_01F4, None),
        Ccsid::Ibm1047 => (0x02B9_0417, None),
        Ccsid::Ibm290 => (0x0494_0122, None),
        Ccsid::Ibm1027 => (0x0494_0403, None),
        // 2 バイト部は CPGID 300（JIS X 0208 相当＋IBM 拡張）
        Ccsid::Ibm930 | Ccsid::Ibm1390 => (0x0494_0122, Some(0x0370_012C)),
        Ccsid::Ibm939 | Ccsid::Ibm1399 => (0x0494_0403, Some(0x0370_012C)),
    }
}

/// Query Reply の構造化フィールドの並び（AID `0x88` から）。
pub fn reply(e: &Emulator) -> Vec<u8> {
    let (rows, cols) = alternate_size(e.model);
    let (dbcs_set, codes) = {
        let (_, db) = cgcsgid(e.ccsid);
        let mut codes = vec![
            QR_SUMMARY,
            QR_USABLE_AREA,
            QR_CHARSETS,
            QR_COLOR,
            QR_HIGHLIGHTING,
            QR_REPLY_MODES,
            QR_IMPLICIT_PART,
            QR_RPQ_NAMES,
        ];
        if db.is_some() {
            codes.push(QR_DBCS_ASIA);
        }
        (db, codes)
    };
    let mut out = vec![AID_SF];
    out.extend(qr(QR_SUMMARY, &codes));

    // 使える領域: 12/14 ビットのアドレス、桁・行、セルの大きさ、バッファの大きさ
    let mut ua = vec![0x01, 0x00];
    ua.extend_from_slice(&(cols as u16).to_be_bytes());
    ua.extend_from_slice(&(rows as u16).to_be_bytes());
    ua.extend_from_slice(&[
        0x01, 0x00, 0x0A, 0x02, 0xE5, 0x00, 0x02, 0x00, 0x6F, 0x09, 0x0C,
    ]);
    ua.extend_from_slice(&((rows * cols) as u16).to_be_bytes());
    out.extend(qr(QR_USABLE_AREA, &ua));

    // 文字セット: 1 バイト部（と 2 バイト部）
    let (sb, _) = cgcsgid(e.ccsid);
    let mut cs = Vec::new();
    if let Some(db) = dbcs_set {
        cs.extend_from_slice(&[0x8E, 0x00, 0x07, 0x07, 0x00, 0x00, 0x00, 0x00, 0x0B]);
        // 1 バイト部: SET 0、LCID 0、SW・SH なし、SUBSN なし
        cs.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        cs.extend_from_slice(&sb.to_be_bytes());
        // 2 バイト部: SET 0x80、DBCS、LCID 0xF8、SW・SH、SUBSN 0x41〜0x7F
        cs.extend_from_slice(&[0x80, 0x20, CS_DBCS, 0x0E, 0x07, 0x41, 0x7F]);
        cs.extend_from_slice(&db.to_be_bytes());
    } else {
        cs.extend_from_slice(&[0x82, 0x00, 0x07, 0x07, 0x00, 0x00, 0x00, 0x00, 0x07]);
        cs.extend_from_slice(&[0x00, 0x10, 0x00]);
        cs.extend_from_slice(&sb.to_be_bytes());
    }
    out.extend(qr(QR_CHARSETS, &cs));

    // 色: 既定（緑）と 7 色
    let mut color = vec![0x00, 0x08, 0x00, 0xF4];
    for c in 0xF1..=0xF7u8 {
        color.extend_from_slice(&[c, c]);
    }
    out.extend(qr(QR_COLOR, &color));

    // 強調: 既定・点滅・反転・下線
    out.extend(qr(
        QR_HIGHLIGHTING,
        &[
            0x04,
            0x00,
            HL_NORMAL,
            HL_BLINK,
            HL_BLINK,
            HL_REVERSE,
            HL_REVERSE,
            HL_UNDERSCORE,
            HL_UNDERSCORE,
        ],
    ));

    // 返答の形: フィールド・拡張フィールド・文字
    out.extend(qr(QR_REPLY_MODES, &[0x00, 0x01, 0x02]));

    // 暗黙のパーティション: 既定と代替の大きさ
    let mut ip = vec![0x00, 0x00, 0x0B, 0x01, 0x00];
    ip.extend_from_slice(&80u16.to_be_bytes());
    ip.extend_from_slice(&24u16.to_be_bytes());
    ip.extend_from_slice(&(cols as u16).to_be_bytes());
    ip.extend_from_slice(&(rows as u16).to_be_bytes());
    out.extend(qr(QR_IMPLICIT_PART, &ip));

    // RPQ Names: 端末のプログラムの名前（EBCDIC）
    let name: Vec<u8> = "YYTERM"
        .chars()
        .filter_map(|c| match Ccsid::Ibm037.encode_char(c) {
            Some(yy_encoding::EbcdicCode::Single(b)) => Some(b),
            _ => None,
        })
        .collect();
    let mut rpq = vec![0, 0, 0, 0, 0, 0, 0, 0, (name.len() + 1) as u8];
    rpq.extend_from_slice(&name);
    out.extend(qr(QR_RPQ_NAMES, &rpq));

    // 日本語: SO/SI を使える、2 バイト部の文字セット 0x80、入力の制御
    if dbcs_set.is_some() {
        out.extend(qr(
            QR_DBCS_ASIA,
            &[0x00, 0x03, 0x01, 0x80, 0x03, 0x02, 0x01],
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 応答を構造化フィールドに分ける（種類と中身）。
    fn split(mut v: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        while v.len() >= 4 {
            let len = usize::from(u16::from_be_bytes([v[0], v[1]]));
            assert!(len >= 4 && len <= v.len(), "{len} {}", v.len());
            assert_eq!(v[2], 0x81);
            out.push((v[3], v[4..len].to_vec()));
            v = &v[len..];
        }
        assert!(v.is_empty());
        out
    }

    #[test]
    fn builds_query_replies() {
        let e = Emulator::new(4, Ccsid::Ibm037);
        let r = reply(&e);
        assert_eq!(r[0], AID_SF);
        let sfs = split(&r[1..]);
        let kinds: Vec<u8> = sfs.iter().map(|(k, _)| *k).collect();
        // 要約に並べたものをすべて返す
        assert_eq!(kinds, sfs[0].1);
        assert!(!kinds.contains(&QR_DBCS_ASIA));
        let ua = &sfs.iter().find(|(k, _)| *k == QR_USABLE_AREA).unwrap().1;
        assert_eq!(&ua[2..6], &[0, 80, 0, 43]);

        let e = Emulator::new(2, Ccsid::Ibm930);
        let sfs = split(&reply(&e)[1..]);
        assert!(sfs.iter().any(|(k, _)| *k == QR_DBCS_ASIA));
        let cs = &sfs.iter().find(|(k, _)| *k == QR_CHARSETS).unwrap().1;
        // 記述の長さ（DL）と 2 つの記述が合う
        assert_eq!(cs[8], 0x0B);
        assert_eq!(cs.len(), 9 + 11 * 2);
        assert_eq!(cs[9 + 11 + 2], CS_DBCS);
    }
}
