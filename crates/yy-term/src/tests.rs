//! 端末の動きのテスト。

use crate::screen::{Color, flags};
use crate::term::{CursorShape, MouseMode, Pos, Terminal};

fn term(cols: usize, rows: usize) -> Terminal {
    Terminal::new(cols, rows, 100)
}

fn lines(t: &Terminal) -> Vec<String> {
    (0..t.rows()).map(|r| t.screen_row(r).text()).collect()
}

#[test]
fn prints_and_wraps() {
    let mut t = term(5, 3);
    t.feed(b"hello world");
    assert_eq!(lines(&t), ["hello", " worl", "d"]);
    assert!(t.screen_row(0).wrapped);
    assert_eq!(t.cursor(), (2, 1));
    // 行末に書いた直後は、次の文字が来るまで折り返さない
    let mut t = term(5, 3);
    t.feed(b"abcde");
    assert_eq!(t.cursor(), (0, 4));
    t.feed(b"\r\n");
    assert_eq!(t.cursor(), (1, 0));
    assert!(!t.screen_row(0).wrapped);
}

#[test]
fn utf8_and_wide_characters() {
    let mut t = term(5, 2);
    t.feed("日本語".as_bytes());
    // 3 文字目は行末に入らないので次の行へ
    assert_eq!(lines(&t), ["日本", "語"]);
    assert_eq!(t.screen_row(0).cells[1].width, 0);
    assert!(t.screen_row(0).wrapped);
    // 分割されて届いた UTF-8
    let mut t = term(10, 1);
    let bytes = "あい".as_bytes();
    t.feed(&bytes[..2]);
    t.feed(&bytes[2..]);
    assert_eq!(lines(&t), ["あい"]);
    // 不正な列
    let mut t = term(10, 1);
    t.feed(b"a\xffb\xe3\x81c");
    assert_eq!(lines(&t), ["a\u{fffd}b\u{fffd}c"]);
}

#[test]
fn combining_characters_join_the_previous_cell() {
    let mut t = term(10, 1);
    t.feed("か\u{3099}e\u{301}".as_bytes());
    let row = t.screen_row(0);
    assert_eq!(row.cells[0].text(), "か\u{3099}");
    assert_eq!(row.cells[2].text(), "e\u{301}");
    assert_eq!(t.cursor(), (0, 3));
}

#[test]
fn overwriting_half_of_a_wide_character_clears_it() {
    let mut t = term(6, 1);
    t.feed("あい".as_bytes());
    t.feed(b"\x1b[2Gx");
    assert_eq!(lines(&t), [" xい"]);
    t.feed(b"\x1b[4Gz");
    assert_eq!(lines(&t), [" x z"]);
}

#[test]
fn cursor_movement_and_erasing() {
    let mut t = term(10, 5);
    t.feed(b"0123456789\r\nabcdefghij");
    t.feed(b"\x1b[1;3H\x1b[K");
    assert_eq!(lines(&t)[0], "01");
    t.feed(b"\x1b[2;5H\x1b[1K");
    assert_eq!(lines(&t)[1], "     fghij");
    t.feed(b"\x1b[3;1Hxyz\x1b[2D\x1b[P");
    assert_eq!(lines(&t)[2], "xz");
    t.feed(b"\x1b[2@");
    assert_eq!(lines(&t)[2], "x  z");
    t.feed(b"\x1b[2X");
    assert_eq!(lines(&t)[2], "x  z");
    t.feed(b"\x1b[H\x1b[2J");
    assert!(lines(&t).iter().all(String::is_empty));
    t.feed(b"\x1b[10;20H");
    assert_eq!(t.cursor(), (4, 9));
    t.feed(b"\x1b[3A\x1b[2D");
    assert_eq!(t.cursor(), (1, 7));
}

#[test]
fn scrolls_into_the_history() {
    let mut t = term(4, 2);
    t.feed(b"1\r\n2\r\n3\r\n4");
    assert_eq!(lines(&t), ["3", "4"]);
    assert_eq!(t.history_len(), 2);
    assert_eq!(t.view_line(2, 0).text(), "1");
    assert_eq!(t.view_line(1, 1).text(), "3");
    assert_eq!(t.pushed_lines(), 2);
    // スクロールバックの上限
    let mut t = Terminal::new(4, 1, 2);
    t.feed(b"a\r\nb\r\nc\r\nd");
    assert_eq!(t.history_len(), 2);
    assert_eq!(t.first_line(), 1);
    assert_eq!(t.line(1).unwrap().text(), "b");
    assert!(t.line(0).is_none());
    // ED 3 はスクロールバックを消す（画面の行番号は変わらない）
    let before = t.screen_line(0);
    t.feed(b"\x1b[3J");
    assert_eq!(t.history_len(), 0);
    assert_eq!(t.screen_line(0), before);
}

#[test]
fn scroll_region() {
    let mut t = term(3, 5);
    t.feed(b"a\r\nb\r\nc\r\nd\r\ne");
    t.feed(b"\x1b[2;4r");
    assert_eq!(t.cursor(), (0, 0));
    t.feed(b"\x1b[4;1H\n");
    assert_eq!(lines(&t), ["a", "c", "d", "", "e"]);
    // 範囲の中のスクロールはスクロールバックに入らない
    assert_eq!(t.history_len(), 0);
    t.feed(b"\x1b[2;1H\x1bM");
    assert_eq!(lines(&t), ["a", "", "c", "d", "e"]);
    t.feed(b"\x1b[3;1H\x1b[L");
    assert_eq!(lines(&t), ["a", "", "", "c", "e"]);
    t.feed(b"\x1b[M");
    assert_eq!(lines(&t), ["a", "", "c", "", "e"]);
}

#[test]
fn colors_and_attributes() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[1;31;44mA\x1b[38;5;200;48;2;1;2;3mB\x1b[38:2::9:8:7;4:2mC\x1b[0;92mD\x1b[mE");
    let c = &t.screen_row(0).cells;
    assert_eq!(c[0].attr.fg, Color::Indexed(1));
    assert_eq!(c[0].attr.bg, Color::Indexed(4));
    assert!(c[0].attr.has(flags::BOLD));
    assert_eq!(c[1].attr.fg, Color::Indexed(200));
    assert_eq!(c[1].attr.bg, Color::Rgb(1, 2, 3));
    assert_eq!(c[2].attr.fg, Color::Rgb(9, 8, 7));
    assert!(c[2].attr.has(flags::DOUBLE_UNDERLINE));
    assert_eq!(c[3].attr.fg, Color::Indexed(10));
    assert!(!c[3].attr.has(flags::BOLD));
    assert_eq!(c[4].attr.fg, Color::Default);
    // 消去は背景色で塗る
    t.feed(b"\x1b[41m\x1b[2K");
    assert_eq!(t.screen_row(0).cells[0].attr.bg, Color::Indexed(1));
}

#[test]
fn alternate_screen_keeps_the_primary_screen() {
    let mut t = term(5, 2);
    t.feed(b"shell\r\n$ ");
    t.feed(b"\x1b[?1049h");
    assert!(t.modes().alt_screen);
    assert_eq!(lines(&t), ["", ""]);
    t.feed(b"vim\r\n~\r\n~");
    // 代替画面のスクロールはスクロールバックに入らない
    assert_eq!(t.history_len(), 0);
    t.feed(b"\x1b[?1049l");
    assert_eq!(lines(&t), ["shell", "$"]);
    assert_eq!(t.cursor(), (1, 2));
}

#[test]
fn reports_and_responses() {
    let mut t = term(10, 5);
    t.feed(b"\x1b[3;4H\x1b[6n\x1b[5n\x1b[c");
    assert_eq!(t.take_responses(), b"\x1b[3;4R\x1b[0n\x1b[?62;22c");
    assert!(t.take_responses().is_empty());
}

#[test]
fn modes_set_by_programs() {
    let mut t = term(10, 5);
    t.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?25l\x1b[5 q\x1b[?1004h");
    let m = t.modes();
    assert!(m.app_cursor && m.bracketed_paste && m.mouse_sgr && !m.cursor_visible);
    assert_eq!(m.mouse, MouseMode::ButtonMotion);
    assert_eq!(m.cursor_shape, CursorShape::Bar);
    assert_eq!(t.focus_report(true), Some(&b"\x1b[I"[..]));
    t.feed(b"\x1b[?1002l\x1b[?25h");
    assert_eq!(t.modes().mouse, MouseMode::Off);
    assert!(t.modes().cursor_visible);
    // 挿入モード
    t.feed(b"\x1b[Habc\x1b[H\x1b[4hX");
    assert_eq!(t.screen_row(0).text(), "Xabc");
}

#[test]
fn titles_and_ignored_strings() {
    let mut t = term(10, 1);
    t.feed("\x1b]0;タイトル\x07a\x1b]2;two\x1b\\b\x1bP1$r\x1b\\c\x1b]8;;http://x\x07d".as_bytes());
    assert_eq!(t.title(), "two");
    assert!(t.take_title_changed());
    assert_eq!(lines(&t), ["abcd"]);
}

#[test]
fn line_drawing_charset() {
    let mut t = term(10, 1);
    t.feed(b"\x1b(0lqk\x1b(Bx");
    assert_eq!(lines(&t), ["┌─┐x"]);
}

#[test]
fn tabs_and_backspace() {
    let mut t = term(20, 1);
    t.feed(b"a\tb\x08c\x1b[3gx\ty");
    // TBC 3 ですべてのタブ位置を消したので最後のタブは行末へ
    assert_eq!(t.screen_row(0).text(), "a       cx         y");
    let mut t = term(20, 1);
    t.feed(b"\x1b[5G\x1bH\r\tz");
    assert_eq!(t.cursor(), (0, 5));
}

#[test]
fn resizing_keeps_the_cursor_line_visible() {
    let mut t = term(10, 4);
    t.feed(b"1\r\n2\r\n3\r\n4");
    t.resize(10, 2);
    assert_eq!(lines(&t), ["3", "4"]);
    assert_eq!(t.cursor(), (1, 1));
    assert_eq!(t.history_len(), 2);
    // 広げるとスクロールバックから戻す
    t.resize(10, 3);
    assert_eq!(lines(&t), ["2", "3", "4"]);
    assert_eq!(t.cursor(), (2, 1));
    // 下が空いていれば下から削る
    let mut t = term(10, 4);
    t.feed(b"x");
    t.resize(5, 2);
    assert_eq!(lines(&t), ["x", ""]);
    assert_eq!(t.history_len(), 0);
    t.feed(b"\x1b[H123456");
    assert_eq!(lines(&t), ["12345", "6"]);
}

#[test]
fn copies_text_joining_wrapped_lines() {
    let mut t = term(5, 4);
    t.feed(b"abcdefg\r\nxy   \r\n");
    let start = Pos {
        line: t.screen_line(0),
        col: 1,
    };
    let end = Pos {
        line: t.screen_line(1),
        col: 5,
    };
    assert_eq!(t.text(start, end), "bcdefg");
    let end = Pos {
        line: t.screen_line(3),
        col: 0,
    };
    assert_eq!(t.text(start, end), "bcdefg\nxy\n");
    // 逆向きでも同じ
    assert_eq!(t.text(end, start), "bcdefg\nxy\n");
}

#[test]
fn selects_words() {
    let mut t = term(30, 1);
    t.feed("ls /usr/local (日本語) x".as_bytes());
    let line = t.screen_line(0);
    let (s, e) = t.word_at(Pos { line, col: 6 });
    assert_eq!(t.text(s, e), "/usr/local");
    let (s, e) = t.word_at(Pos { line, col: 16 });
    assert_eq!(t.text(s, e), "日本語");
}

#[test]
fn repeat_and_reset() {
    let mut t = term(10, 2);
    t.feed(b"a\x1b[3b");
    assert_eq!(lines(&t)[0], "aaaa");
    t.feed(b"\x1b[?1049h\x1b]0;t\x07\x1bc");
    assert!(!t.modes().alt_screen);
    assert_eq!(lines(&t), ["", ""]);
    assert_eq!(t.title(), "");
}

#[test]
fn origin_mode() {
    let mut t = term(5, 5);
    t.feed(b"\x1b[2;4r\x1b[?6h\x1b[1;1Hx\x1b[10;1Hy\x1b[6n");
    assert_eq!(lines(&t), ["", "x", "", "y", ""]);
    assert_eq!(t.take_responses(), b"\x1b[3;2R");
}

#[test]
fn cancel_in_the_middle_of_a_sequence() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[31\x18a\x1b[1;2\x1bbc");
    assert_eq!(lines(&t), ["ac"]);
    assert_eq!(t.screen_row(0).cells[0].attr.fg, Color::Default);
}
