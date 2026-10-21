//! Explorer diagnostics create editable SQL documents; selection never submits SQL.
use super::*;
use std::collections::BTreeMap;
use yy_db_core::diagnostics;

const BASE: u16 = 20000;
const SLOTS: u16 = 16;

#[derive(Clone)]
struct Target {
    name: String,
    kind: DbKind,
    target: String,
}

#[derive(Clone, Default)]
pub(super) struct State {
    origins: BTreeMap<usize, Target>,
    pub connected: Option<(DbKind, String)>,
    pub pending: Option<(DbKind, String)>,
}
impl State {
    pub fn clear_tab(&mut self, index: usize) {
        self.origins.remove(&index);
    }
    pub fn check(&self, index: usize) -> yy_db_core::Result<()> {
        if let Some(origin) = self.origins.get(&index)
            && !self
                .connected
                .as_ref()
                .is_some_and(|(kind, target)| *kind == origin.kind && *target == origin.target)
        {
            return Err(format!(
                "この性能分析SQLの対象は「{}」（{}）です。対象接続先に接続してから実行してください。",
                origin.name,
                origin.kind.name()
            ));
        }
        Ok(())
    }
}

fn decode(id: u16) -> Option<(DbKind, usize)> {
    let slot = id.checked_sub(BASE)?;
    Some((
        *DbKind::ALL.get((slot / SLOTS) as usize)?,
        (slot % SLOTS) as usize,
    ))
}
fn command_id(kind: DbKind, index: usize) -> u16 {
    BASE + DbKind::ALL.iter().position(|k| *k == kind).unwrap() as u16 * SLOTS + index as u16
}

pub(super) fn append_menu(parent: HMENU, kind: DbKind) -> Result<()> {
    let templates =
        diagnostics::templates(kind).map_err(|e| windows::core::Error::new(E_FAIL, e))?;
    unsafe {
        let menu = CreatePopupMenu()?;
        let result = (|| -> Result<()> {
            for (i, template) in templates.iter().enumerate() {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    command_id(kind, i) as usize,
                    &HSTRING::from(template.title),
                )?;
            }
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            AppendMenuW(
                menu,
                MF_STRING,
                HELP_PERFORMANCE as usize,
                w!("必要な権限・指標の読み方…"),
            )?;
            AppendMenuW(
                parent,
                MF_POPUP,
                menu.0 as usize,
                &HSTRING::from(format!("性能分析SQL（{}）", kind.name())),
            )
        })();
        if result.is_err() {
            let _ = DestroyMenu(menu);
        }
        result
    }
}

fn open(a: &mut App, kind: DbKind, index: usize) -> yy_db_core::Result<()> {
    let selected = HTREEITEM(unsafe {
        send_message(
            a.tree,
            TVM_GETNEXTITEM,
            WPARAM(TVGN_CARET as usize),
            LPARAM(0),
        )
        .0
    });
    let profile = tree_profile(a, selected)
        .and_then(|i| a.profiles.get(i))
        .ok_or("性能分析SQLの対象接続先を選択してください")?
        .clone();
    if profile.kind != kind {
        return Err(
            "接続先のDB種類が変わりました。右クリックメニューから選び直してください".into(),
        );
    }
    let templates = diagnostics::templates(kind)?;
    let template = templates
        .get(index)
        .ok_or("未対応の性能分析テンプレートです")?;
    let editor = Editor::new(&template.editor_text(kind, &profile.name), a.dpi)
        .map_err(|e| e.to_string())?;
    document_tabs::select(a, false);
    a.editors.push(editor);
    a.sql_index = a.editors.len() - 1;
    add_sql_tab(
        a.sql_tabs,
        a.sql_index,
        &format!("{}: {}", profile.name, template.title),
    );
    a.performance.origins.insert(
        a.sql_index,
        Target {
            name: profile.name.clone(),
            kind,
            target: profile.target,
        },
    );
    invalidate(a.editor_hwnd);
    set_text(
        a.status,
        &format!(
            "「{}」の{}をSQLタブに表示しました。未実行です。対象接続先に接続し、内容を確認してF5で実行します。",
            profile.name, template.title
        ),
    );
    unsafe {
        let _ = SetFocus(Some(a.editor_hwnd));
    }
    Ok(())
}

pub(super) fn command(id: u16) -> bool {
    let Some((kind, index)) = decode(id) else {
        return false;
    };
    if let Some(Err(error)) = with_app(|a| open(a, kind, index)) {
        message(&error);
    }
    if let Some(hwnd) = with_app(|a| unsafe { GetParent(a.grid_hwnd).unwrap_or_default() }) {
        layout(hwnd);
    }
    true
}

pub(super) fn smoke(a: &mut App, directory: &std::path::Path) -> yy_db_core::Result<()> {
    let old_profiles = a.profiles.clone();
    let old_objects = a.tree_items.iter().map(|e| e.1.clone()).collect();
    let old_index = a.sql_index;
    let old_len = a.editors.len();
    let old_er = a.documents.er_active;
    let old_texts: Vec<_> = a.editors.iter().map(|e| e.text()).collect();
    let old_state = a.performance.clone();
    let old_busy = a.busy;
    let selected = unsafe { send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    a.profiles = DbKind::ALL
        .iter()
        .map(|kind| super::super::profiles::Profile {
            name: format!("性能分析検証 {}", kind.name()),
            kind: *kind,
            target: format!("unused://{}", kind.name()),
            details: None,
        })
        .collect();
    refresh_profiles(a);
    populate_tree(a, vec![]);
    let result = (|| -> yy_db_core::Result<()> {
        let mut total = 0;
        for (profile_index, kind) in DbKind::ALL.into_iter().enumerate() {
            let item = a
                .profile_items
                .iter()
                .find(|(_, i)| *i == profile_index)
                .unwrap()
                .0;
            unsafe {
                send_message(
                    a.tree,
                    TVM_SELECTITEM,
                    WPARAM(TVGN_CARET as usize),
                    LPARAM(item.0),
                );
            }
            let menu = unsafe { CreatePopupMenu().map_err(|e| e.to_string())? };
            let menu_result = append_menu(menu, kind).map_err(|e| e.to_string());
            let templates = diagnostics::templates(kind)?;
            let menu_valid = menu_result.is_ok()
                && unsafe {
                    let submenu = GetSubMenu(menu, 0);
                    GetMenuItemCount(Some(submenu)) == templates.len() as i32 + 2
                        && GetMenuItemID(submenu, 0) == command_id(kind, 0) as u32
                };
            unsafe {
                let _ = DestroyMenu(menu);
            }
            if !menu_valid {
                return Err(format!("{}の性能分析メニューが不正です", kind.name()));
            }
            for (index, template) in templates.iter().enumerate() {
                open(a, kind, index)?;
                if a.editors[a.sql_index].text()
                    != template.editor_text(kind, &a.profiles[profile_index].name)
                    || a.busy != old_busy
                    || a.performance.connected != old_state.connected
                    || a.performance.check(a.sql_index).is_ok()
                {
                    return Err("性能分析SQLの表示・接続先・未実行状態が不正です".into());
                }
                total += 1;
            }
        }
        if old_texts
            .iter()
            .zip(&a.editors)
            .any(|(before, after)| *before != after.text())
        {
            return Err("性能分析テンプレートが既存SQLを上書きしました".into());
        }
        std::fs::write(directory.join("performance-templates-smoke.txt"),format!("7 DB / {total} diagnostic templates: correct menu, editable SQL, no execution, original documents preserved, connection guard OK\n")).map_err(|e| e.to_string())
    })();
    a.editors.truncate(old_len);
    for _ in old_len..a.sql_index + 1 {
        unsafe {
            send_message(a.sql_tabs, TCM_DELETEITEM, WPARAM(old_len), LPARAM(0));
        }
    }
    a.sql_index = old_index;
    a.performance = old_state;
    a.profiles = old_profiles;
    refresh_profiles(a);
    unsafe {
        send_message(
            a.profile_combo,
            CB_SETCURSEL,
            WPARAM(selected as usize),
            LPARAM(0),
        );
    }
    populate_tree(a, old_objects);
    document_tabs::select(a, old_er);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_execution_requires_the_original_database_and_connection() {
        let mut state = State::default();
        state.origins.insert(
            1,
            Target {
                name: "検証".into(),
                kind: DbKind::PostgreSql,
                target: "original".into(),
            },
        );
        assert!(state.check(0).is_ok());
        assert!(state.check(1).is_err());
        state.connected = Some((DbKind::MySql, "original".into()));
        assert!(state.check(1).is_err());
        state.connected = Some((DbKind::PostgreSql, "other".into()));
        assert!(state.check(1).is_err());
        state.connected = Some((DbKind::PostgreSql, "original".into()));
        assert!(state.check(1).is_ok());
        state.clear_tab(1);
        state.connected = None;
        assert!(state.check(1).is_ok());
    }
}
