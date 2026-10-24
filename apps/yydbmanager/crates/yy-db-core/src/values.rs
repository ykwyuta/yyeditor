use serde::{Deserialize, Serialize};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};

/// Exact wire values. Numeric text is never converted through a floating point cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Integer(String),
    Decimal(String),
    Float(#[serde(with = "float_text")] f64),
    Bool(bool),
    Text(String),
    Binary(Vec<u8>),
    Temporal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spooled_pages_cross_sparse_index_and_preserve_special_floats() {
        let mut set = ResultSet::new(vec![Column {
            name: "n".into(),
            type_name: "INTEGER".into(),
        }])
        .unwrap();
        for i in 0..1000 {
            set.push(&[Value::Integer(i.to_string())]).unwrap();
        }
        set.finish().unwrap();
        let rows = set.page(250, 20).unwrap();
        assert_eq!(rows.len(), 20);
        assert_eq!(rows[6][0], Value::Integer("256".into()));
        assert_eq!(set.page(999, 50).unwrap().len(), 1);
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let s = serde_json::to_string(&Value::Float(value)).unwrap();
            let Value::Float(n) = serde_json::from_str(&s).unwrap() else {
                panic!()
            };
            assert!(n == value || n.is_nan() && value.is_nan());
        }
    }
}
mod float_text {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &f64, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<f64, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
impl Value {
    pub fn display(&self) -> String {
        match self {
            Self::Null => "NULL".into(),
            Self::Integer(s) | Self::Decimal(s) | Self::Text(s) | Self::Temporal(s) => s.clone(),
            Self::Float(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Binary(b) => format!(
                "0x{}",
                b.iter().map(|v| format!("{v:02X}")).collect::<String>()
            ),
        }
    }
}
/// Shared Arrow conversion for result grids and scalar/procedural evaluation.
/// Decimal values retain their textual precision, including DECIMAL(38).
pub fn arrow_value(array: &dyn duckdb::arrow::array::Array, row: usize) -> crate::Result<Value> {
    use duckdb::arrow::{
        array::{BinaryArray, LargeBinaryArray},
        datatypes::DataType,
        util::display::array_value_to_string,
    };
    if row >= array.len() {
        return Err("Arrowの行位置が範囲外です".into());
    }
    if array.is_null(row) {
        return Ok(Value::Null);
    }
    let text = || array_value_to_string(array, row).map_err(|e| e.to_string());
    Ok(match array.data_type() {
        DataType::Binary => Value::Binary(
            array
                .as_any()
                .downcast_ref::<BinaryArray>()
                .ok_or("Arrow Binary型が不正です")?
                .value(row)
                .to_vec(),
        ),
        DataType::LargeBinary => Value::Binary(
            array
                .as_any()
                .downcast_ref::<LargeBinaryArray>()
                .ok_or("Arrow LargeBinary型が不正です")?
                .value(row)
                .to_vec(),
        ),
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64 => Value::Integer(text()?),
        DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => Value::Decimal(text()?),
        DataType::Float32 | DataType::Float64 => {
            Value::Float(text()?.parse::<f64>().map_err(|e| e.to_string())?)
        }
        DataType::Boolean => Value::Bool(text()? == "true"),
        DataType::Date32
        | DataType::Date64
        | DataType::Time32(_)
        | DataType::Time64(_)
        | DataType::Timestamp(_, _) => Value::Temporal(text()?),
        _ => Value::Text(text()?),
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub type_name: String,
}

/// Disk-backed rows, with sparse offsets every 256 records. Reuses yysheet's budget.
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub row_count: u64,
    file: tempfile::NamedTempFile,
    offsets: Vec<u64>,
    writer: BufWriter<std::fs::File>,
    pub bytes: u64,
    pub budget: yy_sheet::Budget,
}
impl std::fmt::Debug for ResultSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultSet")
            .field("columns", &self.columns)
            .field("row_count", &self.row_count)
            .finish()
    }
}
impl ResultSet {
    pub fn new(columns: Vec<Column>) -> crate::Result<Self> {
        let file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        let writer = BufWriter::new(file.reopen().map_err(|e| e.to_string())?);
        Ok(Self {
            columns,
            row_count: 0,
            file,
            offsets: vec![],
            writer,
            bytes: 0,
            budget: yy_sheet::Budget::new(512 * 1024 * 1024),
        })
    }
    pub fn push(&mut self, row: &[Value]) -> crate::Result<()> {
        if row.len() != self.columns.len() {
            return Err("列数が一致しません".into());
        }
        let data = serde_json::to_vec(row).map_err(|e| e.to_string())?;
        if data.len() > 64 * 1024 * 1024 {
            return Err("1行は64MiBまでです".into());
        }
        if self.row_count.is_multiple_of(256) {
            self.offsets.push(self.bytes);
        }
        self.writer
            .write_all(&(data.len() as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        self.writer.write_all(&data).map_err(|e| e.to_string())?;
        self.bytes += 4 + data.len() as u64;
        self.row_count += 1;
        Ok(())
    }
    pub fn finish(&mut self) -> crate::Result<()> {
        self.writer.flush().map_err(|e| e.to_string())
    }
    pub fn page(&self, start: u64, count: usize) -> crate::Result<Vec<Vec<Value>>> {
        if start >= self.row_count {
            return Ok(vec![]);
        }
        let block = start / 256;
        let mut reader = BufReader::new(self.file.reopen().map_err(|e| e.to_string())?);
        reader
            .seek(SeekFrom::Start(self.offsets[block as usize]))
            .map_err(|e| e.to_string())?;
        let mut rows = vec![];
        for index in block * 256..(start.saturating_add(count as u64)).min(self.row_count) {
            let mut length = [0; 4];
            reader.read_exact(&mut length).map_err(|e| e.to_string())?;
            let length = u32::from_le_bytes(length) as usize;
            if index < start {
                reader
                    .seek(SeekFrom::Current(length as i64))
                    .map_err(|e| e.to_string())?;
            } else {
                let mut data = vec![0; length];
                reader.read_exact(&mut data).map_err(|e| e.to_string())?;
                rows.push(serde_json::from_slice(&data).map_err(|e| e.to_string())?);
            }
        }
        Ok(rows)
    }
}
