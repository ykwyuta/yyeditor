use crate::InputOptions;
use std::{
    io::{BufWriter, Write},
    path::Path,
    sync::atomic::AtomicU64,
};
use yy_merge_core::{
    Cancellation, Result,
    large::{Input, compare_inputs},
};

pub fn required(left: &Path, right: &Path) -> bool {
    [left, right]
        .iter()
        .any(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > yy_merge_core::MAX_BYTES))
}

pub fn run(
    left: &Path,
    right: &Path,
    encoding: Option<&str>,
    options: &InputOptions,
    report: Option<&Path>,
    stdout: &mut impl Write,
    cancel: &Cancellation,
) -> Result<i32> {
    if encoding.is_some() && (options.left_encoding.is_some() || options.right_encoding.is_some()) {
        return Err("use a common encoding or per-file encodings".into());
    }
    if [left, right].iter().any(|p| {
        yy_merge_core::office::Kind::detect(p).is_some() || yy_merge_core::images::is_image(p)
    }) {
        return Err(
            "large-diff is for text/data files; Office/image limits remain unchanged".into(),
        );
    }
    let a = Input::open(
        left,
        options.left_encoding.as_deref().or(encoding),
        cancel.flag(),
    )
    .map_err(|e| e.to_string())?;
    let b = Input::open(
        right,
        options.right_encoding.as_deref().or(encoding),
        cancel.flag(),
    )
    .map_err(|e| e.to_string())?;
    let index = compare_inputs(&a, &b, cancel.flag(), &AtomicU64::new(0))
        .map_err(|e| e.to_string())?
        .ok_or("cancelled")?;
    let same = index.changes == 0
        && (options.ignore_encoding || a.encoding == b.encoding && a.bom == b.bom);
    if let Some(path) = report {
        if path.to_str().is_some_and(|s| s.starts_with("ssh:")) {
            return Err("report output must be local".into());
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        {
            let mut writer = BufWriter::new(temp.as_file_mut());
            write!(writer,"{{\"mode\":\"disk\",\"equivalent\":{same},\"row_count\":{},\"changed_rows\":{},\"rows\":[",index.rows,index.changes).map_err(|e|e.to_string())?;
            let mut reader = index.open().map_err(|e| e.to_string())?;
            let mut first = true;
            for start in (0..index.rows).step_by(1024) {
                cancel.check()?;
                for row in index
                    .read_rows(&mut reader, start, 1024)
                    .map_err(|e| e.to_string())?
                {
                    if !first {
                        writer.write_all(b",").map_err(|e| e.to_string())?;
                    }
                    first = false;
                    let optional = |v: u64| {
                        if v == yy_merge_core::large::NONE {
                            "null".into()
                        } else {
                            v.to_string()
                        }
                    };
                    write!(writer,"{{\"kind\":\"{:?}\",\"left_offset\":{},\"right_offset\":{},\"left_line\":{},\"right_line\":{},\"left_length\":{},\"right_length\":{}}}",row.kind,optional(row.left),optional(row.right),optional(row.left_line),optional(row.right_line),row.left_len,row.right_len).map_err(|e|e.to_string())?;
                }
            }
            writer
                .write_all(b"]}\n")
                .and_then(|_| writer.flush())
                .map_err(|e| e.to_string())?;
        }
        cancel.check()?;
        temp.persist_noclobber(path).map_err(|e| e.to_string())?;
    }
    writeln!(
        stdout,
        "{} (disk index): {} rows, {} changed rows",
        if same { "identical" } else { "different" },
        index.rows,
        index.changes
    )
    .map_err(|e| e.to_string())?;
    Ok(i32::from(!same))
}
