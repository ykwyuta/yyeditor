//! 行・列の挿入と削除に合わせて参照を付け替える（Excel と同じ。消えた範囲を指す参照は `#REF!`）。

use crate::Error;
use crate::parse::{Area, Expr};

/// 行・列の編集（位置・数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    InsertRows(u64, u64),
    DeleteRows(u64, u64),
    InsertCols(u32, u32),
    DeleteCols(u32, u32),
}

/// 1 次元の区間 `[lo, hi]` を挿入に合わせる（`MAX` は列全体・行全体の端なので動かさない）。
fn insert(lo: &mut u64, hi: &mut u64, at: u64, n: u64, max: u64) {
    let whole = *lo == 0 && *hi == max;
    if *lo >= at && !whole {
        *lo = lo.saturating_add(n);
    }
    if *hi >= at && *hi != max {
        *hi = hi.saturating_add(n);
    }
}

/// 削除に合わせる（すべて消えたら `false`）。
fn delete(lo: &mut u64, hi: &mut u64, at: u64, n: u64, max: u64) -> bool {
    let end = at + n;
    if *lo >= at && *hi < end {
        return false;
    }
    let shrink = |x: u64| {
        if x == max || x < at {
            x
        } else if x >= end {
            x - n
        } else {
            at
        }
    };
    let hi_inside = *hi >= at && *hi < end;
    *lo = shrink(*lo);
    *hi = if hi_inside { at - 1 } else { shrink(*hi) };
    true
}

fn area(a: &mut Area, edit: Edit) -> bool {
    let (mut r0, mut r1) = (a.r0, a.r1);
    let (mut c0, mut c1) = (a.c0 as u64, a.c1 as u64);
    let cmax = u32::MAX as u64;
    let ok = match edit {
        Edit::InsertRows(at, n) => {
            insert(&mut r0, &mut r1, at, n, u64::MAX);
            true
        }
        Edit::DeleteRows(at, n) => delete(&mut r0, &mut r1, at, n, u64::MAX),
        Edit::InsertCols(at, n) => {
            insert(&mut c0, &mut c1, at as u64, n as u64, cmax);
            true
        }
        Edit::DeleteCols(at, n) => delete(&mut c0, &mut c1, at as u64, n as u64, cmax),
    };
    if ok {
        a.r0 = r0;
        a.r1 = r1;
        a.c0 = c0.min(cmax) as u32;
        a.c1 = c1.min(cmax) as u32;
    }
    ok
}

/// 式の参照を付け替える。`hits` は参照のシート名（`None` は式のあるシート）が編集したシートか。
/// 何か変えたら `true`。
pub fn adjust(e: &mut Expr, edit: Edit, hits: &dyn Fn(Option<&str>) -> bool) -> bool {
    match e {
        Expr::Ref(r) => {
            if !hits(r.sheet.as_deref()) {
                return false;
            }
            let before = r.area;
            if area(&mut r.area, edit) {
                r.area != before
            } else {
                *e = Expr::Err(Error::Ref);
                true
            }
        }
        Expr::Neg(x) | Expr::Plus(x) | Expr::Percent(x) | Expr::Paren(x) => adjust(x, edit, hits),
        Expr::Bin(_, l, r) => {
            let a = adjust(l, edit, hits);
            adjust(r, edit, hits) || a
        }
        Expr::Call(_, args) => {
            let mut any = false;
            for a in args {
                any |= adjust(a, edit, hits);
            }
            any
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{formula_text, parse};

    fn run(src: &str, edit: Edit) -> String {
        let mut e = parse(src).unwrap();
        adjust(&mut e, edit, &|s| s.is_none());
        formula_text(&e)
    }

    #[test]
    fn shifts_references() {
        assert_eq!(run("=A5+B2", Edit::InsertRows(3, 2)), "=A7+B2");
        assert_eq!(
            run("=SUMIFS(C2:C10,A:A,1)", Edit::InsertRows(4, 1)),
            "=SUMIFS(C2:C11,A:A,1)"
        );
        assert_eq!(run("=A5", Edit::DeleteRows(4, 1)), "=#REF!");
        assert_eq!(
            run("=SUMIFS(C2:C10,A:A,1)", Edit::DeleteRows(0, 3)),
            "=SUMIFS(C1:C7,A:A,1)"
        );
        assert_eq!(run("=C1+$E$1", Edit::InsertCols(2, 1)), "=D1+$F$1");
        assert_eq!(run("=B:D", Edit::DeleteCols(1, 1)), "=B:C");
        assert_eq!(run("=Other!A5", Edit::InsertRows(0, 1)), "=Other!A5");
        assert_eq!(run("=2:4", Edit::InsertRows(0, 1)), "=3:5");
    }
}
