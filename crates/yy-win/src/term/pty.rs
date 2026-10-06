//! シェルの起動（手元は ConPTY、接続先は SSH の端末つきチャネル）。
//!
//! どちらも [`Backend`]（入力・出力・大きさの変更・終了の待ち合わせ・強制終了）にそろえる。

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

/// 動いているシェル。
pub(crate) struct Backend {
    pub input: Box<dyn Write + Send>,
    pub output: Box<dyn Read + Send>,
    pub resize: Box<dyn Fn(u16, u16) + Send + Sync>,
    /// 終わるまで待って終了コードを返す（出力を読み切れるようにしてから返る）
    pub wait: Box<dyn FnOnce() -> Option<u32> + Send>,
    /// 終わらせる（タブを閉じたとき）
    pub kill: Box<dyn Fn() + Send + Sync>,
}

impl From<yy_remote::Shell> for Backend {
    fn from(s: yy_remote::Shell) -> Backend {
        let finish = s.finish;
        Backend {
            input: s.input,
            output: s.output,
            resize: s.resize,
            wait: Box::new(move || finish().ok().and_then(|e| e.status)),
            kill: s.close,
        }
    }
}

/// 手元のシェルの既定のコマンド（PowerShell 7、なければ Windows PowerShell、なければ cmd）。
pub(crate) fn default_shell() -> Vec<String> {
    if let Some(p) = find_in_path("pwsh.exe") {
        return vec![p.to_string_lossy().into_owned()];
    }
    if let Some(p) = find_in_path("powershell.exe") {
        return vec![p.to_string_lossy().into_owned()];
    }
    vec![std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())]
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

/// コマンドと引数を Windows のコマンドラインにする（必要なら引用符で囲む）。
pub(crate) fn command_line(args: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            out.push_str(a);
            continue;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in a.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    // 引用符の前の \ は 2 倍にしてから \" にする
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                c => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    out.push(c);
                    backslashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
    }
    out
}

/// ConPTY とシェルのプロセス。
struct Conpty {
    /// 疑似コンソール（閉じたら `None`）
    console: Mutex<Option<isize>>,
    process: isize,
}

impl Conpty {
    fn close_console(&self) {
        if let Some(h) = self.console.lock().unwrap().take() {
            unsafe { ClosePseudoConsole(HPCON(h)) };
        }
    }
}

impl Drop for Conpty {
    fn drop(&mut self) {
        self.close_console();
        unsafe {
            let _ = CloseHandle(HANDLE(self.process as *mut c_void));
        }
    }
}

/// 手元のシェルを ConPTY（Windows 10 1809 以降の疑似コンソール）で起動する。
pub(crate) fn spawn_local(
    args: &[String],
    cwd: Option<&Path>,
    size: (u16, u16),
) -> Result<Backend, String> {
    let cmd = command_line(args);
    spawn_conpty(&cmd, cwd, size)
        .map_err(|e| format!("{cmd} を起動できませんでした: {}", e.message()))
}

fn spawn_conpty(cmd: &str, cwd: Option<&Path>, size: (u16, u16)) -> windows::core::Result<Backend> {
    unsafe {
        // シェルへの入力と、シェルからの出力のパイプ
        let (mut in_read, mut in_write) = (HANDLE::default(), HANDLE::default());
        let (mut out_read, mut out_write) = (HANDLE::default(), HANDLE::default());
        CreatePipe(&mut in_read, &mut in_write, None, 0)?;
        if let Err(e) = CreatePipe(&mut out_read, &mut out_write, None, 0) {
            let _ = CloseHandle(in_read);
            let _ = CloseHandle(in_write);
            return Err(e);
        }
        let coord = COORD {
            X: size.0.max(1) as i16,
            Y: size.1.max(1) as i16,
        };
        let console = CreatePseudoConsole(coord, in_read, out_write, 0);
        // 疑似コンソールが複製を持つので、こちら側の端は閉じる
        let _ = CloseHandle(in_read);
        let _ = CloseHandle(out_write);
        let console = match console {
            Ok(c) => c,
            Err(e) => {
                let _ = CloseHandle(in_write);
                let _ = CloseHandle(out_read);
                return Err(e);
            }
        };
        let close_all = || {
            ClosePseudoConsole(console);
            let _ = CloseHandle(in_write);
            let _ = CloseHandle(out_read);
        };

        // 疑似コンソールをつないで起動する
        let mut size_needed = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size_needed);
        let mut buf = vec![0u8; size_needed];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buf.as_mut_ptr().cast());
        if let Err(e) = InitializeProcThreadAttributeList(Some(list), 1, None, &mut size_needed) {
            close_all();
            return Err(e);
        }
        let r = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            Some(console.0 as *const c_void),
            std::mem::size_of::<HPCON>(),
            None,
            None,
        );
        if let Err(e) = r {
            DeleteProcThreadAttributeList(list);
            close_all();
            return Err(e);
        }
        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // 親の標準入出力（リダイレクトされている場合）を引き継がない
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        si.lpAttributeList = list;
        let mut cmdline: Vec<u16> = cmd.encode_utf16().chain([0]).collect();
        let dir: Option<Vec<u16>> = cwd.map(|d| {
            d.as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain([0])
                .collect()
        });
        let mut pi = PROCESS_INFORMATION::default();
        let r = CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            false,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            None,
            dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(list);
        drop(buf);
        if let Err(e) = r {
            close_all();
            return Err(e);
        }
        let _ = CloseHandle(pi.hThread);

        let pty = Arc::new(Conpty {
            console: Mutex::new(Some(console.0)),
            process: pi.hProcess.0 as isize,
        });
        let input = File::from_raw_handle(in_write.0);
        let output = File::from_raw_handle(out_read.0);
        let p = pty.clone();
        let resize = Box::new(move |cols: u16, rows: u16| {
            if let Some(h) = *p.console.lock().unwrap() {
                let _ = ResizePseudoConsole(
                    HPCON(h),
                    COORD {
                        X: cols.max(1) as i16,
                        Y: rows.max(1) as i16,
                    },
                );
            }
        });
        let p = pty.clone();
        let wait = Box::new(move || {
            let process = HANDLE(p.process as *mut c_void);
            WaitForSingleObject(process, INFINITE);
            let mut code = 0u32;
            let code = GetExitCodeProcess(process, &mut code).ok().map(|()| code);
            // シェルが終わっても、疑似コンソールを閉じるまで出力のパイプは閉じない
            p.close_console();
            code
        });
        let p = pty;
        let kill = Box::new(move || {
            let _ = TerminateProcess(HANDLE(p.process as *mut c_void), 1);
        });
        Ok(Backend {
            input: Box::new(input),
            output: Box::new(output),
            resize,
            wait,
            kill,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_command_lines() {
        let a = |v: &[&str]| command_line(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["cmd.exe", "/k", "echo"]), "cmd.exe /k echo");
        assert_eq!(
            a(&[r"C:\Program Files\PowerShell\7\pwsh.exe", "-NoLogo"]),
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#
        );
        assert_eq!(a(&[r#"say "hi""#]), r#""say \"hi\"""#);
        assert_eq!(a(&[r"dir\ x\"]), r#""dir\ x\\""#);
        assert_eq!(a(&[""]), r#""""#);
    }

    /// ConPTY でコマンドを動かし、出力を最後まで読めること（Windows の CI で確かめる）。
    #[test]
    fn runs_a_command_in_a_pseudo_console() {
        let dir = std::env::temp_dir();
        let b = spawn_local(
            &[
                "cmd.exe".into(),
                "/c".into(),
                "echo yyterm-%CD%& exit 7".into(),
            ],
            Some(&dir),
            (80, 25),
        )
        .unwrap();
        let mut output = b.output;
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = output.read_to_end(&mut out);
            out
        });
        let code = (b.wait)();
        let out = String::from_utf8_lossy(&reader.join().unwrap()).into_owned();
        assert_eq!(code, Some(7));
        assert!(out.contains("yyterm-"), "{out:?}");
        drop(b.input);
    }
}
