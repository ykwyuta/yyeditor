//! シンタックスハイライト（10 章）。
//!
//! * 定義は TOML の宣言的な形式（[`def`]）。コンテキスト（状態）ごとのルールを、
//!   コンテキストごとに 1 つの複数パターンの正規表現（`regex-automata`）にまとめ、
//!   1 回の走査で「最も左で一致するルール」を求める
//! * 1 行（論理行）ずつ処理する。入力は行の開始状態、出力はトークンの範囲と行の終了状態
//! * 行をまたぐ状態（ブロックコメントなど）は、一定間隔の位置の状態を記録した
//!   [`SyntaxIndex`] で求める（CSV のレコードインデックスと同じ方式）
//! * 桁位置で意味が決まる COBOL・JCL などのための固定桁の指定（`columns`）

mod def;
mod index;
mod registry;

use std::collections::HashMap;
use std::ops::Range;

use regex_automata::Input;
use regex_automata::meta::Regex;
use regex_automata::util::captures::Captures;
use unicode_width::UnicodeWidthChar;

pub use def::parse;
pub use index::{LineTokens, SyntaxIndex};
pub use registry::Registry;

/// トークンの種類（[`Syntax::token_name`] で名前を引く）。
pub type TokenId = u16;

/// 行内（行の先頭からのバイト位置）のトークンの範囲。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenSpan {
    pub range: Range<u32>,
    pub token: TokenId,
}

/// 行の先頭の状態（コンテキストのスタック。空なら初期状態）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct LineState(Vec<u16>);

impl LineState {
    pub fn is_initial(&self) -> bool {
        self.0.is_empty()
    }
}

/// コンテキストのスタックの深さの上限。
const MAX_DEPTH: usize = 32;
/// 1 行で処理する長さの上限（これより後ろは色を付けず、状態は近似になる）。
pub const MAX_LINE_BYTES: usize = 64 << 10;

#[derive(Clone, Debug)]
pub(crate) enum Pat {
    /// ルールの一致。`push` があればそのコンテキストに入る（範囲の開始）
    Rule {
        token: Option<TokenId>,
        captures: Vec<(usize, TokenId)>,
        push: Option<u16>,
    },
    /// 範囲の終わり（コンテキストから出る）
    End,
    /// 範囲の中で終わりとみなさないもの（エスケープ）
    Skip,
}

pub(crate) struct Ctx {
    pub re: Option<Regex>,
    pub pats: Vec<Pat>,
    /// 範囲の中のトークン（文字列・コメントなど）
    pub token: Option<TokenId>,
    /// キーワードを探すか
    pub keywords: bool,
}

pub(crate) struct Column {
    /// 表示桁（0 始まり、終わりを含まない）
    pub cols: Range<u32>,
    pub token: Option<TokenId>,
    /// 一致したら行全体を `line_token` にする
    pub line_match: Option<(Regex, TokenId)>,
    /// この範囲をコンテキストのルールで色付けする
    pub context: Option<u16>,
}

/// コンパイル済みの定義。
pub struct Syntax {
    pub id: String,
    pub name: String,
    /// 行コメントの開始（コメント化に使う）
    pub line_comment: Option<String>,
    /// ブロックコメントの開始と終わり
    pub block_comment: Option<(String, String)>,
    /// 対応を探す括弧の組
    pub brackets: Vec<(char, char)>,
    pub(crate) tokens: Vec<String>,
    pub(crate) contexts: Vec<Ctx>,
    pub(crate) keywords: HashMap<Vec<u8>, TokenId>,
    pub(crate) keywords_ci: bool,
    pub(crate) word: Regex,
    pub(crate) columns: Vec<Column>,
    /// この行頭では必ず初期状態（遡りをここで止められる）
    pub(crate) sync: Option<Regex>,
    /// 行をまたぐ状態がありうるか（なければ状態の記録は要らない）
    pub(crate) multiline: bool,
}

impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Syntax").field("id", &self.id).finish()
    }
}

/// 重ならない昇順のトークンの列に追加する（同じトークンが続けばつなげる）。
fn push_span(out: &mut Vec<TokenSpan>, range: Range<usize>, token: TokenId) {
    if range.is_empty() {
        return;
    }
    let range = range.start as u32..range.end as u32;
    if let Some(last) = out.last_mut()
        && last.token == token
        && last.range.end == range.start
    {
        last.range.end = range.end;
        return;
    }
    out.push(TokenSpan { range, token });
}

impl Syntax {
    /// トークンの名前（`comment`・`keyword.control` など）。
    pub fn token_name(&self, t: TokenId) -> &str {
        self.tokens.get(t as usize).map_or("", |s| s.as_str())
    }

    /// トークンの名前の一覧（[`TokenId`] の順）。
    pub fn token_names(&self) -> &[String] {
        &self.tokens
    }

    /// 行をまたぐ状態がありうるか。
    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// 行頭が同期点（必ず初期状態）か。
    pub fn is_sync_line(&self, line: &[u8]) -> bool {
        self.sync.as_ref().is_some_and(|r| r.is_match(line))
    }

    /// 1 行（改行を除く）を色付けし、次の行の開始状態を返す。
    pub fn highlight_line(
        &self,
        state: &LineState,
        line: &[u8],
        out: &mut Vec<TokenSpan>,
    ) -> LineState {
        let line = &line[..line.len().min(MAX_LINE_BYTES)];
        if self.columns.is_empty() {
            let mut stack = state.0.clone();
            self.run(0, &mut stack, line, 0..line.len(), out);
            return LineState(stack);
        }
        self.columns_line(state, line, out)
    }

    /// コンテキストのルールで `range` を色付けする。`base` はスタックが空のときのコンテキスト。
    fn run(
        &self,
        base: u16,
        stack: &mut Vec<u16>,
        hay: &[u8],
        range: Range<usize>,
        out: &mut Vec<TokenSpan>,
    ) {
        let end = range.end;
        let mut pos = range.start;
        // 空の一致で状態だけが変わる回数の上限（無限ループの防止）
        let mut empty_budget = 64;
        let mut caps_cache: Vec<Option<Captures>> = Vec::new();
        loop {
            let cid = stack.last().copied().unwrap_or(base) as usize;
            let ctx = &self.contexts[cid];
            let Some(re) = &ctx.re else {
                self.gap(ctx, hay, pos..end, out);
                return;
            };
            if caps_cache.len() <= cid {
                caps_cache.resize_with(cid + 1, || None);
            }
            let caps = caps_cache[cid].get_or_insert_with(|| re.create_captures());
            re.search_captures(&Input::new(hay).span(pos..end), caps);
            let Some(m) = caps.get_match() else {
                self.gap(ctx, hay, pos..end, out);
                return;
            };
            let (ms, me) = (m.start(), m.end());
            self.gap(ctx, hay, pos..ms, out);
            let mut changed = false;
            match &ctx.pats[m.pattern().as_usize()] {
                Pat::Rule {
                    token,
                    captures,
                    push,
                } => {
                    // キーワードはグループの指定より優先する（`if (` を関数呼び出しにしない）
                    let kw = |w: &[u8]| if ctx.keywords { self.keyword(w) } else { None };
                    emit_match(out, hay, ms..me, *token, captures, caps, &kw);
                    if let Some(p) = push
                        && stack.len() < MAX_DEPTH
                    {
                        stack.push(*p);
                        changed = true;
                    }
                }
                Pat::End => {
                    if let Some(t) = ctx.token {
                        push_span(out, ms..me, t);
                    }
                    if stack.pop().is_some() {
                        changed = true;
                    }
                }
                Pat::Skip => {
                    if let Some(t) = ctx.token {
                        push_span(out, ms..me, t);
                    }
                }
            }
            if me > ms {
                pos = me;
            } else if changed && empty_budget > 0 {
                // 空の一致で状態が変わった（行末の `$` で範囲が終わるなど）
                empty_budget -= 1;
            } else if pos < end {
                // 空の一致で進まない: 1 文字を普通の文字として進める
                let n = utf8_len(hay[pos]).clamp(1, end - pos);
                self.gap(ctx, hay, pos..pos + n, out);
                pos += n;
            } else {
                return;
            }
        }
    }

    /// ルールに一致しなかった部分。範囲の中ならそのトークン、キーワードがあればその色。
    fn gap(&self, ctx: &Ctx, hay: &[u8], r: Range<usize>, out: &mut Vec<TokenSpan>) {
        if r.is_empty() {
            return;
        }
        if ctx.keywords && !self.keywords.is_empty() {
            let mut last = r.start;
            for m in self.word.find_iter(Input::new(hay).span(r.clone())) {
                if let Some(t) = self.keyword(&hay[m.range()]) {
                    if let Some(bt) = ctx.token {
                        push_span(out, last..m.start(), bt);
                    }
                    push_span(out, m.range(), t);
                    last = m.end();
                }
            }
            if let Some(bt) = ctx.token {
                push_span(out, last..r.end, bt);
            }
        } else if let Some(t) = ctx.token {
            push_span(out, r, t);
        }
    }

    fn keyword(&self, word: &[u8]) -> Option<TokenId> {
        if self.keywords_ci {
            self.keywords.get(&word.to_ascii_lowercase()).copied()
        } else {
            self.keywords.get(word).copied()
        }
    }

    /// 固定桁の行。
    fn columns_line(&self, state: &LineState, line: &[u8], out: &mut Vec<TokenSpan>) -> LineState {
        let bounds = column_bounds(line);
        let byte_range = |cols: &Range<u32>| {
            let at = |c: u32| {
                bounds
                    .iter()
                    .find(|(_, col)| *col >= c)
                    .map_or(line.len(), |(b, _)| *b)
            };
            at(cols.start)..at(cols.end)
        };
        // 行全体の種類を決める桁（COBOL の標識領域の * など）
        for c in &self.columns {
            if let Some((re, t)) = &c.line_match {
                let r = byte_range(&c.cols);
                if !r.is_empty()
                    && re.is_match(
                        Input::new(line)
                            .span(r)
                            .anchored(regex_automata::Anchored::Yes),
                    )
                {
                    push_span(out, 0..line.len(), *t);
                    return state.clone();
                }
            }
        }
        let mut stack = state.0.clone();
        let mut spans = Vec::new();
        for c in &self.columns {
            let r = byte_range(&c.cols);
            if r.is_empty() {
                continue;
            }
            // 空白だけの範囲には色を付けない（空の一連番号領域など）
            if let Some(t) = c.token
                && !line[r.clone()].iter().all(|b| b.is_ascii_whitespace())
            {
                push_span(&mut spans, r.clone(), t);
            }
            if let Some(ctx) = c.context {
                // 範囲だけを行として扱う（`^` `$` は範囲の先頭・終わり）
                let mut part = Vec::new();
                let sub = &line[r.clone()];
                self.run(ctx, &mut stack, sub, 0..sub.len(), &mut part);
                let off = r.start as u32;
                spans.extend(part.into_iter().map(|s| TokenSpan {
                    range: s.range.start + off..s.range.end + off,
                    token: s.token,
                }));
            }
        }
        spans.sort_by_key(|s| s.range.start);
        for s in spans {
            push_span(out, s.range.start as usize..s.range.end as usize, s.token);
        }
        LineState(stack)
    }
}

/// 一致した範囲のトークン（グループごとの指定があればその部分をそのトークンにする）。
fn emit_match(
    out: &mut Vec<TokenSpan>,
    hay: &[u8],
    r: Range<usize>,
    token: Option<TokenId>,
    captures: &[(usize, TokenId)],
    caps: &Captures,
    keyword: &dyn Fn(&[u8]) -> Option<TokenId>,
) {
    let mut groups: Vec<(Range<usize>, TokenId)> = captures
        .iter()
        .filter_map(|&(g, t)| caps.get_group(g).map(|s| (s.range(), t)))
        .filter(|(s, _)| !s.is_empty())
        .map(|(s, t)| (s.clone(), keyword(&hay[s]).unwrap_or(t)))
        .collect();
    groups.sort_by_key(|(s, _)| s.start);
    let mut pos = r.start;
    for (s, t) in groups {
        if s.start < pos {
            continue;
        }
        if let Some(wt) = token {
            push_span(out, pos..s.start, wt);
        }
        push_span(out, s.clone(), t);
        pos = s.end;
    }
    if let Some(wt) = token {
        push_span(out, pos..r.end, wt);
    }
}

fn utf8_len(b: u8) -> usize {
    match b {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

/// 文字ごとの（バイト位置, 表示桁）。全角は 2 桁、タブ・不正なバイトは 1 桁。
fn column_bounds(line: &[u8]) -> Vec<(usize, u32)> {
    let mut out = Vec::with_capacity(line.len() + 1);
    let mut col = 0u32;
    let mut pos = 0usize;
    for chunk in line.utf8_chunks() {
        for c in chunk.valid().chars() {
            out.push((pos, col));
            col += if c == '\t' {
                1
            } else {
                c.width().unwrap_or(0) as u32
            };
            pos += c.len_utf8();
        }
        for _ in chunk.invalid() {
            out.push((pos, col));
            col += 1;
            pos += 1;
        }
    }
    out.push((pos, col));
    out
}
