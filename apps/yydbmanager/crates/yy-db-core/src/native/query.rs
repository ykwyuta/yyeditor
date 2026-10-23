use super::*;
use crate::Column;
use futures_util::TryStreamExt;
use mysql::prelude::Queryable;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn count(n: u64) -> Result<ResultSet> {
    let mut s = ResultSet::new(vec![Column {
        name: "更新件数".into(),
        type_name: "BIGINT".into(),
    }])?;
    s.push(&[Value::Integer(n.to_string())])?;
    s.finish()?;
    Ok(s)
}
fn cancelled(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Relaxed) {
        Err("取り消されました".into())
    } else {
        Ok(())
    }
}

pub(super) fn run(
    driver: &mut Driver,
    runtime: &tokio::runtime::Runtime,
    sql: &str,
    cancel: &AtomicBool,
) -> Result<Vec<ResultSet>> {
    match driver {
        Driver::Postgres(c) => postgres_query(c, sql, cancel),
        Driver::Mysql(c) => mysql_query(c, sql, cancel),
        Driver::SqlServer(c) => runtime.block_on(sqlserver_query(c, sql, cancel)),
        Driver::Oracle(c) => oracle_query(c, sql, cancel),
        Driver::Db2(c) => db2_query(c, sql, cancel),
    }
}
struct Raw(Vec<u8>);
impl<'a> postgres::types::FromSql<'a> for Raw {
    fn from_sql(
        _: &postgres::types::Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Self(raw.to_vec()))
    }
    fn accepts(_: &postgres::types::Type) -> bool {
        true
    }
}
fn pg_value(row: &postgres::Row, i: usize) -> Result<Value> {
    use postgres::types::Type;
    let ty = row.columns()[i].type_();
    let Some(Raw(raw)) = row.try_get::<_, Option<Raw>>(i).map_err(error)? else {
        return Ok(Value::Null);
    };
    macro_rules! typed {
        ($t:ty,$v:ident) => {
            Value::$v(row.try_get::<_, $t>(i).map_err(error)?.to_string())
        };
    }
    Ok(match *ty {
        Type::INT2 => typed!(i16, Integer),
        Type::INT4 => typed!(i32, Integer),
        Type::INT8 => typed!(i64, Integer),
        Type::OID => typed!(u32, Integer),
        Type::BOOL => Value::Bool(row.try_get(i).map_err(error)?),
        Type::FLOAT4 => Value::Float(row.try_get::<_, f32>(i).map_err(error)? as f64),
        Type::FLOAT8 => Value::Float(row.try_get(i).map_err(error)?),
        Type::BYTEA => Value::Binary(raw),
        Type::NUMERIC => Value::Decimal(pg_numeric(&raw)?),
        Type::DATE => typed!(chrono::NaiveDate, Temporal),
        Type::TIME => typed!(chrono::NaiveTime, Temporal),
        Type::TIMESTAMP => typed!(chrono::NaiveDateTime, Temporal),
        Type::TIMESTAMPTZ => typed!(chrono::DateTime<chrono::Utc>, Temporal),
        Type::JSONB => Value::Text(
            String::from_utf8(raw.get(1..).ok_or("不正なJSONB")?.to_vec()).map_err(error)?,
        ),
        Type::UUID => {
            if raw.len() != 16 {
                return Err("不正なUUID".into());
            }
            let h = raw.iter().map(|b| format!("{b:02x}")).collect::<String>();
            Value::Text(format!(
                "{}-{}-{}-{}-{}",
                &h[..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..]
            ))
        }
        _ => {
            Value::Text(String::from_utf8(raw).map_err(|_| {
                format!("PostgreSQLの型 {} は直接接続の表示に未対応です", ty.name())
            })?)
        }
    })
}
fn pg_numeric(raw: &[u8]) -> Result<String> {
    if raw.len() < 8 {
        return Err("不正なNUMERIC".into());
    }
    let word = |i| u16::from_be_bytes([raw[i], raw[i + 1]]);
    let n = word(0) as usize;
    let weight = word(2) as i16;
    let sign = word(4);
    let scale = word(6) as usize;
    if raw.len() != 8 + n * 2 || ![0, 0x4000].contains(&sign) {
        return Err("非有限／不正なNUMERICは表示できません".into());
    }
    let mut digits = Vec::with_capacity(n);
    for i in 0..n {
        let v = word(8 + i * 2);
        if v >= 10000 {
            return Err("不正なNUMERIC桁".into());
        }
        digits.push(v);
    }
    let group = |power: i32| {
        digits
            .get((i32::from(weight) - power) as usize)
            .copied()
            .unwrap_or(0)
    };
    let mut out = if sign == 0x4000 {
        "-".to_string()
    } else {
        String::new()
    };
    if weight < 0 {
        out.push('0');
    } else {
        for p in (0..=i32::from(weight)).rev() {
            if p == i32::from(weight) {
                out.push_str(&group(p).to_string());
            } else {
                out.push_str(&format!("{:04}", group(p)));
            }
        }
    }
    if scale > 0 {
        out.push('.');
        let mut fraction = String::new();
        for p in 1..=scale.div_ceil(4) {
            fraction.push_str(&format!("{:04}", group(-(p as i32))));
        }
        out.push_str(&fraction[..scale]);
    }
    Ok(out)
}
fn postgres_query(
    c: &mut postgres::Client,
    sql: &str,
    cancel: &AtomicBool,
) -> Result<Vec<ResultSet>> {
    use postgres::fallible_iterator::FallibleIterator;
    let statement = c.prepare(sql).map_err(error)?;
    let mut set = ResultSet::new(
        statement
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().into(),
                type_name: c.type_().name().into(),
            })
            .collect(),
    )?;
    let mut rows = c
        .query_raw(
            &statement,
            std::iter::empty::<&dyn postgres::types::ToSql>(),
        )
        .map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        cancelled(cancel)?;
        let values = (0..row.len())
            .map(|i| pg_value(&row, i))
            .collect::<Result<Vec<_>>>()?;
        set.push(&values)?;
    }
    if set.columns.is_empty() {
        return Ok(vec![count(rows.rows_affected().unwrap_or(0))?]);
    }
    set.finish()?;
    Ok(vec![set])
}
fn mysql_query(c: &mut mysql::Conn, sql: &str, cancel: &AtomicBool) -> Result<Vec<ResultSet>> {
    use mysql::consts::ColumnType;
    let mut result = c.query_iter(sql).map_err(error)?;
    let mut sets = vec![];
    while let Some(rows) = result.iter() {
        let columns = rows.columns().as_ref().to_vec();
        let affected = rows.affected_rows();
        let mut set = ResultSet::new(
            columns
                .iter()
                .map(|c| Column {
                    name: c.name_str().into_owned(),
                    type_name: format!("{:?}", c.column_type()),
                })
                .collect(),
        )?;
        for row in rows {
            cancelled(cancel)?;
            let row = row.map_err(error)?;
            let values = row
                .unwrap()
                .into_iter()
                .zip(&columns)
                .map(|(v, c)| {
                    Ok(match v {
                        mysql::Value::NULL => Value::Null,
                        mysql::Value::Int(n) => Value::Integer(n.to_string()),
                        mysql::Value::UInt(n) => Value::Integer(n.to_string()),
                        mysql::Value::Float(n) => Value::Float(n as f64),
                        mysql::Value::Double(n) => Value::Float(n),
                        mysql::Value::Bytes(b) => {
                            if c.character_set() == 63
                                && matches!(
                                    c.column_type(),
                                    ColumnType::MYSQL_TYPE_BLOB
                                        | ColumnType::MYSQL_TYPE_TINY_BLOB
                                        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
                                        | ColumnType::MYSQL_TYPE_LONG_BLOB
                                        | ColumnType::MYSQL_TYPE_STRING
                                        | ColumnType::MYSQL_TYPE_VAR_STRING
                                        | ColumnType::MYSQL_TYPE_VARCHAR
                                        | ColumnType::MYSQL_TYPE_BIT
                                )
                            {
                                Value::Binary(b)
                            } else {
                                let s = String::from_utf8(b).map_err(error)?;
                                match c.column_type() {
                                    ColumnType::MYSQL_TYPE_DECIMAL
                                    | ColumnType::MYSQL_TYPE_NEWDECIMAL => Value::Decimal(s),
                                    ColumnType::MYSQL_TYPE_TINY
                                    | ColumnType::MYSQL_TYPE_SHORT
                                    | ColumnType::MYSQL_TYPE_LONG
                                    | ColumnType::MYSQL_TYPE_LONGLONG
                                    | ColumnType::MYSQL_TYPE_INT24
                                    | ColumnType::MYSQL_TYPE_YEAR => Value::Integer(s),
                                    ColumnType::MYSQL_TYPE_FLOAT
                                    | ColumnType::MYSQL_TYPE_DOUBLE => {
                                        Value::Float(s.parse::<f64>().map_err(error)?)
                                    }
                                    ColumnType::MYSQL_TYPE_DATE
                                    | ColumnType::MYSQL_TYPE_DATETIME
                                    | ColumnType::MYSQL_TYPE_TIMESTAMP
                                    | ColumnType::MYSQL_TYPE_TIME => Value::Temporal(s),
                                    _ => Value::Text(s),
                                }
                            }
                        }
                        mysql::Value::Date(y, m, d, h, min, s, micro) => Value::Temporal(format!(
                            "{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}.{micro:06}"
                        )),
                        mysql::Value::Time(neg, days, h, m, s, micro) => Value::Temporal(format!(
                            "{}{hours}:{m:02}:{s:02}.{micro:06}",
                            if neg { "-" } else { "" },
                            hours = u64::from(days) * 24 + u64::from(h)
                        )),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            set.push(&values)?;
        }
        if set.columns.is_empty() {
            sets.push(count(affected)?);
        } else {
            set.finish()?;
            sets.push(set);
        }
    }
    Ok(sets)
}
async fn sqlserver_query(
    c: &mut SqlServer,
    sql: &str,
    cancel: &AtomicBool,
) -> Result<Vec<ResultSet>> {
    let mut stream = c.simple_query(sql).await.map_err(error)?;
    let mut sets = vec![];
    let mut current: Option<ResultSet> = None;
    while let Some(item) = stream.try_next().await.map_err(error)? {
        cancelled(cancel)?;
        match item {
            tiberius::QueryItem::Metadata(meta) => {
                if let Some(mut set) = current.take() {
                    set.finish()?;
                    sets.push(set);
                }
                current = Some(ResultSet::new(
                    meta.columns()
                        .iter()
                        .map(|c| Column {
                            name: c.name().into(),
                            type_name: format!("{:?}", c.column_type()),
                        })
                        .collect(),
                )?);
            }
            tiberius::QueryItem::Row(row) => {
                let values = (0..row.len())
                    .map(|i| tds_value(&row, i))
                    .collect::<Result<Vec<_>>>()?;
                current
                    .as_mut()
                    .ok_or("SQL Serverの列情報がありません")?
                    .push(&values)?;
            }
        }
    }
    if let Some(mut set) = current {
        set.finish()?;
        sets.push(set);
    }
    Ok(sets)
}
fn tds_value(row: &tiberius::Row, i: usize) -> Result<Value> {
    use tiberius::ColumnData as C;
    macro_rules! numeric {
        ($n:expr,$v:ident) => {
            $n.map(|n| Value::$v(n.to_string())).unwrap_or(Value::Null)
        };
    }
    macro_rules! temporal {
        ($t:ty) => {
            row.try_get::<$t, _>(i)
                .map_err(error)?
                .map(|v| Value::Temporal(v.to_string()))
                .unwrap_or(Value::Null)
        };
    }
    Ok(match row.get_column_data(i).map_err(error)? {
        C::U8(n) => numeric!(n, Integer),
        C::I16(n) => numeric!(n, Integer),
        C::I32(n) => numeric!(n, Integer),
        C::I64(n) => numeric!(n, Integer),
        C::F32(n) => n.map(|n| Value::Float(n as f64)).unwrap_or(Value::Null),
        C::F64(n) => n.map(Value::Float).unwrap_or(Value::Null),
        C::Bit(n) => n.map(Value::Bool).unwrap_or(Value::Null),
        C::String(s) => s
            .as_ref()
            .map(|s| Value::Text(s.to_string()))
            .unwrap_or(Value::Null),
        C::Binary(b) => b
            .as_ref()
            .map(|b| Value::Binary(b.to_vec()))
            .unwrap_or(Value::Null),
        C::Guid(g) => numeric!(g, Text),
        C::Numeric(n) => numeric!(n, Decimal),
        C::Xml(x) => x
            .as_ref()
            .map(|x| Value::Text(x.to_string()))
            .unwrap_or(Value::Null),
        C::DateTime(_) | C::SmallDateTime(_) | C::DateTime2(_) => temporal!(chrono::NaiveDateTime),
        C::Date(_) => temporal!(chrono::NaiveDate),
        C::Time(_) => temporal!(chrono::NaiveTime),
        C::DateTimeOffset(_) => temporal!(chrono::DateTime<chrono::FixedOffset>),
    })
}
fn oracle_query(
    c: &mut oracledb::Connection,
    sql: &str,
    cancel: &AtomicBool,
) -> Result<Vec<ResultSet>> {
    if sql.trim().eq_ignore_ascii_case("COMMIT") {
        c.commit().map_err(error)?;
        return Ok(vec![count(0)?]);
    }
    if sql.trim().eq_ignore_ascii_case("ROLLBACK") {
        c.rollback().map_err(error)?;
        return Ok(vec![count(0)?]);
    }
    c.set_call_timeout(Some(Duration::from_secs(60)))
        .map_err(error)?;
    let statement = c.statement(sql).map_err(error)?.build().map_err(error)?;
    if !statement.is_query() {
        return Ok(vec![count(
            c.execute(sql, &[]).map_err(error)?.rows_affected(),
        )?]);
    }
    let cursor = statement.query(&[]).map_err(error)?;
    let meta = cursor.columns().to_vec();
    let mut set = ResultSet::new(
        meta.iter()
            .map(|m| Column {
                name: m.name().into(),
                type_name: if m.db_type() == oracledb::DB_TYPE_NUMBER {
                    format!("NUMBER({},{})", m.precision(), m.scale())
                } else {
                    m.db_type().name().into()
                },
            })
            .collect(),
    )?;
    for row in cursor {
        cancelled(cancel)?;
        let row = row.map_err(error)?;
        let values = meta
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let ty = m.db_type();
                macro_rules! formatted {
                    ($t:ty,$v:ident) => {
                        return Ok(row
                            .get::<Option<$t>>(i)
                            .map_err(error)?
                            .map(|v| Value::$v(v.to_string()))
                            .unwrap_or(Value::Null))
                    };
                }
                if ty == oracledb::DB_TYPE_NUMBER {
                    formatted!(oracledb::OracleNumber, Decimal);
                }
                if [
                    oracledb::DB_TYPE_DATE,
                    oracledb::DB_TYPE_TIMESTAMP,
                    oracledb::DB_TYPE_TIMESTAMP_TZ,
                    oracledb::DB_TYPE_TIMESTAMP_LTZ,
                ]
                .contains(&ty)
                {
                    formatted!(oracledb::OracleTimestamp, Temporal);
                }
                if ty == oracledb::DB_TYPE_INTERVAL_DS {
                    formatted!(oracledb::OracleIntervalDS, Temporal);
                }
                if ty == oracledb::DB_TYPE_INTERVAL_YM {
                    formatted!(oracledb::OracleIntervalYM, Temporal);
                }
                if ty == oracledb::DB_TYPE_BINARY_DOUBLE {
                    return Ok(row
                        .get::<Option<f64>>(i)
                        .map_err(error)?
                        .map(Value::Float)
                        .unwrap_or(Value::Null));
                }
                if ty == oracledb::DB_TYPE_BINARY_FLOAT {
                    return Ok(row
                        .get::<Option<f32>>(i)
                        .map_err(error)?
                        .map(|v| Value::Float(v as f64))
                        .unwrap_or(Value::Null));
                }
                if ty == oracledb::DB_TYPE_BOOLEAN {
                    return Ok(row
                        .get::<Option<bool>>(i)
                        .map_err(error)?
                        .map(Value::Bool)
                        .unwrap_or(Value::Null));
                }
                if ty.is_binary_type() {
                    return Ok(row
                        .get::<Option<Vec<u8>>>(i)
                        .map_err(error)?
                        .map(Value::Binary)
                        .unwrap_or(Value::Null));
                }
                let value = row.get::<Option<String>>(i).map_err(error)?;
                Ok(value.map(Value::Text).unwrap_or(Value::Null))
            })
            .collect::<Result<Vec<_>>>()?;
        set.push(&values)?;
    }
    set.finish()?;
    Ok(vec![set])
}
fn db2_query(
    c: &mut zeus::client::Connection,
    sql: &str,
    cancel: &AtomicBool,
) -> Result<Vec<ResultSet>> {
    let trimmed = sql.trim();
    let ca = if trimmed.eq_ignore_ascii_case("COMMIT") {
        Some(c.commit().map_err(error)?)
    } else if trimmed.eq_ignore_ascii_case("ROLLBACK") {
        Some(c.rollback().map_err(error)?)
    } else {
        None
    };
    if let Some(ca) = ca {
        check_ca(&ca)?;
        return Ok(vec![count(0)?]);
    }
    // Preparing a statement identifies queries without executing mutations twice.
    let first = leading_sql(trimmed)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if !["SELECT", "WITH", "VALUES"].contains(&first.as_str()) {
        let ca = c.execute(sql).map_err(error)?;
        check_ca(&ca)?;
        return Ok(vec![count(ca.errd[2].max(0) as u64)?]);
    }
    let rows = c.query(sql).map_err(error)?;
    check_ca(&rows.sqlca)?;
    let mut set = ResultSet::new(
        rows.columns
            .iter()
            .enumerate()
            .map(|(i, c)| Column {
                name: if c.name.is_empty() {
                    format!("COLUMN{}", i + 1)
                } else {
                    c.name.clone()
                },
                type_name: match c.drda_type {
                    0x02 => "INTEGER".into(),
                    0x04 => "SMALLINT".into(),
                    0x16 => "BIGINT".into(),
                    0x0e => format!("DECIMAL({},{})", c.len >> 8, c.len & 255),
                    0x26 | 0x28 => "VARBINARY".into(),
                    0x20 => "DATE".into(),
                    0x22 => "TIME".into(),
                    0x24 => "TIMESTAMP".into(),
                    _ => "VARCHAR".into(),
                },
            })
            .collect(),
    )?;
    for row in rows.rows {
        cancelled(cancel)?;
        let values = row
            .into_iter()
            .zip(&rows.columns)
            .map(|(v, c)| match v {
                zeus::sql::Value::Null => Value::Null,
                zeus::sql::Value::Int(n) => Value::Integer(n.to_string()),
                zeus::sql::Value::Dec(s) => Value::Decimal(s),
                zeus::sql::Value::Float(n) => Value::Float(n),
                zeus::sql::Value::Bytes(b) => Value::Binary(b),
                zeus::sql::Value::Str(s) => {
                    if [0x20, 0x22, 0x24].contains(&c.drda_type) {
                        Value::Temporal(s)
                    } else {
                        Value::Text(s)
                    }
                }
            })
            .collect::<Vec<_>>();
        set.push(&values)?;
    }
    set.finish()?;
    Ok(vec![set])
}
fn leading_sql(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();
        if let Some(rest) = sql.strip_prefix("--") {
            sql = rest.find('\n').map(|i| &rest[i + 1..]).unwrap_or("");
        } else if let Some(rest) = sql.strip_prefix("/*") {
            if let Some(end) = rest.find("*/") {
                sql = &rest[end + 2..];
            } else {
                return "";
            }
        } else {
            return sql;
        }
    }
}
fn check_ca(ca: &zeus::sql::SqlCa) -> Result<()> {
    if ca.sqlcode < 0 {
        Err(format!(
            "Db2 SQLCODE={} SQLSTATE={} {}",
            ca.sqlcode,
            ca.sqlstate,
            ca.tokens.join(" ")
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn postgres_numeric_preserves_precision_scale_sign_and_zero_groups() {
        fn wire(weight: i16, sign: u16, scale: u16, groups: &[u16]) -> Vec<u8> {
            let mut out = vec![];
            for v in [groups.len() as u16, weight as u16, sign, scale]
                .iter()
                .chain(groups)
            {
                out.extend(v.to_be_bytes());
            }
            out
        }
        assert_eq!(
            pg_numeric(&wire(3, 0, 4, &[1234, 5678, 9012, 3456, 1234])).unwrap(),
            "1234567890123456.1234"
        );
        assert_eq!(
            pg_numeric(&wire(-2, 0x4000, 8, &[12])).unwrap(),
            "-0.00000012"
        );
        assert_eq!(pg_numeric(&wire(1, 0, 2, &[1])).unwrap(), "10000.00");
        assert!(pg_numeric(&wire(0, 0, 0, &[10000])).is_err());
        assert!(pg_numeric(&[0; 7]).is_err());
    }
    #[test]
    fn leading_comments_do_not_turn_a_db2_query_into_a_mutation() {
        assert!(leading_sql(" -- 日本語\n /* comment */\n SELECT 1").starts_with("SELECT"));
        assert_eq!(leading_sql("-- comment"), "");
    }
}
