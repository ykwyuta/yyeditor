use super::*;
use yy_db_core::schema::{Level, Model, Names};
use yy_draw::Point;
#[path = "er_columns.rs"]
mod columns;
#[path = "er_design.rs"]
mod design;
#[path = "er_types.rs"]
mod types;
pub(super) const SELECT: u16 = 9101;
pub(super) const OPEN: u16 = 9102;
pub(super) const SAVE: u16 = 9103;
pub(super) const LOGICAL: u16 = 9104;
pub(super) const PHYSICAL_NAMES: u16 = 9105;
pub(super) const LOGICAL_NAMES: u16 = 9106;
pub(super) const BOTH_NAMES: u16 = 9107;
pub(super) const ENTITY_ONLY: u16 = 9108;
pub(super) const KEYS_ONLY: u16 = 9109;
pub(super) const ATTRIBUTES: u16 = 9110;
pub(super) const TYPES: u16 = 9111;
pub(super) const ARRANGE: u16 = 9112;
pub(super) const FIT: u16 = 9113;
pub(super) const NEW_TABLE: u16 = 9114;
pub(super) const EDIT_TABLE: u16 = 9115;
pub(super) const DROP_TABLE: u16 = 9116;
pub(super) const ADD_FIELD: u16 = 9117;
pub(super) const EDIT_FIELD: u16 = 9118;
pub(super) const DROP_FIELD: u16 = 9119;
pub(super) const ADD_KEY: u16 = 9120;
pub(super) const EDIT_KEY: u16 = 9121;
pub(super) const DROP_KEY: u16 = 9122;
pub(super) const ADD_INDEX: u16 = 9123;
pub(super) const EDIT_INDEX: u16 = 9124;
pub(super) const DROP_INDEX: u16 = 9125;
pub(super) const ADD_RELATION: u16 = 9126;
pub(super) const EDIT_RELATION: u16 = 9127;
pub(super) const DROP_RELATION: u16 = 9128;
pub(super) const DDL: u16 = 9129;
pub(super) const UNDO: u16 = 9130;
pub(super) const REDO: u16 = 9131;
pub(super) const NEW: u16 = 9132;
pub(super) const IMPORTANT: u16 = 9133;
pub(super) const MARK_IMPORTANT: u16 = 9134;
pub(super) const ADD_LOGICAL: u16 = 9135;
pub(super) const RECOMMEND: u16 = 9136;
pub(super) const MARKDOWN: u16 = 9137;
pub(super) const RESIZE: u16 = 9138;
pub(super) const AUTO_SIZE: u16 = 9139;
pub(super) const AUTO_SIZE_ALL: u16 = 9140;
pub(super) const EDIT_COLUMNS: u16 = 9141;
pub(super) fn menu() -> Result<HMENU> {
    unsafe {
        let menu = CreatePopupMenu()?;
        for (id, label) in [
            (NEW, "ERモデルを新規作成…"),
            (ER, "DBからER図を取得"),
            (NEW_TABLE, "テーブルを追加…"),
            (EDIT_COLUMNS, "列定義をまとめて編集…"),
            (RESIZE, "テーブルのサイズを指定…"),
            (AUTO_SIZE, "コンテンツにサイズを合わせる"),
            (AUTO_SIZE_ALL, "すべての表をコンテンツに合わせる"),
            (MARK_IMPORTANT, "主要項目を選択…"),
            (ADD_LOGICAL, "論理リレーションを追加…"),
            (RECOMMEND, "名前から論理リレーションを推薦…"),
            (SELECT, "表示する表を選択…"),
            (LOGICAL, "表・列の論理名を編集…"),
            (OPEN, "ERモデルを開く…"),
            (SAVE, "ERモデルを保存…"),
            (MARKDOWN, "Markdown設計書をエクスポート…"),
            (ARRANGE, "表を整列"),
            (FIT, "全体を表示"),
            (DDL, "DDLをSQLタブへ生成"),
            (UNDO, "元に戻す\tCtrl+Z"),
            (REDO, "やり直し\tCtrl+Y"),
        ] {
            AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(label))?;
        }
        let edit = CreatePopupMenu()?;
        for (id, label) in design::actions() {
            AppendMenuW(edit, MF_STRING, id as usize, &HSTRING::from(label))?;
        }
        AppendMenuW(menu, MF_POPUP, edit.0 as usize, w!("モデルを編集"))?;
        for (label, items) in [
            (
                "名前の表示",
                vec![
                    (PHYSICAL_NAMES, "物理名"),
                    (LOGICAL_NAMES, "論理名"),
                    (BOTH_NAMES, "論理名と物理名"),
                ],
            ),
            (
                "表示水準",
                vec![
                    (ENTITY_ONLY, "テーブル名のみ"),
                    (KEYS_ONLY, "主キーのみ"),
                    (ATTRIBUTES, "列"),
                    (TYPES, "列と型"),
                    (IMPORTANT, "主キー・外部キー・主要項目のみ"),
                ],
            ),
        ] {
            let submenu = CreatePopupMenu()?;
            for (id, label) in items {
                AppendMenuW(submenu, MF_STRING, id as usize, &HSTRING::from(label))?;
            }
            AppendMenuW(menu, MF_POPUP, submenu.0 as usize, &HSTRING::from(label))?;
        }
        Ok(menu)
    }
}
#[derive(Default)]
pub(super) struct View {
    pub selected: Option<usize>,
    drag: Option<Drag>,
    undo: Vec<Model>,
    redo: Vec<Model>,
    pending_move: Option<Model>,
    resize_preview: Option<(usize, yy_db_core::schema::TableSize)>,
}
enum Drag {
    Entity(usize, Point),
    Resize(usize),
    Pan(POINT),
}
pub(super) fn active(a: &App) -> bool {
    a.documents.er_active && a.er.is_some() && a.drawing.is_some()
}
pub(super) fn present(a: &mut App, m: Model) {
    document_tabs::select(a, true);
    a.plan = None;
    a.er = Some(m);
    a.er_view = View {
        selected: a
            .er
            .as_ref()
            .and_then(|m| m.entities.iter().position(|e| e.visible)),
        drag: None,
        ..Default::default()
    };
    a.zoom = a.dpi as f64 / 96.;
    a.origin = Point::new(16., 0.);
    refresh(a);
    set_text(
        a.status,
        if a.er.as_ref().is_some_and(|m| m.entities.is_empty()) {
            "空のERモデルを作成しました。図の右クリック →「テーブルを追加…」から設計を始めます。DB接続は不要です。保存は「ファイル → ERモデルを保存…」。"
        } else {
            "ER図: 表をドラッグして配置、空白をドラッグして移動、ホイールでズーム。右クリックで表示対象・論理名・表示水準・保存。* はNOT NULL。○は任意、棒は1、三つ又は多を表します。"
        },
    );
}
pub(super) fn present_document(a: &mut App, document: yy_db_core::schema::ErDocument) {
    present(a, document.model);
    if let Some(view) = document.view {
        a.zoom = (view.zoom * a.dpi as f64 / 96.).clamp(0.05, 4. * a.dpi as f64 / 96.);
        a.origin = view.origin;
        a.er_view.selected = view
            .selected
            .and_then(|id| a.er.as_ref()?.entities.iter().position(|e| e.id == id));
        refresh(a);
    }
}
fn refresh(a: &mut App) {
    if let Some(m) = a.er.as_ref() {
        a.drawing = Some(m.drawing());
    }
    properties(a);
    invalidate(a.grid_hwnd);
}
pub(super) fn properties(a: &mut App) {
    let rows =
        a.er.as_ref()
            .zip(a.er_view.selected)
            .map(|(m, i)| m.entity_details(i))
            .unwrap_or_else(|| {
                vec![(
                    "操作".into(),
                    if a.er.as_ref().is_some_and(|m| m.entities.is_empty()) {
                        "図を右クリック → テーブルを追加… で設計を始めます。DBへの反映はDDLを生成し、確認してから実行します。".into()
                    } else {
                        "表をクリックして列・キー・参照先を表示します".into()
                    },
                )]
            });
    plan_view::property_rows(a, rows);
}
fn at(a: &App, p: POINT) -> Option<usize> {
    let world = Point::new(
        a.origin.x + p.x as f64 / a.zoom,
        a.origin.y + p.y as f64 / a.zoom,
    );
    a.drawing
        .as_ref()?
        .nodes
        .iter()
        .find(|n| n.id >= 1_000_000 && n.id < 2_000_000 && n.rect.contains(world))
        .map(|n| (n.id - 1_000_000) as usize)
}
pub(super) fn command(id: u16) -> bool {
    if !(SELECT..=EDIT_COLUMNS).contains(&id) {
        return false;
    }
    if !matches!(id, NEW | OPEN) {
        with_app(|a| {
            if a.er.is_some() {
                document_tabs::select(a, true);
            }
        });
    }
    let context = with_app(|a| {
        (
            unsafe { GetParent(a.grid_hwnd).unwrap_or_default() },
            a.font,
            a.dpi,
            a.er.clone(),
            a.er_view.selected,
        )
    });
    let Some((parent, font, dpi, model, selected)) = context else {
        return true;
    };
    if !matches!(id, NEW | OPEN) {
        layout(parent);
    }
    let outcome = (|| -> yy_db_core::Result<()> {
        if id == NEW {
            let current_kind = with_app(|a| a.connection_kind).unwrap_or(DbKind::Sqlite);
            let items = DbKind::ALL
                .iter()
                .map(|kind| (kind.name().into(), *kind == current_kind))
                .collect::<Vec<_>>();
            let Some(indices) = choose(
                parent,
                "新しいERモデルの対象DB（接続不要）",
                &items,
                false,
                font,
                dpi,
            ) else {
                return Ok(());
            };
            let Some(kind) = indices.first().and_then(|i| DbKind::ALL.get(*i)).copied() else {
                return Ok(());
            };
            with_app(|a| {
                present(
                    a,
                    Model {
                        version: 2,
                        kind,
                        entities: vec![],
                        relations: vec![],
                        names: Names::Both,
                        level: Level::Types,
                    },
                )
            });
            return Ok(());
        }
        if id == OPEN {
            if let Some(path) = file_dialog(false, "yyer.yaml") {
                let document = yy_db_core::schema::ErDocument::load(&path)?;
                with_app(|a| present_document(a, document));
            }
            return Ok(());
        }
        if id == UNDO || id == REDO {
            with_app(|a| {
                let stack = if id == UNDO {
                    &mut a.er_view.undo
                } else {
                    &mut a.er_view.redo
                };
                if let Some(m) = stack.pop() {
                    if let Some(current) = a.er.replace(m) {
                        if id == UNDO {
                            a.er_view.redo.push(current)
                        } else {
                            a.er_view.undo.push(current)
                        }
                    }
                    refresh(a);
                }
            });
            return Ok(());
        }
        let mut m = model.unwrap_or_else(|| Model {
            version: 1,
            kind: with_app(|a| a.connection_kind).unwrap_or(DbKind::Sqlite),
            entities: vec![],
            relations: vec![],
            names: Names::Physical,
            level: Level::Types,
        });
        let previous = m.clone();
        if id == AUTO_SIZE_ALL {
            for e in &mut m.entities {
                e.size = None;
            }
            with_app(|a| {
                record(a, previous);
                a.er = Some(m);
                a.plan = None;
                refresh(a);
            });
            return Ok(());
        }
        if id == MARKDOWN {
            if let Some(path) = file_dialog_title(true, "md", Some("Markdown設計書の出力先"))
            {
                m.export_markdown(&path)?;
                with_app(|a| {
                    set_text(
                        a.status,
                        "Markdown設計書を出力しました。テーブル一覧のリンク、Mermaid ER図、各表の全列定義を含みます。",
                    )
                });
            }
            return Ok(());
        }
        if id == RECOMMEND {
            let mut suggestions = m.recommend_logical_relations(201);
            let truncated = suggestions.len() > 200;
            suggestions.truncate(200);
            if suggestions.is_empty() {
                return Err("推薦できる名前一致の候補がありません。必要に応じて「論理リレーションを追加…」で登録してください。".into());
            }
            let items = suggestions
                .iter()
                .map(|s| {
                    (
                        format!(
                            "{}.{} → {}.{} / {}",
                            s.relation.child.display(),
                            s.relation.columns.join(","),
                            s.relation.parent.display(),
                            s.relation.referenced_columns.join(","),
                            s.reason
                        ),
                        false,
                    )
                })
                .collect::<Vec<_>>();
            let title = if truncated {
                "論理関係の推薦（先頭200件・意味を確認して選択）"
            } else {
                "論理関係の推薦（意味を確認して選択）"
            };
            let Some(indices) = choose(parent, title, &items, true, font, dpi) else {
                return Ok(());
            };
            if indices.is_empty() {
                return Ok(());
            }
            let count = indices.len();
            for i in indices {
                m.relations.push(suggestions[i].relation.clone());
            }
            m.validate()?;
            with_app(|a| {
                record(a, previous);
                a.er = Some(m);
                a.plan = None;
                refresh(a);
                set_text(
                    a.status,
                    &format!(
                        "論理リレーションを{count}件登録しました。紫の破線で表示し、DDLには外部キー制約として出力しません。"
                    ),
                );
            });
            return Ok(());
        }
        if (NEW_TABLE..=DDL).contains(&id)
            || matches!(
                id,
                MARK_IMPORTANT | ADD_LOGICAL | RESIZE | AUTO_SIZE | EDIT_COLUMNS
            )
        {
            if id == DDL {
                let ddl = m.ddl()?;
                with_app(|a| -> yy_db_core::Result<()> {
                    document_tabs::select(a, false);
                    a.editors
                        .push(Editor::new(&ddl.sql, a.dpi).map_err(|e| e.to_string())?);
                    a.sql_index = a.editors.len() - 1;
                    add_sql_tab(a.sql_tabs, a.sql_index, &format!("DDL ({})", m.kind.name()));
                    invalidate(a.editor_hwnd);
                    set_text(
                        a.status,
                        "DDLを新しいSQLタブへ生成しました。SQLを確認してから実行してください。",
                    );
                    Ok(())
                })
                .unwrap_or_else(|| Err("画面状態がありません".into()))?;
                return Ok(());
            }
            let Some(index) = design::edit(parent, font, dpi, &mut m, selected, id)? else {
                return Ok(());
            };
            m.validate()?;
            with_app(|a| {
                record(a, previous);
                a.er = Some(m);
                a.plan = None;
                a.er_view.selected = index;
                refresh(a);
            });
            return Ok(());
        }
        match id {
            SAVE => {
                if let Some(path) = file_dialog(true, "yyer.yaml") {
                    let view = with_app(|a| yy_db_core::schema::ErViewState {
                        zoom: a.zoom * 96. / a.dpi as f64,
                        origin: a.origin,
                        selected: a
                            .er_view
                            .selected
                            .and_then(|i| m.entities.get(i))
                            .map(|e| e.id.clone()),
                    });
                    yy_db_core::schema::ErDocument::new(m, view).save(&path)?;
                    with_app(|a| {
                        set_text(
                            a.status,
                            "ERモデルをYAMLで保存しました。定義・主要項目・論理関係・配置・表示状態を保持しています。",
                        )
                    });
                }
                return Ok(());
            }
            SELECT => {
                let items = m
                    .entities
                    .iter()
                    .map(|e| (e.id.display(), e.visible))
                    .collect::<Vec<_>>();
                let Some(indices) = choose(
                    parent,
                    "ER図に表示するテーブルとビュー",
                    &items,
                    true,
                    font,
                    dpi,
                ) else {
                    return Ok(());
                };
                for (i, e) in m.entities.iter_mut().enumerate() {
                    e.visible = indices.contains(&i);
                }
                m.arrange();
            }
            LOGICAL => {
                let i = selected.ok_or("論理名を編集する表をクリックしてください")?;
                let e = m.entities.get_mut(i).ok_or("選択した表がありません")?;
                let mut items = vec![(format!("テーブル: {}", e.id.display()), true)];
                items.extend(
                    e.fields
                        .iter()
                        .map(|f| (format!("列: {}  {}", f.name, f.logical_name), false)),
                );
                let Some(indices) =
                    choose(parent, "論理名を編集する対象", &items, false, font, dpi)
                else {
                    return Ok(());
                };
                let Some(index) = indices.first() else {
                    return Ok(());
                };
                let value = if *index == 0 {
                    &mut e.logical_name
                } else {
                    &mut e.fields[*index - 1].logical_name
                };
                let Some(name) = input_text(parent, "論理名を編集", value, font, dpi) else {
                    return Ok(());
                };
                *value = name;
                m.names = Names::Both;
            }
            PHYSICAL_NAMES => m.names = Names::Physical,
            LOGICAL_NAMES => m.names = Names::Logical,
            BOTH_NAMES => m.names = Names::Both,
            ENTITY_ONLY => m.level = Level::Entity,
            KEYS_ONLY => m.level = Level::Keys,
            ATTRIBUTES => m.level = Level::Attributes,
            TYPES => m.level = Level::Types,
            IMPORTANT => m.level = Level::Important,
            ARRANGE => m.arrange(),
            FIT => {
                with_app(fit);
                return Ok(());
            }
            _ => {}
        }
        m.validate()?;
        with_app(|a| {
            record(a, previous);
            a.er = Some(m);
            a.er_view.selected = a.er_view.selected.filter(|i| {
                a.er.as_ref()
                    .unwrap()
                    .entities
                    .get(*i)
                    .is_some_and(|e| e.visible)
            });
            refresh(a);
        });
        Ok(())
    })();
    if let Err(e) = outcome {
        message(&e);
    }
    invalidate(parent);
    true
}
fn record(a: &mut App, m: Model) {
    a.er_view.undo.push(m);
    if a.er_view.undo.len() > 32 {
        a.er_view.undo.remove(0);
    }
    a.er_view.redo.clear();
}
pub(super) fn overlay(a: &App, dc: HDC) {
    let Some(index) = a.er_view.selected else {
        return;
    };
    let Some(n) = a
        .drawing
        .as_ref()
        .and_then(|d| d.nodes.iter().find(|n| n.id == index as u64 + 1_000_000))
    else {
        return;
    };
    let r = RECT {
        left: ((n.rect.x - a.origin.x) * a.zoom) as i32,
        top: ((n.rect.y - a.origin.y) * a.zoom) as i32,
        right: ((n.rect.x + n.rect.w - a.origin.x) * a.zoom) as i32,
        bottom: ((n.rect.y + n.rect.h - 24. - a.origin.y) * a.zoom) as i32,
    };
    let r = if let Some((_, size)) = a.er_view.resize_preview.filter(|(i, _)| *i == index) {
        RECT {
            right: r.left + (size.width * a.zoom) as i32,
            bottom: r.top + (size.height * a.zoom) as i32,
            ..r
        }
    } else {
        r
    };
    unsafe {
        let brush = CreateSolidBrush(COLORREF(0xc86f24));
        FrameRect(dc, &r, brush);
        let half = (4 * a.dpi / 96).max(4) as i32;
        FillRect(
            dc,
            &RECT {
                left: r.right - half,
                top: r.bottom - half,
                right: r.right + half,
                bottom: r.bottom + half,
            },
            brush,
        );
        let _ = DeleteObject(brush.into());
    }
}
fn resize_at(a: &App, p: POINT) -> Option<usize> {
    let index = a.er_view.selected?;
    let n = a
        .drawing
        .as_ref()?
        .nodes
        .iter()
        .find(|n| n.id == index as u64 + 1_000_000)?;
    let x = (n.rect.x + n.rect.w - a.origin.x) * a.zoom;
    let y = (n.rect.y + n.rect.h - 24. - a.origin.y) * a.zoom;
    let tolerance = (8 * a.dpi / 96).max(6) as f64;
    ((p.x as f64 - x).abs() <= tolerance && (p.y as f64 - y).abs() <= tolerance).then_some(index)
}
fn fit(a: &mut App) {
    let Some(doc) = &a.drawing else { return };
    let b = doc.bounds();
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(a.grid_hwnd, &mut r);
    }
    a.zoom = ((r.right - 32).max(1) as f64 / b.w)
        .min((r.bottom - 32).max(1) as f64 / b.h)
        .clamp(0.05, a.dpi as f64 / 96.);
    a.origin = Point::new(b.x - 16. / a.zoom, b.y - 16. / a.zoom);
    invalidate(a.grid_hwnd);
}
pub(super) fn handle(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    if !with_app(|a| active(a)).unwrap_or(false) {
        return None;
    }
    let p = POINT {
        x: l.0 as i16 as i32,
        y: (l.0 >> 16) as i16 as i32,
    };
    match msg {
        WM_SETCURSOR => {
            let mut cursor = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut cursor);
                let _ = ScreenToClient(hwnd, &mut cursor);
            }
            if with_app(|a| {
                resize_at(a, cursor).is_some() || matches!(a.er_view.drag, Some(Drag::Resize(_)))
            })
            .unwrap_or(false)
            {
                unsafe {
                    SetCursor(LoadCursorW(None, IDC_SIZENWSE).ok());
                }
                return Some(LRESULT(1));
            }
            None
        }
        WM_KEYDOWN
            if w.0 == VK_ESCAPE.0 as usize
                && with_app(|a| matches!(a.er_view.drag, Some(Drag::Resize(_))))
                    .unwrap_or(false) =>
        {
            with_app(|a| {
                a.er_view.drag = None;
                a.er_view.resize_preview = None;
                a.er_view.pending_move = None;
            });
            unsafe {
                let _ = ReleaseCapture();
            }
            invalidate(hwnd);
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_MBUTTONDOWN => {
            with_app(|a| {
                a.er_view.drag = if msg == WM_LBUTTONDOWN {
                    if let Some(index) = resize_at(a, p) {
                        Some(Drag::Resize(index))
                    } else {
                        at(a, p).map(|i| {
                            a.er_view.selected = Some(i);
                            let e = &a.er.as_ref().unwrap().entities[i];
                            Drag::Entity(
                                i,
                                Point::new(
                                    a.origin.x + p.x as f64 / a.zoom - e.position.x,
                                    a.origin.y + p.y as f64 / a.zoom - e.position.y,
                                ),
                            )
                        })
                    }
                } else {
                    None
                };
                if a.er_view.drag.is_none() {
                    a.er_view.drag = Some(Drag::Pan(p));
                }
                properties(a);
                if matches!(a.er_view.drag, Some(Drag::Entity(..) | Drag::Resize(..))) {
                    a.er_view.pending_move = a.er.clone();
                }
                invalidate(hwnd);
            });
            unsafe {
                SetCapture(hwnd);
                let _ = SetFocus(Some(hwnd));
            }
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            with_app(|a| match a.er_view.drag.as_mut() {
                Some(Drag::Entity(i, offset)) => {
                    if let Some(m) = a.er.as_mut() {
                        let position = Point::new(
                            (a.origin.x + p.x as f64 / a.zoom - offset.x).clamp(-1e7, 1e7),
                            (a.origin.y + p.y as f64 / a.zoom - offset.y).clamp(-1e7, 1e7),
                        );
                        if let Some(doc) = a.drawing.as_mut() {
                            m.move_entity(*i, position, doc);
                        }
                    }
                    invalidate(hwnd);
                }
                Some(Drag::Resize(i)) => {
                    if let Some(e) = a.er.as_ref().and_then(|m| m.entities.get(*i)) {
                        a.er_view.resize_preview = Some((
                            *i,
                            yy_db_core::schema::TableSize {
                                width: (a.origin.x + p.x as f64 / a.zoom - e.position.x)
                                    .clamp(160., 10000.),
                                height: (a.origin.y + p.y as f64 / a.zoom - e.position.y)
                                    .clamp(48., 120000.),
                            },
                        ));
                        invalidate(hwnd);
                    }
                }
                Some(Drag::Pan(old)) => {
                    a.origin.x -= (p.x - old.x) as f64 / a.zoom;
                    a.origin.y -= (p.y - old.y) as f64 / a.zoom;
                    *old = p;
                    invalidate(hwnd);
                }
                None => {}
            });
            Some(LRESULT(0))
        }
        WM_LBUTTONUP | WM_MBUTTONUP | WM_CAPTURECHANGED => {
            with_app(|a| {
                a.er_view.drag = None;
                if let Some((index, size)) = a.er_view.resize_preview.take() {
                    if let Some(e) = a.er.as_mut().and_then(|m| m.entities.get_mut(index)) {
                        e.size = Some(size);
                    }
                    refresh(a);
                }
                if let Some(old) = a.er_view.pending_move.take()
                    && a.er.as_ref().is_some_and(|m| {
                        old.entities
                            .iter()
                            .zip(&m.entities)
                            .any(|(a, b)| a.position != b.position || a.size != b.size)
                    })
                {
                    record(a, old);
                }
            });
            if msg != WM_CAPTURECHANGED {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONDBLCLK => {
            command(LOGICAL);
            Some(LRESULT(0))
        }
        WM_CONTEXTMENU => {
            let mut cp = p;
            if l.0 == -1 {
                unsafe {
                    let _ = GetCursorPos(&mut cp);
                }
            } else {
                let mut client = cp;
                unsafe {
                    let _ = ScreenToClient(hwnd, &mut client);
                }
                with_app(|a| {
                    if let Some(i) = at(a, client) {
                        a.er_view.selected = Some(i);
                        properties(a);
                    }
                });
            }
            unsafe {
                if let Ok(menu) = CreatePopupMenu() {
                    for (id, label) in [
                        (NEW_TABLE, "テーブルを追加…"),
                        (EDIT_COLUMNS, "列定義をまとめて編集…"),
                        (RESIZE, "テーブルのサイズを指定…"),
                        (AUTO_SIZE, "コンテンツにサイズを合わせる"),
                        (AUTO_SIZE_ALL, "すべての表をコンテンツに合わせる"),
                        (MARK_IMPORTANT, "主要項目を選択…"),
                        (ADD_LOGICAL, "論理リレーションを追加…"),
                        (RECOMMEND, "名前から論理リレーションを推薦…"),
                        (SELECT, "表示する表を選択…"),
                        (LOGICAL, "表・列の論理名を編集…"),
                        (FIT, "全体を表示"),
                        (ARRANGE, "表を整列"),
                        (PHYSICAL_NAMES, "物理名"),
                        (LOGICAL_NAMES, "論理名"),
                        (BOTH_NAMES, "論理名と物理名"),
                        (ENTITY_ONLY, "テーブル名のみ"),
                        (KEYS_ONLY, "主キーのみ"),
                        (ATTRIBUTES, "列を表示"),
                        (TYPES, "列と型を表示"),
                        (IMPORTANT, "主キー・外部キー・主要項目のみ"),
                        (SAVE, "ERモデルを保存…"),
                        (MARKDOWN, "Markdown設計書をエクスポート…"),
                        (DDL, "DDLをSQLタブへ生成"),
                    ] {
                        let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(label));
                    }
                    if let Ok(edit) = CreatePopupMenu() {
                        for (id, label) in design::actions() {
                            let _ =
                                AppendMenuW(edit, MF_STRING, id as usize, &HSTRING::from(label));
                        }
                        let _ = AppendMenuW(menu, MF_POPUP, edit.0 as usize, w!("モデルを編集"));
                    }
                    let id = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        cp.x,
                        cp.y,
                        Some(0),
                        hwnd,
                        None,
                    )
                    .0 as u16;
                    let _ = DestroyMenu(menu);
                    command(id);
                }
            }
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            with_app(|a| {
                let mut cp = p;
                unsafe {
                    let _ = ScreenToClient(hwnd, &mut cp);
                }
                let old = a.zoom;
                let delta = ((w.0 >> 16) as i16 as f64) / 120.;
                a.zoom = (old * 1.12_f64.powf(delta)).clamp(0.05, 4. * a.dpi as f64 / 96.);
                a.origin.x += cp.x as f64 / old - cp.x as f64 / a.zoom;
                a.origin.y += cp.y as f64 / old - cp.y as f64 / a.zoom;
                invalidate(hwnd);
            });
            Some(LRESULT(0))
        }
        WM_KEYDOWN => {
            if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 && matches!(w.0, 0x5A | 0x59) {
                command(if w.0 == 0x5A { UNDO } else { REDO });
                return Some(LRESULT(0));
            }
            if w.0 == VK_F2.0 as usize {
                command(LOGICAL);
                return Some(LRESULT(0));
            }
            if w.0 == VK_HOME.0 as usize {
                command(FIT);
                return Some(LRESULT(0));
            }
            with_app(|a| {
                let d = 48. / a.zoom;
                match w.0 as u16 {
                    x if x == VK_LEFT.0 => a.origin.x -= d,
                    x if x == VK_RIGHT.0 => a.origin.x += d,
                    x if x == VK_UP.0 => a.origin.y -= d,
                    x if x == VK_DOWN.0 => a.origin.y += d,
                    _ => {}
                }
                invalidate(hwnd);
            });
            Some(LRESULT(0))
        }
        _ => None,
    }
}

struct Choice {
    done: std::cell::Cell<bool>,
    accepted: std::cell::Cell<bool>,
    list: HWND,
    multi: bool,
}
unsafe extern "system" fn choose_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(h, GWLP_USERDATA) as *const Choice;
        if !ptr.is_null() {
            let s = &*ptr;
            if msg == WM_CLOSE {
                s.done.set(true);
                let _ = PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            if msg == WM_COMMAND {
                match w.0 & 0xffff {
                    1 | 2 => {
                        s.accepted.set(w.0 & 0xffff == 1);
                        s.done.set(true);
                        let _ = PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0));
                        return LRESULT(0);
                    }
                    3 | 4 if s.multi => {
                        send_message(
                            s.list,
                            LB_SETSEL,
                            WPARAM(usize::from(w.0 & 0xffff == 3)),
                            LPARAM(-1),
                        );
                        return LRESULT(0);
                    }
                    _ => {}
                }
            }
        }
        DefWindowProcW(h, msg, w, l)
    }
}
fn choose(
    parent: HWND,
    title: &str,
    items: &[(String, bool)],
    multi: bool,
    font: HFONT,
    dpi: u32,
) -> Option<Vec<usize>> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        RegisterClassW(&WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("YYDbERChoice"),
            lpfnWndProc: Some(choose_proc),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            ..Default::default()
        });
        let px = |v: i32| v * dpi as i32 / 96;
        let mut r = RECT::default();
        let _ = GetWindowRect(parent, &mut r);
        let h = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            w!("YYDbERChoice"),
            &HSTRING::from(title),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            r.left + px(40),
            r.top + px(40),
            px(650),
            px(500),
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let list = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("LISTBOX"),
            None,
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | WINDOW_STYLE(
                    LBS_NOINTEGRALHEIGHT as u32 | if multi { LBS_EXTENDEDSEL as u32 } else { 0 },
                ),
            px(12),
            px(12),
            px(610),
            px(380),
            Some(h),
            Some(HMENU(10 as *mut _)),
            Some(instance.into()),
            None,
        )
        .ok()?;
        send_message(list, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
        send_message(
            list,
            LB_SETHORIZONTALEXTENT,
            WPARAM(px(1400) as usize),
            LPARAM(0),
        );
        for (i, (label, selected)) in items.iter().enumerate() {
            send_message(
                list,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(label).as_ptr() as isize),
            );
            if *selected {
                send_message(
                    list,
                    if multi { LB_SETSEL } else { LB_SETCURSEL },
                    WPARAM(if multi { 1 } else { i }),
                    LPARAM(if multi { i as isize } else { 0 }),
                );
            }
        }
        let state = Choice {
            done: std::cell::Cell::new(false),
            accepted: std::cell::Cell::new(false),
            list,
            multi,
        };
        SetWindowLongPtrW(h, GWLP_USERDATA, &state as *const _ as isize);
        for (id, label, x) in [
            (1, "適用", 410),
            (2, "キャンセル", 515),
            (3, "すべて選択", 12),
            (4, "選択解除", 125),
        ] {
            if !multi && id >= 3 {
                continue;
            }
            let b = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                &HSTRING::from(label),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WINDOW_STYLE(if id == 1 { BS_DEFPUSHBUTTON as u32 } else { 0 }),
                px(x),
                px(410),
                px(100),
                px(30),
                Some(h),
                Some(HMENU(id as *mut _)),
                Some(instance.into()),
                None,
            )
            .ok()?;
            send_message(b, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
        }
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetFocus(Some(list));
        let mut msg = MSG::default();
        while !state.done.get() {
            let code = GetMessageW(&mut msg, None, 0, 0).0;
            if code <= 0 {
                if code == 0 {
                    PostQuitMessage(msg.wParam.0 as i32);
                }
                break;
            }
            if !IsDialogMessageW(h, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let result = state.accepted.get().then(|| {
            if multi {
                (0..items.len())
                    .filter(|i| send_message(list, LB_GETSEL, WPARAM(*i), LPARAM(0)).0 > 0)
                    .collect()
            } else {
                let i = send_message(list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                if i >= 0 { vec![i as usize] } else { vec![] }
            }
        });
        let _ = EnableWindow(parent, true);
        let _ = DestroyWindow(h);
        let _ = SetForegroundWindow(parent);
        result
    }
}
pub(super) fn smoke(directory: &std::path::Path) -> yy_db_core::Result<()> {
    let Some(grid) = with_app(|a| active(a).then_some(a.grid_hwnd)).flatten() else {
        return Ok(());
    };
    with_app(fit);
    let (point, before) = with_app(|a| {
        let m = a.er.as_ref().unwrap();
        let pos = m.entities[0].position;
        (
            POINT {
                x: ((pos.x + 100. - a.origin.x) * a.zoom) as i32,
                y: ((pos.y + 15. - a.origin.y) * a.zoom) as i32,
            },
            pos,
        )
    })
    .unwrap();
    let pack = |p: POINT| LPARAM((p.x as u16 as u32 | ((p.y as u16 as u32) << 16)) as isize);
    handle(grid, WM_LBUTTONDOWN, WPARAM(0), pack(point));
    handle(
        grid,
        WM_MOUSEMOVE,
        WPARAM(1),
        pack(POINT {
            x: point.x + 50,
            y: point.y + 20,
        }),
    );
    handle(grid, WM_LBUTTONUP, WPARAM(0), LPARAM(0));
    let moved = with_app(|a| {
        a.er.as_ref().unwrap().entities[0].position != before && a.er_view.selected == Some(0)
    })
    .unwrap_or(false);
    if !moved {
        return Err("ERの表の選択・ドラッグ移動が反映されません".into());
    }
    let (corner, old_size) = with_app(|a| {
        let n = a
            .drawing
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.id == 1_000_000)
            .unwrap();
        (
            POINT {
                x: ((n.rect.x + n.rect.w - a.origin.x) * a.zoom) as i32,
                y: ((n.rect.y + n.rect.h - 24. - a.origin.y) * a.zoom) as i32,
            },
            a.er.as_ref().unwrap().entities[0].size,
        )
    })
    .unwrap();
    handle(grid, WM_LBUTTONDOWN, WPARAM(0), pack(corner));
    handle(
        grid,
        WM_MOUSEMOVE,
        WPARAM(1),
        pack(POINT {
            x: corner.x + 60,
            y: corner.y + 30,
        }),
    );
    let preview = with_app(|a| {
        a.er_view.resize_preview.is_some() && a.er.as_ref().unwrap().entities[0].size == old_size
    })
    .unwrap_or(false);
    if !preview {
        return Err("ERのサイズ変更プレビューが不正です".into());
    }
    handle(grid, WM_LBUTTONUP, WPARAM(0), LPARAM(0));
    if !with_app(|a| a.er.as_ref().unwrap().entities[0].size.is_some()).unwrap_or(false) {
        return Err("ERのドラッグによるサイズ変更が反映されません".into());
    }
    command(UNDO);
    if with_app(|a| a.er.as_ref().unwrap().entities[0].size) != Some(old_size) {
        return Err("ERのサイズ変更を元に戻せません".into());
    }
    command(REDO);
    command(AUTO_SIZE);
    if with_app(|a| a.er.as_ref().unwrap().entities[0].size) != Some(None) {
        return Err("ERをコンテンツのサイズに戻せません".into());
    }
    with_app(|a| -> yy_db_core::Result<()> {
        let m = a.er.as_ref().unwrap();
        let fresh = m.drawing();
        let current = a.drawing.as_ref().unwrap();
        if current.connectors != fresh.connectors
            || current.nodes != fresh.nodes
            || current.assets != fresh.assets
        {
            return Err("ERのドラッグ後の線・端点・表の配置が一致しません".into());
        }
        let path = directory.join("edited-er.yyer.yaml");
        let document = yy_db_core::schema::ErDocument::new(
            m.clone(),
            Some(yy_db_core::schema::ErViewState {
                zoom: a.zoom * 96. / a.dpi as f64,
                origin: a.origin,
                selected: a.er_view.selected.map(|i| m.entities[i].id.clone()),
            }),
        );
        document.save(&path)?;
        let reloaded = yy_db_core::schema::ErDocument::load(&path)?;
        if serde_json::to_value(&reloaded).map_err(|e| e.to_string())?
            != serde_json::to_value(&document).map_err(|e| e.to_string())?
        {
            return Err("ERの配置を再読込できません".into());
        }
        present_document(a, reloaded);
        Ok(())
    })
    .unwrap_or_else(|| Err("ERの画面状態がありません".into()))?;
    for mode in [
        LOGICAL_NAMES,
        BOTH_NAMES,
        ENTITY_ONLY,
        KEYS_ONLY,
        ATTRIBUTES,
        TYPES,
        ARRANGE,
        FIT,
    ] {
        command(mode);
    }
    with_app(|a|->yy_db_core::Result<()> {
        let m=a.er.as_mut().unwrap();m.entities[0].logical_name="社員情報".into();m.names=Names::Both;refresh(a);
        let doc=a.drawing.as_ref().unwrap();
        std::fs::write(directory.join("er-edited.png"),a.renderer.png(doc,&yy_db_core::plan::drawing_export(doc),1.)?).map_err(|e|e.to_string())?;
        std::fs::write(directory.join("er-view-smoke.txt"),"ER: select, drag with attached relations, resize/undo/auto-size, properties, YAML roundtrip with view, name modes, display levels, arrange and fit OK\n").map_err(|e|e.to_string())
    }).unwrap_or_else(||Err("ERの画面状態がありません".into()))?;
    let before = with_app(|a| {
        (
            a.editors.len(),
            a.busy,
            a.results.len(),
            serde_json::to_string(a.er.as_ref().unwrap()).unwrap(),
        )
    })
    .unwrap();
    command(UNDO);
    command(REDO);
    command(DDL);
    let correct = with_app(|a| {
        a.editors.len() == before.0 + 1
            && a.busy == before.1
            && a.results.len() == before.2
            && serde_json::to_string(a.er.as_ref().unwrap()).unwrap() == before.3
            && a.editors[a.sql_index].text().contains("CREATE TABLE")
    })
    .unwrap_or(false);
    if !correct {
        return Err("ERのUndo/RedoまたはDDLのSQLタブ生成が不正です".into());
    }
    Ok(())
}
