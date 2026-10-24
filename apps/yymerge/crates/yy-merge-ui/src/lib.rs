//! Windows host with the yy series' shared editor, font and offline help.
#[cfg(windows)]
mod comparison;
#[cfg(windows)]
mod image_view;
#[cfg(windows)]
mod large_view;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod remote_view;
#[cfg(windows)]
pub fn run(g: yy_merge_cli::Gui) -> yy_merge_core::Result<i32> {
    match run_inner(g) {
        Err(error) if error == "cancelled" => Ok(130),
        Err(error) => {
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    None,
                    &windows::core::HSTRING::from(&error),
                    windows::core::w!("yymerge"),
                    windows::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
                );
            }
            Err(error)
        }
        result => result,
    }
}

#[cfg(windows)]
fn run_inner(mut g: yy_merge_cli::Gui) -> yy_merge_core::Result<i32> {
    if g.output
        .as_ref()
        .is_some_and(|p| p.to_str().is_some_and(|s| s.starts_with("ssh:")))
    {
        return Err("出力先はローカルのパスを指定してください。SSH入力は読み取り専用です。".into());
    }
    let mut downloads = Vec::new();
    for (path, label) in [
        (&mut g.left, &mut g.left_label),
        (&mut g.right, &mut g.right_label),
    ] {
        if let Some(path) = path
            && let Some(download) = remote_view::download(path)?
        {
            *path = download.path.clone();
            *label = Some(download.label.clone());
            downloads.push(download);
        }
    }
    if let Some(path) = &mut g.base
        && let Some(download) = remote_view::download(path)?
    {
        *path = download.path.clone();
        downloads.push(download);
    }
    if g.image
        || [&g.left, &g.right]
            .into_iter()
            .flatten()
            .any(|p| yy_merge_core::images::is_image(p))
    {
        image_view::run(g)
    } else if use_large(&g) {
        if g.base.is_some()
            || g.rules.ignore_space
            || g.rules.ignore_case
            || g.rules.ignore_eol
            || g.rules.ignore_trailing_space
            || g.rules.rules_profile.is_some()
        {
            return Err("巨大ファイル表示は2方向の厳密比較です。3方向マージ・無視規則は通常モードの上限が適用されます。".into());
        }
        large_view::run(g)
    } else {
        native::run(g)
    }
}

#[cfg(windows)]
fn use_large(g: &yy_merge_cli::Gui) -> bool {
    g.large
        || g.left.as_ref().zip(g.right.as_ref()).is_some_and(|(l, r)| {
            yy_merge_cli::large_command::required(l, r)
                && [l, r].iter().all(|p| {
                    yy_merge_core::office::Kind::detect(p).is_none()
                        && !yy_merge_core::office_render::is_pdf(p)
                })
        })
}
