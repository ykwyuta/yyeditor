//! Cookie の編集（開発者用。19 章 4.5）の画面の部品: 入力の確かめ、期限の日時の読み書き、一覧の説明。
//! WebView2 の CookieManager との受け渡しは画面の側で行う。

/// SameSite。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SameSite {
    None,
    #[default]
    Lax,
    Strict,
}

impl SameSite {
    pub const ALL: [SameSite; 3] = [SameSite::None, SameSite::Lax, SameSite::Strict];

    pub fn label(self) -> &'static str {
        match self {
            SameSite::None => "None",
            SameSite::Lax => "Lax",
            SameSite::Strict => "Strict",
        }
    }
}

/// Cookie 1 つの欄。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CookieFields {
    pub name: String,
    pub value: String,
    /// ドメイン（`.example.com` のように先頭に点があればサブドメインにも送る）
    pub domain: String,
    pub path: String,
    /// 期限（UNIX 時間の秒）。`None` はセッション Cookie（ブラウザを閉じると消える）
    pub expires: Option<f64>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: SameSite,
}

impl CookieFields {
    /// 確かめる（WebView2 に渡す前）。
    pub fn validate(&self) -> Result<(), String> {
        let n = self.name.trim();
        if n.is_empty() {
            return Err("Cookie の名前を入れてください".into());
        }
        if n.chars()
            .any(|c| c.is_control() || c.is_whitespace() || "()<>@,;:\\\"/[]?={}".contains(c))
        {
            return Err(format!(
                "Cookie の名前「{n}」に使えない文字（空白・; = , など）があります"
            ));
        }
        if self.value.chars().any(|c| c.is_control() || c == ';') {
            return Err("値に ; や改行は使えません".into());
        }
        let d = self.domain.trim().trim_start_matches('.');
        if d.is_empty()
            || !d.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']')
            })
        {
            return Err(format!(
                "ドメイン「{}」が正しくありません",
                self.domain.trim()
            ));
        }
        if !self.path.trim().starts_with('/') {
            return Err("パスは / で始めてください（例: /）".into());
        }
        if self.same_site == SameSite::None && !self.secure {
            return Err(
                "SameSite を None にするときは Secure にしてください（ブラウザが受け付けません）"
                    .into(),
            );
        }
        Ok(())
    }

    /// 一覧の 1 行（`name = value　—　.example.com /　期限 2026-10-09 14:23　HttpOnly Secure Lax`）。
    /// `fmt_time` は UNIX 時間を手元の時刻にする関数。
    pub fn describe(&self, fmt_time: impl Fn(f64) -> String) -> String {
        let mut v: String = self.value.chars().take(60).collect();
        if self.value.chars().count() > 60 {
            v.push('…');
        }
        let exp = match self.expires {
            None => "セッション".to_owned(),
            Some(t) => format!("期限 {}", fmt_time(t)),
        };
        let mut flags = Vec::new();
        if self.http_only {
            flags.push("HttpOnly");
        }
        if self.secure {
            flags.push("Secure");
        }
        flags.push(self.same_site.label());
        format!(
            "{} = {v}　—　{} {}　{exp}　{}",
            self.name,
            self.domain,
            self.path,
            flags.join(" ")
        )
    }
}

/// 日時（年・月・日・時・分・秒）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// `2026-10-09 14:23`・`2026/10/09 14:23:05`・`2026-10-09` を読む（時刻を省けば 0:00）。
pub fn parse_datetime(s: &str) -> Option<DateTime> {
    let s = s.trim();
    let (date, time) = match s.split_once([' ', 'T']) {
        Some((d, t)) => (d, t.trim()),
        None => (s, ""),
    };
    let ds: Vec<&str> = date.split(['-', '/']).collect();
    if ds.len() != 3 {
        return None;
    }
    let year: u16 = ds[0].parse().ok()?;
    let month: u16 = ds[1].parse().ok()?;
    let day: u16 = ds[2].parse().ok()?;
    let (hour, minute, second) = if time.is_empty() {
        (0, 0, 0)
    } else {
        let ts: Vec<&str> = time.split(':').collect();
        if !(2..=3).contains(&ts.len()) {
            return None;
        }
        (
            ts[0].parse().ok()?,
            ts[1].parse().ok()?,
            ts.get(2).map_or(Some(0), |x| x.parse().ok())?,
        )
    };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1970..=9999).contains(&year)
        || day == 0
        || day > days
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

impl DateTime {
    pub fn format(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie() -> CookieFields {
        CookieFields {
            name: "session_id".into(),
            value: "abc123".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            expires: Some(1_800_000_000.0),
            http_only: true,
            secure: true,
            same_site: SameSite::Lax,
        }
    }

    #[test]
    fn validates_cookies() {
        assert!(cookie().validate().is_ok());
        let bad = |f: &dyn Fn(&mut CookieFields)| {
            let mut c = cookie();
            f(&mut c);
            c.validate().is_err()
        };
        assert!(bad(&|c| c.name.clear()));
        assert!(bad(&|c| c.name = "a b".into()));
        assert!(bad(&|c| c.name = "a=b".into()));
        assert!(bad(&|c| c.value = "x;y".into()));
        assert!(bad(&|c| c.value = "x\ny".into()));
        assert!(bad(&|c| c.domain = "".into()));
        assert!(bad(&|c| c.domain = "exa mple.com".into()));
        assert!(bad(&|c| c.path = "x".into()));
        assert!(bad(&|c| {
            c.same_site = SameSite::None;
            c.secure = false;
        }));
        let mut ok = cookie();
        ok.domain = "localhost:8080".into();
        ok.value = String::new();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn describes_cookies() {
        let s = cookie().describe(|_| "2027-01-15 08:00".into());
        assert_eq!(
            s,
            "session_id = abc123　—　.example.com /　期限 2027-01-15 08:00　HttpOnly Secure Lax"
        );
        let mut c = cookie();
        c.expires = None;
        c.http_only = false;
        c.value = "v".repeat(100);
        let s = c.describe(|_| unreachable!());
        assert!(s.contains("セッション"));
        assert!(s.contains('…'));
        assert!(!s.contains("HttpOnly"));
    }

    #[test]
    fn parses_datetimes() {
        let d = parse_datetime("2026-10-09 14:23").unwrap();
        assert_eq!(
            (d.year, d.month, d.day, d.hour, d.minute, d.second),
            (2026, 10, 9, 14, 23, 0)
        );
        assert_eq!(d.format(), "2026-10-09 14:23");
        assert_eq!(parse_datetime("2026/10/09 14:23:05").unwrap().second, 5);
        assert_eq!(parse_datetime("2026-10-09").unwrap().hour, 0);
        assert!(parse_datetime("2028-02-29 00:00").is_some());
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-10-09 24:00",
            "2026-10",
            "abc",
            "1969-01-01",
            "2026-10-09 1:2:3:4",
        ] {
            assert!(parse_datetime(bad).is_none(), "{bad}");
        }
    }
}
