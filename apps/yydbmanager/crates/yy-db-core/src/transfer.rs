use crate::{Column, Result, ResultSet, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct Delimited {
    pub delimiter: char,
    pub header: bool,
    pub null: String,
}
impl Default for Delimited {
    fn default() -> Self {
        Self {
            delimiter: ',',
            header: true,
            null: "\\N".into(),
        }
    }
}
impl Delimited {
    fn validate(&self) -> Result<()> {
        if matches!(self.delimiter, '\r' | '\n' | '"')
            || self.null.contains([self.delimiter, '\r', '\n', '"'])
            || self.null.is_empty()
        {
            return Err("区切り文字またはNULL表現が不正です".into());
        }
        Ok(())
    }
}
fn atomic_write(path: &Path, f: impl FnOnce(&mut std::fs::File) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    f(temporary.as_file_mut())?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn export_delimited(set: &ResultSet, path: &Path, profile: &Delimited) -> Result<()> {
    profile.validate()?;
    atomic_write(path, |file| {
        let mut w = std::io::BufWriter::new(file);
        let field = |w: &mut std::io::BufWriter<&mut std::fs::File>,
                     text: &str,
                     force: bool|
         -> Result<()> {
            if force || text.contains([profile.delimiter, '"', '\r', '\n']) {
                write!(w, "\"{}\"", text.replace('"', "\"\"")).map_err(|e| e.to_string())
            } else {
                w.write_all(text.as_bytes()).map_err(|e| e.to_string())
            }
        };
        if profile.header {
            for (i, c) in set.columns.iter().enumerate() {
                if i > 0 {
                    write!(w, "{}", profile.delimiter).map_err(|e| e.to_string())?;
                }
                field(&mut w, &c.name, false)?;
            }
            w.write_all(b"\r\n").map_err(|e| e.to_string())?;
        }
        let mut start = 0;
        while start < set.row_count {
            for row in set.page(start, 512)? {
                for (i, v) in row.iter().enumerate() {
                    if i > 0 {
                        write!(w, "{}", profile.delimiter).map_err(|e| e.to_string())?;
                    }
                    let text = if *v == Value::Null {
                        profile.null.clone()
                    } else {
                        v.display()
                    };
                    field(&mut w, &text, *v != Value::Null && text == profile.null)?;
                }
                w.write_all(b"\r\n").map_err(|e| e.to_string())?;
            }
            start += 512;
        }
        w.flush().map_err(|e| e.to_string())
    })
}

/// Read one logical record, preserving quote state for NULL-marker disambiguation.
fn record(reader: &mut impl BufRead, delimiter: char) -> Result<Option<Vec<(String, bool)>>> {
    let mut row = vec![];
    let mut value = String::new();
    let mut state = 0;
    let mut quoted = false;
    let mut any = false;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            if state == 2 {
                return Err("引用符が閉じていません".into());
            }
            if !any {
                return Ok(None);
            }
            row.push((value, quoted));
            return Ok(Some(row));
        }
        any = true;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if state == 2 {
                if c == '"' {
                    state = 3;
                } else {
                    value.push(c);
                }
                continue;
            }
            if state == 3 && c == '"' {
                value.push('"');
                state = 2;
                continue;
            }
            if c == delimiter {
                row.push((std::mem::take(&mut value), quoted));
                quoted = false;
                state = 0;
                continue;
            }
            if c == '\r' || c == '\n' {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                if chars.next().is_some() {
                    return Err("レコード区切りが不正です".into());
                }
                row.push((value, quoted));
                return Ok(Some(row));
            }
            if state == 3 {
                return Err("閉じ引用符の後に区切り以外の文字があります".into());
            }
            if c == '"' {
                if state != 0 {
                    return Err("引用されていない項目内の引用符です".into());
                }
                quoted = true;
                state = 2;
            } else {
                value.push(c);
                state = 1;
            }
        }
    }
}
pub fn read_delimited(path: &Path, profile: &Delimited) -> Result<ResultSet> {
    profile.validate()?;
    let mut r = BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let Some(mut first) = record(&mut r, profile.delimiter)? else {
        return ResultSet::new(vec![]);
    };
    if let Some((name, _)) = first.first_mut() {
        *name = name.trim_start_matches('\u{feff}').to_owned();
    }
    let columns = first
        .iter()
        .enumerate()
        .map(|(i, (name, _))| Column {
            name: if profile.header {
                name.clone()
            } else {
                format!("列{}", i + 1)
            },
            type_name: "TEXT".into(),
        })
        .collect();
    let mut set = ResultSet::new(columns)?;
    let values = |r: Vec<(String, bool)>| {
        r.into_iter()
            .map(|(s, q)| {
                if !q && s == profile.null {
                    Value::Null
                } else {
                    Value::Text(s)
                }
            })
            .collect::<Vec<_>>()
    };
    if !profile.header {
        set.push(&values(first))?;
    }
    while let Some(row) = record(&mut r, profile.delimiter)? {
        set.push(&values(row))?;
    }
    set.finish()?;
    Ok(set)
}

pub fn export_fixed(
    set: &ResultSet,
    path: &Path,
    layout: &yy_cobol::Layout,
    codec: yy_cobol::Codec,
    separator: &[u8],
) -> Result<()> {
    if set.columns.len() != layout.fields.len() {
        return Err("DB列数とコピー句の項目数が一致しません".into());
    }
    atomic_write(path, |file| {
        let mut w = std::io::BufWriter::new(file);
        let mut start = 0;
        while start < set.row_count {
            for row in set.page(start, 512)? {
                let mut record = vec![codec.charset.space(); layout.record_len];
                for (field, value) in layout.fields.iter().zip(row) {
                    if value == Value::Null {
                        return Err(format!(
                            "{}: 固定長出力ではNULL代替を明示してください",
                            field.name
                        ));
                    }
                    let text = value.display();
                    let mut issues = yy_cobol::Issues::default();
                    codec.encode(
                        field,
                        yy_cobol::Input::Text(&text),
                        &mut record[field.offset..field.offset + field.len],
                        &mut issues,
                    );
                    if issues.total() > 0 {
                        return Err(format!(
                            "{}: 桁あふれ・文字変換等の問題 {:?}",
                            field.name, issues
                        ));
                    }
                    if let yy_cobol::Decoded::Num(decoded) =
                        codec.decode(field, &record[field.offset..field.offset + field.len])
                    {
                        let original =
                            yy_cobol::Decimal::parse(&text).ok_or("数値を正確に解釈できません")?;
                        if decoded.rescale(original.scale) != Some(original) {
                            return Err(format!(
                                "{}: 小数桁の切り捨ては許可されていません",
                                field.name
                            ));
                        }
                    }
                }
                w.write_all(&record)
                    .and_then(|_| w.write_all(separator))
                    .map_err(|e| e.to_string())?;
            }
            start += 512;
        }
        w.flush().map_err(|e| e.to_string())
    })
}
pub fn read_fixed(
    path: &Path,
    layout: &yy_cobol::Layout,
    codec: yy_cobol::Codec,
    separator: &[u8],
) -> Result<ResultSet> {
    use std::io::Read;
    if layout.record_len == 0 || layout.record_len > 64 * 1024 * 1024 {
        return Err("不正なレコード長です".into());
    }
    let mut reader = BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let mut set = ResultSet::new(
        layout
            .fields
            .iter()
            .map(|f| Column {
                name: f.name.clone(),
                type_name: f.describe.clone(),
            })
            .collect(),
    )?;
    loop {
        let mut data = vec![0; layout.record_len];
        if reader.read(&mut data[..1]).map_err(|e| e.to_string())? == 0 {
            break;
        }
        reader
            .read_exact(&mut data[1..])
            .map_err(|e| e.to_string())?;
        let mut sep = vec![0; separator.len()];
        reader.read_exact(&mut sep).map_err(|e| e.to_string())?;
        if sep != separator {
            return Err("レコード区切りが一致しません".into());
        }
        let mut row = vec![];
        for f in &layout.fields {
            use yy_cobol::Decoded;
            row.push(match codec.decode(f, &data[f.offset..f.offset + f.len]) {
                Decoded::Empty => Value::Text(String::new()),
                Decoded::Text(s) => Value::Text(s),
                Decoded::Num(n) => Value::Decimal(n.to_string()),
                Decoded::Float(n) => Value::Float(n),
                Decoded::Invalid => return Err(format!("{}: 不正な固定長データです", f.name)),
            });
        }
        set.push(&row)?;
    }
    set.finish()?;
    Ok(set)
}

pub fn export_parquet(set: &ResultSet, path: &Path) -> Result<()> {
    use duckdb::arrow::{
        array::*,
        datatypes::{DataType, Field, Schema},
        record_batch::RecordBatch,
    };
    use parquet::arrow::ArrowWriter;
    let mut samples = vec![Value::Null; set.columns.len()];
    let mut scales = vec![0; set.columns.len()];
    let mut start = 0;
    while start < set.row_count {
        for row in set.page(start, 512)? {
            for (i, v) in row.into_iter().enumerate() {
                if v != Value::Null {
                    if let Value::Decimal(s) = &v {
                        scales[i] =
                            scales[i].max(s.split_once('.').map(|(_, s)| s.len()).unwrap_or(0));
                    }
                    if samples[i] == Value::Null {
                        samples[i] = v;
                    } else if std::mem::discriminant(&samples[i]) != std::mem::discriminant(&v) {
                        if matches!(
                            (&samples[i], &v),
                            (Value::Integer(_), Value::Decimal(_))
                                | (Value::Decimal(_), Value::Integer(_))
                        ) {
                            if matches!(samples[i], Value::Integer(_)) {
                                samples[i] = Value::Decimal(samples[i].display());
                            }
                        } else {
                            return Err(format!(
                                "{}: 列内で型が混在しています。明示的にSQLで型変換してください",
                                set.columns[i].name
                            ));
                        }
                    }
                }
            }
        }
        start += 512;
    }
    let types = samples
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            Value::Integer(_) => DataType::Decimal128(38, 0),
            Value::Decimal(_) => DataType::Decimal128(38, scales[i] as i8),
            Value::Float(_) => DataType::Float64,
            Value::Bool(_) => DataType::Boolean,
            Value::Binary(_) => DataType::Binary,
            _ => DataType::Utf8,
        })
        .collect::<Vec<_>>();
    let schema = Arc::new(Schema::new(
        set.columns
            .iter()
            .zip(&types)
            .map(|(c, t)| Field::new(&c.name, t.clone(), true))
            .collect::<Vec<_>>(),
    ));
    atomic_write(path, |file| {
        let mut writer =
            ArrowWriter::try_new(file, schema.clone(), None).map_err(|e| e.to_string())?;
        let mut start = 0;
        while start < set.row_count {
            let rows = set.page(start, 4096)?;
            let mut arrays: Vec<ArrayRef> = vec![];
            for (col, t) in types.iter().enumerate() {
                let vs = rows.iter().map(|r| &r[col]).collect::<Vec<_>>();
                let data: ArrayRef = match t {
                    DataType::Decimal128(_, scale) => {
                        let values = vs
                            .iter()
                            .map(|v| {
                                if **v == Value::Null {
                                    Ok(None)
                                } else {
                                    let text = v.display();
                                    let d = yy_cobol::Decimal::parse(&text).ok_or_else(|| {
                                        format!("Parquet DECIMALに変換できません: {text}")
                                    })?;
                                    Ok(Some(
                                        d.rescale(*scale as i32).ok_or("DECIMALの桁あふれ")?.value,
                                    ))
                                }
                            })
                            .collect::<Result<Vec<_>>>()?;
                        Arc::new(
                            Decimal128Array::from(values)
                                .with_precision_and_scale(38, *scale)
                                .map_err(|e| e.to_string())?,
                        )
                    }
                    DataType::Boolean => Arc::new(BooleanArray::from(
                        vs.iter()
                            .map(|v| match v {
                                Value::Bool(b) => Some(*b),
                                _ => None,
                            })
                            .collect::<Vec<_>>(),
                    )),
                    DataType::Float64 => Arc::new(Float64Array::from(
                        vs.iter()
                            .map(|v| match v {
                                Value::Float(n) => Some(*n),
                                _ => None,
                            })
                            .collect::<Vec<_>>(),
                    )),
                    DataType::Binary => {
                        let mut b = BinaryBuilder::new();
                        for v in vs {
                            match v {
                                Value::Binary(v) => b.append_value(v),
                                Value::Null => b.append_null(),
                                _ => return Err("列内の型が混在しています".into()),
                            }
                        }
                        Arc::new(b.finish())
                    }
                    _ => Arc::new(StringArray::from(
                        vs.iter()
                            .map(|v| {
                                if **v == Value::Null {
                                    None
                                } else {
                                    Some(v.display())
                                }
                            })
                            .collect::<Vec<_>>(),
                    )),
                };
                arrays.push(data);
            }
            writer
                .write(&RecordBatch::try_new(schema.clone(), arrays).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            start += 4096;
        }
        writer.close().map_err(|e| e.to_string())?;
        Ok(())
    })
}
pub fn read_parquet(path: &Path) -> Result<ResultSet> {
    use duckdb::arrow::{
        array::{Array, BinaryArray},
        datatypes::DataType,
        util::display::array_value_to_string,
    };
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        std::fs::File::open(path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut set = ResultSet::new(
        reader
            .schema()
            .fields()
            .iter()
            .map(|f| Column {
                name: f.name().clone(),
                type_name: f.data_type().to_string(),
            })
            .collect(),
    )?;
    for batch in reader
        .with_batch_size(4096)
        .build()
        .map_err(|e| e.to_string())?
    {
        let batch = batch.map_err(|e| e.to_string())?;
        for row in 0..batch.num_rows() {
            let mut values = vec![];
            for a in batch.columns() {
                let text = || array_value_to_string(a.as_ref(), row).map_err(|e| e.to_string());
                values.push(if a.is_null(row) {
                    Value::Null
                } else {
                    match a.data_type() {
                        DataType::Binary => Value::Binary(
                            a.as_any()
                                .downcast_ref::<BinaryArray>()
                                .unwrap()
                                .value(row)
                                .to_vec(),
                        ),
                        DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => {
                            Value::Decimal(text()?)
                        }
                        DataType::Int8
                        | DataType::Int16
                        | DataType::Int32
                        | DataType::Int64
                        | DataType::UInt8
                        | DataType::UInt16
                        | DataType::UInt32
                        | DataType::UInt64 => Value::Integer(text()?),
                        DataType::Boolean => Value::Bool(text()? == "true"),
                        DataType::Float32 | DataType::Float64 => {
                            Value::Float(text()?.parse::<f64>().map_err(|e| e.to_string())?)
                        }
                        _ => Value::Text(text()?),
                    }
                });
            }
            set.push(&values)?;
        }
    }
    set.finish()?;
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_packed_decimal_is_exact_and_failed_export_preserves_file() {
        let layout =
            yy_cobol::parse("       01 RECORD-A.\n          05 AMOUNT PIC S9(18)V99 COMP-3.\n")
                .unwrap();
        let codec = yy_cobol::Codec::new(yy_cobol::Charset::Ms932);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.dat");
        let mut set = ResultSet::new(vec![Column {
            name: "AMOUNT".into(),
            type_name: "DECIMAL".into(),
        }])
        .unwrap();
        set.push(&[Value::Decimal("123456789012345678.90".into())])
            .unwrap();
        set.finish().unwrap();
        export_fixed(&set, &path, &layout, codec, &[]).unwrap();
        assert_eq!(
            read_fixed(&path, &layout, codec, &[])
                .unwrap()
                .page(0, 1)
                .unwrap()[0][0],
            Value::Decimal("123456789012345678.90".into())
        );
        let original = std::fs::read(&path).unwrap();
        let mut bad = ResultSet::new(set.columns.clone()).unwrap();
        bad.push(&[Value::Decimal("1.234".into())]).unwrap();
        bad.finish().unwrap();
        assert!(export_fixed(&bad, &path, &layout, codec, &[]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    #[test]
    fn delimited_null_empty_quotes_newlines_and_unicode_separator() {
        for delimiter in [',', '\t', '｜'] {
            let p = Delimited {
                delimiter,
                ..Default::default()
            };
            let temp = tempfile::tempdir().unwrap();
            let file = temp.path().join("test.csv");
            let mut set = ResultSet::new(
                (0..5)
                    .map(|i| Column {
                        name: format!("c{i}"),
                        type_name: "TEXT".into(),
                    })
                    .collect(),
            )
            .unwrap();
            let row = vec![
                Value::Null,
                Value::Text("".into()),
                Value::Text("\\N".into()),
                Value::Text("日本\r\n語\"".into()),
                Value::Text(format!("a{delimiter}b")),
            ];
            set.push(&row).unwrap();
            set.finish().unwrap();
            export_delimited(&set, &file, &p).unwrap();
            let read = read_delimited(&file, &p).unwrap();
            assert_eq!(read.page(0, 1).unwrap()[0], row);
        }
    }
    #[test]
    fn parquet_preserves_exact_decimal_and_binary() {
        let temp = tempfile::tempdir().unwrap();
        let mut set = ResultSet::new(vec![
            Column {
                name: "n".into(),
                type_name: "DECIMAL".into(),
            },
            Column {
                name: "b".into(),
                type_name: "BLOB".into(),
            },
        ])
        .unwrap();
        let row = vec![
            Value::Decimal("123456789012345678901234567890.12345678".into()),
            Value::Binary(vec![0, 255]),
        ];
        set.push(&row).unwrap();
        set.push(&[Value::Null, Value::Null]).unwrap();
        set.finish().unwrap();
        let path = temp.path().join("t.parquet");
        export_parquet(&set, &path).unwrap();
        assert_eq!(
            read_parquet(&path).unwrap().page(0, 2).unwrap(),
            vec![row, vec![Value::Null, Value::Null]]
        );
    }
}
