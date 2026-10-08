//! 接続先のファイル・フォルダを、このパソコンのダウンロード フォルダにコピーする（エディタ・ターミナルの
//! ワークスペースの右クリック。11 章・12 章）。
//!
//! - 置き場所は Windows のダウンロード フォルダ（`FOLDERID_Downloads`。場所を変えていればその場所）。同じ名前が
//!   あれば、ブラウザと同じく `名前 (2).拡張子` にする（上書きしない）。
//! - エージェントを使う設定（エディタは常に）ならエージェント、でなければ SFTP（接続先に何も置かない）で読む。
//! - 始める前に量を数え、10 MB 以上なら確かめる（大きさの上限はない）。進み具合はステータスバーに出し、Esc で
//!   中止すると作りかけを消す。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO, MessageBoxW,
};
use windows::core::HSTRING;
use yy_remote::RemoteUri;
use yy_remote::transfer::{self, Loc};

/// この大きさ以上なら始める前に確かめる
const CONFIRM_BYTES: u64 = 10 << 20;

/// ダウンロード フォルダ。
pub(crate) fn downloads_dir() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
    .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")))
}

/// `dir` の中で使われていない名前（`name`、あれば `name (2).ext`・`name (3).ext` …）。
pub(crate) fn unique_target(dir: &Path, name: &str, is_dir: bool) -> PathBuf {
    let first = dir.join(name);
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !is_dir && !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or(first)
}

/// 接続先の `uri`（ファイルかフォルダ）をダウンロード フォルダにコピーする。結果の説明を返す（取り消せば
/// `None`）。待つ間はメッセージを処理するので、アプリの状態を借りていないところで呼ぶ。
pub(crate) fn download(hwnd: HWND, uri: &RemoteUri) -> Result<Option<String>, String> {
    let Some(dir) = downloads_dir() else {
        return Err("ダウンロード フォルダが分かりません。".into());
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}\n\n{e}", dir.display()))?;
    let target = uri.target();
    let show = crate::remote::show_status;
    let src = if crate::remote::use_agent() {
        Loc::Remote(crate::remote::session(&target, &show)?, uri.path.clone())
    } else {
        Loc::Sftp(crate::remote::sftp_fs(&target, &show)?, uri.path.clone())
    };
    let name = yy_proto::display_path(yy_proto::file_name(&uri.path));
    let name = if name.is_empty() {
        target.host.clone()
    } else {
        name
    };
    // 量を数える
    show(&format!("{name} の大きさを調べています…（Esc で中止）"));
    let s2 = src.clone();
    let measured = crate::remote::wait(&show, move |work| {
        transfer::measure(&s2, &mut |st| {
            work.report(format!(
                "大きさを調べています… {} 個のファイル、{}（Esc で中止）",
                crate::util::group_digits(st.files),
                crate::util::human_size(st.bytes)
            ));
            !work.cancelled()
        })
    });
    show("");
    let size = match measured {
        Ok(st) => st,
        Err(e) if transfer::is_cancelled(&e) => {
            return Ok(Some("ダウンロードを中止しました".into()));
        }
        Err(e) => return Err(format!("{uri}\n\n{e}")),
    };
    let is_dir = size.dirs > 0;
    if size.bytes >= CONFIRM_BYTES {
        let files = if size.files > 1 {
            format!("（{} 個のファイル）", crate::util::group_digits(size.files))
        } else {
            String::new()
        };
        let text = format!(
            "「{name}」は {}{files} あります。\n\nダウンロード フォルダ（{}）にコピーしますか？",
            crate::util::human_size(size.bytes),
            dir.display()
        );
        let r = unsafe {
            MessageBoxW(
                Some(hwnd),
                &HSTRING::from(text),
                &HSTRING::from("ダウンロード"),
                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
            )
        };
        if r != IDYES {
            return Ok(None);
        }
    }
    let dest = unique_target(&dir, &name, is_dir);
    show(&format!("{name} をダウンロードしています…（Esc で中止）"));
    let d2 = Loc::Local(dest.clone());
    let total = size.bytes;
    let r = crate::remote::wait(&show, move |work| {
        transfer::copy(&src, &d2, &mut |st| {
            let pct = (st.bytes * 100)
                .checked_div(total)
                .map(|p| format!("{p}%、"))
                .unwrap_or_default();
            work.report(format!(
                "ダウンロードしています… {pct}{} 個のファイル、{}（Esc で中止）",
                crate::util::group_digits(st.files),
                crate::util::human_size(st.bytes)
            ));
            !work.cancelled()
        })
    });
    show("");
    match r {
        Ok(st) => {
            let mut m = format!("ダウンロード フォルダにコピーしました: {}", dest.display());
            if st.skipped > 0 {
                m.push_str(&format!(
                    "（フォルダを指すリンクなど {} 個は飛ばしました）",
                    crate::util::group_digits(st.skipped)
                ));
            }
            Ok(Some(m))
        }
        Err(e) if transfer::is_cancelled(&e) => Ok(Some(
            "ダウンロードを中止しました（作りかけは消しました）".into(),
        )),
        Err(e) => Err(format!("{uri} をダウンロードできませんでした。\n\n{e}")),
    }
}
