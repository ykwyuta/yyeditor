//! Weekdays excluding the imported closure/holiday CSV.
use chrono::{Datelike, Days, NaiveDate, NaiveDateTime, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;

pub const DEFAULT_CSV: &[u8] = include_bytes!("../data/japan-holidays-2026-2031.csv");
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Adjustment {
    #[default]
    None,
    Previous,
    Next,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Calendar {
    pub name: String,
    pub holidays: BTreeSet<NaiveDate>,
}
impl Default for Calendar {
    fn default() -> Self {
        Self::from_csv(DEFAULT_CSV, "日本の祝日 2026–2031".into())
            .expect("bundled holiday CSV must be valid")
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
impl Calendar {
    pub fn years(&self) -> (i32, i32) {
        (
            self.holidays.first().map_or(0, Datelike::year),
            self.holidays.last().map_or(0, Datelike::year),
        )
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.holidays.is_empty()
            || self.holidays.len() > 100_000
            || self
                .holidays
                .iter()
                .any(|d| !(1900..=9998).contains(&d.year()))
        {
            return Err(invalid("休業日カレンダーが不正です。"));
        }
        Ok(())
    }
    pub fn from_csv(bytes: &[u8], name: String) -> io::Result<Self> {
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(invalid("CSVは4 MiB以下にしてください。"));
        }
        let decoded = match std::str::from_utf8(bytes) {
            Ok(s) => std::borrow::Cow::Borrowed(s),
            Err(_) => {
                let (s, _, failed) = encoding_rs::SHIFT_JIS.decode(bytes);
                if failed {
                    return Err(invalid("CSVはUTF-8またはCP932で保存してください。"));
                }
                s
            }
        };
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(decoded.trim_start_matches('\u{feff}').as_bytes());
        let mut holidays = BTreeSet::new();
        for (index, record) in reader.records().enumerate() {
            let record = record.map_err(|e| invalid(format!("CSVを読み込めません: {e}")))?;
            if record.len() > 2 {
                return Err(invalid(format!(
                    "CSVの{}行目: 日付と名称（任意）の2列までです。",
                    index + 1
                )));
            }
            let raw = record.get(0).unwrap_or("").trim();
            if index == 0
                && [
                    "日付",
                    "休業日",
                    "祝日",
                    "date",
                    "holiday",
                    "国民の祝日・休日月日",
                ]
                .iter()
                .any(|h| raw.eq_ignore_ascii_case(h))
            {
                continue;
            }
            let date = ["%Y-%m-%d", "%Y/%m/%d", "%Y%m%d"]
                .iter()
                .find_map(|fmt| NaiveDate::parse_from_str(raw, fmt).ok())
                .filter(|d| (1900..=9998).contains(&d.year()))
                .ok_or_else(|| {
                    invalid(format!("CSVの{}行目: 日付「{raw}」が不正です。", index + 1))
                })?;
            holidays.insert(date);
            if holidays.len() > 100_000 {
                return Err(invalid("休業日は100,000日までです。"));
            }
        }
        if holidays.is_empty() {
            return Err(invalid("CSVに休業日の日付がありません。"));
        }
        Ok(Self { name, holidays })
    }
    pub fn is_business_day(&self, date: NaiveDate) -> bool {
        !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) && !self.holidays.contains(&date)
    }
    pub fn adjust(&self, due: NaiveDateTime, direction: Adjustment) -> io::Result<NaiveDateTime> {
        if direction == Adjustment::None {
            return Ok(due);
        }
        let mut date = due.date();
        let (first, last) = self.years();
        // The imported calendar is finite; walking at most its size + one week is sufficient.
        for _ in 0..self.holidays.len() * 2 + 14 {
            if !(first..=last).contains(&date.year()) {
                return Err(invalid(format!(
                    "{}年の休業日CSVが未登録です（登録済み: {first}～{last}年）。",
                    date.year()
                )));
            }
            if self.is_business_day(date) {
                return Ok(date.and_time(due.time()));
            }
            date = match direction {
                Adjustment::Previous => date.checked_sub_days(Days::new(1)),
                Adjustment::Next => date.checked_add_days(Days::new(1)),
                Adjustment::None => unreachable!(),
            }
            .filter(|d| (1900..=9998).contains(&d.year()))
            .ok_or_else(|| invalid("補正後の期限が範囲を超えています。"))?;
        }
        Err(invalid("移動先の営業日が見つかりません。"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn date(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }
    #[test]
    fn csv_bom_quotes_encodings_and_invalid_dates() {
        let c = Calendar::from_csv(
            "\u{feff}日付,名称\r\n2026/11/3,\"文化の日,休業\"\r\n20261103,重複\n2026-11-04,休業\n"
                .as_bytes(),
            "test".into(),
        )
        .unwrap();
        assert_eq!(c.holidays.len(), 2);
        let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode("日付,名称\r\n2026/11/3,文化の日\r\n");
        assert!(Calendar::from_csv(&encoded, "CP932".into()).is_ok());
        for csv in [
            "",
            "日付,名称\n",
            "2026-02-30,x\n",
            "2026-11-03,x,y\n",
            "nonsense,x\n",
        ] {
            assert!(
                Calendar::from_csv(csv.as_bytes(), "bad".into()).is_err(),
                "{csv}"
            );
        }
    }
    #[test]
    fn both_directions_preserve_time_and_skip_consecutive_closures() {
        let c = Calendar::from_csv(b"2026-11-02\n2026-11-03\n", "test".into()).unwrap();
        assert_eq!(
            c.adjust(date("2026-11-01 18:37"), Adjustment::Previous)
                .unwrap(),
            date("2026-10-30 18:37")
        );
        assert_eq!(
            c.adjust(date("2026-11-01 18:37"), Adjustment::Next)
                .unwrap(),
            date("2026-11-04 18:37")
        );
        assert_eq!(
            c.adjust(date("2026-11-01 18:37"), Adjustment::None)
                .unwrap(),
            date("2026-11-01 18:37")
        );
    }
    #[test]
    fn bundled_japan_calendar_covers_six_years_and_special_holidays() {
        let c = Calendar::default();
        for year in 2026..=2031 {
            assert!(
                c.holidays
                    .contains(&NaiveDate::from_ymd_opt(year, 1, 1).unwrap())
            );
        }
        for day in [
            "2026-05-06 10:00",
            "2026-09-22 10:00",
            "2027-03-22 10:00",
            "2029-09-24 10:00",
        ] {
            assert!(!c.is_business_day(date(day).date()), "{day}");
        }
        assert_eq!(c.holidays.first().unwrap().year(), 2026);
        assert_eq!(c.holidays.last().unwrap().year(), 2031);
    }
}
