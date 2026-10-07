use std::collections::BTreeMap;

use yy_numfmt::DateSystem;

use super::*;

/// シートの名前と、（行, 列）→ 値。
type MemSheet = (String, BTreeMap<(u64, u32), Val>);

/// メモリ上の格子。
struct Mem {
    sheets: Vec<MemSheet>,
    cache: Cache,
}

impl Mem {
    fn new() -> Mem {
        Mem {
            sheets: vec![("Sheet1".into(), BTreeMap::new())],
            cache: Cache::default(),
        }
    }

    /// `A1` の形で値を入れる。
    fn set(&mut self, sheet: usize, cell: &str, v: Val) {
        let Ok(Expr::Ref(r)) = parse(cell) else {
            panic!("{cell}");
        };
        self.sheets[sheet].1.insert((r.area.r0, r.area.c0), v);
    }

    fn eval(&self, f: &str) -> Val {
        let e = parse(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        eval(
            &e,
            &Context {
                grid: self,
                sheet: 0,
                sys: DateSystem::D1900,
                cache: Some(&self.cache),
                offset: 0,
            },
        )
    }
}

impl Grid for Mem {
    fn sheet(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| eq_text(&s.0, name))
    }

    fn get(&self, sheet: usize, row: u64, col: u32) -> Val {
        self.sheets[sheet]
            .1
            .get(&(row, col))
            .cloned()
            .unwrap_or_default()
    }

    fn used(&self, sheet: usize) -> (u64, u32) {
        let m = &self.sheets[sheet].1;
        (
            m.keys().map(|k| k.0 + 1).max().unwrap_or(0),
            m.keys().map(|k| k.1 + 1).max().unwrap_or(0),
        )
    }
}

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn t(s: &str) -> Val {
    Val::text(s)
}
fn arr(rows: usize, cols: usize, v: Vec<Val>) -> Val {
    Val::Array(Arc::new(Array::new(rows, cols, v)))
}

#[test]
fn parses_and_prints() {
    for (src, want) in [
        ("=1+2*3", "=1+2*3"),
        ("=-2^2", "=-2^2"),
        (
            "=SUMIFS($C:$C,A:A,\"東京\",B2:B10,\">=\"&D1)",
            "=SUMIFS($C:$C,A:A,\"東京\",B2:B10,\">=\"&D1)",
        ),
        (
            "=xlookup(A1,Sheet2!A:A,'売上 2026'!B:C,,1)",
            "=XLOOKUP(A1,Sheet2!A:A,'売上 2026'!B:C,,1)",
        ),
        ("= ( 1 + 2 ) % ", "=(1+2)%"),
        ("={1,2;3,4}", "={1,2;3,4}"),
        ("=\"a\"\"b\"&TRUE", "=\"a\"\"b\"&TRUE"),
        ("=1:3", "=1:3"),
        ("=B3:A1", "=A1:B3"),
        ("=#N/A", "=#N/A"),
    ] {
        let e = parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(print::formula_text(&e), want, "{src}");
    }
    assert!(parse("=1+").is_err());
    assert!(parse("=(1").is_err());
    assert!(parse("=SUM(1,2").is_err());
    assert!(matches!(
        parse("=FOO(1)"),
        Ok(Expr::Call(Func::Unknown(_), _))
    ));
    assert!(matches!(
        parse("=LOG10(1)"),
        Ok(Expr::Call(Func::Unknown(_), _))
    ));
}

#[test]
fn arithmetic_like_excel() {
    let mut g = Mem::new();
    g.set(0, "A1", n(2.0));
    g.set(0, "A2", t("3"));
    g.set(0, "A3", t("abc"));
    g.set(0, "A4", Val::Bool(true));
    assert_eq!(g.eval("=-2^2"), n(4.0));
    assert_eq!(g.eval("=2^3^2"), n(64.0));
    assert_eq!(g.eval("=1+2*3-4/2"), n(5.0));
    assert_eq!(g.eval("=A1+A2"), n(5.0));
    assert_eq!(g.eval("=A1*A4"), n(2.0));
    assert_eq!(g.eval("=A1+A3"), Val::Err(Error::Value));
    assert_eq!(g.eval("=A1/0"), Val::Err(Error::Div0));
    assert_eq!(g.eval("=A9+1"), n(1.0));
    assert_eq!(g.eval("=50%"), n(0.5));
    assert_eq!(g.eval("=0.1+0.2=0.3"), Val::Bool(true));
    assert_eq!(g.eval("=\"b\">\"A\""), Val::Bool(true));
    assert_eq!(g.eval("=\"abc\"=\"ABC\""), Val::Bool(true));
    assert_eq!(g.eval("=1<\"a\""), Val::Bool(true));
    assert_eq!(g.eval("=A1&\"-\"&A4&1.5"), t("2-TRUE1.5"));
    assert_eq!(g.eval("=\"2026/10/7\"+1"), n(46303.0));
    assert_eq!(g.eval("=FOO(1)"), Val::Err(Error::Name));
    assert_eq!(g.eval("=Nope!A1"), Val::Err(Error::Ref));
    // 範囲どうしは要素ごと
    g.set(0, "B1", n(10.0));
    g.set(0, "B2", n(20.0));
    assert_eq!(g.eval("=A1:A2*B1:B2"), arr(2, 1, vec![n(20.0), n(60.0)]));
    assert_eq!(
        g.eval("={1,2}+{10;20}"),
        arr(2, 2, vec![n(11.0), n(12.0), n(21.0), n(22.0)])
    );
    assert_eq!(
        g.eval("=ABS({-1,2,-3})"),
        arr(1, 3, vec![n(1.0), n(2.0), n(3.0)])
    );
}

#[test]
fn product_and_abs() {
    let mut g = Mem::new();
    g.set(0, "A1", n(2.0));
    g.set(0, "A2", t("3"));
    g.set(0, "A3", n(4.0));
    g.set(0, "A4", Val::Bool(true));
    assert_eq!(g.eval("=PRODUCT(A1:A4)"), n(8.0));
    assert_eq!(g.eval("=PRODUCT(A1:A4,\"3\",TRUE)"), n(24.0));
    assert_eq!(g.eval("=PRODUCT(A2)"), n(0.0));
    assert_eq!(g.eval("=PRODUCT(\"x\")"), Val::Err(Error::Value));
    assert_eq!(g.eval("=ABS(-A3)"), n(4.0));
    assert_eq!(g.eval("=ABS(\"-5\")"), n(5.0));
}

#[test]
fn sum_and_count() {
    let mut g = Mem::new();
    g.set(0, "A1", n(2.0));
    g.set(0, "A2", t("3"));
    g.set(0, "A3", n(4.0));
    g.set(0, "A4", Val::Bool(true));
    g.set(0, "B1", n(10.0));
    g.set(0, "B3", n(0.5));
    // 範囲の中の文字列・真偽値・空は無視
    assert_eq!(g.eval("=SUM(A1:A4)"), n(6.0));
    assert_eq!(g.eval("=SUM(A1:B4)"), n(16.5));
    assert_eq!(g.eval("=SUM(A:A)"), n(6.0));
    assert_eq!(g.eval("=SUM(A2)"), n(0.0));
    // 直接書いた値は数値に変える
    assert_eq!(g.eval("=SUM(A1,\"3\",TRUE,1.5)"), n(7.5));
    assert_eq!(g.eval("=SUM(\"x\")"), Val::Err(Error::Value));
    assert_eq!(g.eval("=SUM({1,2;3,4})"), n(10.0));
    // 演算では文字列の数値も数値になる（"3"*2 = 6）
    assert_eq!(g.eval("=SUM(A1:A3*2)"), n(18.0));
    assert_eq!(g.eval("=SUM()"), Val::Err(Error::Value));
    // エラーは伝える
    g.set(0, "C1", Val::Err(Error::Div0));
    assert_eq!(g.eval("=SUM(C1:C2)"), Val::Err(Error::Div0));
    // COUNT: 範囲の中は数値だけ、直接書いた値は数値・真偽値・数値の文字列
    assert_eq!(g.eval("=COUNT(A1:A4)"), n(2.0));
    assert_eq!(g.eval("=COUNT(A1:C4)"), n(4.0));
    assert_eq!(g.eval("=COUNT(A:A,B:B)"), n(4.0));
    assert_eq!(g.eval("=COUNT(1,\"2\",\"x\",TRUE)"), n(3.0));
    assert_eq!(g.eval("=COUNT(C1)"), n(0.0));
    assert_eq!(g.eval("=COUNT(D1:D9)"), n(0.0));
    // 式の中で
    assert_eq!(g.eval("=SUM(A1:A4)/COUNT(A1:A4)"), n(3.0));
    assert_eq!(g.eval("=sum(a1,a3)"), n(6.0));
}

fn sales() -> Mem {
    let mut g = Mem::new();
    let rows = [
        ("地域", "店", "売上"),
        ("東京", "新宿店", "100"),
        ("大阪", "梅田店", "200"),
        ("tokyo", "渋谷", "300"),
        ("東京", "銀座店", "x"),
        ("名古屋", "栄店", "50"),
        ("", "空店", "7"),
    ];
    for (i, (a, b, c)) in rows.iter().enumerate() {
        let r = i + 1;
        if !a.is_empty() {
            g.set(0, &format!("A{r}"), t(a));
        }
        g.set(0, &format!("B{r}"), t(b));
        let v = c.parse::<f64>().map(n).unwrap_or_else(|_| t(c));
        g.set(0, &format!("C{r}"), v);
    }
    g
}

#[test]
fn sumifs_countifs() {
    let g = sales();
    assert_eq!(g.eval("=SUMIFS(C:C,A:A,\"東京\")"), n(100.0));
    assert_eq!(g.eval("=SUMIFS(C:C,A:A,\"Tokyo\")"), n(300.0));
    assert_eq!(g.eval("=SUMIFS(C2:C7,C2:C7,\">=100\")"), n(600.0));
    assert_eq!(g.eval("=SUMIFS(C:C,B:B,\"*店\",C:C,\"<>50\")"), n(307.0));
    assert_eq!(g.eval("=COUNTIFS(B:B,\"??店\")"), n(3.0));
    assert_eq!(g.eval("=COUNTIFS(A2:A7,\"\")"), n(1.0));
    assert_eq!(g.eval("=COUNTIFS(A2:A7,\"<>\")"), n(5.0));
    assert_eq!(g.eval("=COUNTIFS(C2:C7,100)"), n(1.0));
    assert_eq!(g.eval("=COUNTIFS(A:A,\"東京\",C:C,\">\"&99)"), n(1.0));
    assert_eq!(
        g.eval("=SUMIFS(C2:C7,A2:A6,\"東京\")"),
        Val::Err(Error::Value)
    );
    // 条件が配列ならスピル
    assert_eq!(
        g.eval("=COUNTIFS(A:A,{\"東京\",\"大阪\"})"),
        arr(1, 2, vec![n(2.0), n(1.0)])
    );
    // 文字列の数値も等しいに合う
    let mut h = Mem::new();
    h.set(0, "A1", t("100"));
    h.set(0, "A2", n(100.0));
    h.set(0, "A3", t("2026/10/1"));
    assert_eq!(h.eval("=COUNTIFS(A1:A3,\"100\")"), n(2.0));
    assert_eq!(h.eval("=COUNTIFS(A1:A3,\">50\")"), n(1.0));
}

#[test]
fn xlookup_modes() {
    let mut g = sales();
    assert_eq!(g.eval("=XLOOKUP(\"大阪\",A:A,B:B)"), t("梅田店"));
    assert_eq!(g.eval("=XLOOKUP(\"東京\",A:A,B:B,,0,-1)"), t("銀座店"));
    assert_eq!(g.eval("=XLOOKUP(\"札幌\",A:A,B:B)"), Val::Err(Error::NA));
    assert_eq!(g.eval("=XLOOKUP(\"札幌\",A:A,B:B,\"なし\")"), t("なし"));
    assert_eq!(g.eval("=XLOOKUP(\"名*\",A:A,C:C,,2)"), n(50.0));
    assert_eq!(
        g.eval("=XLOOKUP(\"大阪\",A:A,B:C)"),
        arr(1, 2, vec![t("梅田店"), n(200.0)])
    );
    assert_eq!(
        g.eval("=XLOOKUP(\"大阪\",A:A,B1:B3)"),
        Val::Err(Error::Value)
    );
    // 近似一致・二分探索
    for (i, v) in [10.0, 20.0, 30.0, 40.0].iter().enumerate() {
        g.set(0, &format!("E{}", i + 1), n(*v));
        g.set(0, &format!("F{}", i + 1), t(&format!("r{}", i + 1)));
    }
    assert_eq!(g.eval("=XLOOKUP(25,E1:E4,F1:F4,,-1)"), t("r2"));
    assert_eq!(g.eval("=XLOOKUP(25,E1:E4,F1:F4,,1)"), t("r3"));
    assert_eq!(g.eval("=XLOOKUP(25,E1:E4,F1:F4)"), Val::Err(Error::NA));
    assert_eq!(g.eval("=XLOOKUP(30,E1:E4,F1:F4,,0,2)"), t("r3"));
    assert_eq!(g.eval("=XLOOKUP(25,E1:E4,F1:F4,,-1,2)"), t("r2"));
    assert_eq!(g.eval("=XLOOKUP(25,E1:E4,F1:F4,,1,2)"), t("r3"));
    assert_eq!(g.eval("=XLOOKUP(5,E1:E4,F1:F4,,-1,2)"), Val::Err(Error::NA));
    // 横
    assert_eq!(g.eval("=XLOOKUP(\"売上\",A1:C1,A2:C2)"), n(100.0));
    // 検索値が配列
    assert_eq!(
        g.eval("=XLOOKUP({20;40},E1:E4,F1:F4)"),
        arr(2, 1, vec![t("r2"), t("r4")])
    );
}

#[test]
fn text_functions() {
    let mut g = Mem::new();
    g.set(0, "A1", t("a"));
    g.set(0, "A2", n(1.5));
    g.set(0, "B1", Val::Bool(false));
    assert_eq!(g.eval("=CONCAT(A1:B2,\"!\")"), t("aFALSE1.5!"));
    assert_eq!(g.eval("=CONCATENATE(\"x\",A2)"), t("x1.5"));
    assert_eq!(g.eval("=TEXTJOIN(\",\",TRUE,A1:B2)"), t("a,FALSE,1.5"));
    assert_eq!(g.eval("=TEXTJOIN(\",\",FALSE,A1:B2)"), t("a,FALSE,1.5,"));
    assert_eq!(g.eval("=TEXTJOIN({\"-\",\"+\"},TRUE,1,2,3)"), t("1-2+3"));
    assert_eq!(
        g.eval("=TEXTSPLIT(\"a,b,,c\",\",\")"),
        arr(1, 4, vec![t("a"), t("b"), t(""), t("c")])
    );
    assert_eq!(
        g.eval("=TEXTSPLIT(\"a,b,,c\",\",\",,TRUE)"),
        arr(1, 3, vec![t("a"), t("b"), t("c")])
    );
    assert_eq!(
        g.eval("=TEXTSPLIT(\"a=1;b=2;c\",\"=\",\";\")"),
        arr(
            3,
            2,
            vec![t("a"), t("1"), t("b"), t("2"), t("c"), Val::Err(Error::NA)]
        )
    );
    assert_eq!(
        g.eval("=TEXTSPLIT(\"1x2X3\",\"x\",,,1,\"-\")"),
        arr(1, 3, vec![t("1"), t("2"), t("3")])
    );
    assert_eq!(
        g.eval("=TEXTSPLIT(\"a b-c\",{\" \",\"-\"})"),
        arr(1, 3, vec![t("a"), t("b"), t("c")])
    );
    assert_eq!(g.eval("=TEXTSPLIT(\"abc\",\",\")"), t("abc"));
    assert_eq!(g.eval("=\"a\"&#N/A"), Val::Err(Error::NA));
}

#[test]
fn wildcards() {
    assert!(wildcard_match("東*", "東京"));
    assert!(wildcard_match("??店", "新宿店"));
    assert!(!wildcard_match("??店", "新宿駅店"));
    assert!(wildcard_match("*~**", "a*b"));
    assert!(!wildcard_match("*~**", "ab"));
    assert!(wildcard_match("A*C", "abbbc"));
}

/// 索引・まとめた計算を使う格子（中身は `Mem` と同じ）。
struct Fast<'a>(&'a Mem);

impl Grid for Fast<'_> {
    fn sheet(&self, name: &str) -> Option<usize> {
        self.0.sheet(name)
    }
    fn get(&self, sheet: usize, row: u64, col: u32) -> Val {
        self.0.get(sheet, row, col)
    }
    fn used(&self, sheet: usize) -> (u64, u32) {
        self.0.used(sheet)
    }
    fn exact_index(
        &self,
        sheet: usize,
        col: u32,
        rows: std::ops::Range<u64>,
    ) -> Option<Arc<ExactIndex>> {
        Some(Arc::new(ExactIndex::build(self, sheet, col, rows)))
    }
    fn stable(&self, _: usize, _: &Area) -> bool {
        true
    }
}

#[test]
fn indexes_and_batches_match_plain_evaluation() {
    let mut g = Mem::new();
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let cities = ["東京", "大阪", "TOKYO", "tokyo", "名古屋", "", "100"];
    for r in 1..=400 {
        let c = cities[next(cities.len() as u64) as usize];
        if !c.is_empty() {
            g.set(0, &format!("A{r}"), t(c));
        }
        g.set(0, &format!("B{r}"), n(next(5) as f64));
        if next(10) == 0 {
            g.set(0, &format!("C{r}"), t("x"));
        } else {
            g.set(0, &format!("C{r}"), n(next(1000) as f64));
        }
        g.set(0, &format!("D{r}"), n(r as f64 * 3.0));
    }
    let fast = Fast(&g);
    let cache = Cache::default();
    let mut formulas = Vec::new();
    for c in ["東京", "tokyo", "大阪", "名古屋", "", "100", "札幌"] {
        for b in 0..5 {
            formulas.push(format!("=SUMIFS(C:C,A:A,\"{c}\",B:B,{b})"));
            formulas.push(format!("=COUNTIFS(A1:A400,\"{c}\",B1:B400,\"{b}\")"));
        }
        formulas.push(format!("=XLOOKUP(\"{c}\",A:A,D:D,\"なし\")"));
        formulas.push(format!("=XLOOKUP(\"{c}\",A:A,D:D,,0,-1)"));
    }
    for k in [3.0, 30.0, 1200.0, 1201.0] {
        formulas.push(format!("=XLOOKUP({k},D:D,B:B)"));
    }
    for f in &formulas {
        let e = parse(f).unwrap();
        let plain = eval(
            &e,
            &Context {
                grid: &g,
                sheet: 0,
                sys: DateSystem::D1900,
                cache: None,
                offset: 0,
            },
        );
        // まとめた計算は 2 回目から使われるので 2 回ずつ
        for _ in 0..2 {
            let quick = eval(
                &e,
                &Context {
                    grid: &fast,
                    sheet: 0,
                    sys: DateSystem::D1900,
                    cache: Some(&cache),
                    offset: 0,
                },
            );
            assert_eq!(quick, plain, "{f}");
        }
    }
}

#[test]
fn shared_rows_match_per_row_evaluation() {
    let mut g = Mem::new();
    for r in 1..=300 {
        g.set(0, &format!("A{r}"), n(r as f64 - 150.0));
        if r % 7 != 0 {
            g.set(0, &format!("B{r}"), t(&format!("x{r}")));
        }
        g.set(0, &format!("C{r}"), n((r % 5) as f64));
    }
    g.set(0, "E1", n(10.0));
    for f in [
        "=A2*C2+$E$1",
        "=ABS(A2)&\"-\"&B2",
        "=CONCAT(B2,\":\",C2)",
        "=A2/C2",
        "=A1+A3-50%",
        "=SUMIFS(A:A,C:C,C2)",
        "=XLOOKUP(C2,C1:C5,A1:A5)",
        "=A2>0",
    ] {
        let e = parse(f).unwrap();
        let cx = Context {
            grid: &g,
            sheet: 0,
            sys: DateSystem::D1900,
            cache: None,
            offset: 0,
        };
        let mut got = Vec::new();
        eval_rows(&e, &cx, 250, &mut |i, v| {
            assert_eq!(i as usize, got.len());
            got.push(v);
        });
        for (i, v) in got.iter().enumerate() {
            // 1 行ずつ、ずらした式を評価したものと同じ
            let want = eval(&shift(&e, i as i64), &cx);
            let want = match want {
                Val::Array(a) => a.data[0].clone(),
                v => v,
            };
            assert_eq!(*v, want, "{f} row {i}");
        }
    }
    assert_eq!(
        formula_text(&shift(
            &parse("=A2+$B$1+B$2+SUMIFS(C:C,D2:D9,1)").unwrap(),
            3
        )),
        "=A5+$B$1+B$2+SUMIFS(C:C,D5:D12,1)"
    );
    assert_eq!(formula_text(&shift(&parse("=A2").unwrap(), -5)), "=#REF!");
    let Expr::Ref(r) = parse("=B2:B4").unwrap() else {
        panic!()
    };
    let s = spread(&r.area, 10);
    assert_eq!((s.r0, s.r1), (1, 12));
}

#[test]
fn shared_lookup_rows_use_the_index() {
    let mut g = Mem::new();
    for r in 1..=200 {
        g.set(0, &format!("A{r}"), n(r as f64 * 10.0));
        g.set(0, &format!("B{r}"), t(&format!("k{}", r % 37)));
        g.set(0, &format!("C{r}"), n((r % 9) as f64));
    }
    let fast = Fast(&g);
    for f in [
        "=XLOOKUP(C2,$C$1:$C$50,$A$1:$A$50)",
        "=XLOOKUP(B2,B:B,A:A,\"なし\",0,-1)",
        "=XLOOKUP(C2+100,C:C,A:A,-1)",
        "=XLOOKUP(\"K\"&C2,B:B,A:A)",
    ] {
        let e = parse(f).unwrap();
        let cx = Context {
            grid: &fast,
            sheet: 0,
            sys: DateSystem::D1900,
            cache: None,
            offset: 0,
        };
        let mut got = Vec::new();
        eval_rows(&e, &cx, 150, &mut |_, v| got.push(v));
        for (i, v) in got.iter().enumerate() {
            let want = eval(&shift(&e, i as i64), &Context { grid: &g, ..cx });
            assert_eq!(*v, want, "{f} row {i}");
        }
    }
}

#[test]
fn cobol_figurative_constants() {
    let mut g = Mem::new();
    for f in ["=LOW-VALUE()", "=low-values()", "=Low-Value( )"] {
        assert_eq!(g.eval(f), t(LOW_VALUE), "{f}");
    }
    assert_eq!(g.eval("=HIGH-VALUE()"), t(HIGH_VALUE));
    assert_eq!(g.eval("=HIGH-VALUES()"), t(HIGH_VALUE));
    assert_eq!(figurative(LOW_VALUE), Some(0x00));
    assert_eq!(figurative(HIGH_VALUE), Some(0xFF));
    assert_eq!(figurative("LOW-VALUE"), None);
    assert_eq!(g.eval("=LOW-VALUE(1)"), Val::Err(Error::Value));
    // 比べる（COBOL の IF X = LOW-VALUE）
    g.set(0, "A1", t(LOW_VALUE));
    assert_eq!(g.eval("=A1=LOW-VALUE()"), Val::Bool(true));
    assert_eq!(g.eval("=A1=HIGH-VALUE()"), Val::Bool(false));
    // 名前のあとにかっこがなければ、今までどおり引き算・名前
    g.set(0, "B1", n(5.0));
    assert_eq!(g.eval("=B1-1"), n(4.0));
    assert_eq!(g.eval("=LOW-VALUE"), Val::Err(Error::Name));
    // 式の文字列に戻す
    let e = parse("=low-values()").unwrap();
    assert_eq!(formula_text(&e), "=LOW-VALUE()");
    assert!(refs_in("=LOW-VALUE()").is_empty());
    assert!(refs_in("=HIGH-VALUE()").is_empty());
    // 入力の補助
    let w = |s: &str| typing(s, s.chars().count()).word.map(|w| w.1);
    assert_eq!(w("=LOW-V"), Some("LOW-V".into()));
    assert_eq!(w("=high-"), Some("high-".into()));
    assert_eq!(w("=A1-V"), Some("V".into()));
    assert_eq!(
        FuncInfo::complete("LOW-")
            .iter()
            .map(|f| f.name)
            .collect::<Vec<_>>(),
        ["LOW-VALUE"]
    );
    let c = typing("=LOW-VALUE(", 11).call;
    assert_eq!(c.map(|c| c.0), Some("LOW-VALUE".into()));
}
