//! 接続先のファイル・フォルダを、このパソコンのダウンロード フォルダにコピーする（エディタ・ターミナルの
//! ワークスペースの右クリック。11 章・12 章）。
//!
//! - 置き場所は Windows のダウンロード フォルダ（`FOLDERID_Downloads`。場所を変えていればその場所）。同じ名前が
//!   あれば、ブラウザと同じく `名前 (2).拡張子` にする（上書きしない）。
//! - 共通のホスト別設定に従い、エージェントまたはSFTP／SCPで読む。
//! - 始める前に量を数え、10 MB 以上なら確かめる（大きさの上限はない）。進み具合はステータスバーに出し、Esc で
//!   中止すると作りかけを消す。
//!
//! 「zip にしてダウンロード」（[`zip_download`]）は、接続先のエージェント（Linux）にフォルダを zip にさせて
//! 1 つのファイルとして受け取る。保存先は毎回選ぶ。zip が 100 MB を超えるときは受け取らず、yysftp の転送の
//! 一覧にフォルダのダウンロードとして入れる（保存先のフォルダを毎回選ぶ）。

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
/// 「zip にしてダウンロード」で受け取る zip の大きさの上限（超えたら yysftp に任せる）
pub(crate) const ZIP_LIMIT: u64 = 100 << 20;

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
    let show = crate::remote::show_status;
    let src = crate::remote::file_location(uri, &show)?;
    let name = local_name(uri)?;
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

/// 接続先の名前（フォルダの名前。ルートなら接続先の名前）を Windows のファイル名にしたもの。
fn local_name(uri: &RemoteUri) -> Result<String, String> {
    let name = yy_proto::display_path(yy_proto::file_name(&uri.path));
    let name = if name.is_empty() {
        uri.host.clone()
    } else {
        name
    };
    Ok(transfer::local_file_name(name.as_bytes())
        .map_err(|e| format!("{uri}\n\n{e}"))?
        .to_string_lossy()
        .into_owned())
}

fn interrupted() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "中止しました")
}

/// 接続先のフォルダ `uri` を zip にしてダウンロードする（保存先は毎回選ぶ）。zip が [`ZIP_LIMIT`] を超える
/// ときは受け取らず、yysftp の転送の一覧に入れる。結果の説明を返す（取り消せば `None`）。
/// 待つ間はメッセージを処理するので、アプリの状態を借りていないところで呼ぶ。
pub(crate) fn zip_download(hwnd: HWND, uri: &RemoteUri) -> Result<Option<String>, String> {
    use yy_remote::ZipState;
    let target = uri.target();
    let show = crate::remote::show_status;
    if !crate::remote::agent_for(&target) {
        return queue_in_sftp(
            hwnd,
            uri,
            &local_name(uri)?,
            "接続先の共通設定でエージェントを使わないため、選択された転送方式でフォルダを転送します。",
        );
    }
    let session = crate::remote::session(&target, &show)?;
    let name = local_name(uri)?;
    show(&format!("{name} を zip にしています…（Esc で中止）"));
    let s2 = session.clone();
    let path = uri.path.clone();
    let offset = crate::fm::tz_offset() as i32;
    let zipped = crate::remote::wait(&show, move |work| {
        let mut p = s2.zip_begin(&path, ZIP_LIMIT, offset)?;
        while p.state == ZipState::Running {
            if work.cancelled() {
                let _ = s2.zip_end(p.job);
                return Err(interrupted());
            }
            let pct = (p.read * 100)
                .checked_div(p.total_bytes)
                .map(|p| format!("{p}%、"))
                .unwrap_or_default();
            work.report(format!(
                "zip にしています… {pct}{} / {} 個のファイル、zip {}（Esc で中止）",
                crate::util::group_digits(p.files),
                crate::util::group_digits(p.total_files),
                crate::util::human_size(p.written)
            ));
            p = s2.zip_step(p.job)?;
        }
        Ok(p)
    });
    show("");
    let p = match zipped {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
            return Ok(Some("zip にするのを中止しました".into()));
        }
        Err(e) => return Err(format!("{uri} を zip にできませんでした。\n\n{e}")),
    };
    let end = |job: u32| {
        let s = session.clone();
        crate::remote::wait(&show, move |_| s.zip_end(job))
    };
    if let ZipState::TooLarge(why) = &p.state {
        let _ = end(p.job);
        return queue_in_sftp(hwnd, uri, &name, why);
    }
    // 保存先を選ぶ（毎回）
    let default = format!("{name}.zip");
    let Some(dest) = crate::fm::pick_file(
        hwnd,
        ("zip ファイル (*.zip)", "*.zip"),
        downloads_dir().as_deref(),
        Some(&default),
    ) else {
        let _ = end(p.job);
        return Ok(None);
    };
    show(&format!(
        "{name}.zip をダウンロードしています…（Esc で中止）"
    ));
    let s2 = session.clone();
    let (temp, job, total) = (p.temp.clone(), p.job, p.written);
    let d2 = dest.clone();
    let got = crate::remote::wait(&show, move |work| {
        let r = receive(&s2, &temp, &d2, total, work);
        let _ = s2.zip_end(job);
        r
    });
    show("");
    match got {
        Ok(()) => {
            let mut m = format!(
                "zip にしてダウンロードしました: {}（{}、{} 個のファイル）",
                dest.display(),
                crate::util::human_size(total),
                crate::util::group_digits(p.files)
            );
            if p.skipped > 0 {
                m.push_str(&format!(
                    "。読めなかった {} 個は入れていません",
                    crate::util::group_digits(p.skipped)
                ));
            }
            Ok(Some(m))
        }
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(Some(
            "ダウンロードを中止しました（作りかけは消しました）".into(),
        )),
        Err(e) => Err(format!(
            "{} をダウンロードできませんでした。\n\n{e}",
            dest.display()
        )),
    }
}

/// 接続先の zip（一時ファイル）を `dest` に受け取る（作りかけは `.yypart` に書き、終わったら置き換える）。
fn receive(
    session: &yy_remote::Session,
    temp: &[u8],
    dest: &Path,
    total: u64,
    work: &crate::remote::Work,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut part = dest.as_os_str().to_owned();
    part.push(".yypart");
    let part = PathBuf::from(part);
    let r = (|| {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&part)?);
        session.download(temp, &mut out, &mut |done, _| {
            let pct = (done * 100)
                .checked_div(total)
                .map(|p| format!("{p}%、"))
                .unwrap_or_default();
            work.report(format!(
                "zip をダウンロードしています… {pct}{} / {}（Esc で中止）",
                crate::util::human_size(done),
                crate::util::human_size(total)
            ));
            !work.cancelled()
        })?;
        out.flush()?;
        drop(out);
        // 保存の画面で上書きを確かめてある
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        std::fs::rename(&part, dest)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r
}

/// zip が大きすぎたフォルダを、yysftp の転送の一覧にダウンロードとして入れる（保存先のフォルダを選ぶ）。
fn queue_in_sftp(
    hwnd: HWND,
    uri: &RemoteUri,
    name: &str,
    why: &str,
) -> Result<Option<String>, String> {
    use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONINFORMATION, MB_OKCANCEL};
    let text = format!(
        "「{name}」の zip 作成をここでは利用できません。\n（{why}、zip の上限: {}）\n\n\
         yysftp の転送の一覧に、フォルダのダウンロードとして入れます。次の画面で保存先のフォルダを\
         選んでください（その中に「{name}」フォルダを作ります）。",
        crate::util::human_size(ZIP_LIMIT)
    );
    let r = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from("zip にしてダウンロード"),
            MB_OKCANCEL | MB_ICONINFORMATION,
        )
    };
    if r != IDOK {
        return Ok(None);
    }
    let Some(dir) = crate::grepdlg::browse_folder(hwnd) else {
        return Ok(None);
    };
    let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("yysftp.exe")))
        .filter(|p| p.is_file())
    else {
        return Err("yysftp.exe が同じフォルダにありません。".into());
    };
    std::process::Command::new(exe)
        .arg("--download")
        .arg(uri.to_string())
        .arg("--to")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("yysftp を起動できませんでした。\n{e}"))?;
    Ok(Some(format!(
        "zip が {} を超えるため、yysftp の転送の一覧に入れました（保存先 {}）",
        crate::util::human_size(ZIP_LIMIT),
        dir.display()
    )))
}
