//! 3270 データストリームと入力の規則のテスト。

use super::*;

/// EBCDIC（037）の文字列。
fn e(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match Ccsid::Ibm037.encode_char(c) {
            Some(EbcdicCode::Single(b)) => b,
            _ => panic!("{c}"),
        })
        .collect()
}

fn sba(row: usize, col: usize) -> Vec<u8> {
    let mut v = vec![ORDER_SBA];
    v.extend_from_slice(&encode_address(row * 80 + col));
    v
}

/// ログオン画面のような画面: 保護の見出し、非保護の利用者 ID（8 桁）、非表示のパスワード（8 桁）、
/// 数字のフィールド（4 桁）。カーソルは利用者 ID。
fn logon_screen() -> Emulator {
    let mut em = Emulator::new(2, Ccsid::Ibm037);
    let mut rec = vec![0xF5, WCC_RESTORE | WCC_RESET_MDT];
    rec.extend(sba(0, 0));
    rec.extend([ORDER_SF, FA_PROTECT | FA_INTENSIFIED]);
    rec.extend(e("LOGON"));
    rec.extend(sba(2, 0));
    rec.extend([ORDER_SF, FA_PROTECT]);
    rec.extend(e("USERID"));
    rec.extend([ORDER_SF, 0x00, ORDER_IC]);
    rec.extend(sba(2, 16));
    rec.extend([ORDER_SF, FA_PROTECT]);
    rec.extend(sba(3, 0));
    rec.extend([ORDER_SF, FA_PROTECT]);
    rec.extend(e("PASSWORD"));
    rec.extend([ORDER_SF, FA_NONDISPLAY]);
    rec.extend(sba(3, 18));
    rec.extend([ORDER_SF, FA_PROTECT]);
    rec.extend(sba(4, 0));
    rec.extend([ORDER_SF, FA_PROTECT]);
    rec.extend(e("ACCT"));
    rec.extend([ORDER_SF, FA_NUMERIC]);
    rec.extend(sba(4, 10));
    rec.extend([ORDER_SF, FA_PROTECT | FA_NUMERIC]);
    let r = em.process(&rec);
    assert!(r.changed);
    em
}

fn typed(em: &mut Emulator, s: &str) {
    for c in s.chars() {
        em.key(Key::Char(c));
    }
}

#[test]
fn erase_write_builds_fields_and_unlocks() {
    let em = logon_screen();
    assert_eq!(em.lock, Lock::None);
    let s = &em.screen;
    assert_eq!(s.text_lines(Ccsid::Ibm037)[0], " LOGON");
    assert_eq!(s.text_lines(Ccsid::Ibm037)[2], " USERID");
    // カーソルは利用者 ID の先頭（行 2 桁 8）
    assert_eq!(s.cursor, 2 * 80 + 8);
    assert!(!s.is_protected(s.cursor));
    assert!(s.is_protected(1));
}

#[test]
fn typing_sets_mdt_and_enter_sends_read_modified() {
    let mut em = logon_screen();
    typed(&mut em, "IBMUSER");
    assert_eq!(em.screen.cursor, 2 * 80 + 15);
    // Tab でパスワード、Tab で数字
    em.key(Key::Tab);
    assert_eq!(em.screen.cursor, 3 * 80 + 10);
    typed(&mut em, "SECRET");
    let r = em.key(Key::Enter);
    assert_eq!(em.lock, Lock::System);
    let d = r.data.unwrap();
    let mut want = vec![AID_ENTER];
    want.extend_from_slice(&encode_address(3 * 80 + 16));
    want.extend(sba(2, 8));
    want.extend(e("IBMUSER"));
    want.extend(sba(3, 10));
    want.extend(e("SECRET"));
    assert_eq!(d, want);
    // 非表示のフィールドは画面の文字に出ない
    assert_eq!(em.screen.text_lines(Ccsid::Ibm037)[3], " PASSWORD");
}

#[test]
fn protected_numeric_and_lock_rules() {
    let mut em = logon_screen();
    // 保護フィールドへの入力は誤り。Reset まで入力できない
    em.key(Key::MoveTo(1));
    em.key(Key::Char('X'));
    assert_eq!(em.lock, Lock::Operator(OperatorError::Protected));
    em.key(Key::MoveTo(2 * 80 + 8));
    em.key(Key::Char('A'));
    assert_eq!(em.screen.cells[2 * 80 + 8].b, 0);
    em.key(Key::Reset);
    assert_eq!(em.lock, Lock::None);
    // 数字フィールド
    em.key(Key::MoveTo(4 * 80 + 6));
    em.key(Key::Char('A'));
    assert_eq!(em.lock, Lock::Operator(OperatorError::Numeric));
    em.key(Key::Reset);
    typed(&mut em, "12");
    assert_eq!(em.lock, Lock::None);
    // システムのロック中のキーは捨てる（先打ちしない）
    em.key(Key::Enter);
    em.key(Key::Char('Z'));
    assert_eq!(em.lock, Lock::System);
    // ホストが WCC でキーボードを戻す
    em.process(&[0xF1, WCC_RESTORE]);
    assert_eq!(em.lock, Lock::None);
}

#[test]
fn auto_skip_insert_delete_and_erase() {
    let mut em = logon_screen();
    typed(&mut em, "ABCDEFGH");
    // 8 桁を埋めたら次の非保護フィールド（パスワード）へ
    assert_eq!(em.screen.cursor, 3 * 80 + 10);
    em.key(Key::Home);
    assert_eq!(em.screen.cursor, 2 * 80 + 8);
    // 挿入: いっぱいのフィールドには入らない
    em.key(Key::Insert);
    em.key(Key::Char('Z'));
    assert_eq!(em.lock, Lock::Operator(OperatorError::Overflow));
    em.key(Key::Reset);
    assert!(!em.insert);
    // 削除して空きを作り、挿入
    em.key(Key::MoveTo(2 * 80 + 9));
    em.key(Key::Delete);
    assert_eq!(field_text(&em, 2), "ACDEFGH");
    em.key(Key::Insert);
    em.key(Key::Char('B'));
    assert_eq!(field_text(&em, 2), "ABCDEFGH");
    em.key(Key::Insert);
    // Erase EOF
    em.key(Key::MoveTo(2 * 80 + 12));
    em.key(Key::EraseEof);
    assert_eq!(field_text(&em, 2), "ABCD");
    // Back Tab: フィールドの先頭、もう一度で前のフィールド
    em.key(Key::BackTab);
    assert_eq!(em.screen.cursor, 2 * 80 + 8);
    em.key(Key::BackTab);
    assert_eq!(em.screen.cursor, 4 * 80 + 6);
    // New Line: 次の行以降の最初の非保護フィールド
    em.key(Key::MoveTo(2 * 80 + 9));
    em.key(Key::NewLine);
    assert_eq!(em.screen.cursor, 3 * 80 + 10);
    // Erase Input: すべての非保護フィールドを消す
    em.key(Key::EraseInput);
    assert_eq!(field_text(&em, 2), "");
    assert_eq!(em.screen.cursor, 2 * 80 + 8);
}

/// 行 `row` の利用者 ID のフィールド（8 桁）の文字。
fn field_text(em: &Emulator, row: usize) -> String {
    em.screen.text_lines(Ccsid::Ibm037)[row]
        .chars()
        .skip(8)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[test]
fn orders_repeat_erase_tab_and_attributes() {
    let mut em = Emulator::new(2, Ccsid::Ibm037);
    let mut rec = vec![0xF5, WCC_RESTORE];
    // RA: 行 0 を '-' で埋める
    rec.push(ORDER_RA);
    rec.extend_from_slice(&encode_address(80));
    rec.extend(e("-"));
    // SFE: 非保護・赤・反転のフィールド
    rec.extend([
        ORDER_SFE,
        3,
        XA_3270,
        0x00,
        XA_FOREGROUND,
        0xF2,
        XA_HIGHLIGHTING,
        HL_REVERSE,
    ]);
    rec.extend(e("AB"));
    // SA: 黄色の文字
    rec.extend([ORDER_SA, XA_FOREGROUND, 0xF6]);
    rec.extend(e("C"));
    rec.extend([ORDER_SA, XA_ALL, 0]);
    rec.extend(sba(1, 20));
    rec.extend([ORDER_SF, FA_PROTECT]);
    rec.extend(e("END"));
    em.process(&rec);
    let d = em.screen.display(Ccsid::Ibm037);
    assert!(d[..80].iter().all(|c| c.text == "-"));
    assert_eq!(d[81].text, "A");
    assert_eq!((d[81].fg, d[81].hl), (0xF2, HL_REVERSE));
    assert_eq!(d[83].fg, 0xF6);
    // PT: フィールドの途中のデータの後ろは null で埋め、次の非保護フィールドへ
    let mut rec = vec![0xF1, 0x00];
    rec.extend(sba(1, 1));
    rec.extend(e("X"));
    rec.push(ORDER_PT);
    rec.extend(e("Y"));
    em.process(&rec);
    // 非保護フィールドは 1 つだけなので、PT は折り返して同じフィールドの先頭へ（Y が X を上書き）
    let d = em.screen.display(Ccsid::Ibm037);
    assert_eq!(d[81].text, "Y");
    assert_eq!(d[82].text, " ");
    // EUA: 非保護の位置だけを消す
    let mut rec = vec![0xF1, 0x00];
    rec.extend(sba(1, 0));
    rec.push(ORDER_EUA);
    rec.extend_from_slice(&encode_address(2 * 80));
    em.process(&rec);
    let lines = em.screen.text_lines(Ccsid::Ibm037);
    assert_eq!(lines[1].trim(), "END");
    // MF: 属性を保護に変える
    let mut rec = vec![0xF1, 0x00];
    rec.extend(sba(1, 0));
    rec.extend([ORDER_MF, 1, XA_3270, FA_PROTECT]);
    em.process(&rec);
    assert!(em.screen.is_protected(81));
}

#[test]
fn reads_buffer_and_short_reads() {
    let mut em = logon_screen();
    typed(&mut em, "U");
    // PA1・Clear は AID だけ
    let mut em2 = logon_screen();
    assert_eq!(em2.key(Key::Pa(1)).data.unwrap(), vec![AID_PA1]);
    let mut em3 = logon_screen();
    assert_eq!(em3.key(Key::Clear).data.unwrap(), vec![AID_CLEAR]);
    assert!(!em3.screen.formatted());
    // Read Buffer: AID・カーソル・SF と属性・文字
    let rb = em.process(&[0xF2]).data.unwrap();
    assert_eq!(rb[0], AID_NONE);
    assert_eq!(rb.len(), 3 + 1920 + em.screen.field_attrs().len());
    assert_eq!(
        &rb[3..5],
        &[ORDER_SF, encode_fa(FA_PROTECT | FA_INTENSIFIED)]
    );
    // Read Modified（ホストから）: 変更したフィールドだけ
    let rm = em.process(&[0xF6]).data.unwrap();
    let mut want = vec![AID_NONE];
    want.extend_from_slice(&encode_address(2 * 80 + 9));
    want.extend(sba(2, 8));
    want.extend(e("U"));
    assert_eq!(rm, want);
    // Read Partition Query: Query Reply
    let mut wsf = vec![0xF3];
    wsf.extend_from_slice(&[0x00, 0x05, SF_READ_PARTITION, 0xFF, RP_QUERY]);
    let q = em.process(&wsf).data.unwrap();
    assert_eq!(q[0], AID_SF);
    // Erase/Reset（代替の大きさ）
    let mut em4 = Emulator::new(4, Ccsid::Ibm037);
    em4.process(&[0xF3, 0x00, 0x04, SF_ERASE_RESET, 0x80]);
    assert_eq!((em4.screen.rows, em4.screen.cols), (43, 80));
    em4.process(&[0x7E, WCC_RESTORE]);
    assert_eq!(em4.screen.rows, 43);
    em4.process(&[0xF5, WCC_RESTORE]);
    assert_eq!(em4.screen.rows, 24);
}

#[test]
fn double_byte_input_adds_shift_codes() {
    let mut em = Emulator::new(2, Ccsid::Ibm930);
    // 非保護の 12 桁のフィールド
    let mut rec = vec![0xF5, WCC_RESTORE];
    rec.extend([ORDER_SF, 0x00, ORDER_IC]);
    rec.extend(sba(0, 13));
    rec.extend([ORDER_SF, FA_PROTECT]);
    em.process(&rec);
    typed(&mut em, "A漢字B");
    let line = em.screen.text_lines(Ccsid::Ibm930)[0].clone();
    assert_eq!(line.trim(), "A 漢字 B");
    // A SO 漢 字 SI B
    let bytes: Vec<u8> = em.screen.cells[1..10].iter().map(|c| c.b).collect();
    let kan = match Ccsid::Ibm930.encode_char('漢').unwrap() {
        EbcdicCode::Double(d) => d.to_be_bytes(),
        _ => panic!(),
    };
    assert_eq!(
        bytes[0],
        Ccsid::Ibm930
            .encode_char('A')
            .map(|c| match c {
                EbcdicCode::Single(b) => b,
                _ => 0,
            })
            .unwrap()
    );
    assert_eq!(bytes[1], FC_SO);
    assert_eq!(&bytes[2..4], &kan);
    assert_eq!(bytes[6], FC_SI);
    // カーソルは B の次
    assert_eq!(em.screen.cursor, 9);
    // 「漢」を消すと SO/SI の対応を保ったまま詰める
    em.key(Key::MoveTo(3));
    em.key(Key::Delete);
    assert_eq!(em.screen.text_lines(Ccsid::Ibm930)[0].trim(), "A 字 B");
    // 2 バイト文字の 2 セル目にはカーソルを置かない
    em.key(Key::MoveTo(4));
    assert_eq!(em.screen.cursor, 5);
    // 挿入で入りきらなくなったら誤り（上書きでは自動スキップで同じフィールドの先頭に戻る）
    em.key(Key::MoveTo(1));
    em.key(Key::Insert);
    typed(&mut em, "日本語の文字");
    assert_eq!(em.lock, Lock::Operator(OperatorError::Overflow));
    // 入らなかった文字は入れていない（SO/SI の対応も保つ）
    // 属性・SO・日本・SI・A・SO・字・SI・B でちょうど 12 桁
    assert_eq!(em.screen.text_lines(Ccsid::Ibm930)[0], "  日本 A 字 B");
}

#[test]
fn double_byte_fields_reject_single_bytes() {
    let mut em = Emulator::new(2, Ccsid::Ibm939);
    let mut rec = vec![0xF5, WCC_RESTORE];
    rec.extend([ORDER_SFE, 2, XA_3270, 0x00, XA_CHARSET, CS_DBCS, ORDER_IC]);
    rec.extend(sba(0, 11));
    rec.extend([ORDER_SF, FA_PROTECT]);
    em.process(&rec);
    typed(&mut em, "東京");
    assert_eq!(em.screen.text_lines(Ccsid::Ibm939)[0].trim(), "東京");
    // SO/SI は入れない
    assert!(em.screen.cells[1..5].iter().all(|c| c.b != FC_SO));
    em.key(Key::Char('a'));
    assert_eq!(em.lock, Lock::Operator(OperatorError::WrongCharset));
}

#[test]
fn paste_fills_unprotected_fields() {
    let mut em = logon_screen();
    em.paste("USER1\tPW\n");
    assert_eq!(field_text(&em, 2), "USER1");
    assert_eq!(em.screen.cursor, 4 * 80 + 6);
}
