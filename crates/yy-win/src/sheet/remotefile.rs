//! 接続先（SSH）のファイルを開く・保存する（15 章 12.2.1）。
//!
//! - 開く: 接続先のファイルを手元の一時フォルダ（`%TEMP%\yysheet-remote-…\名前`）に取り寄せ、その写しを
//!   開く（CSV・`.yys`・固定長の見分け方は手元のファイルと同じ）。
//! - 保存: 写しに書いてから接続先に送る。接続先では同じフォルダに一時の名前で書いてから置き換えるので、
//!   途中で切れても元のファイルは残る。開いた（前に送った）あとで接続先のファイルが変わっていれば、
//!   置き換えてよいかを確かめる。送れなければ、写しには保存したまま未保存の印を残す。
//! - 読み書きは、エージェントを使う設定（`[sheet] use_agent`・ワークスペース メニュー）ならエージェント、
//!   でなければ SFTP（接続先に何も置かない）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO, MessageBoxW,
};
use windows::core::HSTRING;
use yy_remote::transfer::{self, Loc};
use yy_remote::{FileId, RemoteUri};

use super::{confirm_discard, fixedui, multiui, open_path, set_status, with};
use crate::util::error_box;

/// 取り寄せたファイル（手元の写しのパス → 接続先）。
pub(super) struct Link {
    pub uri: RemoteUri,
    /// 取り寄せた（前に送った）ときの接続先のファイル（ほかで変更されたかを見分ける）
    id: Option<FileId>,
}

/// 開き方。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum How {
    /// 開く（`.yys`・CSV、それ以外は固定長として）
    Open,
    /// 固定長ファイル（マルチレイアウト）として開く
    OpenMulti,
    /// 固定長ファイルを今の文書にシートとして追加する
    AddFixed,
    /// 固定長ファイル（マルチレイアウト）を今の文書にシートとして追加する
    AddMulti,
}

/// 一時フォルダの名前の始め。
const CACHE_PREFIX: &str = "yysheet-remote-";

static CACHE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `path` が `ssh://` の場所なら、その接続先。
pub(super) fn remote_uri(path: &Path) -> Option<RemoteUri> {
    crate::wsbar::remote_uri(path)
}

/// 手元のファイルか接続先のファイル（`ssh://…`）を開く。`confirm` なら、今の文書を置き換える前に
/// 変更を保存するかを尋ねる。
pub(super) fn open_item(path: &Path, how: How, confirm: bool) {
    let replaces = matches!(how, How::Open | How::OpenMulti);
    if confirm && replaces && !confirm_discard() {
        return;
    }
    let Some((frame, ctx)) = with(|a| (a.frame, a.ctx.clone())) else {
        return;
    };
    let local = match remote_uri(path) {
        None => path.to_owned(),
        Some(uri) => match fetch(&uri) {
            Ok(Some(p)) => {
                crate::remote::set_last(uri);
                p
            }
            Ok(None) => return,
            Err(e) => {
                error_box(frame, &e);
                return;
            }
        },
    };
    match how {
        How::Open => open_path(&local),
        How::OpenMulti => multiui::open_multi_path(frame, ctx, &local, false),
        How::AddFixed => fixedui::open_fixed_path(frame, ctx, &local, true),
        How::AddMulti => multiui::open_multi_path(frame, ctx, &local, true),
    }
}

/// 読み書きする場所（エージェントか SFTP）。
fn loc(uri: &RemoteUri) -> Result<Loc, String> {
    let target = uri.target();
    let show = crate::remote::show_status;
    Ok(if crate::remote::use_agent() {
        Loc::Remote(crate::remote::session(&target, &show)?, uri.path.clone())
    } else {
        Loc::Sftp(crate::remote::sftp_fs(&target, &show)?, uri.path.clone())
    })
}

/// Windows のファイル名に使えない文字を `_` にする。
fn local_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "remote".into() } else { s }
}

/// 接続先のファイルを手元の一時フォルダに取り寄せ、そのパスを返す（中止したら `None`）。
fn fetch(uri: &RemoteUri) -> Result<Option<PathBuf>, String> {
    let src = loc(uri)?;
    let s2 = src.clone();
    let info = crate::remote::wait(&set_status, move |_| transfer::info(&s2))
        .map_err(|e| format!("{uri}\n\n{e}"))?
        .ok_or_else(|| format!("{uri}\n\nファイルが見つかりません。"))?;
    if info.is_dir() {
        return Err(format!("{uri}\n\nフォルダは開けません。"));
    }
    let n = CACHE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{CACHE_PREFIX}{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}\n\n{e}", dir.display()))?;
    let name = local_name(&yy_proto::display_path(yy_proto::file_name(&uri.path)));
    let dest = dir.join(name);
    let total = info.len();
    let d2 = Loc::Local(dest.clone());
    set_status(&format!("{uri} を取り寄せています…（Esc で中止）"));
    let r = crate::remote::wait(&set_status, move |work| {
        transfer::copy(&src, &d2, &mut |st| {
            let pct = (st.bytes * 100)
                .checked_div(total)
                .map(|p| format!("{p}%、"))
                .unwrap_or_default();
            work.report(format!(
                "取り寄せています… {pct}{}（Esc で中止）",
                crate::util::human_size(st.bytes)
            ));
            !work.cancelled()
        })
    });
    set_status("");
    if let Err(e) = r {
        let _ = std::fs::remove_dir_all(&dir);
        if transfer::is_cancelled(&e) {
            set_status("開くのを中止しました");
            return Ok(None);
        }
        return Err(format!("{uri} を取り寄せられませんでした。\n\n{e}"));
    }
    with(|a| {
        a.remote_files.insert(
            dest.clone(),
            Link {
                uri: uri.clone(),
                id: Some(info.id),
            },
        );
    });
    Ok(Some(dest))
}

/// 手元の写し `local` の接続先（取り寄せたファイルでなければ `None`）。
pub(super) fn uri_of(local: &Path) -> Option<RemoteUri> {
    with(|a| a.remote_files.get(local).map(|l| l.uri.clone())).flatten()
}

/// 表示用の場所（取り寄せたファイルなら接続先の `ssh://…`）。
pub(super) fn shown(path: &Path) -> String {
    match uri_of(path) {
        Some(u) => u.to_string(),
        None => path.display().to_string(),
    }
}

/// 書いた `local` が取り寄せたファイルの写しなら、接続先に送る。送らなくてよいか送れたら `true`。
/// 送れなければ（取りやめを含む）知らせて `false`（写しには保存してある）。
pub(super) fn push(owner: HWND, local: &Path) -> bool {
    let Some((uri, id)) =
        with(|a| a.remote_files.get(local).map(|l| (l.uri.clone(), l.id))).flatten()
    else {
        return true;
    };
    let dest = match loc(&uri) {
        Ok(d) => d,
        Err(e) => {
            error_box(owner, &not_sent(&uri, local, &e));
            return false;
        }
    };
    // 開いたあとで、ほかで変更・削除されていないか
    let d2 = dest.clone();
    let now = match crate::remote::wait(&set_status, move |_| transfer::info(&d2)) {
        Ok(i) => i.map(|i| i.id),
        Err(e) => {
            error_box(owner, &not_sent(&uri, local, &e.to_string()));
            return false;
        }
    };
    if now != id {
        let what = if now.is_none() {
            "は、開いたあとで（ほかの人やプログラムによって）削除されています。作り直しますか？"
        } else {
            "は、開いたあとで（ほかの人やプログラムによって）変更されています。置き換えますか？"
        };
        let text = format!(
            "{uri}\n\n{what}\n\nいいえ: 送らずに、手元の写し（{}）にだけ保存したままにします。",
            local.display()
        );
        let r = unsafe {
            MessageBoxW(
                Some(owner),
                &HSTRING::from(text),
                &HSTRING::from("yysheet"),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        if r != IDYES {
            set_status("接続先には送りませんでした");
            return false;
        }
    }
    let total = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
    let src = local.to_owned();
    let d2 = dest.clone();
    set_status(&format!("{uri} に送っています…"));
    let r = crate::remote::wait(&set_status, move |work| {
        transfer::replace(&src, &d2, &mut |st| {
            let pct = (st.bytes * 100)
                .checked_div(total)
                .map(|p| format!("{p}%、"))
                .unwrap_or_default();
            work.report(format!(
                "接続先に送っています… {pct}{}（Esc で中止）",
                crate::util::human_size(st.bytes)
            ));
            !work.cancelled()
        })?;
        transfer::info(&d2)
    });
    set_status("");
    match r {
        Ok(info) => {
            with(|a| {
                if let Some(l) = a.remote_files.get_mut(local) {
                    l.id = info.map(|i| i.id);
                }
            });
            true
        }
        Err(e) => {
            let e = if transfer::is_cancelled(&e) {
                "送るのを中止しました（接続先のファイルは元のままです）。".to_string()
            } else {
                e.to_string()
            };
            error_box(owner, &not_sent(&uri, local, &e));
            false
        }
    }
}

fn not_sent(uri: &RemoteUri, local: &Path, why: &str) -> String {
    format!(
        "{uri} に保存できませんでした。\n\n{why}\n\n手元の写し（{}）には保存してあります。もう一度上書き保存すると、送り直します。",
        local.display()
    )
}

/// 使わなくなった写し（今の文書・固定長のシートが指していないもの）を消す。
pub(super) fn forget_unused() {
    let unused: Vec<PathBuf> = with(|a| {
        let keep: Vec<&Path> = a
            .doc
            .path
            .iter()
            .chain(a.fixed_paths.values())
            .map(PathBuf::as_path)
            .collect();
        let gone: Vec<PathBuf> = a
            .remote_files
            .keys()
            .filter(|p| !keep.contains(&p.as_path()))
            .cloned()
            .collect();
        for p in &gone {
            a.remote_files.remove(p);
        }
        gone
    })
    .unwrap_or_default();
    for p in unused {
        remove_cache(&p);
    }
}

/// 写しをすべて消す（終わるとき）。
pub(super) fn forget_all(files: impl IntoIterator<Item = PathBuf>) {
    for p in files {
        remove_cache(&p);
    }
}

fn remove_cache(local: &Path) {
    if let Some(dir) = local.parent()
        && dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(CACHE_PREFIX))
    {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 以前の異常終了で残った写しのうち、1 日以上前のものを消す。
pub(super) fn remove_stale() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with(CACHE_PREFIX) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_names_are_valid_on_windows() {
        assert_eq!(local_name("売上.csv"), "売上.csv");
        assert_eq!(local_name("a:b*c?.dat"), "a_b_c_.dat");
        assert_eq!(local_name("x. "), "x");
        assert_eq!(local_name(""), "remote");
    }
}
