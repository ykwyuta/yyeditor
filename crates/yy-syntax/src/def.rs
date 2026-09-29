//! 定義ファイル（TOML、10 章 4）の読み込みとコンパイル。
//!
//! ```toml
//! name = "C"
//! extensions = ["c", "h"]
//! line_comment = "//"
//! block_comment = ["/*", "*/"]
//!
//! [keywords]
//! keyword = ["if", "else", "for"]
//! type = ["int", "char"]
//! case_sensitive = true
//!
//! [[context.main]]
//! match = '//.*$'
//! token = "comment"
//!
//! [[context.main]]
//! begin = '/\*'
//! end = '\*/'
//! token = "comment"
//! ```

use std::collections::{BTreeMap, HashMap};

use regex_automata::meta::Regex;
use serde::Deserialize;

use crate::{Column, Ctx, Pat, Syntax, TokenId};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DefFile {
    pub name: String,
    /// TextMate のスコープ名（説明用）
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    /// ファイル名の完全一致（`Makefile` など）
    #[serde(default)]
    pub filenames: Vec<String>,
    /// 先頭行のパターン（`#!.*python` など）
    #[serde(default)]
    pub first_line: Option<String>,
    /// モードラインなどで使う別名
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    line_comment: Option<String>,
    #[serde(default)]
    block_comment: Option<(String, String)>,
    /// 対応を探す括弧（`"()[]{}"`）
    #[serde(default)]
    brackets: Option<String>,
    /// 識別子（キーワードとして引く単語）のパターン
    #[serde(default)]
    word: Option<String>,
    /// この行頭では必ず初期状態
    #[serde(default)]
    sync: Option<String>,
    #[serde(default)]
    keywords: toml::Table,
    #[serde(default)]
    context: BTreeMap<String, Vec<RuleDef>>,
    #[serde(default)]
    columns: Vec<ColumnDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDef {
    #[serde(rename = "match")]
    pattern: Option<String>,
    begin: Option<String>,
    end: Option<String>,
    skip: Option<String>,
    token: Option<String>,
    /// グループ番号 → トークン（`match` または `begin` のグループ）
    #[serde(default)]
    captures: BTreeMap<String, String>,
    /// 範囲の中で有効にするコンテキスト
    #[serde(default)]
    contains: Contains,
    /// 行頭でのみ有効
    #[serde(default)]
    line_start: bool,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum Contains {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl Contains {
    fn names(&self) -> Vec<String> {
        match self {
            Contains::None => Vec::new(),
            Contains::One(s) => vec![s.clone()],
            Contains::Many(v) => v.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnDef {
    /// 表示桁（1 始まり、両端を含む）
    range: [u32; 2],
    token: Option<String>,
    #[serde(rename = "match")]
    pattern: Option<String>,
    line_token: Option<String>,
    context: Option<String>,
}

/// コンパイル前のコンテキスト。
struct CtxBuild {
    pats: Vec<(String, Pat)>,
    token: Option<TokenId>,
    keywords: bool,
    contains: Vec<String>,
}

struct Builder {
    tokens: Vec<String>,
    ctx: Vec<CtxBuild>,
    names: HashMap<String, u16>,
}

impl Builder {
    fn token(&mut self, name: &str) -> TokenId {
        match self.tokens.iter().position(|t| t == name) {
            Some(i) => i as TokenId,
            None => {
                self.tokens.push(name.to_owned());
                (self.tokens.len() - 1) as TokenId
            }
        }
    }

    fn rule(&mut self, ctx: &str, r: &RuleDef) -> Result<(String, Pat), String> {
        let wrap = |p: &str| {
            if r.line_start {
                format!("^(?:{p})")
            } else {
                p.to_owned()
            }
        };
        let token = r.token.as_deref().map(|t| self.token(t));
        let mut captures = Vec::new();
        for (g, t) in &r.captures {
            let g: usize = g
                .parse()
                .map_err(|_| format!("{ctx}: captures のキーはグループ番号です: {g}"))?;
            captures.push((g, self.token(t)));
        }
        match (&r.pattern, &r.begin, &r.end) {
            (Some(p), None, None) => Ok((
                wrap(p),
                Pat::Rule {
                    token,
                    captures,
                    push: None,
                },
            )),
            (None, Some(b), Some(e)) => {
                let id = self.ctx.len() as u16;
                let mut pats = vec![(e.clone(), Pat::End)];
                if let Some(s) = &r.skip {
                    pats.push((s.clone(), Pat::Skip));
                }
                let contains = r.contains.names();
                self.ctx.push(CtxBuild {
                    pats,
                    token,
                    keywords: !contains.is_empty(),
                    contains,
                });
                Ok((
                    wrap(b),
                    Pat::Rule {
                        token,
                        captures,
                        push: Some(id),
                    },
                ))
            }
            _ => Err(format!(
                "{ctx}: ルールには match か、begin と end の組を書いてください"
            )),
        }
    }
}

fn compile(patterns: &[String], what: &str) -> Result<Option<Regex>, String> {
    if patterns.is_empty() {
        return Ok(None);
    }
    Regex::new_many(patterns)
        .map(Some)
        .map_err(|e| format!("{what}: 正規表現が不正です: {e}"))
}

fn compile_one(p: &str, what: &str) -> Result<Regex, String> {
    Regex::new(p).map_err(|e| format!("{what}: 正規表現が不正です: {e}"))
}

/// 定義ファイルを読む。`id` はファイル名（拡張子を除く）。
pub fn parse(id: &str, text: &str) -> Result<Syntax, String> {
    let def: DefFile = toml::from_str(text).map_err(|e| e.to_string())?;
    build(id, def)
}

pub(crate) fn parse_def(text: &str) -> Result<DefFile, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

pub(crate) fn build(id: &str, def: DefFile) -> Result<Syntax, String> {
    let _ = &def.scope;
    let mut b = Builder {
        tokens: Vec::new(),
        ctx: Vec::new(),
        names: HashMap::new(),
    };
    // 名前付きのコンテキスト（main を 0 番にする）
    let mut order: Vec<&String> = def.context.keys().collect();
    order.sort_by_key(|n| *n != "main");
    if !def.context.contains_key("main") {
        b.ctx.push(CtxBuild {
            pats: Vec::new(),
            token: None,
            keywords: true,
            contains: Vec::new(),
        });
        b.names.insert("main".to_owned(), 0);
    }
    for name in &order {
        b.names.insert((*name).clone(), b.ctx.len() as u16);
        b.ctx.push(CtxBuild {
            pats: Vec::new(),
            token: None,
            keywords: true,
            contains: Vec::new(),
        });
    }
    for name in &order {
        let id = b.names[*name] as usize;
        for r in &def.context[*name] {
            let p = b.rule(&format!("context.{name}"), r)?;
            b.ctx[id].pats.push(p);
        }
    }
    // 範囲の中で有効にするコンテキストのルールを加える
    for i in 0..b.ctx.len() {
        for name in b.ctx[i].contains.clone() {
            let &src = b
                .names
                .get(&name)
                .ok_or_else(|| format!("contains: コンテキスト {name} がありません"))?;
            let extra = b.ctx[src as usize].pats.clone();
            b.ctx[i].pats.extend(extra);
        }
    }
    let mut keywords = HashMap::new();
    let mut keywords_ci = false;
    for (k, v) in &def.keywords {
        if k == "case_sensitive" {
            keywords_ci = !v
                .as_bool()
                .ok_or("keywords.case_sensitive は true / false です")?;
            continue;
        }
        let list = v
            .as_array()
            .ok_or_else(|| format!("keywords.{k} は文字列の配列です"))?;
        let t = b.token(k);
        for w in list {
            let w = w
                .as_str()
                .ok_or_else(|| format!("keywords.{k} は文字列の配列です"))?;
            keywords.insert(w.as_bytes().to_vec(), t);
        }
    }
    if keywords_ci {
        keywords = keywords
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect();
    }
    let mut columns = Vec::new();
    for c in &def.columns {
        let [a, z] = c.range;
        if a == 0 || z < a {
            return Err(format!(
                "columns: range は 1 始まりの [開始, 終了] です: {a}, {z}"
            ));
        }
        let line_match = match (&c.pattern, &c.line_token) {
            (Some(p), Some(t)) => Some((compile_one(p, "columns")?, b.token(t))),
            (None, None) => None,
            _ => return Err("columns: match と line_token は組で書いてください".to_owned()),
        };
        let context = match &c.context {
            Some(n) => Some(
                *b.names
                    .get(n)
                    .ok_or_else(|| format!("columns: コンテキスト {n} がありません"))?,
            ),
            None => None,
        };
        let token = c.token.as_deref().map(|t| b.token(t));
        columns.push(Column {
            cols: a - 1..z,
            token,
            line_match,
            context,
        });
    }
    // 行末でも終わる範囲（`end = '"|$'` など）は行をまたがない
    let ends_at_eol = |p: &str| p == "$" || p.ends_with("|$");
    let multiline = b.ctx.iter().any(|c| {
        c.pats
            .first()
            .is_some_and(|(p, pat)| matches!(pat, Pat::End) && !ends_at_eol(p))
    });
    let mut contexts = Vec::with_capacity(b.ctx.len());
    for (i, c) in b.ctx.iter().enumerate() {
        let (srcs, pats): (Vec<String>, Vec<Pat>) = c.pats.iter().cloned().unzip();
        contexts.push(Ctx {
            re: compile(&srcs, &format!("コンテキスト {i}"))?,
            pats,
            token: c.token,
            keywords: c.keywords,
        });
    }
    let brackets = def
        .brackets
        .as_deref()
        .unwrap_or("()[]{}")
        .chars()
        .collect::<Vec<_>>()
        .chunks(2)
        .filter(|p| p.len() == 2)
        .map(|p| (p[0], p[1]))
        .collect();
    Ok(Syntax {
        id: id.to_owned(),
        name: def.name,
        line_comment: def.line_comment,
        block_comment: def.block_comment,
        brackets,
        tokens: b.tokens,
        contexts,
        keywords,
        keywords_ci,
        word: compile_one(
            def.word.as_deref().unwrap_or(r"[\p{L}_][\p{L}\p{N}_]*"),
            "word",
        )?,
        columns,
        sync: def
            .sync
            .as_deref()
            .map(|p| compile_one(p, "sync"))
            .transpose()?,
        multiline,
    })
}
