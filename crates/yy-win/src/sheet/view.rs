//! 絞り込み・並べ替えの操作（15 章 9）。
//!
//! 計算（絞り込みのビット列・並べ替え・値の一覧・並べ替えの確定）はバックグラウンドで行い、
//! 終わったらシートの表示（[`View`]）を置き換える（Undo できる）。計算の間に表の行数が変わったら
//! 結果は捨てる。

use std::io;
use std::sync::Arc;

use yy_sheet::query::{self, ColFilter, Cond, SortKey};
use yy_sheet::{Table, View};

use super::filter::{self, Entry, FilterChoice};
use super::*;

/// 値の一覧に出す値の種類の上限。
const VALUE_LIMIT: usize = 10_000;

/// バックグラウンドで計算する（Esc で中止。中止・失敗なら `None`）。
pub(super) fn run_bg<T: Send + 'static>(
    label: &str,
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Option<T> {
    set_status(&format!("{label}（Esc で中止）"));
    let r = crate::remote::wait(&set_status, move |w| {
        let r = f();
        if w.cancelled() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "中止しました"))
        } else {
            r
        }
    });
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            set_status("");
            if e.kind() != io::ErrorKind::Interrupted
                && let Some(f) = with(|a| a.frame)
            {
                error_box(f, &format!("できませんでした: {e}"));
            }
            None
        }
    }
}

/// 表の列の見せる名前（「A: 地域」）。
pub(super) fn col_label(t: &Table, c: u32) -> String {
    let letter = yy_sheet::col_name(c);
    if t.header {
        format!("{letter}: {}", t.columns[c as usize].name)
    } else {
        letter
    }
}

/// 今のシートの番号・表・表示・コンテキスト。
pub(super) fn current() -> Option<(usize, Table, View, Arc<SheetCtx>)> {
    with(|a| {
        a.end_edit(true);
        let s = a.sheet();
        (a.sheet, s.table.clone(), s.view.clone(), a.ctx.clone())
    })
}

/// 手で入れた・貼り付けたデータ（表の外の自由なセル）を表に取り込む（`col` 列目まで）。取り込んだら
/// `true`（Undo できる編集として扱う）。
fn absorb_free_cells(col: u32) -> bool {
    with(|a| {
        a.end_edit(true);
        let sheet = a.sheet;
        let had_table = a.sheet().table.cols() > 0;
        // 取り込むものがなければ Err で戻し、Undo の履歴に残さない
        let r = a.doc.edit(|b, ctx| {
            if b.sheets[sheet].absorb_free_cells(ctx, col)? {
                Ok(())
            } else {
                Err(io::Error::other("取り込むものなし"))
            }
        });
        if r.is_err() {
            return false;
        }
        a.after_edit();
        set_status(if had_table {
            "右の列を表に取り込みました（元に戻すで取り消せます）"
        } else {
            "入力したデータを表にしました（1 行目を見出しにしました。元に戻すで取り消せます）"
        });
        true
    })
    .unwrap_or(false)
}

/// アクティブなセルの表の列（表の外なら、入力したデータを表に取り込む。だめなら案内して `None`）。
fn active_col() -> Option<u32> {
    let (frame, col, cols) = with(|a| (a.frame, a.cur.1, a.sheet().table.cols()))?;
    if col < cols
        || (absorb_free_cells(col) && with(|a| col < a.sheet().table.cols()) == Some(true))
    {
        return Some(col);
    }
    info_box(
        frame,
        "絞り込み・並べ替えは、1 行目に見出しがある表に対して行います。\n\
         A1 から見出しの行・データの行の順に入れてから、もう一度選んでください。",
    );
    None
}

/// 表示を計算し直してシートに入れる。`record` なら編集として扱う（Undo できる・変更あり）。
fn set_view(sheet: usize, mut view: View, record: bool) {
    let Some((sh, ctx)) = with(|a| (a.doc.book.sheets.get(sheet).cloned(), a.ctx.clone())) else {
        return;
    };
    let Some(sh) = sh else {
        return;
    };
    let table = sh.table.clone();
    let rows = table.rows;
    let label = if view.is_empty() {
        "解除しています…"
    } else {
        "絞り込み・並べ替えをしています…"
    };
    let Some(view) = run_bg(label, move || {
        // 数式のセルは結果の値で
        let t2 = sh.query_table(&ctx)?;
        query::apply(&ctx, &t2, &mut view)?;
        Ok(view)
    }) else {
        return;
    };
    with(|a| {
        let Some(s) = a.doc.book.sheets.get(sheet) else {
            return;
        };
        if s.table.rows != rows || s.table.cols() != table.cols() {
            set_status("計算の間に表が変わったので、結果を捨てました（再適用してください）");
            return;
        }
        let msg = status_text(&table, &view);
        if record {
            let _ = a.doc.edit(|b, _| {
                b.sheets[sheet].view = view;
                Ok(())
            });
        } else {
            a.doc.book.sheets[sheet].view = view;
        }
        if a.sheet == sheet {
            a.top = 0;
            a.cur = (0, a.cur.1);
            a.anchor = a.cur;
        }
        a.after_edit();
        set_status(&msg);
    });
}

/// 表示の説明（ステータスバー）。
pub(super) fn status_text(t: &Table, v: &View) -> String {
    let mut parts = Vec::new();
    if !v.filters.is_empty() {
        let shown = v.rows.as_ref().map_or(t.rows, |r| r.len() as u64);
        let first = &v.filters[0];
        let more = if v.filters.len() > 1 {
            format!(" ほか {} 段階", v.filters.len() - 1)
        } else {
            String::new()
        };
        parts.push(format!(
            "絞り込み: {} {}{more} → {} ／ {} 行",
            col_label(t, first.col),
            filter::describe(&first.cond),
            crate::util::group_digits(shown),
            crate::util::group_digits(t.rows)
        ));
    }
    if !v.sort.is_empty() {
        let keys: Vec<String> = v
            .sort
            .iter()
            .map(|k| {
                format!(
                    "{}（{}）",
                    col_label(t, k.col),
                    if k.desc { "降順" } else { "昇順" }
                )
            })
            .collect();
        parts.push(format!("並べ替え: {}", keys.join("・")));
    }
    if parts.is_empty() {
        "絞り込み・並べ替えを解除しました".into()
    } else {
        parts.join("　")
    }
}

/// 列の絞り込みのダイアログを開く（`col` がなければアクティブなセルの列）。
pub(super) fn filter_column(col: Option<u32>) {
    let Some(col) = col.or_else(active_col) else {
        return;
    };
    let Some((sheet, table, view, ctx)) = current() else {
        return;
    };
    if col >= table.cols() {
        return;
    }
    // 値の一覧は、ほかの列の条件で残った行の値だけ（Excel と同じ）
    let others: Vec<ColFilter> = view
        .filters
        .iter()
        .filter(|f| f.col != col)
        .cloned()
        .collect();
    let Some(sh) = with(|a| a.doc.book.sheets.get(sheet).cloned()).flatten() else {
        return;
    };
    let Some((counts, blanks, cut)) = run_bg("値の一覧を作っています…", move || {
        let t2 = sh.query_table(&ctx)?;
        let mask = if others.is_empty() {
            None
        } else {
            Some(query::filter(&ctx, &t2, &others)?.0)
        };
        query::value_counts(&ctx, &t2.columns[col as usize], mask.as_ref(), VALUE_LIMIT)
    }) else {
        return;
    };
    set_status("");
    let Some((frame, sys)) = with(|a| (a.frame, a.sys())) else {
        return;
    };
    let fmt = table.columns[col as usize].format.clone();
    let mut entries: Vec<Entry> = counts
        .into_iter()
        .map(|vc| {
            let key = yy_sheet::query::Key::of(yy_sheet::chunk::CellRef::of(&vc.value));
            Entry {
                label: display(&vc.value, fmt.as_deref(), 60.0, sys).0,
                key,
                count: vc.count,
            }
        })
        .collect();
    if blanks > 0 {
        entries.push(Entry {
            label: "（空白）".into(),
            key: None,
            count: blanks,
        });
    }
    let current = view.filter_of(col).map(|f| &f.cond);
    let choice = filter::filter_dialog(frame, &col_label(&table, col), entries, cut, current, sys);
    let mut view = view;
    match choice {
        FilterChoice::Cancel => return,
        FilterChoice::Clear => {
            if view.filter_of(col).is_none() {
                return;
            }
            view.filters.retain(|f| f.col != col);
        }
        FilterChoice::SortAsc | FilterChoice::SortDesc => {
            view.sort = vec![SortKey {
                col,
                desc: matches!(choice, FilterChoice::SortDesc),
            }];
        }
        FilterChoice::Set(cond) => set_filter(&mut view, col, cond),
    }
    set_view(sheet, view, true);
}

/// 列の条件を入れる（すでにあれば同じ段階のまま置き換える）。
fn set_filter(view: &mut View, col: u32, cond: Cond) {
    match view.filters.iter_mut().find(|f| f.col == col) {
        Some(f) => f.cond = cond,
        None => view.filters.push(ColFilter { col, cond }),
    }
}

/// アクティブなセルの列で並べ替える（ほかのキーは外す）。
pub(super) fn sort_active(desc: bool) {
    let Some(col) = active_col() else {
        return;
    };
    let Some((sheet, _, mut view, _)) = current() else {
        return;
    };
    view.sort = vec![SortKey { col, desc }];
    set_view(sheet, view, true);
}

/// 並べ替えのダイアログ。
pub(super) fn sort_dialog() {
    if with(|a| a.sheet().table.cols()) == Some(0) {
        let col = with(|a| a.cur.1).unwrap_or(0);
        absorb_free_cells(col);
    }
    let Some((sheet, table, mut view, _)) = current() else {
        return;
    };
    let Some((frame, col)) = with(|a| (a.frame, a.cur.1)) else {
        return;
    };
    if table.cols() == 0 {
        info_box(frame, "並べ替える表がありません。");
        return;
    }
    let names = (0..table.cols()).map(|c| col_label(&table, c)).collect();
    let Some(keys) = filter::sort_dialog(frame, names, &view.sort, col) else {
        return;
    };
    if keys == view.sort {
        return;
    }
    view.sort = keys;
    set_view(sheet, view, true);
}

/// 絞り込みの段階のダイアログ。
pub(super) fn stages_dialog() {
    let Some((sheet, table, mut view, _)) = current() else {
        return;
    };
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    if view.filters.is_empty() {
        info_box(
            frame,
            "絞り込みの条件がありません。列見出しの ▼ から条件を付けてください。",
        );
        return;
    }
    let names = (0..table.cols()).map(|c| col_label(&table, c)).collect();
    let Some(filters) = filter::stages_dialog(frame, names, &view.filters, &view.counts) else {
        return;
    };
    view.filters = filters;
    set_view(sheet, view, true);
}

/// 条件とキーはそのまま、計算し直す（編集したあとに）。
pub(super) fn reapply() {
    let Some((sheet, _, view, _)) = current() else {
        return;
    };
    if view.is_empty() {
        set_status("絞り込み・並べ替えの条件がありません");
        return;
    }
    set_view(sheet, view, false);
}

/// 開いたファイルに保存されていた条件を当て直す。
pub(super) fn apply_saved() {
    let Some(views) = with(|a| {
        a.doc
            .book
            .sheets
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.view.is_empty())
            .map(|(i, s)| (i, s.view.clone()))
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    for (i, v) in views {
        set_view(i, v, false);
    }
}

/// 絞り込み・並べ替えを解除する。
pub(super) fn clear() {
    let Some((sheet, table, view, _)) = current() else {
        return;
    };
    if view.is_empty() && view.rows.is_none() {
        return;
    }
    with(|a| {
        let _ = a.doc.edit(|b, _| {
            b.sheets[sheet].view = View::default();
            Ok(())
        });
        a.top = 0;
        a.after_edit();
    });
    set_status(&status_text(&table, &View::default()));
}

/// 並べ替えを確定する（表を並べた順に作り直す。絞り込みの条件は残して当て直す）。
pub(super) fn commit_sort() {
    let Some((sheet, table, view, ctx)) = current() else {
        return;
    };
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    if view.sort.is_empty() {
        info_box(
            frame,
            "並べ替えのキーがありません。先に並べ替えてください。",
        );
        return;
    }
    let rows = table.rows;
    let t2 = table.clone();
    let Some(sh) = with(|a| a.doc.book.sheets.get(sheet).cloned()).flatten() else {
        return;
    };
    let Some((new_table, new_view)) =
        run_bg("並べ替えを確定しています…", move || {
            // 順番は数式の結果の値で決める
            let order = query::sort(&ctx, &sh.query_table(&ctx)?, &view.sort, None)?;
            let nt = query::permute(&ctx, &t2, &order)?;
            let mut nv = View {
                filters: view.filters.clone(),
                ..View::default()
            };
            query::apply(&ctx, &nt, &mut nv)?;
            Ok((nt, nv))
        })
    else {
        return;
    };
    with(|a| {
        if a.doc.book.sheets.get(sheet).map(|s| s.table.rows) != Some(rows) {
            set_status("計算の間に表が変わったので、確定をやめました");
            return;
        }
        let _ = a.doc.edit(|b, _| {
            let s = &mut b.sheets[sheet];
            s.table = new_table;
            s.formulas.touch_all();
            s.view = new_view;
            Ok(())
        });
        a.top = 0;
        a.after_edit();
    });
    set_status("並べ替えを確定しました（元に戻すは Ctrl+Z）");
}
