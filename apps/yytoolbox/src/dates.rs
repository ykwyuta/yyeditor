use crate::{Output, Params, Result, value};
use chrono::{Datelike, NaiveDate, Offset, TimeZone};

#[derive(Clone, Copy)]
struct Era {
    name: &'static str,
    short: &'static str,
    base: i32,
    start: (i32, u32, u32),
    end: (i32, u32, u32),
}
const ERAS: [Era; 5] = [
    Era {
        name: "明治",
        short: "M",
        base: 1868,
        start: (1873, 1, 1),
        end: (1912, 7, 29),
    },
    Era {
        name: "大正",
        short: "T",
        base: 1912,
        start: (1912, 7, 30),
        end: (1926, 12, 24),
    },
    Era {
        name: "昭和",
        short: "S",
        base: 1926,
        start: (1926, 12, 25),
        end: (1989, 1, 7),
    },
    Era {
        name: "平成",
        short: "H",
        base: 1989,
        start: (1989, 1, 8),
        end: (2019, 4, 30),
    },
    Era {
        name: "令和",
        short: "R",
        base: 2019,
        start: (2019, 5, 1),
        end: (9999, 12, 31),
    },
];
fn date((y, m, d): (i32, u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("fixed calendar boundary")
}
fn checked_date(y: i32, m: u32, d: u32) -> Result<NaiveDate> {
    if !(1873..=9999).contains(&y) {
        return Err("1873年〜9999年を指定してください。旧暦期間は対象外です".into());
    }
    NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| "存在しない日付です".into())
}
fn era_year(n: i32) -> String {
    if n == 1 { "元".into() } else { n.to_string() }
}

pub(crate) fn calendar(input: &str) -> Result<Output> {
    let normalized: String = input
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            '年' | '月' | '.' | '-' => '/',
            _ => c,
        })
        .collect();
    let text = normalized.trim_end_matches(['日', '/']);
    let (era, rest) = ERAS
        .iter()
        .find_map(|era| {
            text.strip_prefix(era.name)
                .or_else(|| text.strip_prefix(era.short))
                .or_else(|| text.strip_prefix(&era.short.to_lowercase()))
                .map(|rest| (*era, rest))
        })
        .map_or((None, text), |(e, r)| (Some(e), r));
    let parts: Vec<_> = rest.split('/').collect();
    if !matches!(parts.len(), 1 | 3) {
        return Err("YYYY-MM-DD / YYYY / 令和元年5月1日 / R1.5.1 の形式で指定してください".into());
    }
    let year = if parts[0] == "元" && era.is_some() {
        1
    } else {
        parts[0].parse::<i32>().map_err(|_| "年を読み取れません")?
    };
    if year <= 0 {
        return Err("年は1以上です".into());
    }
    let western = match era {
        Some(e) => e.base.checked_add(year - 1).ok_or("年が大きすぎます")?,
        None => year,
    };
    if parts.len() == 1 && (1868..=1872).contains(&western) {
        if era.is_some_and(|e| e.name != "明治") {
            return Err("指定年はその元号の期間外です".into());
        }
        return Ok(Output::text(
            format!(
                "西暦: {western}年\n和暦: 明治{}年\n",
                era_year(western - 1867)
            ),
            "calendar.txt",
            "旧暦期間は年の対応のみを表示します。月日の換算は行いません",
        ));
    }
    checked_date(western, 1, 1)?;
    if parts.len() == 3 {
        let day = checked_date(
            western,
            parts[1].parse().map_err(|_| "月が不正です")?,
            parts[2].parse().map_err(|_| "日が不正です")?,
        )?;
        let actual = ERAS
            .iter()
            .find(|e| day >= date(e.start) && day <= date(e.end))
            .ok_or("対応する元号がありません")?;
        if era.is_some_and(|e| e.name != actual.name) {
            return Err("指定日はその元号の期間外です".into());
        }
        return Ok(Output::text(
            format!(
                "西暦: {}\n和暦: {}{}年{}月{}日\n",
                day,
                actual.name,
                era_year(day.year() - actual.base + 1),
                day.month(),
                day.day()
            ),
            "calendar.txt",
            "元号の開始日・終了日を確認しました",
        ));
    }
    let start = checked_date(western, 1, 1)?;
    let end = checked_date(western, 12, 31)?;
    let mut out = format!("西暦: {western}年\n");
    let mut found = false;
    for e in ERAS
        .iter()
        .filter(|e| era.is_none_or(|chosen| chosen.name == e.name))
    {
        let from = start.max(date(e.start));
        let to = end.min(date(e.end));
        if from <= to {
            found = true;
            out.push_str(&format!(
                "{}{}年: {} 〜 {}\n",
                e.name,
                era_year(western - e.base + 1),
                from,
                to
            ));
        }
    }
    if !found {
        return Err("指定年はその元号の期間外です".into());
    }
    Ok(Output::text(
        out,
        "calendar.txt",
        "改元年は元号ごとの期間を表示します",
    ))
}

fn parse_date(text: &str) -> Result<NaiveDate> {
    let day = NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
        .map_err(|_| "日付はYYYY-MM-DDで指定してください")?;
    checked_date(day.year(), day.month(), day.day())
}
fn age(birth: NaiveDate, asof: NaiveDate) -> i32 {
    asof.year()
        - birth.year()
        - i32::from((asof.month(), asof.day()) < (birth.month(), birth.day()))
}
fn grade(n: i32) -> String {
    match n {
        0..=5 => format!("小学{}年", n + 1),
        6..=8 => format!("中学{}年", n - 5),
        9..=11 => format!("高校{}年", n - 8),
        12..=15 => format!("大学{}年（4年制想定）", n - 11),
        n if n < 0 => "未就学".into(),
        _ => "標準進級では卒業後".into(),
    }
}

pub(crate) fn age_grade(params: &Params) -> Result<Output> {
    let birth = parse_date(value(params, "birth", ""))?;
    let asof = parse_date(value(params, "asof", ""))?;
    if birth > asof {
        return Err("生年月日が基準日より後です".into());
    }
    let academic = asof.year() - i32::from(asof.month() < 4);
    let entry = birth.year() + 6 + i32::from((birth.month(), birth.day()) > (4, 1));
    let notice = format!(
        "満{}歳（誕生日基準）・{}年度 {}。標準入学 {}年4月。2/29生まれの非閏年の年齢切替は3/1",
        age(birth, asof),
        academic,
        grade(academic - entry),
        entry
    );
    let mut rows = vec![vec![
        "学年（標準進級）".into(),
        "生年月日・開始".into(),
        "生年月日・終了".into(),
        "基準日の年齢・最小".into(),
        "最大".into(),
    ]];
    for n in 0..16 {
        let entry = academic - n;
        let from = checked_date(entry - 7, 4, 2)?;
        let to = checked_date(entry - 6, 4, 1)?;
        rows.push(vec![
            grade(n),
            from.to_string(),
            to.to_string(),
            age(to, asof).to_string(),
            age(from, asof).to_string(),
        ]);
    }
    Output::csv(rows, notice)
}

pub(crate) fn time_zones(input: &str, params: &Params) -> Result<Output> {
    let instant = value(params, "instant", "now");
    let instant = if instant.trim() == "now" || instant.trim().is_empty() {
        chrono::Utc::now()
    } else {
        chrono::DateTime::parse_from_rfc3339(instant.trim())
            .map_err(|e| format!("基準日時: {e}"))?
            .with_timezone(&chrono::Utc)
    };
    let origin: chrono_tz::Tz = value(params, "origin", "Asia/Tokyo")
        .parse()
        .map_err(|_| "基準地域のIANA名が不正です")?;
    let base = origin
        .offset_from_utc_datetime(&instant.naive_utc())
        .fix()
        .local_minus_utc();
    let zones: Vec<chrono_tz::Tz> = if input.trim() == "*" {
        chrono_tz::TZ_VARIANTS.to_vec()
    } else {
        input
            .split([',', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().map_err(|_| format!("不明なタイムゾーン: {s}")))
            .collect::<Result<_>>()?
    };
    if zones.is_empty() {
        return Err("IANA名を入力してください（例: Asia/Tokyo）。*で全地域を表示します".into());
    }
    let mut zones: Vec<_> = zones
        .into_iter()
        .map(|tz| {
            (
                instant.with_timezone(&tz).offset().fix().local_minus_utc(),
                tz,
            )
        })
        .collect();
    zones.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name().cmp(b.1.name())));
    zones.dedup();
    let mut rows = vec![vec![
        "IANA地域".into(),
        "現地日時".into(),
        "UTCオフセット".into(),
        format!("{origin}との時差"),
        "略称".into(),
    ]];
    let offset = |seconds: i32| {
        let mut text = format!(
            "{}{:02}:{:02}",
            if seconds < 0 { '-' } else { '+' },
            seconds.abs() / 3600,
            seconds.abs() % 3600 / 60
        );
        if seconds.abs() % 60 != 0 {
            text.push_str(&format!(":{:02}", seconds.abs() % 60));
        }
        text
    };
    for (seconds, tz) in zones {
        let local = instant.with_timezone(&tz);
        rows.push(vec![
            tz.name().into(),
            local.format("%Y-%m-%d %H:%M:%S").to_string(),
            offset(seconds),
            offset(seconds - base),
            local.format("%Z").to_string(),
        ]);
    }
    Output::csv(
        rows,
        format!(
            "基準UTC: {} / 基準地域: {origin} / IANA tzdb {}。指定時点の夏時間を反映",
            instant.to_rfc3339(),
            chrono_tz::IANA_TZDB_VERSION
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn era_boundaries_and_invalid_dates() {
        for (source, expected) in [
            ("1989-01-07", "昭和64年"),
            ("1989-01-08", "平成元年"),
            ("2019-04-30", "平成31年"),
            ("2019-05-01", "令和元年"),
            ("R1.5.1", "2019-05-01"),
            ("明治6年1月1日", "1873-01-01"),
            ("明治元年", "1868年"),
            ("1872", "明治5年"),
        ] {
            assert!(
                calendar(source).unwrap().text.contains(expected),
                "{source}"
            );
        }
        for source in [
            "平成31年5月1日",
            "令和元年4月30日",
            "2025-02-29",
            "明治5年1月1日",
        ] {
            assert!(calendar(source).is_err(), "{source}");
        }
        let text = calendar("1989").unwrap().text;
        assert!(text.contains("昭和64年") && text.contains("平成元年"));
    }
    #[test]
    fn april_first_and_second_and_dst() {
        let mut p = crate::Tool::AgeGrade.defaults();
        p.insert("asof".into(), "2026-04-01".into());
        p.insert("birth".into(), "2020-04-01".into());
        assert!(age_grade(&p).unwrap().notice.contains("小学1年"));
        p.insert("birth".into(), "2020-04-02".into());
        assert!(age_grade(&p).unwrap().notice.contains("未就学"));
        let mut p = crate::Tool::TimeZones.defaults();
        p.insert("instant".into(), "2026-03-08T06:59:59Z".into());
        let before = time_zones("America/New_York", &p).unwrap();
        assert!(before.text.contains("-05:00"));
        p.insert("instant".into(), "2026-03-08T07:00:00Z".into());
        let after = time_zones("America/New_York", &p).unwrap();
        assert!(after.text.contains("-04:00"));
    }
}
