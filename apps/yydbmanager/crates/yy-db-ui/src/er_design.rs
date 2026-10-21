use super::*;
use yy_db_core::schema::{Entity, Field, Index, Key, Relation, TableId};
pub(super) fn actions() -> Vec<(u16, &'static str)> {
    vec![
        (NEW_TABLE, "テーブルを追加…"),
        (EDIT_TABLE, "テーブルを編集…"),
        (DROP_TABLE, "テーブルを削除"),
        (RESIZE, "テーブルのサイズを指定…"),
        (AUTO_SIZE, "コンテンツにサイズを合わせる"),
        (EDIT_COLUMNS, "列定義をまとめて編集…"),
        (ADD_FIELD, "列を追加…"),
        (EDIT_FIELD, "列を編集…"),
        (DROP_FIELD, "列を削除…"),
        (MARK_IMPORTANT, "主要項目を選択…"),
        (ADD_KEY, "キーを追加…"),
        (EDIT_KEY, "キーを編集…"),
        (DROP_KEY, "キーを削除…"),
        (ADD_INDEX, "索引を追加…"),
        (EDIT_INDEX, "索引を編集…"),
        (DROP_INDEX, "索引を削除…"),
        (ADD_RELATION, "リレーションを追加…"),
        (ADD_LOGICAL, "論理リレーションを追加…"),
        (EDIT_RELATION, "リレーションを編集…"),
        (DROP_RELATION, "リレーションを削除…"),
    ]
}
fn pick(
    parent: HWND,
    title: &str,
    names: Vec<String>,
    selected: Option<usize>,
    font: HFONT,
    dpi: u32,
) -> Option<usize> {
    choose(
        parent,
        title,
        &names
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, Some(i) == selected.or(Some(0))))
            .collect::<Vec<_>>(),
        false,
        font,
        dpi,
    )?
    .first()
    .copied()
}
fn columns(
    parent: HWND,
    title: &str,
    e: &Entity,
    selected: &[String],
    font: HFONT,
    dpi: u32,
) -> Option<Vec<String>> {
    let items = e
        .fields
        .iter()
        .map(|f| (f.name.clone(), selected.contains(&f.name)))
        .collect::<Vec<_>>();
    let mut remaining = choose(parent, title, &items, true, font, dpi)?
        .into_iter()
        .map(|i| e.fields[i].name.clone())
        .collect::<Vec<_>>();
    let mut ordered = vec![];
    while remaining.len() > 1 {
        let default = selected
            .get(ordered.len())
            .and_then(|c| remaining.iter().position(|n| n == c));
        let i = pick(
            parent,
            &format!("{}: 第{}列を選択", title, ordered.len() + 1),
            remaining.clone(),
            default,
            font,
            dpi,
        )?;
        ordered.push(remaining.remove(i));
    }
    ordered.extend(remaining);
    Some(ordered)
}
pub(super) fn edit(
    parent: HWND,
    font: HFONT,
    dpi: u32,
    m: &mut Model,
    selected: Option<usize>,
    id: u16,
) -> yy_db_core::Result<Option<Option<usize>>> {
    if id == NEW_TABLE {
        let schema = if matches!(m.kind, DbKind::Sqlite | DbKind::DuckDb) {
            "main"
        } else if m.kind == DbKind::SqlServer {
            "dbo"
        } else {
            ""
        };
        let Some(v) = form(
            parent,
            "テーブルを追加",
            &[
                ("スキーマ", schema.into(), false),
                ("物理名", format!("table_{}", m.entities.len() + 1), false),
                ("論理名", String::new(), false),
            ],
            font,
            dpi,
        ) else {
            return Ok(None);
        };
        let index = m.entities.len();
        m.entities.push(Entity {
            size: None,
            id: TableId {
                schema: v[0].clone(),
                name: v[1].clone(),
            },
            logical_name: v[2].clone(),
            view: false,
            fields: vec![Field {
                important: false,
                name: "id".into(),
                logical_name: String::new(),
                data_type: "INTEGER".into(),
                nullable: false,
                default_sql: None,
            }],
            keys: vec![Key {
                name: format!("PK_{}", v[1]),
                primary: true,
                columns: vec!["id".into()],
            }],
            position: Point::new(
                40. + (index % 3) as f64 * 620.,
                32. + (index / 3) as f64 * 250.,
            ),
            visible: true,
            indexes: vec![],
            catalog_definition: false,
        });
        m.validate()?;
        return Ok(Some(Some(index)));
    }
    let i = selected.ok_or("編集するテーブルを選択してください")?;
    let mut e = m.entities.get(i).ok_or("テーブルがありません")?.clone();
    if id == EDIT_COLUMNS {
        return Ok(super::columns::edit(parent, font, dpi, m, i)?.then_some(Some(i)));
    }
    match id {
        RESIZE => {
            let size = m.table_size(i).ok_or("テーブルがありません")?;
            let Some(v) = form(
                parent,
                "テーブルのサイズを指定",
                &[
                    ("幅（160～10000）", size.width.to_string(), false),
                    ("高さ（48～120000）", size.height.to_string(), false),
                ],
                font,
                dpi,
            ) else {
                return Ok(None);
            };
            e.size = Some(yy_db_core::schema::TableSize {
                width: v[0]
                    .parse::<f64>()
                    .map_err(|_| "幅は数値で指定してください")?,
                height: v[1]
                    .parse::<f64>()
                    .map_err(|_| "高さは数値で指定してください")?,
            });
            m.replace_entity(i, e)?;
        }
        AUTO_SIZE => {
            e.size = None;
            m.replace_entity(i, e)?;
        }
        MARK_IMPORTANT => {
            let items = e
                .fields
                .iter()
                .map(|f| (format!("{} {}", f.name, f.logical_name), f.important))
                .collect::<Vec<_>>();
            let Some(indices) = choose(
                parent,
                "主要項目を選択（主キー・外部キーは常に表示）",
                &items,
                true,
                font,
                dpi,
            ) else {
                return Ok(None);
            };
            for (index, f) in e.fields.iter_mut().enumerate() {
                f.important = indices.contains(&index);
            }
            m.replace_entity(i, e)?;
        }
        EDIT_TABLE => {
            let Some(v) = form(
                parent,
                "テーブルを編集",
                &[
                    ("スキーマ", e.id.schema.clone(), false),
                    ("物理名", e.id.name.clone(), false),
                    ("論理名", e.logical_name.clone(), false),
                ],
                font,
                dpi,
            ) else {
                return Ok(None);
            };
            e.id = TableId {
                schema: v[0].clone(),
                name: v[1].clone(),
            };
            e.logical_name = v[2].clone();
            m.replace_entity(i, e)?;
        }
        DROP_TABLE => {
            if m.relations
                .iter()
                .any(|r| r.child == e.id || r.parent == e.id)
            {
                return Err(
                    "リレーションで使用中の表です。先にリレーションを削除してください".into(),
                );
            }
            m.entities.remove(i);
            return Ok(Some(None));
        }
        ADD_FIELD | EDIT_FIELD | DROP_FIELD => {
            let index = if id == ADD_FIELD {
                None
            } else {
                let Some(index) = pick(
                    parent,
                    "編集する列",
                    e.fields.iter().map(|f| f.name.clone()).collect(),
                    None,
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                Some(index)
            };
            if id == DROP_FIELD {
                m.delete_field(i, index.unwrap())?;
            } else {
                let f = index.map(|i| e.fields[i].clone()).unwrap_or(Field {
                    important: false,
                    name: format!("column_{}", e.fields.len() + 1),
                    logical_name: String::new(),
                    data_type: match m.kind {
                        DbKind::Oracle => "VARCHAR2(100)",
                        DbKind::Sqlite => "TEXT",
                        _ => "VARCHAR(100)",
                    }
                    .into(),
                    nullable: true,
                    default_sql: None,
                });
                let Some(v) = form_with_types(
                    parent,
                    "列を編集",
                    &[
                        ("物理名", f.name, false),
                        ("論理名", f.logical_name, false),
                        ("型（桁数を含む）", f.data_type, false),
                        (
                            "NULLを許可",
                            if f.nullable { "1" } else { "0" }.into(),
                            true,
                        ),
                        (
                            "既定値（SQL式・空欄は未設定）",
                            f.default_sql.unwrap_or_default(),
                            false,
                        ),
                        (
                            "主要項目（絞り込みER図に表示）",
                            if f.important { "1" } else { "0" }.into(),
                            true,
                        ),
                    ],
                    font,
                    dpi,
                    Some(&super::types::choices(m)),
                ) else {
                    return Ok(None);
                };
                m.replace_field(
                    i,
                    index,
                    Field {
                        important: v[5] == "1",
                        name: v[0].clone(),
                        logical_name: v[1].clone(),
                        data_type: v[2].clone(),
                        nullable: v[3] == "1",
                        default_sql: (!v[4].trim().is_empty()).then(|| v[4].clone()),
                    },
                )?;
            }
        }
        ADD_KEY | EDIT_KEY | DROP_KEY => {
            let index = if id == ADD_KEY {
                None
            } else {
                let Some(index) = pick(
                    parent,
                    "編集するキー",
                    e.keys
                        .iter()
                        .map(|k| format!("{} ({})", k.name, k.columns.join(", ")))
                        .collect(),
                    None,
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                Some(index)
            };
            if id == DROP_KEY {
                e.keys.remove(index.unwrap());
            } else {
                let k = index.map(|i| e.keys[i].clone()).unwrap_or(Key {
                    name: format!("UQ_{}_{}", e.id.name, e.keys.len() + 1),
                    primary: false,
                    columns: vec![],
                });
                let Some(v) = form(
                    parent,
                    "キーを編集",
                    &[
                        ("制約名", k.name, false),
                        (
                            "主キー（オフは一意キー）",
                            if k.primary { "1" } else { "0" }.into(),
                            true,
                        ),
                    ],
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                let Some(cols) = columns(parent, "キーを構成する列", &e, &k.columns, font, dpi)
                else {
                    return Ok(None);
                };
                let k = Key {
                    name: v[0].clone(),
                    primary: v[1] == "1",
                    columns: cols,
                };
                if k.primary {
                    for f in &mut e.fields {
                        if k.columns.contains(&f.name) {
                            f.nullable = false;
                        }
                    }
                }
                if let Some(i) = index {
                    e.keys[i] = k;
                } else {
                    e.keys.push(k);
                }
            }
            m.replace_entity(i, e)?;
        }
        ADD_INDEX | EDIT_INDEX | DROP_INDEX => {
            let index = if id == ADD_INDEX {
                None
            } else {
                let Some(index) = pick(
                    parent,
                    "編集する索引",
                    e.indexes
                        .iter()
                        .map(|k| format!("{} ({})", k.name, k.columns.join(", ")))
                        .collect(),
                    None,
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                Some(index)
            };
            if id == DROP_INDEX {
                e.indexes.remove(index.unwrap());
            } else {
                let k = index.map(|i| e.indexes[i].clone()).unwrap_or(Index {
                    name: format!("IX_{}_{}", e.id.name, e.indexes.len() + 1),
                    unique: false,
                    columns: vec![],
                });
                let Some(v) = form(
                    parent,
                    "索引を編集",
                    &[
                        ("索引名", k.name, false),
                        ("一意索引", if k.unique { "1" } else { "0" }.into(), true),
                    ],
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                let Some(cols) = columns(parent, "索引を構成する列", &e, &k.columns, font, dpi)
                else {
                    return Ok(None);
                };
                let k = Index {
                    name: v[0].clone(),
                    unique: v[1] == "1",
                    columns: cols,
                };
                if let Some(i) = index {
                    e.indexes[i] = k;
                } else {
                    e.indexes.push(k);
                }
            }
            m.replace_entity(i, e)?;
        }
        ADD_RELATION | ADD_LOGICAL | EDIT_RELATION | DROP_RELATION => {
            let candidates = m
                .relations
                .iter()
                .enumerate()
                .filter(|(_, r)| r.child == e.id)
                .map(|(i, r)| {
                    (
                        i,
                        format!(
                            "{} {} → {}",
                            if r.logical {
                                "[論理]"
                            } else {
                                "[外部キー]"
                            },
                            r.name,
                            r.parent.display()
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let index = if matches!(id, ADD_RELATION | ADD_LOGICAL) {
                None
            } else {
                let Some(index) = pick(
                    parent,
                    "編集するリレーション",
                    candidates.iter().map(|(_, n)| n.clone()).collect(),
                    None,
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                Some(candidates[index].0)
            };
            if id == DROP_RELATION {
                m.relations.remove(index.unwrap());
            } else {
                let old = index.map(|i| m.relations[i].clone());
                let logical = id == ADD_LOGICAL || old.as_ref().is_some_and(|r| r.logical);
                let Some(p) = pick(
                    parent,
                    "参照先テーブル",
                    m.entities.iter().map(|e| e.id.display()).collect(),
                    old.as_ref()
                        .and_then(|r| m.entities.iter().position(|e| e.id == r.parent)),
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                let target = &m.entities[p];
                let target_columns = if logical {
                    let Some(cols) = columns(
                        parent,
                        "論理関係の参照先列（制約不要）",
                        target,
                        old.as_ref()
                            .map(|r| r.referenced_columns.as_slice())
                            .unwrap_or(&[]),
                        font,
                        dpi,
                    ) else {
                        return Ok(None);
                    };
                    if cols.is_empty() {
                        return Err("参照先の列を選択してください".into());
                    }
                    cols
                } else {
                    let Some(k) = pick(
                        parent,
                        "参照先の主キーまたは一意キー",
                        target
                            .keys
                            .iter()
                            .map(|k| format!("{} ({})", k.name, k.columns.join(", ")))
                            .collect(),
                        None,
                        font,
                        dpi,
                    ) else {
                        return Ok(None);
                    };
                    target.keys[k].columns.clone()
                };
                let mut cols = vec![];
                for (ordinal, column) in target_columns.iter().enumerate() {
                    let Some(c) = pick(
                        parent,
                        &format!("{} の {} に対応する参照元列", target.id.display(), column),
                        e.fields.iter().map(|f| f.name.clone()).collect(),
                        old.as_ref()
                            .and_then(|r| r.columns.get(ordinal))
                            .and_then(|c| e.fields.iter().position(|f| &f.name == c)),
                        font,
                        dpi,
                    ) else {
                        return Ok(None);
                    };
                    cols.push(e.fields[c].name.clone());
                }
                let Some(name) = input_text(
                    parent,
                    if logical {
                        "論理リレーション名（DB制約は作成しません）"
                    } else {
                        "外部キーの制約名"
                    },
                    &old.map(|r| r.name).unwrap_or_else(|| {
                        format!(
                            "{}_{}_{}",
                            if logical { "LR" } else { "FK" },
                            e.id.name,
                            target.id.name
                        )
                    }),
                    font,
                    dpi,
                ) else {
                    return Ok(None);
                };
                m.replace_relation(
                    index,
                    Relation {
                        logical,
                        name,
                        child: e.id,
                        parent: target.id.clone(),
                        columns: cols,
                        referenced_columns: target_columns,
                    },
                )?;
            }
        }
        _ => {}
    }
    Ok(Some(Some(i)))
}

struct FormState {
    done: std::cell::Cell<bool>,
    accepted: std::cell::Cell<bool>,
}
unsafe extern "system" fn form_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(h, GWLP_USERDATA) as *const FormState;
        if !ptr.is_null() {
            let state = &*ptr;
            if msg == WM_CLOSE || msg == WM_COMMAND && matches!(w.0 & 0xffff, 1 | 2) {
                state.accepted.set(msg == WM_COMMAND && w.0 & 0xffff == 1);
                state.done.set(true);
                let _ = PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
        }
        DefWindowProcW(h, msg, w, l)
    }
}
fn form(
    parent: HWND,
    title: &str,
    fields: &[(&str, String, bool)],
    font: HFONT,
    dpi: u32,
) -> Option<Vec<String>> {
    form_with_types(parent, title, fields, font, dpi, None)
}
fn form_with_types(
    parent: HWND,
    title: &str,
    fields: &[(&str, String, bool)],
    font: HFONT,
    dpi: u32,
    types: Option<&[String]>,
) -> Option<Vec<String>> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        RegisterClassW(&WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("YYDbERForm"),
            lpfnWndProc: Some(form_proc),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            ..Default::default()
        });
        let px = |v: i32| v * dpi as i32 / 96;
        let mut r = RECT::default();
        let _ = GetWindowRect(parent, &mut r);
        let height = 60 + fields.len() as i32 * 60;
        let h = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            w!("YYDbERForm"),
            &HSTRING::from(title),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            r.left + px(40),
            r.top + px(40),
            px(640),
            px(height + 45),
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let state = FormState {
            done: std::cell::Cell::new(false),
            accepted: std::cell::Cell::new(false),
        };
        SetWindowLongPtrW(h, GWLP_USERDATA, &state as *const _ as isize);
        let mut controls = vec![];
        for (i, (label, value, checkbox)) in fields.iter().enumerate() {
            let y = 12 + i as i32 * 60;
            let label = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                &HSTRING::from(*label),
                WS_CHILD | WS_VISIBLE,
                px(14),
                px(y),
                px(590),
                px(20),
                Some(h),
                None,
                Some(instance.into()),
                None,
            )
            .ok()?;
            send_message(label, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            let combo = types.is_some() && i == 2;
            let control = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                if *checkbox {
                    w!("BUTTON")
                } else if combo {
                    w!("COMBOBOX")
                } else {
                    w!("EDIT")
                },
                &HSTRING::from(if *checkbox { "有効" } else { value }),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | if *checkbox {
                        WINDOW_STYLE(BS_AUTOCHECKBOX as u32)
                    } else if combo {
                        WS_VSCROLL | WINDOW_STYLE((CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32)
                    } else {
                        WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32)
                    },
                px(14),
                px(y + 22),
                px(590),
                px(if combo { 300 } else { 28 }),
                Some(h),
                Some(HMENU((100 + i) as *mut _)),
                Some(instance.into()),
                None,
            )
            .ok()?;
            send_message(control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            if combo {
                super::types::populate(control, types.unwrap());
                set_text(control, value);
            } else if *checkbox {
                send_message(
                    control,
                    BM_SETCHECK,
                    WPARAM(usize::from(value == "1")),
                    LPARAM(0),
                );
            } else {
                send_message(control, EM_SETLIMITTEXT, WPARAM(4096), LPARAM(0));
            }
            controls.push(control);
        }
        for (id, label, x) in [(1, "適用", 390), (2, "キャンセル", 500)] {
            let b = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                &HSTRING::from(label),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WINDOW_STYLE(if id == 1 { BS_DEFPUSHBUTTON as u32 } else { 0 }),
                px(x),
                px(height - 40),
                px(105),
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
        let _ = SetFocus(controls.first().copied());
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
            controls
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    if fields[i].2 {
                        if send_message(*h, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1 {
                            "1"
                        } else {
                            "0"
                        }
                        .into()
                    } else {
                        text(*h)
                    }
                })
                .collect()
        });
        let _ = EnableWindow(parent, true);
        let _ = DestroyWindow(h);
        let _ = SetForegroundWindow(parent);
        result
    }
}
