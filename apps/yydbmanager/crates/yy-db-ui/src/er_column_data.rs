use yy_db_core::schema::{Entity, Field};
pub const HEADERS: [&str; 6] = [
    "物理名",
    "論理名",
    "データ型",
    "NULL許可",
    "既定値(SQL)",
    "主要項目",
];
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub original: Option<usize>,
    pub cells: [String; 6],
}
impl Row {
    pub fn blank() -> Self {
        Self {
            original: None,
            cells: Default::default(),
        }
    }
}
pub fn from_entity(e: &Entity) -> Vec<Row> {
    e.fields
        .iter()
        .enumerate()
        .map(|(i, f)| Row {
            original: Some(i),
            cells: [
                f.name.clone(),
                f.logical_name.clone(),
                f.data_type.clone(),
                f.nullable.to_string(),
                f.default_sql.clone().unwrap_or_default(),
                f.important.to_string(),
            ],
        })
        .collect()
}
fn boolean(s: &str) -> Result<bool, String> {
    match s.trim().to_lowercase().as_str() {
        "" | "false" | "0" | "no" | "いいえ" | "無効" => Ok(false),
        "true" | "1" | "yes" | "はい" | "有効" | "○" | "〇" | "✓" => Ok(true),
        _ => Err("true / false または 1 / 0 を指定してください".into()),
    }
}
pub fn fields(rows: &[Row]) -> Result<Vec<(Option<usize>, Field)>, String> {
    let end = rows
        .iter()
        .rposition(|r| r.original.is_some() || r.cells.iter().any(|s| !s.is_empty()))
        .map_or(0, |i| i + 1);
    rows[..end]
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let error = |c: usize, e: String| format!("{}行目・{}: {e}", i + 1, HEADERS[c]);
            Ok((
                r.original,
                Field {
                    name: r.cells[0].trim().into(),
                    logical_name: r.cells[1].clone(),
                    data_type: r.cells[2].trim().into(),
                    nullable: boolean(&r.cells[3]).map_err(|e| error(3, e))?,
                    default_sql: (!r.cells[4].is_empty()).then(|| r.cells[4].clone()),
                    important: boolean(&r.cells[5]).map_err(|e| error(5, e))?,
                },
            ))
        })
        .collect()
}
pub fn copy(rows: &[Row], rect: (usize, usize, usize, usize)) -> String {
    let mut w = csv::WriterBuilder::new()
        .delimiter(b'\t')
        .terminator(csv::Terminator::CRLF)
        .from_writer(vec![]);
    for row in &rows[rect.0..=rect.2] {
        w.write_record(&row.cells[rect.1..=rect.3])
            .expect("memory writer");
    }
    String::from_utf8(w.into_inner().expect("memory writer")).expect("UTF-8 records")
}
/// Parse and bounds-check the entire paste before changing the edit buffer.
pub fn paste(
    rows: &mut Vec<Row>,
    row: usize,
    col: usize,
    tsv: &str,
) -> Result<(usize, usize), String> {
    if tsv.len() > 8 * 1024 * 1024 {
        return Err("貼り付けは8MiBまでです".into());
    }
    let data = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .flexible(true)
        .from_reader(tsv.as_bytes())
        .records()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if data.is_empty() {
        return Ok((row, col));
    }
    let width = data.iter().map(|r| r.len()).max().unwrap_or(0);
    if col + width > 6 || row + data.len() > 4096 {
        return Err("貼り付け先は6項目・4096行以内にしてください".into());
    }
    rows.resize_with(rows.len().max(row + data.len()), Row::blank);
    for (i, record) in data.iter().enumerate() {
        for (j, value) in record.iter().enumerate() {
            rows[row + i].cells[col + j] = value.into();
        }
    }
    Ok((row + data.len() - 1, col + width - 1))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excel_rectangles_preserve_quotes_newlines_and_identity() {
        let mut rows = vec![Row {
            original: Some(7),
            cells: [
                "id".into(),
                "a\tb\n\"c\"".into(),
                "INTEGER".into(),
                "0".into(),
                "'x\ny'".into(),
                "1".into(),
            ],
        }];
        let clip = copy(&rows, (0, 0, 0, 5));
        paste(&mut rows, 1, 0, &clip).unwrap();
        assert_eq!(rows[0].cells, rows[1].cells);
        assert_eq!(rows[0].original, Some(7));
        assert_eq!(rows[1].original, None);
        let out = fields(&rows).unwrap();
        assert!(!out[0].1.nullable && out[0].1.important);
        assert_eq!(out[1].1.default_sql.as_deref(), Some("'x\ny'"));
        let before = rows.clone();
        assert!(paste(&mut rows, 0, 5, "a\tb").is_err());
        assert_eq!(rows, before);
        assert!(paste(&mut rows, 4095, 0, "a\nb").is_err());
        rows[0].cells[3] = "maybe".into();
        assert!(fields(&rows).unwrap_err().contains("1行目・NULL許可"));
    }
}
