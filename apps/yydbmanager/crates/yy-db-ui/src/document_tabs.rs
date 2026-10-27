//! SQL and ER share the document tab strip, retaining independent canvases.
use super::*;
#[derive(Default)]
struct Canvas {
    plan: Option<Plan>,
    drawing: Option<yy_draw::Document>,
    zoom: f64,
    origin: yy_draw::Point,
}
#[derive(Default)]
pub(super) struct State {
    pub er_open: bool,
    pub er_active: bool,
    other: Canvas,
}
/// Exchange only backing state, without changing selection or window placement.
/// Worker completions can update SQL results while the user keeps editing an ER.
pub(super) fn swap_canvas(a: &mut App) {
    let other = &mut a.documents.other;
    std::mem::swap(&mut a.plan, &mut other.plan);
    std::mem::swap(&mut a.drawing, &mut other.drawing);
    std::mem::swap(&mut a.zoom, &mut other.zoom);
    std::mem::swap(&mut a.origin, &mut other.origin);
    a.documents.er_active = !a.documents.er_active;
}
pub(super) fn select(a: &mut App, er: bool) {
    if er && !a.documents.er_open {
        add_sql_tab(a.sql_tabs, a.editors.len(), "ER図");
        a.documents.er_open = true;
    }
    if a.documents.er_active != er {
        swap_canvas(a);
        if er {
            er_view::properties(a);
        } else {
            plan_view::properties(a);
        }
        a.grid_painted = (0, 0);
        invalidate(a.grid_hwnd);
        invalidate(a.editor_hwnd);
        set_text(
            a.status,
            if er {
                "ER図: テーブルを右クリックして定義を編集できます。"
            } else {
                "SQL: F5で実行します。ER図の編集内容はER図タブに保持しています。"
            },
        );
    }
    unsafe {
        if let Ok(parent) = GetParent(a.grid_hwnd) {
            let _ = PostMessageW(Some(parent), WM_APP + 43, WPARAM(0), LPARAM(0));
        }
        send_message(
            a.sql_tabs,
            TCM_SETCURSEL,
            WPARAM(if er { a.editors.len() } else { a.sql_index }),
            LPARAM(0),
        );
    }
}
pub(super) fn properties_visible(a: &App) -> bool {
    (a.plan.is_some() || a.documents.er_active && a.er.is_some()) && a.drawing.is_some()
}

pub(super) fn smoke(directory: &std::path::Path) -> yy_db_core::Result<()> {
    let signature = |a: &App| {
        serde_json::to_string(&(
            a.plan.as_ref(),
            a.drawing.as_ref(),
            a.zoom.to_bits(),
            a.origin,
        ))
        .unwrap()
    };
    let sql_before = with_app(|a| {
        select(a, false);
        signature(a)
    })
    .ok_or("画面がありません")?;
    let before = with_app(|a| {
        select(a, true);
        (
            serde_json::to_string(a.er.as_ref().unwrap()).unwrap(),
            a.zoom,
            a.origin,
            a.er_view.selected,
            a.editors[a.sql_index].text(),
        )
    })
    .ok_or("画面がありません")?;
    let hwnd = with_app(|a| unsafe { GetParent(a.grid_hwnd).unwrap_or_default() }).unwrap();
    layout(hwnd);
    let valid = with_app(|a| unsafe {
        GetWindowLongPtrW(a.editor_hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0
            && GetWindowLongPtrW(a.result_tabs, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0
            && send_message(a.sql_tabs, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize
                == a.editors.len()
            && send_message(a.sql_tabs, TCM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 as usize
                == a.editors.len() + 1
    })
    .unwrap_or(false);
    if !valid {
        return Err("ER図がSQLと同じタブ列の独立した画面になっていません".into());
    }
    with_app(|a| select(a, false));
    layout(hwnd);
    if !with_app(|a| unsafe {
        GetWindowLongPtrW(a.editor_hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0
    })
    .unwrap_or(false)
    {
        return Err("SQLタブのエディタが再表示されません".into());
    }
    if with_app(|a| signature(a)).as_deref() != Some(sql_before.as_str()) {
        return Err("SQLの結果・実行計画のキャンバスが復元されません".into());
    }
    let valid = with_app(|a| {
        let text = a.editors[a.sql_index].text();
        select(a, true);
        text == before.4
            && serde_json::to_string(a.er.as_ref().unwrap()).unwrap() == before.0
            && a.zoom.to_bits() == before.1.to_bits()
            && a.origin == before.2
            && a.er_view.selected == before.3
    })
    .unwrap_or(false);
    if !valid {
        return Err("SQL/ERタブの切替で内容や表示状態が変わりました".into());
    }
    with_app(|a| select(a, false));
    layout(hwnd);
    std::fs::write(directory.join("document-tabs-smoke.txt"),"SQL/ER document tabs: shared tab strip, independent editor/canvas and results, model/zoom/origin/selection retention OK\n").map_err(|e|e.to_string())
}
