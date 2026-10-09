//! PDF の文字列の取り出し（中身の検索のため。18 章 8.3）。
//!
//! 表示のための完全な解釈はせず、検索に足りるだけを読む:
//!
//! * `N G obj … endobj` を先頭から順に拾い、オブジェクト ストリーム（`/Type /ObjStm`）の中のものも足す
//!   （相互参照表は使わない。壊れたファイルでも読めるところまで読む）。
//! * ストリームは `/FlateDecode`（PNG の予測子を含む）だけを展開する。
//! * ページ（`/Type /Page`）ごとに内容ストリームの文字列の演算子（`Tj`・`TJ`・`'`・`"`）を拾い、
//!   フォントの `/ToUnicode`（CMap の `bfchar`・`bfrange`）で文字にする。`/ToUnicode` のない単純なフォントは
//!   1 バイトを WinAnsi（Latin-1）として読む。CID フォントで `/ToUnicode` がなければ読めない（飛ばす）。
//! * 暗号化された PDF は読まない。

use std::collections::HashMap;
use std::io;

/// PDF か（拡張子）。
pub fn is_pdf(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case("pdf"))
}

/// 1 つのオブジェクト。
#[derive(Clone, Debug, Default)]
struct Obj {
    /// 辞書などの本体（`stream` の前まで）
    body: Vec<u8>,
    /// ストリームの中身（展開する前）
    stream: Option<Vec<u8>>,
}

/// 字句。
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    /// 文字列（リテラル・16 進数のどちらも、生のバイト）
    Str(Vec<u8>),
    Name(Vec<u8>),
    ArrOpen,
    ArrClose,
    DictOpen,
    DictClose,
    /// 演算子・キーワード（`R`・`obj`・`Tj` など）
    Op(Vec<u8>),
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | 0)
}

fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// 字句に分ける。
struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a [u8]) -> Self {
        Lexer { s, i: 0 }
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() {
            let b = self.s[self.i];
            if is_ws(b) {
                self.i += 1;
            } else if b == b'%' {
                while self.i < self.s.len() && !matches!(self.s[self.i], b'\r' | b'\n') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next_tok(&mut self) -> Option<Tok> {
        self.skip_ws();
        let s = self.s;
        let b = *s.get(self.i)?;
        match b {
            b'(' => {
                self.i += 1;
                let mut out = Vec::new();
                let mut depth = 1;
                while self.i < s.len() {
                    let c = s[self.i];
                    self.i += 1;
                    match c {
                        b'\\' => {
                            let Some(&e) = s.get(self.i) else { break };
                            self.i += 1;
                            match e {
                                b'n' => out.push(b'\n'),
                                b'r' => out.push(b'\r'),
                                b't' => out.push(b'\t'),
                                b'b' => out.push(8),
                                b'f' => out.push(12),
                                b'\r' => {
                                    if s.get(self.i) == Some(&b'\n') {
                                        self.i += 1;
                                    }
                                }
                                b'\n' => {}
                                b'0'..=b'7' => {
                                    let mut v = (e - b'0') as u32;
                                    for _ in 0..2 {
                                        match s.get(self.i) {
                                            Some(&d @ b'0'..=b'7') => {
                                                v = v * 8 + (d - b'0') as u32;
                                                self.i += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    out.push(v as u8);
                                }
                                other => out.push(other),
                            }
                        }
                        b'(' => {
                            depth += 1;
                            out.push(c);
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                }
                Some(Tok::Str(out))
            }
            b'<' if s.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                Some(Tok::DictOpen)
            }
            b'>' if s.get(self.i + 1) == Some(&b'>') => {
                self.i += 2;
                Some(Tok::DictClose)
            }
            b'<' => {
                self.i += 1;
                let start = self.i;
                while self.i < s.len() && s[self.i] != b'>' {
                    self.i += 1;
                }
                let hex = &s[start..self.i];
                self.i = (self.i + 1).min(s.len());
                Some(Tok::Str(unhex(hex)))
            }
            b'>' => {
                self.i += 1;
                self.next_tok()
            }
            b'[' => {
                self.i += 1;
                Some(Tok::ArrOpen)
            }
            b']' => {
                self.i += 1;
                Some(Tok::ArrClose)
            }
            b'{' | b'}' => {
                self.i += 1;
                self.next_tok()
            }
            b'/' => {
                self.i += 1;
                let start = self.i;
                while self.i < s.len() && !is_ws(s[self.i]) && !is_delim(s[self.i]) {
                    self.i += 1;
                }
                Some(Tok::Name(s[start..self.i].to_vec()))
            }
            _ => {
                let start = self.i;
                while self.i < s.len() && !is_ws(s[self.i]) && !is_delim(s[self.i]) {
                    self.i += 1;
                }
                if self.i == start {
                    self.i += 1; // 知らない区切り（`)` だけなど）は飛ばす
                    return self.next_tok();
                }
                let w = &s[start..self.i];
                if let Some(n) = std::str::from_utf8(w)
                    .ok()
                    .filter(|t| t.bytes().all(|c| c.is_ascii_digit() || b"+-.".contains(&c)))
                    .and_then(|t| t.parse::<f64>().ok())
                {
                    Some(Tok::Num(n))
                } else {
                    Some(Tok::Op(w.to_vec()))
                }
            }
        }
    }
}

fn unhex(h: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = h
        .iter()
        .filter_map(|&c| (c as char).to_digit(16).map(|d| d as u8))
        .collect();
    digits
        .chunks(2)
        .map(|p| (p[0] << 4) | p.get(1).copied().unwrap_or(0))
        .collect()
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// 辞書の値（簡易）。
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Num(f64),
    Ref(u32),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Arr(Vec<Val>),
    Dict(HashMap<Vec<u8>, Val>),
    Other,
}

impl Val {
    fn dict(&self) -> Option<&HashMap<Vec<u8>, Val>> {
        match self {
            Val::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// 字句から値を 1 つ読む（`a b R` は参照）。
fn parse_val(toks: &[Tok], i: &mut usize, depth: usize) -> Val {
    if depth > 32 {
        return Val::Other;
    }
    let Some(t) = toks.get(*i) else {
        return Val::Other;
    };
    *i += 1;
    match t {
        Tok::Num(n) => {
            if let (Some(Tok::Num(_)), Some(Tok::Op(op))) = (toks.get(*i), toks.get(*i + 1))
                && op == b"R"
            {
                *i += 2;
                return Val::Ref(*n as u32);
            }
            Val::Num(*n)
        }
        Tok::Name(n) => Val::Name(n.clone()),
        Tok::Str(s) => Val::Str(s.clone()),
        Tok::ArrOpen => {
            let mut v = Vec::new();
            while let Some(t) = toks.get(*i) {
                if *t == Tok::ArrClose {
                    *i += 1;
                    break;
                }
                v.push(parse_val(toks, i, depth + 1));
            }
            Val::Arr(v)
        }
        Tok::DictOpen => {
            let mut d = HashMap::new();
            while let Some(t) = toks.get(*i) {
                match t {
                    Tok::DictClose => {
                        *i += 1;
                        break;
                    }
                    Tok::Name(k) => {
                        let k = k.clone();
                        *i += 1;
                        let v = parse_val(toks, i, depth + 1);
                        d.insert(k, v);
                    }
                    _ => *i += 1,
                }
            }
            Val::Dict(d)
        }
        _ => Val::Other,
    }
}

fn lex_all(s: &[u8]) -> Vec<Tok> {
    let mut l = Lexer::new(s);
    let mut v = Vec::new();
    while let Some(t) = l.next_tok() {
        v.push(t);
    }
    v
}

fn parse_body(body: &[u8]) -> Val {
    let toks = lex_all(body);
    let mut i = 0;
    parse_val(&toks, &mut i, 0)
}

/// PDF 全体。
struct Doc {
    objs: HashMap<u32, Obj>,
}

impl Doc {
    fn parse(data: &[u8]) -> io::Result<Doc> {
        if !data.starts_with(b"%PDF") && find(&data[..data.len().min(1024)], b"%PDF", 0).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PDF ではありません",
            ));
        }
        let mut objs = HashMap::new();
        let mut pos = 0;
        while let Some(at) = find(data, b" obj", pos) {
            pos = at + 4;
            // 直前の「番号 世代」
            let Some((num, _)) = obj_header(data, at) else {
                continue;
            };
            let start = at + 4;
            let end = find(data, b"endobj", start).unwrap_or(data.len());
            let (body, stream) = match find(&data[..end], b"stream", start) {
                Some(sp) if !data[start..sp].ends_with(b"end") => {
                    let mut ds = sp + 6;
                    if data.get(ds) == Some(&b'\r') {
                        ds += 1;
                    }
                    if data.get(ds) == Some(&b'\n') {
                        ds += 1;
                    }
                    let body = data[start..sp].to_vec();
                    let len =
                        parse_body(&body)
                            .dict()
                            .and_then(|d| match d.get(b"Length".as_slice()) {
                                Some(Val::Num(n)) => Some(*n as usize),
                                _ => None,
                            });
                    let se = match len {
                        Some(l)
                            if ds + l <= data.len()
                                && find(
                                    &data[..(ds + l + 16).min(data.len())],
                                    b"endstream",
                                    ds + l,
                                )
                                .is_some() =>
                        {
                            ds + l
                        }
                        _ => find(data, b"endstream", ds).unwrap_or(end),
                    };
                    let end2 = find(data, b"endobj", se).unwrap_or(data.len());
                    pos = end2;
                    (body, Some(data[ds..se.max(ds)].to_vec()))
                }
                _ => {
                    pos = end;
                    (data[start..end].to_vec(), None)
                }
            };
            objs.insert(num, Obj { body, stream });
        }
        let mut doc = Doc { objs };
        // 暗号化: 相互参照（trailer か XRef ストリーム）に /Encrypt があり、/Filter /Standard の辞書がある
        let encrypted = find(data, b"/Encrypt", 0).is_some()
            && doc.objs.values().any(|o| {
                o.stream.is_none()
                    && find(&o.body, b"Standard", 0).is_some()
                    && parse_body(&o.body).dict().is_some_and(|d| {
                        d.get(b"Filter".as_slice()) == Some(&Val::Name(b"Standard".to_vec()))
                    })
            });
        if encrypted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "暗号化された PDF は読めません",
            ));
        }
        doc.expand_object_streams();
        Ok(doc)
    }

    /// オブジェクト ストリームの中のオブジェクトを足す。
    fn expand_object_streams(&mut self) {
        let mut add = Vec::new();
        for o in self.objs.values() {
            let Some(raw) = &o.stream else { continue };
            let v = parse_body(&o.body);
            let Some(d) = v.dict() else { continue };
            if d.get(b"Type".as_slice()) != Some(&Val::Name(b"ObjStm".to_vec())) {
                continue;
            }
            let (Some(Val::Num(n)), Some(Val::Num(first))) =
                (d.get(b"N".as_slice()), d.get(b"First".as_slice()))
            else {
                continue;
            };
            let Some(data) = decode(d, raw) else { continue };
            let first = *first as usize;
            if first > data.len() {
                continue;
            }
            let head = lex_all(&data[..first]);
            let nums: Vec<usize> = head
                .iter()
                .filter_map(|t| match t {
                    Tok::Num(x) => Some(*x as usize),
                    _ => None,
                })
                .collect();
            let pairs: Vec<(usize, usize)> = nums
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| (c[0], c[1]))
                .take(*n as usize)
                .collect();
            for (k, &(num, off)) in pairs.iter().enumerate() {
                let s = first + off;
                let e = pairs
                    .get(k + 1)
                    .map_or(data.len(), |&(_, o2)| first + o2)
                    .min(data.len());
                if s < e {
                    add.push((
                        num as u32,
                        Obj {
                            body: data[s..e].to_vec(),
                            stream: None,
                        },
                    ));
                }
            }
        }
        for (k, o) in add {
            self.objs.entry(k).or_insert(o);
        }
    }

    fn get(&self, num: u32) -> Option<(Val, Option<&Obj>)> {
        let o = self.objs.get(&num)?;
        Some((parse_body(&o.body), Some(o)))
    }

    /// 参照ならたどる。
    fn resolve(&self, v: &Val) -> Val {
        let mut v = v.clone();
        for _ in 0..8 {
            match v {
                Val::Ref(n) => match self.get(n) {
                    Some((x, _)) => v = x,
                    None => return Val::Other,
                },
                _ => return v,
            }
        }
        Val::Other
    }

    /// ストリームを展開した中身。
    fn stream_of(&self, num: u32) -> Option<Vec<u8>> {
        let o = self.objs.get(&num)?;
        let raw = o.stream.as_ref()?;
        let v = parse_body(&o.body);
        decode(v.dict()?, raw)
    }
}

/// `at`（` obj` の位置）の直前の「番号 世代」。
fn obj_header(data: &[u8], at: usize) -> Option<(u32, u32)> {
    let mut i = at;
    let mut nums = Vec::new();
    for _ in 0..2 {
        let end = i;
        while i > 0 && data[i - 1].is_ascii_digit() {
            i -= 1;
        }
        if i == end {
            return None;
        }
        nums.push(
            std::str::from_utf8(&data[i..end])
                .ok()?
                .parse::<u32>()
                .ok()?,
        );
        if nums.len() == 2 {
            break;
        }
        if i == 0 || !is_ws(data[i - 1]) {
            return None;
        }
        while i > 0 && is_ws(data[i - 1]) {
            i -= 1;
        }
    }
    // 番号の前は区切り
    if i > 0 && !(is_ws(data[i - 1]) || is_delim(data[i - 1])) {
        return None;
    }
    Some((nums[1], nums[0]))
}

/// ストリームを展開する（`/FlateDecode` だけ。ほかの圧縮は `None`）。
fn decode(d: &HashMap<Vec<u8>, Val>, raw: &[u8]) -> Option<Vec<u8>> {
    let filters: Vec<Vec<u8>> = match d.get(b"Filter".as_slice()) {
        None => Vec::new(),
        Some(Val::Name(n)) => vec![n.clone()],
        Some(Val::Arr(a)) => a
            .iter()
            .filter_map(|v| match v {
                Val::Name(n) => Some(n.clone()),
                _ => None,
            })
            .collect(),
        _ => return None,
    };
    let mut data = raw.to_vec();
    for f in &filters {
        match f.as_slice() {
            b"FlateDecode" | b"Fl" => {
                data = inflate(&data)?;
                let parms = match d.get(b"DecodeParms".as_slice()) {
                    Some(Val::Dict(p)) => Some(p.clone()),
                    Some(Val::Arr(a)) => a.iter().find_map(|v| v.dict().cloned()),
                    _ => None,
                };
                if let Some(p) = parms
                    && let Some(Val::Num(pred)) = p.get(b"Predictor".as_slice())
                    && *pred >= 10.0
                {
                    let cols = match p.get(b"Columns".as_slice()) {
                        Some(Val::Num(c)) => *c as usize,
                        _ => 1,
                    };
                    data = unpredict(&data, cols.max(1));
                }
            }
            _ => return None,
        }
    }
    Some(data)
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    // zlib の頭があればそのまま、なければ生の deflate として試す
    miniz_oxide::inflate::decompress_to_vec_zlib(data)
        .or_else(|_| miniz_oxide::inflate::decompress_to_vec(data))
        .ok()
        .or_else(|| {
            // 末尾が壊れていても、展開できたところまで使う
            let mut st = miniz_oxide::inflate::stream::InflateState::new_boxed(
                miniz_oxide::DataFormat::Zlib,
            );
            let mut out = vec![0u8; data.len().saturating_mul(8).clamp(4096, 64 << 20)];
            let r = miniz_oxide::inflate::stream::inflate(
                &mut st,
                data,
                &mut out,
                miniz_oxide::MZFlush::Finish,
            );
            (r.bytes_written > 0).then(|| {
                out.truncate(r.bytes_written);
                out
            })
        })
}

/// PNG の予測子を外す（1 画素 1 バイトとして）。
fn unpredict(data: &[u8], cols: usize) -> Vec<u8> {
    let row = cols + 1;
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; cols];
    for chunk in data.chunks(row) {
        if chunk.len() < row {
            break;
        }
        let ty = chunk[0];
        let mut cur = chunk[1..].to_vec();
        for i in 0..cols {
            let left = if i > 0 { cur[i - 1] } else { 0 };
            let up = prev[i];
            let ul = if i > 0 { prev[i - 1] } else { 0 };
            cur[i] = match ty {
                1 => cur[i].wrapping_add(left),
                2 => cur[i].wrapping_add(up),
                3 => cur[i].wrapping_add(((left as u16 + up as u16) / 2) as u8),
                4 => {
                    let p = left as i16 + up as i16 - ul as i16;
                    let (pa, pb, pc) = (
                        (p - left as i16).abs(),
                        (p - up as i16).abs(),
                        (p - ul as i16).abs(),
                    );
                    let pr = if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        ul
                    };
                    cur[i].wrapping_add(pr)
                }
                _ => cur[i],
            };
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    out
}

/// フォントの文字の対応。
#[derive(Clone, Debug, Default)]
struct CMap {
    /// 符号の長さ（バイト）の候補（短い順）
    lens: Vec<usize>,
    map: HashMap<Vec<u8>, String>,
    /// `/ToUnicode` のない単純なフォント（1 バイト = Latin-1）
    simple: bool,
}

fn utf16be(b: &[u8]) -> String {
    let u: Vec<u16> = b
        .chunks(2)
        .map(|c| ((c[0] as u16) << 8) | c.get(1).copied().unwrap_or(0) as u16)
        .collect();
    String::from_utf16_lossy(&u)
}

fn inc(code: &mut [u8]) {
    for b in code.iter_mut().rev() {
        let (v, carry) = b.overflowing_add(1);
        *b = v;
        if !carry {
            break;
        }
    }
}

/// `/ToUnicode` の CMap を読む。
fn parse_cmap(data: &[u8]) -> CMap {
    let toks = lex_all(data);
    let mut c = CMap::default();
    let mut i = 0;
    let op = |t: &Tok, w: &[u8]| matches!(t, Tok::Op(o) if o == w);
    while i < toks.len() {
        if op(&toks[i], b"begincodespacerange") {
            i += 1;
            while i + 1 < toks.len() && !op(&toks[i], b"endcodespacerange") {
                if let Tok::Str(lo) = &toks[i]
                    && !c.lens.contains(&lo.len())
                {
                    c.lens.push(lo.len());
                }
                i += 2;
            }
        } else if op(&toks[i], b"beginbfchar") {
            i += 1;
            while i + 1 < toks.len() && !op(&toks[i], b"endbfchar") {
                if let (Tok::Str(src), Tok::Str(dst)) = (&toks[i], &toks[i + 1]) {
                    c.map.insert(src.clone(), utf16be(dst));
                    if !c.lens.contains(&src.len()) {
                        c.lens.push(src.len());
                    }
                }
                i += 2;
            }
        } else if op(&toks[i], b"beginbfrange") {
            i += 1;
            while i + 2 < toks.len() && !op(&toks[i], b"endbfrange") {
                let (Tok::Str(lo), Tok::Str(hi)) = (&toks[i], &toks[i + 1]) else {
                    i += 1;
                    continue;
                };
                if !c.lens.contains(&lo.len()) {
                    c.lens.push(lo.len());
                }
                let mut code = lo.clone();
                match &toks[i + 2] {
                    Tok::Str(dst) => {
                        let mut d = dst.clone();
                        let mut n = 0;
                        while code.len() == hi.len() && code <= *hi && n < 65536 {
                            c.map.insert(code.clone(), utf16be(&d));
                            inc(&mut code);
                            if d.is_empty() {
                                break;
                            }
                            inc(&mut d);
                            n += 1;
                            if code.iter().all(|&b| b == 0) {
                                break;
                            }
                        }
                        i += 3;
                    }
                    Tok::ArrOpen => {
                        let mut j = i + 3;
                        while j < toks.len() && toks[j] != Tok::ArrClose {
                            if let Tok::Str(dst) = &toks[j] {
                                c.map.insert(code.clone(), utf16be(dst));
                                inc(&mut code);
                            }
                            j += 1;
                        }
                        i = j + 1;
                    }
                    _ => i += 3,
                }
            }
        } else {
            i += 1;
        }
    }
    c.lens.sort_unstable();
    if c.lens.is_empty() {
        c.lens.push(1);
    }
    c
}

impl CMap {
    fn decode(&self, s: &[u8], out: &mut String) {
        if self.simple {
            out.extend(s.iter().map(|&b| win_ansi(b)));
            return;
        }
        let mut i = 0;
        while i < s.len() {
            let mut done = false;
            for &l in &self.lens {
                if i + l <= s.len()
                    && let Some(t) = self.map.get(&s[i..i + l])
                {
                    out.push_str(t);
                    i += l;
                    done = true;
                    break;
                }
            }
            if !done {
                // 対応がない符号は飛ばす（いちばん短い長さで進む）
                i += self.lens[0].max(1);
            }
        }
    }
}

/// WinAnsiEncoding の 1 バイト（0x80〜0x9F の主なものだけ。ほかは Latin-1）。
fn win_ansi(b: u8) -> char {
    match b {
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x80 => '\u{20AC}',
        0x85 => '\u{2026}',
        b if b < 0x20 && b != b'\t' => ' ',
        b => b as char,
    }
}

/// ページのフォント（名前 → 対応）を集める。`None` は読めないフォント（CID で `/ToUnicode` がない）。
fn page_fonts(
    doc: &Doc,
    page: &HashMap<Vec<u8>, Val>,
    cache: &mut HashMap<u32, Option<CMap>>,
) -> HashMap<Vec<u8>, Option<CMap>> {
    // /Resources は親から受け継ぐこともある
    let mut res = page.get(b"Resources".as_slice()).map(|v| doc.resolve(v));
    let mut cur = page.clone();
    for _ in 0..16 {
        if res.as_ref().and_then(Val::dict).is_some() {
            break;
        }
        let Some(parent) = cur.get(b"Parent".as_slice()).map(|v| doc.resolve(v)) else {
            break;
        };
        let Some(pd) = parent.dict().cloned() else {
            break;
        };
        res = pd.get(b"Resources".as_slice()).map(|v| doc.resolve(v));
        cur = pd;
    }
    let mut out = HashMap::new();
    let Some(res) = res else { return out };
    let Some(fonts) = res
        .dict()
        .and_then(|r| r.get(b"Font".as_slice()))
        .map(|f| doc.resolve(f))
    else {
        return out;
    };
    let Some(fonts) = fonts.dict() else {
        return out;
    };
    for (name, fv) in fonts {
        let key = match fv {
            Val::Ref(n) => Some(*n),
            _ => None,
        };
        if let Some(k) = key
            && let Some(c) = cache.get(&k)
        {
            out.insert(name.clone(), c.clone());
            continue;
        }
        let f = doc.resolve(fv);
        let cmap = f.dict().and_then(|fd| {
            if let Some(Val::Ref(tu)) = fd.get(b"ToUnicode".as_slice()) {
                if let Some(data) = doc.stream_of(*tu) {
                    return Some(parse_cmap(&data));
                }
            }
            let sub = match fd.get(b"Subtype".as_slice()) {
                Some(Val::Name(n)) => n.clone(),
                _ => Vec::new(),
            };
            (sub != b"Type0").then(|| CMap {
                lens: vec![1],
                simple: true,
                ..CMap::default()
            })
        });
        if let Some(k) = key {
            cache.insert(k, cmap.clone());
        }
        out.insert(name.clone(), cmap);
    }
    out
}

/// 内容ストリームから文字列を取り出す。
fn content_text(data: &[u8], fonts: &HashMap<Vec<u8>, Option<CMap>>, out: &mut String) {
    let mut l = Lexer::new(data);
    let mut stack: Vec<Tok> = Vec::new();
    let fallback = CMap {
        lens: vec![1],
        simple: true,
        ..CMap::default()
    };
    let mut font: Option<&CMap> = Some(&fallback);
    let newline = |out: &mut String| {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    };
    let mut arr_depth = 0usize;
    let mut arr: Vec<Tok> = Vec::new();
    while let Some(t) = l.next_tok() {
        match t {
            Tok::ArrOpen => {
                arr_depth += 1;
                if arr_depth == 1 {
                    arr.clear();
                }
            }
            Tok::ArrClose => {
                arr_depth = arr_depth.saturating_sub(1);
                if arr_depth == 0 {
                    stack.push(Tok::Op(b"[]".to_vec()));
                }
            }
            t if arr_depth > 0 => arr.push(t),
            Tok::Op(op) => {
                match op.as_slice() {
                    b"Tf" => {
                        if let Some(Tok::Name(n)) = stack.iter().rev().nth(1) {
                            font = fonts.get(n).map_or(Some(&fallback), |f| f.as_ref());
                        }
                    }
                    b"Tj" | b"'" | b"\"" => {
                        if op != b"Tj" {
                            newline(out);
                        }
                        if let (Some(Tok::Str(s)), Some(f)) = (stack.last(), font) {
                            f.decode(s, out);
                        }
                    }
                    b"TJ" => {
                        if let Some(f) = font {
                            for t in &arr {
                                match t {
                                    Tok::Str(s) => f.decode(s, out),
                                    // 大きく右へ動かす数は語の区切り
                                    Tok::Num(n) if *n < -200.0 && !out.ends_with(' ') => {
                                        out.push(' ')
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    b"T*" => newline(out),
                    b"Td" | b"TD" => {
                        if let Some(Tok::Num(ty)) = stack.last()
                            && ty.abs() > 0.01
                        {
                            newline(out);
                        } else if !out.ends_with([' ', '\n']) && !out.is_empty() {
                            out.push(' ');
                        }
                    }
                    b"Tm" => newline(out),
                    b"ET" => newline(out),
                    b"BI" => {
                        // 埋め込みの画像は「ID」から「EI」まで飛ばす
                        if let Some(id) = find(l.s, b"ID", l.i) {
                            let mut p = id + 2;
                            loop {
                                match find(l.s, b"EI", p) {
                                    Some(e)
                                        if e > 0
                                            && is_ws(l.s[e - 1])
                                            && l.s.get(e + 2).is_none_or(|&c| is_ws(c)) =>
                                    {
                                        l.i = e + 2;
                                        break;
                                    }
                                    Some(e) => p = e + 2,
                                    None => {
                                        l.i = l.s.len();
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                stack.clear();
            }
            t => {
                stack.push(t);
                if stack.len() > 64 {
                    stack.drain(..32);
                }
            }
        }
    }
    newline(out);
}

/// PDF の文字列を取り出す（ページの順。ページの間は改行）。
pub fn extract_text(data: &[u8]) -> io::Result<String> {
    let doc = Doc::parse(data)?;
    // ページの順: /Pages の木をたどる（見つからなければ番号の順）
    let mut pages: Vec<u32> = Vec::new();
    let root = doc.objs.iter().find_map(|(n, o)| {
        let v = parse_body(&o.body);
        let d = v.dict()?;
        (d.get(b"Type".as_slice()) == Some(&Val::Name(b"Catalog".to_vec())))
            .then(|| d.get(b"Pages".as_slice()).cloned())
            .flatten()
            .map(|p| (*n, p))
    });
    if let Some((_, Val::Ref(p))) = root {
        walk_pages(&doc, p, &mut pages, 0);
    }
    if pages.is_empty() {
        let mut v: Vec<u32> = doc
            .objs
            .iter()
            .filter(|(_, o)| {
                parse_body(&o.body).dict().is_some_and(|d| {
                    d.get(b"Type".as_slice()) == Some(&Val::Name(b"Page".to_vec()))
                })
            })
            .map(|(n, _)| *n)
            .collect();
        v.sort_unstable();
        pages = v;
    }
    let mut out = String::new();
    let mut font_cache = HashMap::new();
    for p in pages {
        let Some((pv, _)) = doc.get(p) else { continue };
        let Some(pd) = pv.dict() else { continue };
        let fonts = page_fonts(&doc, pd, &mut font_cache);
        let contents: Vec<u32> = match pd.get(b"Contents".as_slice()) {
            Some(Val::Ref(n)) => match doc.get(*n) {
                // 参照の先が配列のこともある
                Some((Val::Arr(a), _)) => a
                    .iter()
                    .filter_map(|v| match v {
                        Val::Ref(r) => Some(*r),
                        _ => None,
                    })
                    .collect(),
                _ => vec![*n],
            },
            Some(Val::Arr(a)) => a
                .iter()
                .filter_map(|v| match v {
                    Val::Ref(r) => Some(*r),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        // 内容ストリームはつなげて 1 つとして読む（途中で切れた演算子もある）
        let mut all = Vec::new();
        for c in contents {
            if let Some(d) = doc.stream_of(c) {
                all.extend_from_slice(&d);
                all.push(b'\n');
            }
        }
        content_text(&all, &fonts, &mut out);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

fn walk_pages(doc: &Doc, n: u32, out: &mut Vec<u32>, depth: usize) {
    if depth > 64 || out.len() > 100_000 {
        return;
    }
    let Some((v, _)) = doc.get(n) else { return };
    let Some(d) = v.dict() else { return };
    match d.get(b"Type".as_slice()) {
        Some(Val::Name(t)) if t == b"Page" => out.push(n),
        _ => {
            if let Some(Val::Arr(kids)) = d.get(b"Kids".as_slice()).map(|k| doc.resolve(k)) {
                for k in kids {
                    if let Val::Ref(r) = k {
                        walk_pages(doc, r, out, depth + 1);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 試験用の PDF を作る（オブジェクトの本体の並び。ストリームは `Some` の中身を FlateDecode する）。
    pub(crate) fn build(objs: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        for (k, (body, stream)) in objs.iter().enumerate() {
            // 空の本体はオブジェクト ストリームの中にある番号（ここには書かない）
            if body.is_empty() && stream.is_none() {
                continue;
            }
            out.extend_from_slice(format!("{} 0 obj\n", k + 1).as_bytes());
            match stream {
                Some(raw) => {
                    let z = miniz_oxide::deflate::compress_to_vec_zlib(raw, 6);
                    out.extend_from_slice(
                        format!(
                            "<< {body} /Length {} /Filter /FlateDecode >>\nstream\n",
                            z.len()
                        )
                        .as_bytes(),
                    );
                    out.extend_from_slice(&z);
                    out.extend_from_slice(b"\nendstream\n");
                }
                None => {
                    out.extend_from_slice(body.as_bytes());
                    out.push(b'\n');
                }
            }
            out.extend_from_slice(b"endobj\n");
        }
        out.extend_from_slice(b"trailer\n<< /Root 1 0 R >>\n%%EOF\n");
        out
    }

    #[test]
    fn extracts_latin_and_cid_text() {
        // 「見積」= U+898B U+7A4D を CID 0x0101・0x0102 で、「AB」を続いた bfrange で、「税込」を配列の bfrange で
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
            1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
            2 beginbfchar <0101> <898B> <0102> <7A4D> endbfchar\n\
            1 beginbfrange <0200> <0201> <0041> endbfrange\n\
            1 beginbfrange <0300> <0301> [<7A0E> <8FBC>] endbfrange\n\
            endcmap CMapName currentdict /CMap defineresource pop end end";
        let content = b"BT /F1 12 Tf 72 700 Td (Hello \\(PDF\\) World) Tj ET\n\
            BT /F2 10 Tf 72 680 Td <01010102> Tj 0 -14 Td [<0200> -50 <0201>] TJ\n\
            T* <03000301> Tj ET\n\
            BI /W 2 /H 2 /BPC 8 /CS /G ID \x01\x02\x03\x04 EI\n\
            BT /F1 12 Tf 1 0 0 1 72 600 Tm [(Total) -300 (1,000)] TJ ET";
        let pdf = build(&[
            ("<< /Type /Catalog /Pages 2 0 R >>", None),
            ("<< /Type /Pages /Kids [3 0 R] /Count 1 >>", None),
            (
                "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>",
                None,
            ),
            ("", Some(content)),
            (
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
                None,
            ),
            (
                "<< /Type /Font /Subtype /Type0 /BaseFont /MS-Gothic /Encoding /Identity-H /ToUnicode 7 0 R >>",
                None,
            ),
            ("", Some(cmap)),
        ]);
        let t = extract_text(&pdf).unwrap();
        assert!(t.contains("Hello (PDF) World"), "{t}");
        assert!(t.contains("見積"), "{t}");
        assert!(t.contains("\nAB\n"), "{t}");
        assert!(t.contains("税込"), "{t}");
        assert!(t.contains("Total 1,000"), "{t}");
        assert!(!t.contains('\u{1}'));
    }

    #[test]
    fn reads_object_streams_and_rejects_broken_files() {
        // ページとフォントをオブジェクト ストリームの中に置く
        let inner_objs = [
            (
                3u32,
                "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
            ),
            (5, "<< /Type /Font /Subtype /TrueType /BaseFont /Arial >>"),
        ];
        let mut head = String::new();
        let mut body = String::new();
        for (n, b) in inner_objs {
            head.push_str(&format!("{n} {} ", body.len()));
            body.push_str(b);
            body.push(' ');
        }
        let first = head.len();
        let objstm = format!("{head}{body}");
        let pdf = build(&[
            ("<< /Type /Catalog /Pages 2 0 R >>", None),
            ("<< /Type /Pages /Kids [3 0 R] /Count 1 >>", None),
            ("", None),
            ("", Some(b"BT /F1 9 Tf (in object stream) Tj ET")),
            ("", None),
            (
                &format!("/Type /ObjStm /N 2 /First {first}"),
                Some(objstm.as_bytes()),
            ),
        ]);
        let t = extract_text(&pdf).unwrap();
        assert!(t.contains("in object stream"), "{t}");
        assert!(extract_text(b"not a pdf").is_err());
        // 途中で切れたファイルでも落ちない
        for cut in [10, pdf.len() / 3, pdf.len() / 2, pdf.len() - 5] {
            let _ = extract_text(&pdf[..cut]);
        }
        assert!(is_pdf("a.PDF") && !is_pdf("a.pdfx"));
        // 暗号化
        let enc = build(&[
            ("<< /Type /Catalog /Pages 2 0 R >>", None),
            (
                "<< /Filter /Standard /V 2 /R 3 /O <00> /U <00> /P -4 >>",
                None,
            ),
        ]);
        let mut enc = enc;
        enc.extend_from_slice(b"trailer << /Root 1 0 R /Encrypt 2 0 R >>\n");
        assert!(extract_text(&enc).is_err());
    }

    #[test]
    fn lexes_strings_and_predictors() {
        let mut l = Lexer::new(b"(a\\101\\nb(c)) <48 65 6C6C6F> /Name 1.5 -2 [ ] << >> Tj");
        assert_eq!(l.next_tok(), Some(Tok::Str(b"aA\nb(c)".to_vec())));
        assert_eq!(l.next_tok(), Some(Tok::Str(b"Hello".to_vec())));
        assert_eq!(l.next_tok(), Some(Tok::Name(b"Name".to_vec())));
        assert_eq!(l.next_tok(), Some(Tok::Num(1.5)));
        assert_eq!(l.next_tok(), Some(Tok::Num(-2.0)));
        assert_eq!(l.next_tok(), Some(Tok::ArrOpen));
        assert_eq!(l.next_tok(), Some(Tok::ArrClose));
        assert_eq!(l.next_tok(), Some(Tok::DictOpen));
        assert_eq!(l.next_tok(), Some(Tok::DictClose));
        assert_eq!(l.next_tok(), Some(Tok::Op(b"Tj".to_vec())));
        assert_eq!(l.next_tok(), None);
        // 予測子 2（上と同じ）: 2 行 × 3 列
        assert_eq!(unpredict(&[2, 1, 2, 3, 2, 1, 1, 1], 3), [1, 2, 3, 2, 3, 4]);
    }
}
