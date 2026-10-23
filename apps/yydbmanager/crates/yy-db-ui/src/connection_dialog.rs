use super::super::profiles::{ConnectionDetails, Profile};
use super::*;
use std::cell::Cell;

struct Dialog {
    fields: Vec<HWND>,
    labels: Vec<HWND>,
    done: Cell<bool>,
    saved: Option<Profile>,
    names: Vec<String>,
    raw: bool,
    kind: DbKind,
    native: bool,
}
fn quote(value: &str) -> String {
    format!("{{{}}}", value.replace('}', "}}"))
}
fn target(kind: DbKind, values: &[String], raw: bool) -> yy_db_core::Result<String> {
    if matches!(kind, DbKind::Sqlite | DbKind::DuckDb) {
        return Ok(if values[2].trim().is_empty() {
            ":memory:".into()
        } else {
            values[2].trim().into()
        });
    }
    if values.get(12).is_some_and(|v| v == "Rust直接接続") {
        let mut config = serde_json::json!({"host":values[4].trim(),"port":values[5].trim().parse::<u16>().map_err(|_|"ポート番号は1～65535です")?,"database":values[6].trim(),"username":values[7].trim(),"password":values[8]});
        if !values[9].trim().is_empty() {
            let extras: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&values[9])
                    .map_err(|e| format!("追加設定はJSONで入力してください: {e}"))?;
            for (k, v) in extras {
                if !["tls", "ca_file", "trust_server_certificate"].contains(&k.as_str()) {
                    return Err(format!("追加設定 {k} は未対応です"));
                }
                config
                    .as_object_mut()
                    .ok_or("接続設定が不正です")?
                    .insert(k, v);
            }
        }
        let config: yy_db_core::native::Config =
            serde_json::from_value(config).map_err(|e| e.to_string())?;
        config.validate()?;
        return config.encode();
    }
    if raw {
        return if values[11].trim().is_empty() {
            Err("接続文字列を入力してください".into())
        } else {
            Ok(values[11].clone())
        };
    }
    if values[3].trim().is_empty() {
        return Err("64bit ODBCドライバーを選択してください".into());
    }
    if values[4].trim().is_empty() {
        return Err("サーバー名を入力してください".into());
    }
    let port = values[5]
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or("ポート番号は1～65535です")?;
    if values[6].trim().is_empty() {
        return Err("データベース名（Oracleはサービス名）を入力してください".into());
    }
    let mut out = format!("Driver={};", quote(values[3].trim()));
    match kind {
        DbKind::Oracle => out.push_str(&format!(
            "DBQ={};",
            quote(&format!(
                "//{}:{port}/{}",
                values[4].trim(),
                values[6].trim()
            ))
        )),
        DbKind::SqlServer => out.push_str(&format!(
            "Server={};Database={};",
            quote(&format!("{},{port}", values[4].trim())),
            quote(values[6].trim())
        )),
        DbKind::Db2 => out.push_str(&format!(
            "Hostname={};Port={port};Database={};Protocol=TCPIP;",
            quote(values[4].trim()),
            quote(values[6].trim())
        )),
        _ => out.push_str(&format!(
            "Server={};Port={port};Database={};",
            quote(values[4].trim()),
            quote(values[6].trim())
        )),
    }
    if kind == DbKind::SqlServer && values[7].trim().is_empty() {
        out.push_str("Trusted_Connection=yes;");
    } else {
        if values[7].trim().is_empty() {
            return Err("ユーザー名を入力してください".into());
        }
        out.push_str(&format!(
            "Uid={};Pwd={};",
            quote(values[7].trim()),
            quote(&values[8])
        ));
    }
    out.push_str(values[9].trim());
    Ok(out)
}
fn refresh(h: HWND, s: &mut Dialog) {
    let index = unsafe {
        send_message(s.fields[1], CB_GETCURSEL, WPARAM(0), LPARAM(0))
            .0
            .max(0) as usize
    };
    let kind = DbKind::ALL[index.min(6)];
    if kind != s.kind {
        s.raw = false;
        unsafe {
            send_message(s.fields[10], BM_SETCHECK, WPARAM(0), LPARAM(0));
        }
        set_text(
            s.fields[5],
            match kind {
                DbKind::Oracle => "1521",
                DbKind::SqlServer => "1433",
                DbKind::PostgreSql => "5432",
                DbKind::MySql => "3306",
                DbKind::Db2 => "50000",
                _ => "",
            },
        );
        s.kind = kind;
        let driver = yy_db_core::session::odbc_drivers()
            .unwrap_or_default()
            .into_iter()
            .find(|d| {
                let d = d.to_lowercase();
                match kind {
                    DbKind::Oracle => d.contains("oracle"),
                    DbKind::SqlServer => d.contains("sql server"),
                    DbKind::PostgreSql => d.contains("postgres"),
                    DbKind::MySql => d.contains("mysql") || d.contains("mariadb"),
                    DbKind::Db2 => d.contains("db2"),
                    _ => false,
                }
            })
            .unwrap_or_default();
        set_text(s.fields[3], &driver);
    }
    let embedded = matches!(kind, DbKind::Sqlite | DbKind::DuckDb);
    for i in 0..s.fields.len() {
        let visible = match i {
            0 | 1 => true,
            2 => embedded,
            3 => !embedded && !s.native && !s.raw,
            4..=9 => !embedded && (!s.raw || s.native),
            10 => !embedded && !s.native,
            11 => !embedded && !s.native && s.raw,
            12 => !embedded,
            _ => false,
        };
        unsafe {
            let _ = ShowWindow(s.fields[i], if visible { SW_SHOW } else { SW_HIDE });
            let _ = ShowWindow(s.labels[i], if visible { SW_SHOW } else { SW_HIDE });
        }
    }
    unsafe {
        let _ = ShowWindow(
            GetDlgItem(Some(h), 3).unwrap_or_default(),
            if embedded { SW_SHOW } else { SW_HIDE },
        );
    }
    let dpi = unsafe { GetDpiForWindow(h) }.max(96);
    let px = |n: i32| n * dpi as i32 / 96;
    unsafe {
        let _ = SetWindowPos(
            h,
            None,
            0,
            0,
            px(700),
            px(if embedded { 314 } else { 616 }),
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let hint = GetDlgItem(Some(h), 20).unwrap_or_default();
        let _ = MoveWindow(
            hint,
            px(16),
            px(if embedded { 140 } else { 462 }),
            px(646),
            px(54),
            true,
        );
        for (id, x) in [(1, 442), (2, 558)] {
            let _ = MoveWindow(
                GetDlgItem(Some(h), id).unwrap_or_default(),
                px(x),
                px(if embedded { 234 } else { 538 }),
                px(104),
                px(28),
                true,
            );
        }
    }
    set_text(
        unsafe { GetDlgItem(Some(h), 20).unwrap_or_default() },
        if embedded {
            "ファイルはフルパスを指定します。空欄または :memory: は保存しないメモリDBです。"
        } else if s.native {
            "ODBC・Java不要。追加設定はJSONです（例: {\"tls\":\"disable\"}）。通常はTLS必須。Oracleは試験対応、Db2はZeus由来のDRDA実装です。制限はヘルプを参照してください。"
        } else if s.raw {
            "既存の接続文字列は伏せて表示します。項目入力へ切り替える場合は接続情報を入力し直してください。"
        } else {
            "ポートは通常の接続ポートです。追加オプションは Encrypt=yes; などを必要に応じて指定します。SQL Serverはユーザー名を空欄にするとWindows認証です。"
        },
    );
}
unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let p = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *mut Dialog;
    if !p.is_null() {
        let s = unsafe { &mut *p };
        if m == WM_CLOSE {
            s.done.set(true);
            unsafe {
                PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0)).ok();
            }
            return LRESULT(0);
        }
        if m == WM_COMMAND {
            let id = (w.0 & 0xffff) as u16;
            if id == 112 && (w.0 >> 16) == CBN_SELCHANGE as usize {
                s.native =
                    unsafe { send_message(s.fields[12], CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 }
                        == 0;
                s.raw = false;
                unsafe {
                    send_message(s.fields[10], BM_SETCHECK, WPARAM(0), LPARAM(0));
                }
                set_text(s.fields[9], "");
                refresh(h, s);
                return LRESULT(0);
            }
            if id == 101 && (w.0 >> 16) == CBN_SELCHANGE as usize {
                refresh(h, s);
                return LRESULT(0);
            }
            if id == 110 {
                s.raw =
                    unsafe { send_message(s.fields[10], BM_GETCHECK, WPARAM(0), LPARAM(0)).0 } == 1;
                refresh(h, s);
                return LRESULT(0);
            }
            if id == 3 {
                if let Some(path) = file_dialog(false, "sqlite") {
                    set_text(s.fields[2], &path.to_string_lossy());
                }
                return LRESULT(0);
            }
            if id == 1 {
                let values = s.fields.iter().map(|f| text(*f)).collect::<Vec<_>>();
                let name = values[0].trim();
                let value = if name.is_empty() {
                    Err("接続名を入力してください".into())
                } else if s.names.iter().any(|n| n == name) {
                    Err("同じ接続名が登録されています。別の名前を指定してください".into())
                } else {
                    target(s.kind, &values, s.raw)
                };
                match value {
                    Ok(target) => {
                        s.saved = Some(Profile {
                            name: name.into(),
                            kind: s.kind,
                            target,
                            details: if s.raw || matches!(s.kind, DbKind::Sqlite | DbKind::DuckDb) {
                                None
                            } else {
                                Some(ConnectionDetails {
                                    native: s.native,
                                    driver: values[3].clone(),
                                    server: values[4].clone(),
                                    port: values[5].clone(),
                                    database: values[6].clone(),
                                    username: values[7].clone(),
                                    password: values[8].clone(),
                                    options: values[9].clone(),
                                })
                            },
                        });
                        s.done.set(true);
                    }
                    Err(e) => unsafe {
                        MessageBoxW(
                            Some(h),
                            &HSTRING::from(e),
                            w!("接続先の設定"),
                            MB_OK | MB_ICONWARNING,
                        );
                    },
                }
                unsafe {
                    PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0)).ok();
                }
                return LRESULT(0);
            }
            if id == 2 {
                s.done.set(true);
                unsafe {
                    PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0)).ok();
                }
                return LRESULT(0);
            }
        }
    }
    unsafe { DefWindowProcW(h, m, w, l) }
}
pub(super) fn edit(
    parent: HWND,
    profile: Option<Profile>,
    names: Vec<String>,
    font: HFONT,
    dpi: u32,
) -> Option<Profile> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        RegisterClassW(&WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("YYDbConnection"),
            lpfnWndProc: Some(proc),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        });
        let px = |v: i32| v * dpi as i32 / 96;
        let mut r = RECT::default();
        GetWindowRect(parent, &mut r).ok()?;
        let h = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            w!("YYDbConnection"),
            w!("接続先の設定"),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            r.left + px(30),
            r.top + px(20),
            px(700),
            px(580),
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let is_new = profile.is_none();
        let p = profile.unwrap_or(Profile {
            name: String::new(),
            kind: DbKind::Sqlite,
            target: ":memory:".into(),
            details: None,
        });
        let embedded = matches!(p.kind, DbKind::Sqlite | DbKind::DuckDb);
        let direct = yy_db_core::native::Config::decode(&p.target).ok().flatten();
        let mut s = Dialog {
            fields: vec![],
            labels: vec![],
            done: Cell::new(false),
            saved: None,
            names,
            raw: !embedded && p.details.is_none(),
            kind: p.kind,
            native: is_new || direct.is_some(),
        };
        let labels = [
            "接続名（必須）",
            "データベースの種類",
            "ファイルのパス",
            "ODBCドライバー",
            "サーバー名",
            "ポート番号",
            "DB名 / サービス名",
            "ユーザー名",
            "パスワード",
            "追加オプション（任意）",
            "",
            "接続文字列（詳細）",
            "接続方式",
        ];
        for (i, label) in labels.iter().enumerate() {
            let y = match i {
                0 => 16,
                1 => 52,
                2 | 12 => 88,
                3 => 124,
                4 => 160,
                5 => 196,
                6 => 232,
                7 => 268,
                8 => 304,
                9 => 340,
                10 => 380,
                _ => 418,
            };
            let lab = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                &HSTRING::from(*label),
                WS_CHILD | WS_VISIBLE,
                px(16),
                px(y + 4),
                px(176),
                px(24),
                Some(h),
                None,
                Some(instance.into()),
                None,
            )
            .ok()?;
            let (class, style) = if i == 1 || i == 12 {
                (w!("COMBOBOX"), CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0)
            } else if i == 3 {
                (w!("COMBOBOX"), CBS_DROPDOWN as u32 | WS_VSCROLL.0)
            } else if i == 10 {
                (w!("BUTTON"), BS_AUTOCHECKBOX as u32)
            } else {
                (
                    w!("EDIT"),
                    WS_BORDER.0
                        | ES_AUTOHSCROLL as u32
                        | if i == 8 || i == 11 {
                            ES_PASSWORD as u32
                        } else {
                            0
                        },
                )
            };
            let f = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                if i == 10 {
                    w!("接続文字列を直接指定する（詳細設定）")
                } else {
                    w!("")
                },
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(style),
                px(196),
                px(y),
                px(if i == 2 { 350 } else { 466 }),
                px(if i == 1 || i == 3 || i == 12 { 240 } else { 28 }),
                Some(h),
                Some(HMENU((100 + i) as *mut _)),
                Some(instance.into()),
                None,
            )
            .ok()?;
            send_message(lab, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            send_message(f, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            s.labels.push(lab);
            s.fields.push(f);
        }
        for kind in DbKind::ALL {
            send_message(
                s.fields[1],
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(kind.name()).as_ptr() as isize),
            );
        }
        send_message(
            s.fields[1],
            CB_SETCURSEL,
            WPARAM(DbKind::ALL.iter().position(|k| *k == p.kind).unwrap_or(0)),
            LPARAM(0),
        );
        for driver in yy_db_core::session::odbc_drivers().unwrap_or_default() {
            send_message(
                s.fields[3],
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(&driver).as_ptr() as isize),
            );
        }
        for label in ["Rust直接接続", "ODBC"] {
            send_message(
                s.fields[12],
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(label).as_ptr() as isize),
            );
        }
        send_message(
            s.fields[12],
            CB_SETCURSEL,
            WPARAM(usize::from(!s.native)),
            LPARAM(0),
        );
        set_text(s.fields[0], &p.name);
        set_text(s.fields[2], if embedded { &p.target } else { ":memory:" });
        set_text(s.fields[11], if embedded { "" } else { &p.target });
        send_message(
            s.fields[10],
            BM_SETCHECK,
            WPARAM(usize::from(s.raw)),
            LPARAM(0),
        );
        if let Some(d) = &p.details {
            for (i, value) in [
                (3, &d.driver),
                (4, &d.server),
                (5, &d.port),
                (6, &d.database),
                (7, &d.username),
                (8, &d.password),
                (9, &d.options),
            ] {
                set_text(s.fields[i], value);
            }
        }
        if let Some(d) = direct {
            for (i,value) in [(4,d.host),(5,d.port.to_string()),(6,d.database),(7,d.username),(8,d.password),(9,serde_json::json!({"tls":d.tls,"ca_file":d.ca_file,"trust_server_certificate":d.trust_server_certificate}).to_string())] {set_text(s.fields[i],&value);}
            s.raw = false;
        }
        for (id, label, x, y, width, height, class, style) in [
            (
                3,
                "参照…",
                554,
                88,
                108,
                28,
                w!("BUTTON"),
                BS_PUSHBUTTON as u32,
            ),
            (20, "", 16, 426, 646, 54, w!("STATIC"), 0),
            (
                1,
                "保存",
                442,
                502,
                104,
                28,
                w!("BUTTON"),
                BS_DEFPUSHBUTTON as u32,
            ),
            (
                2,
                "キャンセル",
                558,
                502,
                104,
                28,
                w!("BUTTON"),
                BS_PUSHBUTTON as u32,
            ),
        ] {
            let f = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                &HSTRING::from(label),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(style),
                px(x),
                px(y),
                px(width),
                px(height),
                Some(h),
                Some(HMENU(id as *mut _)),
                Some(instance.into()),
                None,
            )
            .ok()?;
            send_message(f, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
        }
        SetWindowLongPtrW(h, GWLP_USERDATA, &mut s as *mut _ as isize);
        refresh(h, &mut s);
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(h, SW_SHOW);
        SetFocus(Some(s.fields[0])).ok();
        let mut msg = MSG::default();
        while !s.done.get() {
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
        let _ = EnableWindow(parent, true);
        DestroyWindow(h).ok();
        let _ = SetForegroundWindow(parent);
        s.saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_connection_needs_no_odbc_driver_and_preserves_credentials() {
        let mut values = vec![String::new(); 13];
        for (i, value) in [
            (4, "localhost"),
            (5, "50000"),
            (6, "YYDEMO"),
            (7, "db2inst1"),
            (8, "p;'}\\w"),
            (9, "{\"tls\":\"disable\"}"),
            (12, "Rust直接接続"),
        ] {
            values[i] = value.into();
        }
        let target = target(DbKind::Db2, &values, false).unwrap();
        let decoded = yy_db_core::native::Config::decode(&target)
            .unwrap()
            .unwrap();
        assert_eq!(decoded.password, "p;'}\\w");
        assert_eq!(decoded.tls, "disable");
        values[9] = "{\"password\":\"override\"}".into();
        assert!(super::target(DbKind::Db2, &values, false).is_err());
    }
    #[test]
    fn structured_targets_quote_credentials_and_select_native_server_attributes() {
        let mut values = vec![String::new(); 12];
        for (i, v) in [
            (3, "Driver"),
            (4, "db.example"),
            (5, "5432"),
            (6, "sales;archive"),
            (7, "user"),
            (8, "pass;}word"),
        ] {
            values[i] = v.into();
        }
        let pg = target(DbKind::PostgreSql, &values, false).unwrap();
        assert!(pg.contains("Database={sales;archive};") && pg.contains("Pwd={pass;}}word};"));
        let oracle = target(DbKind::Oracle, &values, false).unwrap();
        assert!(oracle.contains("DBQ={//db.example:5432/sales;archive};"));
        values[7].clear();
        values[8].clear();
        let mssql = target(DbKind::SqlServer, &values, false).unwrap();
        assert!(
            mssql.contains("Server={db.example,5432};")
                && mssql.contains("Trusted_Connection=yes;")
        );
        for bad in ["0", "65536", "1433;Uid=other"] {
            values[5] = bad.into();
            assert!(target(DbKind::MySql, &values, false).is_err());
        }
        values[11] = "Driver={legacy};Pwd={a;b};".into();
        assert_eq!(target(DbKind::Db2, &values, true).unwrap(), values[11]);
    }
}
