//! Headless commands share the GUI's exact comparison and merge model.
pub use clap::Parser as ArgumentParser;
use clap::{Parser, Subcommand, ValueEnum};
use std::{ffi::OsString, io::Write, path::PathBuf};
use yy_merge_core::{
    Cancellation, Kind, Result,
    files::{Snapshot, save},
    folders,
};
static CANCEL: Cancellation = Cancellation::new();
fn install_interrupt() {
    #[cfg(windows)]
    unsafe {
        use windows::{Win32::System::Console::*, core::BOOL};
        unsafe extern "system" fn handler(event: u32) -> BOOL {
            if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
                CANCEL.cancel();
                true.into()
            } else {
                false.into()
            }
        }
        let _ = SetConsoleCtrlHandler(Some(handler), true);
    }
    #[cfg(unix)]
    unsafe {
        extern "C" fn handler(_: libc::c_int) {
            CANCEL.cancel();
        }
        libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
    }
}

#[derive(Parser)]
#[command(name="yymerge", version, about="Exact text comparison and three-way merge", long_about=None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Subcommand)]
pub enum Command {
    /// Compare text files. Exit 0 identical, 1 different, 2 incomplete/error.
    Diff {
        left: PathBuf,
        right: PathBuf,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        encoding: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Merge using an explicit common ancestor. Inputs are never overwritten.
    Merge {
        #[arg(long)]
        base: PathBuf,
        #[arg(long)]
        left: PathBuf,
        #[arg(long)]
        right: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        encoding: Option<String>,
        #[arg(long)]
        conflict_markers: bool,
        #[arg(long)]
        overwrite: bool,
    },
    /// Compare folder contents byte for byte; never synchronize or follow links.
    Folders {
        left: PathBuf,
        right: PathBuf,
        #[arg(long)]
        recursive: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: FolderFormat,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Start yymergew; --wait returns its comparison/merge result.
    Gui(Gui),
}
#[derive(Clone, Debug, Default, clap::Args)]
pub struct Gui {
    #[arg(long, requires = "right")]
    pub left: Option<PathBuf>,
    #[arg(long, requires = "left")]
    pub right: Option<PathBuf>,
    #[arg(long,requires_all=["left","right"])]
    pub base: Option<PathBuf>,
    #[arg(long, requires = "base")]
    pub output: Option<PathBuf>,
    #[arg(long)]
    pub encoding: Option<String>,
    #[arg(long)]
    pub wait: bool,
    #[arg(long, hide = true)]
    pub ui_smoke_test: Option<PathBuf>,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    Text,
    Unified,
    Json,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum FolderFormat {
    Text,
    Json,
}

pub fn entry(
    args: impl IntoIterator<Item = OsString>,
    gui: impl FnOnce(Gui) -> Result<i32>,
) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let help = e.use_stderr();
            let _ = e.print();
            return if help { 64 } else { 0 };
        }
    };
    install_interrupt();
    let result = match cli.command {
        Command::Gui(g) => gui(g),
        command => execute(command, &mut std::io::stdout().lock(), &CANCEL),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("yymerge: {e}");
            if e == "cancelled" { 130 } else { 2 }
        }
    }
}
fn emit(
    text: &str,
    path: Option<PathBuf>,
    inputs: &[&Snapshot],
    overwrite: bool,
    stdout: &mut impl Write,
    cancel: &Cancellation,
) -> Result<()> {
    if let Some(path) = path {
        save(&path, text.as_bytes(), inputs, overwrite, cancel)
    } else {
        stdout.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }
}
pub fn execute(command: Command, stdout: &mut impl Write, cancel: &Cancellation) -> Result<i32> {
    match command {
        Command::Diff {
            left,
            right,
            format,
            report,
            encoding,
            overwrite,
        } => {
            let a = Snapshot::open(&left, encoding.as_deref(), cancel)?;
            let b = Snapshot::open(&right, encoding.as_deref(), cancel)?;
            let diff = yy_merge_core::compare(&a.text, &b.text, cancel)?;
            let same = diff.edits.is_empty() && a.encoding == b.encoding && a.bom == b.bom;
            let text=match format {
                Format::Json=>serde_json::to_string_pretty(&serde_json::json!({"identical":same,"left_encoding":a.encoding.name(),"right_encoding":b.encoding.name(),"left_bom":a.bom,"right_bom":b.bom,"diff":diff})).map_err(|e|e.to_string())?+"\n",
                Format::Text=>format!("{}: {} <> {} ({} changed regions; {} / {})\n",if same{"identical"}else{"different"},left.display(),right.display(),diff.edits.len(),a.encoding,b.encoding),
                Format::Unified=>{
                    if a.encoding.name()!="UTF-8" || b.encoding.name()!="UTF-8" || !a.bom.is_empty() || !b.bom.is_empty() || a.text.contains('\r') || b.text.contains('\r') {return Err("unified output currently requires UTF-8 without BOM and LF newlines; use --format json for other encodings/EOL".into());}
                    if same {String::new()}else{
                        let aa=yy_merge_core::lines(&a.text);let bb=yy_merge_core::lines(&b.text);
                        // Fixed labels prevent path/newline injection in patch headers.
                        let mut out=format!("--- a/left\n+++ b/right\n@@ -{},{} +{},{} @@\n",usize::from(!aa.is_empty()),aa.len(),usize::from(!bb.is_empty()),bb.len());
                        for row in &diff.rows {
                            let line=if let Some(i)=row.left {aa[i]}else{bb[row.right.unwrap()]};
                            out.push(match row.kind {Kind::Equal=>' ',Kind::Delete=>'-',Kind::Insert=>'+'});out.push_str(line);
                            if !line.ends_with('\n') {out.push_str("\n\\ No newline at end of file\n");}
                        }
                        out
                    }
                }
            };
            a.verify(cancel)?;
            b.verify(cancel)?;
            emit(&text, report, &[&a, &b], overwrite, stdout, cancel)?;
            Ok(i32::from(!same))
        }
        Command::Merge {
            base,
            left,
            right,
            output,
            encoding,
            conflict_markers,
            overwrite,
        } => {
            let b = Snapshot::open(&base, encoding.as_deref(), cancel)?;
            let l = Snapshot::open(&left, encoding.as_deref(), cancel)?;
            let r = Snapshot::open(&right, encoding.as_deref(), cancel)?;
            if b.encoding != l.encoding
                || b.encoding != r.encoding
                || b.bom != l.bom
                || b.bom != r.bom
            {
                return Err("merge inputs must use the same encoding and BOM".into());
            }
            let merged = yy_merge_core::merge(&b.text, &l.text, &r.text, cancel)?;
            if merged.unresolved() > 0 && !conflict_markers {
                writeln!(
                    stdout,
                    "{} unresolved conflicts; output unchanged",
                    merged.unresolved()
                )
                .map_err(|e| e.to_string())?;
                return Ok(1);
            }
            save(
                &output,
                &b.encode(&merged.render(conflict_markers)?)?,
                &[&b, &l, &r],
                overwrite,
                cancel,
            )?;
            Ok(i32::from(merged.unresolved() > 0))
        }
        Command::Folders {
            left,
            right,
            recursive,
            format,
            report,
            overwrite,
        } => {
            if let Some(path) = &report {
                let parent = std::fs::canonicalize(
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(std::path::Path::new(".")),
                )
                .map_err(|e| e.to_string())?;
                for root in [&left, &right] {
                    if parent.starts_with(std::fs::canonicalize(root).map_err(|e| e.to_string())?) {
                        return Err("folder report must be outside both input trees".into());
                    }
                }
            }
            let entries = folders::compare(&left, &right, recursive, cancel)?;
            let same = entries
                .iter()
                .all(|e| matches!(e.status, folders::Status::Equal));
            if entries
                .iter()
                .any(|e| matches!(e.status, folders::Status::SkippedLink))
            {
                return Err("folder comparison incomplete: links/reparse points skipped".into());
            }
            let text = match format {
                FolderFormat::Json => {
                    serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())? + "\n"
                }
                FolderFormat::Text => entries
                    .iter()
                    .map(|e| format!("{:?}\t{}\n", e.status, e.path.display()))
                    .collect(),
            };
            emit(&text, report, &[], overwrite, stdout, cancel)?;
            Ok(i32::from(!same))
        }
        Command::Gui(_) => Err("GUI must be dispatched by the application".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_preserves_output_on_conflict_and_reports_status() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path().join("base");
        let l = d.path().join("left");
        let r = d.path().join("right");
        let o = d.path().join("out");
        for (p, s) in [(&b, "a\n"), (&l, "b\n"), (&r, "c\n"), (&o, "keep")] {
            std::fs::write(p, s).unwrap();
        }
        let command = |markers| Command::Merge {
            base: b.clone(),
            left: l.clone(),
            right: r.clone(),
            output: o.clone(),
            encoding: None,
            conflict_markers: markers,
            overwrite: true,
        };
        assert_eq!(
            execute(command(false), &mut Vec::new(), &Cancellation::default()).unwrap(),
            1
        );
        assert_eq!(std::fs::read_to_string(&o).unwrap(), "keep");
        assert_eq!(
            execute(command(true), &mut Vec::new(), &Cancellation::default()).unwrap(),
            1
        );
        assert!(
            std::fs::read_to_string(&o)
                .unwrap()
                .contains("||||||| BASE")
        );
    }
}
