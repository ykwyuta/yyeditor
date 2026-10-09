//! プロキシのプロファイルの編集（19 章 3.2）。メモリ上のダイアログテンプレートから作る
//! （[`crate::goto::Template`]）。
//!
//! 文字の入力はホスト名・ポートなどの最小限にし、種類や対象はプルダウンで選ぶ。ドメインごとのプロキシと
//! ホストの転送は一覧にして、追加・編集は小さな画面（プルダウン + ホスト・ポート）で行う。説明は入力欄の
//! 薄い字（キューバナー）に出し、見切れないようにする。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, CheckDlgButton, EM_SETCUEBANNER, IsDlgButtonChecked,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_browser::form::{
    Endpoint, PortChoice, ProxyKind, Target, describe_host_map, describe_rule, join_address,
    join_pattern, rule_route, split_host_map, split_pattern,
};
use yy_browser::{HostMap, ProfileList, ProxyMode, ProxyProfile, ProxyRule};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const CLASS_LISTBOX: u16 = 0x0083;
const CLASS_COMBO: u16 = 0x0085;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const NO_ID: u16 = 0xFFFF;

// ---- 共通 ---------------------------------------------------------------------------------

fn item(dlg: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(Some(dlg), id as i32).unwrap_or_default() }
}

fn get_text(dlg: HWND, id: u16) -> String {
    unsafe {
        let h = item(dlg, id);
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got]).trim().to_owned()
    }
}

fn set_text(dlg: HWND, id: u16, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(dlg, id as i32, &HSTRING::from(s));
    }
}

fn cue(dlg: HWND, id: u16, s: &str) {
    let s = HSTRING::from(s);
    unsafe {
        SendMessageW(
            item(dlg, id),
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(s.as_ptr() as isize)),
        );
    }
}

fn enable(dlg: HWND, id: u16, on: bool) {
    unsafe {
        let _ = EnableWindow(item(dlg, id), on);
    }
}

fn checked(dlg: HWND, id: u16) -> bool {
    unsafe { IsDlgButtonChecked(dlg, id as i32) == BST_CHECKED.0 }
}

fn set_check(dlg: HWND, id: u16, on: bool) {
    unsafe {
        let _ = CheckDlgButton(dlg, id as i32, if on { BST_CHECKED } else { BST_UNCHECKED });
    }
}

fn combo_fill(dlg: HWND, id: u16, items: &[&str], sel: usize) {
    unsafe {
        SendDlgItemMessageW(dlg, id as i32, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for s in items {
            let s = HSTRING::from(*s);
            SendDlgItemMessageW(
                dlg,
                id as i32,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(dlg, id as i32, CB_SETCURSEL, WPARAM(sel), LPARAM(0));
    }
}

fn combo_sel(dlg: HWND, id: u16) -> usize {
    unsafe {
        SendDlgItemMessageW(dlg, id as i32, CB_GETCURSEL, WPARAM(0), LPARAM(0))
            .0
            .max(0) as usize
    }
}

fn combo_set(dlg: HWND, id: u16, sel: usize) {
    unsafe {
        SendDlgItemMessageW(dlg, id as i32, CB_SETCURSEL, WPARAM(sel), LPARAM(0));
    }
}

fn list_fill(dlg: HWND, id: u16, items: &[String], sel: Option<usize>) {
    unsafe {
        SendDlgItemMessageW(dlg, id as i32, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for s in items {
            let s = HSTRING::from(s.as_str());
            SendDlgItemMessageW(
                dlg,
                id as i32,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        if let Some(i) = sel.filter(|i| *i < items.len()) {
            SendDlgItemMessageW(dlg, id as i32, LB_SETCURSEL, WPARAM(i), LPARAM(0));
        }
    }
}

fn list_sel(dlg: HWND, id: u16) -> Option<usize> {
    let i = unsafe { SendDlgItemMessageW(dlg, id as i32, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    (i >= 0).then_some(i as usize)
}

fn focus(dlg: HWND, id: u16) {
    unsafe {
        let _ = SetFocus(Some(item(dlg, id)));
    }
}

/// ポートの欄を読む（空なら `None`、だめなら理由）。
fn read_port(dlg: HWND, id: u16) -> Result<Option<u16>, String> {
    let s = get_text(dlg, id);
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<u16>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(format!("ポート「{s}」は 1〜65535 の数で入れてください")),
    }
}

/// テンプレートに部品を足す小さな道具。
struct T(Template);

impl T {
    fn label(&mut self, x: i16, y: i16, w: i16, text: &str) {
        self.0.item(0, x, y + 2, w, 10, NO_ID, CLASS_STATIC, text);
    }
    fn edit(&mut self, x: i16, y: i16, w: i16, id: u16, extra: u32) {
        self.0.item(
            (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32 | extra,
            x,
            y,
            w,
            13,
            id,
            CLASS_EDIT,
            "",
        );
    }
    fn combo(&mut self, x: i16, y: i16, w: i16, id: u16) {
        self.0.item(
            (WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
            x,
            y,
            w,
            140,
            id,
            CLASS_COMBO,
            "",
        );
    }
    fn button(&mut self, x: i16, y: i16, w: i16, id: u16, text: &str) {
        self.0.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            x,
            y,
            w,
            14,
            id,
            CLASS_BUTTON,
            text,
        );
    }
    fn check(&mut self, x: i16, y: i16, w: i16, id: u16, text: &str) {
        self.0.item(
            WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            x,
            y,
            w,
            12,
            id,
            CLASS_BUTTON,
            text,
        );
    }
    fn group(&mut self, x: i16, y: i16, w: i16, h: i16, id: u16, text: &str) {
        self.0
            .item(BS_GROUPBOX as u32, x, y, w, h, id, CLASS_BUTTON, text);
    }
    fn list(&mut self, x: i16, y: i16, w: i16, h: i16, id: u16) {
        self.0.item(
            (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            x,
            y,
            w,
            h,
            id,
            CLASS_LISTBOX,
            "",
        );
    }
    fn ok_cancel(&mut self, right: i16, y: i16, ok: &str) {
        self.0.item(
            WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
            right - 104,
            y,
            50,
            14,
            IDOK_,
            CLASS_BUTTON,
            ok,
        );
        self.button(right - 50, y, 50, IDCANCEL_, "キャンセル");
    }
}

/// モーダルのダイアログを出す（`state` は `GWLP_USERDATA` で渡す）。
fn run<S>(owner: HWND, t: T, proc_: DLGPROC, state: &mut S) {
    let aligned = t.0.aligned();
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            proc_,
            LPARAM(state as *mut S as isize),
        );
    }
}

/// ダイアログの状態（`GWLP_USERDATA`）。
fn state_of<'a, S>(dlg: HWND) -> &'a mut S {
    unsafe { &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut S) }
}

const KIND_ADVANCED: usize = 4;

/// 種類のプルダウンの項目（プロキシの種類 + 「詳しい指定」）。
fn kind_items(advanced: bool) -> Vec<&'static str> {
    let mut v: Vec<&str> = ProxyKind::ALL.iter().map(|k| k.label()).collect();
    if advanced {
        v.push("詳しい指定");
    }
    v
}

// ---- プロファイルの一覧（メインの画面） -----------------------------------------------------

const ID_PROFILE: u16 = 100;
const ID_NEW: u16 = 101;
const ID_COPY: u16 = 102;
const ID_DELETE: u16 = 103;
const ID_NAME: u16 = 104;
const ID_MODE: u16 = 105;
const ID_KIND: u16 = 106;
const ID_HOST: u16 = 107;
const ID_PORT: u16 = 108;
const ID_LOCAL: u16 = 110;
const ID_BYPASS: u16 = 111;
const ID_PAC: u16 = 112;
const ID_RULES_GROUP: u16 = 119;
const ID_RULES: u16 = 120;
const ID_RULE_ADD: u16 = 121;
const ID_RULE_EDIT: u16 = 122;
const ID_RULE_DEL: u16 = 123;
const ID_RULE_UP: u16 = 124;
const ID_RULE_DOWN: u16 = 125;
const ID_HOSTS: u16 = 130;
const ID_HOST_ADD: u16 = 131;
const ID_HOST_EDIT: u16 = 132;
const ID_HOST_DEL: u16 = 133;
const ID_ADBLOCK: u16 = 140;
const ID_DEFAULT: u16 = 141;

struct State {
    list: ProfileList,
    /// 表示中のプロファイル（`list.profiles` の番号）
    current: usize,
    /// OK で閉じたときの結果
    done: bool,
}

/// プロファイルの一覧を編集する。OK なら編集後の一覧（確かめ済み）。
pub(super) fn edit(owner: HWND, list: ProfileList) -> Option<ProfileList> {
    let mut t = T(Template::dialog("プロキシの設定", 400, 368));
    // プロファイル
    t.label(7, 7, 60, "プロファイル");
    t.combo(70, 7, 166, ID_PROFILE);
    t.button(242, 6, 46, ID_NEW, "新規");
    t.button(292, 6, 46, ID_COPY, "複製");
    t.button(342, 6, 51, ID_DELETE, "削除");
    t.label(7, 26, 60, "名前");
    t.edit(70, 26, 323, ID_NAME, 0);
    // ふだんの経路
    t.group(7, 44, 386, 104, NO_ID, "ふだんの経路");
    t.label(15, 58, 52, "やり方");
    t.combo(70, 58, 166, ID_MODE);
    t.label(15, 76, 52, "プロキシ");
    t.combo(70, 76, 66, ID_KIND);
    t.edit(140, 76, 180, ID_HOST, 0);
    t.label(323, 76, 6, ":");
    t.edit(331, 76, 54, ID_PORT, ES_NUMBER as u32);
    t.check(
        70,
        93,
        315,
        ID_LOCAL,
        "ドットのない名前（社内のサーバーなど）は直接",
    );
    t.label(15, 108, 52, "直接にする");
    t.edit(70, 108, 315, ID_BYPASS, 0);
    t.label(15, 127, 52, "PAC の URL");
    t.edit(70, 127, 315, ID_PAC, 0);
    // ドメインごと
    t.group(7, 154, 386, 92, ID_RULES_GROUP, "");
    t.list(15, 168, 300, 70, ID_RULES);
    t.button(322, 168, 63, ID_RULE_ADD, "追加...");
    t.button(322, 183, 63, ID_RULE_EDIT, "編集...");
    t.button(322, 198, 63, ID_RULE_DEL, "削除");
    t.button(322, 213, 30, ID_RULE_UP, "↑");
    t.button(355, 213, 30, ID_RULE_DOWN, "↓");
    // ホストの転送
    t.group(
        7,
        252,
        386,
        70,
        NO_ID,
        "ホストの転送（hosts の書き換えと同じ。このプロファイルだけ）",
    );
    t.list(15, 266, 300, 48, ID_HOSTS);
    t.button(322, 266, 63, ID_HOST_ADD, "追加...");
    t.button(322, 281, 63, ID_HOST_EDIT, "編集...");
    t.button(322, 296, 63, ID_HOST_DEL, "削除");
    t.check(7, 330, 150, ID_ADBLOCK, "広告ブロックを使う");
    t.check(
        170,
        330,
        223,
        ID_DEFAULT,
        "起動するときにこのプロファイルを使う",
    );
    t.ok_cancel(393, 348, "保存");
    let current = list
        .profiles
        .iter()
        .position(|p| p.name == list.default)
        .unwrap_or(0);
    let mut state = State {
        list,
        current,
        done: false,
    };
    run(owner, t, Some(main_proc), &mut state);
    state.done.then_some(state.list)
}

/// プロファイルの組み合わせボックスを作り直す。
fn fill_profiles(dlg: HWND, st: &State) {
    let names: Vec<&str> = st.list.profiles.iter().map(|p| p.name.as_str()).collect();
    combo_fill(dlg, ID_PROFILE, &names, st.current);
}

/// 表示中のプロファイルを欄に出す。
fn show(dlg: HWND, st: &State) {
    let Some(p) = st.list.profiles.get(st.current) else {
        return;
    };
    set_text(dlg, ID_NAME, &p.name);
    let mode = ProxyMode::ALL
        .iter()
        .position(|m| *m == p.mode)
        .unwrap_or(0);
    combo_set(dlg, ID_MODE, mode);
    // プロキシ: 種類・ホスト・ポート（読めない書き方は「詳しい指定」）
    match Endpoint::parse(&p.server) {
        Some(e) => {
            combo_fill(
                dlg,
                ID_KIND,
                &kind_items(false),
                ProxyKind::ALL
                    .iter()
                    .position(|k| *k == e.kind)
                    .unwrap_or(0),
            );
            set_text(dlg, ID_HOST, &e.host);
            set_text(dlg, ID_PORT, &e.port.to_string());
        }
        None if p.server.trim().is_empty() => {
            combo_fill(dlg, ID_KIND, &kind_items(false), 0);
            set_text(dlg, ID_HOST, "");
            set_text(dlg, ID_PORT, "");
        }
        None => {
            combo_fill(dlg, ID_KIND, &kind_items(true), KIND_ADVANCED);
            set_text(dlg, ID_HOST, &p.server);
            set_text(dlg, ID_PORT, "");
        }
    }
    let parts: Vec<&str> = p
        .bypass
        .split([';', ',', '\n'])
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .collect();
    set_check(dlg, ID_LOCAL, parts.contains(&"<local>"));
    let others: Vec<&str> = parts.into_iter().filter(|b| *b != "<local>").collect();
    set_text(dlg, ID_BYPASS, &others.join("; "));
    set_text(dlg, ID_PAC, &p.pac_url);
    set_check(dlg, ID_ADBLOCK, !p.adblock_off);
    set_check(dlg, ID_DEFAULT, st.list.default == p.name);
    fill_rules(dlg, p, None);
    fill_hosts(dlg, p, None);
    enable_fields(dlg);
}

fn fill_rules(dlg: HWND, p: &ProxyProfile, sel: Option<usize>) {
    let items: Vec<String> = p.rules.iter().map(describe_rule).collect();
    list_fill(dlg, ID_RULES, &items, sel);
}

fn fill_hosts(dlg: HWND, p: &ProxyProfile, sel: Option<usize>) {
    let items: Vec<String> = p.hosts.iter().map(describe_host_map).collect();
    list_fill(dlg, ID_HOSTS, &items, sel);
}

/// やり方に合わせて欄を使える・使えないにする。
fn enable_fields(dlg: HWND) {
    let mode = ProxyMode::ALL
        .get(combo_sel(dlg, ID_MODE))
        .copied()
        .unwrap_or_default();
    let manual = mode == ProxyMode::Manual;
    let advanced = combo_sel(dlg, ID_KIND) == KIND_ADVANCED;
    enable(dlg, ID_KIND, manual);
    enable(dlg, ID_HOST, manual);
    enable(dlg, ID_PORT, manual && !advanced);
    enable(dlg, ID_LOCAL, manual);
    enable(dlg, ID_BYPASS, manual);
    enable(dlg, ID_PAC, mode == ProxyMode::Pac);
    let rules_ok = matches!(mode, ProxyMode::Direct | ProxyMode::Manual);
    for id in [
        ID_RULES,
        ID_RULE_ADD,
        ID_RULE_EDIT,
        ID_RULE_DEL,
        ID_RULE_UP,
        ID_RULE_DOWN,
    ] {
        enable(dlg, id, rules_ok);
    }
    set_text(
        dlg,
        ID_RULES_GROUP,
        if rules_ok {
            "ドメインごとのプロキシ（上から順に当てはめる）"
        } else {
            "ドメインごとのプロキシ（やり方が「使わない」「指定」のときだけ）"
        },
    );
}

/// 欄の内容を表示中のプロファイルに書き戻す（確かめはしない）。
fn store(dlg: HWND, st: &mut State) {
    let mode = ProxyMode::ALL
        .get(combo_sel(dlg, ID_MODE))
        .copied()
        .unwrap_or_default();
    let kind = combo_sel(dlg, ID_KIND);
    let host = get_text(dlg, ID_HOST);
    let port = get_text(dlg, ID_PORT);
    let local = checked(dlg, ID_LOCAL);
    let others = get_text(dlg, ID_BYPASS);
    let pac = get_text(dlg, ID_PAC);
    let name = get_text(dlg, ID_NAME);
    let adblock = checked(dlg, ID_ADBLOCK);
    let is_default = checked(dlg, ID_DEFAULT);
    let Some(p) = st.list.profiles.get_mut(st.current) else {
        return;
    };
    let old_name = p.name.clone();
    p.name = name;
    p.mode = mode;
    p.server = if kind == KIND_ADVANCED {
        host
    } else if host.is_empty() {
        String::new()
    } else {
        let k = ProxyKind::ALL.get(kind).copied().unwrap_or(ProxyKind::Http);
        match port.parse::<u16>() {
            Ok(n) if n > 0 => Endpoint {
                kind: k,
                host,
                port: n,
            }
            .to_spec(),
            // 空ならよく使うポート。数でなければそのまま（確かめで理由を出す）
            _ if port.is_empty() => Endpoint {
                kind: k,
                host,
                port: k.default_port(),
            }
            .to_spec(),
            _ => format!("{host}:{port}"),
        }
    };
    let mut bypass: Vec<String> = Vec::new();
    if local {
        bypass.push("<local>".into());
    }
    bypass.extend(
        others
            .split([';', ',', ' '])
            .map(str::trim)
            .filter(|b| !b.is_empty() && *b != "<local>")
            .map(str::to_owned),
    );
    p.bypass = bypass.join(";");
    p.pac_url = pac;
    p.adblock_off = !adblock;
    if is_default {
        st.list.default = p.name.clone();
    } else if st.list.default == old_name {
        st.list.default = String::new();
    }
}

/// すべてのプロファイルを確かめる（だめなら、そのプロファイルを表示して理由を返す）。
fn check_all(st: &mut State) -> Result<(), String> {
    for (i, p) in st.list.profiles.iter().enumerate() {
        if let Err(e) = p.validate() {
            st.current = i;
            return Err(format!("「{}」: {e}", p.name));
        }
        if st.list.profiles[..i].iter().any(|q| q.name == p.name) {
            st.current = i;
            return Err(format!(
                "「{}」という名前のプロファイルが 2 つあります",
                p.name
            ));
        }
    }
    if st.list.get(&st.list.default).is_none() {
        st.list.default = st.list.profiles[0].name.clone();
    }
    Ok(())
}

fn unique_name(list: &ProfileList, base: &str) -> String {
    let mut n = 1;
    loop {
        let cand = if n == 1 && !base.starts_with("新しい") {
            format!("{base} のコピー")
        } else {
            format!("{base} {n}")
        };
        if list.get(&cand).is_none() {
            return cand;
        }
        n += 1;
    }
}

extern "system" fn main_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0) };
            let st = state_of::<State>(dlg);
            let modes: Vec<&str> = ProxyMode::ALL.iter().map(|m| m.label()).collect();
            combo_fill(dlg, ID_MODE, &modes, 0);
            cue(dlg, ID_HOST, "ホスト名か IP アドレス");
            cue(dlg, ID_PORT, "ポート");
            cue(dlg, ID_BYPASS, "例: *.example.co.jp; 192.168.0.0/16");
            cue(dlg, ID_PAC, "例: http://wpad.example.jp/proxy.pac");
            fill_profiles(dlg, st);
            show(dlg, st);
            1
        }
        WM_COMMAND => {
            let st = state_of::<State>(dlg);
            let id = (wparam.0 & 0xffff) as u16;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            match id {
                ID_PROFILE if code == CBN_SELCHANGE => {
                    store(dlg, st);
                    st.current = combo_sel(dlg, ID_PROFILE);
                    fill_profiles(dlg, st);
                    show(dlg, st);
                }
                ID_MODE | ID_KIND if code == CBN_SELCHANGE => enable_fields(dlg),
                ID_NEW | ID_COPY => {
                    store(dlg, st);
                    let p = if id == ID_COPY {
                        let mut p = st.list.profiles[st.current].clone();
                        p.name = unique_name(&st.list, &p.name);
                        p
                    } else {
                        ProxyProfile {
                            name: unique_name(&st.list, "新しいプロファイル"),
                            mode: ProxyMode::Manual,
                            server: "127.0.0.1:8080".into(),
                            bypass: "<local>".into(),
                            ..ProxyProfile::default()
                        }
                    };
                    st.list.profiles.push(p);
                    st.current = st.list.profiles.len() - 1;
                    fill_profiles(dlg, st);
                    show(dlg, st);
                    focus(dlg, ID_NAME);
                }
                ID_DELETE => {
                    if st.list.profiles.len() <= 1 {
                        crate::util::info_box(dlg, "最後のプロファイルは消せません。");
                    } else {
                        let name = st.list.profiles[st.current].name.clone();
                        let _ = st.list.remove(&name);
                        st.current = st.current.min(st.list.profiles.len() - 1);
                        fill_profiles(dlg, st);
                        show(dlg, st);
                    }
                }
                ID_RULE_ADD => {
                    if let Some(r) = edit_rule(dlg, None) {
                        let p = &mut st.list.profiles[st.current];
                        p.rules.push(r);
                        fill_rules(dlg, p, Some(p.rules.len() - 1));
                    }
                }
                ID_RULE_EDIT => edit_selected_rule(dlg, st),
                ID_RULES if code == LBN_DBLCLK => edit_selected_rule(dlg, st),
                ID_RULE_DEL => {
                    if let Some(i) = list_sel(dlg, ID_RULES) {
                        let p = &mut st.list.profiles[st.current];
                        p.rules.remove(i);
                        fill_rules(dlg, p, Some(i.min(p.rules.len().saturating_sub(1))));
                    }
                }
                ID_RULE_UP | ID_RULE_DOWN => {
                    if let Some(i) = list_sel(dlg, ID_RULES) {
                        let p = &mut st.list.profiles[st.current];
                        let j = if id == ID_RULE_UP {
                            i.checked_sub(1)
                        } else {
                            Some(i + 1)
                        };
                        if let Some(j) = j.filter(|j| *j < p.rules.len()) {
                            p.rules.swap(i, j);
                            fill_rules(dlg, p, Some(j));
                        }
                    }
                }
                ID_HOST_ADD => {
                    if let Some(m) = edit_host(dlg, None) {
                        let p = &mut st.list.profiles[st.current];
                        p.hosts.push(m);
                        fill_hosts(dlg, p, Some(p.hosts.len() - 1));
                    }
                }
                ID_HOST_EDIT => edit_selected_host(dlg, st),
                ID_HOSTS if code == LBN_DBLCLK => edit_selected_host(dlg, st),
                ID_HOST_DEL => {
                    if let Some(i) = list_sel(dlg, ID_HOSTS) {
                        let p = &mut st.list.profiles[st.current];
                        p.hosts.remove(i);
                        fill_hosts(dlg, p, Some(i.min(p.hosts.len().saturating_sub(1))));
                    }
                }
                IDOK_ => {
                    store(dlg, st);
                    match check_all(st) {
                        Ok(()) => {
                            st.done = true;
                            unsafe {
                                let _ = EndDialog(dlg, IDOK_ as isize);
                            }
                        }
                        Err(e) => {
                            fill_profiles(dlg, st);
                            show(dlg, st);
                            crate::util::error_box(dlg, &e);
                        }
                    }
                }
                IDCANCEL_ => unsafe {
                    let _ = EndDialog(dlg, IDCANCEL_ as isize);
                },
                _ => {}
            }
            1
        }
        _ => 0,
    }
}

fn edit_selected_rule(dlg: HWND, st: &mut State) {
    let Some(i) = list_sel(dlg, ID_RULES) else {
        return;
    };
    let old = st.list.profiles[st.current].rules[i].clone();
    if let Some(r) = edit_rule(dlg, Some(old)) {
        let p = &mut st.list.profiles[st.current];
        p.rules[i] = r;
        fill_rules(dlg, p, Some(i));
    }
}

fn edit_selected_host(dlg: HWND, st: &mut State) {
    let Some(i) = list_sel(dlg, ID_HOSTS) else {
        return;
    };
    let old = st.list.profiles[st.current].hosts[i].clone();
    if let Some(m) = edit_host(dlg, Some(old)) {
        let p = &mut st.list.profiles[st.current];
        p.hosts[i] = m;
        fill_hosts(dlg, p, Some(i));
    }
}

// ---- ドメインごとのプロキシ（1 つ） ---------------------------------------------------------

const R_TARGET: u16 = 200;
const R_VALUE: u16 = 201;
const R_ROUTE: u16 = 202;
const R_HOST: u16 = 203;
const R_PORT: u16 = 204;

struct RuleState {
    rule: Option<ProxyRule>,
    result: Option<ProxyRule>,
}

/// 規則を 1 つ足す・直す。
fn edit_rule(owner: HWND, rule: Option<ProxyRule>) -> Option<ProxyRule> {
    let mut t = T(Template::dialog("ドメインごとのプロキシ", 300, 116));
    t.label(7, 7, 50, "対象");
    t.combo(60, 7, 233, R_TARGET);
    t.label(7, 25, 50, "ドメイン");
    t.edit(60, 25, 233, R_VALUE, 0);
    t.label(7, 47, 50, "経路");
    t.combo(60, 47, 80, R_ROUTE);
    t.label(7, 65, 50, "プロキシ");
    t.edit(60, 65, 160, R_HOST, 0);
    t.label(223, 65, 6, ":");
    t.edit(231, 65, 62, R_PORT, ES_NUMBER as u32);
    t.ok_cancel(293, 95, "OK");
    let mut st = RuleState { rule, result: None };
    run(owner, t, Some(rule_proc), &mut st);
    st.result
}

fn rule_enable(dlg: HWND) {
    let proxy = combo_sel(dlg, R_ROUTE) > 0;
    enable(dlg, R_HOST, proxy);
    enable(dlg, R_PORT, proxy);
    let t = Target::ALL
        .get(combo_sel(dlg, R_TARGET))
        .copied()
        .unwrap_or(Target::DomainAndSubdomains);
    cue(dlg, R_VALUE, t.example());
}

extern "system" fn rule_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0) };
            let st = state_of::<RuleState>(dlg);
            let targets: Vec<&str> = Target::ALL.iter().map(|t| t.label()).collect();
            let mut routes = vec!["直接"];
            routes.extend(ProxyKind::ALL.iter().map(|k| k.label()));
            let (target, value, route, host, port) = match &st.rule {
                Some(r) => {
                    let (t, v) = split_pattern(&r.pattern);
                    let e = rule_route(r);
                    let route = e
                        .as_ref()
                        .and_then(|e| ProxyKind::ALL.iter().position(|k| *k == e.kind))
                        .map_or(0, |i| i + 1);
                    (
                        Target::ALL.iter().position(|x| *x == t).unwrap_or(0),
                        v,
                        route,
                        e.as_ref().map(|e| e.host.clone()).unwrap_or_default(),
                        e.map(|e| e.port.to_string()).unwrap_or_default(),
                    )
                }
                None => (0, String::new(), 1, String::new(), String::new()),
            };
            combo_fill(dlg, R_TARGET, &targets, target);
            combo_fill(dlg, R_ROUTE, &routes, route);
            set_text(dlg, R_VALUE, &value);
            set_text(dlg, R_HOST, &host);
            set_text(dlg, R_PORT, &port);
            cue(dlg, R_HOST, "ホスト名か IP アドレス");
            cue(dlg, R_PORT, "ポート");
            rule_enable(dlg);
            focus(dlg, R_VALUE);
            0
        }
        WM_COMMAND => {
            let st = state_of::<RuleState>(dlg);
            let id = (wparam.0 & 0xffff) as u16;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            match id {
                R_TARGET | R_ROUTE if code == CBN_SELCHANGE => rule_enable(dlg),
                IDOK_ => {
                    let target = Target::ALL[combo_sel(dlg, R_TARGET).min(Target::ALL.len() - 1)];
                    let value = get_text(dlg, R_VALUE);
                    if value.is_empty() {
                        crate::util::error_box(dlg, "対象のドメイン（か範囲）を入れてください。");
                        focus(dlg, R_VALUE);
                        return 1;
                    }
                    let route = combo_sel(dlg, R_ROUTE);
                    let proxy = if route == 0 {
                        "direct".to_owned()
                    } else {
                        let kind = ProxyKind::ALL[(route - 1).min(ProxyKind::ALL.len() - 1)];
                        let host = get_text(dlg, R_HOST);
                        if host.is_empty() {
                            crate::util::error_box(
                                dlg,
                                "プロキシのホスト名か IP アドレスを入れてください。",
                            );
                            focus(dlg, R_HOST);
                            return 1;
                        }
                        let port = match read_port(dlg, R_PORT) {
                            Ok(p) => p.unwrap_or(kind.default_port()),
                            Err(e) => {
                                crate::util::error_box(dlg, &e);
                                focus(dlg, R_PORT);
                                return 1;
                            }
                        };
                        Endpoint { kind, host, port }.to_spec()
                    };
                    let r = ProxyRule {
                        pattern: join_pattern(target, &value),
                        proxy,
                    };
                    match r.validate() {
                        Ok(()) => {
                            st.result = Some(r);
                            unsafe {
                                let _ = EndDialog(dlg, IDOK_ as isize);
                            }
                        }
                        Err(e) => crate::util::error_box(dlg, &e),
                    }
                }
                IDCANCEL_ => unsafe {
                    let _ = EndDialog(dlg, IDCANCEL_ as isize);
                },
                _ => {}
            }
            1
        }
        _ => 0,
    }
}

// ---- ホストの転送（1 つ） -------------------------------------------------------------------

const H_HOST: u16 = 300;
const H_PORTSEL: u16 = 301;
const H_PORT: u16 = 302;
const H_DEST: u16 = 303;
const H_DPORT: u16 = 304;
const H_CERT: u16 = 305;
const H_FP: u16 = 306;

/// 開発者用証明書のプルダウン。
const CERT_NONE: usize = 0;
const CERT_KEEP: usize = 1;
const CERT_NEW: usize = 2;
const CERT_FILE: usize = 3;

struct HostState {
    map: Option<HostMap>,
    /// 今の指紋（登録済み・ファイルから読んだもの）
    pinned: String,
    result: Option<HostMap>,
}

/// 転送を 1 つ足す・直す。
fn edit_host(owner: HWND, map: Option<HostMap>) -> Option<HostMap> {
    let mut t = T(Template::dialog("ホストの転送", 320, 154));
    t.label(7, 7, 62, "ホスト");
    t.edit(72, 7, 241, H_HOST, 0);
    t.label(7, 25, 62, "対象のポート");
    t.combo(72, 25, 100, H_PORTSEL);
    t.edit(176, 25, 60, H_PORT, ES_NUMBER as u32);
    t.label(7, 43, 62, "転送先");
    t.edit(72, 43, 150, H_DEST, 0);
    t.label(225, 43, 6, ":");
    t.edit(233, 43, 80, H_DPORT, ES_NUMBER as u32);
    t.label(7, 65, 64, "開発者用証明書");
    t.combo(72, 65, 241, H_CERT);
    // 指紋は長い（95 文字）ので 2 行で見せる
    t.0.item(
        WS_BORDER.0 | (ES_MULTILINE | ES_READONLY) as u32,
        72,
        83,
        241,
        20,
        H_FP,
        CLASS_EDIT,
        "",
    );
    t.label(
        72,
        106,
        241,
        "https のページで、この証明書のときだけエラーを許します",
    );
    t.ok_cancel(313, 133, "OK");
    let pinned = map.as_ref().and_then(|m| m.pinned()).unwrap_or_default();
    let mut st = HostState {
        map,
        pinned,
        result: None,
    };
    run(owner, t, Some(host_proc), &mut st);
    st.result
}

fn cert_items(st: &HostState) -> Vec<String> {
    vec![
        "使わない".into(),
        if st.pinned.is_empty() {
            "登録済みのもの（なし）".into()
        } else {
            "登録済みのものを使う".into()
        },
        "新しく作る（自己署名）".into(),
        "証明書ファイルから登録...".into(),
    ]
}

fn host_refresh(dlg: HWND, st: &HostState, sel: usize) {
    let items = cert_items(st);
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    combo_fill(dlg, H_CERT, &refs, sel);
    host_show_fp(dlg, st);
}

fn host_show_fp(dlg: HWND, st: &HostState) {
    let s = match combo_sel(dlg, H_CERT) {
        CERT_KEEP => {
            if st.pinned.is_empty() {
                String::new()
            } else {
                format!("SHA-256 {}", st.pinned)
            }
        }
        CERT_NEW => "保存するときに作ります（証明書と秘密鍵を書き出します）".into(),
        _ => String::new(),
    };
    set_text(dlg, H_FP, &s);
    let other = PortChoice::ALL.get(combo_sel(dlg, H_PORTSEL)).copied() == Some(PortChoice::Other);
    enable(dlg, H_PORT, other);
}

extern "system" fn host_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0) };
            let st = state_of::<HostState>(dlg);
            let ports: Vec<&str> = PortChoice::ALL.iter().map(|p| p.label()).collect();
            let (host, port, dest, dport) = match &st.map {
                Some(m) => split_host_map(m),
                None => (String::new(), None, "127.0.0.1".into(), None),
            };
            let choice = PortChoice::of(port);
            combo_fill(
                dlg,
                H_PORTSEL,
                &ports,
                PortChoice::ALL
                    .iter()
                    .position(|p| *p == choice)
                    .unwrap_or(0),
            );
            set_text(dlg, H_HOST, &host);
            set_text(
                dlg,
                H_PORT,
                &if choice == PortChoice::Other {
                    port.map(|p| p.to_string()).unwrap_or_default()
                } else {
                    String::new()
                },
            );
            set_text(dlg, H_DEST, &dest);
            set_text(
                dlg,
                H_DPORT,
                &dport.map(|p| p.to_string()).unwrap_or_default(),
            );
            cue(dlg, H_HOST, "例: www.example.com");
            cue(dlg, H_PORT, "ポート");
            cue(dlg, H_DEST, "IP アドレス（例: 127.0.0.1）");
            cue(dlg, H_DPORT, "元のまま");
            let sel = if st.pinned.is_empty() {
                CERT_NONE
            } else {
                CERT_KEEP
            };
            host_refresh(dlg, st, sel);
            focus(dlg, H_HOST);
            0
        }
        WM_COMMAND => {
            let st = state_of::<HostState>(dlg);
            let id = (wparam.0 & 0xffff) as u16;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            match id {
                H_PORTSEL if code == CBN_SELCHANGE => host_show_fp(dlg, st),
                H_CERT if code == CBN_SELCHANGE => match combo_sel(dlg, H_CERT) {
                    CERT_KEEP if st.pinned.is_empty() => host_refresh(dlg, st, CERT_NONE),
                    CERT_NEW if super::DEV_CERT.with(|d| d.get()).is_none() => {
                        crate::util::error_box(
                            dlg,
                            "このビルドの yybrowser では証明書を作れません。",
                        );
                        host_refresh(dlg, st, CERT_NONE);
                    }
                    CERT_FILE => {
                        if let Some(fp) = pick_cert_file(dlg) {
                            st.pinned = fp;
                            host_refresh(dlg, st, CERT_KEEP);
                        } else {
                            let sel = if st.pinned.is_empty() {
                                CERT_NONE
                            } else {
                                CERT_KEEP
                            };
                            host_refresh(dlg, st, sel);
                        }
                    }
                    _ => host_show_fp(dlg, st),
                },
                IDOK_ => {
                    if let Some(m) = build_host_map(dlg, st) {
                        st.result = Some(m);
                        unsafe {
                            let _ = EndDialog(dlg, IDOK_ as isize);
                        }
                    }
                }
                IDCANCEL_ => unsafe {
                    let _ = EndDialog(dlg, IDCANCEL_ as isize);
                },
                _ => {}
            }
            1
        }
        _ => 0,
    }
}

/// 欄から転送を作る（だめなら理由を出して `None`）。「新しく作る」なら証明書を作って書き出す。
fn build_host_map(dlg: HWND, st: &mut HostState) -> Option<HostMap> {
    let fail = |e: &str, id: u16| {
        crate::util::error_box(dlg, e);
        focus(dlg, id);
        None
    };
    let host = get_text(dlg, H_HOST);
    if host.is_empty() {
        return fail(
            "転送するホスト（例: www.example.com）を入れてください。",
            H_HOST,
        );
    }
    let choice = PortChoice::ALL[combo_sel(dlg, H_PORTSEL).min(PortChoice::ALL.len() - 1)];
    let other = match read_port(dlg, H_PORT) {
        Ok(p) => p,
        Err(e) => return fail(&e, H_PORT),
    };
    if choice == PortChoice::Other && other.is_none() {
        return fail("対象のポートを入れてください。", H_PORT);
    }
    let dest = get_text(dlg, H_DEST);
    if dest.is_empty() {
        return fail(
            "転送先の IP アドレス（例: 127.0.0.1）を入れてください。",
            H_DEST,
        );
    }
    let dport = match read_port(dlg, H_DPORT) {
        Ok(p) => p,
        Err(e) => return fail(&e, H_DPORT),
    };
    let mut m = HostMap {
        host: join_address(&host, choice.port(other)),
        address: join_address(&dest, dport),
        cert_sha256: String::new(),
    };
    if let Err(e) = m.validate() {
        return fail(&e, H_HOST);
    }
    match combo_sel(dlg, H_CERT) {
        CERT_KEEP => m.cert_sha256 = st.pinned.clone(),
        CERT_NEW => {
            let generate = super::DEV_CERT.with(|d| d.get())?;
            let cert = match generate(&m.host) {
                Ok(c) => c,
                Err(e) => return fail(&format!("証明書を作れません: {e}"), H_CERT),
            };
            let dir = super::dev_cert_dir();
            let (crt, key) = match yy_browser::rules::write_dev_cert(&dir, &m.host, &cert) {
                Ok(v) => v,
                Err(e) => {
                    return fail(
                        &format!("証明書を保存できません: {}: {e}", dir.display()),
                        H_CERT,
                    );
                }
            };
            m.cert_sha256 = cert.sha256.clone();
            crate::util::info_box(
                dlg,
                &format!(
                    "{host} の開発者用証明書を作りました。\n\n証明書: {}\n秘密鍵: {}\n\n\
                     転送先（{}）のサーバーに、この証明書と秘密鍵を設定してください。\n\
                     yybrowser は、サーバーがこの証明書を出したときだけ証明書のエラーを許し、\
                     アドレスバーに「開発者用証明書を利用中」と出します。\n\
                     OS の証明書ストアには入れません。",
                    crt.display(),
                    key.display(),
                    m.address
                ),
            );
        }
        _ => {}
    }
    Some(m)
}

/// 証明書のファイル（PEM・DER）を選んで指紋を返す。
fn pick_cert_file(dlg: HWND) -> Option<String> {
    let path = crate::fm::pick_file(
        dlg,
        (
            "証明書 (*.crt;*.pem;*.cer;*.der)",
            "*.crt;*.pem;*.cer;*.der",
        ),
        Some(&super::dev_cert_dir()),
        None,
    )?;
    let fp = std::fs::read(&path)
        .ok()
        .and_then(|b| yy_browser::rules::cert_fingerprint(&b));
    if fp.is_none() {
        crate::util::error_box(
            dlg,
            &format!("{} は証明書（PEM・DER）として読めません。", path.display()),
        );
    }
    fp
}
