//! シンタックスハイライト（10 章）: 行の開始状態の記録の維持と、表示範囲の色付け。
//!
//! 区切り文字モードの [`crate::csv::CsvView`] と同じく、編集後は変わっていない先頭部分の記録を
//! 残して続きから読み直す（残りが少なければその場で、多ければバックグラウンドで）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yy_buffer::Snapshot;
use yy_delimited::common_prefix;
use yy_jobs::{JobHandle, JobPool, Notifier};
use yy_syntax::{LineTokens, Syntax, SyntaxIndex};

pub use yy_syntax::{Registry, TokenId, TokenSpan};

/// この大きさまでの残りはその場で読む
const SYNC_BYTES: u64 = 256 << 10;
/// バックグラウンドで 1 回にロックを持って読む量
const STEP_BYTES: u64 = 1 << 20;
/// これより大きい文書は全体を読まない（表示範囲の近くだけ。暫定の色になることがある）
const FULL_SCAN_LIMIT: u64 = 1 << 30;
/// 括弧の対応を探す範囲
const BRACKET_LIMIT: u64 = 1 << 20;

struct Page {
    snap: Snapshot,
    first: u64,
    until: u64,
    generation: u64,
    lines: Arc<Vec<LineTokens>>,
    exact: bool,
}

/// 文書のハイライトの状態。
pub struct SyntaxView {
    syntax: Arc<Syntax>,
    index: Arc<Mutex<SyntaxIndex>>,
    /// 記録が対応している内容
    indexed: Snapshot,
    job: Option<JobHandle>,
    /// 記録が進んだ回数（暫定の色を描き直すため）
    generation: u64,
    page: Option<Page>,
}

impl SyntaxView {
    pub fn new(syntax: Arc<Syntax>) -> SyntaxView {
        SyntaxView {
            index: Arc::new(Mutex::new(SyntaxIndex::new(syntax.clone()))),
            syntax,
            indexed: Snapshot::empty(),
            job: None,
            generation: 0,
            page: None,
        }
    }

    pub fn syntax(&self) -> &Arc<Syntax> {
        &self.syntax
    }

    /// 表示の版（記録が進むと変わる）。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 文書の内容 `snap` に合わせる。
    pub fn sync(&mut self, snap: &Snapshot, pool: &JobPool, notify: Notifier) {
        if !self.syntax.is_multiline() {
            return;
        }
        let same =
            snap.len() == self.indexed.len() && common_prefix(snap, &self.indexed) == snap.len();
        if same && (self.job.is_some() || self.index.lock().unwrap().is_complete()) {
            return;
        }
        if let Some(j) = self.job.take() {
            j.cancel();
        }
        let prefix = common_prefix(snap, &self.indexed);
        let mut idx = self.index.lock().unwrap();
        idx.truncate(prefix);
        self.indexed = snap.clone();
        self.generation += 1;
        let rest = snap.len().saturating_sub(idx.scanned());
        if rest <= SYNC_BYTES {
            idx.extend(snap, u64::MAX);
            return;
        }
        if snap.len() > FULL_SCAN_LIMIT {
            return;
        }
        drop(idx);
        let index = self.index.clone();
        let snap = snap.clone();
        self.job = Some(pool.spawn(move |ctx| {
            ctx.progress.set_total(snap.len());
            let mut last = Instant::now();
            loop {
                let (done, pos) = {
                    let mut idx = index.lock().unwrap();
                    if ctx.cancel.is_cancelled() {
                        return;
                    }
                    let done = idx.extend(&snap, STEP_BYTES);
                    (done, idx.scanned())
                };
                ctx.progress.set_done(pos);
                if done || last.elapsed() >= Duration::from_millis(300) {
                    notify();
                    last = Instant::now();
                }
                if done {
                    return;
                }
            }
        }));
    }

    /// バックグラウンドの読み込みの進み具合を反映する。表示を描き直すべきなら `true`。
    pub fn poll(&mut self) -> bool {
        if self.job.is_none() {
            return false;
        }
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            self.job = None;
        }
        // 暫定の色の部分が確定しているかもしれない
        if self.page.as_ref().is_some_and(|p| !p.exact) {
            self.generation += 1;
            self.page = None;
            return true;
        }
        false
    }

    /// 行頭 `first` から、行頭が `until` より前の行までのトークン（同じ範囲ならキャッシュを使う）。
    /// 2 番目の値は開始状態が確定しているか。
    pub fn lines(
        &mut self,
        snap: &Snapshot,
        first: u64,
        until: u64,
    ) -> (Arc<Vec<LineTokens>>, bool) {
        if let Some(p) = &self.page
            && p.first == first
            && p.until == until
            && p.generation == self.generation
            && p.snap.len() == snap.len()
            && common_prefix(&p.snap, snap) == snap.len()
        {
            return (p.lines.clone(), p.exact);
        }
        let (lines, exact) = self
            .index
            .lock()
            .unwrap()
            .highlight_lines(snap, first, until);
        let lines = Arc::new(lines);
        self.page = Some(Page {
            snap: snap.clone(),
            first,
            until,
            generation: self.generation,
            lines: lines.clone(),
            exact,
        });
        (lines, exact)
    }

    /// トークンが文字列・コメントか（括弧の対応で無視する）。
    fn is_literal(&self, t: TokenId) -> bool {
        let name = self.syntax.token_name(t);
        ["comment", "string", "code", "regex"]
            .iter()
            .any(|p| name == *p || name.starts_with(&format!("{p}.")))
    }

    /// 位置 `offset` の直後（なければ直前）の括弧と、対応する括弧の位置。
    /// 文字列・コメントの中の括弧は無視する。
    pub fn matching_bracket(&self, snap: &Snapshot, offset: u64) -> Option<(u64, u64)> {
        let pairs = &self.syntax.brackets;
        let at = |o: u64| snap.byte_at(o).map(|b| b as char);
        let find = |c: char| pairs.iter().find(|(a, b)| *a == c || *b == c).copied();
        let (pos, pair) = [Some(offset), offset.checked_sub(1)]
            .into_iter()
            .flatten()
            .find_map(|o| at(o).and_then(find).map(|p| (o, p)))?;
        let forward = at(pos) == Some(pair.0);
        let index = self.index.lock().unwrap();
        // `o` を含む行の行頭（探す範囲に見つからなければ諦める）
        let line_start = |o: u64| -> Option<u64> {
            if o == 0 {
                return Some(0);
            }
            match snap.find_prev(o.saturating_sub(BRACKET_LIMIT)..o, b'\n') {
                Some(n) => Some(n + 1),
                None if o <= BRACKET_LIMIT => Some(0),
                None => None,
            }
        };
        const WINDOW: u64 = 64 << 10;
        let mut depth = 0i64;
        let mut scanned = 0u64;
        if forward {
            let mut first = line_start(pos)?;
            while scanned < BRACKET_LIMIT && first < snap.len() {
                let (lines, _) = index.highlight_lines(snap, first, first + WINDOW);
                for line in lines.iter() {
                    let bytes = snap.read(line.start..line.end);
                    for (i, &b) in bytes.iter().enumerate() {
                        let o = line.start + i as u64;
                        if o < pos || self.in_literal(line, i) {
                            continue;
                        }
                        if b as char == pair.0 {
                            depth += 1;
                        } else if b as char == pair.1 {
                            depth -= 1;
                            if depth == 0 {
                                return Some((pos, o));
                            }
                        }
                    }
                    scanned += line.next - line.start;
                    first = line.next;
                }
                if lines.last().is_none_or(|l| l.next >= snap.len()) {
                    break;
                }
            }
        } else {
            // `hi` より前を、行頭 `first` からの窓ごとに後ろから調べる
            let mut hi = pos + 1;
            let mut first = line_start(pos)?;
            loop {
                let (lines, _) = index.highlight_lines(snap, first, hi);
                for line in lines.iter().rev() {
                    let bytes = snap.read(line.start..line.end);
                    for (i, &b) in bytes.iter().enumerate().rev() {
                        let o = line.start + i as u64;
                        if o >= hi || self.in_literal(line, i) {
                            continue;
                        }
                        if b as char == pair.1 {
                            depth += 1;
                        } else if b as char == pair.0 {
                            depth -= 1;
                            if depth == 0 {
                                return Some((o, pos));
                            }
                        }
                    }
                }
                scanned += hi - first;
                if first == 0 || scanned >= BRACKET_LIMIT {
                    break;
                }
                hi = first;
                first = line_start(first.saturating_sub(WINDOW))?;
            }
        }
        None
    }

    fn in_literal(&self, line: &LineTokens, i: usize) -> bool {
        let i = i as u32;
        line.spans
            .iter()
            .any(|s| s.range.start <= i && i < s.range.end && self.is_literal(s.token))
    }
}

impl Drop for SyntaxView {
    fn drop(&mut self) {
        if let Some(j) = &self.job {
            j.cancel();
        }
    }
}

/// 行コメントの付け外し（10 章 9）。`lines` の行（改行を除く内容）を書き換えた内容を返す。
/// 空白だけの行は変えない。すべての行がコメントなら外し、そうでなければ付ける。
pub fn toggle_comment(
    lines: &[Vec<u8>],
    line: Option<&str>,
    block: Option<(&str, &str)>,
) -> Option<Vec<Vec<u8>>> {
    let indent = |l: &[u8]| l.iter().take_while(|b| **b == b' ' || **b == b'\t').count();
    let blank = |l: &[u8]| l.iter().all(|b| b.is_ascii_whitespace());
    if let Some(p) = line {
        let p = p.as_bytes();
        let commented = lines
            .iter()
            .filter(|l| !blank(l))
            .all(|l| l[indent(l)..].starts_with(p));
        return Some(
            lines
                .iter()
                .map(|l| {
                    if blank(l) {
                        return l.clone();
                    }
                    let i = indent(l);
                    let mut out = l[..i].to_vec();
                    if commented {
                        let mut rest = &l[i + p.len()..];
                        if rest.first() == Some(&b' ') {
                            rest = &rest[1..];
                        }
                        out.extend_from_slice(rest);
                    } else {
                        out.extend_from_slice(p);
                        out.push(b' ');
                        out.extend_from_slice(&l[i..]);
                    }
                    out
                })
                .collect(),
        );
    }
    let (open, close) = block?;
    let (open, close) = (open.as_bytes(), close.as_bytes());
    let trimmed_end = |l: &[u8]| {
        l.len()
            - l.iter()
                .rev()
                .take_while(|b| b.is_ascii_whitespace())
                .count()
    };
    let commented = lines.iter().filter(|l| !blank(l)).all(|l| {
        let i = indent(l);
        l[i..].starts_with(open)
            && l[..trimmed_end(l)].ends_with(close)
            && trimmed_end(l) >= i + open.len() + close.len()
    });
    Some(
        lines
            .iter()
            .map(|l| {
                if blank(l) {
                    return l.clone();
                }
                let i = indent(l);
                let e = trimmed_end(l);
                let mut out = l[..i].to_vec();
                if commented {
                    let mut body = &l[i + open.len()..e - close.len()];
                    if body.first() == Some(&b' ') {
                        body = &body[1..];
                    }
                    if body.last() == Some(&b' ') {
                        body = &body[..body.len() - 1];
                    }
                    out.extend_from_slice(body);
                } else {
                    out.extend_from_slice(open);
                    out.push(b' ');
                    out.extend_from_slice(&l[i..e]);
                    out.push(b' ');
                    out.extend_from_slice(close);
                }
                out.extend_from_slice(&l[e..]);
                out
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: &str) -> Snapshot {
        Snapshot::from_bytes(text.as_bytes().to_vec())
    }

    #[test]
    fn brackets_skip_strings_and_comments() {
        let reg = Registry::builtin();
        let v = SyntaxView::new(reg.get("c").unwrap());
        let text = "f(a, \")\", /* ) */ g(b)\n  )x";
        let s = snap(text);
        let close = text.rfind(')').unwrap() as u64;
        assert_eq!(v.matching_bracket(&s, 1), Some((1, close)));
        // 閉じ括弧の直後から
        assert_eq!(v.matching_bracket(&s, close + 1), Some((1, close)));
        // 内側の括弧
        let g = text.find("g(").unwrap() as u64 + 1;
        assert_eq!(v.matching_bracket(&s, g), Some((g, g + 2)));
        assert_eq!(v.matching_bracket(&s, 3), None);
    }

    #[test]
    fn brackets_across_windows() {
        let reg = Registry::builtin();
        let v = SyntaxView::new(reg.get("c").unwrap());
        // 括弧の間が探す窓（64 KB）より長い。途中の文字列・コメントの括弧は数えない
        let body = "x = \"}\"; /* } */ y();\n".repeat(5000);
        let text = format!("f() {{\n{body}}}\n");
        let s = snap(&text);
        let open = text.find('{').unwrap() as u64;
        let close = text.rfind('}').unwrap() as u64;
        assert!(close - open > 64 << 10);
        assert_eq!(v.matching_bracket(&s, open), Some((open, close)));
        assert_eq!(v.matching_bracket(&s, close + 1), Some((open, close)));
    }

    #[test]
    fn toggles_line_comments() {
        let lines: Vec<Vec<u8>> = ["  a", "", "\tb"]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let on = toggle_comment(&lines, Some("//"), None).unwrap();
        assert_eq!(on, [b"  // a".to_vec(), b"".to_vec(), b"\t// b".to_vec()]);
        let off = toggle_comment(&on, Some("//"), None).unwrap();
        assert_eq!(off, lines);
        // 一部だけコメントなら付ける
        let mixed = vec![b"// a".to_vec(), b"b".to_vec()];
        assert_eq!(
            toggle_comment(&mixed, Some("//"), None).unwrap(),
            [b"// // a".to_vec(), b"// b".to_vec()]
        );
        let block = toggle_comment(&[b"  <p>".to_vec()], None, Some(("<!--", "-->"))).unwrap();
        assert_eq!(block, [b"  <!-- <p> -->".to_vec()]);
        assert_eq!(
            toggle_comment(&block, None, Some(("<!--", "-->"))).unwrap(),
            [b"  <p>".to_vec()]
        );
        assert_eq!(toggle_comment(&lines, None, None), None);
    }
}
