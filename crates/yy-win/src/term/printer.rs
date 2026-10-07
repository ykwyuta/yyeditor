//! 3270 のプリンター（3287）のタブの出力（14 章 12.3）。
//!
//! 受け取った印刷（[`PrintJob`]）を、Windows のプリンター（GDI）・PDF（「Microsoft Print to PDF」に
//! 出力先のファイルを指定して印刷する）・テキストのファイルに出す。用紙の大きさと、ジョブの
//! 桁数・行数から文字の大きさを決め、全角は 2 桁に置く（等幅の MS ゴシック）。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Printing::GetDefaultPrinterW;
use windows::Win32::Storage::Xps::{DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};
use yy_3270::PrintJob;
use yy_3270::print::is_wide;

/// PDF を作るプリンター（Windows 10 以降に付属）
pub(crate) const PDF_PRINTER: &str = "Microsoft Print to PDF";

/// 受け取った印刷の出力先。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    Printer,
    Pdf,
    Text,
    /// 一覧に溜める（自動で出さない）
    Ask,
}

impl Output {
    pub(crate) fn from_config(s: &str) -> Output {
        match s.trim().to_ascii_lowercase().as_str() {
            "printer" => Output::Printer,
            "pdf" => Output::Pdf,
            "text" => Output::Text,
            _ => Output::Ask,
        }
    }
}

/// 受け取ったジョブ（プリンターのタブの一覧の 1 行）。
pub(crate) struct StoredJob {
    pub number: usize,
    /// 受け取った時刻（`HH:MM:SS`）
    pub time: String,
    pub job: PrintJob,
    /// 出した先（「プリンター名に印刷しました」など）。まだなら空
    pub output: String,
}

/// 既定のプリンターの名前。
pub(crate) fn default_printer() -> Option<String> {
    unsafe {
        let mut len = 0u32;
        let _ = GetDefaultPrinterW(None, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if !GetDefaultPrinterW(Some(PWSTR(buf.as_mut_ptr())), &mut len).as_bool() {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

/// 印刷の 1 ページの桁数・行数（ジョブの指定と中身の大きい方）。
pub(crate) fn page_grid(job: &PrintJob) -> (usize, usize) {
    let cols = job.width().max(80).max(job.columns.min(220));
    let longest = job.pages.iter().map(Vec::len).max().unwrap_or(0);
    let rows = longest.max(job.lines_per_page).max(60);
    (cols, rows)
}

/// ジョブを印刷する。`printer` が空なら既定のプリンター、`output` を渡すとそのファイルに出す
/// （PDF のプリンターで使う）。印刷したページ数を返す。
pub(crate) fn print(
    job: &PrintJob,
    printer: &str,
    output: Option<&Path>,
    title: &str,
) -> Result<usize, String> {
    let name = if printer.trim().is_empty() {
        default_printer().ok_or("既定のプリンターがありません")?
    } else {
        printer.trim().to_owned()
    };
    let (cols, rows) = page_grid(job);
    unsafe {
        let hdc = CreateDCW(
            w!("WINSPOOL"),
            &HSTRING::from(name.as_str()),
            PCWSTR::null(),
            None,
        );
        if hdc.is_invalid() {
            return Err(format!("プリンター「{name}」を開けません"));
        }
        let title_w = HSTRING::from(title);
        let out_w = output.map(|p| HSTRING::from(p.as_os_str()));
        let doc = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: PCWSTR(title_w.as_ptr()),
            lpszOutput: out_w
                .as_ref()
                .map_or(PCWSTR::null(), |o| PCWSTR(o.as_ptr())),
            lpszDatatype: PCWSTR::null(),
            fwType: 0,
        };
        if StartDocW(hdc, &doc) <= 0 {
            let _ = DeleteDC(hdc);
            return Err(format!("プリンター「{name}」で印刷を始められません"));
        }
        let dpi_x = GetDeviceCaps(Some(hdc), LOGPIXELSX).max(72);
        let dpi_y = GetDeviceCaps(Some(hdc), LOGPIXELSY).max(72);
        let width = GetDeviceCaps(Some(hdc), HORZRES);
        let height = GetDeviceCaps(Some(hdc), VERTRES);
        // 余白は 0.4 インチ（印刷できない端の分はドライバーが除いている）
        let (mx, my) = (dpi_x * 2 / 5, dpi_y * 2 / 5);
        let cell_w = ((width - 2 * mx) / cols as i32).max(1);
        let cell_h = ((height - 2 * my) / rows as i32).max(1);
        // 半角の幅が桁の幅に収まる高さ（等幅のゴシックは高さの半分が半角の幅）
        let font_h = cell_h.min(cell_w * 2);
        let font = CreateFontW(
            -font_h,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            SHIFTJIS_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            FIXED_PITCH.0 as u32,
            w!("MS Gothic"),
        );
        let old = SelectObject(hdc, font.into());
        SetBkMode(hdc, TRANSPARENT);
        let mut pages = 0;
        let mut result = Ok(());
        for page in &job.pages {
            if StartPage(hdc) <= 0 {
                result = Err("ページを始められません".to_owned());
                break;
            }
            for (r, line) in page.iter().enumerate() {
                let y = my + r as i32 * cell_h;
                let mut col = 0usize;
                for c in line.chars() {
                    let wide = is_wide(c);
                    if c != ' ' && c != '\u{3000}' {
                        let mut buf = [0u16; 2];
                        let units = c.encode_utf16(&mut buf);
                        let _ = TextOutW(hdc, mx + col as i32 * cell_w, y, units);
                    }
                    col += if wide { 2 } else { 1 };
                }
            }
            if EndPage(hdc) <= 0 {
                result = Err("ページを終えられません".to_owned());
                break;
            }
            pages += 1;
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(font.into());
        if result.is_ok() {
            EndDoc(hdc);
        } else {
            windows::Win32::Storage::Xps::AbortDoc(hdc);
        }
        let _ = DeleteDC(hdc);
        result.map(|()| pages)
    }
}

/// PDF・テキストを自動で保存するフォルダー。
pub(crate) fn output_folder(configured: &str) -> PathBuf {
    if !configured.trim().is_empty() {
        return PathBuf::from(configured.trim());
    }
    std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join("Documents").join("yyterm-print"))
        .unwrap_or_else(|| PathBuf::from("yyterm-print"))
}

/// 自動で保存するファイルの名前（`ラベル-日時-番号.拡張子`。ファイル名に使えない文字は `_`）。
pub(crate) fn output_name(label: &str, stamp: &str, number: usize, ext: &str) -> String {
    let clean: String = format!("{label}-{stamp}-{number}")
        .chars()
        .map(|c| {
            if c.is_control() || r#"\/:*?"<>|[] "#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("{clean}.{ext}")
}

/// ジョブをテキストのファイル（UTF-8。改ページは FF）に保存する。
pub(crate) fn save_text(job: &PrintJob, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    }
    std::fs::write(path, job.to_text()).map_err(|e| format!("{} に書けません: {e}", path.display()))
}

/// ジョブを PDF に保存する（PDF のプリンターに出力先を指定して印刷する）。
pub(crate) fn save_pdf(job: &PrintJob, path: &Path, title: &str) -> Result<usize, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    }
    print(job, PDF_PRINTER, Some(path), title)
}

/// 保存先を選ぶ（`pdf` なら PDF、そうでなければテキスト）。
pub(crate) fn pick_save(owner: HWND, name: &str, pdf: bool) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH};
    unsafe {
        let d: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = if pdf {
            [COMDLG_FILTERSPEC {
                pszName: w!("PDF (*.pdf)"),
                pszSpec: w!("*.pdf"),
            }]
        } else {
            [COMDLG_FILTERSPEC {
                pszName: w!("テキスト (*.txt)"),
                pszSpec: w!("*.txt"),
            }]
        };
        let _ = d.SetFileTypes(&filters);
        let _ = d.SetDefaultExtension(if pdf { w!("pdf") } else { w!("txt") });
        let _ = d.SetFileName(&HSTRING::from(name));
        let _ = d.SetTitle(if pdf {
            w!("印刷を PDF で保存")
        } else {
            w!("印刷をテキストで保存")
        });
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// プリンターのタブに表示する文字（ジョブごとの見出しとページの内容。古い順で、最後に `status`）。
/// 端末の画面は最後の行を見せるので、新しいジョブが見える。古いものはスクロールで見る。
pub(crate) fn listing(status: &str, jobs: &[StoredJob], limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    if jobs.is_empty() {
        out.push("（まだ印刷を受け取っていません）".to_owned());
    }
    let skip = jobs.len().saturating_sub(limit);
    for j in &jobs[skip..] {
        let pages = j.job.pages.len();
        let lines: usize = j.job.pages.iter().map(Vec::len).sum();
        out.push(format!(
            "━━ #{} {}  {pages} ページ・{lines} 行  {}",
            j.number,
            j.time,
            if j.output.is_empty() {
                "（未出力。メニューから印刷・保存できます）"
            } else {
                j.output.as_str()
            }
        ));
        for (i, page) in j.job.pages.iter().enumerate() {
            if i > 0 {
                out.push(format!("── {} ページ ──", i + 1));
            }
            out.extend(page.iter().cloned());
        }
        out.push(String::new());
    }
    out.push(status.to_owned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(pages: Vec<Vec<&str>>) -> PrintJob {
        PrintJob {
            pages: pages
                .into_iter()
                .map(|p| p.into_iter().map(str::to_owned).collect())
                .collect(),
            columns: 132,
            lines_per_page: 0,
            bytes: 10,
        }
    }

    #[test]
    fn grids_names_and_listing() {
        let j = job(vec![vec!["売上"; 70]]);
        assert_eq!(page_grid(&j), (132, 70));
        assert_eq!(Output::from_config("PDF"), Output::Pdf);
        assert_eq!(Output::from_config(""), Output::Ask);
        assert_eq!(
            output_name("mvs [3287 P1]", "20261007-101500", 3, "pdf"),
            "mvs__3287_P1_-20261007-101500-3.pdf"
        );
        let jobs = vec![
            StoredJob {
                number: 1,
                time: "10:00:00".into(),
                job: job(vec![vec!["A"], vec!["B"]]),
                output: String::new(),
            },
            StoredJob {
                number: 2,
                time: "10:01:00".into(),
                job: job(vec![vec!["C"]]),
                output: "印刷しました".into(),
            },
        ];
        let l = listing("S", &jobs, 10);
        assert!(l[0].starts_with("━━ #1 10:00:00  2 ページ・2 行  （未出力"));
        assert_eq!(&l[1..5], &["A", "── 2 ページ ──", "B", ""]);
        assert!(l[5].starts_with("━━ #2 10:01:00  1 ページ・1 行  印刷しました"));
        assert_eq!(&l[6..], &["C", "", "S"]);
        // 上限より古いものは出さない
        assert_eq!(
            listing("S", &jobs, 1),
            vec![l[5].clone(), "C".into(), "".into(), "S".into()]
        );
        assert_eq!(
            listing("S", &[], 1),
            vec!["（まだ印刷を受け取っていません）", "S"]
        );
    }
}
