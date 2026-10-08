//! 端末の文字列のリンク（12 章）。URL はブラウザで、ファイルのパスはエディタ（yyeditor）で開き、
//! フォルダは手元ならエクスプローラー・接続先なら yysftp で開く。接続先のタブの作業フォルダを
//! yysftp で開く。
//!
//! 相対パス・`~` は作業フォルダから解決する。作業フォルダは、シェルが知らせたもの（OSC 7・OSC 9;9）、
//! なければ上のプロンプトの行（`user@host:~/dir$`・`PS C:\dir>`・`C:\dir>`）、なければタブを開いた
//! フォルダ、接続先ならホーム。接続先のパスは接続先で `test -e` を使って確かめてから開く。

use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::core::HSTRING;
use yy_remote::RemoteUri;
use yy_remote::uri::Target;
use yy_term::LinkTarget;

use super::Place;
use crate::util::error_box;

/// リンクを開くのに要ること（状態を借りている間に集める）。
#[derive(Clone)]
pub(super) struct LinkContext {
    pub place: Place,
    /// シェルが知らせた作業フォルダ
    pub cwd: Option<String>,
    /// 上のプロンプトの行から読んだ作業フォルダ（`~` で始まることがある）
    pub prompt: Option<String>,
    /// 接続先のホーム（分かっていれば）
    pub home: Option<Vec<u8>>,
}

/// 開いた結果（ステータスバーに出す）と、分かった接続先のホーム（タブに覚える）。
pub(super) struct Opened {
    pub message: String,
    pub home: Option<Vec<u8>>,
}

/// 実行ファイルと同じフォルダのアプリ。
fn sibling(name: &str) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|d| d.join(name))
        .filter(|p| p.is_file())
}

/// URL をブラウザ（既定のアプリ）で開く。
pub(super) fn open_url(hwnd: HWND, url: &str) -> String {
    let r = unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            Some(hwnd),
            &HSTRING::from("open"),
            &HSTRING::from(url),
            None,
            None,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    if r.0 as isize > 32 {
        format!("ブラウザで開きました: {url}")
    } else {
        error_box(hwnd, &format!("{url} を開けませんでした。"));
        String::new()
    }
}

/// エディタ（yyeditor）でファイルを開く（`target` は手元のパスか `ssh://…`）。
fn launch_editor(hwnd: HWND, target: &str, line: Option<u32>) -> bool {
    let Some(exe) = sibling("yyeditor.exe") else {
        error_box(
            hwnd,
            "yyeditor.exe が yyterm.exe と同じフォルダにありません。",
        );
        return false;
    };
    let mut c = std::process::Command::new(exe);
    if let Some(l) = line {
        c.arg("--line").arg(l.to_string());
    }
    c.arg(target);
    match c.spawn() {
        Ok(_) => true,
        Err(e) => {
            error_box(hwnd, &format!("yyeditor を起動できませんでした。\n{e}"));
            false
        }
    }
}

/// yysftp で接続先のフォルダを開く。
pub(super) fn launch_sftp(hwnd: HWND, target: &Target, dir: &[u8]) -> bool {
    let Some(exe) = sibling("yysftp.exe") else {
        error_box(
            hwnd,
            "yysftp.exe が yyterm.exe と同じフォルダにありません。",
        );
        return false;
    };
    let uri = RemoteUri {
        user: target.user.clone(),
        host: target.host.clone(),
        port: target.port,
        path: dir.to_vec(),
    };
    match std::process::Command::new(exe).arg(uri.to_string()).spawn() {
        Ok(_) => true,
        Err(e) => {
            error_box(hwnd, &format!("yysftp を起動できませんでした。\n{e}"));
            false
        }
    }
}

/// 手元のパスの作業フォルダの候補（Windows のパスにする）。
fn local_bases(ctx: &LinkContext) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut add = |p: &str| {
        let p = p.replace('/', "\\");
        let pb = PathBuf::from(&p);
        if pb.is_absolute() && !v.contains(&pb) {
            v.push(pb);
        }
    };
    if let Some(c) = &ctx.cwd {
        add(c);
    }
    if let Some(p) = &ctx.prompt {
        add(p);
    }
    if let Place::Local(Some(d)) = &ctx.place {
        add(&d.to_string_lossy());
    }
    if let Ok(d) = std::env::current_dir() {
        add(&d.to_string_lossy());
    }
    v
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// 手元のパスを解決する（あるものだけ）。
fn resolve_local(path: &str, ctx: &LinkContext) -> Option<PathBuf> {
    let expanded = if let Some(rest) = path.strip_prefix("~/").or(path.strip_prefix("~\\")) {
        home_dir()?.join(rest.replace('/', "\\"))
    } else {
        PathBuf::from(path.replace('/', "\\"))
    };
    if expanded.is_absolute() || path.starts_with("\\\\") {
        return expanded.exists().then_some(expanded);
    }
    local_bases(ctx)
        .into_iter()
        .map(|b| b.join(&expanded))
        .find(|p| p.exists())
}

/// 接続先の `~` を展開する。
fn expand_home(p: &[u8], home: &[u8]) -> Vec<u8> {
    if p == b"~" {
        return home.to_vec();
    }
    match p.strip_prefix(b"~/") {
        Some(rest) => yy_proto::join_path(home, rest),
        None => p.to_vec(),
    }
}

/// 接続先のホーム（`$HOME`）。
fn remote_home(t: &dyn yy_remote::Transport) -> Option<Vec<u8>> {
    let o = yy_remote::run(t, b"printf '%s' \"$HOME\"", b"").ok()?;
    (o.success() && o.stdout.starts_with(b"/")).then_some(o.stdout)
}

/// 接続先の作業フォルダ（シェルが知らせたもの → プロンプト → タブを開いたフォルダ → ホーム）。
fn remote_cwd(ctx: &LinkContext, home: &[u8]) -> Vec<u8> {
    let cands = [
        ctx.cwd.as_ref().map(|c| c.as_bytes().to_vec()),
        ctx.prompt.as_ref().map(|p| expand_home(p.as_bytes(), home)),
        match &ctx.place {
            Place::Remote { dir: Some(d), .. } => Some(expand_home(d, home)),
            _ => None,
        },
    ];
    cands
        .into_iter()
        .flatten()
        .find(|c| c.starts_with(b"/"))
        .unwrap_or_else(|| home.to_vec())
}

/// 接続先でパスの候補を確かめる（最初にあるもの。フォルダなら `true`）。
fn remote_exists(t: &dyn yy_remote::Transport, cands: &[Vec<u8>]) -> Option<(Vec<u8>, bool)> {
    let mut cmd = b"for p in".to_vec();
    for c in cands {
        cmd.push(b' ');
        cmd.extend(yy_git::shell_quote(c));
    }
    cmd.extend_from_slice(
        b"; do if [ -d \"$p\" ]; then printf 'd%s' \"$p\"; exit 0; elif [ -e \"$p\" ]; then printf 'f%s' \"$p\"; exit 0; fi; done; exit 1",
    );
    let o = yy_remote::run(t, &cmd, b"").ok()?;
    if !o.success() {
        return None;
    }
    let (kind, path) = o.stdout.split_first()?;
    Some((path.to_vec(), *kind == b'd'))
}

/// ファイルのパスのリンクを開く（ファイルはエディタ、フォルダはエクスプローラー・yysftp）。
pub(super) fn open_path(hwnd: HWND, path: &str, line: Option<u32>, ctx: &LinkContext) -> Opened {
    match &ctx.place {
        Place::Remote { target, .. } => open_remote_path(hwnd, target, path, line, ctx),
        _ => {
            let message = match resolve_local(path, ctx) {
                Some(p) if p.is_dir() => match crate::app::workspacemode::open_explorer(&p) {
                    Ok(()) => format!("エクスプローラーで開きました: {}", p.display()),
                    Err(e) => {
                        error_box(hwnd, &e);
                        String::new()
                    }
                },
                Some(p) => {
                    if launch_editor(hwnd, &p.to_string_lossy(), line) {
                        format!("エディタで開きました: {}", p.display())
                    } else {
                        String::new()
                    }
                }
                None => format!(
                    "{path} が見つかりません（作業フォルダ: {}）",
                    local_bases(ctx)
                        .first()
                        .map(|b| b.display().to_string())
                        .unwrap_or_else(|| "不明".into())
                ),
            };
            Opened {
                message,
                home: None,
            }
        }
    }
}

fn open_remote_path(
    hwnd: HWND,
    target: &Target,
    path: &str,
    line: Option<u32>,
    ctx: &LinkContext,
) -> Opened {
    let none = |message: String| Opened {
        message,
        home: None,
    };
    // Windows のパスは接続先にはない
    if path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\") {
        return none(format!("{path} は接続先のパスではありません"));
    }
    let Some(t) = crate::remote::live_transport(target) else {
        return none(format!("{target} への接続が切れています"));
    };
    let home = ctx.home.clone().or_else(|| remote_home(t.as_ref()));
    let Some(h) = home.clone() else {
        return none(format!("{target} のホームが分かりません"));
    };
    let p = expand_home(path.as_bytes(), &h);
    let cands: Vec<Vec<u8>> = if p.starts_with(b"/") {
        vec![p]
    } else {
        let mut v = Vec::new();
        for base in [
            ctx.cwd.as_ref().map(|c| c.as_bytes().to_vec()),
            ctx.prompt.as_ref().map(|c| expand_home(c.as_bytes(), &h)),
            match &ctx.place {
                Place::Remote { dir: Some(d), .. } => Some(expand_home(d, &h)),
                _ => None,
            },
            Some(h.clone()),
        ]
        .into_iter()
        .flatten()
        .filter(|b| b.starts_with(b"/"))
        {
            let c = yy_proto::join_path(&base, &p);
            if !v.contains(&c) {
                v.push(c);
            }
        }
        v
    };
    let message = match remote_exists(t.as_ref(), &cands) {
        Some((dir, true)) => {
            if launch_sftp(hwnd, target, &dir) {
                format!("yysftp で開きました: {}", String::from_utf8_lossy(&dir))
            } else {
                String::new()
            }
        }
        Some((file, false)) => {
            let uri = RemoteUri {
                user: target.user.clone(),
                host: target.host.clone(),
                port: target.port,
                path: file.clone(),
            };
            if launch_editor(hwnd, &uri.to_string(), line) {
                format!("エディタで開きました: {}", String::from_utf8_lossy(&file))
            } else {
                String::new()
            }
        }
        None => format!(
            "{path} が {target} に見つかりません（作業フォルダ: {}）",
            String::from_utf8_lossy(&remote_cwd(ctx, &h))
        ),
    };
    Opened { message, home }
}

/// リンクを開く。
pub(super) fn open(hwnd: HWND, target: &LinkTarget, ctx: &LinkContext) -> Opened {
    match target {
        LinkTarget::Url(u) => Opened {
            message: open_url(hwnd, u),
            home: None,
        },
        LinkTarget::Path { path, line, .. } => open_path(hwnd, path, *line, ctx),
    }
}

/// リンクの文字列（コピー用）。
pub(super) fn text_of(target: &LinkTarget) -> String {
    match target {
        LinkTarget::Url(u) => u.clone(),
        LinkTarget::Path { path, line, col } => match (line, col) {
            (Some(l), Some(c)) => format!("{path}:{l}:{c}"),
            (Some(l), None) => format!("{path}:{l}"),
            _ => path.clone(),
        },
    }
}

/// 接続先のタブの作業フォルダを yysftp で開く。
pub(super) fn sftp_here(hwnd: HWND, ctx: &LinkContext) -> Opened {
    let Place::Remote { target, .. } = &ctx.place else {
        return Opened {
            message: String::new(),
            home: None,
        };
    };
    let Some(t) = crate::remote::live_transport(target) else {
        return Opened {
            message: format!("{target} への接続が切れています"),
            home: None,
        };
    };
    let home = ctx.home.clone().or_else(|| remote_home(t.as_ref()));
    let dir = remote_cwd(ctx, home.as_deref().unwrap_or(b"/"));
    let message = if launch_sftp(hwnd, target, &dir) {
        format!(
            "yysftp で開きました: {}:{}",
            target,
            String::from_utf8_lossy(&dir)
        )
    } else {
        String::new()
    };
    Opened { message, home }
}

/// 手元のタブの作業フォルダをエクスプローラーで開く。
pub(super) fn explorer_here(hwnd: HWND, ctx: &LinkContext) -> String {
    let Some(dir) = local_bases(ctx).into_iter().find(|d| d.is_dir()) else {
        return "作業フォルダが分かりません".into();
    };
    match crate::app::workspacemode::open_explorer(&dir) {
        Ok(()) => format!("エクスプローラーで開きました: {}", dir.display()),
        Err(e) => {
            error_box(hwnd, &e);
            String::new()
        }
    }
}

/// 接続先の作業フォルダの説明（メニューに出す。ホームが分からなければ `~` のまま）。
pub(super) fn remote_cwd_label(ctx: &LinkContext) -> String {
    let home = ctx.home.clone().unwrap_or_else(|| b"~".to_vec());
    String::from_utf8_lossy(&remote_cwd(ctx, &home)).into_owned()
}
