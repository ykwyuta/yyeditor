//! プリンター（3287）の印刷の組み立て（14 章 12 節）。
//!
//! - LU1 の **SCS**（SNA Character String）: 改行・復帰・改ページ・タブ・用紙の形式（SHF・SVF）・
//!   位置（PP）・透過（TRN）・SO/SI の 2 バイト文字を解釈して、行とページにする（[`Scs`]）。
//! - LU3 の **3270 データストリーム**: 画面と同じバッファに書かれたものを、WCC の「印刷開始」で
//!   WCC の行の長さ（40・64・80 桁、または NL・EM・CR・FF で区切る形）の行にする（[`lu3_pages`]）。
//!
//! ページは文字列の行の並び（行末の空白は除く。全角は 2 桁を占める）。

use yy_encoding::Ccsid;

use crate::screen::Screen;

/// 1 ページ（行の並び）。
pub type Page = Vec<String>;

/// 印刷の 1 ジョブ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrintJob {
    pub pages: Vec<Page>,
    /// 1 行の最大の桁数（SCS の SHF の MPP。印刷の文字の大きさを決める目安）
    pub columns: usize,
    /// 1 ページの行数（SCS の SVF の MPL。0 は指定なし）
    pub lines_per_page: usize,
    /// 受け取ったデータのバイト数
    pub bytes: usize,
}

impl PrintJob {
    /// テキスト（UTF-8。ページの区切りは FF）。
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (i, page) in self.pages.iter().enumerate() {
            if i > 0 {
                out.push('\u{0C}');
            }
            for line in page {
                out.push_str(line);
                out.push_str("\r\n");
            }
        }
        out
    }

    /// 全ページの行の最大の幅（桁。全角は 2）。
    pub fn width(&self) -> usize {
        self.pages
            .iter()
            .flatten()
            .map(|l| text_width(l))
            .max()
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.pages.iter().all(|p| p.iter().all(|l| l.is_empty()))
    }
}

/// 文字列の桁数（全角は 2）。
pub fn text_width(s: &str) -> usize {
    s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

/// 全角（2 桁）の文字か。半角カナ・ASCII・ラテン文字は 1 桁。
pub fn is_wide(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x20000..=0x3FFFD)
}

/// 既定の 1 行の最大の桁数
pub const DEFAULT_MPP: usize = 132;

/// 行とページを組み立てる。
#[derive(Debug)]
struct Builder {
    pages: Vec<Page>,
    page: Page,
    /// 今の行のセル（`None` は空白、`Some("")` は全角の 2 桁目）
    line: Vec<Option<String>>,
    col: usize,
    /// 1 行の最大の桁数（SHF の MPP）
    mpp: usize,
    mpl: usize,
    max_cols: usize,
    /// 横のタブの位置（1 から）
    htabs: Vec<usize>,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            pages: Vec::new(),
            page: Vec::new(),
            line: Vec::new(),
            col: 0,
            mpp: DEFAULT_MPP,
            mpl: 0,
            max_cols: 0,
            htabs: Vec::new(),
        }
    }

    /// 文字を今の位置に置く（右端を超えるなら折り返す）。
    fn put(&mut self, text: String, width: usize) {
        if self.col + width > self.mpp {
            self.new_line();
        }
        if self.line.len() < self.col + width {
            self.line.resize(self.col + width, None);
        }
        // 重ね打ち（CR の後）では空白で消さない
        let blank = text == " " || text == "\u{3000}" && width == 2;
        if !blank || self.line[self.col].is_none() {
            // 全角の途中に重ねるときは、前の全角を空白にする
            if self.line[self.col].as_deref() == Some("") && self.col > 0 {
                self.line[self.col - 1] = None;
            }
            self.line[self.col] = Some(text);
            if width == 2 {
                self.line[self.col + 1] = Some(String::new());
            }
        }
        self.col += width;
        self.max_cols = self.max_cols.max(self.col);
    }

    fn render(line: &[Option<String>]) -> String {
        let mut s = String::new();
        for c in line {
            match c {
                None => s.push(' '),
                Some(t) => s.push_str(t),
            }
        }
        s.trim_end().to_owned()
    }

    /// 行を終える（桁はそのまま）。ページの行数に達したら改ページ。
    fn end_line(&mut self) {
        let text = Builder::render(&self.line);
        self.page.push(text);
        self.line.clear();
        if self.mpl > 0 && self.page.len() >= self.mpl {
            self.end_page();
        }
    }

    /// 改行（NL）: 行を終えて左端へ。
    fn new_line(&mut self) {
        self.end_line();
        self.col = 0;
    }

    fn end_page(&mut self) {
        let page = std::mem::take(&mut self.page);
        self.pages.push(page);
    }

    /// 改ページ（FF）。書きかけの行があれば終えてから。
    fn form_feed(&mut self) {
        if !self.line.is_empty() {
            self.end_line();
        }
        if !self.page.is_empty() || self.pages.is_empty() {
            self.end_page();
        }
        self.col = 0;
    }

    fn tab(&mut self) {
        let next = self
            .htabs
            .iter()
            .map(|t| t.saturating_sub(1))
            .find(|&t| t > self.col)
            .unwrap_or((self.col / 8 + 1) * 8);
        if next >= self.mpp {
            self.new_line();
        } else {
            self.col = next;
        }
    }

    /// 縦の位置（1 から）へ。今のページの行が既にそこを過ぎていれば次のページ。
    fn goto_line(&mut self, line: usize) {
        let target = line.saturating_sub(1);
        if !self.line.is_empty() {
            self.end_line();
        }
        if self.page.len() > target {
            self.end_page();
        }
        while self.page.len() < target {
            self.page.push(String::new());
        }
    }

    fn finish(&mut self) -> Vec<Page> {
        if !self.line.is_empty() {
            self.end_line();
        }
        if !self.page.is_empty() {
            self.end_page();
        }
        self.col = 0;
        // 末尾の空のページは除く
        while self
            .pages
            .last()
            .is_some_and(|p| p.iter().all(|l| l.is_empty()))
        {
            self.pages.pop();
        }
        std::mem::take(&mut self.pages)
    }
}

// SCS の制御コード
const SCS_NUL: u8 = 0x00;
const SCS_HT: u8 = 0x05;
const SCS_VT: u8 = 0x0B;
const SCS_FF: u8 = 0x0C;
const SCS_CR: u8 = 0x0D;
const SCS_SO: u8 = 0x0E;
const SCS_SI: u8 = 0x0F;
const SCS_NL: u8 = 0x15;
const SCS_BS: u8 = 0x16;
const SCS_IRS: u8 = 0x1E;
const SCS_LF: u8 = 0x25;
const SCS_SA: u8 = 0x28;
const SCS_FMT: u8 = 0x2B;
const SCS_PP: u8 = 0x34;
const SCS_TRN: u8 = 0x35;

/// LU1（SCS）の印刷を組み立てる。データは区切りなく続けて渡してよい。
#[derive(Debug)]
pub struct Scs {
    ccsid: Ccsid,
    b: Builder,
    /// 解釈しきれていない（続きを待つ）バイト
    pending: Vec<u8>,
    shift: bool,
    bytes: usize,
}

impl Scs {
    pub fn new(ccsid: Ccsid) -> Scs {
        Scs {
            ccsid,
            b: Builder::new(),
            pending: Vec::new(),
            shift: false,
            bytes: 0,
        }
    }

    /// 何か受け取っている（ジョブの途中）。
    pub fn has_data(&self) -> bool {
        self.bytes > 0
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.bytes += data.len();
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(data);
        let used = self.parse(&buf);
        self.pending = buf[used..].to_vec();
    }

    /// ジョブを終える（PRINT-EOJ・一定時間データがない）。何もなければ `None`。
    pub fn finish(&mut self) -> Option<PrintJob> {
        if self.bytes == 0 {
            return None;
        }
        let pages = self.b.finish();
        let job = PrintJob {
            pages,
            columns: self.b.mpp.max(self.b.max_cols),
            lines_per_page: self.b.mpl,
            bytes: self.bytes,
        };
        // 用紙の形式は次のジョブにも引き継ぐ（ホストは最初のジョブでだけ送ることがある）
        self.b.max_cols = 0;
        self.pending.clear();
        self.shift = false;
        self.bytes = 0;
        Some(job)
    }

    /// 解釈できたバイト数を返す（残りは続きを待つ）。
    fn parse(&mut self, buf: &[u8]) -> usize {
        let mut i = 0;
        while i < buf.len() {
            let c = buf[i];
            if self.shift && c >= 0x40 {
                let Some(&c2) = buf.get(i + 1) else {
                    return i;
                };
                let code = u16::from_be_bytes([c, c2]);
                let text = if code == 0x4040 {
                    "\u{3000}".to_owned()
                } else {
                    self.ccsid
                        .decode_double(code)
                        .unwrap_or_else(|| "〓".to_owned())
                };
                self.b.put(text, 2);
                i += 2;
                continue;
            }
            match c {
                SCS_NUL => i += 1,
                SCS_HT => {
                    self.b.tab();
                    i += 1;
                }
                SCS_NL | SCS_IRS | SCS_VT => {
                    self.b.new_line();
                    i += 1;
                }
                SCS_CR => {
                    self.b.col = 0;
                    i += 1;
                }
                SCS_LF => {
                    self.b.end_line();
                    i += 1;
                }
                SCS_FF => {
                    self.b.form_feed();
                    i += 1;
                }
                SCS_BS => {
                    self.b.col = self.b.col.saturating_sub(1);
                    i += 1;
                }
                SCS_SO => {
                    self.shift = true;
                    i += 1;
                }
                SCS_SI => {
                    self.shift = false;
                    i += 1;
                }
                SCS_SA => {
                    if buf.len() < i + 3 {
                        return i;
                    }
                    i += 3;
                }
                SCS_PP => {
                    let (Some(&kind), Some(&n)) = (buf.get(i + 1), buf.get(i + 2)) else {
                        return i;
                    };
                    let n = usize::from(n);
                    match kind {
                        // 横の絶対位置・相対位置
                        0xC0 => self.b.col = n.saturating_sub(1),
                        0xC8 => self.b.col += n,
                        // 縦の絶対位置・相対位置
                        0x4C => self.b.goto_line(n),
                        0x48 => {
                            let col = self.b.col;
                            for _ in 0..n {
                                self.b.end_line();
                            }
                            self.b.col = col;
                        }
                        _ => {}
                    }
                    i += 3;
                }
                SCS_TRN => {
                    let Some(&n) = buf.get(i + 1) else {
                        return i;
                    };
                    let n = usize::from(n);
                    if buf.len() < i + 2 + n {
                        return i;
                    }
                    for &b in &buf[i + 2..i + 2 + n] {
                        self.single(b);
                    }
                    i += 2 + n;
                }
                SCS_FMT => {
                    let (Some(&class), Some(&ll)) = (buf.get(i + 1), buf.get(i + 2)) else {
                        return i;
                    };
                    let ll = usize::from(ll).max(1);
                    if buf.len() < i + 2 + ll {
                        return i;
                    }
                    let params = &buf[i + 3..i + 2 + ll];
                    self.format(class, params);
                    i += 2 + ll;
                }
                c if c < 0x40 => i += 1,
                c => {
                    self.single(c);
                    i += 1;
                }
            }
        }
        i
    }

    fn single(&mut self, b: u8) {
        let ch = if b == 0x40 {
            ' '
        } else {
            self.ccsid
                .decode_single(b)
                .filter(|c| !c.is_control())
                .unwrap_or(' ')
        };
        self.b.put(ch.to_string(), 1);
    }

    /// 0x2B の命令（SHF・SVF など）。
    fn format(&mut self, class: u8, params: &[u8]) {
        match class {
            // SHF: 最大の桁・左右の余白・タブの位置
            0xC1 => {
                self.b.mpp = params
                    .first()
                    .map(|&m| usize::from(m))
                    .filter(|&m| m > 0)
                    .unwrap_or(DEFAULT_MPP);
                self.b.htabs = params.iter().skip(3).map(|&t| usize::from(t)).collect();
            }
            // SVF: 最大の行・上下の余白
            0xC2 => {
                self.b.mpl = params.first().map_or(0, |&m| usize::from(m));
            }
            // SLD（行間）・SCGL（文字セット）など: 文字の並びには影響しない
            _ => {}
        }
    }
}

/// LU3 の印刷: 画面のバッファを WCC の行の長さで行にする（FF でページを分ける）。
/// ページの末尾の空の行は除く。
pub fn lu3_pages(screen: &Screen, ccsid: Ccsid, wcc: u8) -> Vec<Page> {
    let cells = screen.display(ccsid);
    let n = screen.len();
    let line_len = match (wcc >> 4) & 0x03 {
        1 => Some(40),
        2 => Some(64),
        3 => Some(80),
        // 00: バッファの NL・EM・CR・FF で区切る
        _ => None,
    };
    let mut b = Builder::new();
    b.mpp = line_len.unwrap_or(DEFAULT_MPP);
    for (a, cell) in cells.iter().enumerate().take(n) {
        let raw = screen.cells[a].b;
        if line_len.is_none() && screen.cells[a].fa.is_none() {
            match raw {
                0x15 => {
                    b.new_line();
                    continue;
                }
                // EM: 印刷の終わり
                0x19 => break,
                0x0D => {
                    b.col = 0;
                    continue;
                }
                0x0C => {
                    b.form_feed();
                    continue;
                }
                _ => {}
            }
        }
        if cell.width == 0 {
            continue;
        }
        let text = if cell.hidden || cell.attribute || cell.text.chars().all(char::is_control) {
            " ".to_owned()
        } else {
            cell.text.clone()
        };
        b.put(text, usize::from(cell.width));
    }
    let mut pages = b.finish();
    for p in &mut pages {
        while p.last().is_some_and(|l| l.is_empty()) {
            p.pop();
        }
    }
    pages
}

#[cfg(test)]
mod tests;
