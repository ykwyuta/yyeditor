//! Development-data discovery UI and bounded, read-only CLI execution.
use super::*;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use yy_files::cleanup::{Candidate, Ecosystem, Location, Options, Report};

pub(super) const FILTERS: [&str; 19] = [
    "すべて",
    "Python",
    "Node.js / JS",
    "Java",
    "Rust",
    ".NET",
    "Docker",
    "Podman",
    "C/C++・ゲーム開発",
    "Go",
    "Ruby",
    "PHP",
    "携帯・Android/iOS",
    "仮想マシン・WSL",
    "ブラウザー・アプリ",
    "データベース・メール",
    "動画・音声・ディスク",
    "システム・ログ",
    "その他大容量データ",
];
const ECOSYSTEMS: [Ecosystem; 18] = [
    Ecosystem::Python,
    Ecosystem::Node,
    Ecosystem::Java,
    Ecosystem::Rust,
    Ecosystem::DotNet,
    Ecosystem::Docker,
    Ecosystem::Podman,
    Ecosystem::Native,
    Ecosystem::Go,
    Ecosystem::Ruby,
    Ecosystem::Php,
    Ecosystem::Mobile,
    Ecosystem::VirtualMachine,
    Ecosystem::Browser,
    Ecosystem::Database,
    Ecosystem::Media,
    Ecosystem::System,
    Ecosystem::Other,
];

pub(super) fn find() {
    with(|a| {
        if a.busy.is_some() {
            return;
        }
        let parse = |h| text_of(h).trim().parse::<u32>();
        let (Ok(min), Ok(age)) = (parse(a.edits.clean_min), parse(a.edits.clean_age)) else {
            error_box(
                a.frame,
                "最小 MiB とビルド経過日には0以上の整数を指定してください。",
            );
            return;
        };
        let filter = unsafe { SendMessageW(a.edits.clean_ecosystem, CB_GETCURSEL, None, None).0 };
        let ecosystem = usize::try_from(filter)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| ECOSYSTEMS.get(i))
            .copied();
        let options = Options {
            min_bytes: u64::from(min) * 1024 * 1024,
            build_age_days: age,
            ecosystem,
            ..Options::default()
        };
        let containers_only = matches!(ecosystem, Some(Ecosystem::Docker | Ecosystem::Podman));
        let roots = if containers_only {
            Vec::new()
        } else {
            roots_of(&text_of(a.edits.clean_roots))
        };
        let checked = |h| unsafe { SendMessageW(h, BM_GETCHECK, None, None).0 != 0 };
        let caches = if checked(a.edits.clean_user) {
            yy_files::cleanup::user_caches(|key| {
                std::env::var_os(key)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
        } else {
            Vec::new()
        };
        let engines = checked(a.edits.clean_engines) && (ecosystem.is_none() || containers_only);
        if roots.is_empty() && caches.is_empty() && !engines {
            error_box(
                a.frame,
                "選択した環境の探索場所か共通キャッシュを指定してください。Docker / Podman を調べる場合は、環境の選択と調査チェックも確認してください。",
            );
            return;
        }
        a.cleanup = Report::default();
        a.refresh_list(TAB_CLEANUP);
        a.start(
            "キャッシュ・大容量データの整理候補を探索",
            move |cx| {
                let result = (|| {
                    let mut report = yy_files::cleanup::scan(
                        &yy_files::Local,
                        &roots,
                        &caches,
                        &options,
                        &|n, path| cx.progress(&format!("探索: {n} 項目・{}", path.display())),
                    )?;
                    if engines {
                        for engine in [Ecosystem::Docker, Ecosystem::Podman] {
                            if options.ecosystem.is_some_and(|e| e != engine) {
                                continue;
                            }
                            match yy_files::cleanup::containers(
                                engine,
                                options.min_bytes,
                                &|tool, args| run_tool(cx, tool, args),
                            ) {
                                Ok(other) => {
                                    let omitted = other
                                        .warning_count
                                        .saturating_sub(other.warnings.len() as u64);
                                    report.items.extend(other.items);
                                    for warning in other.warnings {
                                        report.warn(warning);
                                    }
                                    report.warning_count += omitted;
                                }
                                Err(e) if yy_files::is_cancelled(&e) => return Err(e),
                                Err(e) => {
                                    report.warn(format!("{}: 調査できません: {e}", engine.label()))
                                }
                            }
                        }
                    }
                    report.sort();
                    Ok(report)
                })();
                cx.send(Msg::Cleaned(result.map_err(|e| work::describe(&e))));
            },
        );
    });
}

fn executable(tool: &str) -> io::Result<PathBuf> {
    // Windows' implicit current-directory executable lookup must not run a project's docker.exe.
    if !matches!(tool, "docker" | "podman") {
        return Err(io::Error::other("未対応のツール"));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(format!("{tool}.exe")))
        .find(|p| p.is_file())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{tool}.exe が PATH にありません"),
            )
        })
}

fn run_tool(cx: &work::Ctx, tool: &str, args: &[&str]) -> io::Result<String> {
    command_output(
        &executable(tool)?,
        args,
        &|| cx.progress(&format!("{} を調査中（現在の接続先）", tool)),
        Duration::from_secs(60),
    )
}

fn command_output(
    program: &Path,
    args: &[&str],
    progress: &impl Fn() -> bool,
    timeout: Duration,
) -> io::Result<String> {
    capture(Command::new(program).args(args), progress, timeout)
}

struct ToolChild(std::process::Child);

impl Drop for ToolChild {
    fn drop(&mut self) {
        // Also reap on early metadata/read errors, not only cancellation and timeout.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn capture(
    command: &mut Command,
    progress: &impl Fn() -> bool,
    timeout: Duration,
) -> io::Result<String> {
    use std::os::windows::process::CommandExt;
    if !progress() {
        return Err(yy_files::cancelled());
    }
    // Files avoid pipe deadlocks and remain bounded even if a tool emits excessive output.
    let stdout = tempfile::NamedTempFile::new()?;
    let stderr = tempfile::NamedTempFile::new()?;
    let mut child = ToolChild(
        command
            .stdin(Stdio::null())
            .stdout(stdout.as_file().try_clone()?)
            .stderr(stderr.as_file().try_clone()?)
            .creation_flags(0x08000000)
            .spawn()?,
    );
    let start = Instant::now();
    const LIMIT: u64 = 8 * 1024 * 1024;
    let status = loop {
        let stop = if !progress() {
            Some(yy_files::cancelled())
        } else if start.elapsed() >= timeout {
            Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "ツールの応答が制限時間内に返りません",
            ))
        } else if stdout.as_file().metadata()?.len() > LIMIT
            || stderr.as_file().metadata()?.len() > LIMIT
        {
            Some(io::Error::other("ツールの出力が8 MiBを超えました"))
        } else {
            None
        };
        if let Some(error) = stop {
            return Err(error);
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                return Err(e);
            }
        }
    };
    let read = |path: &Path, limit| -> io::Result<String> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(io::Error::other("ツールの出力が上限を超えました"));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    if !status.success() {
        return Err(io::Error::other(format!(
            "終了コード {status}: {}",
            read(stderr.path(), 16 * 1024)?.trim()
        )));
    }
    read(stdout.path(), LIMIT)
}

pub(super) fn finished(result: std::result::Result<Report, String>) {
    with(|a| match result {
        Err(error) => finish(a, &format!("探索終了: {error}")),
        Ok(report) => {
            let folders = report
                .items
                .iter()
                .filter(|c| matches!(c.location, Location::Folder(_) | Location::File(_)))
                .fold(0u64, |n, c| n.saturating_add(c.bytes));
            let summary = format!(
                "候補 {} 件・ファイル/フォルダ論理容量 {}・警告 {} 件。不要とは未判定・イメージ共有容量に注意",
                report.items.len(),
                yy_files::human_size(folders),
                report.warning_count
            );
            for warning in &report.warnings {
                a.log_line(warning);
            }
            a.cleanup = report;
            a.refresh_list(TAB_CLEANUP);
            finish(a, &summary);
        }
    });
}

fn selected(row: Option<usize>) -> Option<(HWND, Candidate)> {
    with(|a| {
        let index = row.or_else(|| a.selected_rows(TAB_CLEANUP).first().copied())?;
        a.cleanup.items.get(index).cloned().map(|c| (a.frame, c))
    })
    .flatten()
}

pub(super) fn details(row: Option<usize>) {
    if let Some((frame, c)) = selected(row) {
        info_box(
            frame,
            &format!(
                "{} / {}\n{}\n\n論理容量: {}{}\n{}\n\n整理方法:\n{}\n\n削除前に再生成できること・現在使っていないことを確認してください。コンテナのコマンドは探索時と同じ接続先で実行してください。",
                c.ecosystem.label(),
                c.kind,
                c.address(),
                yy_files::human_size(c.bytes),
                if c.incomplete {
                    "（一部未集計）"
                } else {
                    ""
                },
                c.reason,
                c.advice
            ),
        );
    }
}

pub(super) fn open() {
    if let Some((frame, c)) = selected(None) {
        if let Location::Folder(path) | Location::File(path) = &c.location {
            let path = if matches!(c.location, Location::File(_)) {
                path.parent().unwrap_or(path)
            } else {
                path.as_path()
            };
            with(|a| a.show_tab(TAB_BROWSE));
            browse::open_path(path);
        } else {
            info_box(
                frame,
                "この候補はコンテナエンジンが管理します。「詳細」でIDと整理方法を確認してください。",
            );
        }
    }
}

pub(super) fn copy() {
    let Some((frame, text)) = with(|a| {
        let items: Vec<_> = a
            .selected_rows(TAB_CLEANUP)
            .into_iter()
            .filter_map(|i| a.cleanup.items.get(i))
            .map(|c| {
                format!(
                    "# {}\r\n{}",
                    c.address().replace(['\r', '\n'], " "),
                    c.advice
                )
            })
            .collect();
        (a.frame, items.join("\r\n\r\n"))
    }) else {
        return;
    };
    if !text.is_empty() && !crate::clipboard::set_text(frame, &text, false) {
        error_box(frame, "整理方法をクリップボードへコピーできませんでした。");
    }
}

pub(super) fn export() {
    let Some((frame, text)) = with(|a| (a.frame, a.cleanup.csv())) else {
        return;
    };
    let Some(path) = pick_file(
        frame,
        ("CSV (*.csv)", "*.csv"),
        None,
        Some("development-cleanup.csv"),
    ) else {
        return;
    };
    if let Err(e) = std::fs::write(&path, text) {
        error_box(frame, &format!("{}: {e}", path.display()));
    } else {
        with(|a| a.set_status(&format!("CSVを書き出しました: {}", path.display())));
    }
}

pub(super) fn menu(frame: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        for (id, label) in [
            (ID_CLEAN_DETAIL, "詳細"),
            (ID_CLEAN_OPEN, "フォルダで確認"),
            (ID_CLEAN_COPY, "整理方法をコピー"),
            (ID_CLEAN_EXPORT, "CSV 出力..."),
        ] {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(label));
        }
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let result = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            frame,
            None,
        );
        let _ = DestroyMenu(menu);
        if result.0 != 0 {
            command(result.0 as u16, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an available local Docker CLI and engine"]
    fn cleanup_real_docker_cli() {
        let executable = executable("docker").unwrap();
        let report = yy_files::cleanup::containers(Ecosystem::Docker, 0, &|_, args| {
            command_output(&executable, args, &|| true, Duration::from_secs(60))
        })
        .unwrap();
        assert_eq!(
            report.warning_count, 0,
            "Docker discovery warnings: {:?}",
            report.warnings
        );
        println!(
            "Docker read-only discovery: {} candidates",
            report.items.len()
        );
    }

    #[test]
    fn cleanup_native_discovery_is_async_and_fits_small_window() {
        const CHILD: &str = "YY_FM_CLEANUP_TEST_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            native_fixture(&PathBuf::from(root));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        for folder in ["profile", "appdata", "local"] {
            std::fs::create_dir(dir.path().join(folder)).unwrap();
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "fm::cleanup::tests::cleanup_native_discovery_is_async_and_fits_small_window",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, dir.path())
            .env("APPDATA", dir.path().join("appdata"))
            .env("USERPROFILE", dir.path().join("profile"))
            .env("LOCALAPPDATA", dir.path().join("local"));
        let output = capture(&mut command, &|| true, Duration::from_secs(90)).unwrap();
        assert!(
            output.contains("1 passed"),
            "native child failed or selected no test: {output}"
        );
    }

    fn native_fixture(root: &Path) {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES
                    | ICC_TAB_CLASSES
                    | ICC_LISTVIEW_CLASSES
                    | ICC_STANDARD_CLASSES,
            });
        }
        let project = root.join("project");
        for (folder, file) in [
            ("node_modules/package", "data"),
            ("__pycache__", "module.pyc"),
        ] {
            std::fs::create_dir_all(project.join(folder)).unwrap();
            std::fs::write(project.join(folder).join(file), "fixture data").unwrap();
        }
        let frame = create().unwrap();
        struct Frame(HWND);
        impl Drop for Frame {
            fn drop(&mut self) {
                cancel();
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
                APP.with(|a| a.borrow_mut().take());
            }
        }
        let _frame = Frame(frame);
        with(|a| {
            set_text(a.edits.clean_roots, &project.to_string_lossy());
            set_text(a.edits.clean_min, "0");
            unsafe {
                SendMessageW(a.edits.clean_user, BM_SETCHECK, Some(WPARAM(0)), None);
                SendMessageW(a.edits.clean_engines, BM_SETCHECK, Some(WPARAM(0)), None);
            }
            a.show_tab(TAB_CLEANUP);
        });
        command(ID_CLEAN_FIND, 0);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message != WM_QUIT {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
            let done = with(|a| {
                a.busy.is_none()
                    && a.cleanup
                        .items
                        .iter()
                        .any(|c| c.address().ends_with("node_modules"))
                    && a.cleanup
                        .items
                        .iter()
                        .any(|c| c.address().ends_with("__pycache__"))
            }) == Some(true);
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "native scan timeout: {:?}",
                with(|a| (
                    a.busy.as_ref().map(|(_, s)| s.clone()),
                    a.log_text.clone(),
                    a.cleanup
                        .items
                        .iter()
                        .map(Candidate::address)
                        .collect::<Vec<_>>()
                ))
            );
            unsafe {
                MsgWaitForMultipleObjectsEx(None, 25, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }
        with(|a| {
            assert_eq!(a.row_count(TAB_CLEANUP), 2);
            let row = a
                .cleanup
                .items
                .iter()
                .position(|c| c.address().ends_with("node_modules"))
                .unwrap();
            assert_eq!(a.cell(TAB_CLEANUP, row, 0), "Node.js / JS");
            let csv = a.cleanup.csv();
            assert!(csv.contains("node_modules") && csv.contains("__pycache__"));
            std::fs::write(root.join("result.csv"), csv).unwrap();
        });
        for width in [640, 1100] {
            unsafe {
                MoveWindow(frame, 0, 0, width, 520, true).unwrap();
            }
            with(|a| {
                a.layout();
                let mut client = RECT::default();
                let mut origin = POINT::default();
                unsafe {
                    GetClientRect(frame, &mut client).unwrap();
                    windows::Win32::Graphics::Gdi::ClientToScreen(frame, &mut origin).unwrap();
                }
                for row in &a.rows[TAB_CLEANUP] {
                    for cell in row {
                        let mut rect = RECT::default();
                        unsafe {
                            GetWindowRect(cell.hwnd, &mut rect).unwrap();
                        }
                        assert!(
                            rect.left >= origin.x && rect.right <= origin.x + client.right,
                            "control outside {width}px window: {} {rect:?} client={client:?}",
                            text_of(cell.hwnd)
                        );
                    }
                }
                let mut list = RECT::default();
                unsafe {
                    GetWindowRect(a.lists[TAB_CLEANUP], &mut list).unwrap();
                }
                assert!(list.bottom > list.top && list.bottom <= origin.y + client.bottom);
            });
        }
        assert_eq!(
            std::fs::read_to_string(project.join("node_modules/package/data")).unwrap(),
            "fixture data"
        );
    }

    #[test]
    fn cleanup_command_is_bounded_and_reports_errors() {
        const CHILD: &str = "YY_FM_COMMAND_TEST_CHILD";
        if let Ok(mode) = std::env::var(CHILD) {
            assert_eq!(mode, "ok", "expected helper failure");
            println!("cleanup-command-output");
            return;
        }
        let make = |mode| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "fm::cleanup::tests::cleanup_command_is_bounded_and_reports_errors",
                    "--nocapture",
                ])
                .env(CHILD, mode);
            command
        };
        assert!(
            capture(&mut make("ok"), &|| true, Duration::from_secs(10))
                .unwrap()
                .contains("cleanup-command-output")
        );
        assert!(
            capture(&mut make("error"), &|| true, Duration::from_secs(10))
                .unwrap_err()
                .to_string()
                .contains("expected helper failure")
        );
        let error = command_output(
            Path::new("not-an-executable"),
            &[],
            &|| false,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(yy_files::is_cancelled(&error));
        let error = capture(&mut make("ok"), &|| true, Duration::ZERO).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
