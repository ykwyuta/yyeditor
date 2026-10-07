//! プリンターの印刷の組み立てのテスト。

use super::*;
use crate::codes::*;
use crate::emu::Emulator;
use yy_encoding::EbcdicCode;

/// 文字列を CCSID 930 のバイト列にする（2 バイト文字の続きは 1 組の SO〜SI）。
fn e(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut shift = false;
    for c in s.chars() {
        match Ccsid::Ibm930.encode_char(c).unwrap() {
            EbcdicCode::Single(b) => {
                if shift {
                    out.push(SCS_SI);
                    shift = false;
                }
                out.push(b);
            }
            EbcdicCode::Double(d) => {
                if !shift {
                    out.push(SCS_SO);
                    shift = true;
                }
                out.extend_from_slice(&d.to_be_bytes());
            }
        }
    }
    if shift {
        out.push(SCS_SI);
    }
    out
}

#[test]
fn scs_lines_pages_and_japanese() {
    let mut scs = Scs::new(Ccsid::Ibm930);
    assert!(scs.finish().is_none());
    let mut data = e("ABC");
    data.push(SCS_NL);
    data.extend(e("売上 一覧"));
    data.push(SCS_NL);
    data.push(SCS_NL);
    data.extend(e("X"));
    data.push(SCS_HT);
    data.extend(e("Y"));
    data.push(SCS_FF);
    data.extend(e("2 ページ目"));
    // 2 バイト文字の途中で区切って渡しても同じ
    let (a, b) = data.split_at(7);
    scs.feed(a);
    scs.feed(b);
    let job = scs.finish().unwrap();
    assert_eq!(
        job.pages,
        vec![
            vec![
                "ABC".to_owned(),
                "売上 一覧".to_owned(),
                String::new(),
                "X       Y".to_owned()
            ],
            vec!["2 ページ目".to_owned()],
        ]
    );
    assert_eq!(job.columns, DEFAULT_MPP);
    assert_eq!(job.bytes, data.len());
    assert_eq!(text_width("売上 一覧"), 9);
    // 次のジョブは空から
    assert!(scs.finish().is_none());
}

#[test]
fn scs_formats_positions_and_overprint() {
    let mut scs = Scs::new(Ccsid::Ibm037);
    let mut data = Vec::new();
    // SHF: 最大 10 桁、タブ位置 5
    data.extend([SCS_FMT, 0xC1, 0x05, 10, 1, 10, 5]);
    // SVF: 1 ページ 2 行
    data.extend([SCS_FMT, 0xC2, 0x02, 2]);
    data.extend(e("ABCDEFGHIJKL")); // 10 桁で折り返す
    data.push(SCS_NL);
    data.push(SCS_HT);
    data.extend(e("T"));
    data.push(SCS_CR);
    data.extend(e("__"));
    // 横の絶対位置 8 へ
    data.extend([SCS_PP, 0xC0, 8]);
    data.extend(e("P"));
    // 透過（TRN）と属性の設定（SA）、知らない 0x2B は飛ばす
    data.extend([
        SCS_TRN, 2, 0xC1, 0xC2, SCS_SA, 0x00, 0x00, SCS_FMT, 0xD1, 0x03, 0x00,
    ]);
    let job = {
        scs.feed(&data);
        scs.finish().unwrap()
    };
    assert_eq!(
        job.pages,
        vec![
            vec!["ABCDEFGHIJ".to_owned(), "KL".to_owned()],
            vec!["__  T  PAB".to_owned()],
        ]
    );
    assert_eq!(job.lines_per_page, 2);
    assert_eq!(job.columns, 10);
    assert_eq!(job.to_text(), "ABCDEFGHIJ\r\nKL\r\n\u{0C}__  T  PAB\r\n");
}

#[test]
fn lu3_prints_by_line_length_or_controls() {
    let mut emu = Emulator::new(2, Ccsid::Ibm930);
    // 40 桁の行: 非表示のフィールドは印刷しない
    let mut rec = vec![0xF5, WCC_START_PRINTER | 0x10];
    rec.extend(e("見出し"));
    rec.push(ORDER_SBA);
    rec.extend_from_slice(&encode_address(40));
    rec.extend(e("NEXT"));
    rec.extend([ORDER_SF, FA_PROTECT | FA_NONDISPLAY]);
    rec.extend(e("SECRET"));
    // 最後のフィールドは先頭に折り返すので、非表示のフィールドをここで終える
    rec.extend([ORDER_SF, FA_PROTECT]);
    emu.process(&rec);
    let wcc = emu.print_wcc.take().unwrap();
    let pages = lu3_pages(&emu.screen, Ccsid::Ibm930, wcc);
    assert_eq!(pages, vec![vec![" 見出し".to_owned(), "NEXT".to_owned()]]);
    // 00: NL・FF・EM で区切る（EM の後は印刷しない）
    let mut rec = vec![0xF5, WCC_START_PRINTER];
    rec.extend(e("L1"));
    rec.push(0x15);
    rec.extend(e("L2"));
    rec.push(0x0C);
    rec.extend(e("P2"));
    rec.push(0x19);
    rec.extend(e("IGNORED"));
    emu.process(&rec);
    let wcc = emu.print_wcc.take().unwrap();
    assert_eq!(
        lu3_pages(&emu.screen, Ccsid::Ibm930, wcc),
        vec![
            vec!["L1".to_owned(), "L2".to_owned()],
            vec!["P2".to_owned()]
        ]
    );
}
