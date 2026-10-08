//! 端末（VT100/xterm 互換の制御シーケンスの解釈と画面の状態）。
//!
//! シェルやプログラムの出力（バイト列）を [`Terminal::feed`] に渡すと、制御シーケンスを解釈して
//! 画面（[`Line`] の並び）とスクロールバックを更新する。カーソル位置の問い合わせなどへの応答は
//! [`Terminal::take_responses`] で取り出して、シェルの入力に送る。
//!
//! 解釈の状態遷移は DEC の VT500 系の構文（Paul Williams の状態図）に沿う。UTF-8 だけを扱う。

use std::collections::VecDeque;

use crate::screen::{Attr, Cell, Color, Line, char_width, flags};

/// 画面の上での位置（`line` は [`Terminal::first_line`] から数える絶対的な行番号）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: u64,
    pub col: usize,
}

/// カーソルの形。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// マウスの報告の種類（プログラムが要求したもの）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    Off,
    /// ボタンを押したときだけ（X10、`?9`）
    Press,
    /// 押す・離す（`?1000`）
    Normal,
    /// ボタンを押したままの移動も（`?1002`）
    ButtonMotion,
    /// すべての移動（`?1003`）
    AnyMotion,
}

/// プログラムが切り替えるモード。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Modes {
    /// カーソルキーをアプリケーション モードで送る（`ESC O A` など。`?1`）
    pub app_cursor: bool,
    /// テンキーのアプリケーション モード（`ESC =`）
    pub app_keypad: bool,
    /// 貼り付けを `ESC [200~` 〜 `ESC [201~` で囲む（`?2004`）
    pub bracketed_paste: bool,
    pub mouse: MouseMode,
    /// マウスの報告を SGR 形式（`ESC [<…M`）で送る（`?1006`）
    pub mouse_sgr: bool,
    /// フォーカスの出入りを知らせる（`?1004`）
    pub focus_events: bool,
    pub cursor_visible: bool,
    pub cursor_shape: CursorShape,
    /// 行末で折り返す（`?7`）
    pub autowrap: bool,
    /// カーソルの位置をスクロール範囲の中で数える（`?6`）
    pub origin: bool,
    /// 挿入モード（`4h`）
    pub insert: bool,
    /// LF で行頭にも戻る（`20h`）
    pub newline: bool,
    /// 画面全体の白黒反転（`?5`）
    pub reverse_video: bool,
    /// 代替画面（vim・less などの全画面表示）を使っている
    pub alt_screen: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            app_cursor: false,
            app_keypad: false,
            bracketed_paste: false,
            mouse: MouseMode::Off,
            mouse_sgr: false,
            focus_events: false,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            autowrap: true,
            origin: false,
            insert: false,
            newline: false,
            reverse_video: false,
            alt_screen: false,
        }
    }
}

/// 文字集合（`ESC ( 0` の罫線など）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Charset {
    #[default]
    Ascii,
    DecSpecial,
}

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    row: usize,
    col: usize,
    /// 行末に書いた直後（次の文字で折り返す）
    pending_wrap: bool,
    attr: Attr,
}

/// `ESC 7`（DECSC）で保存するもの。
#[derive(Clone, Copy, Debug, Default)]
struct Saved {
    cursor: Cursor,
    origin: bool,
    charsets: [Charset; 2],
    gl: usize,
}

/// 解釈の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    CsiIgnore,
    Osc,
    OscEscape,
    /// DCS・SOS・PM・APC（読み飛ばす）
    Str,
    StrEscape,
}

/// OSC の長さの上限
const OSC_LIMIT: usize = 4096;
/// CSI の引数の数の上限
const PARAM_LIMIT: usize = 32;
/// 結合文字の長さの上限（バイト）
const COMBINING_LIMIT: usize = 32;

/// 端末。
pub struct Terminal {
    cols: usize,
    rows: usize,
    grid: Vec<Line>,
    /// 代替画面を使っている間の、元の画面
    primary: Option<Vec<Line>>,
    scrollback: VecDeque<Line>,
    scrollback_limit: usize,
    /// スクロールバックの先頭から捨てた行の数（行番号の基準）
    dropped: u64,
    /// スクロールバックに送った行の数の累計
    pushed: u64,
    cursor: Cursor,
    saved: Saved,
    saved_alt: Saved,
    top: usize,
    bottom: usize,
    tabs: Vec<bool>,
    modes: Modes,
    charsets: [Charset; 2],
    gl: usize,
    title: String,
    title_changed: bool,
    /// シェルが知らせた作業フォルダ（OSC 7 の `file://…`・OSC 9;9）
    cwd: Option<String>,
    bell: bool,
    dirty: bool,
    responses: Vec<u8>,
    last_char: Option<char>,
    /// 東アジアの幅が曖昧な文字を全角とする
    pub ambiguous_wide: bool,

    state: State,
    params: Vec<Vec<u32>>,
    private: Option<u8>,
    intermediates: Vec<u8>,
    osc: Vec<u8>,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_need: usize,
}

impl Terminal {
    /// `cols` × `rows` の端末。スクロールバックは `scrollback` 行まで残す。
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Terminal {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Terminal {
            cols,
            rows,
            grid: (0..rows)
                .map(|_| Line::new(cols, Attr::default()))
                .collect(),
            primary: None,
            scrollback: VecDeque::new(),
            scrollback_limit: scrollback,
            dropped: 0,
            pushed: 0,
            cursor: Cursor::default(),
            saved: Saved::default(),
            saved_alt: Saved::default(),
            top: 0,
            bottom: rows - 1,
            tabs: default_tabs(cols),
            modes: Modes::default(),
            charsets: [Charset::Ascii; 2],
            gl: 0,
            title: String::new(),
            title_changed: false,
            cwd: None,
            bell: false,
            dirty: true,
            responses: Vec::new(),
            last_char: None,
            ambiguous_wide: false,
            state: State::Ground,
            params: Vec::new(),
            private: None,
            intermediates: Vec::new(),
            osc: Vec::new(),
            utf8: [0; 4],
            utf8_len: 0,
            utf8_need: 0,
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn modes(&self) -> &Modes {
        &self.modes
    }

    /// カーソルの位置（画面の中の `(行, 桁)`）。
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor.row, self.cursor.col)
    }

    /// ウィンドウのタイトル（OSC 0・2 で設定されたもの）。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// タイトルが変わったか（読むと元に戻る）。
    pub fn take_title_changed(&mut self) -> bool {
        std::mem::take(&mut self.title_changed)
    }

    /// ベルが鳴ったか（読むと元に戻る）。
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    /// 画面が変わったか（読むと元に戻る）。
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// 問い合わせへの応答（シェルの入力に送る）。
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.responses)
    }

    /// スクロールバックの行数。
    pub fn history_len(&self) -> usize {
        self.scrollback.len()
    }

    /// スクロールバックに送った行の数の累計（表示位置を保つのに使う）。
    pub fn pushed_lines(&self) -> u64 {
        self.pushed
    }

    /// いちばん古い行（スクロールバックの先頭）の行番号。
    pub fn first_line(&self) -> u64 {
        self.dropped
    }

    /// いちばん新しい行の次の行番号。
    pub fn end_line(&self) -> u64 {
        self.dropped + (self.scrollback.len() + self.rows) as u64
    }

    /// 画面の `row` 行目の行番号。
    pub fn screen_line(&self, row: usize) -> u64 {
        self.dropped + (self.scrollback.len() + row) as u64
    }

    /// 行番号 `line` の行（スクロールバックか画面）。
    pub fn line(&self, line: u64) -> Option<&Line> {
        let i = usize::try_from(line.checked_sub(self.dropped)?).ok()?;
        if i < self.scrollback.len() {
            self.scrollback.get(i)
        } else {
            self.grid.get(i - self.scrollback.len())
        }
    }

    /// `back` 行さかのぼって表示しているときの、表示の `row` 行目。
    pub fn view_line(&self, back: usize, row: usize) -> &Line {
        let back = back.min(self.scrollback.len());
        let i = self.scrollback.len() - back + row;
        if i < self.scrollback.len() {
            &self.scrollback[i]
        } else {
            &self.grid[(i - self.scrollback.len()).min(self.rows - 1)]
        }
    }

    /// 画面の `row` 行目。
    pub fn screen_row(&self, row: usize) -> &Line {
        &self.grid[row]
    }

    /// フォーカスの出入りの報告（要求されていなければ `None`）。
    pub fn focus_report(&self, focused: bool) -> Option<&'static [u8]> {
        self.modes
            .focus_events
            .then_some(if focused { b"\x1b[I" } else { b"\x1b[O" })
    }

    // ---- 大きさ ---------------------------------------------------------------

    /// 大きさを変える（行の折り返しはやり直さない）。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let alt = self.modes.alt_screen;
        for l in &mut self.grid {
            l.resize(cols);
        }
        if let Some(p) = &mut self.primary {
            for l in p.iter_mut() {
                l.resize(cols);
            }
            resize_rows_simple(p, rows, cols);
        }
        if rows < self.rows {
            let mut excess = self.rows - rows;
            // カーソルより下の空いた行から消す
            while excess > 0
                && self.grid.len() > self.cursor.row + 1
                && is_blank(self.grid.last().expect("non-empty"))
            {
                self.grid.pop();
                excess -= 1;
            }
            // 残りは上から消す（通常の画面ではスクロールバックへ）
            for _ in 0..excess {
                let l = self.grid.remove(0);
                if !alt {
                    self.push_history(l);
                }
                self.cursor.row = self.cursor.row.saturating_sub(1);
            }
            self.grid.truncate(rows);
        } else if rows > self.rows {
            let mut add = rows - self.rows;
            if !alt {
                // スクロールバックから戻す
                while add > 0 {
                    let Some(mut l) = self.scrollback.pop_back() else {
                        break;
                    };
                    l.resize(cols);
                    self.grid.insert(0, l);
                    self.cursor.row += 1;
                    add -= 1;
                }
            }
            for _ in 0..add {
                self.grid.push(Line::new(cols, Attr::default()));
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.tabs = default_tabs(cols);
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.pending_wrap = false;
        self.dirty = true;
    }

    // ---- 入力 -----------------------------------------------------------------

    /// シェル・プログラムの出力を解釈する。
    pub fn feed(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.dirty = true;
        }
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        // CAN・SUB はどの状態からでも中止する
        if matches!(b, 0x18 | 0x1a) && self.state != State::Ground {
            self.state = State::Ground;
            return;
        }
        match self.state {
            State::Ground => self.ground(b),
            State::Escape => match b {
                0x1b => self.enter_escape(),
                b'[' => {
                    self.params.clear();
                    self.private = None;
                    self.intermediates.clear();
                    self.state = State::Csi;
                }
                b']' => {
                    self.osc.clear();
                    self.state = State::Osc;
                }
                b'P' | b'X' | b'^' | b'_' => self.state = State::Str,
                0x20..=0x2f => {
                    self.intermediates.push(b);
                    self.state = State::EscapeIntermediate;
                }
                0x30..=0x7e => {
                    self.state = State::Ground;
                    self.esc_dispatch(b);
                }
                0x00..=0x1f => self.control(b),
                _ => self.state = State::Ground,
            },
            State::EscapeIntermediate => match b {
                0x20..=0x2f => self.intermediates.push(b),
                0x30..=0x7e => {
                    self.state = State::Ground;
                    self.esc_dispatch(b);
                }
                0x1b => self.enter_escape(),
                0x00..=0x1f => self.control(b),
                _ => self.state = State::Ground,
            },
            State::Csi => match b {
                b'0'..=b'9' => {
                    if self.params.is_empty() {
                        self.params.push(vec![0]);
                    }
                    let p = self.params.last_mut().expect("non-empty");
                    let v = p.last_mut().expect("non-empty");
                    *v = v.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                }
                b';' => {
                    if self.params.is_empty() {
                        self.params.push(vec![0]);
                    }
                    if self.params.len() < PARAM_LIMIT {
                        self.params.push(vec![0]);
                    }
                }
                b':' => {
                    if self.params.is_empty() {
                        self.params.push(vec![0]);
                    }
                    let p = self.params.last_mut().expect("non-empty");
                    if p.len() < 8 {
                        p.push(0);
                    }
                }
                0x3c..=0x3f => {
                    if self.params.is_empty()
                        && self.private.is_none()
                        && self.intermediates.is_empty()
                    {
                        self.private = Some(b);
                    } else {
                        self.state = State::CsiIgnore;
                    }
                }
                0x20..=0x2f => self.intermediates.push(b),
                0x40..=0x7e => {
                    self.state = State::Ground;
                    self.csi_dispatch(b);
                }
                0x1b => self.enter_escape(),
                0x00..=0x1f => self.control(b),
                _ => self.state = State::CsiIgnore,
            },
            State::CsiIgnore => match b {
                0x40..=0x7e => self.state = State::Ground,
                0x1b => self.enter_escape(),
                0x00..=0x1f => self.control(b),
                _ => {}
            },
            State::Osc => match b {
                0x07 => {
                    self.state = State::Ground;
                    self.osc_dispatch();
                }
                0x1b => self.state = State::OscEscape,
                _ => {
                    if self.osc.len() < OSC_LIMIT {
                        self.osc.push(b);
                    }
                }
            },
            State::OscEscape => {
                self.osc_dispatch();
                if b == b'\\' {
                    self.state = State::Ground;
                } else {
                    self.enter_escape();
                    self.byte(b);
                }
            }
            State::Str => match b {
                0x1b => self.state = State::StrEscape,
                0x07 => self.state = State::Ground,
                _ => {}
            },
            State::StrEscape => {
                if b == b'\\' {
                    self.state = State::Ground;
                } else {
                    self.enter_escape();
                    self.byte(b);
                }
            }
        }
    }

    fn enter_escape(&mut self) {
        self.intermediates.clear();
        self.state = State::Escape;
    }

    /// 通常の状態のバイト（UTF-8 を組み立てる）。
    fn ground(&mut self, b: u8) {
        if self.utf8_need > 0 {
            if (0x80..=0xbf).contains(&b) {
                self.utf8[self.utf8_len] = b;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_need {
                    let c = std::str::from_utf8(&self.utf8[..self.utf8_len])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('\u{fffd}');
                    self.utf8_need = 0;
                    self.utf8_len = 0;
                    self.print(c);
                }
                return;
            }
            // 途中で切れた
            self.utf8_need = 0;
            self.utf8_len = 0;
            self.print('\u{fffd}');
        }
        match b {
            0x1b => self.enter_escape(),
            0x00..=0x1f => self.control(b),
            0x7f => {}
            0x20..=0x7e => self.print(b as char),
            0xc2..=0xf4 => {
                self.utf8[0] = b;
                self.utf8_len = 1;
                self.utf8_need = match b {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
            }
            _ => self.print('\u{fffd}'),
        }
    }

    /// C0 制御文字。
    fn control(&mut self, b: u8) {
        match b {
            0x07 => self.bell = true,
            0x08 => {
                self.cursor.pending_wrap = false;
                self.cursor.col = self.cursor.col.saturating_sub(1);
            }
            0x09 => self.tab_forward(1),
            0x0a..=0x0c => {
                self.linefeed();
                if self.modes.newline {
                    self.cursor.col = 0;
                }
            }
            0x0d => {
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            0x0e => self.gl = 1,
            0x0f => self.gl = 0,
            _ => {}
        }
    }

    // ---- 文字の表示 -------------------------------------------------------------

    fn print(&mut self, c: char) {
        let c = if self.charsets[self.gl] == Charset::DecSpecial {
            dec_special(c)
        } else {
            c
        };
        let w = char_width(c, self.ambiguous_wide);
        if w == 0 {
            self.combine(c);
            return;
        }
        let cols = self.cols;
        if w > cols {
            return;
        }
        if self.cursor.pending_wrap {
            self.cursor.pending_wrap = false;
            if self.modes.autowrap {
                self.grid[self.cursor.row].wrapped = true;
                self.linefeed();
                self.cursor.col = 0;
            }
        }
        if self.cursor.col + w > cols {
            if self.modes.autowrap {
                // 全角の文字が入らない: 行末を空けて次の行へ
                let row = self.cursor.row;
                let col = self.cursor.col;
                self.grid[row].split_wide_at(col);
                self.grid[row].cells[col] = Cell::blank(self.cursor.attr);
                self.grid[row].wrapped = true;
                self.linefeed();
                self.cursor.col = 0;
            } else {
                self.cursor.col = cols - w;
            }
        }
        if self.modes.insert {
            self.insert_blanks(w);
        }
        let (row, col) = (self.cursor.row, self.cursor.col);
        let attr = self.cursor.attr;
        let line = &mut self.grid[row];
        line.split_wide_at(col);
        if w == 2 {
            line.split_wide_at(col + 1);
        }
        line.cells[col] = Cell {
            ch: c,
            combining: None,
            width: w as u8,
            attr,
        };
        if w == 2 {
            line.cells[col + 1] = Cell {
                ch: ' ',
                combining: None,
                width: 0,
                attr,
            };
        }
        self.cursor.col += w;
        if self.cursor.col >= cols {
            self.cursor.col = cols - 1;
            if self.modes.autowrap {
                self.cursor.pending_wrap = true;
            }
        }
        self.last_char = Some(c);
    }

    /// 幅のない文字（結合文字など）を直前の文字に付ける。
    fn combine(&mut self, c: char) {
        let row = self.cursor.row;
        let mut col = if self.cursor.pending_wrap {
            self.cursor.col
        } else if self.cursor.col > 0 {
            self.cursor.col - 1
        } else {
            return;
        };
        let line = &mut self.grid[row];
        if line.cells[col].is_continuation() && col > 0 {
            col -= 1;
        }
        let cell = &mut line.cells[col];
        let mut s = cell.combining.take().map(String::from).unwrap_or_default();
        if s.len() + c.len_utf8() <= COMBINING_LIMIT {
            s.push(c);
        }
        cell.combining = Some(s.into_boxed_str());
    }

    // ---- カーソルの移動とスクロール ---------------------------------------------

    fn linefeed(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.bottom {
            self.scroll_up(1);
        } else if self.cursor.row < self.rows - 1 {
            self.cursor.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.top {
            self.scroll_down(1);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
    }

    fn blank_line(&self) -> Line {
        Line::new(self.cols, self.cursor.attr)
    }

    fn push_history(&mut self, line: Line) {
        if self.scrollback_limit == 0 {
            self.dropped += 1;
            return;
        }
        if self.scrollback.len() >= self.scrollback_limit {
            self.scrollback.pop_front();
            self.dropped += 1;
        }
        self.scrollback.push_back(line);
        self.pushed += 1;
    }

    /// スクロール範囲を `n` 行上へ送る（範囲が画面の上端からなら、送り出した行はスクロールバックへ）。
    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        for _ in 0..n {
            let l = self.grid.remove(self.top);
            if self.top == 0 && !self.modes.alt_screen {
                self.push_history(l);
            }
            let blank = self.blank_line();
            self.grid.insert(self.bottom, blank);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        for _ in 0..n {
            self.grid.remove(self.bottom);
            let blank = self.blank_line();
            self.grid.insert(self.top, blank);
        }
    }

    fn tab_forward(&mut self, n: usize) {
        self.cursor.pending_wrap = false;
        for _ in 0..n {
            let mut c = self.cursor.col + 1;
            while c < self.cols - 1 && !self.tabs[c] {
                c += 1;
            }
            self.cursor.col = c.min(self.cols - 1);
        }
    }

    fn tab_backward(&mut self, n: usize) {
        self.cursor.pending_wrap = false;
        for _ in 0..n {
            let mut c = self.cursor.col.saturating_sub(1);
            while c > 0 && !self.tabs[c] {
                c -= 1;
            }
            self.cursor.col = c;
        }
    }

    /// カーソルを `(row, col)` へ（origin モードではスクロール範囲の中で数える）。
    fn goto(&mut self, row: usize, col: usize) {
        let (lo, hi) = if self.modes.origin {
            (self.top, self.bottom)
        } else {
            (0, self.rows - 1)
        };
        self.cursor.row = (lo + row).min(hi);
        self.cursor.col = col.min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    fn insert_blanks(&mut self, n: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let blank = Cell::blank(self.cursor.attr);
        let line = &mut self.grid[row];
        line.split_wide_at(col);
        let n = n.min(self.cols - col);
        for _ in 0..n {
            line.cells.pop();
            line.cells.insert(col, blank.clone());
        }
        // 右端で切れた全角の文字
        if let Some(last) = line.cells.last_mut()
            && last.width == 2
        {
            *last = blank;
        }
    }

    fn delete_chars(&mut self, n: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let blank = Cell::blank(self.cursor.attr);
        let line = &mut self.grid[row];
        line.split_wide_at(col);
        let n = n.min(self.cols - col);
        if col + n < self.cols {
            line.split_wide_at(col + n);
        }
        for _ in 0..n {
            line.cells.remove(col);
            line.cells.push(blank.clone());
        }
        self.cursor.pending_wrap = false;
    }

    fn erase_chars(&mut self, row: usize, from: usize, to: usize) {
        let blank = Cell::blank(self.cursor.attr);
        let line = &mut self.grid[row];
        let to = to.min(self.cols);
        if from >= to {
            return;
        }
        line.split_wide_at(from);
        if to < self.cols {
            line.split_wide_at(to);
        }
        for c in &mut line.cells[from..to] {
            *c = blank.clone();
        }
    }

    fn erase_line(&mut self, row: usize) {
        self.grid[row] = self.blank_line();
    }

    // ---- ESC・CSI・OSC --------------------------------------------------------

    fn esc_dispatch(&mut self, b: u8) {
        let inter = std::mem::take(&mut self.intermediates);
        match (inter.as_slice(), b) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.linefeed(),
            ([], b'E') => {
                self.linefeed();
                self.cursor.col = 0;
            }
            ([], b'H') => self.tabs[self.cursor.col] = true,
            ([], b'M') => self.reverse_index(),
            ([], b'c') => self.full_reset(),
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([b'('], f) => self.charsets[0] = charset(f),
            ([b')'], f) => self.charsets[1] = charset(f),
            ([b'#'], b'8') => {
                for l in &mut self.grid {
                    for c in &mut l.cells {
                        *c = Cell {
                            ch: 'E',
                            ..Cell::default()
                        };
                    }
                }
            }
            _ => {}
        }
    }

    fn param(&self, i: usize, default: u32) -> u32 {
        match self.params.get(i).and_then(|p| p.first()) {
            Some(&0) | None => default,
            Some(&v) => v,
        }
    }

    fn csi_dispatch(&mut self, b: u8) {
        let n = self.param(0, 1) as usize;
        let inter = std::mem::take(&mut self.intermediates);
        let private = self.private;
        match (private, inter.as_slice(), b) {
            (None, [], b'@') => self.insert_blanks(n),
            (None, [], b'A') => {
                let lo = if self.cursor.row >= self.top {
                    self.top
                } else {
                    0
                };
                self.cursor.row = self.cursor.row.saturating_sub(n).max(lo);
                self.cursor.pending_wrap = false;
            }
            (None, [], b'B' | b'e') => {
                let hi = if self.cursor.row <= self.bottom {
                    self.bottom
                } else {
                    self.rows - 1
                };
                self.cursor.row = (self.cursor.row + n).min(hi);
                self.cursor.pending_wrap = false;
            }
            (None, [], b'C' | b'a') => {
                self.cursor.col = (self.cursor.col + n).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            (None, [], b'D') => {
                self.cursor.col = self.cursor.col.saturating_sub(n);
                self.cursor.pending_wrap = false;
            }
            (None, [], b'E') => {
                let hi = if self.cursor.row <= self.bottom {
                    self.bottom
                } else {
                    self.rows - 1
                };
                self.cursor.row = (self.cursor.row + n).min(hi);
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            (None, [], b'F') => {
                let lo = if self.cursor.row >= self.top {
                    self.top
                } else {
                    0
                };
                self.cursor.row = self.cursor.row.saturating_sub(n).max(lo);
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            (None, [], b'G' | b'`') => {
                self.cursor.col = (n - 1).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            (None, [], b'H' | b'f') => {
                let row = self.param(0, 1) as usize - 1;
                let col = self.param(1, 1) as usize - 1;
                self.goto(row, col);
            }
            (None, [], b'I') => self.tab_forward(n),
            (None | Some(b'?'), [], b'J') => self.erase_display(self.param(0, 0)),
            (None | Some(b'?'), [], b'K') => {
                let row = self.cursor.row;
                match self.param(0, 0) {
                    0 => self.erase_chars(row, self.cursor.col, self.cols),
                    1 => self.erase_chars(row, 0, self.cursor.col + 1),
                    2 => self.erase_line(row),
                    _ => {}
                }
                self.grid[row].wrapped = false;
            }
            (None, [], b'L') => {
                if (self.top..=self.bottom).contains(&self.cursor.row) {
                    let n = n.min(self.bottom - self.cursor.row + 1);
                    for _ in 0..n {
                        self.grid.remove(self.bottom);
                        let blank = self.blank_line();
                        self.grid.insert(self.cursor.row, blank);
                    }
                    self.cursor.col = 0;
                    self.cursor.pending_wrap = false;
                }
            }
            (None, [], b'M') => {
                if (self.top..=self.bottom).contains(&self.cursor.row) {
                    let n = n.min(self.bottom - self.cursor.row + 1);
                    for _ in 0..n {
                        self.grid.remove(self.cursor.row);
                        let blank = self.blank_line();
                        self.grid.insert(self.bottom, blank);
                    }
                    self.cursor.col = 0;
                    self.cursor.pending_wrap = false;
                }
            }
            (None, [], b'P') => self.delete_chars(n),
            (None, [], b'S') => self.scroll_up(n),
            (None, [], b'T') if self.params.len() <= 1 => self.scroll_down(n),
            (None, [], b'X') => {
                let (row, col) = (self.cursor.row, self.cursor.col);
                self.erase_chars(row, col, col + n);
                self.cursor.pending_wrap = false;
            }
            (None, [], b'Z') => self.tab_backward(n),
            (None, [], b'b') => {
                if let Some(c) = self.last_char {
                    for _ in 0..n.min(65_535) {
                        self.print(c);
                    }
                }
            }
            (None, [], b'c') if self.param(0, 0) == 0 => {
                // VT220 相当、ANSI の色
                self.responses.extend_from_slice(b"\x1b[?62;22c");
            }
            (Some(b'>'), [], b'c') if self.param(0, 0) == 0 => {
                self.responses.extend_from_slice(b"\x1b[>1;10;0c");
            }
            (None, [], b'd') => {
                let col = self.cursor.col;
                self.goto(n - 1, col);
            }
            (None, [], b'g') => match self.param(0, 0) {
                0 => self.tabs[self.cursor.col] = false,
                3 => self.tabs.iter_mut().for_each(|t| *t = false),
                _ => {}
            },
            (None, [], b'h') => self.set_ansi_modes(true),
            (None, [], b'l') => self.set_ansi_modes(false),
            (Some(b'?'), [], b'h') => self.set_dec_modes(true),
            (Some(b'?'), [], b'l') => self.set_dec_modes(false),
            (None, [], b'm') => self.sgr(),
            (None, [], b'n') => match self.param(0, 0) {
                5 => self.responses.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let (r, c) = self.report_position();
                    self.responses
                        .extend_from_slice(format!("\x1b[{r};{c}R").as_bytes());
                }
                _ => {}
            },
            (Some(b'?'), [], b'n') if self.param(0, 0) == 6 => {
                let (r, c) = self.report_position();
                self.responses
                    .extend_from_slice(format!("\x1b[?{r};{c}R").as_bytes());
            }
            (None, [], b'r') => {
                let top = self.param(0, 1) as usize - 1;
                let bottom = (self.param(1, self.rows as u32) as usize).min(self.rows) - 1;
                if top < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    self.goto(0, 0);
                }
            }
            (None, [], b's') => self.save_cursor(),
            (None, [], b'u') => self.restore_cursor(),
            (None, [b' '], b'q') => {
                self.modes.cursor_shape = match self.param(0, 0) {
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Bar,
                    _ => CursorShape::Block,
                };
            }
            (None, [b'!'], b'p') => self.soft_reset(),
            _ => {}
        }
        self.private = None;
    }

    /// カーソル位置の報告に使う `(行, 桁)`（1 始まり）。
    fn report_position(&self) -> (usize, usize) {
        let row = if self.modes.origin {
            self.cursor.row.saturating_sub(self.top)
        } else {
            self.cursor.row
        };
        (row + 1, self.cursor.col + 1)
    }

    fn erase_display(&mut self, mode: u32) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        match mode {
            0 => {
                self.erase_chars(row, col, self.cols);
                self.grid[row].wrapped = false;
                for r in row + 1..self.rows {
                    self.erase_line(r);
                }
            }
            1 => {
                for r in 0..row {
                    self.erase_line(r);
                }
                self.erase_chars(row, 0, col + 1);
            }
            2 => {
                for r in 0..self.rows {
                    self.erase_line(r);
                }
            }
            3 => {
                self.dropped += self.scrollback.len() as u64;
                self.scrollback.clear();
            }
            _ => {}
        }
    }

    fn set_ansi_modes(&mut self, on: bool) {
        for i in 0..self.params.len() {
            match self.param(i, 0) {
                4 => self.modes.insert = on,
                20 => self.modes.newline = on,
                _ => {}
            }
        }
    }

    fn set_dec_modes(&mut self, on: bool) {
        for i in 0..self.params.len().max(1) {
            match self.param(i, 0) {
                1 => self.modes.app_cursor = on,
                5 => self.modes.reverse_video = on,
                6 => {
                    self.modes.origin = on;
                    self.goto(0, 0);
                }
                7 => self.modes.autowrap = on,
                9 => self.modes.mouse = if on { MouseMode::Press } else { MouseMode::Off },
                25 => self.modes.cursor_visible = on,
                47 | 1047 => self.switch_screen(on, false),
                1048 => {
                    if on {
                        self.save_cursor();
                    } else {
                        self.restore_cursor();
                    }
                }
                1049 => {
                    if on {
                        self.save_cursor();
                        self.switch_screen(true, true);
                    } else {
                        self.switch_screen(false, false);
                        self.restore_cursor();
                    }
                }
                1000 => {
                    self.modes.mouse = if on {
                        MouseMode::Normal
                    } else {
                        MouseMode::Off
                    }
                }
                1002 => {
                    self.modes.mouse = if on {
                        MouseMode::ButtonMotion
                    } else {
                        MouseMode::Off
                    }
                }
                1003 => {
                    self.modes.mouse = if on {
                        MouseMode::AnyMotion
                    } else {
                        MouseMode::Off
                    }
                }
                1004 => self.modes.focus_events = on,
                1006 => self.modes.mouse_sgr = on,
                2004 => self.modes.bracketed_paste = on,
                _ => {}
            }
        }
    }

    /// 代替画面に切り替える・戻る。
    fn switch_screen(&mut self, alt: bool, clear: bool) {
        if alt == self.modes.alt_screen {
            if alt && clear {
                for r in 0..self.rows {
                    self.erase_line(r);
                }
            }
            return;
        }
        if alt {
            let blank: Vec<Line> = (0..self.rows)
                .map(|_| Line::new(self.cols, Attr::default()))
                .collect();
            self.primary = Some(std::mem::replace(&mut self.grid, blank));
            std::mem::swap(&mut self.saved, &mut self.saved_alt);
        } else if let Some(p) = self.primary.take() {
            self.grid = p;
            std::mem::swap(&mut self.saved, &mut self.saved_alt);
        }
        self.modes.alt_screen = alt;
        self.cursor.row = self.cursor.row.min(self.rows - 1);
        self.cursor.pending_wrap = false;
    }

    fn save_cursor(&mut self) {
        self.saved = Saved {
            cursor: self.cursor,
            origin: self.modes.origin,
            charsets: self.charsets,
            gl: self.gl,
        };
    }

    fn restore_cursor(&mut self) {
        let s = self.saved;
        self.cursor = s.cursor;
        self.cursor.row = self.cursor.row.min(self.rows - 1);
        self.cursor.col = self.cursor.col.min(self.cols - 1);
        self.modes.origin = s.origin;
        self.charsets = s.charsets;
        self.gl = s.gl;
    }

    fn soft_reset(&mut self) {
        self.modes.insert = false;
        self.modes.origin = false;
        self.modes.autowrap = true;
        self.modes.cursor_visible = true;
        self.modes.app_cursor = false;
        self.modes.app_keypad = false;
        self.top = 0;
        self.bottom = self.rows - 1;
        self.cursor.attr = Attr::default();
        self.cursor.pending_wrap = false;
        self.charsets = [Charset::Ascii; 2];
        self.gl = 0;
        self.saved = Saved::default();
    }

    fn full_reset(&mut self) {
        if self.modes.alt_screen {
            self.switch_screen(false, false);
        }
        self.modes = Modes::default();
        self.soft_reset();
        self.cursor = Cursor::default();
        for r in 0..self.rows {
            self.erase_line(r);
        }
        self.tabs = default_tabs(self.cols);
        self.title.clear();
        self.title_changed = true;
    }

    /// SGR（文字の色と飾り）。
    fn sgr(&mut self) {
        if self.params.is_empty() {
            self.cursor.attr = Attr::default();
            return;
        }
        let params = std::mem::take(&mut self.params);
        let a = &mut self.cursor.attr;
        let mut i = 0;
        while i < params.len() {
            let p = &params[i];
            let code = p[0];
            match code {
                0 => *a = Attr::default(),
                1 => a.flags |= flags::BOLD,
                2 => a.flags |= flags::DIM,
                3 => a.flags |= flags::ITALIC,
                4 => {
                    a.flags &= !(flags::UNDERLINE | flags::DOUBLE_UNDERLINE);
                    match p.get(1) {
                        Some(0) => {}
                        Some(2) => a.flags |= flags::DOUBLE_UNDERLINE,
                        _ => a.flags |= flags::UNDERLINE,
                    }
                }
                5 | 6 => a.flags |= flags::BLINK,
                7 => a.flags |= flags::INVERSE,
                8 => a.flags |= flags::HIDDEN,
                9 => a.flags |= flags::STRIKE,
                21 => a.flags |= flags::DOUBLE_UNDERLINE,
                22 => a.flags &= !(flags::BOLD | flags::DIM),
                23 => a.flags &= !flags::ITALIC,
                24 => a.flags &= !(flags::UNDERLINE | flags::DOUBLE_UNDERLINE),
                25 => a.flags &= !flags::BLINK,
                27 => a.flags &= !flags::INVERSE,
                28 => a.flags &= !flags::HIDDEN,
                29 => a.flags &= !flags::STRIKE,
                30..=37 => a.fg = Color::Indexed((code - 30) as u8),
                39 => a.fg = Color::Default,
                40..=47 => a.bg = Color::Indexed((code - 40) as u8),
                49 => a.bg = Color::Default,
                90..=97 => a.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => a.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 | 58 => {
                    let (color, used) = extended_color(&params, i);
                    i += used;
                    match (code, color) {
                        (38, Some(c)) => a.fg = c,
                        (48, Some(c)) => a.bg = c,
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn osc_dispatch(&mut self) {
        let data = std::mem::take(&mut self.osc);
        let text = String::from_utf8_lossy(&data);
        let (code, rest) = text.split_once(';').unwrap_or((&text, ""));
        if matches!(code, "0" | "2") {
            let title: String = rest.chars().filter(|c| !c.is_control()).take(256).collect();
            if title != self.title {
                self.title = title;
                self.title_changed = true;
            }
        }
        // 作業フォルダ: OSC 7（`file://ホスト/パス`）、OSC 9;9（Windows Terminal・ConEmu の形）
        if code == "7"
            && let Some(p) = crate::links::file_url_path(rest)
        {
            self.cwd = Some(p);
        }
        if code == "9"
            && let Some(p) = rest.strip_prefix("9;")
        {
            let p = p.trim_matches('"');
            if !p.is_empty() {
                self.cwd = Some(p.to_string());
            }
        }
    }

    // ---- リンク・作業フォルダ ---------------------------------------------------

    /// シェルが知らせた作業フォルダ（OSC 7・OSC 9;9。知らせていなければ `None`）。
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// 行 `line` を含む、折り返しでつながった行の文字と、文字ごとの位置（行・始めの桁・終わりの桁）。
    fn logical_line(&self, line: u64) -> (Vec<char>, Vec<(u64, usize, usize)>) {
        let mut first = line;
        while first > self.dropped && self.line(first - 1).is_some_and(|l| l.wrapped) {
            first -= 1;
        }
        let mut chars = Vec::new();
        let mut map = Vec::new();
        let mut n = first;
        while let Some(l) = self.line(n) {
            for (col, c) in l.cells.iter().enumerate() {
                if c.is_continuation() {
                    continue;
                }
                let w = usize::from(c.width.max(1));
                // 結合文字は前の文字と同じ位置
                for ch in c.text().chars() {
                    chars.push(ch);
                    map.push((n, col, col + w));
                }
            }
            if !l.wrapped {
                break;
            }
            n += 1;
        }
        (chars, map)
    }

    /// `pos` にあるリンク（URL・パス）と、その範囲（行ごとの `(始め, 終わり)`。終わりは含まない）。
    pub fn link_at(&self, pos: Pos) -> Option<(crate::links::LinkTarget, Vec<(Pos, Pos)>)> {
        let (chars, map) = self.logical_line(pos.line);
        let i = map
            .iter()
            .position(|&(l, a, b)| l == pos.line && pos.col >= a && pos.col < b)?;
        let text: String = chars.iter().collect();
        let found = crate::links::find(&text)
            .into_iter()
            .find(|f| f.start <= i && i < f.end)?;
        let mut ranges: Vec<(Pos, Pos)> = Vec::new();
        for &(l, a, b) in &map[found.start..found.end] {
            match ranges.last_mut() {
                Some((s, e)) if s.line == l => e.col = e.col.max(b),
                _ => ranges.push((Pos { line: l, col: a }, Pos { line: l, col: b })),
            }
        }
        Some((found.target, ranges))
    }

    /// 行 `line` から上へ（`limit` 行まで）、プロンプトの行を探して作業フォルダを読む
    /// （[`crate::links::prompt_cwd`]。`~` で始まることがある）。
    pub fn prompt_cwd_above(&self, line: u64, limit: u64) -> Option<String> {
        let mut n = line;
        let stop = line.saturating_sub(limit).max(self.dropped);
        loop {
            if let Some(l) = self.line(n)
                && let Some(p) = crate::links::prompt_cwd(&l.text())
            {
                return Some(p);
            }
            if n <= stop {
                return None;
            }
            n -= 1;
        }
    }

    // ---- 選択とコピー ---------------------------------------------------------

    /// `start` から `end` の手前までの文字列（行の間は改行。折り返した行はつなぐ）。
    pub fn text(&self, start: Pos, end: Pos) -> String {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut out = String::new();
        let mut line = start.line;
        while line <= end.line {
            let Some(l) = self.line(line) else {
                line += 1;
                continue;
            };
            let from = if line == start.line { start.col } else { 0 };
            let to = if line == end.line {
                end.col.min(l.cells.len())
            } else {
                l.cells.len()
            };
            let mut s = String::new();
            for c in l.cells.iter().take(to).skip(from) {
                if !c.is_continuation() {
                    s.push_str(&c.text());
                }
            }
            if line == end.line {
                if end.col >= l.cells.len() {
                    s.truncate(s.trim_end_matches(' ').len());
                }
                out.push_str(&s);
            } else if l.wrapped {
                out.push_str(&s);
            } else {
                out.push_str(s.trim_end_matches(' '));
                out.push('\n');
            }
            line += 1;
        }
        out
    }

    /// `pos` を含む単語の範囲（`end` は単語の次の桁）。
    pub fn word_at(&self, pos: Pos) -> (Pos, Pos) {
        let Some(l) = self.line(pos.line) else {
            return (pos, pos);
        };
        let n = l.cells.len();
        let mut col = pos.col.min(n.saturating_sub(1));
        if n == 0 {
            return (pos, pos);
        }
        if l.cells[col].is_continuation() && col > 0 {
            col -= 1;
        }
        let class = |c: &Cell| char_class(c.ch);
        let k = class(&l.cells[col]);
        let mut s = col;
        while s > 0 {
            let prev = &l.cells[s - 1];
            if !prev.is_continuation() && class(prev) != k {
                break;
            }
            s -= 1;
        }
        let mut e = col + 1;
        while e < n {
            let c = &l.cells[e];
            if !c.is_continuation() && class(c) != k {
                break;
            }
            e += 1;
        }
        (
            Pos {
                line: pos.line,
                col: s,
            },
            Pos {
                line: pos.line,
                col: e,
            },
        )
    }

    /// すべての行の文字列（テスト・保存用）。
    pub fn screen_text(&self) -> String {
        let mut out = String::new();
        for l in &self.grid {
            out.push_str(&l.text());
            out.push('\n');
        }
        out
    }
}

/// 単語の区切りの種類（空白・記号・それ以外）。
fn char_class(c: char) -> u8 {
    if c == ' ' || c == '\t' {
        0
    } else if "()[]{}<>'\"`|,;&!".contains(c) {
        1
    } else {
        2
    }
}

/// 拡張色（`38;5;n`・`38;2;r;g;b`・`38:2::r:g:b`）。戻り値の 2 つ目は使った後続の引数の数。
fn extended_color(params: &[Vec<u32>], i: usize) -> (Option<Color>, usize) {
    let p = &params[i];
    let byte = |v: u32| v.min(255) as u8;
    if p.len() > 1 {
        // コロン区切り
        return (
            match p.get(1) {
                Some(5) => p.get(2).map(|&n| Color::Indexed(byte(n))),
                Some(2) if p.len() >= 6 => Some(Color::Rgb(byte(p[3]), byte(p[4]), byte(p[5]))),
                Some(2) if p.len() == 5 => Some(Color::Rgb(byte(p[2]), byte(p[3]), byte(p[4]))),
                _ => None,
            },
            0,
        );
    }
    let next = |k: usize| params.get(i + k).map(|v| v[0]);
    match next(1) {
        Some(5) => (next(2).map(|n| Color::Indexed(byte(n))), 2),
        Some(2) => match (next(2), next(3), next(4)) {
            (Some(r), Some(g), Some(b)) => (Some(Color::Rgb(byte(r), byte(g), byte(b))), 4),
            _ => (None, params.len() - i - 1),
        },
        _ => (None, 0),
    }
}

fn charset(f: u8) -> Charset {
    if f == b'0' {
        Charset::DecSpecial
    } else {
        Charset::Ascii
    }
}

/// DEC の特殊図形（罫線など）。
fn dec_special(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        c => c,
    }
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c > 0 && c % 8 == 0).collect()
}

fn is_blank(l: &Line) -> bool {
    l.cells
        .iter()
        .all(|c| c.ch == ' ' && c.attr.bg == Color::Default)
}

/// 行数を変える（下の行を足す・消すだけ）。
fn resize_rows_simple(lines: &mut Vec<Line>, rows: usize, cols: usize) {
    while lines.len() > rows {
        lines.remove(0);
    }
    while lines.len() < rows {
        lines.push(Line::new(cols, Attr::default()));
    }
}
