//! 正規表現による検索と置換（05 章）。
//!
//! 文書（ピースツリーのスナップショット）はメモリ上で連続していないため、ウィンドウ単位で
//! 読み出して検索する。ウィンドウの境界をまたぐマッチを取りこぼさないよう、次のウィンドウは
//! 前のウィンドウの末尾 `overlap` バイトと重ねる（05 章 3.2）。1 つのマッチの長さは
//! `overlap` 以下であることを前提にする（長さの上限が分かるパターンはその長さ、
//! 分からないパターンは 1 MiB）。
//!
//! エンジンは `regex-automata`（線形時間保証）。`^` `$` は行頭・行末（CRLF 対応）、
//! `\b` `\<` `\>` などの前後の文字は、ウィンドウの外（文書の実際の内容）も見て判定する。

mod replace;

use std::ops::Range;

use regex_automata::meta::Regex;
use regex_automata::util::captures::Captures;
use regex_automata::util::syntax;
use regex_automata::{Input, Match};
use yy_buffer::Snapshot;

pub use replace::{Edit, ReplaceError, Replacement, collect_edits, rewrite};

/// 検索条件。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    /// 正規表現として扱うか（`false` なら文字列そのもの）
    pub regex: bool,
    pub case_sensitive: bool,
    /// 単語単位（前後が単語の文字でない位置だけ）
    pub whole_word: bool,
}

/// パターンの誤り。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError(pub String);

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QueryError {}

/// 中止された。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

/// 前後の文字（`^` `\b` など）の判定のためにウィンドウの外に読む量
const CONTEXT: u64 = 8;
/// マッチの長さの上限が分からないパターンの上限
const UNBOUNDED_OVERLAP: u64 = 1 << 20;
/// 既定のウィンドウの大きさ
const WINDOW: u64 = 8 << 20;
/// 後方検索の最初のウィンドウ（見つかりやすいので小さく始めて広げる）
const BACK_WINDOW: u64 = 64 << 10;

/// コンパイル済みの検索条件。
pub struct Searcher {
    re: Regex,
    window: u64,
    overlap: u64,
}

/// [`Searcher::scan`] に渡すコールバック（文書内の範囲, キャプチャ付きならウィンドウとキャプチャ）。
pub(crate) type MatchFn<'a> = dyn FnMut(Range<u64>, Option<(&Window, &Captures)>) -> bool + 'a;

/// ウィンドウ（文書から読み出した範囲）。
pub(crate) struct Window {
    pub(crate) buf: Vec<u8>,
    /// `buf[0]` の文書内の位置
    base: u64,
}

impl Window {
    fn new() -> Window {
        Window {
            buf: Vec::new(),
            base: 0,
        }
    }

    /// 文書の `range` を前後の文脈付きで読み出す。
    fn load(&mut self, snap: &Snapshot, range: Range<u64>) {
        let start = range.start.saturating_sub(CONTEXT);
        let end = (range.end + CONTEXT).min(snap.len());
        self.buf.clear();
        for c in snap.chunks(start..end) {
            self.buf.extend_from_slice(c);
        }
        self.base = start;
    }

    fn rel(&self, pos: u64) -> usize {
        (pos - self.base) as usize
    }
}

/// UTF-8 の先頭バイトから文字の長さ（不正なら 1）。
fn char_len_at(buf: &[u8], i: usize) -> usize {
    match buf.get(i) {
        Some(0xC2..=0xDF) => 2,
        Some(0xE0..=0xEF) => 3,
        Some(0xF0..=0xF4) => 4,
        _ => 1,
    }
    .min(buf.len().saturating_sub(i).max(1))
}

impl Searcher {
    pub fn new(q: &Query) -> Result<Searcher, QueryError> {
        if q.pattern.is_empty() {
            return Err(QueryError("検索する文字列が空です".into()));
        }
        let mut pat = if q.regex {
            q.pattern.clone()
        } else {
            regex_syntax::escape(&q.pattern)
        };
        if q.whole_word {
            pat = format!(r"\b{{start-half}}(?:{pat})\b{{end-half}}");
        }
        let cfg = syntax::Config::new()
            .case_insensitive(!q.case_sensitive)
            .multi_line(true)
            .crlf(true);
        let hir = regex_syntax::ParserBuilder::new()
            .case_insensitive(!q.case_sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
            .parse(&pat)
            .map_err(|e| QueryError(format!("正規表現が正しくありません。\n{e}")))?;
        let overlap = match hir.properties().maximum_len() {
            Some(n) => (n as u64 + CONTEXT).max(16),
            None => UNBOUNDED_OVERLAP,
        };
        let re = Regex::builder()
            .syntax(cfg)
            .build(&pat)
            .map_err(|e| QueryError(format!("正規表現が正しくありません。\n{e}")))?;
        Ok(Searcher {
            re,
            window: WINDOW.max(overlap * 4),
            overlap,
        })
    }

    /// ウィンドウの大きさを変える（テスト用）。`window` は `overlap` の 2 倍より大きいこと。
    pub fn with_window(mut self, window: u64) -> Searcher {
        self.window = window.max(self.overlap * 2 + 1);
        self
    }

    /// 1 つのマッチの長さの上限（これより長いマッチは見つからないことがある）。
    pub fn max_match_len(&self) -> u64 {
        self.overlap
    }

    pub(crate) fn regex(&self) -> &Regex {
        &self.re
    }

    /// ウィンドウの `at..end`（文書内の位置）で最初のマッチを探す。
    fn search_in(
        &self,
        w: &Window,
        at: u64,
        end: u64,
        caps: Option<&mut Captures>,
    ) -> Option<Range<u64>> {
        let input = Input::new(&w.buf).range(w.rel(at)..w.rel(end));
        let m: Option<Match> = match caps {
            Some(c) => {
                self.re.search_captures(&input, c);
                c.get_match()
            }
            None => self.re.search(&input),
        };
        m.map(|m| w.base + m.start() as u64..w.base + m.end() as u64)
    }

    /// `range` の中で位置 `from` 以降の最初のマッチ。
    ///
    /// `step(処理済みの位置)` が `false` を返したら中止する。
    pub fn find_next(
        &self,
        snap: &Snapshot,
        range: Range<u64>,
        from: u64,
        step: &mut dyn FnMut(u64) -> bool,
    ) -> Result<Option<Range<u64>>, Cancelled> {
        let mut found = None;
        let mut first = true;
        self.scan(snap, range, from, None, step, &mut |m, _| {
            if first {
                found = Some(m);
                first = false;
            }
            false
        })?;
        Ok(found)
    }

    /// `range` の中で開始位置が `before` より前の最後のマッチ
    /// （マッチはどの位置からでも始められるものとして、開始位置が最大のもの）。
    pub fn find_prev(
        &self,
        snap: &Snapshot,
        range: Range<u64>,
        before: u64,
        step: &mut dyn FnMut(u64) -> bool,
    ) -> Result<Option<Range<u64>>, Cancelled> {
        let mut before = before.min(range.end);
        let mut size = BACK_WINDOW.max(self.overlap * 2);
        let mut w = Window::new();
        while before > range.start {
            let w_start = before.saturating_sub(size).max(range.start);
            // 開始位置が before より前のマッチは before + overlap までに終わる
            let w_end = (before + self.overlap).min(range.end);
            w.load(snap, w_start..w_end);
            let mut best = None;
            let mut at = w_start;
            while at <= w_end {
                match self.search_in(&w, at, w_end, None) {
                    Some(m) if m.start < before => {
                        at = m.start + char_len_at(&w.buf, w.rel(m.start)) as u64;
                        best = Some(m);
                    }
                    _ => break,
                }
            }
            if best.is_some() {
                return Ok(best);
            }
            if !step(range.end - w_start) {
                return Err(Cancelled);
            }
            if w_start == range.start {
                break;
            }
            before = w_start;
            size = (size * 4).min(self.window);
        }
        Ok(None)
    }

    /// `range` の中の、`from` 以降のマッチを先頭から順に（重ならないように）`f` に渡す。
    /// `f` が `false` を返したら終わる。`caps` を渡すとキャプチャも求める（置換用）。
    ///
    /// `f` には（文書内の範囲, キャプチャ付きならウィンドウ）を渡す。
    pub(crate) fn scan(
        &self,
        snap: &Snapshot,
        range: Range<u64>,
        from: u64,
        mut caps: Option<&mut Captures>,
        step: &mut dyn FnMut(u64) -> bool,
        f: &mut MatchFn<'_>,
    ) -> Result<(), Cancelled> {
        let end = range.end.min(snap.len());
        let mut pos = from.clamp(range.start, end);
        // 直前のマッチの終わり（そこでの空のマッチは数えない）
        let mut last_end: Option<u64> = None;
        let mut w = Window::new();
        loop {
            let w_end = (pos + self.window).min(end);
            w.load(snap, pos..w_end);
            let mut at = pos;
            while at <= w_end {
                let Some(m) = self.search_in(&w, at, w_end, caps.as_deref_mut()) else {
                    break;
                };
                if m.is_empty() && last_end == Some(m.start) {
                    // 直前のマッチに続く空のマッチは飛ばす
                    at = m.start + char_len_at(&w.buf, w.rel(m.start)) as u64;
                    if m.start >= w_end {
                        break;
                    }
                    continue;
                }
                if w_end != end && m.start + self.overlap > w_end {
                    // ウィンドウの末尾で切れているかもしれない（その手前で始まる、末尾をまたぐ
                    // マッチもありうる）ので、次のウィンドウで探し直す
                    break;
                }
                let cont = f(m.clone(), caps.as_deref().map(|c| (&w, c)));
                if !cont {
                    return Ok(());
                }
                last_end = Some(m.end);
                at = if m.is_empty() {
                    m.end + char_len_at(&w.buf, w.rel(m.end)) as u64
                } else {
                    m.end
                };
            }
            if w_end == end {
                return Ok(());
            }
            if !step(w_end) {
                return Err(Cancelled);
            }
            // w_end - overlap より前で始まるマッチはこのウィンドウ内で完結しているので、
            // 次のウィンドウはそこ（か最後のマッチの終わり）から始める
            pos = at.max(w_end - self.overlap);
        }
    }

    /// 文書全体で、位置 `from` から前方（`forward`）または後方に次のマッチを探し、
    /// 見つからなければ反対の端から続けて探す。（マッチ, 端を越えて探したか）を返す。
    pub fn find_wrapping(
        &self,
        snap: &Snapshot,
        from: u64,
        forward: bool,
        step: &mut dyn FnMut(u64) -> bool,
    ) -> Result<Option<(Range<u64>, bool)>, Cancelled> {
        let len = snap.len();
        if forward {
            if let Some(m) = self.find_next(snap, 0..len, from, step)? {
                return Ok(Some((m, false)));
            }
            let end = (from + self.overlap).min(len);
            Ok(self
                .find_next(snap, 0..end, 0, step)?
                .filter(|m| m.start < from)
                .map(|m| (m, true)))
        } else {
            if let Some(m) = self.find_prev(snap, 0..len, from, step)? {
                return Ok(Some((m, false)));
            }
            Ok(self
                .find_prev(snap, 0..len, len + 1, step)?
                .filter(|m| m.start >= from)
                .map(|m| (m, true)))
        }
    }

    /// `range` の中のマッチの数（`limit` で打ち切る）。
    pub fn count(
        &self,
        snap: &Snapshot,
        range: Range<u64>,
        limit: u64,
        step: &mut dyn FnMut(u64) -> bool,
    ) -> Result<u64, Cancelled> {
        let mut n = 0u64;
        self.scan(snap, range.clone(), range.start, None, step, &mut |_, _| {
            n += 1;
            n < limit
        })?;
        Ok(n)
    }

    /// `range` の中のすべてのマッチ（表示範囲のハイライトなど、狭い範囲用）。
    pub fn matches_in(&self, snap: &Snapshot, range: Range<u64>, limit: usize) -> Vec<Range<u64>> {
        self.matches_cancellable(snap, range, limit, &mut |_| true)
            .unwrap_or_default()
    }

    /// 広い範囲の一致を、進捗通知と中止を受けながら収集する。
    pub fn matches_cancellable(
        &self,
        snap: &Snapshot,
        range: Range<u64>,
        limit: usize,
        step: &mut dyn FnMut(u64) -> bool,
    ) -> Result<Vec<Range<u64>>, Cancelled> {
        let mut out = Vec::new();
        self.scan(snap, range.clone(), range.start, None, step, &mut |m, _| {
            out.push(m);
            out.len() < limit
        })?;
        Ok(out)
    }
}
