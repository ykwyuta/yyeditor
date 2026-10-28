use crate::random::{Random, Secure, Seeded};
use crate::{Output, Params, Result, checkpoint, integer, value};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    columns: Vec<Column>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Column {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    min: Option<f64>,
    max: Option<f64>,
    scale: Option<u8>,
    start: Option<i64>,
    width: Option<u8>,
    prefix: Option<String>,
    from: Option<String>,
    to: Option<String>,
    values: Option<Vec<String>>,
    value: Option<String>,
    #[serde(default)]
    null_percent: u8,
    #[serde(default)]
    unique: bool,
}

enum Kind {
    Sequence {
        start: i64,
        prefix: String,
        width: usize,
    },
    Integer {
        min: i64,
        span: u64,
    },
    Decimal {
        min: i64,
        span: u64,
        scale: u8,
    },
    Date {
        start: NaiveDate,
        span: u64,
    },
    DateTime {
        start: i64,
        span: u64,
    },
    Choice(Vec<String>),
    Literal(String),
    Uuid,
    Named(String),
}

struct Prepared {
    column: Column,
    kind: Kind,
    permutation: Option<Permutation>,
    seen: HashSet<String>,
}
struct Permutation {
    count: u64,
    shift: u64,
    step: u64,
    span: u64,
}
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
impl Permutation {
    fn new(span: u64, rng: &mut impl Random) -> Result<Self> {
        let shift = rng.index(span as usize)? as u64;
        let mut step = (rng.index(span as usize)? as u64).max(1);
        while gcd(step, span) != 1 {
            step = (step + 1) % span;
        }
        Ok(Self {
            count: 0,
            shift,
            step,
            span,
        })
    }
    fn next(&mut self) -> Result<u64> {
        if self.count >= self.span {
            return Err("unique条件を満たす値が足りません".into());
        }
        let n = ((u128::from(self.shift) + u128::from(self.count) * u128::from(self.step))
            % u128::from(self.span)) as u64;
        self.count += 1;
        Ok(n)
    }
}

fn range(c: &Column, scale: u8) -> Result<(i64, u64)> {
    if scale > 6 {
        return Err("scaleは0〜6です".into());
    }
    let min = c.min.unwrap_or(0.0);
    let max = c.max.unwrap_or(100.0);
    if !min.is_finite() || !max.is_finite() || min > max || min.abs() > 1e12 || max.abs() > 1e12 {
        return Err("min/maxは-1兆〜1兆で、min <= maxにしてください".into());
    }
    let factor = 10f64.powi(i32::from(scale));
    let lo = (min * factor).ceil() as i64;
    let hi = (max * factor).floor() as i64;
    if lo > hi || hi.checked_sub(lo).is_none() {
        return Err("数値範囲が不正です".into());
    }
    Ok((lo, (hi - lo) as u64 + 1))
}

fn prepare(column: Column, rng: &mut impl Random, protect: bool) -> Result<Prepared> {
    if column.name.trim().is_empty() || column.null_percent > 100 {
        return Err("列名は必須で、null_percentは0〜100です".into());
    }
    let kind = match column.kind.as_str() {
        "sequence" => Kind::Sequence {
            start: column.start.unwrap_or(1),
            prefix: column.prefix.clone().unwrap_or_default(),
            width: usize::from(column.width.unwrap_or(0)),
        },
        "integer" => {
            if column.min.is_some_and(|n| n.fract() != 0.0)
                || column.max.is_some_and(|n| n.fract() != 0.0)
            {
                return Err("integerのmin/maxは整数です".into());
            }
            let (min, span) = range(&column, 0)?;
            Kind::Integer { min, span }
        }
        "decimal" => {
            let scale = column.scale.unwrap_or(2);
            let (min, span) = range(&column, scale)?;
            Kind::Decimal { min, span, scale }
        }
        "date" => {
            let start = NaiveDate::parse_from_str(
                column.from.as_deref().unwrap_or("2000-01-01"),
                "%Y-%m-%d",
            )
            .map_err(|e| e.to_string())?;
            let end =
                NaiveDate::parse_from_str(column.to.as_deref().unwrap_or("2030-12-31"), "%Y-%m-%d")
                    .map_err(|e| e.to_string())?;
            let days = (end - start).num_days();
            if !(0..=3_650_000).contains(&days) {
                return Err("dateのfrom/toの順序・範囲が不正です".into());
            }
            Kind::Date {
                start,
                span: days as u64 + 1,
            }
        }
        "datetime" => {
            let start = DateTime::parse_from_rfc3339(
                column.from.as_deref().unwrap_or("2026-01-01T00:00:00Z"),
            )
            .map_err(|e| e.to_string())?
            .timestamp();
            let end = DateTime::parse_from_rfc3339(
                column.to.as_deref().unwrap_or("2026-12-31T23:59:59Z"),
            )
            .map_err(|e| e.to_string())?
            .timestamp();
            let span = end
                .checked_sub(start)
                .filter(|n| *n >= 0 && *n <= 315_576_000_000)
                .ok_or("datetimeのfrom/toの範囲が不正です")? as u64
                + 1;
            Kind::DateTime { start, span }
        }
        "choice" => {
            let mut values = column.values.clone().unwrap_or_default();
            if values.is_empty() {
                return Err("choiceにはvaluesが必要です".into());
            }
            if column.unique {
                let mut seen = HashSet::new();
                values.retain(|v| {
                    seen.insert(if protect {
                        csv_safe(v.clone())
                    } else {
                        v.clone()
                    })
                });
            }
            Kind::Choice(values)
        }
        "boolean" => Kind::Choice(vec!["true".into(), "false".into()]),
        "literal" => Kind::Literal(column.value.clone().unwrap_or_default()),
        "uuid" => Kind::Uuid,
        "person_name" | "company_name" | "company_id" | "email" | "phone" | "postal_code"
        | "address" | "department" | "job_title" | "product_name" | "ip_address" | "url" => {
            Kind::Named(column.kind.clone())
        }
        _ => return Err(format!("未対応のデータ種: {}", column.kind)),
    };
    let span = match &kind {
        Kind::Integer { span, .. }
        | Kind::Decimal { span, .. }
        | Kind::Date { span, .. }
        | Kind::DateTime { span, .. } => Some(*span),
        Kind::Choice(v) => Some(v.len() as u64),
        _ => None,
    };
    let permutation = if column.unique {
        span.map(|n| Permutation::new(n, rng)).transpose()?
    } else {
        None
    };
    Ok(Prepared {
        column,
        kind,
        permutation,
        seen: HashSet::new(),
    })
}

impl Prepared {
    fn cell(&mut self, row: usize, rng: &mut impl Random) -> Result<Option<(String, bool)>> {
        if self.column.null_percent > 0 && rng.index(100)? < usize::from(self.column.null_percent) {
            return Ok(None);
        }
        for _ in 0..1024 {
            let index = match &mut self.permutation {
                Some(p) => Some(p.next()?),
                None => None,
            };
            let mut numeric = false;
            let text = match &self.kind {
                Kind::Sequence {
                    start,
                    prefix,
                    width,
                } => format!(
                    "{prefix}{:0width$}",
                    start
                        .checked_add(row as i64)
                        .ok_or("sequenceが整数範囲を超えました")?
                ),
                Kind::Integer { min, span } => {
                    numeric = true;
                    (min + (match index {
                        Some(n) => n,
                        None => rng.index(*span as usize)? as u64,
                    }) as i64)
                        .to_string()
                }
                Kind::Decimal { min, span, scale } => {
                    numeric = true;
                    let units = min
                        + (match index {
                            Some(n) => n,
                            None => rng.index(*span as usize)? as u64,
                        }) as i64;
                    let factor = 10i64.pow(u32::from(*scale));
                    if *scale == 0 {
                        units.to_string()
                    } else {
                        format!(
                            "{}{}.{:0width$}",
                            if units < 0 { "-" } else { "" },
                            units.abs() / factor,
                            units.abs() % factor,
                            width = usize::from(*scale)
                        )
                    }
                }
                Kind::Date { start, span } => start
                    .checked_add_signed(Duration::days(
                        (match index {
                            Some(n) => n,
                            None => rng.index(*span as usize)? as u64,
                        }) as i64,
                    ))
                    .ok_or("日付が範囲を超えました")?
                    .to_string(),
                Kind::DateTime { start, span } => DateTime::<Utc>::from_timestamp(
                    start
                        + (match index {
                            Some(n) => n,
                            None => rng.index(*span as usize)? as u64,
                        }) as i64,
                    0,
                )
                .ok_or("日時が範囲を超えました")?
                .to_rfc3339(),
                Kind::Choice(values) => values[match index {
                    Some(n) => n as usize,
                    None => rng.index(values.len())?,
                }]
                .clone(),
                Kind::Literal(text) => text.clone(),
                Kind::Uuid => {
                    let mut bytes = [0u8; 16];
                    bytes[..8].copy_from_slice(&rng.word()?.to_le_bytes());
                    bytes[8..].copy_from_slice(&rng.word()?.to_le_bytes());
                    bytes[6] = (bytes[6] & 0x0f) | 0x40;
                    bytes[8] = (bytes[8] & 0x3f) | 0x80;
                    let hex: String = bytes.iter().map(|n| format!("{n:02x}")).collect();
                    format!(
                        "{}-{}-{}-{}-{}",
                        &hex[..8],
                        &hex[8..12],
                        &hex[12..16],
                        &hex[16..20],
                        &hex[20..]
                    )
                }
                Kind::Named(kind) => named(kind, row, rng)?,
            };
            if !self.column.unique || self.permutation.is_some() || self.seen.insert(text.clone()) {
                return Ok(Some((text, numeric)));
            }
        }
        Err(format!(
            "{}: unique条件を満たす値を生成できません",
            self.column.name
        ))
    }
}

fn named(kind: &str, row: usize, rng: &mut impl Random) -> Result<String> {
    fn choose(rng: &mut impl Random, values: &[&str]) -> Result<String> {
        Ok(values[rng.index(values.len())?].into())
    }
    Ok(match kind {
        "person_name" => format!(
            "テスト{} {}",
            choose(rng, &["山田", "佐藤", "鈴木", "高橋", "伊藤", "渡辺"])?,
            choose(rng, &["太郎", "花子", "健", "美咲", "さくら", "一郎"])?
        ),
        "company_name" => format!(
            "テスト{}株式会社{:05}",
            choose(rng, &["商事", "技研", "物流", "食品", "情報", "製作所"])?,
            row + 1
        ),
        "company_id" => format!("TEST-CORP-{:08}", row + 1),
        "email" => format!("test-{:08}@example.invalid", row + 1),
        "phone" => format!("000-{:04}-{:04}", rng.index(10000)?, rng.index(10000)?),
        "postal_code" => format!("000-{:04}", rng.index(10000)?),
        "address" => format!(
            "架空県テスト市テスト町{}丁目{}番{}号",
            rng.index(9)? + 1,
            rng.index(99)? + 1,
            rng.index(99)? + 1
        ),
        "department" => choose(
            rng,
            &[
                "テスト営業部",
                "テスト開発部",
                "テスト総務部",
                "テスト経理部",
            ],
        )?,
        "job_title" => choose(
            rng,
            &["テスト担当", "テスト主任", "テスト係長", "テスト部長"],
        )?,
        "product_name" => choose(
            rng,
            &["テスト商品A", "テスト商品B", "テスト商品C", "テスト部品D"],
        )?,
        "ip_address" => format!(
            "{}.{}",
            choose(rng, &["192.0.2", "198.51.100", "203.0.113"])?,
            rng.index(254)? + 1
        ),
        "url" => format!("https://example.invalid/test/{}", row + 1),
        _ => return Err("未対応のテストデータ種".into()),
    })
}

fn csv_safe(value: String) -> String {
    if value.starts_with(['=', '+', '-', '@', '\t', '\r', '\n']) {
        format!("'{value}")
    } else {
        value
    }
}

pub(crate) fn generate(
    input: &str,
    params: &Params,
    cancelled: &impl Fn() -> bool,
) -> Result<Output> {
    let definition: Definition =
        serde_yaml_ng::from_str(input).map_err(|e| format!("列定義YAML: {e}"))?;
    if definition.columns.is_empty() || definition.columns.len() > 64 {
        return Err("列数は1〜64です".into());
    }
    let mut names = HashSet::new();
    for c in &definition.columns {
        if !names.insert(c.name.clone()) {
            return Err(format!("列名が重複しています: {}", c.name));
        }
    }
    let count = integer(params, "rows", 1, 100000)? as usize;
    let seed = if value(params, "seed", "").trim().is_empty() {
        Secure::default().word()?
    } else {
        value(params, "seed", "")
            .trim()
            .parse::<u64>()
            .map_err(|_| "seedは0〜18446744073709551615です")?
    };
    let mut rng = Seeded(seed);
    let protect = value(params, "csv_safe", "表計算ソフト向け") == "表計算ソフト向け";
    let mut prepared: Vec<_> = definition
        .columns
        .into_iter()
        .map(|c| {
            let name = c.name.clone();
            prepare(c, &mut rng, protect).map_err(|e| format!("{name}: {e}"))
        })
        .collect::<Result<_>>()?;
    for p in &prepared {
        if let Some(unique) = &p.permutation
            && p.column.null_percent == 0
            && count as u64 > unique.span
        {
            return Err(format!(
                "{}: unique条件に対し行数が多すぎます",
                p.column.name
            ));
        }
    }
    let mut headers = HashSet::new();
    for p in &prepared {
        if !headers.insert(if protect {
            csv_safe(p.column.name.clone())
        } else {
            p.column.name.clone()
        }) {
            return Err("数式対策後の列名が重複します。列名を変更してください".into());
        }
    }
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_writer(Vec::new());
    writer
        .write_record(prepared.iter().map(|p| {
            if protect {
                csv_safe(p.column.name.clone())
            } else {
                p.column.name.clone()
            }
        }))
        .map_err(|e| e.to_string())?;
    for row in 0..count {
        checkpoint(cancelled)?;
        let mut record = Vec::new();
        for p in &mut prepared {
            let value = p
                .cell(row, &mut rng)
                .map_err(|e| format!("{}行目 {}: {e}", row + 1, p.column.name))?
                .map_or_else(String::new, |(s, numeric)| {
                    if protect && !numeric { csv_safe(s) } else { s }
                });
            record.push(value);
        }
        writer.write_record(record).map_err(|e| e.to_string())?;
        if writer.get_ref().len() > crate::MAX_OUTPUT - 8192 {
            return Err("CSVが16 MiBを超えました。行数・列数を減らしてください".into());
        }
    }
    let bytes = writer.into_inner().map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes.clone()).map_err(|e| e.to_string())?;
    let bytes = if value(params, "bom", "あり") == "あり" {
        [b"\xef\xbb\xbf".as_slice(), bytes.as_slice()].concat()
    } else {
        bytes
    };
    Ok(Output {
        text,
        bytes,
        filename: "dummy-data.csv",
        qr: None,
        notice: format!(
            "架空テストデータ {count}行・{}列 / seed={seed} / {}",
            prepared.len(),
            if protect {
                "文字列の数式対策あり"
            } else {
                "原文のまま"
            }
        ),
    })
}

pub(crate) fn preset(name: &str) -> String {
    use serde_json::json;
    let seq = |name, prefix| json!({"name":name,"type":"sequence","prefix":prefix,"width":6});
    let field = |name, kind| json!({"name":name,"type":kind});
    let choice = |name, values: Vec<&str>| json!({"name":name,"type":"choice","values":values});
    let decimal =
        |name, min, max| json!({"name":name,"type":"decimal","min":min,"max":max,"scale":2});
    let columns = match name {
        "企業" => vec![
            field("企業ID", "company_id"),
            field("企業名", "company_name"),
            field("部署", "department"),
            choice("業種", vec!["製造", "小売", "物流", "IT", "サービス"]),
            json!({"name":"従業員数","type":"integer","min":1,"max":5000}),
            decimal("売上", 0, 100000000),
        ],
        "EC注文" => vec![
            seq("注文ID", "TEST-ORDER-"),
            field("商品名", "product_name"),
            json!({"name":"数量","type":"integer","min":1,"max":10}),
            decimal("単価", 100, 50000),
            choice("状態", vec!["受付", "出荷待ち", "出荷済", "取消"]),
            field("注文日時", "datetime"),
        ],
        "医療" => vec![
            seq("患者ID", "TEST-PATIENT-"),
            field("架空氏名", "person_name"),
            choice(
                "テスト分類",
                vec!["テスト分類A", "テスト分類B", "テスト分類C"],
            ),
            choice("血液型", vec!["A", "B", "O", "AB", "不明"]),
            decimal("体温", 35, 40),
            field("来院日", "date"),
        ],
        "金融" => vec![
            seq("テスト口座ID", "TEST-ACCOUNT-"),
            field("架空名義", "person_name"),
            decimal("残高", 0, 10000000),
            choice("通貨", vec!["JPY", "USD", "EUR"]),
            decimal("テスト利率", 0, 10),
            field("更新日時", "datetime"),
        ],
        "物流" => vec![
            seq("追跡ID", "TEST-TRACK-"),
            field("架空配送先", "address"),
            decimal("重量kg", 1, 100),
            choice("状態", vec!["受付", "輸送中", "配送完了", "返送"]),
            field("出荷日", "date"),
        ],
        "ITログ" => vec![
            field("イベントID", "uuid"),
            field("日時", "datetime"),
            choice("レベル", vec!["INFO", "WARN", "ERROR"]),
            field("文書用IP", "ip_address"),
            field("テストURL", "url"),
            json!({"name":"処理時間ms","type":"integer","min":0,"max":5000}),
        ],
        "IoT" => vec![
            seq("センサーID", "TEST-SENSOR-"),
            field("日時", "datetime"),
            decimal("温度", -20, 50),
            decimal("湿度", 0, 100),
            choice("状態", vec!["正常", "警告", "停止"]),
        ],
        _ => vec![
            seq("ID", "TEST-"),
            field("架空氏名", "person_name"),
            field("メール", "email"),
            field("電話", "phone"),
            field("郵便番号", "postal_code"),
            field("架空住所", "address"),
            json!({"name":"生年月日","type":"date","from":"1960-01-01","to":"2010-12-31"}),
        ],
    };
    serde_yaml_ng::to_string(&json!({"columns":columns})).expect("fixed YAML preset")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_sector_is_reproducible_and_writes_valid_csv() {
        let p = crate::Tool::DummyCsv.defaults();
        for name in [
            "個人", "企業", "EC注文", "医療", "金融", "物流", "ITログ", "IoT",
        ] {
            let spec = preset(name);
            assert!(spec.starts_with("columns:\n"));
            let out = generate(&spec, &p, &|| false).unwrap();
            assert_eq!(out.bytes, generate(&spec, &p, &|| false).unwrap().bytes);
            let mut reader = csv::Reader::from_reader(out.text.as_bytes());
            assert!(reader.headers().unwrap().len() >= 4);
            assert_eq!(reader.records().count(), 100);
            assert!(out.bytes.starts_with(b"\xef\xbb\xbf"));
        }
    }
    #[test]
    fn unique_ranges_nulls_and_quoted_fields() {
        let mut p = crate::Tool::DummyCsv.defaults();
        p.insert("rows".into(), "100".into());
        let input = r#"# CSVの引用符・改行と空欄の試験
columns:
  - name: id
    type: integer
    min: 1
    max: 100
    unique: true
  - name: 'null'
    type: literal
    value: x
    null_percent: 100
  - name: quoted
    type: literal
    value: |-
      a,"b
      次行
  - name: formula
    type: literal
    value: '=1+2'
"#;
        let out = generate(input, &p, &|| false).unwrap();
        let mut ids = HashSet::new();
        for row in csv::Reader::from_reader(out.text.as_bytes()).records() {
            let row = row.unwrap();
            ids.insert(row[0].to_owned());
            assert_eq!(&row[1], "");
            assert_eq!(&row[2], "a,\"b\n次行");
            assert_eq!(&row[3], "'=1+2");
        }
        assert_eq!(ids.len(), 100);
        p.insert("rows".into(), "101".into());
        assert!(generate(input, &p, &|| false).is_err());
        assert!(generate(input, &p, &|| true).is_err());
    }
}
