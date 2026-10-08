//! 日付のシリアル値（Excel の 1900 年・1904 年の日付）。
//!
//! 1900 年の日付では、1900 年 1 月 1 日が 1。Excel と同じく 1900 年 2 月 29 日（シリアル値 60）が
//! あるものとする（Lotus 1-2-3 から引き継いだうるう年の誤り）。1904 年の日付では 1904 年 1 月 1 日が 0。

/// 日付の基準。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DateSystem {
    /// 1900 年（Windows の Excel の既定）
    #[default]
    D1900,
    /// 1904 年（Mac の Excel の古い既定）
    D1904,
}

/// 年月日と時刻。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// ミリ秒
    pub milli: u32,
}

/// 1970-01-01 からの日数（先発グレゴリオ暦）。
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 1970-01-01 からの日数から年月日。
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// 年月日が正しいか。
pub fn valid_date(y: i32, m: u32, d: u32) -> bool {
    if !(1..=12).contains(&m) || d == 0 {
        return false;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    d <= days[m as usize - 1]
}

const UNIX_1899_12_30: i64 = -25_569;
const UNIX_1899_12_31: i64 = -25_568;
const UNIX_1904_01_01: i64 = -24_107;

/// 年月日のシリアル値（日付が扱える範囲〔1900 年の日付で 1900-01-01〜9999-12-31〕の外なら `None`）。
/// 1900 年の日付では `1900, 2, 29` も受け付けて 60 を返す（Excel と同じ）。
pub fn serial_from_date(sys: DateSystem, y: i32, m: u32, d: u32) -> Option<f64> {
    if sys == DateSystem::D1900 && (y, m, d) == (1900, 2, 29) {
        return Some(60.0);
    }
    if !valid_date(y, m, d) || y > 9999 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    let serial = match sys {
        DateSystem::D1900 => {
            if (y, m) < (1900, 3) {
                days - UNIX_1899_12_31
            } else {
                days - UNIX_1899_12_30
            }
        }
        DateSystem::D1904 => days - UNIX_1904_01_01,
    };
    let min = if sys == DateSystem::D1900 { 1 } else { 0 };
    (serial >= min).then_some(serial as f64)
}

/// 日時のシリアル値。
pub fn serial_from_datetime(sys: DateSystem, dt: &DateTime) -> Option<f64> {
    if dt.hour > 23 || dt.minute > 59 || dt.second > 59 || dt.milli > 999 {
        return None;
    }
    let day = serial_from_date(sys, dt.year, dt.month, dt.day)?;
    Some(day + time_fraction(dt.hour, dt.minute, dt.second, dt.milli))
}

/// 時刻の日の割合。
pub fn time_fraction(h: u32, m: u32, s: u32, ms: u32) -> f64 {
    (((h * 60 + m) * 60 + s) as f64 * 1000.0 + ms as f64) / 86_400_000.0
}

/// シリアル値の日時（範囲外・負なら `None`）。時刻はミリ秒に丸める（Excel と同じく、丸めで日を
/// 繰り上げる）。
pub fn datetime_from_serial(sys: DateSystem, serial: f64) -> Option<DateTime> {
    if !serial.is_finite() || serial < 0.0 {
        return None;
    }
    let ms_total = (serial * 86_400_000.0).round() as i64;
    let mut day = ms_total.div_euclid(86_400_000);
    let ms = ms_total.rem_euclid(86_400_000);
    let (year, month, d) = match sys {
        DateSystem::D1900 => {
            if day == 0 {
                // シリアル値 0 は Excel では 1900/1/0
                (1900, 1, 0)
            } else if day == 60 {
                (1900, 2, 29)
            } else {
                if day < 60 {
                    day += 1;
                }
                civil_from_days(day + UNIX_1899_12_30)
            }
        }
        DateSystem::D1904 => civil_from_days(day + UNIX_1904_01_01),
    };
    if year > 9999 {
        return None;
    }
    let secs = ms / 1000;
    Some(DateTime {
        year,
        month,
        day: d,
        hour: (secs / 3600) as u32,
        minute: (secs / 60 % 60) as u32,
        second: (secs % 60) as u32,
        milli: (ms % 1000) as u32,
    })
}

/// 曜日（0 = 日曜）。1900 年の日付では Excel と同じく 1900/1/1 を日曜として数える（実際は月曜。
/// 架空の 2 月 29 日があるので、3 月 1 日からは実際の曜日と合う）。
pub fn weekday(sys: DateSystem, serial: f64) -> Option<u32> {
    if !serial.is_finite() || serial < 0.0 {
        return None;
    }
    let day = serial.floor() as i64;
    Some(match sys {
        DateSystem::D1900 => (day - 1).rem_euclid(7),
        // 1904/1/1 は金曜
        DateSystem::D1904 => (day + 5).rem_euclid(7),
    } as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_serials() {
        let s = |y, m, d| serial_from_date(DateSystem::D1900, y, m, d);
        assert_eq!(s(1900, 1, 1), Some(1.0));
        assert_eq!(s(1900, 2, 28), Some(59.0));
        assert_eq!(s(1900, 2, 29), Some(60.0));
        assert_eq!(s(1900, 3, 1), Some(61.0));
        assert_eq!(s(2026, 10, 7), Some(46302.0));
        assert_eq!(s(9999, 12, 31), Some(2_958_465.0));
        assert_eq!(s(1899, 12, 31), None);
        assert_eq!(s(2026, 2, 29), None);
        assert_eq!(serial_from_date(DateSystem::D1904, 1904, 1, 1), Some(0.0));
        assert_eq!(
            serial_from_date(DateSystem::D1904, 2026, 10, 7),
            Some(46302.0 - 1462.0)
        );
        for serial in [1.0, 59.0, 60.0, 61.0, 46302.0, 2_958_465.0] {
            let dt = datetime_from_serial(DateSystem::D1900, serial).unwrap();
            assert_eq!(s(dt.year, dt.month, dt.day), Some(serial), "{dt:?}");
        }
        let dt = datetime_from_serial(DateSystem::D1900, 46302.75).unwrap();
        assert_eq!((dt.day, dt.hour, dt.minute), (7, 18, 0));
        // 23:59:59.9996 はミリ秒に丸めて翌日
        let dt = datetime_from_serial(DateSystem::D1900, 46302.0 + 0.999_999_999_5).unwrap();
        assert_eq!((dt.day, dt.hour), (8, 0));
        assert_eq!(weekday(DateSystem::D1900, 46302.0), Some(3)); // 2026-10-07 は水曜
        assert_eq!(weekday(DateSystem::D1900, 1.0), Some(0)); // Excel では 1900/1/1 は日曜
        assert_eq!(weekday(DateSystem::D1904, 46302.0 - 1462.0), Some(3));
    }
}
