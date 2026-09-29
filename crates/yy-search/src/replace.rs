//! 置換（05 章 4）。

use std::io::{self, Write};
use std::ops::Range;

use regex_automata::PatternID;
use regex_automata::util::captures::Captures;
use yy_buffer::Snapshot;

use crate::{Cancelled, Searcher};

#[derive(Debug)]
pub enum ReplaceError {
    /// 置換文字列の誤り
    Template(String),
    Cancelled,
    Io(io::Error),
}

impl std::fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplaceError::Template(s) => f.write_str(s),
            ReplaceError::Cancelled => f.write_str("中止しました"),
            ReplaceError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ReplaceError {}

impl From<Cancelled> for ReplaceError {
    fn from(_: Cancelled) -> Self {
        ReplaceError::Cancelled
    }
}

impl From<io::Error> for ReplaceError {
    fn from(e: io::Error) -> Self {
        ReplaceError::Io(e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Lit(Vec<u8>),
    Group(usize),
    /// `\U`（以降を大文字に）
    Upper,
    /// `\L`（以降を小文字に）
    Lower,
    /// `\E`（大文字・小文字の変換を終える）
    End,
    /// `\u`（次の 1 文字を大文字に）
    NextUpper,
    /// `\l`（次の 1 文字を小文字に）
    NextLower,
}

/// コンパイル済みの置換文字列。
///
/// 正規表現の場合に使える記法:
/// `$0` `$1` `${name}` `\0`〜`\9`（キャプチャ）、`$$`（`$`）、`\n` `\t` `\r` `\\`、
/// `\U` `\L` `\E` `\u` `\l`（大文字・小文字の変換）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    toks: Vec<Tok>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    None,
    Upper,
    Lower,
}

fn push_lit(toks: &mut Vec<Tok>, b: &[u8]) {
    if let Some(Tok::Lit(v)) = toks.last_mut() {
        v.extend_from_slice(b);
    } else {
        toks.push(Tok::Lit(b.to_vec()));
    }
}

impl Replacement {
    /// 文字列そのもので置き換える。
    pub fn literal(s: &str) -> Replacement {
        Replacement {
            toks: vec![Tok::Lit(s.as_bytes().to_vec())],
        }
    }

    /// 正規表現用の置換文字列を解釈する。存在しないキャプチャを参照していればエラー。
    pub fn parse(s: &str, searcher: &Searcher) -> Result<Replacement, ReplaceError> {
        let info = searcher.regex().group_info();
        let groups = info.group_len(PatternID::ZERO);
        let group = |i: usize| -> Result<Tok, ReplaceError> {
            if i < groups {
                Ok(Tok::Group(i))
            } else {
                Err(ReplaceError::Template(format!(
                    "置換文字列の ${i} に対応するグループがありません"
                )))
            }
        };
        let mut toks = Vec::new();
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        let mut buf = [0u8; 4];
        while i < chars.len() {
            let c = chars[i];
            match c {
                '$' => match chars.get(i + 1) {
                    Some('$') => {
                        push_lit(&mut toks, b"$");
                        i += 2;
                    }
                    Some('{') => {
                        let close =
                            chars[i + 2..]
                                .iter()
                                .position(|&c| c == '}')
                                .ok_or_else(|| {
                                    ReplaceError::Template(
                                        "置換文字列の ${ が閉じていません".into(),
                                    )
                                })?;
                        let name: String = chars[i + 2..i + 2 + close].iter().collect();
                        let tok = match name.parse::<usize>() {
                            Ok(n) => group(n)?,
                            Err(_) => match info.to_index(PatternID::ZERO, &name) {
                                Some(n) => Tok::Group(n),
                                None => {
                                    return Err(ReplaceError::Template(format!(
                                        "置換文字列の ${{{name}}} に対応するグループがありません"
                                    )));
                                }
                            },
                        };
                        toks.push(tok);
                        i += close + 3;
                    }
                    Some(d) if d.is_ascii_digit() => {
                        let mut j = i + 1;
                        let mut n = 0usize;
                        while let Some(d) = chars.get(j).and_then(|c| c.to_digit(10)) {
                            n = n * 10 + d as usize;
                            j += 1;
                        }
                        toks.push(group(n)?);
                        i = j;
                    }
                    _ => {
                        push_lit(&mut toks, b"$");
                        i += 1;
                    }
                },
                '\\' => {
                    let Some(&e) = chars.get(i + 1) else {
                        push_lit(&mut toks, b"\\");
                        i += 1;
                        continue;
                    };
                    match e {
                        'n' => push_lit(&mut toks, b"\n"),
                        't' => push_lit(&mut toks, b"\t"),
                        'r' => push_lit(&mut toks, b"\r"),
                        '\\' => push_lit(&mut toks, b"\\"),
                        '$' => push_lit(&mut toks, b"$"),
                        'U' => toks.push(Tok::Upper),
                        'L' => toks.push(Tok::Lower),
                        'E' => toks.push(Tok::End),
                        'u' => toks.push(Tok::NextUpper),
                        'l' => toks.push(Tok::NextLower),
                        d if d.is_ascii_digit() => toks.push(group(d as usize - '0' as usize)?),
                        other => {
                            // 知らないエスケープはそのまま
                            push_lit(&mut toks, b"\\");
                            push_lit(&mut toks, other.encode_utf8(&mut buf).as_bytes());
                        }
                    }
                    i += 2;
                }
                c => {
                    push_lit(&mut toks, c.encode_utf8(&mut buf).as_bytes());
                    i += 1;
                }
            }
        }
        Ok(Replacement { toks })
    }

    /// 置換文字列が固定（キャプチャ・大文字小文字変換を使わない）ならその内容。
    pub fn as_literal(&self) -> Option<&[u8]> {
        match self.toks.as_slice() {
            [] => Some(b""),
            [Tok::Lit(v)] => Some(v),
            _ => None,
        }
    }

    fn needs_captures(&self) -> bool {
        self.as_literal().is_none()
    }

    /// マッチ 1 つ分の置換結果を `out` に追加する。`hay` はキャプチャの位置の基準。
    fn expand(&self, hay: &[u8], caps: Option<&Captures>, out: &mut Vec<u8>) {
        let mut case = Case::None;
        let mut next = Case::None;
        for t in &self.toks {
            match t {
                Tok::Lit(v) => emit(v, &mut case, &mut next, out),
                Tok::Group(i) => {
                    if let Some(sp) = caps.and_then(|c| c.get_group(*i)) {
                        emit(&hay[sp.start..sp.end], &mut case, &mut next, out);
                    }
                }
                Tok::Upper => case = Case::Upper,
                Tok::Lower => case = Case::Lower,
                Tok::End => case = Case::None,
                Tok::NextUpper => next = Case::Upper,
                Tok::NextLower => next = Case::Lower,
            }
        }
    }
}

/// 大文字・小文字の変換をしながら追加する（不正な UTF-8 のバイトはそのまま）。
fn emit(bytes: &[u8], case: &mut Case, next: &mut Case, out: &mut Vec<u8>) {
    if *case == Case::None && *next == Case::None {
        out.extend_from_slice(bytes);
        return;
    }
    let mut buf = [0u8; 4];
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            let mode = if *next != Case::None {
                std::mem::replace(next, Case::None)
            } else {
                *case
            };
            match mode {
                Case::Upper => c.to_uppercase().for_each(|u| {
                    out.extend_from_slice(u.encode_utf8(&mut buf).as_bytes());
                }),
                Case::Lower => c.to_lowercase().for_each(|u| {
                    out.extend_from_slice(u.encode_utf8(&mut buf).as_bytes());
                }),
                Case::None => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
            }
        }
        out.extend_from_slice(chunk.invalid());
    }
}

/// 置換 1 つ分（範囲, 置換後の内容）。
pub type Edit = (Range<u64>, Vec<u8>);

/// `range` 内のすべてのマッチの（範囲, 置換後の内容）。`limit` 件を超えたら `None`。
pub fn collect_edits(
    searcher: &Searcher,
    snap: &Snapshot,
    range: Range<u64>,
    repl: &Replacement,
    limit: usize,
    step: &mut dyn FnMut(u64) -> bool,
) -> Result<Option<Vec<Edit>>, Cancelled> {
    let mut out = Vec::new();
    let mut over = false;
    let mut caps = repl
        .needs_captures()
        .then(|| searcher.regex().create_captures());
    searcher.scan(
        snap,
        range.clone(),
        range.start,
        caps.as_mut(),
        step,
        &mut |m, w| {
            if out.len() >= limit {
                over = true;
                return false;
            }
            let mut v = Vec::new();
            match w {
                Some((w, c)) => repl.expand(&w.buf, Some(c), &mut v),
                None => repl.expand(&[], None, &mut v),
            }
            out.push((m, v));
            true
        },
    )?;
    Ok((!over).then_some(out))
}

/// 文書全体を、`range` 内のマッチを置き換えながら `w` に書き出す。置き換えた数を返す。
///
/// 書き出した内容の大きさに関係なくメモリの使用量は一定（ウィンドウ 1 つ分）。
pub fn rewrite(
    searcher: &Searcher,
    snap: &Snapshot,
    range: Range<u64>,
    repl: &Replacement,
    w: &mut dyn Write,
    step: &mut dyn FnMut(u64) -> bool,
) -> Result<u64, ReplaceError> {
    let mut caps = repl
        .needs_captures()
        .then(|| searcher.regex().create_captures());
    let mut copied = 0u64;
    let mut count = 0u64;
    let mut err: Option<io::Error> = None;
    let mut out = Vec::new();
    let copy = |w: &mut dyn Write, r: Range<u64>| -> io::Result<()> {
        for c in snap.chunks(r) {
            w.write_all(c)?;
        }
        Ok(())
    };
    searcher.scan(
        snap,
        range.clone(),
        range.start,
        caps.as_mut(),
        step,
        &mut |m, win| {
            out.clear();
            match win {
                Some((win, c)) => repl.expand(&win.buf, Some(c), &mut out),
                None => repl.expand(&[], None, &mut out),
            }
            let r = copy(w, copied..m.start).and_then(|_| w.write_all(&out));
            if let Err(e) = r {
                err = Some(e);
                return false;
            }
            copied = m.end;
            count += 1;
            true
        },
    )?;
    if let Some(e) = err {
        return Err(e.into());
    }
    copy(w, copied..snap.len())?;
    Ok(count)
}
