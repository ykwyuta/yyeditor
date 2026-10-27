//! Headless commands share the GUI's exact comparison and merge model.
pub use clap::Parser as ArgumentParser;
pub mod git_tool;
pub mod large_command;
mod remote_input;
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
    /// Bounded-memory text comparison with a disk index (including SSH inputs).
    LargeDiff {
        left: PathBuf,
        right: PathBuf,
        #[arg(long)]
        encoding: Option<String>,
        #[command(flatten)]
        inputs: InputOptions,
        /// JSON summary and per-row source offsets; written atomically.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Compare every PDF page visually without external tools.
    PdfDiff {
        left: PathBuf,
        right: PathBuf,
        #[arg(long, default_value_t = 0)]
        threshold: u8,
        #[arg(long, value_enum, default_value = "json")]
        format: ImageReport,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Compare raster pixels, PDF pages or rendered Excel/PowerPoint pages.
    ImageDiff {
        left: PathBuf,
        right: PathBuf,
        /// One-based PDF page number. Raster images require 1.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=128))]
        page: u16,
        #[arg(long, default_value_t = 0)]
        threshold: u8,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        offset_x: i32,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        offset_y: i32,
        #[arg(long, value_enum, default_value = "json")]
        format: ImageReport,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        diff_image: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Save or inspect folder comparison setup; opening rescans current contents.
    FolderSession {
        #[command(subcommand)]
        command: FolderSessionCommand,
    },
    /// Save or inspect a two-way comparison work session.
    ComparisonSession {
        #[command(subcommand)]
        command: ComparisonSessionCommand,
    },
    /// Create or inspect a resumable three-way merge work session.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Save or validate a portable comparison-rules profile.
    RulesProfile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Git external diff/merge adapters and configuration examples.
    GitTool {
        #[command(subcommand)]
        command: git_tool::GitCommand,
    },
    /// Compare text or OOXML Excel/PowerPoint files. Exit 0 equivalent, 1 different, 2 incomplete/error.
    Diff {
        left: PathBuf,
        right: PathBuf,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        encoding: Option<String>,
        #[command(flatten)]
        inputs: InputOptions,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        rules: RuleArgs,
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
        /// Common ancestor folder: classify additions, deletions and changes in three trees.
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long)]
        recursive: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: FolderFormat,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        filter: FolderFilterArgs,
    },
    /// Start yymergew; --wait returns its comparison/merge result.
    Gui(Box<Gui>),
    /// Review, execute and restore journalled folder synchronization.
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
}
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum FolderStatus {
    #[default]
    All,
    Different,
    Equal,
    LeftOnly,
    RightOnly,
    Review,
    Uncompared,
}
impl From<FolderStatus> for folders::filter::Selection {
    fn from(value: FolderStatus) -> Self {
        use folders::filter::Selection as S;
        match value {
            FolderStatus::All => S::All,
            FolderStatus::Different => S::Different,
            FolderStatus::Equal => S::Equal,
            FolderStatus::LeftOnly => S::LeftOnly,
            FolderStatus::RightOnly => S::RightOnly,
            FolderStatus::Review => S::Review,
            FolderStatus::Uncompared => S::Uncompared,
        }
    }
}
#[derive(Clone, Debug, Default, clap::Args)]
pub struct FolderFilterArgs {
    /// Display only relative paths containing this literal text (Unicode lowercase, slash-normalized).
    #[arg(long)]
    path_contains: Option<String>,
    /// Match the whole relative path with * (any sequence including /) and ? (one character).
    #[arg(long, conflicts_with = "path_contains")]
    path_glob: Option<String>,
    /// Display group only; never changes scan scope, global exit status or sync inputs.
    #[arg(long, value_enum, default_value = "all")]
    status: FolderStatus,
}
impl FolderFilterArgs {
    fn core(self) -> Result<folders::filter::Filter> {
        if let Some(pattern) = self.path_glob {
            folders::filter::Filter::wildcard(pattern, self.status.into())
        } else {
            folders::filter::Filter::new(self.path_contains.unwrap_or_default(), self.status.into())
        }
    }
}
#[derive(Subcommand)]
pub enum FolderSessionCommand {
    Create {
        #[arg(long)]
        left: PathBuf,
        #[arg(long)]
        right: PathBuf,
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long)]
        recursive: bool,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        filter: FolderFilterArgs,
    },
    /// Validate setup and roots, without scanning or restoring old results.
    Inspect { path: PathBuf },
}
#[derive(Subcommand)]
pub enum ComparisonSessionCommand {
    /// Capture paths, input fingerprints and comparison rules (no input copies).
    Create {
        #[arg(long)]
        left: PathBuf,
        #[arg(long)]
        right: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        encoding: Option<String>,
        #[command(flatten)]
        inputs: InputOptions,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        rules: RuleArgs,
    },
    /// Verify inputs and recompute the comparison; never write either input.
    Inspect { path: PathBuf },
}
#[derive(Subcommand)]
pub enum SessionCommand {
    /// Save an initial merge work session, including unresolved conflicts.
    Create {
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
        overwrite: bool,
    },
    /// Verify the session and its inputs, then print a summary (no result writes).
    Inspect { path: PathBuf },
}
#[derive(Subcommand)]
pub enum ProfileCommand {
    /// Save selected rules as versioned UTF-8 JSON. Existing files require --overwrite.
    Save {
        path: PathBuf,
        #[command(flatten)]
        rules: RuleArgs,
        #[arg(long)]
        overwrite: bool,
    },
    /// Validate and print a profile without comparing or modifying any files.
    Show { path: PathBuf },
}
#[derive(Subcommand)]
pub enum SyncCommand {
    /// Continue an interrupted run after verifying all inputs and prepared candidates.
    Resume {
        run_dir: PathBuf,
        #[arg(long, required = true)]
        execute: bool,
    },
    /// Validate a saved plan. Actual writes require --execute and a fresh backup directory.
    Apply {
        plan: PathBuf,
        #[arg(long, requires = "run_dir")]
        execute: bool,
        #[arg(long, requires = "execute")]
        run_dir: Option<PathBuf>,
    },
    /// Restore a run, preserving created files and refusing to replace edited results.
    Rollback {
        run_dir: PathBuf,
        #[arg(long, required = true)]
        execute: bool,
    },
    /// Recompare inputs and emit a read-only JSON operation proposal.
    Plan {
        #[arg(long)]
        left: PathBuf,
        #[arg(long)]
        right: PathBuf,
        #[arg(long, required_if_eq("mode", "merge"))]
        base: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "update-right")]
        mode: PlanMode,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, requires = "output")]
        overwrite: bool,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum PlanMode {
    UpdateRight,
    UpdateLeft,
    MirrorRight,
    MirrorLeft,
    Merge,
}
impl From<PlanMode> for yy_merge_core::sync_plan::Mode {
    fn from(mode: PlanMode) -> Self {
        use yy_merge_core::sync_plan::Mode as M;
        match mode {
            PlanMode::UpdateRight => M::UpdateRight,
            PlanMode::UpdateLeft => M::UpdateLeft,
            PlanMode::MirrorRight => M::MirrorRight,
            PlanMode::MirrorLeft => M::MirrorLeft,
            PlanMode::Merge => M::Merge,
        }
    }
}
#[derive(Clone, Debug, Default, clap::Args)]
pub struct RuleArgs {
    /// Read rules from a versioned JSON profile; cannot combine with ignore flags.
    #[arg(long, conflicts_with_all = ["ignore_space", "ignore_trailing_space", "ignore_case", "ignore_eol"])]
    pub rules_profile: Option<PathBuf>,
    /// Ignore ASCII space and tab within a line (preserve line boundaries).
    #[arg(long)]
    pub ignore_space: bool,
    /// Ignore ASCII spaces and tabs at the end of each line.
    #[arg(long)]
    pub ignore_trailing_space: bool,
    /// Compare Unicode lowercase keys (no locale-dependent case folding).
    #[arg(long)]
    pub ignore_case: bool,
    /// Treat CRLF and LF terminators as equivalent; keep final-newline differences.
    #[arg(long)]
    pub ignore_eol: bool,
}
impl RuleArgs {
    pub fn from_core(rules: yy_merge_core::rules::Rules) -> Self {
        Self {
            ignore_space: rules.ignore_space,
            ignore_trailing_space: rules.ignore_trailing_space,
            ignore_case: rules.ignore_case,
            ignore_eol: rules.ignore_eol,
            rules_profile: None,
        }
    }
    fn resolve(
        &self,
        cancel: &Cancellation,
    ) -> Result<(yy_merge_core::rules::Rules, Option<Snapshot>)> {
        if let Some(path) = &self.rules_profile {
            let (profile, snapshot) = yy_merge_core::profile::Profile::open(path, cancel)?;
            Ok((profile.rules, Some(snapshot)))
        } else {
            Ok((self.core(), None))
        }
    }
    pub fn core(&self) -> yy_merge_core::rules::Rules {
        yy_merge_core::rules::Rules {
            ignore_space: self.ignore_space,
            ignore_trailing_space: self.ignore_trailing_space,
            ignore_case: self.ignore_case,
            ignore_eol: self.ignore_eol,
        }
    }
}
#[derive(Clone, Debug, Default, clap::Args)]
pub struct InputOptions {
    /// Left encoding, e.g. UTF-8, CP932 or IBM-939/fixed:80. Read-only byte review.
    #[arg(long, conflicts_with = "encoding")]
    pub left_encoding: Option<String>,
    /// Right encoding. Invalid bytes remain visible.
    #[arg(long, conflicts_with = "encoding")]
    pub right_encoding: Option<String>,
    /// Ignore encoding/BOM differences when comparing decoded contents.
    #[arg(long)]
    pub ignore_encoding: bool,
}
impl InputOptions {
    pub fn active(&self) -> bool {
        self.left_encoding.is_some() || self.right_encoding.is_some() || self.ignore_encoding
    }
    pub fn open_pair(
        &self,
        left: &std::path::Path,
        right: &std::path::Path,
        common: Option<&str>,
        c: &Cancellation,
    ) -> Result<[Snapshot; 2]> {
        if common.is_some() && (self.left_encoding.is_some() || self.right_encoding.is_some()) {
            return Err("use either --encoding or per-file encodings".into());
        }
        let mut files = Vec::new();
        for (path, encoding) in [
            (left, self.left_encoding.as_deref()),
            (right, self.right_encoding.as_deref()),
        ] {
            files.push(if self.active() {
                Snapshot::open_review(
                    path,
                    encoding.or(common),
                    yy_merge_core::review::Mode {
                        ignore_encoding: self.ignore_encoding,
                    },
                    c,
                )?
            } else {
                Snapshot::open_comparison(path, common, c)?
            });
        }
        files[0].ensure_comparable(&files[1])?;
        files.try_into().map_err(|_| "two inputs required".into())
    }
}
#[derive(Clone, Debug, Default, clap::Args)]
pub struct Gui {
    /// Use the disk-backed large text viewer even for small files.
    #[arg(long, requires_all=["left","right"], conflicts_with_all=["base","output","image","session","comparison_session","folder_session","git_merge"])]
    pub large: bool,
    #[command(flatten)]
    pub inputs: InputOptions,
    /// Force the dedicated image viewer (also for files with nonstandard extensions).
    #[arg(long, requires_all=["left","right"], conflicts_with_all=["base","output","encoding","session","comparison_session","folder_session","git_merge"])]
    pub image: bool,
    /// Rescan folders using saved roots, recursion and display filter.
    #[arg(long, conflicts_with_all=["session","comparison_session","left","right","base","output","encoding","rules_profile","ignore_space","ignore_trailing_space","ignore_case","ignore_eol","git_diff","git_merge"])]
    pub folder_session: Option<PathBuf>,
    /// Restore a two-way comparison session, including rules and selected positions.
    #[arg(long, conflicts_with_all=["session","left","right","base","output","encoding","rules_profile","ignore_space","ignore_trailing_space","ignore_case","ignore_eol","git_diff","git_merge"])]
    pub comparison_session: Option<PathBuf>,
    /// Restore a three-way work session after verifying its input fingerprints.
    #[arg(long, conflicts_with_all=["left","right","base","output","encoding","rules_profile","ignore_space","ignore_trailing_space","ignore_case","ignore_eol","git_diff","git_merge"])]
    pub session: Option<PathBuf>,
    #[command(flatten)]
    pub rules: RuleArgs,
    #[arg(long, requires = "right")]
    pub left: Option<PathBuf>,
    #[arg(long, requires = "left")]
    pub right: Option<PathBuf>,
    /// Display name for the left input; does not change its file path.
    #[arg(long, requires = "left", allow_hyphen_values = true)]
    pub left_label: Option<String>,
    /// Display name for the right input; does not change its file path.
    #[arg(long, requires = "right", allow_hyphen_values = true)]
    pub right_label: Option<String>,
    #[arg(long,requires_all=["left","right"])]
    pub base: Option<PathBuf>,
    #[arg(long, requires = "base")]
    pub output: Option<PathBuf>,
    #[arg(long)]
    pub encoding: Option<String>,
    #[arg(long)]
    pub wait: bool,
    #[arg(long, hide=true, requires_all=["base","left","right","output"])]
    pub git_merge: bool,
    #[arg(long,hide=true,requires_all=["left","right"],conflicts_with="git_merge")]
    pub git_diff: bool,
    #[arg(long, hide = true, requires = "git_diff")]
    pub git_left_missing: bool,
    #[arg(long, hide = true, requires = "git_diff")]
    pub git_right_missing: bool,
    #[arg(long, hide = true, requires = "git_merge")]
    pub git_merged_target: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub ui_smoke_test: Option<PathBuf>,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    Text,
    Unified,
    Json,
    Html,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum FolderFormat {
    Text,
    Json,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum ImageReport {
    Text,
    Json,
    Html,
}

struct ImageOutput {
    page: u16,
    format: ImageReport,
    report: Option<PathBuf>,
    difference: Option<PathBuf>,
    overwrite: bool,
}
struct PdfOutput {
    threshold: u8,
    format: ImageReport,
    report: Option<PathBuf>,
    overwrite: bool,
}
fn pdf_diff(
    left: PathBuf,
    right: PathBuf,
    output: PdfOutput,
    stdout: &mut impl Write,
    c: &Cancellation,
) -> Result<i32> {
    use yy_merge_core::{images::Settings, pdf_render};
    if !pdf_render::is_pdf(&left) || !pdf_render::is_pdf(&right) {
        return Err("pdf-diff requires two PDF files".into());
    }
    let pair = pdf_render::Pair::open(&left, &right, c)?;
    let mut results = Vec::new();
    let mut equivalent = true;
    let mut html = String::from(
        "<!doctype html><html lang=ja><meta charset=utf-8><meta http-equiv=Content-Security-Policy content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'\"><title>yymerge PDF比較</title><style>body{font-family:monospace}main{display:flex;gap:12px}main>section{width:33%;overflow:auto}img{max-width:100%}pre{white-space:pre-wrap}article{border-top:1px solid #888}</style><h1>yymerge PDF全ページ比較</h1>",
    );
    for number in 0..pair.pages() {
        c.check()?;
        let page = pair.page(number, c)?;
        let comparison = page.compare(
            Settings {
                threshold: output.threshold,
                ..Default::default()
            },
            c,
        )?;
        equivalent &= comparison.summary.equivalent;
        if matches!(output.format, ImageReport::Html) {
            let rendered = yy_merge_core::images::html(&page.left, &page.right, &comparison, c)?;
            let fragment = rendered
                .split_once("</h1>")
                .ok_or("image HTML header missing")?
                .1
                .trim_end()
                .trim_end_matches("</html>");
            let heading = format!(
                "<article id=page-{}><h2>Page {} / {}</h2><p>Missing LEFT={} RIGHT={}</p>",
                number + 1,
                number + 1,
                pair.pages(),
                page.missing[0],
                page.missing[1]
            );
            if html.len() + heading.len() + fragment.len() + 32 > yy_merge_core::MAX_BYTES as usize
            {
                return Err("PDF HTML report exceeds 32 MiB".into());
            }
            html.push_str(&heading);
            html.push_str(fragment);
            html.push_str("</article>");
        }
        results.push(serde_json::json!({"page":number+1,"missing":page.missing,"comparison":comparison.summary}));
    }
    let identical = pair.left.source.original_identical(&pair.right.source);
    let text = match output.format {
        ImageReport::Json => serde_json::to_string_pretty(&serde_json::json!({"mode":"pdf","pdf_rendered":true,"pages":pair.pages(),"left_pages":pair.left.pages,"right_pages":pair.right.pages,"original_identical":identical,"equivalent":equivalent,"page_results":results})).map_err(|e|e.to_string())? + "\n",
        ImageReport::Text => {
            let mut text = format!("PDF {}: {} pages (LEFT {}, RIGHT {}) | raw file identical: {identical}\n", if equivalent { "equivalent" } else { "different" }, pair.pages(), pair.left.pages, pair.right.pages);
            for row in results { text.push_str(&format!("page {}: {} | changed {} pixels | missing {}\n", row["page"], if row["comparison"]["equivalent"] == true { "equivalent" } else { "different" }, row["comparison"]["changed_pixels"], row["missing"])); }
            text
        }
        ImageReport::Html => { html.push_str("</html>"); html }
    };
    pair.left.source.verify(c)?;
    pair.right.source.verify(c)?;
    emit(
        &text,
        output.report,
        &[&pair.left.source, &pair.right.source],
        output.overwrite,
        stdout,
        c,
    )?;
    Ok(i32::from(!equivalent))
}
fn image_diff(
    left: PathBuf,
    right: PathBuf,
    settings: yy_merge_core::images::Settings,
    output: ImageOutput,
    stdout: &mut impl Write,
    cancel: &Cancellation,
) -> Result<i32> {
    use yy_merge_core::images::{self, Input};
    if !(1..=128).contains(&output.page) {
        return Err("image page must be in 1..128".into());
    }
    if yy_merge_core::office::Kind::detect(&left).is_some()
        || yy_merge_core::office::Kind::detect(&right).is_some()
    {
        return Err(
            "Excel/PowerPoint image comparison is unsupported; use diff for content comparison"
                .into(),
        );
    }
    let pdf = yy_merge_core::pdf_render::is_pdf(&left) || yy_merge_core::pdf_render::is_pdf(&right);
    let (a, b, missing, pages) = if pdf {
        let docs = yy_merge_core::pdf_render::Pair::open(&left, &right, cancel)?;
        let page = docs.page(usize::from(output.page - 1), cancel)?;
        (page.left, page.right, page.missing, docs.pages())
    } else {
        if output.page != 1 {
            return Err("raster images have only page 1".into());
        }
        (
            Input::open(&left, cancel)?,
            Input::open(&right, cancel)?,
            [false; 2],
            1,
        )
    };
    let comparison = yy_merge_core::pdf_render::compare(&a, &b, missing, settings, cancel)?;
    let original_identical = a.source.original_identical(&b.source);
    let text = match output.format {
        ImageReport::Json => serde_json::to_string_pretty(&serde_json::json!({"mode":"image","pdf_rendered":pdf,"page":output.page,"pages":pages,"missing":missing,"original_identical":original_identical,"left":a.source.path,"right":b.source.path,"comparison":comparison.summary})).map_err(|e| e.to_string())? + "\n",
        ImageReport::Text => format!("image {}: {} <> {} | {} / {} changed pixels | threshold {} | offset {},{} | raw file identical: {}\n", if comparison.summary.equivalent { "equivalent" } else { "different" }, left.display(),right.display(),comparison.summary.changed_pixels,comparison.summary.compared_pixels,settings.threshold,settings.offset_x,settings.offset_y,original_identical),
        ImageReport::Html => {
            let html = images::html(&a,&b,&comparison,cancel)?;
            if pdf { html.replacen("<main>", &format!("<p>PDF rendered page {} / {} | missing LEFT={} RIGHT={} | embedded Hayro</p><main>", output.page, pages, missing[0], missing[1]), 1) } else { html }
        },
    };
    let png = output
        .difference
        .as_ref()
        .map(|_| images::png(&comparison.difference, cancel))
        .transpose()?;
    // Validate all outputs before any write; each save is atomic, the pair is not a transaction.
    let normalize = |p: &std::path::Path| -> Result<PathBuf> {
        let parent = std::fs::canonicalize(
            p.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new(".")),
        )
        .map_err(|e| e.to_string())?;
        let target = parent.join(p.file_name().ok_or("output filename required")?);
        match std::fs::symlink_metadata(&target) {
            Ok(meta) => {
                if !meta.is_file() || folders::is_link(&target, meta.file_type())? {
                    return Err("image output must be a regular file, not a link".into());
                }
                if !output.overwrite {
                    return Err("image output exists; use --overwrite".into());
                }
                std::fs::canonicalize(target).map_err(|e| e.to_string())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(target),
            Err(e) => Err(e.to_string()),
        }
    };
    let mut destinations = Vec::new();
    for path in [output.report.as_ref(), output.difference.as_ref()]
        .into_iter()
        .flatten()
    {
        let target = normalize(path)?;
        if target == a.source.path || target == b.source.path {
            return Err("image output must differ from both input paths".into());
        }
        destinations.push(target);
    }
    if destinations.len() == 2 {
        let same = if cfg!(windows) {
            destinations[0].to_string_lossy().to_lowercase()
                == destinations[1].to_string_lossy().to_lowercase()
        } else {
            destinations[0] == destinations[1]
        };
        if same {
            return Err("image report and difference must have different output paths".into());
        }
    }
    a.source.verify(cancel)?;
    b.source.verify(cancel)?;
    if let (Some(path), Some(bytes)) = (output.difference, png) {
        save(
            &path,
            &bytes,
            &[&a.source, &b.source],
            output.overwrite,
            cancel,
        )?;
    }
    emit(
        &text,
        output.report,
        &[&a.source, &b.source],
        output.overwrite,
        stdout,
        cancel,
    )?;
    Ok(i32::from(!comparison.summary.equivalent))
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
        Command::Gui(mut g) => (|| {
            if g.inputs.active()
                && (g.base.is_some()
                    || g.git_diff
                    || g.git_merge
                    || g.session.is_some()
                    || g.comparison_session.is_some()
                    || g.folder_session.is_some()
                    || g.image)
            {
                return Err(
                    "per-file encodings/byte review require a regular two-way text comparison"
                        .into(),
                );
            }
            let (rules, _) = g.rules.resolve(&CANCEL)?;
            let has_profile = g.rules.rules_profile.is_some();
            if (g.image
                || [&g.left, &g.right]
                    .into_iter()
                    .flatten()
                    .any(|p| yy_merge_core::images::is_image(p)))
                && (rules.active() || has_profile)
            {
                return Err("image comparison cannot use text comparison rules/profiles".into());
            }
            g.rules = RuleArgs::from_core(rules);
            if (g.base.is_some() || g.git_diff || g.git_merge) && (rules.active() || has_profile) {
                Err(
                    "comparison rules/profiles are for ordinary two-way views only; merge and Git views use exact inputs"
                        .into(),
                )
            } else {
                gui(*g)
            }
        })(),
        Command::GitTool { command } => {
            git_tool::run(command, gui, &mut std::io::stdout().lock(), &CANCEL)
        }
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
    cancel.check()?;
    if let Some(path) = path {
        save(&path, text.as_bytes(), inputs, overwrite, cancel)
    } else {
        stdout.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }
}
fn folder_heading(
    filter: &folders::filter::Filter,
    matched: usize,
    total: usize,
    differences: usize,
    incomplete: usize,
) -> String {
    if !filter.active() {
        return String::new();
    }
    format!(
        "# matched {matched}/{total}; global differences {differences}; global incomplete {incomplete}; path={:?}; path_glob={:?}; status={:?}\n",
        filter.query(),
        filter.glob(),
        filter.status()
    )
}
fn folder_json(
    entries: serde_json::Value,
    filter: &folders::filter::Filter,
    total: usize,
    differences: usize,
    incomplete: usize,
) -> Result<String> {
    let text = if filter.active() {
        serde_json::to_string_pretty(&serde_json::json!({
            "version":1,"filtered":true,"filter":filter,"total_count":total,"matched_count":entries.as_array().map_or(0, Vec::len),
            "total_differences":differences,"total_incomplete":incomplete,"entries":entries
        }))
    } else {
        serde_json::to_string_pretty(&entries)
    };
    text.map(|text| text + "\n").map_err(|e| e.to_string())
}
pub fn execute(
    mut command: Command,
    stdout: &mut impl Write,
    cancel: &Cancellation,
) -> Result<i32> {
    let _remote_inputs = remote_input::prepare(&mut command, cancel)?;
    match command {
        Command::LargeDiff {
            left,
            right,
            encoding,
            inputs,
            report,
        } => large_command::run(
            &left,
            &right,
            encoding.as_deref(),
            &inputs,
            report.as_deref(),
            stdout,
            cancel,
        ),
        Command::PdfDiff {
            left,
            right,
            threshold,
            format,
            report,
            overwrite,
        } => pdf_diff(
            left,
            right,
            PdfOutput {
                threshold,
                format,
                report,
                overwrite,
            },
            stdout,
            cancel,
        ),
        Command::ImageDiff {
            left,
            right,
            page,
            threshold,
            offset_x,
            offset_y,
            format,
            report,
            diff_image,
            overwrite,
        } => image_diff(
            left,
            right,
            yy_merge_core::images::Settings {
                threshold,
                offset_x,
                offset_y,
            },
            ImageOutput {
                page,
                format,
                report,
                difference: diff_image,
                overwrite,
            },
            stdout,
            cancel,
        ),
        Command::FolderSession { command } => {
            use yy_merge_core::folder_session::Session;
            match command {
                FolderSessionCommand::Create {
                    left,
                    right,
                    base,
                    recursive,
                    output,
                    overwrite,
                    filter,
                } => {
                    let filter = filter.core()?;
                    Session::capture(
                        &left,
                        &right,
                        base.as_deref(),
                        recursive,
                        &filter,
                        None,
                        cancel,
                    )?
                    .save(&output, overwrite, cancel)?;
                }
                FolderSessionCommand::Inspect { path } => {
                    let session = Session::open(&path, cancel)?;
                    let text=serde_json::to_string_pretty(&serde_json::json!({"version":1,"kind":"yymerge-folder-session","roots_verified":true,"left":session.left,"right":session.right,"base":session.base,"recursive":session.recursive,"path_contains":session.path_contains,"path_glob":session.path_glob,"status":session.status,"selected":session.selected,"results_restored":false,"sync_plan_restored":false})).map_err(|e|e.to_string())?+"\n";
                    emit(&text, None, &[], false, stdout, cancel)?;
                }
            }
            Ok(0)
        }
        Command::ComparisonSession { command } => {
            use yy_merge_core::comparison_session::{Position, Session};
            match command {
                ComparisonSessionCommand::Create {
                    left,
                    right,
                    output,
                    encoding,
                    inputs,
                    overwrite,
                    rules,
                } => {
                    let [l, r] = inputs.open_pair(&left, &right, encoding.as_deref(), cancel)?;
                    let (rules, profile) = rules.resolve(cancel)?;
                    let state = Session::capture([&l, &r], rules, Position::default(), cancel)?;
                    let mut protected = vec![&l, &r];
                    if let Some(profile) = &profile {
                        protected.push(profile);
                    }
                    save(&output, &state.json(cancel)?, &protected, overwrite, cancel)?;
                }
                ComparisonSessionCommand::Inspect { path } => {
                    let (state, _) = Session::open(&path, cancel)?;
                    let text=serde_json::to_string_pretty(&serde_json::json!({
                        "version":if state.inputs[0].review.is_some() {3} else if state.inputs[0].office.is_some() {2} else {1},"kind":"yymerge-comparison-session","inputs_verified":true,
                        "left":state.inputs[0].path,"right":state.inputs[1].path,"rules":state.rules,
                        "position":state.position,"differences":state.comparison.diff.edits.len(),
                        "ignored":state.comparison.ignored.len(),"undo_history_restored":false,
                        "left_encoding":state.inputs[0].encoding.spec(),"right_encoding":state.inputs[1].encoding.spec(),
                        "equivalent":state.inputs[0].equivalent(&state.inputs[1],state.comparison.diff.edits.is_empty()),"byte_review":state.inputs[0].review
                    })).map_err(|e|e.to_string())?+"\n";
                    emit(&text, None, &[], false, stdout, cancel)?;
                }
            }
            Ok(0)
        }
        Command::Session { command } => {
            use yy_merge_core::session::Session;
            match command {
                SessionCommand::Create {
                    base,
                    left,
                    right,
                    output,
                    encoding,
                    overwrite,
                } => {
                    let b = Snapshot::open(&base, encoding.as_deref(), cancel)?;
                    let l = Snapshot::open(&left, encoding.as_deref(), cancel)?;
                    let r = Snapshot::open(&right, encoding.as_deref(), cancel)?;
                    let model = yy_merge_core::merge(&b.text, &l.text, &r.text, cancel)?;
                    let state = Session::capture(
                        [&b, &l, &r],
                        &model,
                        model.render(true)?,
                        false,
                        0,
                        cancel,
                    )?;
                    save(
                        &output,
                        &state.json(cancel)?,
                        &[&b, &l, &r],
                        overwrite,
                        cancel,
                    )?;
                }
                SessionCommand::Inspect { path } => {
                    let (state, _) = Session::open(&path, cancel)?;
                    let text = serde_json::to_string_pretty(&serde_json::json!({
                        "version":1,"kind":"yymerge-merge-session","inputs_verified":true,
                        "base":state.inputs[0].path,"left":state.inputs[1].path,"right":state.inputs[2].path,
                        "unresolved":state.model.unresolved(),"manual":state.manual,
                        "selected_conflict":state.selected,"result_utf8_bytes":state.result.len(),
                        "undo_history_restored":false,"merge_result_saved":false
                    })).map_err(|e|e.to_string())? + "\n";
                    emit(&text, None, &[], false, stdout, cancel)?;
                }
            }
            Ok(0)
        }
        Command::RulesProfile { command } => {
            use yy_merge_core::profile::Profile;
            match command {
                ProfileCommand::Save {
                    path,
                    rules,
                    overwrite,
                } => {
                    let (rules, input) = rules.resolve(cancel)?;
                    let inputs: Vec<_> = input.iter().collect();
                    emit(
                        &Profile::new(rules).json()?,
                        Some(path),
                        &inputs,
                        overwrite,
                        stdout,
                        cancel,
                    )?;
                }
                ProfileCommand::Show { path } => {
                    let (profile, _) = Profile::open(&path, cancel)?;
                    emit(&profile.json()?, None, &[], false, stdout, cancel)?;
                }
            }
            Ok(0)
        }
        Command::Diff {
            left,
            right,
            format,
            report,
            encoding,
            inputs,
            overwrite,
            rules,
        } => {
            let (rules, profile_input) = rules.resolve(cancel)?;
            if large_command::required(&left, &right)
                && yy_merge_core::office::Kind::detect(&left).is_none()
                && yy_merge_core::office::Kind::detect(&right).is_none()
                && !yy_merge_core::images::is_image(&left)
                && !yy_merge_core::images::is_image(&right)
                && !yy_merge_core::pdf_render::is_pdf(&left)
                && !yy_merge_core::pdf_render::is_pdf(&right)
            {
                if rules.active()
                    || profile_input.is_some()
                    || !matches!(format, Format::Text)
                    || report.is_some()
                {
                    return Err("large text uses disk comparison; use large-diff with --report for a streamed JSON index; ignore rules/unified/HTML are unavailable in this mode".into());
                }
                return large_command::run(
                    &left,
                    &right,
                    encoding.as_deref(),
                    &inputs,
                    None,
                    stdout,
                    cancel,
                );
            }
            if yy_merge_core::images::is_image(&left) || yy_merge_core::images::is_image(&right) {
                if rules.active()
                    || profile_input.is_some()
                    || encoding.is_some()
                    || inputs.active()
                {
                    return Err(
                        "image comparison cannot use text encodings or comparison rules".into(),
                    );
                }
                let format = match format {
                    Format::Text => ImageReport::Text,
                    Format::Json => ImageReport::Json,
                    Format::Html => ImageReport::Html,
                    Format::Unified => {
                        return Err("image comparison cannot generate text patches".into());
                    }
                };
                if yy_merge_core::pdf_render::is_pdf(&left)
                    || yy_merge_core::pdf_render::is_pdf(&right)
                {
                    return pdf_diff(
                        left,
                        right,
                        PdfOutput {
                            threshold: 0,
                            format,
                            report,
                            overwrite,
                        },
                        stdout,
                        cancel,
                    );
                }
                return image_diff(
                    left,
                    right,
                    Default::default(),
                    ImageOutput {
                        page: 1,
                        format,
                        report,
                        difference: None,
                        overwrite,
                    },
                    stdout,
                    cancel,
                );
            }
            if matches!(format, Format::Unified) && (rules.active() || inputs.active()) {
                return Err(
                    "unified patches require exact comparison; remove ignore options".into(),
                );
            }
            let [a, b] = inputs.open_pair(&left, &right, encoding.as_deref(), cancel)?;
            a.ensure_comparable(&b)?;
            if matches!(format, Format::Unified) && a.office.is_some() {
                return Err(
                    "Office comparison cannot generate a unified patch; use text, json or html"
                        .into(),
                );
            }
            let compared = yy_merge_core::rules::compare(&a.text, &b.text, rules, cancel)?;
            let diff = &compared.diff;
            let same = a.original_identical(&b);
            let equivalent = a.equivalent(&b, diff.edits.is_empty());
            let status = if same && equivalent {
                "Identical"
            } else if equivalent && a.office.is_some() && a.text == b.text {
                "EquivalentOfficeContent"
            } else if equivalent {
                "EquivalentUnderRules"
            } else {
                "Different"
            };
            let mut text=match format {
                Format::Html=>yy_merge_core::report::html(&a,&b,rules,&compared,cancel)?,
                Format::Json=>serde_json::to_string_pretty(&serde_json::json!({"identical":same,"office":a.office,"equivalent":equivalent,"status":status,"rules":rules,"ignored_line_pairs":compared.ignored,"left_encoding":a.encoding.spec(),"right_encoding":b.encoding.spec(),"byte_review":a.review,"left_review":a.review_info,"right_review":b.review_info,"left_bom":a.bom,"right_bom":b.bom,"diff":diff})).map_err(|e|e.to_string())?+"\n",
                Format::Text=>format!("{}: {} <> {} ({} changed regions; {} ignored line pairs; {}; {} / {})\n",if same && equivalent {"identical"} else if equivalent {if a.office.is_some() && a.text == b.text { "equivalent Office content (package bytes differ)" } else { "equivalent under rules" }} else {"different"},left.display(),right.display(),diff.edits.len(),compared.ignored.len(),rules.label(),a.encoding,b.encoding),
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
            if matches!(format, Format::Text) && a.office.is_some() {
                let changes = yy_merge_core::office::changes(&a.text, &b.text, diff, cancel)?;
                if text.len() + changes.len() > yy_merge_core::MAX_BYTES as usize {
                    return Err("Office text report exceeds 32 MiB".into());
                }
                text.push_str(&changes);
            }
            a.verify(cancel)?;
            b.verify(cancel)?;
            let mut inputs = vec![&a, &b];
            inputs.extend(profile_input.iter());
            emit(&text, report, &inputs, overwrite, stdout, cancel)?;
            Ok(i32::from(!equivalent))
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
            base,
            recursive,
            format,
            report,
            overwrite,
            filter,
        } => {
            use folders::filter::{Facts, path_key};
            let filter = filter.core()?;
            if let Some(path) = &report {
                let parent = std::fs::canonicalize(
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(std::path::Path::new(".")),
                )
                .map_err(|e| e.to_string())?;
                for root in [&left, &right].into_iter().chain(base.as_ref()) {
                    if parent.starts_with(std::fs::canonicalize(root).map_err(|e| e.to_string())?) {
                        return Err("folder report must be outside all input trees".into());
                    }
                }
            }
            if let Some(base) = base {
                let entries = folders::three::compare(&base, &left, &right, recursive, cancel)?;
                let incomplete = entries.iter().filter(|e| e.incomplete()).count();
                let differences = entries
                    .iter()
                    .filter(|e| !e.incomplete() && e.status != folders::three::Status::Unchanged)
                    .count();
                let mut visible = Vec::new();
                for (i, e) in entries.iter().enumerate() {
                    if i & 1023 == 0 {
                        cancel.check()?;
                    }
                    if !filter.active()
                        || filter.matches(
                            &path_key(&e.path),
                            Facts::three(e.status, e.left.is_some(), e.right.is_some()),
                        )
                    {
                        visible.push(e);
                    }
                }
                let code = if entries.iter().any(|e| e.incomplete()) {
                    2
                } else {
                    i32::from(
                        entries
                            .iter()
                            .any(|e| e.status != folders::three::Status::Unchanged),
                    )
                };
                let text = match format {
                    FolderFormat::Json => {
                        if filter.active() {
                            folder_json(
                                serde_json::json!(&visible),
                                &filter,
                                entries.len(),
                                differences,
                                incomplete,
                            )?
                        } else {
                            serde_json::to_string_pretty(&visible).map_err(|e| e.to_string())?
                                + "\n"
                        }
                    }
                    FolderFormat::Text => {
                        folder_heading(
                            &filter,
                            visible.len(),
                            entries.len(),
                            differences,
                            incomplete,
                        ) + &visible
                            .iter()
                            .map(|e| {
                                format!(
                                    "{:?}\t{}{}\n",
                                    e.status,
                                    e.path.display(),
                                    e.detail
                                        .as_ref()
                                        .map(|d| format!("\t{d}"))
                                        .unwrap_or_default()
                                )
                            })
                            .collect::<String>()
                    }
                };
                emit(&text, report, &[], overwrite, stdout, cancel)?;
                return Ok(code);
            }
            let entries = folders::compare(&left, &right, recursive, cancel)?;
            let incomplete = entries
                .iter()
                .filter(|e| Facts::two(&e.status).incomplete)
                .count();
            let differences = entries
                .iter()
                .filter(|e| {
                    let f = Facts::two(&e.status);
                    !f.incomplete && !f.equal
                })
                .count();
            let same = entries
                .iter()
                .all(|e| matches!(e.status, folders::Status::Equal));
            if !filter.active()
                && entries.iter().any(|e| {
                    matches!(
                        e.status,
                        folders::Status::SkippedLink | folders::Status::Error
                    )
                })
            {
                return Err("folder comparison incomplete: links/reparse points skipped or files could not be read".into());
            }
            let mut visible = Vec::new();
            for (i, e) in entries.iter().enumerate() {
                if i & 1023 == 0 {
                    cancel.check()?;
                }
                if !filter.active() || filter.matches(&path_key(&e.path), Facts::two(&e.status)) {
                    visible.push(e);
                }
            }
            let text = match format {
                FolderFormat::Json => {
                    if filter.active() {
                        folder_json(
                            serde_json::json!(&visible),
                            &filter,
                            entries.len(),
                            differences,
                            incomplete,
                        )?
                    } else {
                        serde_json::to_string_pretty(&visible).map_err(|e| e.to_string())? + "\n"
                    }
                }
                FolderFormat::Text => {
                    folder_heading(
                        &filter,
                        visible.len(),
                        entries.len(),
                        differences,
                        incomplete,
                    ) + &visible
                        .iter()
                        .map(|e| format!("{:?}\t{}\n", e.status, e.path.display()))
                        .collect::<String>()
                }
            };
            emit(&text, report, &[], overwrite, stdout, cancel)?;
            Ok(if incomplete > 0 { 2 } else { i32::from(!same) })
        }
        Command::GitTool { .. } => Err("Git adapter must be dispatched with the GUI host".into()),
        Command::Gui(_) => Err("GUI must be dispatched by the application".into()),
        Command::Sync {
            command:
                SyncCommand::Apply {
                    plan,
                    execute,
                    run_dir,
                },
        } => {
            let plan = yy_merge_core::sync_run::read_plan(&plan, cancel)?;
            if !execute {
                yy_merge_core::sync_run::validate(&plan, cancel)?;
                stdout.write_all(b"Plan validated; no files changed. Use --execute --run-dir <new-directory> to apply.\n").map_err(|e| e.to_string())?;
                return Ok(0);
            }
            let report = yy_merge_core::sync_run::apply(
                plan,
                &run_dir.ok_or("missing run directory")?,
                cancel,
            )?;
            stdout
                .write_all(report.json()?.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(report.exit_code())
        }
        Command::Sync {
            command:
                SyncCommand::Resume {
                    run_dir,
                    execute: _,
                },
        } => {
            let report = yy_merge_core::sync_run::resume(&run_dir, cancel)?;
            stdout
                .write_all(report.json()?.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(report.exit_code())
        }
        Command::Sync {
            command:
                SyncCommand::Rollback {
                    run_dir,
                    execute: _,
                },
        } => {
            let report = yy_merge_core::sync_run::rollback(&run_dir, cancel)?;
            stdout
                .write_all(report.json()?.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(report.exit_code())
        }
        Command::Sync {
            command:
                SyncCommand::Plan {
                    left,
                    right,
                    base,
                    mode,
                    output,
                    overwrite,
                },
        } => {
            let plan = yy_merge_core::sync_plan::build(
                &left,
                &right,
                base.as_deref(),
                mode.into(),
                cancel,
            )?;
            if let Some(path) = &output {
                plan.validate_report_path(path)?;
            }
            let code = plan.exit_code();
            emit(&plan.report()?, output, &[], overwrite, stdout, cancel)?;
            Ok(code)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn editor_snapshot_labels_are_literal_and_require_inputs() {
        let label = "--未保存 \"作業ツリー\"";
        assert_eq!(
            super::entry(
                [
                    "yymerge",
                    "gui",
                    "--left",
                    "left.txt",
                    "--right",
                    "right.txt",
                    "--encoding",
                    "UTF-8",
                    "--left-label",
                    label,
                    "--right-label",
                    "HEAD"
                ]
                .map(std::ffi::OsString::from),
                |g| {
                    assert_eq!(g.left_label.as_deref(), Some(label));
                    assert_eq!(g.right_label.as_deref(), Some("HEAD"));
                    assert_eq!(g.encoding.as_deref(), Some("UTF-8"));
                    Ok(0)
                }
            ),
            0
        );
        assert_ne!(
            super::entry(
                ["yymerge", "gui", "--left-label", "missing"].map(std::ffi::OsString::from),
                |_| panic!("missing inputs accepted")
            ),
            0
        );
    }

    #[test]
    fn folder_session_gui_dispatch_preserves_unicode_path() {
        assert_eq!(
            super::entry(
                ["yymerge", "gui", "--folder-session", "比較 folder.json"]
                    .map(std::ffi::OsString::from),
                |g| {
                    assert_eq!(
                        g.folder_session,
                        Some(std::path::PathBuf::from("比較 folder.json"))
                    );
                    assert!(
                        g.left.is_none() && g.session.is_none() && g.comparison_session.is_none()
                    );
                    Ok(0)
                }
            ),
            0
        );
    }
    #[test]
    fn comparison_session_gui_dispatch_preserves_unicode_path() {
        assert_eq!(
            super::entry(
                [
                    "yymerge",
                    "gui",
                    "--comparison-session",
                    "比較 session.json"
                ]
                .map(std::ffi::OsString::from),
                |g| {
                    assert_eq!(
                        g.comparison_session,
                        Some(std::path::PathBuf::from("比較 session.json"))
                    );
                    assert!(g.session.is_none() && g.left.is_none() && g.output.is_none());
                    Ok(0)
                }
            ),
            0
        );
    }
    use super::*;
    #[test]
    fn session_gui_dispatch_preserves_path_and_rejects_conflicting_inputs() {
        assert_eq!(
            entry(
                ["yymerge", "gui", "--session", "作業 session.json"].map(OsString::from),
                |g| {
                    assert_eq!(g.session, Some(PathBuf::from("作業 session.json")));
                    assert!(g.left.is_none() && g.base.is_none() && g.output.is_none());
                    Ok(0)
                }
            ),
            0
        );
        for extra in [
            vec!["--ignore-case"],
            vec!["--left", "left", "--right", "right"],
            vec!["--rules-profile", "rules.json"],
        ] {
            let mut args = vec!["yymerge", "gui", "--session", "session.json"];
            args.extend(extra);
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
    #[test]
    fn gui_resolves_profiles_before_dispatch_and_blocks_non_two_way_modes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.json");
        std::fs::write(
            &path,
            yy_merge_core::profile::Profile::new(yy_merge_core::rules::Rules {
                ignore_case: true,
                ..Default::default()
            })
            .json()
            .unwrap(),
        )
        .unwrap();
        let args = vec![
            "yymerge".into(),
            "gui".into(),
            "--rules-profile".into(),
            path.clone().into_os_string(),
        ];
        assert_eq!(
            entry(args.clone(), |g| {
                assert!(g.rules.ignore_case);
                assert!(g.rules.rules_profile.is_none());
                Ok(0)
            }),
            0
        );
        for git in [false, true] {
            // Even an exact profile is forbidden in dedicated Git and merge views.
            std::fs::write(
                &path,
                yy_merge_core::profile::Profile::new(Default::default())
                    .json()
                    .unwrap(),
            )
            .unwrap();
            let mut args = args.clone();
            args.extend(["--left", "left", "--right", "right"].map(OsString::from));
            if git {
                args.push("--git-diff".into());
            } else {
                args.extend(["--base", "base"].map(OsString::from));
            }
            assert_eq!(
                entry(args, |_| panic!("invalid profile mode launched GUI")),
                2
            );
        }
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(entry(args, |_| panic!("invalid profile launched GUI")), 2);
    }
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
